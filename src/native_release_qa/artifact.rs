use super::contract::{
    CheckEvidence, EVIDENCE_SCHEMA, ManualAttestation, NativeQaMode, NativeQaRequest,
    NativeReleaseEvidence, NativeVerdict, QaSubject, QaSubjectBinding,
};
use super::macos_probe::{capture_display_topology, command_stdout, detect_capabilities};
use super::policy::{
    attestation_template, evaluate_verdict, mixed_scale_topology, popup_placement_matrix,
    validate_attestation,
};
use serde::Serialize;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "visual-qa")]
pub fn run(request: NativeQaRequest) -> Result<(), String> {
    prepare_output(&request.output_root)?;

    let now = now_unix();
    let mut topology = capture_display_topology();
    topology.sort_by(|left, right| {
        left.frame
            .min_x
            .total_cmp(&right.frame.min_x)
            .then(left.frame.min_y.total_cmp(&right.frame.min_y))
            .then(left.id.cmp(&right.id))
    });
    let fingerprint = topology_fingerprint(&topology)?;
    let subject = QaSubject {
        commit: current_commit(),
        binary_blake3: current_binary_digest()?,
        worktree_clean: worktree_clean(),
    };
    let binding = QaSubjectBinding {
        commit: subject.commit.clone(),
        binary_blake3: subject.binary_blake3.clone(),
        topology_fingerprint: fingerprint.clone(),
    };
    write_json(
        &request.output_root.join("manual-attestation.template.json"),
        &attestation_template(&binding),
    )?;

    let capabilities = detect_capabilities(&topology);
    let renderer = crate::native_menu::native_qa_renderer_check().unwrap_or_else(|error| {
        super::contract::NativeMenuRendererEvidence {
            check: CheckEvidence::failed("native_menu_renderer", error),
            popup_size: None,
        }
    });
    let checks = vec![
        renderer.check,
        popup_placement_matrix(&topology, renderer.popup_size),
        mixed_scale_topology(&topology),
    ];
    let attestation = load_attestation(request.attestation.as_deref(), &binding, now);
    let (verdict, reasons) = evaluate_verdict(&capabilities, &subject, &checks, &attestation);
    let evidence = NativeReleaseEvidence {
        schema: EVIDENCE_SCHEMA,
        mode: request.mode,
        generated_at_unix: now,
        subject,
        macos_build: command_stdout("sw_vers", &["-buildVersion"])
            .unwrap_or_else(|| "unavailable".to_string()),
        architecture: std::env::consts::ARCH.to_string(),
        capabilities,
        topology_fingerprint: fingerprint,
        topology,
        automated_checks: checks,
        manual_attestation: attestation,
        verdict,
        reasons,
    };
    write_json(&request.output_root.join("evidence.json"), &evidence)?;

    if request.mode == NativeQaMode::Strict && verdict != NativeVerdict::Passed {
        return Err(format!(
            "native release QA is {verdict:?}; see {}",
            request.output_root.join("evidence.json").display()
        ));
    }
    Ok(())
}

fn prepare_output(output_root: &Path) -> Result<(), String> {
    if output_root.exists() && !output_root.is_dir() {
        return Err(format!(
            "native QA output is not a directory: {}",
            output_root.display()
        ));
    }
    std::fs::create_dir_all(output_root)
        .map_err(|error| format!("could not create native QA output: {error}"))?;
    for name in ["evidence.json", "manual-attestation.template.json"] {
        let path = output_root.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "could not remove stale native QA artifact {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn topology_fingerprint(displays: &[super::contract::DisplayTopology]) -> Result<String, String> {
    let encoded = serde_json::to_vec(displays)
        .map_err(|error| format!("could not serialize display topology: {error}"))?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

fn current_binary_digest() -> Result<String, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the current executable: {error}"))?;
    let mut file = File::open(&executable)
        .map_err(|error| format!("could not open {}: {error}", executable.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not hash {}: {error}", executable.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn current_commit() -> String {
    std::env::var("COMMANDER_QA_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("GITHUB_SHA")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| command_stdout("git", &["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string())
}

fn worktree_clean() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .is_some_and(|output| output.status.success() && output.stdout.is_empty())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn load_attestation(path: Option<&Path>, expected: &QaSubjectBinding, now: u64) -> CheckEvidence {
    let Some(path) = path else {
        return CheckEvidence::blocked("manual_attestation", "no attestation supplied");
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return CheckEvidence::blocked(
                "manual_attestation",
                format!("could not read {}: {error}", path.display()),
            );
        }
    };
    match serde_json::from_slice::<ManualAttestation>(&bytes) {
        Ok(attestation) => validate_attestation(&attestation, expected, now),
        Err(error) => CheckEvidence::failed(
            "manual_attestation",
            format!("invalid {}: {error}", path.display()),
        ),
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("could not serialize {}: {error}", path.display()))?;
    std::fs::write(path, bytes)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}
