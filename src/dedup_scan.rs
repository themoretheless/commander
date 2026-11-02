//! Background duplicate scan. Hashing every same-size file of a folder can
//! take minutes on large files, so it runs on a worker thread; the dialog
//! polls [`DupScan`] and shows a scanning state meanwhile.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::dedup::{DupGroup, FileKey};
use crate::panel::FileEntry;

/// Inclusive byte limits. Empty input means no bound; zero-size files stay excluded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SizeRange {
    pub min: u64,
    pub max: Option<u64>,
}

impl SizeRange {
    pub fn parse(min: &str, max: &str) -> Result<Self, &'static str> {
        let number = |value: &str| {
            let value = value.trim();
            if value.is_empty() {
                return Ok(None);
            }
            if value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("Size limits must be whole byte counts (0 to 18446744073709551615)");
            }
            value
                .parse::<u64>()
                .map(Some)
                .map_err(|_| "Size limit exceeds 18446744073709551615 bytes")
        };
        let min = number(min)?.unwrap_or(0);
        let max = number(max)?;
        if max.is_some_and(|max| min > max) {
            return Err("Minimum size must not exceed maximum size");
        }
        Ok(Self { min, max })
    }

    fn contains(self, size: u64) -> bool {
        size > 0 && size >= self.min && self.max.is_none_or(|max| size <= max)
    }

    pub fn label(self) -> String {
        match (self.min, self.max) {
            (0, None) => "Applied size: all nonempty files".into(),
            (min, None) => format!("Applied size: at least {min} bytes"),
            (min, Some(max)) => format!("Applied size: {min}–{max} bytes (inclusive)"),
        }
    }
}

/// Subfolder levels below the root; empty means unlimited and zero means root only.
pub fn parse_depth(raw: &str) -> Result<Option<u32>, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.len() > 10 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(
            "Subfolder levels must be a whole number from 0 to 4294967295, or empty for unlimited",
        );
    }
    raw.parse::<u32>()
        .map(Some)
        .map_err(|_| "Subfolder levels exceed 4294967295")
}

/// At least two distinct verified file copies; this does not set the keeper count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MinCopies(u32);
impl Default for MinCopies {
    fn default() -> Self {
        Self(2)
    }
}
impl MinCopies {
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(Self::default());
        }
        if raw.len() > 10 || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Minimum copies must be a whole number from 2 to 4294967295");
        }
        match raw.parse::<u32>() {
            Ok(n) if n >= 2 => Ok(Self(n)),
            _ => Err("Minimum copies must be from 2 to 4294967295"),
        }
    }
    pub fn get(self) -> usize {
        self.0 as usize
    }
}

/// A running scan. Dropping it cancels the worker.
pub struct DupScan {
    receiver: Receiver<Vec<DupGroup>>,
    cancelled: Arc<AtomicBool>,
    hashed: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    enumerating: Arc<AtomicBool>,
    issues: Arc<AtomicUsize>,
}

impl DupScan {
    /// The groups once the worker finished; `None` while it runs. A worker
    /// that died reports no groups rather than scanning forever.
    pub fn poll(&self) -> Option<Vec<DupGroup>> {
        match self.receiver.try_recv() {
            Ok(groups) => Some(groups),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.issues.fetch_add(1, Ordering::Relaxed);
                Some(Vec::new())
            }
        }
    }

    pub fn enumerating(&self) -> bool {
        self.enumerating.load(Ordering::Acquire)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Acquire)
    }

    pub fn issues(&self) -> usize {
        self.issues.load(Ordering::Relaxed)
    }

    /// Candidates hashed so far.
    pub fn hashed(&self) -> usize {
        self.hashed.load(Ordering::Relaxed)
    }
}

impl Drop for DupScan {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Hash `candidates` on a worker thread; `notify` wakes the UI when done.
#[cfg(test)]
pub fn spawn(
    candidates: Vec<FileEntry>,
    sizes: SizeRange,
    notify: impl Fn() + Send + 'static,
) -> DupScan {
    spawn_filtered(
        candidates,
        sizes,
        Default::default(),
        MinCopies::default(),
        notify,
    )
}

/// Enumerate a directory tree afresh on the worker, never following symlinks.
/// Empty files and repeated physical-file identities are omitted as in the pane scan.
#[cfg(test)]
pub fn spawn_tree(root: PathBuf, sizes: SizeRange, notify: impl Fn() + Send + 'static) -> DupScan {
    spawn_tree_filtered(
        root,
        sizes,
        Default::default(),
        None,
        MinCopies::default(),
        notify,
    )
}

/// Captured listing candidates with an explicit origin for relative-path grouping.
pub fn spawn_filtered_at(
    root: PathBuf,
    candidates: Vec<FileEntry>,
    sizes: SizeRange,
    names: crate::duplicate_name_filter::Filter,
    min_copies: MinCopies,
    notify: impl Fn() + Send + 'static,
) -> DupScan {
    spawn_work(
        Some(candidates),
        Some(RootScan {
            paths: vec![root],
            cross_only: false,
            fresh: false,
        }),
        sizes,
        names,
        None,
        min_copies,
        notify,
    )
}

#[cfg(test)]
pub fn spawn_filtered(
    candidates: Vec<FileEntry>,
    sizes: SizeRange,
    names: crate::duplicate_name_filter::Filter,
    min_copies: MinCopies,
    notify: impl Fn() + Send + 'static,
) -> DupScan {
    spawn_work(
        Some(candidates),
        None,
        sizes,
        names,
        None,
        min_copies,
        notify,
    )
}
pub fn spawn_tree_filtered(
    root: PathBuf,
    sizes: SizeRange,
    names: crate::duplicate_name_filter::Filter,
    max_depth: Option<u32>,
    min_copies: MinCopies,
    notify: impl Fn() + Send + 'static,
) -> DupScan {
    spawn_roots_filtered(vec![root], sizes, names, max_depth, min_copies, notify)
}

/// Fresh enumeration of a union of roots; bounds are relative to each root.
pub fn spawn_roots_filtered(
    roots: Vec<PathBuf>,
    sizes: SizeRange,
    names: crate::duplicate_name_filter::Filter,
    max_depth: Option<u32>,
    min_copies: MinCopies,
    notify: impl Fn() + Send + 'static,
) -> DupScan {
    spawn_roots_selected(roots, sizes, names, max_depth, min_copies, false, notify)
}

pub fn spawn_roots_selected(
    roots: Vec<PathBuf>,
    sizes: SizeRange,
    names: crate::duplicate_name_filter::Filter,
    max_depth: Option<u32>,
    min_copies: MinCopies,
    cross_only: bool,
    notify: impl Fn() + Send + 'static,
) -> DupScan {
    spawn_work(
        None,
        Some(RootScan {
            paths: roots,
            cross_only,
            fresh: true,
        }),
        sizes,
        names,
        max_depth,
        min_copies,
        notify,
    )
}
struct RootScan {
    fresh: bool,
    paths: Vec<PathBuf>,
    cross_only: bool,
}

/// Assign overlapping paths to the most specific root, as the UI labels do.
pub fn root_owner<'a>(
    path: &std::path::Path,
    roots: impl IntoIterator<Item = &'a std::path::Path>,
) -> Option<usize> {
    roots
        .into_iter()
        .enumerate()
        .filter(|(_, root)| path.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(i, _)| i)
}
fn spans_roots(group: &DupGroup, roots: &[PathBuf]) -> bool {
    let mut first = None;
    for file in &group.files {
        if let Some(owner) = root_owner(&file.path, roots.iter().map(PathBuf::as_path)) {
            if first.is_some_and(|previous| previous != owner) {
                return true;
            }
            first = Some(owner);
        }
    }
    false
}
fn prune_unshared_sizes(files: &mut Vec<FileEntry>, roots: &[PathBuf]) {
    let mut buckets: HashMap<u64, (Option<usize>, bool)> = HashMap::new();
    for file in files.iter() {
        if let Some(owner) = root_owner(&file.path, roots.iter().map(PathBuf::as_path)) {
            let (first, shared) = buckets.entry(file.size).or_default();
            *shared |= first.is_some_and(|previous| previous != owner);
            *first = Some(owner);
        }
    }
    files.retain(|file| buckets.get(&file.size).is_some_and(|(_, shared)| *shared));
}

fn spawn_work(
    candidates: Option<Vec<FileEntry>>,
    roots: Option<RootScan>,
    sizes: SizeRange,
    names: crate::duplicate_name_filter::Filter,
    max_depth: Option<u32>,
    min_copies: MinCopies,
    notify: impl Fn() + Send + 'static,
) -> DupScan {
    let candidates = candidates.map(|mut files| {
        let mut inodes = std::collections::HashSet::new();
        files.retain(|file| {
            !file.is_dir
                && !file.is_symlink()
                && sizes.contains(file.size)
                && names.matches(&file.path)
                && crate::workspace::hard_link_key(file).is_none_or(|key| inodes.insert(key))
        });
        files
    });
    let cancelled = Arc::new(AtomicBool::new(false));
    let hashed = Arc::new(AtomicUsize::new(0));
    let to_hash = candidates
        .as_ref()
        .map_or(0, |files| needs_hash(files, min_copies).count());
    let total = Arc::new(AtomicUsize::new(to_hash));
    let enumerating = Arc::new(AtomicBool::new(roots.as_ref().is_some_and(|r| r.fresh)));
    let issues = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    let worker_cancelled = Arc::clone(&cancelled);
    let worker_hashed = Arc::clone(&hashed);
    let worker_total = Arc::clone(&total);
    let worker_enumerating = Arc::clone(&enumerating);
    let worker_issues = Arc::clone(&issues);
    std::thread::spawn(move || {
        let mut candidates = match roots.as_ref().filter(|r| r.fresh) {
            Some(roots) => tree_candidates_roots(
                &roots.paths,
                sizes,
                &names,
                max_depth,
                &worker_cancelled,
                &worker_issues,
            ),
            None => candidates.unwrap_or_default(),
        };
        let grouping_roots = roots.as_ref().map_or(&[][..], |r| r.paths.as_slice());
        if names.rules.grouping == crate::duplicate_name_group::Component::RelativePath {
            candidates.retain(|file| {
                if worker_cancelled.load(Ordering::Acquire) {
                    return false;
                }
                let valid = crate::duplicate_name_group::relative_path(&file.path, grouping_roots)
                    .is_some();
                if !valid {
                    worker_issues.fetch_add(1, Ordering::Relaxed);
                }
                valid
            });
        }
        if let Some(roots) = roots.as_ref().filter(|roots| roots.cross_only) {
            prune_unshared_sizes(&mut candidates, &roots.paths);
        }
        worker_total.store(
            needs_hash(&candidates, min_copies).count(),
            Ordering::Release,
        );
        worker_enumerating.store(false, Ordering::Release);
        let mut groups = hash_duplicates_with_issues(
            &candidates,
            &worker_cancelled,
            &worker_hashed,
            &worker_issues,
            min_copies,
        );
        groups = crate::duplicate_extension::partition(
            groups,
            names.rules.extension,
            min_copies.get(),
            || worker_cancelled.load(Ordering::Acquire),
        );
        groups = crate::duplicate_name_group::partition(
            grouping_roots,
            groups,
            names.rules.grouping,
            names.rules.grouping_ignore_ascii_case,
            min_copies.get(),
            || worker_cancelled.load(Ordering::Acquire),
        );
        if let Some(roots) = roots.as_ref().filter(|roots| roots.cross_only) {
            groups.retain(|group| spans_roots(group, &roots.paths));
        }
        if !worker_cancelled.load(Ordering::Acquire) {
            let _ = sender.send(groups);
            notify();
        }
    });
    DupScan {
        receiver,
        cancelled,
        hashed,
        total,
        enumerating,
        issues,
    }
}

#[cfg(test)]
fn tree_candidates(
    root: &std::path::Path,
    sizes: SizeRange,
    cancelled: &AtomicBool,
    issues: &AtomicUsize,
) -> Vec<FileEntry> {
    tree_candidates_filtered(root, sizes, &Default::default(), None, cancelled, issues)
}
#[cfg(test)]
fn tree_candidates_filtered(
    root: &std::path::Path,
    sizes: SizeRange,
    names: &crate::duplicate_name_filter::Filter,
    max_depth: Option<u32>,
    cancelled: &AtomicBool,
    issues: &AtomicUsize,
) -> Vec<FileEntry> {
    tree_candidates_roots(
        &[root.to_path_buf()],
        sizes,
        names,
        max_depth,
        cancelled,
        issues,
    )
}
fn tree_candidates_roots(
    roots: &[PathBuf],
    sizes: SizeRange,
    names: &crate::duplicate_name_filter::Filter,
    max_depth: Option<u32>,
    cancelled: &AtomicBool,
    issues: &AtomicUsize,
) -> Vec<FileEntry> {
    if cancelled.load(Ordering::Acquire) {
        return Vec::new();
    }
    let mut pending = Vec::new();
    let mut unique_roots = std::collections::HashSet::new();
    for root in roots {
        if cancelled.load(Ordering::Acquire) {
            return Vec::new();
        }
        if !unique_roots.insert(root.clone()) {
            continue;
        }
        if std::fs::symlink_metadata(root).is_ok_and(|meta| meta.is_dir()) {
            pending.push((root.clone(), 0u32));
        } else {
            issues.fetch_add(1, Ordering::Relaxed);
        }
    }
    let mut files = Vec::new();
    // A later root may reach the same directory at a shallower level. Revisit
    // then so depth pruning relative to an overlapping root cannot lose files.
    let mut directories = std::collections::HashMap::new();
    while let Some((dir, depth)) = pending.pop() {
        if cancelled.load(Ordering::Acquire) {
            return Vec::new();
        }
        // Recheck before descent; a shallower route may permit more descendants.
        let meta = match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => meta,
            _ => {
                issues.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        let identity = crate::path_identity::PathIdentity::from_metadata(dir.clone(), &meta);
        if let (Some(volume), Some(file_id)) = (identity.volume, identity.file_id) {
            if directories
                .get(&(volume, file_id))
                .is_some_and(|&seen| seen <= depth)
            {
                continue;
            }
            directories.insert((volume, file_id), depth);
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                issues.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        for entry in entries {
            if cancelled.load(Ordering::Acquire) {
                return Vec::new();
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    issues.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => {
                    issues.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            if meta.is_dir() {
                if max_depth.is_none_or(|limit| depth < limit) {
                    pending.push((path, depth.saturating_add(1)));
                }
            } else if meta.is_file()
                && sizes.contains(meta.len())
                && names.matches(&path)
                && let Some(file) = FileEntry::from_meta(path, &meta)
            {
                files.push(file);
            }
        }
    }
    files.sort_by(|a, b| {
        a.path
            .components()
            .count()
            .cmp(&b.path.components().count())
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut inodes = std::collections::HashSet::new();
    let mut paths = std::collections::HashSet::new();
    files.retain(|file| {
        paths.insert(file.path.clone())
            && crate::workspace::hard_link_key(file).is_none_or(|key| inodes.insert(key))
    });
    files
}

/// Buckets too small to meet the requested verified-copy threshold need no hash.
fn needs_hash(files: &[FileEntry], min_copies: MinCopies) -> impl Iterator<Item = &FileEntry> {
    let mut size_counts: HashMap<u64, usize> = HashMap::new();
    for file in files {
        *size_counts.entry(file.size).or_insert(0) += 1;
    }
    files
        .iter()
        .filter(move |file| size_counts.get(&file.size).copied().unwrap_or(0) >= min_copies.get())
}

/// Group byte-identical `files`: size-prefiltered, grouped by (size, digest
/// prefix), then confirmed by the full 256-bit digest (a 64-bit prefix match
/// is not proof), so the dialog never offers to trash a false positive. Each
/// candidate is read once. A cancelled scan stops early and returns nothing.
#[cfg(test)]
pub fn hash_duplicates(
    files: &[FileEntry],
    cancelled: &AtomicBool,
    hashed: &AtomicUsize,
) -> Vec<DupGroup> {
    hash_duplicates_with_issues(
        files,
        cancelled,
        hashed,
        &AtomicUsize::new(0),
        MinCopies::default(),
    )
}

fn hash_duplicates_with_issues(
    files: &[FileEntry],
    cancelled: &AtomicBool,
    hashed: &AtomicUsize,
    issues: &AtomicUsize,
    min_copies: MinCopies,
) -> Vec<DupGroup> {
    let mut keys: Vec<FileKey> = Vec::new();
    let mut digests: HashMap<PathBuf, crate::verified_hash::VerifiedHash> = HashMap::new();
    for file in needs_hash(files, min_copies) {
        if cancelled.load(Ordering::Acquire) {
            return Vec::new();
        }
        // Unreadable, non-regular or changing files fail closed.
        let digest = crate::verified_hash::file(&file.path);
        hashed.fetch_add(1, Ordering::Relaxed);
        let Ok(digest) = digest else {
            issues.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let mut prefix = [0_u8; 8];
        prefix.copy_from_slice(&digest[..8]);
        keys.push(FileKey {
            path: file.path.clone(),
            identity: file.identity.clone(),
            size: file.size,
            hash: u64::from_be_bytes(prefix),
            modified: file.modified,
        });
        digests.insert(file.path.clone(), digest);
    }
    crate::dedup::group_duplicates(&keys)
        .into_iter()
        .flat_map(|group| confirm_group(group, &digests))
        .filter(|group| group.files.len() >= min_copies.get())
        .collect()
}

/// Split a prefix-matched group into clusters with the same full digest,
/// keeping only those with two or more members. Guards against 64-bit
/// prefix collisions being trashed as duplicates.
pub fn confirm_group(
    group: DupGroup,
    digests: &HashMap<PathBuf, crate::verified_hash::VerifiedHash>,
) -> Vec<DupGroup> {
    let size = group.size;
    let mut clusters: Vec<(crate::verified_hash::VerifiedHash, Vec<FileKey>)> = Vec::new();
    for file in group.files {
        let Some(digest) = digests.get(&file.path).copied() else {
            continue;
        };
        if let Some((_, cluster)) = clusters.iter_mut().find(|(other, _)| *other == digest) {
            cluster.push(file);
        } else {
            clusters.push((digest, vec![file]));
        }
    }
    clusters
        .into_iter()
        .filter(|(_, files)| files.len() >= 2)
        .map(|(_, files)| DupGroup { size, files })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn entry(path: PathBuf) -> FileEntry {
        let meta = std::fs::metadata(&path).unwrap();
        FileEntry::from_meta(path, &meta).unwrap()
    }

    #[test]
    fn size_range_validates_exact_inclusive_byte_limits_without_lossy_numbers() {
        assert_eq!(SizeRange::parse("", " ").unwrap(), SizeRange::default());
        let exact = SizeRange::parse(" 10 ", "10").unwrap();
        assert!(!exact.contains(9));
        assert!(exact.contains(10));
        assert!(!exact.contains(11));
        assert!(!SizeRange::default().contains(0));
        assert!(!SizeRange::parse("0", "0").unwrap().contains(1));
        let largest = SizeRange::parse("18446744073709551615", "18446744073709551615").unwrap();
        assert!(largest.contains(u64::MAX));
        for (min, max) in [
            ("11", "10"),
            ("-1", ""),
            ("+1", ""),
            ("1.0", ""),
            ("1e3", ""),
            ("1 KiB", ""),
            ("１２", ""),
            ("", "18446744073709551616"),
            ("100000000000000000000", ""),
            ("1 0", ""),
        ] {
            assert!(SizeRange::parse(min, max).is_err(), "{min:?} {max:?}");
        }
        assert_eq!(exact.label(), "Applied size: 10–10 bytes (inclusive)");
    }

    #[test]
    fn listed_size_limits_exclude_unreadable_outside_candidates_before_hashing() {
        let dir = TempDir::new();
        let small = [dir.file("a", "0123456789"), dir.file("b", "0123456789")];
        let large = [
            dir.file("large-a", &"x".repeat(64)),
            dir.file("large-b", &"x".repeat(64)),
        ];
        let files: Vec<_> = small.iter().chain(&large).cloned().map(entry).collect();
        for path in &large {
            std::fs::remove_file(path).unwrap();
        }
        let scan = spawn(files.clone(), SizeRange::parse("10", "10").unwrap(), || {});
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].size, 10);
        assert_eq!(scan.total(), 2);
        assert_eq!(scan.hashed(), 2);
        assert_eq!(scan.issues(), 0);
        let scan = spawn(files, SizeRange::parse("64", "").unwrap(), || {});
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.total(), 2);
        assert_eq!(scan.hashed(), 2);
        assert_eq!(scan.issues(), 2);
    }

    #[test]
    fn several_roots_share_name_size_copy_and_physical_identity_filters() {
        let dir = TempDir::new();
        let roots: Vec<_> = ["a", "b", "c"].into_iter().map(|n| dir.dir(n)).collect();
        for name in ["a/cross.bin", "b/cross.bin", "c/cross.bin", "c/skip.bin"] {
            dir.file(name, "0123456789");
        }
        for name in ["a/local1.bin", "a/local2.bin"] {
            dir.file(name, &"L".repeat(64));
        }
        std::fs::hard_link(roots[0].join("cross.bin"), roots[1].join("alias.bin")).unwrap();
        let names = crate::duplicate_name_filter::Rules {
            include: "*.bin".into(),
            exclude: "skip*".into(),
            case: false,
            extension: Default::default(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        let scan = spawn_roots_filtered(
            vec![
                roots[0].clone(),
                roots[1].clone(),
                roots[2].clone(),
                roots[0].clone(),
            ],
            SizeRange {
                min: 10,
                max: Some(10),
            },
            names.clone(),
            Some(0),
            MinCopies::parse("3").unwrap(),
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 3);
        assert_eq!(scan.hashed(), 3);
        assert_eq!(scan.issues(), 0);
        let scan = spawn_roots_filtered(
            roots.clone(),
            SizeRange {
                min: 10,
                max: Some(10),
            },
            names,
            Some(0),
            MinCopies::parse("4").unwrap(),
            || {},
        );
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.hashed(), 0);
        let scan = spawn_roots_filtered(
            vec![roots[0].clone(), dir.path().join("missing")],
            SizeRange::default(),
            Default::default(),
            Some(0),
            Default::default(),
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].size, 64);
        assert_eq!(scan.issues(), 1);
    }
    #[test]
    fn overlapping_root_depth_is_order_independent_without_duplicate_paths() {
        let dir = TempDir::new();
        for name in [
            "p.dat",
            "child/c.dat",
            "child/grand/g.dat",
            "child/grand/deep/d.dat",
        ] {
            dir.file(name, "same data");
        }
        let parent = dir.path().to_path_buf();
        let child = parent.join("child");
        for roots in [
            vec![child.clone(), parent.clone()],
            vec![parent.clone(), child.clone()],
        ] {
            let scan = spawn_roots_filtered(
                roots,
                SizeRange::default(),
                Default::default(),
                Some(1),
                Default::default(),
                || {},
            );
            let groups = finish(&scan);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].files.len(), 3);
            assert_eq!(scan.hashed(), 3);
            assert_eq!(scan.issues(), 0);
            let paths: std::collections::HashSet<_> =
                groups[0].files.iter().map(|f| f.path.clone()).collect();
            assert_eq!(paths.len(), 3);
            assert!(paths.contains(&parent.join("child/grand/g.dat")));
            assert!(!paths.contains(&parent.join("child/grand/deep/d.dat")));
        }
        let scan = spawn_roots_filtered(
            vec![parent.clone(), parent],
            SizeRange::default(),
            Default::default(),
            None,
            Default::default(),
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups[0].files.len(), 4);
        assert_eq!(scan.hashed(), 4);
    }

    #[test]
    fn minimum_copy_counts_validate_defaults_bounds_and_reject_unique_mode() {
        assert_eq!(MinCopies::parse(" ").unwrap().get(), 2);
        assert_eq!(MinCopies::parse("3").unwrap().get(), 3);
        assert_eq!(
            MinCopies::parse("4294967295").unwrap().get(),
            u32::MAX as usize
        );
        for raw in [
            "0",
            "1",
            "-2",
            "+3",
            "3.0",
            "4294967296",
            "99999999999",
            "three",
        ] {
            assert!(MinCopies::parse(raw).is_err(), "{raw}");
        }
    }
    #[test]
    fn minimum_copies_prunes_size_buckets_after_name_and_identity_filters_in_both_scopes() {
        let dir = TempDir::new();
        let mut files = Vec::new();
        for name in ["a1.bin", "a2.bin", "a3.bin"] {
            files.push(FileEntry::observe_lexical(dir.file(name, "0123456789")).unwrap());
        }
        for name in ["b1.bin", "b2.bin"] {
            files.push(FileEntry::observe_lexical(dir.file(name, &"L".repeat(64))).unwrap());
        }
        dir.file("nested/b3.bin", &"L".repeat(64));
        dir.file("skip.bin", &"L".repeat(64));
        let alias = dir.path().join("a-link.bin");
        std::fs::hard_link(dir.path().join("a1.bin"), &alias).unwrap();
        files.push(FileEntry::observe_lexical(alias).unwrap());
        let names = crate::duplicate_name_filter::Rules {
            include: "*.bin".into(),
            exclude: "skip*".into(),
            case: false,
            extension: Default::default(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        let three = MinCopies::parse("3").unwrap();
        let scan = spawn_filtered(files, SizeRange::default(), names.clone(), three, || {});
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].size, 10);
        assert_eq!(groups[0].files.len(), 3);
        assert_eq!(scan.total(), 3);
        assert_eq!(scan.hashed(), 3);
        assert_eq!(scan.issues(), 0);
        let scan = spawn_tree_filtered(
            dir.path().to_owned(),
            SizeRange::default(),
            names.clone(),
            None,
            three,
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.files.len() == 3));
        assert_eq!(scan.hashed(), 6);
        let scan = spawn_tree_filtered(
            dir.path().to_owned(),
            SizeRange::default(),
            names.clone(),
            Some(0),
            three,
            || {},
        );
        assert_eq!(finish(&scan).len(), 1);
        assert_eq!(scan.hashed(), 3);
        let scan = spawn_tree_filtered(
            dir.path().to_owned(),
            SizeRange {
                min: 64,
                max: Some(64),
            },
            names.clone(),
            None,
            three,
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].size, 64);
        assert_eq!(scan.hashed(), 3);
        let scan = spawn_tree_filtered(
            dir.path().to_owned(),
            SizeRange::default(),
            names,
            None,
            MinCopies::parse("4").unwrap(),
            || {},
        );
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.total(), 0);
        assert_eq!(scan.hashed(), 0);
    }
    #[test]
    fn duplicate_relative_paths_keep_root_scope_thresholds_and_representatives() {
        use crate::duplicate_name_group::Component;
        let dir = TempDir::new();
        let a = dir.dir("a");
        let b = dir.dir("b");
        let c = dir.dir("a/nested");
        let d = dir.dir("d");
        for name in [
            "a/docs/report.bin",
            "b/docs/report.bin",
            "a/nested/docs/report.bin",
            "a/misc/report.bin",
            "b/else/report.bin",
            "d/DOCS/REPORT.BIN",
        ] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "same-data!!").unwrap();
        }
        let unique = dir.file("b/docs/unique.bin", "other-data!");
        let rules = crate::duplicate_name_filter::Rules {
            grouping: Component::RelativePath,
            ..Default::default()
        };
        for (fold, min, expected) in [
            (false, 2, 3),
            (true, 2, 4),
            (false, 3, 3),
            (true, 4, 4),
            (false, 4, 0),
        ] {
            let names = crate::duplicate_name_filter::Rules {
                grouping_ignore_ascii_case: fold,
                ..rules.clone()
            }
            .compile()
            .unwrap();
            let scan = spawn_roots_selected(
                vec![a.clone(), b.clone(), c.clone(), d.clone()],
                Default::default(),
                names,
                None,
                MinCopies::parse(&min.to_string()).unwrap(),
                true,
                || {},
            );
            let groups = finish(&scan);
            assert_eq!(
                groups.iter().map(|g| g.files.len()).sum::<usize>(),
                expected,
                "{fold}:{min}"
            );
            assert_eq!(scan.issues(), 0);
            assert!(
                !groups
                    .iter()
                    .flat_map(|g| &g.files)
                    .any(|f| f.path == unique)
            );
        }
        // Captured listing does not enumerate extra files, and rejects unowned paths visibly.
        let root = dir.dir("listing");
        let x = dir.file("listing/x.bin", "same-data!!");
        let y = dir.file("listing/y.bin", "same-data!!");
        dir.dir("listing/nested");
        dir.file("listing/nested/unseen.bin", "same-data!!");
        let rows = [x, y, unique]
            .into_iter()
            .map(|p| FileEntry::observe_lexical(p).unwrap())
            .collect::<Vec<_>>();
        let scan = spawn_filtered_at(
            root.clone(),
            rows.clone(),
            Default::default(),
            rules.compile().unwrap(),
            Default::default(),
            || {},
        );
        assert!(!scan.enumerating());
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.issues(), 1);
        assert_eq!(scan.total(), 2);
        assert_eq!(scan.hashed(), 2);
        let scan = spawn_filtered(
            rows,
            Default::default(),
            rules.compile().unwrap(),
            Default::default(),
            || {},
        );
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.issues(), 3);
        #[cfg(unix)]
        {
            let links = TempDir::new();
            let one = links.dir("one");
            let two = links.dir("two");
            let original = links.file("one/a-original.bin", "same-data!!");
            let alias = one.join("copy.bin");
            std::fs::hard_link(&original, &alias).unwrap();
            links.file("two/copy.bin", "same-data!!");
            let scan = spawn_roots_selected(
                vec![one.clone(), two.clone()],
                Default::default(),
                crate::duplicate_name_filter::Rules {
                    include: "copy.bin".into(),
                    ..rules.clone()
                }
                .compile()
                .unwrap(),
                None,
                Default::default(),
                true,
                || {},
            );
            let groups = finish(&scan);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].files.len(), 2);
            let scan = spawn_roots_selected(
                vec![one, two],
                Default::default(),
                rules.compile().unwrap(),
                None,
                Default::default(),
                true,
                || {},
            );
            let groups = finish(&scan);
            // The earlier a-original representative prevents the differently named alias from forming another copy.
            assert!(groups.is_empty());
        }
    }

    #[test]
    fn duplicate_name_components_apply_to_verified_listing_tree_and_thresholds() {
        use crate::duplicate_name_group::Component as C;
        let dir = TempDir::new();
        let names = [
            "A/album/report.txt",
            "B/album/report.txt",
            "C/Album/REPORT.TXT",
            "D/other/report.bin",
            "E/other/another.bin",
        ];
        let mut files = Vec::new();
        for name in names.into_iter().chain(["F/third/unique.txt"]) {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                if name.ends_with("unique.txt") {
                    "other-data!"
                } else {
                    "same-data!!"
                },
            )
            .unwrap();
            files.push(FileEntry::observe_lexical(path).unwrap());
        }
        for tree in [false, true] {
            for (grouping, ignore, minimum, extension, expected) in [
                (
                    C::Any,
                    false,
                    2,
                    crate::duplicate_extension::Mode::Any,
                    vec![5],
                ),
                (
                    C::Basename,
                    false,
                    2,
                    crate::duplicate_extension::Mode::Any,
                    vec![2],
                ),
                (
                    C::Basename,
                    true,
                    2,
                    crate::duplicate_extension::Mode::Any,
                    vec![3],
                ),
                (
                    C::Stem,
                    false,
                    2,
                    crate::duplicate_extension::Mode::Any,
                    vec![3],
                ),
                (
                    C::Stem,
                    true,
                    2,
                    crate::duplicate_extension::Mode::Any,
                    vec![4],
                ),
                (
                    C::ParentName,
                    false,
                    2,
                    crate::duplicate_extension::Mode::Any,
                    vec![2, 2],
                ),
                (
                    C::ParentName,
                    true,
                    2,
                    crate::duplicate_extension::Mode::Any,
                    vec![2, 3],
                ),
                (
                    C::ParentName,
                    false,
                    3,
                    crate::duplicate_extension::Mode::Any,
                    vec![],
                ),
                (
                    C::ParentName,
                    true,
                    3,
                    crate::duplicate_extension::Mode::Any,
                    vec![3],
                ),
                (
                    C::Stem,
                    true,
                    2,
                    crate::duplicate_extension::Mode::CaseSensitive,
                    vec![2],
                ),
            ] {
                let names = crate::duplicate_name_filter::Rules {
                    grouping,
                    grouping_ignore_ascii_case: ignore,
                    extension,
                    ..Default::default()
                }
                .compile()
                .unwrap();
                let minimum = MinCopies::parse(&minimum.to_string()).unwrap();
                let scan = if tree {
                    spawn_tree_filtered(
                        dir.path().to_owned(),
                        Default::default(),
                        names,
                        None,
                        minimum,
                        || {},
                    )
                } else {
                    spawn_filtered(files.clone(), Default::default(), names, minimum, || {})
                };
                let groups = finish(&scan);
                let mut counts = groups.iter().map(|g| g.files.len()).collect::<Vec<_>>();
                counts.sort_unstable();
                assert_eq!(
                    counts, expected,
                    "{tree}:{grouping:?}:{ignore}:{extension:?}"
                );
                assert_eq!(scan.issues(), 0);
                assert!(
                    !groups
                        .iter()
                        .flat_map(|g| &g.files)
                        .any(|f| f.path.ends_with("unique.txt"))
                );
            }
        }
    }
    #[test]
    fn duplicate_name_components_preserve_cross_root_and_hardlink_ownership() {
        use crate::duplicate_name_group::Component as C;
        let dir = TempDir::new();
        let a = dir.dir("a");
        let b = dir.dir("b");
        dir.dir("a/nested");
        for name in [
            "a/report.txt",
            "a/nested/report.txt",
            "b/other.txt",
            "b/else.txt",
        ] {
            dir.file(name, "same-data!!");
        }
        let rules = crate::duplicate_name_filter::Rules {
            grouping: C::Basename,
            ..Default::default()
        };
        let scan = spawn_roots_selected(
            vec![a.clone(), b.clone()],
            Default::default(),
            rules.compile().unwrap(),
            None,
            Default::default(),
            true,
            || {},
        );
        assert!(finish(&scan).is_empty());
        dir.file("b/report.txt", "same-data!!");
        let scan = spawn_roots_selected(
            vec![a, b],
            Default::default(),
            rules.compile().unwrap(),
            None,
            MinCopies::parse("3").unwrap(),
            true,
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 3);
        #[cfg(unix)]
        {
            let links = TempDir::new();
            for name in ["a", "b", "c"] {
                links.dir(name);
            }
            let original = links.file("a/original.txt", "same-data!!");
            let alias = links.path().join("b/report.bin");
            std::fs::hard_link(&original, &alias).unwrap();
            let copy = links.file("c/report.bin", "same-data!!");
            let rows = [original, alias, copy]
                .into_iter()
                .map(|p| FileEntry::observe_lexical(p).unwrap())
                .collect::<Vec<_>>();
            assert!(
                finish(&spawn_filtered(
                    rows.clone(),
                    Default::default(),
                    rules.compile().unwrap(),
                    Default::default(),
                    || {}
                ))
                .is_empty()
            );
            let filter = crate::duplicate_name_filter::Rules {
                include: "*.bin".into(),
                ..rules
            }
            .compile()
            .unwrap();
            let groups = finish(&spawn_filtered(
                rows,
                Default::default(),
                filter,
                Default::default(),
                || {},
            ));
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].files.len(), 2);
        }
    }

    #[test]
    fn duplicate_extensions_partition_verified_listing_tree_and_cross_root_results() {
        use crate::duplicate_extension::Mode;
        let dir = TempDir::new();
        let names = [
            "a.txt", "b.txt", "c.TXT", "d.bin", "e.bin", "bare-a", "bare-b",
        ];
        let mut files = names
            .iter()
            .map(|n| FileEntry::observe_lexical(dir.file(n, "same ten!!")).unwrap())
            .collect::<Vec<_>>();
        files.push(FileEntry::observe_lexical(dir.file("unique.txt", "other ten!")).unwrap());
        for tree in [false, true] {
            for (mode, minimum, expected) in [
                (Mode::Any, 2, vec![7]),
                (Mode::CaseSensitive, 2, vec![2, 2, 2]),
                (Mode::IgnoreAsciiCase, 2, vec![2, 2, 3]),
                (Mode::IgnoreAsciiCase, 3, vec![3]),
            ] {
                let filter = crate::duplicate_name_filter::Rules {
                    extension: mode,
                    ..Default::default()
                }
                .compile()
                .unwrap();
                let minimum = MinCopies::parse(&minimum.to_string()).unwrap();
                let scan = if tree {
                    spawn_tree_filtered(
                        dir.path().to_path_buf(),
                        Default::default(),
                        filter,
                        None,
                        minimum,
                        || {},
                    )
                } else {
                    spawn_filtered(files.clone(), Default::default(), filter, minimum, || {})
                };
                let groups = finish(&scan);
                let mut counts = groups.iter().map(|g| g.files.len()).collect::<Vec<_>>();
                counts.sort_unstable();
                assert_eq!(counts, expected, "{tree}:{mode:?}");
                assert_eq!(scan.issues(), 0);
                assert!(
                    !groups
                        .iter()
                        .any(|g| g.files.iter().any(|f| f.path.ends_with("unique.txt")))
                );
            }
        }
        let roots = TempDir::new();
        let a = roots.dir("a");
        let b = roots.dir("b");
        for n in ["a/one.txt", "a/two.txt", "a/one.bin", "b/two.bin"] {
            roots.file(n, "same ten!!");
        }
        let filter = crate::duplicate_name_filter::Rules {
            extension: Mode::CaseSensitive,
            ..Default::default()
        }
        .compile()
        .unwrap();
        let scan = spawn_roots_selected(
            vec![a, b],
            Default::default(),
            filter,
            None,
            Default::default(),
            true,
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 2);
        assert!(
            groups[0]
                .files
                .iter()
                .all(|f| f.path.extension().unwrap() == "bin")
        );
        #[cfg(unix)]
        {
            let links = TempDir::new();
            let original = links.file("original.txt", "same ten!!");
            let alias = links.path().join("alias.bin");
            std::fs::hard_link(&original, &alias).unwrap();
            let copy = links.file("copy.bin", "same ten!!");
            let rows = [original, alias, copy]
                .into_iter()
                .map(|p| FileEntry::observe_lexical(p).unwrap())
                .collect::<Vec<_>>();
            let filter = crate::duplicate_name_filter::Rules {
                extension: Mode::CaseSensitive,
                ..Default::default()
            }
            .compile()
            .unwrap();
            assert!(
                finish(&spawn_filtered(
                    rows.clone(),
                    Default::default(),
                    filter,
                    Default::default(),
                    || {}
                ))
                .is_empty()
            );
            let filter = crate::duplicate_name_filter::Rules {
                include: "*.bin".into(),
                extension: Mode::CaseSensitive,
                ..Default::default()
            }
            .compile()
            .unwrap();
            let groups = finish(&spawn_filtered(
                rows,
                Default::default(),
                filter,
                Default::default(),
                || {},
            ));
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].files.len(), 2);
        }
    }

    #[test]
    fn minimum_copy_threshold_requires_verified_content_not_size_or_failed_candidates() {
        let dir = TempDir::new();
        let mut files = Vec::new();
        for (name, contents) in [
            ("a1", "same A"),
            ("a2", "same A"),
            ("b1", "same B"),
            ("b2", "same B"),
        ] {
            files.push(FileEntry::observe_lexical(dir.file(name, contents)).unwrap());
        }
        let three = MinCopies::parse("3").unwrap();
        let scan = spawn_filtered(
            files,
            SizeRange::default(),
            Default::default(),
            three,
            || {},
        );
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.hashed(), 4);
        assert_eq!(scan.issues(), 0);
        let files: Vec<_> = ["c1", "c2", "c3"]
            .into_iter()
            .map(|name| FileEntry::observe_lexical(dir.file(name, "same C")).unwrap())
            .collect();
        std::fs::remove_file(&files[2].path).unwrap();
        let scan = spawn_filtered(
            files,
            SizeRange::default(),
            Default::default(),
            three,
            || {},
        );
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.hashed(), 3);
        assert_eq!(scan.issues(), 1);
    }

    #[test]
    fn depth_drafts_validate_zero_unlimited_boundaries_and_reject_nonintegers() {
        assert_eq!(parse_depth(""), Ok(None));
        assert_eq!(parse_depth("  "), Ok(None));
        assert_eq!(parse_depth(" 0 "), Ok(Some(0)));
        assert_eq!(parse_depth("1"), Ok(Some(1)));
        assert_eq!(parse_depth("4294967295"), Ok(Some(u32::MAX)));
        for raw in ["-1", "+1", "1.5", "4294967296", "99999999999", "one", "١"] {
            assert!(parse_depth(raw).is_err(), "{raw}");
        }
    }
    #[test]
    fn depth_boundaries_compose_with_name_size_identity_and_hash_totals() {
        let dir = TempDir::new();
        for name in [
            "a.dat",
            "b.dat",
            "one/c.dat",
            "one/two/d.dat",
            "one/two/three/e.dat",
            "one/two/three/skip.dat",
            "one/two/three/f.txt",
        ] {
            dir.file(name, "0123456789");
        }
        std::fs::hard_link(
            dir.path().join("a.dat"),
            dir.path().join("one/two/three/link.dat"),
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path(), dir.path().join("one/loop")).unwrap();
        let names = crate::duplicate_name_filter::Rules {
            include: "*.dat".into(),
            exclude: "skip*".into(),
            case: false,
            extension: Default::default(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        for (depth, count) in [
            (Some(0), 2),
            (Some(1), 3),
            (Some(2), 4),
            (None, 5),
            (Some(u32::MAX), 5),
        ] {
            let scan = spawn_tree_filtered(
                dir.path().to_owned(),
                SizeRange {
                    min: 10,
                    max: Some(10),
                },
                names.clone(),
                depth,
                MinCopies::default(),
                || {},
            );
            let groups = finish(&scan);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].files.len(), count, "{depth:?}");
            assert_eq!(scan.total(), count);
            assert_eq!(scan.hashed(), count);
            assert_eq!(scan.issues(), 0);
            if let Some(limit) = depth {
                assert!(groups[0].files.iter().all(|f| {
                    f.path
                        .strip_prefix(dir.path())
                        .unwrap()
                        .components()
                        .count()
                        <= limit as usize + 1
                }));
            }
        }
        let issues = AtomicUsize::new(0);
        assert!(
            tree_candidates_filtered(
                dir.path(),
                SizeRange::default(),
                &names,
                Some(0),
                &AtomicBool::new(true),
                &issues
            )
            .is_empty()
        );
        assert_eq!(issues.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn name_filters_precede_hashing_and_identity_collapse_in_pane_and_tree_scopes() {
        let dir = TempDir::new();
        let alias = dir.file("alias.bin", "same bytes");
        let wanted = dir.path().join("wanted.JPG");
        std::fs::hard_link(&alias, &wanted).unwrap();
        let copy = dir.file("copy.jpg", "same bytes");
        let skipped = dir.file("skip.jpg", "same bytes");
        let missing = dir.file("missing.txt", "same bytes");
        let nested = dir.file("skip-folder.jpg/child.jpg", "same bytes");
        let files: Vec<_> = [
            alias,
            wanted.clone(),
            copy.clone(),
            skipped,
            missing.clone(),
        ]
        .into_iter()
        .map(|path| FileEntry::observe_lexical(path).unwrap())
        .collect();
        std::fs::remove_file(missing).unwrap();
        let filter = crate::duplicate_name_filter::Rules {
            include: "*.jpg".into(),
            exclude: "skip*".into(),
            case: false,
            extension: Default::default(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        let scan = spawn_filtered(
            files,
            SizeRange::default(),
            filter.clone(),
            MinCopies::default(),
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 2);
        assert_eq!(scan.total(), 2);
        assert_eq!(scan.hashed(), 2);
        assert_eq!(scan.issues(), 0);
        assert!(groups[0].files.iter().any(|f| f.path == wanted));
        let scan = spawn_tree_filtered(
            dir.path().to_owned(),
            SizeRange::default(),
            filter,
            None,
            MinCopies::default(),
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 3);
        assert_eq!(scan.total(), 3);
        assert_eq!(scan.hashed(), 3);
        assert_eq!(scan.issues(), 0);
        assert!(groups[0].files.iter().any(|f| f.path == nested));
        let filter = crate::duplicate_name_filter::Rules {
            include: "*.jpg".into(),
            exclude: "skip*".into(),
            case: true,
            extension: Default::default(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        let scan = spawn_tree_filtered(
            dir.path().to_owned(),
            SizeRange {
                min: 10,
                max: Some(10),
            },
            filter,
            None,
            MinCopies::default(),
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups[0].files.len(), 2);
        assert_eq!(scan.hashed(), 2);
    }

    #[test]
    fn recursive_size_limits_are_applied_before_hash_totals_and_include_boundaries() {
        let dir = TempDir::new();
        for name in ["a", "nested/b"] {
            dir.file(name, "0123456789");
        }
        for name in ["large-a", "nested/large-b"] {
            dir.file(name, &"x".repeat(64));
        }
        dir.file("empty", "");
        dir.file("nested/empty", "");
        for (min, max, size) in [("10", "10", 10), ("11", "64", 64)] {
            let scan = spawn_tree(
                dir.path().to_path_buf(),
                SizeRange::parse(min, max).unwrap(),
                || {},
            );
            let groups = finish(&scan);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].size, size);
            assert_eq!(groups[0].files.len(), 2);
            assert_eq!(scan.total(), 2);
            assert_eq!(scan.hashed(), 2);
            assert_eq!(scan.issues(), 0);
        }
        let scan = spawn_tree(
            dir.path().to_path_buf(),
            SizeRange::parse("11", "63").unwrap(),
            || {},
        );
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.total(), 0);
        assert_eq!(scan.hashed(), 0);
        assert_eq!(scan.issues(), 0);
    }

    #[test]
    fn cross_root_filter_checks_content_and_prunes_local_sizes_before_hashing() {
        let dir = TempDir::new();
        for (path, content) in [
            ("a/one", "shared"),
            ("a/two", "shared"),
            ("b/three", "shared"),
            ("a/local1", "local-only"),
            ("a/local2", "local-only"),
            ("a/different", "aaaaaa"),
            ("b/different", "bbbbbb"),
        ] {
            dir.file(path, content);
        }
        let scan = spawn_roots_selected(
            vec![dir.path().join("a"), dir.path().join("b")],
            SizeRange::default(),
            Default::default(),
            None,
            MinCopies::parse("3").unwrap(),
            true,
            || {},
        );
        let groups = finish(&scan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 3);
        assert_eq!(scan.total(), 5);
        assert_eq!(scan.hashed(), 5);
        assert_eq!(scan.issues(), 0);
        assert!(groups[0].files.iter().all(|f| f.size == 6));
    }

    #[test]
    fn cross_root_ownership_handles_nested_roots_and_does_not_count_aliases() {
        let dir = TempDir::new();
        dir.file("inside/local1", "nested-only");
        dir.file("inside/local2", "nested-only");
        let outer = dir.file("outer", "shared");
        dir.file("inside/independent", "shared");
        std::fs::hard_link(&outer, dir.path().join("inside/alias")).unwrap();
        for roots in [
            vec![dir.path().to_owned(), dir.path().join("inside")],
            vec![dir.path().join("inside"), dir.path().to_owned()],
        ] {
            let scan = spawn_roots_selected(
                roots,
                Default::default(),
                Default::default(),
                None,
                Default::default(),
                true,
                || {},
            );
            let groups = finish(&scan);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].files.len(), 2);
            assert!(groups[0].files.iter().any(|f| f.path == outer));
            assert_eq!(scan.total(), 2);
        }
        let scan = spawn_roots_selected(
            vec![dir.path().to_owned(), dir.path().to_owned()],
            Default::default(),
            Default::default(),
            None,
            Default::default(),
            true,
            || {},
        );
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.hashed(), 0);
    }

    fn finish(scan: &DupScan) -> Vec<DupGroup> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(groups) = scan.poll() {
                return groups;
            }
            assert!(std::time::Instant::now() < deadline, "scan timed out");
            std::thread::yield_now();
        }
    }

    #[cfg(unix)]
    #[test]
    fn tree_scan_discovers_hidden_nested_files_collapses_links_and_refreshes() {
        let dir = TempDir::new();
        let outside = TempDir::new();
        let original = dir.file("original", "same data");
        let nested = dir.file("nested/copy", "same data");
        let hidden = dir.file(".hidden/deep/copy", "same data");
        dir.file("empty", "");
        dir.file("nested/empty", "");
        dir.file("unique", "different content");
        let target = outside.file("external", "same data");
        std::os::unix::fs::symlink(&target, dir.path().join("file-link")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("directory-link")).unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
        std::fs::hard_link(&original, dir.path().join("nested/zz-hardlink")).unwrap();
        let scan = spawn_tree(dir.path().to_path_buf(), SizeRange::default(), || {});
        let groups = finish(&scan);
        assert_eq!(scan.issues(), 0);
        assert!(!scan.enumerating());
        assert_eq!(scan.total(), 3);
        assert_eq!(scan.hashed(), 3);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 3);
        for path in [&original, &nested, &hidden] {
            assert!(groups[0].files.iter().any(|file| &file.path == path));
        }
        assert_eq!(groups[0].size, 9);
        assert!(
            groups[0]
                .files
                .iter()
                .all(|file| matches!(file.identity, crate::panel::ListingIdentity::Captured(_)))
        );
        std::fs::write(&hidden, "different").unwrap();
        let added = dir.file("new/deep/new-copy", "same data");
        let scan = spawn_tree(dir.path().to_path_buf(), SizeRange::default(), || {});
        let groups = finish(&scan);
        assert_eq!(scan.issues(), 0);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 3);
        assert!(groups[0].files.iter().any(|file| file.path == added));
        assert!(!groups[0].files.iter().any(|file| file.path == hidden));
        assert_eq!(std::fs::read(target).unwrap(), b"same data");
    }

    #[test]
    fn tree_scan_reports_missing_or_invalid_root_and_obeys_cancellation() {
        let dir = TempDir::new();
        let path = dir.file("file", "same data");
        for root in [dir.path().join("missing"), path] {
            let scan = spawn_tree(root, SizeRange::default(), || {});
            assert!(finish(&scan).is_empty());
            assert_eq!(scan.issues(), 1);
            assert_eq!(scan.hashed(), 0);
        }
        let issues = AtomicUsize::new(0);
        assert!(
            tree_candidates(
                dir.path(),
                SizeRange::default(),
                &AtomicBool::new(true),
                &issues
            )
            .is_empty()
        );
        assert_eq!(issues.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn scan_counts_failed_hashes_without_offering_unverified_groups() {
        let dir = TempDir::new();
        let a = dir.file("a", "same");
        let b = dir.file("b", "same");
        let files = vec![entry(a.clone()), entry(b)];
        std::fs::remove_file(a).unwrap();
        let scan = spawn(files, SizeRange::default(), || {});
        assert!(finish(&scan).is_empty());
        assert_eq!(scan.issues(), 1);
        assert_eq!(scan.hashed(), 2);
    }

    #[test]
    fn background_scan_reports_identical_files() {
        let dir = TempDir::new();
        let files = vec![
            entry(dir.file("a.txt", "same content")),
            entry(dir.file("b.txt", "same content")),
            entry(dir.file("c.txt", "diff content")),
            entry(dir.file("d.txt", "x")),
        ];
        let scan = spawn(files, SizeRange::default(), || {});
        assert_eq!(scan.total(), 3, "d.txt has a unique size");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let groups = loop {
            if let Some(groups) = scan.poll() {
                break groups;
            }
            assert!(std::time::Instant::now() < deadline, "scan never finished");
            std::thread::yield_now();
        };
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 2);
        assert_eq!(scan.hashed(), 3);
    }

    #[test]
    fn cancelled_scan_hashes_nothing_and_reports_nothing() {
        let dir = TempDir::new();
        let files = vec![
            entry(dir.file("a.txt", "same content")),
            entry(dir.file("b.txt", "same content")),
        ];
        let hashed = AtomicUsize::new(0);
        assert!(hash_duplicates(&files, &AtomicBool::new(true), &hashed).is_empty());
        assert_eq!(hashed.load(Ordering::Relaxed), 0);
    }
}
