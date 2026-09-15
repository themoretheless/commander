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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadFile {
    pub relative: PathBuf,
    pub digest: String,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VersionStoreUsage {
    pub bytes_used: u64,
    pub bytes_quota: u64,
    pub unique_blobs: usize,
    pub records: usize,
}

impl VersionStoreUsage {
    pub fn over_quota(self) -> bool {
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
        bytes_used: blobs.values().map(|b| b.size).sum(),
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
        if let Some(blob) = blobs.get(&digest) {
            if blob.exemplar != absolute {
                let _ = hardlink_replace(&blob.exemplar, &absolute);
            }
            if let Some(blob) = blobs.get_mut(&digest) {
                blob.refs = blob.refs.saturating_add(1);
            }
        } else {
            blobs.insert(
                digest.clone(),
                ContentBlob {
                    size,
                    refs: 1,
                    exemplar: absolute.clone(),
                },
            );
        }
        files.push(PayloadFile {
            relative,
            digest,
            size,
        });
    }
    payload_files.insert(key.to_string(), files);
    Ok(())
}

fn hardlink_replace(exemplar: &Path, target: &Path) -> Result<(), String> {
    if exemplar == target {
        return Ok(());
    }
    let Ok(meta) = fs::symlink_metadata(exemplar) else {
        return Ok(());
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Ok(());
    }
    let tmp = target.with_extension("commander-dedup-tmp");
    let _ = fs::remove_file(&tmp);
    if fs::hard_link(exemplar, &tmp).is_ok() {
        if fs::rename(&tmp, target).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
    Ok(())
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
    let mut used: u64 = blobs.values().map(|b| b.size).sum();
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
}
