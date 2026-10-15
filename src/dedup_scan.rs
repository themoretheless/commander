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

/// A running scan. Dropping it cancels the worker.
pub struct DupScan {
    receiver: Receiver<Vec<DupGroup>>,
    cancelled: Arc<AtomicBool>,
    hashed: Arc<AtomicUsize>,
    /// Files whose size collides and so need hashing.
    pub to_hash: usize,
}

impl DupScan {
    /// The groups once the worker finished; `None` while it runs. A worker
    /// that died reports no groups rather than scanning forever.
    pub fn poll(&self) -> Option<Vec<DupGroup>> {
        match self.receiver.try_recv() {
            Ok(groups) => Some(groups),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Vec::new()),
        }
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
pub fn spawn(candidates: Vec<FileEntry>, notify: impl Fn() + Send + 'static) -> DupScan {
    let cancelled = Arc::new(AtomicBool::new(false));
    let hashed = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    let to_hash = needs_hash(&candidates).count();
    let worker_cancelled = Arc::clone(&cancelled);
    let worker_hashed = Arc::clone(&hashed);
    std::thread::spawn(move || {
        let groups = hash_duplicates(&candidates, &worker_cancelled, &worker_hashed);
        if !worker_cancelled.load(Ordering::Acquire) {
            let _ = sender.send(groups);
            notify();
        }
    });
    DupScan {
        receiver,
        cancelled,
        hashed,
        to_hash,
    }
}

/// Candidates whose size is shared with another candidate; a unique size
/// cannot have a duplicate.
fn needs_hash(files: &[FileEntry]) -> impl Iterator<Item = &FileEntry> {
    let mut size_counts: HashMap<u64, usize> = HashMap::new();
    for file in files {
        *size_counts.entry(file.size).or_insert(0) += 1;
    }
    files
        .iter()
        .filter(move |file| size_counts.get(&file.size).copied().unwrap_or(0) >= 2)
}

/// Group byte-identical `files`: size-prefiltered, grouped by (size, digest
/// prefix), then confirmed by the full 256-bit digest (a 64-bit prefix match
/// is not proof), so the dialog never offers to trash a false positive. Each
/// candidate is read once. A cancelled scan stops early and returns nothing.
pub fn hash_duplicates(
    files: &[FileEntry],
    cancelled: &AtomicBool,
    hashed: &AtomicUsize,
) -> Vec<DupGroup> {
    let mut keys: Vec<FileKey> = Vec::new();
    let mut digests: HashMap<PathBuf, crate::verified_hash::VerifiedHash> = HashMap::new();
    for file in needs_hash(files) {
        if cancelled.load(Ordering::Acquire) {
            return Vec::new();
        }
        // Unreadable, non-regular or changing files fail closed.
        let digest = crate::verified_hash::file(&file.path);
        hashed.fetch_add(1, Ordering::Relaxed);
        let Ok(digest) = digest else {
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
    fn background_scan_reports_identical_files() {
        let dir = TempDir::new();
        let files = vec![
            entry(dir.file("a.txt", "same content")),
            entry(dir.file("b.txt", "same content")),
            entry(dir.file("c.txt", "diff content")),
            entry(dir.file("d.txt", "x")),
        ];
        let scan = spawn(files, || {});
        assert_eq!(scan.to_hash, 3, "d.txt has a unique size");
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
