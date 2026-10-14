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
    /// Physical copies (hard-link groups) of this content, keyed by copy id,
    /// with how many referencing files share each one. A file that could not
    /// be linked to the exemplar starts its own copy. Empty in records written
    /// before copies were tracked, where every reference shares copy 0.
    #[serde(default)]
    pub copies: BTreeMap<u32, u32>,
    /// The copy the exemplar belongs to.
    #[serde(default)]
    pub exemplar_copy: u32,
}

impl ContentBlob {
    fn bytes_on_disk(&self) -> u64 {
        let copies = self.copies.len().max(1) as u64;
        self.size.saturating_mul(copies)
    }

    /// Fill in the single shared copy implied by records that predate copy
    /// tracking.
    fn normalize_copies(&mut self) {
        if self.copies.is_empty() && self.refs > 0 {
            self.copies.insert(self.exemplar_copy, self.refs);
        }
    }

    fn next_copy_id(&self) -> u32 {
        self.copies
            .keys()
            .next_back()
            .map_or(0, |last| last.saturating_add(1))
            .max(self.exemplar_copy.saturating_add(1))
    }

    /// Drop one reference held through `copy`. Returns true when no
    /// references remain.
    fn release(&mut self, copy: u32) -> bool {
        self.normalize_copies();
        if let Some(count) = self.copies.get_mut(&copy) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.copies.remove(&copy);
            }
        }
        self.refs = self.refs.saturating_sub(1);
        self.refs == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadFile {
    pub relative: PathBuf,
    pub digest: String,
    pub size: u64,
    /// The blob copy (hard-link group) this file belongs to.
    #[serde(default)]
    pub copy: u32,
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

/// Sum of every blob's on-disk bytes. Sizes come from the persisted manifest,
/// so a corrupt huge size must saturate rather than overflow.
fn total_bytes_on_disk(blobs: &BTreeMap<String, ContentBlob>) -> u64 {
    blobs
        .values()
        .map(ContentBlob::bytes_on_disk)
        .fold(0, u64::saturating_add)
}

pub fn usage_of(
    blobs: &BTreeMap<String, ContentBlob>,
    record_count: usize,
    quota: VersionStoreQuota,
) -> VersionStoreUsage {
    VersionStoreUsage {
        bytes_used: total_bytes_on_disk(blobs),
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
        let drop = blobs
            .get_mut(&file.digest)
            .is_some_and(|blob| blob.release(file.copy));
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
        let copy = if let Some(blob) = blobs.get_mut(&digest) {
            blob.normalize_copies();
            let copy = match hardlink_replace(&blob.exemplar, &absolute) {
                LinkOutcome::Linked => blob.exemplar_copy,
                LinkOutcome::NotLinked => blob.next_copy_id(),
                // The exemplar's record was pruned: surviving references
                // still share its inode, so this file is a new physical copy
                // and becomes the exemplar later duplicates link to.
                LinkOutcome::ExemplarMissing => {
                    let copy = blob.next_copy_id();
                    blob.exemplar.clone_from(&absolute);
                    blob.exemplar_copy = copy;
                    copy
                }
            };
            *blob.copies.entry(copy).or_insert(0) += 1;
            blob.refs = blob.refs.saturating_add(1);
            copy
        } else {
            blobs.insert(
                digest.clone(),
                ContentBlob {
                    size,
                    refs: 1,
                    exemplar: absolute.clone(),
                    copies: BTreeMap::from([(0, 1)]),
                    exemplar_copy: 0,
                },
            );
            0
        };
        files.push(PayloadFile {
            relative,
            digest,
            size,
            copy,
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
    let Ok(own) = fs::symlink_metadata(target) else {
        return LinkOutcome::NotLinked;
    };
    // Already the exemplar's inode: renaming a link onto another link to the
    // same inode is a no-op that would leave the temporary link behind.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if own.dev() == meta.dev() && own.ino() == meta.ino() {
            return LinkOutcome::Linked;
        }
    }
    // A hard link shares the inode's mode, so restoring this version would
    // apply the exemplar's permissions; keep a separate copy instead.
    if own.permissions() != meta.permissions() {
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
    // Belt and braces for platforms without the inode check above: a rename
    // between two links to one inode succeeds without removing the source.
    if fs::symlink_metadata(&tmp).is_ok() {
        let _ = fs::remove_file(&tmp);
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
    let mut used = total_bytes_on_disk(blobs);
    if used <= quota.max_bytes {
        return Vec::new();
    }
    let mut refs = blobs.clone();
    for blob in refs.values_mut() {
        blob.normalize_copies();
    }
    let mut remaining = oldest_first.len();
    let mut expired = Vec::new();
    for key in oldest_first {
        if used <= quota.max_bytes || remaining <= 1 {
            break;
        }
        if let Some(files) = payload_files.get(key) {
            for file in files {
                if let Some(blob) = refs.get_mut(&file.digest) {
                    let before = blob.bytes_on_disk();
                    let drop = blob.release(file.copy);
                    let after = if drop { 0 } else { blob.bytes_on_disk() };
                    used = used.saturating_sub(before - after);
                    if drop {
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

    /// Store a, b (linked to a), prune a, then store c (new exemplar) and d
    /// (linked to c): b's inode and the c/d inode are two physical copies.
    fn pruned_exemplar_store(
        temp: &TempDir,
    ) -> (
        BTreeMap<String, ContentBlob>,
        BTreeMap<String, Vec<PayloadFile>>,
    ) {
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
        register_payload_dedup(&d, "4", &mut blobs, &mut files).unwrap();
        (blobs, files)
    }

    #[test]
    fn releasing_the_old_link_group_drops_its_copy() {
        let temp = TempDir::new();
        let (mut blobs, mut files) = pruned_exemplar_store(&temp);
        release_payload_refs(&mut blobs, &mut files, "2");
        assert_eq!(usage(&blobs), 4, "only the c/d inode remains");
        release_payload_refs(&mut blobs, &mut files, "3");
        assert_eq!(usage(&blobs), 4, "d still holds the c/d inode");
        release_payload_refs(&mut blobs, &mut files, "4");
        assert_eq!(usage(&blobs), 0);
    }

    #[test]
    fn releasing_the_new_exemplar_keeps_both_copies_counted() {
        let temp = TempDir::new();
        let (mut blobs, mut files) = pruned_exemplar_store(&temp);
        release_payload_refs(&mut blobs, &mut files, "3");
        assert_eq!(usage(&blobs), 8, "b's inode and d's inode both remain");
        release_payload_refs(&mut blobs, &mut files, "4");
        assert_eq!(usage(&blobs), 4, "only b's inode remains");
        release_payload_refs(&mut blobs, &mut files, "2");
        assert_eq!(usage(&blobs), 0);
    }

    #[test]
    fn quota_overflow_frees_a_copy_only_when_its_group_empties() {
        let temp = TempDir::new();
        let (blobs, files) = pruned_exemplar_store(&temp);
        let quota = VersionStoreQuota { max_bytes: 4 };
        let order = ["3", "2", "4"].map(String::from);
        // Dropping c frees nothing (d shares its inode); dropping b frees
        // b's copy and brings usage to 4.
        assert_eq!(
            quota_overflow_keys(&order, &blobs, &files, quota),
            vec!["3".to_string(), "2".to_string()]
        );
    }

    #[test]
    fn corrupt_huge_blob_sizes_saturate_usage_and_quota() {
        let blob = |size| ContentBlob {
            size,
            refs: 1,
            exemplar: PathBuf::from("x"),
            copies: BTreeMap::from([(0, 1)]),
            exemplar_copy: 0,
        };
        let blobs = BTreeMap::from([
            ("a".to_string(), blob(u64::MAX)),
            ("b".to_string(), blob(u64::MAX - 1)),
        ]);
        assert_eq!(usage(&blobs), u64::MAX);
        let files = BTreeMap::from([
            (
                "1".to_string(),
                vec![PayloadFile {
                    relative: PathBuf::from("a"),
                    digest: "a".into(),
                    size: u64::MAX,
                    copy: 0,
                }],
            ),
            ("2".to_string(), Vec::new()),
        ]);
        let order = ["1", "2"].map(String::from);
        let quota = VersionStoreQuota { max_bytes: 4 };
        assert_eq!(
            quota_overflow_keys(&order, &blobs, &files, quota),
            vec!["1".to_string()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn dedup_of_an_existing_hard_link_leaves_no_temp_link() {
        let temp = TempDir::new();
        let a = temp.file("one/a.bin", "same");
        let dir = a.parent().unwrap().parent().unwrap().join("two");
        fs::create_dir_all(&dir).unwrap();
        fs::hard_link(&a, dir.join("a.bin")).unwrap();
        let mut blobs = BTreeMap::new();
        let mut files = BTreeMap::new();
        register_payload_dedup(&a, "1", &mut blobs, &mut files).unwrap();
        register_payload_dedup(&dir, "2", &mut blobs, &mut files).unwrap();
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("a.bin")]);
        assert_eq!(usage(&blobs), 4);
    }
}
