use super::contract::{
    CheckEvidence, EVIDENCE_SCHEMA, ManualAttestation, NativeQaMode, NativeQaRequest,
    NativeReleaseEvidence, NativeVerdict, QaSubjectBinding,
};
use super::identity::{capture_subject, topology_fingerprint};
use super::macos_probe::{capture_display_topology, command_stdout, detect_capabilities};
use super::policy::{
    attestation_template, evaluate_verdict, mixed_scale_topology, popup_placement_matrix,
    validate_attestation,
};
use super::secure_artifact::{ArtifactDirectory, AttestationReadError, read_attestation};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run(request: NativeQaRequest) -> Result<(), String> {
    let output = ArtifactDirectory::prepare(&request.output_root)?;
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
    let subject = capture_subject();
    let binding = QaSubjectBinding {
        commit: subject.commit.clone(),
        binary_blake3: subject.binary_blake3.clone(),
        topology_fingerprint: fingerprint.clone(),
    };
    output.write_json(
        "manual-attestation.template.json",
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
        macos_build: command_stdout("/usr/bin/sw_vers", &["-buildVersion"])
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
    output.write_json("evidence.json", &evidence)?;

    if request.mode == NativeQaMode::Strict && verdict != NativeVerdict::Passed {
        return Err(format!(
            "native release QA is {verdict:?}; see {}",
            output.path("evidence.json").display()
        ));
    }
    Ok(())
}

fn load_attestation(path: Option<&Path>, expected: &QaSubjectBinding, now: u64) -> CheckEvidence {
    let Some(path) = path else {
        return CheckEvidence::blocked("manual_attestation", "no attestation supplied");
    };
    let bytes = match read_attestation(path) {
        Ok(bytes) => bytes,
        Err(AttestationReadError::Missing(detail)) => {
            return CheckEvidence::blocked("manual_attestation", detail);
        }
        Err(AttestationReadError::Invalid(detail)) => {
            return CheckEvidence::failed("manual_attestation", detail);
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

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
