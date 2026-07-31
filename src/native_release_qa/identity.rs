use super::contract::{DisplayTopology, QaSubject, ScreenRect};
use serde::Serialize;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::process::Command;

pub(super) fn topology_fingerprint(displays: &[DisplayTopology]) -> Result<String, String> {
    #[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
    struct CanonicalDisplay {
        id: String,
        main: bool,
        frame: [i64; 4],
        visible_frame: [i64; 4],
        backing_scale: i64,
    }

    fn fixed(value: f64, label: &str) -> Result<i64, String> {
        if !value.is_finite() || value.abs() > 1_000_000_000.0 {
            return Err(format!("invalid display {label}: {value}"));
        }
        Ok((value * 1_000_000.0).round() as i64)
    }

    fn rect(value: ScreenRect, label: &str) -> Result<[i64; 4], String> {
        Ok([
            fixed(value.min_x, label)?,
            fixed(value.min_y, label)?,
            fixed(value.max_x, label)?,
            fixed(value.max_y, label)?,
        ])
    }

    if !displays.is_empty() && displays.iter().filter(|display| display.main).count() != 1 {
        return Err("display topology must contain exactly one main display".to_string());
    }
    let mut ids = HashSet::new();
    let mut canonical = Vec::with_capacity(displays.len());
    for display in displays {
        if display.id.trim().is_empty() || !ids.insert(display.id.clone()) {
            return Err(format!(
                "display id is empty or duplicated: {:?}",
                display.id
            ));
        }
        if !display.frame.is_valid()
            || !display.visible_frame.is_valid()
            || !display.frame.contains_rect(display.visible_frame)
            || !display.backing_scale.is_finite()
            || display.backing_scale <= 0.0
        {
            return Err(format!("invalid display topology entry: {}", display.id));
        }
        canonical.push(CanonicalDisplay {
            id: display.id.clone(),
            main: display.main,
            frame: rect(display.frame, "frame")?,
            visible_frame: rect(display.visible_frame, "visible frame")?,
            backing_scale: fixed(display.backing_scale, "backing scale")?,
        });
    }
    canonical.sort();
    let encoded = serde_json::to_vec(&("commander.display-topology.v1", canonical))
        .map_err(|error| format!("could not serialize display topology: {error}"))?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

pub(super) fn capture_subject() -> QaSubject {
    let commit = env!("COMMANDER_BUILD_GIT_COMMIT").to_string();
    let build_clean = env!("COMMANDER_BUILD_GIT_DIRTY") == "false";
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let git = if cfg!(target_os = "macos") {
        "/usr/bin/git"
    } else {
        "git"
    };
    let checkout_commit = Command::new(git)
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string());
    let checkout_clean = Command::new(git)
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
        .ok()
        .is_some_and(|output| output.status.success() && output.stdout.is_empty());
    let (binary_blake3, binary_identity_verified) =
        current_binary_digest().unwrap_or_else(|_| ("unavailable".to_string(), false));
    QaSubject {
        worktree_clean: exact_build_checkout(
            &commit,
            build_clean,
            checkout_commit.as_deref(),
            checkout_clean,
        ),
        commit,
        binary_blake3,
        binary_identity_verified,
    }
}

fn current_binary_digest() -> Result<(String, bool), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the current executable: {error}"))?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(&executable)
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
    let identity_verified = loaded_executable_matches(&executable, &file).unwrap_or(false);
    Ok((hasher.finalize().to_hex().to_string(), identity_verified))
}

#[cfg(target_os = "macos")]
fn loaded_executable_matches(executable: &Path, opened: &File) -> Result<bool, String> {
    let opened_metadata = opened
        .metadata()
        .map_err(|error| format!("could not inspect the opened executable: {error}"))?;
    let running_cdhash = running_code_directory_hash()?;
    let disk_cdhash = disk_code_directory_hash(executable)?;
    let verified = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--all-architectures", "--"])
        .arg(executable)
        .output()
        .map_err(|error| format!("could not verify the executable code signature: {error}"))?
        .status
        .success();
    let current_metadata = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(executable)
        .and_then(|file| file.metadata())
        .map_err(|error| format!("could not re-open the current executable: {error}"))?;
    Ok(verified
        && running_cdhash == disk_cdhash
        && same_file_version(&opened_metadata, &current_metadata))
}

#[cfg(not(target_os = "macos"))]
fn loaded_executable_matches(_executable: &Path, _opened: &File) -> Result<bool, String> {
    Ok(false)
}

#[cfg(target_os = "macos")]
fn running_code_directory_hash() -> Result<[u8; 20], String> {
    const CS_OPS_CDHASH: u32 = 5;
    unsafe extern "C" {
        fn csops(
            pid: libc::pid_t,
            operations: u32,
            user_address: *mut libc::c_void,
            user_size: libc::size_t,
        ) -> libc::c_int;
    }

    let mut hash = [0_u8; 20];
    let result = unsafe {
        csops(
            libc::getpid(),
            CS_OPS_CDHASH,
            hash.as_mut_ptr().cast(),
            hash.len(),
        )
    };
    if result == 0 {
        Ok(hash)
    } else {
        Err(format!(
            "could not read the running executable CDHash: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(target_os = "macos")]
fn disk_code_directory_hash(executable: &Path) -> Result<[u8; 20], String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-d", "--verbose=4", "--"])
        .arg(executable)
        .output()
        .map_err(|error| format!("could not inspect the executable code signature: {error}"))?;
    if !output.status.success() {
        return Err("codesign could not inspect the current executable".to_string());
    }
    parse_codesign_cdhash(&output.stderr)
        .or_else(|| parse_codesign_cdhash(&output.stdout))
        .ok_or_else(|| "codesign did not report one valid executable CDHash".to_string())
}

#[cfg(target_os = "macos")]
fn parse_codesign_cdhash(output: &[u8]) -> Option<[u8; 20]> {
    let output = std::str::from_utf8(output).ok()?;
    let mut values = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix("CDHash=").and_then(decode_cdhash));
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

#[cfg(target_os = "macos")]
fn decode_cdhash(value: &str) -> Option<[u8; 20]> {
    fn nibble(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            _ => None,
        }
    }

    if value.len() != 40 {
        return None;
    }
    let mut decoded = [0_u8; 20];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(decoded)
}

#[cfg(unix)]
fn same_file_version(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

fn exact_build_checkout(
    build_commit: &str,
    build_clean: bool,
    checkout_commit: Option<&str>,
    checkout_clean: bool,
) -> bool {
    build_clean && checkout_clean && checkout_commit == Some(build_commit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topology() -> Vec<DisplayTopology> {
        vec![
            DisplayTopology {
                id: "display-2".to_string(),
                main: false,
                frame: ScreenRect::new(-1920.0, -0.0, 1920.0, 1080.0),
                visible_frame: ScreenRect::new(-1920.0, 24.0, 1920.0, 1056.0),
                backing_scale: 1.0,
            },
            DisplayTopology {
                id: "display-1".to_string(),
                main: true,
                frame: ScreenRect::new(0.0, 0.0, 1512.0, 982.0),
                visible_frame: ScreenRect::new(0.0, 38.0, 1512.0, 944.0),
                backing_scale: 2.0,
            },
        ]
    }

    #[test]
    fn fingerprint_is_order_independent_and_normalizes_negative_zero() {
        let mut left = topology();
        let mut right = left.iter().cloned().rev().collect::<Vec<_>>();
        right[1].frame.min_y = 0.0;
        assert_eq!(
            topology_fingerprint(&left).unwrap(),
            topology_fingerprint(&right).unwrap()
        );
        left[1].id = left[0].id.clone();
        assert!(topology_fingerprint(&left).is_err());
    }

    #[test]
    fn build_identity_rejects_spoofed_or_stale_checkout_state() {
        let commit = "a".repeat(40);
        assert!(exact_build_checkout(&commit, true, Some(&commit), true));
        assert!(!exact_build_checkout(
            &commit,
            true,
            Some(&"b".repeat(40)),
            true
        ));
        // A dirty-build binary remains blocked after its source edit is
        // reverted and runtime git becomes clean.
        assert!(!exact_build_checkout(&commit, false, Some(&commit), true));
        assert!(!exact_build_checkout(&commit, true, Some(&commit), false));
        assert!(!exact_build_checkout(&commit, true, None, true));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn codesign_cdhash_parser_is_exact_and_rejects_duplicates() {
        let expected = [0xab; 20];
        let line = format!(
            "CandidateCDHash sha256=ignored\nCDHash={}\n",
            "ab".repeat(20)
        );
        assert_eq!(parse_codesign_cdhash(line.as_bytes()), Some(expected));
        assert!(
            parse_codesign_cdhash(format!("{line}CDHash={}\n", "ab".repeat(20)).as_bytes())
                .is_none()
        );
        assert!(parse_codesign_cdhash(b"CDHash=ABCDEF").is_none());
    }
}
