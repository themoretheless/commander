//! User-facing checksum / verify report built from existing hash primitives.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksumRow {
    pub path: PathBuf,
    pub content_hash: Option<u64>,
    pub blake3_hex: Option<String>,
    pub error: Option<String>,
    /// Why the entry was not hashed (a folder or symlink). Skipped rows are
    /// neither verified nor failed, so they never turn the report into an
    /// error.
    pub skipped: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumReport {
    pub rows: Vec<ChecksumRow>,
}

impl ChecksumReport {
    pub fn summary_line(&self) -> String {
        let skipped = self.rows.iter().filter(|row| row.skipped.is_some()).count();
        let failed = self.rows.iter().filter(|row| row.error.is_some()).count();
        let ok = self.rows.len().saturating_sub(skipped + failed);
        let mut line = format!("Verified {ok} file(s)");
        if failed > 0 {
            line.push_str(&format!(", {failed} failed"));
        }
        if skipped > 0 {
            line.push_str(&format!(", {skipped} skipped"));
        }
        line
    }

    pub fn detailed_text(&self) -> String {
        let mut lines = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let name = row
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| row.path.display().to_string());
            if let Some(error) = &row.error {
                lines.push(format!("{name}: ERROR {error}"));
                continue;
            }
            if let Some(reason) = &row.skipped {
                lines.push(format!("{name}: skipped ({reason})"));
                continue;
            }
            let content = row
                .content_hash
                .map(|hash| format!("{hash:016x}"))
                .unwrap_or_else(|| "-".to_string());
            let blake = row.blake3_hex.as_deref().unwrap_or("-");
            lines.push(format!("{name}: content={content} blake3={blake}"));
        }
        lines.join("\n")
    }
}

pub fn verify_paths(paths: impl IntoIterator<Item = PathBuf>) -> ChecksumReport {
    let mut report = ChecksumReport::default();
    for path in paths {
        report.rows.push(verify_one(&path));
    }
    report
}

fn verify_one(path: &Path) -> ChecksumRow {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) => {
            return ChecksumRow {
                path: path.to_path_buf(),
                content_hash: None,
                blake3_hex: None,
                error: Some(error.to_string()),
                skipped: None,
            };
        }
    };
    if meta.file_type().is_dir() {
        return ChecksumRow {
            path: path.to_path_buf(),
            content_hash: None,
            blake3_hex: None,
            error: None,
            skipped: Some("directories have no content checksum".to_string()),
        };
    }
    if meta.file_type().is_symlink() {
        return ChecksumRow {
            path: path.to_path_buf(),
            content_hash: None,
            blake3_hex: None,
            error: None,
            skipped: Some("symlinks are not hashed".to_string()),
        };
    }
    // `content_hash` is the leading 8 bytes of the file's BLAKE3 digest (see
    // `fs_util::content_hash`), so derive it from the verified hash rather than
    // streaming the whole file a second time on the UI thread.
    match crate::verified_hash::file(path) {
        Ok(hash) => ChecksumRow {
            path: path.to_path_buf(),
            content_hash: Some(content_hash_of(&hash)),
            blake3_hex: Some(blake3::Hash::from(hash).to_hex().to_string()),
            error: None,
            skipped: None,
        },
        Err(error) => ChecksumRow {
            path: path.to_path_buf(),
            content_hash: None,
            blake3_hex: None,
            error: Some(error.to_string()),
            skipped: None,
        },
    }
}

/// The 64-bit content hash [`crate::fs_util::content_hash`] reports, taken
/// from an already computed BLAKE3 digest.
fn content_hash_of(blake3: &[u8; 32]) -> u64 {
    let mut prefix = [0_u8; 8];
    prefix.copy_from_slice(&blake3[..8]);
    u64::from_be_bytes(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

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
            "Verified 1 file(s), 1 failed, 1 skipped"
        );
        assert!(report.detailed_text().contains("Photos: skipped ("));
        // A file plus a folder is a clean report.
        let report = verify_paths([temp.path().join("a.bin"), temp.path().join("Photos")]);
        assert!(report.rows.iter().all(|row| row.error.is_none()));
        assert_eq!(report.summary_line(), "Verified 1 file(s), 1 skipped");
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
        };
        assert_eq!(
            report.detailed_text(),
            "ok.bin: content=00000000000000ab blake3=cafe\n\
             bad.bin: ERROR permission denied\n\
             link: skipped (symlinks are not hashed)\n\
             unknown.bin: content=- blake3=-"
        );
        // A path without a file name (the root) falls back to the full path.
        let root = ChecksumReport {
            rows: vec![row("/")],
        };
        assert_eq!(root.detailed_text(), "/: content=- blake3=-");
    }

    #[test]
    fn summary_line_for_empty_and_failed_only_reports() {
        assert_eq!(
            ChecksumReport::default().summary_line(),
            "Verified 0 file(s)"
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
        };
        assert_eq!(report.summary_line(), "Verified 1 file(s), 1 failed");
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
        assert_eq!(report.summary_line(), "Verified 0 file(s), 2 skipped");
    }
}
