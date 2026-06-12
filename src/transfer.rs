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
        if self.speed_samples.len() < 2 {
            return 0.0;
        }
        let now = self.started_at.elapsed().as_secs_f64();
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
        if self.speed_samples.last().map_or(true, |&(t, _)| now - t >= 0.5) {
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
fn copy_file_buffered(
    src: &Path,
    dst: &Path,
    state: &TransferState,
) -> std::io::Result<u64> {
    let result = copy_file_buffered_inner(src, dst, state);
    if result.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    result
}

fn copy_file_buffered_inner(
    src: &Path,
    dst: &Path,
    state: &TransferState,
) -> std::io::Result<u64> {
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
fn copy_dir_buffered(
    src: &Path,
    dst: &Path,
    state: &TransferState,
) -> std::io::Result<u64> {
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
