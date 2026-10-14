//! User-facing checksum / verify report built from existing hash primitives.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksumRow {
    pub path: PathBuf,
    pub content_hash: Option<u64>,
    pub blake3_hex: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumReport {
    pub rows: Vec<ChecksumRow>,
}

impl ChecksumReport {
    pub fn summary_line(&self) -> String {
        let ok = self.rows.iter().filter(|row| row.error.is_none()).count();
        let failed = self.rows.len().saturating_sub(ok);
        if failed == 0 {
            format!("Verified {ok} file(s)")
        } else {
            format!("Verified {ok} file(s), {failed} failed")
        }
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
            };
        }
    };
    if meta.file_type().is_dir() {
        return ChecksumRow {
            path: path.to_path_buf(),
            content_hash: None,
            blake3_hex: None,
            error: Some("directories have no content checksum".to_string()),
        };
    }
    if meta.file_type().is_symlink() {
        return ChecksumRow {
            path: path.to_path_buf(),
            content_hash: None,
            blake3_hex: None,
            error: Some("symlinks are not hashed".to_string()),
        };
    }
    // `content_hash` is the leading 8 bytes of the file's BLAKE3 digest (see
    // `fs_util::content_hash`), so derive it from the verified hash rather than
    // streaming the whole file a second time on the UI thread.
    match crate::verified_hash::file(path) {
        Ok(hash) => ChecksumRow {
            path: path.to_path_buf(),
            content_hash: Some(content_hash_of(&hash)),
            blake3_hex: Some(hex32(&hash)),
            error: None,
        },
        Err(error) => ChecksumRow {
            path: path.to_path_buf(),
            content_hash: None,
            blake3_hex: None,
            error: Some(error.to_string()),
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

fn hex32(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
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
    fn content_hash_matches_the_streaming_content_hash() {
        let temp = TempDir::new();
        let path = temp.file("a.bin", "some payload bytes");
        let report = verify_paths([path.clone()]);
        let row = &report.rows[0];
        assert_eq!(row.error, None);
        assert_eq!(row.content_hash, crate::fs_util::content_hash(&path));
        assert!(row.content_hash.is_some());
    }
}
