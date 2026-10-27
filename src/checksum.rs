//! Fresh, cancellable checksum calculation and explicit expected-digest comparison.
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumRow {
    pub path: PathBuf,
    pub content_hash: Option<u64>,
    pub blake3_hex: Option<String>,
    pub sha256_hex: Option<String>,
    pub error: Option<String>,
    pub skipped: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumReport {
    pub rows: Vec<ChecksumRow>,
    pub cancelled: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpectedMatch {
    Empty,
    Invalid,
    Unavailable,
    Match,
    Mismatch,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Algorithm {
    #[default]
    Sha256,
    Blake3,
}
impl Algorithm {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sha256 => "SHA-256",
            Self::Blake3 => "BLAKE3",
        }
    }
    fn digest(self, row: &ChecksumRow) -> Option<&str> {
        let value = match self {
            Self::Sha256 => row.sha256_hex.as_deref(),
            Self::Blake3 => row.blake3_hex.as_deref(),
        };
        value.filter(|value| valid_digest(value))
    }
}
impl ChecksumReport {
    /// A single completed, valid digest; never guess an algorithm from its length.
    pub fn single_digest(&self, algorithm: Algorithm) -> Option<&str> {
        if self.cancelled || self.rows.len() != 1 {
            return None;
        }
        let row = &self.rows[0];
        if row.error.is_some() || row.skipped.is_some() {
            return None;
        }
        algorithm.digest(row)
    }
    pub fn successful(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.error.is_none() && row.skipped.is_none() && row.sha256_hex.is_some())
            .count()
    }
    pub fn summary_line(&self) -> String {
        let skipped = self.rows.iter().filter(|row| row.skipped.is_some()).count();
        let failed = self.rows.iter().filter(|row| row.error.is_some()).count();
        let ok = self.successful();
        let mut line = format!("Calculated {ok} file(s)");
        if failed > 0 {
            line.push_str(&format!(", {failed} failed"));
        }
        if skipped > 0 {
            line.push_str(&format!(", {skipped} skipped"));
        }
        let unavailable = self.rows.len().saturating_sub(ok + skipped + failed);
        if unavailable > 0 {
            line.push_str(&format!(", {unavailable} unavailable"));
        }
        if self.cancelled {
            line.push_str(", cancelled");
        }
        line
    }
    pub fn detailed_text(&self) -> String {
        self.rows
            .iter()
            .map(|row| {
                let path = row.path.to_string_lossy().escape_debug().to_string();
                if let Some(error) = &row.error {
                    return format!("{path}: ERROR {error}");
                }
                if let Some(reason) = &row.skipped {
                    return format!("{path}: skipped ({reason})");
                }
                format!(
                    "{path}: content={} blake3={} sha256={}",
                    row.content_hash
                        .map(|hash| format!("{hash:016x}"))
                        .unwrap_or_else(|| "-".into()),
                    row.blake3_hex.as_deref().unwrap_or("-"),
                    row.sha256_hex.as_deref().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[cfg(test)]
    pub fn compare_expected(&self, expected: &str) -> ExpectedMatch {
        self.compare_expected_algorithm(expected, Algorithm::Sha256)
    }
    pub fn compare_expected_algorithm(
        &self,
        expected: &str,
        algorithm: Algorithm,
    ) -> ExpectedMatch {
        let expected = expected.trim();
        if expected.is_empty() {
            return ExpectedMatch::Empty;
        }
        if !valid_digest(expected) {
            return ExpectedMatch::Invalid;
        }
        let Some(actual) = self.single_digest(algorithm) else {
            return ExpectedMatch::Unavailable;
        };
        if actual.eq_ignore_ascii_case(expected) {
            ExpectedMatch::Match
        } else {
            ExpectedMatch::Mismatch
        }
    }
    /// GNU untagged binary-mode format, with its explicit filename escaping.
    /// Only successfully completed files are exported; no hashes of skipped rows.
    pub fn sha256_list(&self) -> Result<String, String> {
        let mut lines = Vec::new();
        for row in &self.rows {
            if row.error.is_some() || row.skipped.is_some() {
                continue;
            }
            let Some(hash) = row
                .sha256_hex
                .as_deref()
                .filter(|value| valid_digest(value))
            else {
                continue;
            };
            let path = row
                .path
                .to_str()
                .ok_or("A path is not UTF-8; it cannot be copied losslessly as a checksum list")?;
            if !row.path.is_absolute() || path.contains('\0') {
                return Err("Checksum lists require absolute paths without NUL characters".into());
            }
            let escaped = path.contains(['\\', '\n', '\r']);
            let path = path
                .replace('\\', "\\\\")
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            lines.push(format!(
                "{}{hash} *{path}\n",
                if escaped { "\\" } else { "" }
            ));
        }
        if lines.is_empty() {
            return Err("No completed SHA-256 checksums to copy".into());
        }
        Ok(lines.concat())
    }
}
fn valid_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
pub fn verify_paths(paths: impl IntoIterator<Item = PathBuf>) -> ChecksumReport {
    ChecksumReport {
        rows: paths
            .into_iter()
            .map(|path| verify_one(&path, &|| false))
            .collect(),
        cancelled: false,
    }
}
pub fn verify_paths_cancellable(
    paths: Vec<PathBuf>,
    cancelled: &crate::workload::CancellationToken,
    completed: &std::sync::atomic::AtomicUsize,
) -> ChecksumReport {
    let mut report = ChecksumReport::default();
    for path in paths {
        if cancelled.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let row = verify_one(&path, &|| cancelled.is_cancelled());
        if cancelled.is_cancelled() {
            report.cancelled = true;
            break;
        }
        report.rows.push(row);
        completed.fetch_add(1, std::sync::atomic::Ordering::Release);
    }
    report
}
fn verify_one(path: &Path, cancelled: &dyn Fn() -> bool) -> ChecksumRow {
    let mut row = ChecksumRow {
        path: path.into(),
        ..Default::default()
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) => {
            row.error = Some(error.to_string());
            return row;
        }
    };
    if meta.is_dir() {
        row.skipped = Some("directories have no content checksum".into());
        return row;
    }
    if meta.file_type().is_symlink() {
        row.skipped = Some("symlinks are not hashed".into());
        return row;
    }
    if !meta.is_file() {
        row.skipped = Some("only regular files are hashed".into());
        return row;
    }
    match digests(path, &meta, cancelled) {
        Ok((blake, sha)) => {
            row.content_hash = Some(content_hash_of(&blake));
            row.blake3_hex = Some(blake3::Hash::from(blake).to_hex().to_string());
            row.sha256_hex = Some(sha);
        }
        Err(error) => row.error = Some(error.to_string()),
    }
    row
}
fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "File changed while checksums were calculated; refresh and retry",
    )
}
fn digests(
    path: &Path,
    expected: &std::fs::Metadata,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<([u8; 32], String)> {
    use crate::path_identity::PathIdentity;
    let cancel = || {
        if cancelled() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Checksum calculation cancelled",
            ))
        } else {
            Ok(())
        }
    };
    cancel()?;
    let before = PathIdentity::from_metadata(path.into(), expected);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY);
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || !before.same_shallow_binding(&PathIdentity::from_metadata(path.into(), &opened))
    {
        return Err(changed());
    }
    let mut remaining = opened.len();
    let mut buffer = [0u8; 64 * 1024];
    let mut blake = blake3::Hasher::new();
    let mut sha = Sha256::new();
    loop {
        cancel()?;
        let limit = remaining.saturating_add(1).min(buffer.len() as u64) as usize;
        let count = match file.read(&mut buffer[..limit]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        if count as u64 > remaining {
            return Err(changed());
        }
        remaining -= count as u64;
        blake.update(&buffer[..count]);
        sha.update(&buffer[..count]);
    }
    let after = file.metadata()?;
    if remaining != 0
        || !before.same_shallow_binding(&PathIdentity::from_metadata(path.into(), &after))
        || !before.same_shallow_binding(&PathIdentity::observe(path)?)
    {
        return Err(changed());
    }
    cancel()?;
    let sha = sha.finalize();
    let hex = sha.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok((*blake.finalize().as_bytes(), hex))
}
fn content_hash_of(blake3: &[u8; 32]) -> u64 {
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&blake3[..8]);
    u64::from_be_bytes(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    #[test]
    fn explicit_algorithm_comparison_never_guesses_from_equal_digest_lengths() {
        let temp = TempDir::new();
        temp.file("abc", "abc");
        let report = verify_paths([temp.path().join("abc")]);
        let blake = "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85";
        assert_eq!(report.single_digest(Algorithm::Blake3), Some(blake));
        assert_eq!(report.single_digest(Algorithm::Sha256), Some(ABC_SHA));
        assert_eq!(
            report.compare_expected_algorithm(
                &format!(" {} ", blake.to_uppercase()),
                Algorithm::Blake3
            ),
            ExpectedMatch::Match
        );
        assert_eq!(
            report.compare_expected_algorithm(ABC_SHA, Algorithm::Blake3),
            ExpectedMatch::Mismatch
        );
        assert_eq!(
            report.compare_expected_algorithm(blake, Algorithm::Sha256),
            ExpectedMatch::Mismatch
        );
        assert_eq!(
            report.compare_expected_algorithm("bad", Algorithm::Blake3),
            ExpectedMatch::Invalid
        );
        assert_eq!(
            report.compare_expected_algorithm("", Algorithm::Blake3),
            ExpectedMatch::Empty
        );
    }

    #[test]
    fn digest_copy_and_comparison_reject_incomplete_or_ambiguous_reports() {
        let row = ChecksumRow {
            blake3_hex: Some("a".repeat(64)),
            sha256_hex: Some("b".repeat(64)),
            ..Default::default()
        };
        let mut report = ChecksumReport {
            rows: vec![row.clone()],
            cancelled: false,
        };
        for algorithm in [Algorithm::Sha256, Algorithm::Blake3] {
            assert!(report.single_digest(algorithm).is_some());
            report.cancelled = true;
            assert!(report.single_digest(algorithm).is_none());
            report.cancelled = false;
        }
        for bad_row in [
            ChecksumRow {
                error: Some("changed".into()),
                ..row.clone()
            },
            ChecksumRow {
                skipped: Some("folder".into()),
                ..row.clone()
            },
            ChecksumRow {
                blake3_hex: Some("z".repeat(64)),
                sha256_hex: None,
                ..row.clone()
            },
        ] {
            report.rows = vec![bad_row];
            for algorithm in [Algorithm::Sha256, Algorithm::Blake3] {
                assert!(report.single_digest(algorithm).is_none());
                assert_eq!(
                    report.compare_expected_algorithm(&"a".repeat(64), algorithm),
                    ExpectedMatch::Unavailable
                );
            }
        }
        report.rows = vec![row.clone(), row];
        assert!(report.single_digest(Algorithm::Blake3).is_none());
        report.rows.clear();
        assert!(report.single_digest(Algorithm::Blake3).is_none());
    }

    #[test]
    fn sha256_known_vectors_and_expected_digest_comparison() {
        let temp = TempDir::new();
        let abc = temp.file("abc", "abc");
        let empty = temp.file("empty", "");
        let report = verify_paths([abc]);
        assert_eq!(report.rows[0].sha256_hex.as_deref(), Some(ABC_SHA));
        assert_eq!(
            report.compare_expected(&format!(" {} ", ABC_SHA.to_uppercase())),
            ExpectedMatch::Match
        );
        assert_eq!(
            report.compare_expected(&"0".repeat(64)),
            ExpectedMatch::Mismatch
        );
        assert_eq!(report.compare_expected("bad"), ExpectedMatch::Invalid);
        assert_eq!(
            report.compare_expected(&"g".repeat(64)),
            ExpectedMatch::Invalid
        );
        assert_eq!(report.compare_expected(""), ExpectedMatch::Empty);
        assert_eq!(
            verify_paths([empty]).rows[0].sha256_hex.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        let million = temp.file("million", &"a".repeat(1_000_000));
        assert_eq!(
            verify_paths([million]).rows[0].sha256_hex.as_deref(),
            Some("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0")
        );
        let cancelled = ChecksumReport {
            cancelled: true,
            ..report.clone()
        };
        assert_eq!(
            cancelled.compare_expected(ABC_SHA),
            ExpectedMatch::Unavailable
        );
    }

    #[test]
    fn cancellation_and_changed_file_never_publish_partial_digests() {
        let temp = TempDir::new();
        let path = temp.file("large", &"a".repeat(200_000));
        let ticks = std::cell::Cell::new(0);
        let row = verify_one(&path, &|| {
            ticks.set(ticks.get() + 1);
            ticks.get() >= 3
        });
        assert!(row.error.as_deref().unwrap().contains("cancelled"));
        assert!(row.sha256_hex.is_none() && row.blake3_hex.is_none());
        let ticks = std::cell::Cell::new(0);
        let row = verify_one(&path, &|| {
            ticks.set(ticks.get() + 1);
            if ticks.get() == 3 {
                std::fs::rename(&path, temp.path().join("original")).unwrap();
                std::fs::write(&path, "new file").unwrap();
            }
            false
        });
        assert!(row.error.as_deref().unwrap().contains("changed"));
        assert!(row.sha256_hex.is_none() && row.blake3_hex.is_none());
    }

    #[test]
    fn growing_and_truncated_files_never_publish_digests() {
        use std::io::Write;
        let temp = TempDir::new();
        for grow in [true, false] {
            let path = temp.file(
                if grow { "growing" } else { "truncated" },
                &"a".repeat(200_000),
            );
            let ticks = std::cell::Cell::new(0);
            let row = verify_one(&path, &|| {
                ticks.set(ticks.get() + 1);
                if ticks.get() == 3 {
                    if grow {
                        std::fs::OpenOptions::new()
                            .append(true)
                            .open(&path)
                            .unwrap()
                            .write_all(b"extra")
                            .unwrap();
                    } else {
                        std::fs::OpenOptions::new()
                            .write(true)
                            .open(&path)
                            .unwrap()
                            .set_len(0)
                            .unwrap();
                    }
                }
                false
            });
            assert!(row.error.as_deref().unwrap().contains("changed"));
            assert!(row.sha256_hex.is_none() && row.blake3_hex.is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn checksum_open_rejects_symlink_substitution_and_special_files() {
        let temp = TempDir::new();
        let path = temp.file("selected", "abc");
        let target = temp.file("target", "secret");
        let changed = std::cell::Cell::new(false);
        let row = verify_one(&path, &|| {
            if !changed.replace(true) {
                std::fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(&target, &path).unwrap();
            }
            false
        });
        assert!(row.error.is_some());
        assert!(row.sha256_hex.is_none());
        let fifo = temp.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let row = verify_one(&fifo, &|| false);
        assert!(row.skipped.as_deref().unwrap().contains("regular"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sha256_list_round_trips_through_system_shasum_with_escaped_names() {
        let temp = TempDir::new();
        let files = ["normal file", "back\\slash", "line\nbreak", "é"];
        let report = verify_paths(files.iter().map(|name| temp.file(name, "abc")));
        let listing = report.sha256_list().unwrap();
        let manifest = temp.path().join("checksums.sha256");
        std::fs::write(&manifest, &listing).unwrap();
        let result = std::process::Command::new("/usr/bin/shasum")
            .args(["-a", "256", "--strict", "-c"])
            .arg(&manifest)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "stdout={} stderr={} manifest={listing:?}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn sha256_list_escapes_carriage_return_and_rejects_unavailable_rows() {
        let temp = TempDir::new();
        let path = temp.file("carriage\rreturn", "abc");
        let report = verify_paths([path.clone(), temp.dir("folder")]);
        assert_eq!(report.compare_expected(ABC_SHA), ExpectedMatch::Unavailable);
        assert_eq!(
            report.sha256_list().unwrap(),
            format!(
                "\\{ABC_SHA} *{}\n",
                path.to_str().unwrap().replace('\r', "\\r")
            )
        );
        assert!(ChecksumReport::default().sha256_list().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn checksum_list_rejects_non_utf8_paths_without_lossy_export() {
        use std::os::unix::ffi::OsStringExt;
        let temp = TempDir::new();
        let path = temp.path().join(std::ffi::OsString::from_vec(vec![0xff]));
        let report = ChecksumReport {
            rows: vec![ChecksumRow {
                path,
                sha256_hex: Some(ABC_SHA.into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(report.successful(), 1);
        assert!(report.sha256_list().unwrap_err().contains("UTF-8"));
    }

    #[test]
    fn identical_files_share_hashes() {
        let temp = TempDir::new();
        let a = temp.file("a.bin", "payload");
        let b = temp.file("b.bin", "payload");
        let report = verify_paths([a, b]);
        assert_eq!(report.rows.len(), 2);
        assert!(report.rows.iter().all(|row| row.error.is_none()));
        assert_eq!(report.rows[0].content_hash, report.rows[1].content_hash);
        assert_eq!(report.rows[0].blake3_hex, report.rows[1].blake3_hex);
    }

    #[test]
    fn folders_are_skipped_not_failed() {
        let temp = TempDir::new();
        let file = temp.file("a.bin", "payload");
        let folder = temp.dir("Photos");
        let missing = temp.path().join("gone.bin");
        let report = verify_paths([file, folder, missing]);
        assert_eq!(report.rows[1].error, None);
        assert!(report.rows[1].skipped.is_some());
        assert!(report.rows[2].error.is_some());
        assert_eq!(
            report.summary_line(),
            "Calculated 1 file(s), 1 failed, 1 skipped"
        );
        assert!(report.detailed_text().contains("Photos: skipped ("));
        // A file plus a folder is a clean report.
        let report = verify_paths([temp.path().join("a.bin"), temp.path().join("Photos")]);
        assert!(report.rows.iter().all(|row| row.error.is_none()));
        assert_eq!(report.summary_line(), "Calculated 1 file(s), 1 skipped");
    }

    #[test]
    fn content_hash_matches_the_streaming_content_hash() {
        let temp = TempDir::new();
        let path = temp.file("a.bin", "some payload bytes");
        let report = verify_paths([path.clone()]);
        let row = &report.rows[0];
        assert_eq!(row.error, None);
        assert_eq!(row.content_hash, crate::fs_util::content_hash(&path));
        assert!(row.content_hash.is_some());
    }

    #[test]
    fn blake3_hex_is_the_lowercase_digest_of_the_content() {
        let temp = TempDir::new();
        let path = temp.file("a.bin", "some payload bytes");
        let report = verify_paths([path]);
        let expected = blake3::hash(b"some payload bytes").to_hex().to_string();
        assert_eq!(
            report.rows[0].blake3_hex.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(expected.len(), 64);
    }

    fn row(name: &str) -> ChecksumRow {
        ChecksumRow {
            path: PathBuf::from(name),
            content_hash: None,
            blake3_hex: None,
            sha256_hex: None,
            error: None,
            skipped: None,
        }
    }

    #[test]
    fn detailed_text_formats_ok_error_skipped_and_unknown_rows() {
        let report = ChecksumReport {
            rows: vec![
                ChecksumRow {
                    content_hash: Some(0xab),
                    blake3_hex: Some("cafe".to_string()),
                    ..row("/data/ok.bin")
                },
                ChecksumRow {
                    error: Some("permission denied".to_string()),
                    // An error wins over any partial hash.
                    content_hash: Some(1),
                    ..row("/data/bad.bin")
                },
                ChecksumRow {
                    skipped: Some("symlinks are not hashed".to_string()),
                    ..row("/data/link")
                },
                row("/data/unknown.bin"),
            ],
            ..Default::default()
        };
        assert_eq!(
            report.detailed_text(),
            "/data/ok.bin: content=00000000000000ab blake3=cafe sha256=-\n\
             /data/bad.bin: ERROR permission denied\n\
             /data/link: skipped (symlinks are not hashed)\n\
             /data/unknown.bin: content=- blake3=- sha256=-"
        );
        // A path without a file name (the root) falls back to the full path.
        let root = ChecksumReport {
            rows: vec![row("/")],
            ..Default::default()
        };
        assert_eq!(root.detailed_text(), "/: content=- blake3=- sha256=-");
    }

    #[test]
    fn summary_line_for_empty_and_failed_only_reports() {
        assert_eq!(
            ChecksumReport::default().summary_line(),
            "Calculated 0 file(s)"
        );
        assert_eq!(ChecksumReport::default().detailed_text(), "");
        let report = ChecksumReport {
            rows: vec![
                row("/a"),
                ChecksumRow {
                    error: Some("gone".to_string()),
                    ..row("/b")
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            report.summary_line(),
            "Calculated 0 file(s), 1 failed, 1 unavailable"
        );
    }

    #[test]
    fn missing_paths_report_the_os_error_and_folders_explain_the_skip() {
        let temp = TempDir::new();
        let missing = temp.path().join("gone.bin");
        let folder = temp.dir("Photos");
        let report = verify_paths([missing.clone(), folder]);
        let expected = std::fs::symlink_metadata(&missing).unwrap_err().to_string();
        assert_eq!(report.rows[0].error.as_deref(), Some(expected.as_str()));
        assert_eq!(report.rows[0].skipped, None);
        assert_eq!(
            report.rows[1].skipped.as_deref(),
            Some("directories have no content checksum")
        );
        assert_eq!(report.rows[1].content_hash, None);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped_without_following_them() {
        let temp = TempDir::new();
        let target = temp.file("target.bin", "payload");
        let link = temp.path().join("link.bin");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let dangling = temp.path().join("dangling.bin");
        std::os::unix::fs::symlink(temp.path().join("nowhere"), &dangling).unwrap();
        let report = verify_paths([link, dangling]);
        for row in &report.rows {
            assert_eq!(row.error, None);
            assert_eq!(row.blake3_hex, None);
            assert_eq!(row.skipped.as_deref(), Some("symlinks are not hashed"));
        }
        assert_eq!(report.summary_line(), "Calculated 0 file(s), 2 skipped");
    }
}
