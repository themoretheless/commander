//! Content-chunk digests, hard-link dedup, and visible store quota (J002).
use crate::operation::VersionStoreQuota;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const CONTENT_CHUNK_SIZE: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentBlob {
    pub size: u64,
    pub refs: u32,
    pub exemplar: PathBuf,
    /// Referencing files that hold their own physical copy because they could
    /// not be hard-linked to the exemplar; each adds `size` to store usage.
    #[serde(default)]
    pub separate_copies: u32,
}

impl ContentBlob {
    const fn bytes_on_disk(&self) -> u64 {
        self.size.saturating_mul(1 + self.separate_copies as u64)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadFile {
    pub relative: PathBuf,
    pub digest: String,
    pub size: u64,
    /// True when this file is not hard-linked to its blob's exemplar.
    #[serde(default)]
    pub separate: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VersionStoreUsage {
    pub bytes_used: u64,
    pub bytes_quota: u64,
    pub unique_blobs: usize,
    pub records: usize,
}

impl VersionStoreUsage {
    pub const fn over_quota(self) -> bool {
        self.bytes_used > self.bytes_quota
    }
    pub fn label(self) -> String {
        format!(
            "{} / {} ({} unique)",
            format_store_bytes(self.bytes_used),
            format_store_bytes(self.bytes_quota),
            self.unique_blobs
        )
    }
}

pub fn format_store_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else {
        format!("{bytes} B")
    }
}

pub fn usage_of(
    blobs: &BTreeMap<String, ContentBlob>,
    record_count: usize,
    quota: VersionStoreQuota,
) -> VersionStoreUsage {
    VersionStoreUsage {
        bytes_used: blobs.values().map(ContentBlob::bytes_on_disk).sum(),
        bytes_quota: quota.max_bytes,
        unique_blobs: blobs.len(),
        records: record_count,
    }
}

pub fn hash_file_chunked(path: &Path) -> Result<(String, u64), String> {
    let mut file = fs::File::open(path)
        .map_err(|e| format!("Could not open {} for content hashing: {e}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; CONTENT_CHUNK_SIZE];
    let mut size = 0_u64;
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("Could not hash {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        size += n as u64;
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

pub fn release_payload_refs(
    blobs: &mut BTreeMap<String, ContentBlob>,
    payload_files: &mut BTreeMap<String, Vec<PayloadFile>>,
    key: &str,
) {
    let Some(files) = payload_files.remove(key) else {
        return;
    };
    for file in files {
        let drop = blobs.get_mut(&file.digest).is_some_and(|blob| {
            if file.separate {
                blob.separate_copies = blob.separate_copies.saturating_sub(1);
            }
            blob.refs = blob.refs.saturating_sub(1);
            blob.refs == 0
        });
        if drop {
            blobs.remove(&file.digest);
        }
    }
}

pub fn register_payload_dedup(
    stored: &Path,
    key: &str,
    blobs: &mut BTreeMap<String, ContentBlob>,
    payload_files: &mut BTreeMap<String, Vec<PayloadFile>>,
) -> Result<(), String> {
    release_payload_refs(blobs, payload_files, key);
    let mut files = Vec::new();
    for absolute in collect_regular_files(stored)? {
        let relative = absolute
            .strip_prefix(stored)
            .map_or_else(|_| PathBuf::from("."), Path::to_path_buf);
        let (digest, size) = hash_file_chunked(&absolute)?;
        let mut separate = false;
        if let Some(blob) = blobs.get_mut(&digest) {
            match hardlink_replace(&blob.exemplar, &absolute) {
                LinkOutcome::Linked => {}
                LinkOutcome::NotLinked => separate = true,
                // The exemplar's record was pruned: surviving references
                // still share its inode, so this file is a new physical copy
                // and becomes the exemplar later duplicates link to.
                LinkOutcome::ExemplarMissing => {
                    blob.exemplar.clone_from(&absolute);
                    separate = true;
                }
            }
            blob.refs = blob.refs.saturating_add(1);
            if separate {
                blob.separate_copies = blob.separate_copies.saturating_add(1);
            }
        } else {
            blobs.insert(
                digest.clone(),
                ContentBlob {
                    size,
                    refs: 1,
                    exemplar: absolute.clone(),
                    separate_copies: 0,
                },
            );
        }
        files.push(PayloadFile {
            relative,
            digest,
            size,
            separate,
        });
    }
    payload_files.insert(key.to_string(), files);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkOutcome {
    Linked,
    NotLinked,
    ExemplarMissing,
}

fn hardlink_replace(exemplar: &Path, target: &Path) -> LinkOutcome {
    if exemplar == target {
        return LinkOutcome::Linked;
    }
    let meta = match fs::symlink_metadata(exemplar) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return LinkOutcome::ExemplarMissing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return LinkOutcome::ExemplarMissing;
        }
        Err(_) => return LinkOutcome::NotLinked,
    };
    // A hard link shares the inode's mode, so restoring this version would
    // apply the exemplar's permissions; keep a separate copy instead.
    if !fs::symlink_metadata(target).is_ok_and(|own| own.permissions() == meta.permissions()) {
        return LinkOutcome::NotLinked;
    }
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return LinkOutcome::NotLinked;
    };
    // A unique hidden name, never pre-deleted, so a real payload file that
    // happens to share it cannot be removed.
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".dedup-{:016x}", rand::random::<u64>()));
    let tmp = parent.join(tmp_name);
    if fs::hard_link(exemplar, &tmp).is_err() {
        return LinkOutcome::NotLinked;
    }
    if fs::rename(&tmp, target).is_err() {
        let _ = fs::remove_file(&tmp);
        return LinkOutcome::NotLinked;
    }
    LinkOutcome::Linked
}

fn collect_regular_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let meta = fs::symlink_metadata(root)
        .map_err(|e| format!("Could not inspect version payload {}: {e}", root.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(Vec::new());
    }
    if meta.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    if !meta.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .map_err(|e| format!("Could not list version payload {}: {e}", dir.display()))?
        {
            let entry = entry.map_err(|e| {
                format!(
                    "Could not read version payload under {}: {e}",
                    dir.display()
                )
            })?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)
                .map_err(|e| format!("Could not inspect {}: {e}", path.display()))?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

pub fn quota_overflow_keys(
    oldest_first: &[String],
    blobs: &BTreeMap<String, ContentBlob>,
    payload_files: &BTreeMap<String, Vec<PayloadFile>>,
    quota: VersionStoreQuota,
) -> Vec<String> {
    let mut used: u64 = blobs.values().map(ContentBlob::bytes_on_disk).sum();
    if used <= quota.max_bytes {
        return Vec::new();
    }
    let mut refs = blobs.clone();
    let mut remaining = oldest_first.len();
    let mut expired = Vec::new();
    for key in oldest_first {
        if used <= quota.max_bytes || remaining <= 1 {
            break;
        }
        if let Some(files) = payload_files.get(key) {
            for file in files {
                if let Some(blob) = refs.get_mut(&file.digest) {
                    if file.separate {
                        used = used.saturating_sub(blob.size);
                    }
                    blob.refs = blob.refs.saturating_sub(1);
                    if blob.refs == 0 {
                        used = used.saturating_sub(blob.size);
                        refs.remove(&file.digest);
                    }
                }
            }
        }
        expired.push(key.clone());
        remaining -= 1;
    }
    expired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn identical_files_share_one_blob() {
        let temp = TempDir::new();
        let a = temp.file("a.bin", &"x".repeat(CONTENT_CHUNK_SIZE + 3));
        let b = temp.file("b.bin", &"x".repeat(CONTENT_CHUNK_SIZE + 3));
        let mut blobs = BTreeMap::new();
        let mut files = BTreeMap::new();
        register_payload_dedup(&a, "1", &mut blobs, &mut files).unwrap();
        register_payload_dedup(&b, "2", &mut blobs, &mut files).unwrap();
        assert_eq!(blobs.len(), 1);
        assert_eq!(blobs.values().next().unwrap().refs, 2);
    }

    fn usage(blobs: &BTreeMap<String, ContentBlob>) -> u64 {
        usage_of(blobs, 0, VersionStoreQuota::default()).bytes_used
    }

    #[test]
    fn dedup_never_deletes_a_payload_file_named_like_its_temp() {
        let temp = TempDir::new();
        let a = temp.file("one/a.bin", "same");
        let b = temp.file("two/a.bin", "same");
        let bystander = temp.file("two/a.commander-dedup-tmp", "keep me");
        let mut blobs = BTreeMap::new();
        let mut files = BTreeMap::new();
        register_payload_dedup(&a, "1", &mut blobs, &mut files).unwrap();
        register_payload_dedup(&b, "2", &mut blobs, &mut files).unwrap();
        assert_eq!(fs::read_to_string(bystander).unwrap(), "keep me");
        assert_eq!(fs::read_to_string(b).unwrap(), "same");
    }

    #[cfg(unix)]
    #[test]
    fn dedup_keeps_a_separate_copy_when_permissions_differ() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new();
        let public = temp.file("one/a.bin", "same");
        let private = temp.file("two/a.bin", "same");
        fs::set_permissions(&public, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
        let mut blobs = BTreeMap::new();
        let mut files = BTreeMap::new();
        register_payload_dedup(&public, "1", &mut blobs, &mut files).unwrap();
        register_payload_dedup(&private, "2", &mut blobs, &mut files).unwrap();

        let mode = fs::metadata(&private).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // Two physical copies count twice toward the quota.
        assert_eq!(usage(&blobs), 8);
        release_payload_refs(&mut blobs, &mut files, "2");
        assert_eq!(usage(&blobs), 4);
    }

    #[test]
    fn duplicates_after_the_exemplar_is_pruned_are_counted_and_relinked() {
        let temp = TempDir::new();
        let a = temp.file("one/a.bin", "same");
        let b = temp.file("two/a.bin", "same");
        let c = temp.file("three/a.bin", "same");
        let d = temp.file("four/a.bin", "same");
        let mut blobs = BTreeMap::new();
        let mut files = BTreeMap::new();
        register_payload_dedup(&a, "1", &mut blobs, &mut files).unwrap();
        register_payload_dedup(&b, "2", &mut blobs, &mut files).unwrap();
        release_payload_refs(&mut blobs, &mut files, "1");
        fs::remove_file(&a).unwrap();

        register_payload_dedup(&c, "3", &mut blobs, &mut files).unwrap();
        // b's inode and c are two physical copies.
        assert_eq!(usage(&blobs), 8);
        assert_eq!(blobs.values().next().unwrap().exemplar, c);

        register_payload_dedup(&d, "4", &mut blobs, &mut files).unwrap();
        assert_eq!(usage(&blobs), 8, "d links to the new exemplar");
    }
}
