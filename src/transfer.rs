//! Background transfer engine: copy/move with progress reporting.
//!
//! This module is UI-agnostic. Progress is shared through [`TransferState`]
//! and the caller supplies a `notify` callback (e.g. a repaint request), so
//! the engine never depends on egui.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::fs_util;
use crate::panel::FileEntry;

const COPY_BUF_SIZE: usize = 1024 * 1024; // 1 MB buffer

/// What to do when destination file already exists.
#[derive(Clone, Copy, PartialEq)]
pub enum OverwritePolicy {
    Ask,
    OverwriteAll,
    SkipAll,
}

/// Copy strategy.
#[derive(Clone, Copy, PartialEq)]
pub enum CopyMethod {
    /// Byte-by-byte with 1MB buffer, full progress tracking.
    Buffered,
    /// Native macOS copyfile() with APFS clone support, xattr/ACL preservation.
    Native,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TransferKind {
    Copy,
    Move,
}

/// Live transfer progress shared between background thread and UI.
#[derive(Clone)]
pub struct TransferProgress {
    pub total_bytes: u64,
    pub copied_bytes: u64,
    pub current_file: String,
    pub current_file_size: u64,
    pub current_file_copied: u64,
    pub files_done: usize,
    pub files_total: usize,
    pub speed_samples: Vec<(f64, f64)>, // (timestamp_secs, bytes_at_that_time)
    pub started_at: std::time::Instant,
    pub finished: bool,
    pub cancelled: bool,
    /// Per-file failures collected during the transfer.
    pub errors: Vec<String>,
}

pub type TransferState = Arc<Mutex<TransferProgress>>;

impl TransferProgress {
    pub fn new(total_bytes: u64, files_total: usize) -> Self {
        Self {
            total_bytes,
            copied_bytes: 0,
            current_file: String::new(),
            current_file_size: 0,
            current_file_copied: 0,
            files_done: 0,
            files_total,
            speed_samples: vec![(0.0, 0.0)],
            started_at: std::time::Instant::now(),
            finished: false,
            cancelled: false,
            errors: Vec::new(),
        }
    }

    /// Current speed in bytes/sec (averaged over last 2 seconds).
    pub fn speed_bps(&self) -> f64 {
        self.speed_bps_at(self.started_at.elapsed().as_secs_f64())
    }

    /// Same as [`speed_bps`](Self::speed_bps) with an explicit "now"
    /// (seconds since transfer start) so the math is testable.
    fn speed_bps_at(&self, now: f64) -> f64 {
        if self.speed_samples.len() < 2 {
            return 0.0;
        }
        // Find sample ~2 seconds ago
        let window = 2.0;
        let cutoff = now - window;
        let old = self
            .speed_samples
            .iter()
            .rev()
            .find(|(t, _)| *t <= cutoff)
            .unwrap_or(&self.speed_samples[0]);
        let dt = now - old.0;
        if dt < 0.01 {
            return 0.0;
        }
        (self.copied_bytes as f64 - old.1) / dt
    }

    /// Estimated time remaining in seconds.
    pub fn eta_secs(&self) -> f64 {
        let speed = self.speed_bps();
        if speed < 1.0 {
            return 0.0;
        }
        let remaining = self.total_bytes.saturating_sub(self.copied_bytes) as f64;
        remaining / speed
    }

    /// Record a sample if at least 500ms passed since the last one.
    pub fn maybe_sample(&mut self) {
        let now = self.started_at.elapsed().as_secs_f64();
        if self
            .speed_samples
            .last()
            .is_none_or(|&(t, _)| now - t >= 0.5)
        {
            self.record_sample();
        }
    }

    /// Record a speed sample (call periodically from copy thread).
    pub fn record_sample(&mut self) {
        let t = self.started_at.elapsed().as_secs_f64();
        self.speed_samples.push((t, self.copied_bytes as f64));
        // Keep last 120 samples (~60 seconds at 2Hz)
        if self.speed_samples.len() > 120 {
            self.speed_samples.remove(0);
        }
    }
}

impl CopyMethod {
    /// Strategy entry point: copy one top-level entry with this method.
    /// Returns bytes copied (used as the progress base for the next entry).
    fn copy_entry(
        self,
        src: &Path,
        is_dir: bool,
        dest: &Path,
        progress: &TransferState,
        base_bytes: u64,
    ) -> std::io::Result<u64> {
        match self {
            CopyMethod::Native => {
                if is_dir {
                    crate::native_copy::copy_dir_native(src, dest, progress, base_bytes)
                } else {
                    crate::native_copy::copy_file_native(src, dest, progress, base_bytes)
                }
            }
            CopyMethod::Buffered => {
                if is_dir {
                    copy_dir_buffered(src, dest, progress)
                } else {
                    copy_file_buffered(src, dest, progress)
                }
            }
        }
    }
}

/// A copy/move request, fully described and detached from any UI state.
pub struct TransferSpec {
    pub kind: TransferKind,
    pub entries: Vec<FileEntry>,
    pub target: PathBuf,
    pub conflicts: Vec<String>,
    pub policy: OverwritePolicy,
    pub method: CopyMethod,
}

/// Total bytes for all entries (recursively for dirs).
pub fn total_bytes(entries: &[FileEntry]) -> u64 {
    entries
        .iter()
        .map(|e| {
            if e.is_dir {
                fs_util::dir_size_recursive(&e.path)
            } else {
                e.size
            }
        })
        .sum()
}

/// Run the transfer on a background thread.
///
/// `notify` is invoked whenever visible progress changed; the UI passes a
/// repaint request here, keeping this module free of egui types.
pub fn spawn_transfer(
    spec: TransferSpec,
    progress: TransferState,
    notify: impl Fn() + Send + 'static,
) {
    std::thread::spawn(move || {
        let is_move = spec.kind == TransferKind::Move;
        let mut base_bytes: u64 = 0;

        for (i, entry) in spec.entries.iter().enumerate() {
            let dest = spec.target.join(&entry.name);
            let exists = spec.conflicts.contains(&entry.name);

            if exists && spec.policy == OverwritePolicy::SkipAll {
                let mut s = progress.lock().unwrap();
                s.files_done = i + 1;
                continue;
            }

            {
                let mut s = progress.lock().unwrap();
                s.current_file = entry.name.clone();
                if s.cancelled {
                    return;
                }
            }

            if exists && dest.is_dir() {
                let _ = std::fs::remove_dir_all(&dest);
            }

            let errors_before = progress.lock().unwrap().errors.len();

            let result = spec
                .method
                .copy_entry(&entry.path, entry.is_dir, &dest, &progress, base_bytes)
                .map(|b| base_bytes += b);

            if let Err(ref e) = result {
                let mut s = progress.lock().unwrap();
                if s.cancelled {
                    return;
                }
                s.errors.push(format!("{}: {}", entry.name, e));
            }

            // Delete source only when the copy fully succeeded:
            // the native callback skips per-file errors and reports
            // them via the error list instead of the result.
            let copy_clean =
                result.is_ok() && progress.lock().unwrap().errors.len() == errors_before;
            if is_move && copy_clean {
                if entry.is_dir {
                    let _ = std::fs::remove_dir_all(&entry.path);
                } else {
                    let _ = std::fs::remove_file(&entry.path);
                }
            }

            {
                let mut s = progress.lock().unwrap();
                s.files_done = i + 1;
                s.record_sample();
            }
            notify();
        }

        {
            let mut s = progress.lock().unwrap();
            s.finished = true;
            s.record_sample();
        }
        notify();
    });
}

/// Copy a single file with progress reporting (buffered strategy).
/// Removes the partial destination file on any failure.
/// Returns the file size on success.
fn copy_file_buffered(src: &Path, dst: &Path, state: &TransferState) -> std::io::Result<u64> {
    let result = copy_file_buffered_inner(src, dst, state);
    if result.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    result
}

fn copy_file_buffered_inner(src: &Path, dst: &Path, state: &TransferState) -> std::io::Result<u64> {
    let file_size = src.metadata().map(|m| m.len()).unwrap_or(0);

    // Init per-file progress
    {
        let mut s = state.lock().unwrap();
        s.current_file = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        s.current_file_size = file_size;
        s.current_file_copied = 0;
    }

    let mut reader = std::io::BufReader::with_capacity(COPY_BUF_SIZE, std::fs::File::open(src)?);
    let mut writer = std::io::BufWriter::with_capacity(COPY_BUF_SIZE, std::fs::File::create(dst)?);

    if let Ok(meta) = src.metadata() {
        let _ = std::fs::set_permissions(dst, meta.permissions());
    }

    let mut buf = vec![0u8; COPY_BUF_SIZE];

    loop {
        {
            let s = state.lock().unwrap();
            if s.cancelled {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
        }

        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;

        {
            let mut s = state.lock().unwrap();
            s.copied_bytes += n as u64;
            s.current_file_copied += n as u64;
            s.maybe_sample();
        }
    }
    writer.flush()?;
    Ok(file_size)
}

/// Recursively copy a directory with progress (buffered strategy).
/// Returns total bytes copied.
fn copy_dir_buffered(src: &Path, dst: &Path, state: &TransferState) -> std::io::Result<u64> {
    std::fs::create_dir_all(dst)?;
    let mut copied = 0u64;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copied += copy_dir_buffered(&src_path, &dst_path, state)?;
        } else {
            copied += copy_file_buffered(&src_path, &dst_path, state)?;
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn entry_for(path: &Path) -> FileEntry {
        let meta = std::fs::metadata(path).unwrap();
        FileEntry::from_meta(path.to_path_buf(), &meta).unwrap()
    }

    /// Run a transfer to completion and return the final progress state.
    fn run(spec: TransferSpec) -> TransferProgress {
        let total = total_bytes(&spec.entries);
        let progress: TransferState =
            Arc::new(Mutex::new(TransferProgress::new(total, spec.entries.len())));
        spawn_transfer(spec, progress.clone(), || {});

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            {
                let s = progress.lock().unwrap();
                if s.finished {
                    return s.clone();
                }
            }
            assert!(std::time::Instant::now() < deadline, "transfer timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn spec(
        kind: TransferKind,
        method: CopyMethod,
        entries: Vec<FileEntry>,
        target: &Path,
        conflicts: Vec<String>,
        policy: OverwritePolicy,
    ) -> TransferSpec {
        TransferSpec {
            kind,
            entries,
            target: target.to_path_buf(),
            conflicts,
            policy,
            method,
        }
    }

    #[test]
    fn buffered_copy_file_copies_bytes_and_reports_progress() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "hello world");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "hello world"
        );
        assert!(file.exists(), "copy keeps the source");
        assert!(s.errors.is_empty());
        assert_eq!(s.files_done, 1);
        assert_eq!(s.copied_bytes, 11);
    }

    #[test]
    fn native_copy_file_works() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "native");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&file)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "native"
        );
        assert!(s.errors.is_empty());
    }

    #[test]
    fn buffered_copy_dir_recurses() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("folder");
        src.file("folder/one.txt", "1");
        src.file("folder/nested/two.txt", "22");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("folder/one.txt")).unwrap(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("folder/nested/two.txt")).unwrap(),
            "22"
        );
        assert!(s.errors.is_empty());
    }

    #[test]
    fn move_deletes_source_only_on_clean_copy() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "move me");

        run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert!(dst.path().join("a.txt").exists());
        assert!(!file.exists(), "clean move removes the source");
    }

    #[test]
    fn move_keeps_source_when_copy_fails() {
        use std::os::unix::fs::PermissionsExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("folder");
        let secret = src.file("folder/secret.txt", "no read access");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        // Restore permissions so TempDir cleanup works everywhere.
        let _ = std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644));

        assert!(!s.errors.is_empty(), "the failure must be reported");
        assert!(dir.exists(), "source must survive a failed move");
        assert!(secret.exists());
    }

    #[test]
    fn skip_all_leaves_existing_destination_untouched() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "new contents");
        dst.file("a.txt", "old contents");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::SkipAll,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "old contents"
        );
        assert_eq!(s.files_done, 1, "skipped entries still count as processed");
    }

    #[test]
    fn overwrite_all_replaces_existing_file() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "new contents");
        dst.file("a.txt", "old contents");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "new contents"
        );
        assert!(s.errors.is_empty());
    }

    #[test]
    fn failed_buffered_copy_removes_partial_destination() {
        use std::os::unix::fs::PermissionsExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let secret = src.file("secret.txt", "data");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&secret)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        let _ = std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644));

        assert!(!s.errors.is_empty());
        assert!(
            !dst.path().join("secret.txt").exists(),
            "partial destination must be cleaned up"
        );
    }

    #[test]
    fn total_bytes_sums_files_and_dirs() {
        let tmp = TempDir::new();
        let f = tmp.file("a.bin", "12345");
        let d = tmp.dir("folder");
        tmp.file("folder/b.bin", "123");

        let entries = vec![entry_for(&f), entry_for(&d)];
        assert_eq!(total_bytes(&entries), 8);
    }

    #[test]
    fn speed_is_averaged_over_recent_samples() {
        let mut p = TransferProgress::new(1000, 1);
        // 100 bytes/sec: sample at t=0 (0 bytes) and t=4 (400 bytes).
        p.speed_samples = vec![(0.0, 0.0), (4.0, 400.0)];
        p.copied_bytes = 600;
        // At t=6 the 2-second window looks back to the t=4 sample:
        // (600 - 400) / (6 - 4) = 100 B/s.
        assert!((p.speed_bps_at(6.0) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn speed_needs_at_least_two_samples() {
        let p = TransferProgress::new(1000, 1);
        assert_eq!(p.speed_bps_at(5.0), 0.0);
    }
}
