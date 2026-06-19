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
    /// Write the incoming entry under a fresh "name copy" name, keeping the
    /// existing destination intact (Finder's "Keep Both").
    KeepBoth,
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
///
/// Note there is no conflict list here: the engine checks the destination
/// live at copy time (the confirmation dialog may sit open while the
/// filesystem changes), driven only by [`OverwritePolicy`].
pub struct TransferSpec {
    pub kind: TransferKind,
    pub entries: Vec<FileEntry>,
    pub target: PathBuf,
    pub policy: OverwritePolicy,
    pub method: CopyMethod,
}

/// Size of one entry: its byte length, or the recursive size of a directory.
fn entry_size(entry: &FileEntry) -> u64 {
    if entry.is_dir {
        fs_util::dir_size_recursive(&entry.path)
    } else {
        entry.size
    }
}

/// Total bytes for all entries (recursively for dirs).
pub fn total_bytes(entries: &[FileEntry]) -> u64 {
    entries.iter().map(entry_size).sum()
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

            {
                let mut s = progress.lock().unwrap();
                s.current_file = entry.name.clone();
                if s.cancelled {
                    break; // fall through to the finished-setter below
                }
            }

            // Advance the progress bar past an entry we are about to skip
            // (self-reference, skip-on-conflict, refused overwrite) and record
            // it as processed. `err` is an optional message to surface.
            let skip_entry = |progress: &TransferState, base: &mut u64, err: Option<String>| {
                *base += entry_size(entry);
                let mut s = progress.lock().unwrap();
                s.copied_bytes = *base;
                if let Some(msg) = err {
                    s.errors.push(format!("{}: {}", entry.name, msg));
                }
                s.files_done = i + 1;
                drop(s);
                notify();
            };

            // Reject destructive self-referential transfers (a directory into
            // itself or its own subtree, a file onto itself) before touching
            // anything, so neither source nor destination is harmed.
            if fs_util::is_within_or_equal(&dest, &entry.path) {
                skip_entry(
                    &progress,
                    &mut base_bytes,
                    Some("cannot copy a path into itself".to_string()),
                );
                continue;
            }

            // Check the destination LIVE, not the scan-time conflict list: the
            // confirmation dialog can sit open while the filesystem changes.
            // `path_is_taken` (not `exists`) so a broken symlink occupying the
            // name is honoured as a conflict, matching `find_conflicts`.
            let dest_present = fs_util::path_is_taken(&dest);
            if dest_present {
                match spec.policy {
                    OverwritePolicy::SkipAll => {
                        skip_entry(&progress, &mut base_bytes, None);
                        continue;
                    }
                    OverwritePolicy::Ask => {
                        // No overwrite was confirmed (no conflict was shown, or
                        // the destination appeared after the scan): refuse
                        // rather than silently clobber it.
                        skip_entry(
                            &progress,
                            &mut base_bytes,
                            Some("destination already exists".to_string()),
                        );
                        continue;
                    }
                    // Fall through; KeepBoth/OverwriteAll handled below.
                    OverwritePolicy::OverwriteAll | OverwritePolicy::KeepBoth => {}
                }
            }

            let errors_before = progress.lock().unwrap().errors.len();

            // Pick the copy target and whether a swap is needed:
            // - new destination: copy straight to `dest`.
            // - OverwriteAll: copy to a staging sibling, then swap into place,
            //   so the existing destination is never destroyed before the copy
            //   is known good (and native CLONE|EXCL never collides with it).
            // - KeepBoth: copy to a fresh "name copy" sibling, no swap, and the
            //   existing destination is left intact.
            let overwrite = dest_present && spec.policy == OverwritePolicy::OverwriteAll;
            let copy_target = if !dest_present {
                dest.clone()
            } else if overwrite {
                staging_path(&dest)
            } else {
                fs_util::available_copy_name(&dest)
            };

            // Same-volume moves are an instant, atomic rename instead of a
            // copy-then-delete: no transient duplication, no walk-and-copy of
            // every byte, and no second pass to remove the source. The copy
            // path is reserved for cross-volume moves and all copies.
            let renamed = is_move
                && entry
                    .path
                    .parent()
                    .is_some_and(|p| fs_util::same_volume(p, &spec.target));

            let result = if renamed {
                rename_entry(&entry.path, &copy_target, entry, &progress, base_bytes)
            } else {
                spec.method.copy_entry(
                    &entry.path,
                    entry.is_dir,
                    &copy_target,
                    &progress,
                    base_bytes,
                )
            };

            match &result {
                Ok(b) => base_bytes += b,
                Err(e) => {
                    let mut s = progress.lock().unwrap();
                    if s.cancelled {
                        drop(s);
                        // For a rename this is a no-op (a failed rename never
                        // created `copy_target`); for a copy it drops the
                        // partial. Either way the source is left intact.
                        let _ = undo_placement(&copy_target, &entry.path, renamed);
                        break; // fall through to the finished-setter below
                    }
                    s.errors.push(format!("{}: {}", entry.name, e));
                }
            }

            // "Clean" = Ok return AND no per-file errors recorded by the
            // native callback during this entry.
            let clean = result.is_ok() && progress.lock().unwrap().errors.len() == errors_before;

            let placed = if !clean {
                // Undo our placement; a pre-existing dest is untouched. For a
                // rename this restores the source rather than deleting its only
                // copy.
                if let Some(msg) = undo_placement(&copy_target, &entry.path, renamed) {
                    progress
                        .lock()
                        .unwrap()
                        .errors
                        .push(format!("{}: {}", entry.name, msg));
                }
                false
            } else if overwrite {
                match swap_into_place(&copy_target, &dest) {
                    Ok(()) => true,
                    Err(e) => {
                        // The swap left the staged data at `copy_target`. For a
                        // rename that is the source's ONLY copy, so move it back
                        // to the source instead of deleting it (the previous
                        // unconditional cleanup here lost the source on a failed
                        // same-volume overwrite move).
                        let extra = undo_placement(&copy_target, &entry.path, renamed);
                        let mut s = progress.lock().unwrap();
                        s.errors.push(format!("{}: {}", entry.name, e));
                        if let Some(msg) = extra {
                            s.errors.push(format!("{}: {}", entry.name, msg));
                        }
                        false
                    }
                }
            } else {
                // New destination or KeepBoth: the copy already landed at its
                // final path, nothing to swap.
                true
            };

            // Delete the source only once the destination is fully in place.
            // A same-volume rename already moved the source, so there is
            // nothing left to remove.
            if is_move && placed && !renamed {
                let _ = cleanup_path(&entry.path);
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

/// Move one entry to `dst` with a single atomic, no-clobber rename (the
/// same-volume move fast path). `dst` is always a path that should not exist
/// yet (an absent destination, a fresh "copy" name, or a staging sibling), so
/// the rename mirrors the copy path's `EXCL` no-clobber guarantee. The entry's
/// full size is reported as copied, since a rename transfers it whole; the size
/// is read before the move while the source still exists.
fn rename_entry(
    src: &Path,
    dst: &Path,
    entry: &FileEntry,
    progress: &TransferState,
    base_bytes: u64,
) -> std::io::Result<u64> {
    let size = entry_size(entry);
    {
        let mut s = progress.lock().unwrap();
        s.current_file = entry.name.clone();
        s.current_file_size = size;
        s.current_file_copied = 0;
    }
    crate::native_copy::rename_noreplace(src, dst)?;
    let mut s = progress.lock().unwrap();
    s.current_file_copied = size;
    s.copied_bytes = base_bytes + size;
    s.maybe_sample();
    Ok(size)
}

/// A hidden sibling of `dest` that does not exist yet, used to stage an
/// overwrite copy before swapping it into place (same directory = same
/// volume, so the final rename is atomic and clone-friendly).
fn staging_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".to_string());
    let parent = dest.parent().unwrap_or(Path::new("."));
    fs_util::first_available(|i| parent.join(format!(".{}.cmdr-tmp.{}", name, i)))
}

/// Remove a file or directory tree, treating "not found" as success.
fn cleanup_path(path: &Path) -> std::io::Result<()> {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Undo our own placement at `staged` after a failed transfer of one entry.
///
/// For a COPY, `staged` is a disposable duplicate (the original still sits at
/// `source`), so it is simply removed. For the same-volume rename fast path,
/// `staged` IS the source's only copy (the source was already moved into it),
/// so it must be moved back to `source` rather than deleted, or the user's data
/// would be lost. If the restore cannot complete, the data is left in place
/// (never deleted) and its location is returned so it can be recovered.
fn undo_placement(staged: &Path, source: &Path, was_renamed: bool) -> Option<String> {
    if !was_renamed {
        let _ = cleanup_path(staged);
        return None;
    }
    if std::fs::rename(staged, source).is_ok() {
        return None;
    }
    if staged.symlink_metadata().is_ok() {
        Some(format!("data preserved at {}", staged.display()))
    } else {
        None
    }
}

/// Replace `dest` with the freshly-staged `staged`: move the existing `dest`
/// to a backup, rename `staged` into place, then drop the backup. Restores
/// the original on failure, so an interrupted overwrite never loses data.
fn swap_into_place(staged: &Path, dest: &Path) -> std::io::Result<()> {
    if !dest.exists() {
        return std::fs::rename(staged, dest);
    }
    let backup = staging_path(dest);
    std::fs::rename(dest, &backup)?;
    match std::fs::rename(staged, dest) {
        Ok(()) => {
            let _ = cleanup_path(&backup);
            Ok(())
        }
        Err(e) => {
            // Put the original back. If even that fails, the original now
            // lives only at the hidden backup path; name it in the error so
            // it can be recovered rather than vanishing silently.
            if std::fs::rename(&backup, dest).is_err() {
                return Err(std::io::Error::other(format!(
                    "{e}; original preserved at {}",
                    backup.display()
                )));
            }
            Err(e)
        }
    }
}

/// Copy a single file with progress reporting (buffered strategy).
/// Removes the partial destination file on any failure.
/// Returns the file size on success.
fn copy_file_buffered(src: &Path, dst: &Path, state: &TransferState) -> std::io::Result<u64> {
    let result = copy_file_buffered_inner(src, dst, state);
    if let Err(ref e) = result {
        // Clean up our own partial write, but never delete a destination that
        // was already there (AlreadyExists means create_new refused to clobber).
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(dst);
        }
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
    // create_new (O_EXCL): the caller always targets a path that should not
    // exist yet, so refuse to truncate a file that races into being.
    let dst_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)?;
    let mut writer = std::io::BufWriter::with_capacity(COPY_BUF_SIZE, dst_file);

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
    // Apply the source's permissions only after the contents are complete, so
    // a concurrent reader never sees a partial file already wearing its final
    // (possibly executable) mode.
    if let Ok(meta) = src.metadata() {
        let _ = std::fs::set_permissions(dst, meta.permissions());
    }
    Ok(file_size)
}

/// Recursively copy a directory with progress (buffered strategy).
/// Returns total bytes copied. Symlinks are recreated as links rather than
/// followed, so a link pointing back into the tree cannot cause infinite
/// recursion.
fn copy_dir_buffered(src: &Path, dst: &Path, state: &TransferState) -> std::io::Result<u64> {
    std::fs::create_dir_all(dst)?;
    let mut copied = 0u64;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        // file_type() does NOT follow symlinks (unlike Path::is_dir).
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            copy_symlink(&src_path, &dst_path)?;
        } else if ft.is_dir() {
            copied += copy_dir_buffered(&src_path, &dst_path, state)?;
        } else {
            copied += copy_file_buffered(&src_path, &dst_path, state)?;
        }
    }
    Ok(copied)
}

/// Recreate a symlink at `dst` pointing at the same target as `src`.
#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(src)?;
    let _ = std::fs::remove_file(dst);
    std::os::unix::fs::symlink(target, dst)
}

#[cfg(not(unix))]
fn copy_symlink(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Ok(())
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

    // The engine no longer consults a conflict list (it checks the
    // destination live), but the tests keep passing one to document intent;
    // it is ignored here.
    fn spec(
        kind: TransferKind,
        method: CopyMethod,
        entries: Vec<FileEntry>,
        target: &Path,
        _conflicts: Vec<String>,
        policy: OverwritePolicy,
    ) -> TransferSpec {
        TransferSpec {
            kind,
            entries,
            target: target.to_path_buf(),
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
    fn move_keeps_source_when_placement_fails() {
        use std::os::unix::fs::PermissionsExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("folder");
        let inner = src.file("folder/data.txt", "payload");
        // Make the destination unwritable so neither the rename fast path nor
        // the copy path can land the entry. The source must survive untouched.
        std::fs::set_permissions(dst.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        // Restore permissions so TempDir cleanup works everywhere.
        let _ = std::fs::set_permissions(dst.path(), std::fs::Permissions::from_mode(0o755));

        assert!(!s.errors.is_empty(), "the failure must be reported");
        assert!(dir.exists(), "source must survive a failed move");
        assert!(inner.exists());
    }

    #[test]
    fn same_volume_move_relocates_whole_tree_via_rename() {
        // src and dst temp dirs share a volume, so a Move takes the rename
        // fast path: the source is relinked, not copied byte-by-byte.
        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("folder");
        src.file("folder/a.txt", "one");
        src.file("folder/sub/b.txt", "two");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Native,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert!(s.errors.is_empty());
        assert!(!dir.exists(), "source is gone after a move");
        assert_eq!(
            std::fs::read_to_string(dst.path().join("folder/a.txt")).unwrap(),
            "one"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("folder/sub/b.txt")).unwrap(),
            "two"
        );
        assert_eq!(s.files_done, 1);
        assert_eq!(s.copied_bytes, s.total_bytes);
    }

    #[test]
    fn same_volume_move_needs_no_read_access_to_contents() {
        use std::os::unix::fs::PermissionsExt;
        // A rename relocates an unreadable file whole; the old copy-then-delete
        // path would have failed trying to read it. This documents that a
        // same-volume move is a true rename, not a copy.
        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("folder");
        let secret = src.file("folder/secret.txt", "unreadable");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Native,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        let moved = dst.path().join("folder/secret.txt");
        let _ = std::fs::set_permissions(&moved, std::fs::Permissions::from_mode(0o644));

        assert!(
            s.errors.is_empty(),
            "rename needs no read access to contents"
        );
        assert!(!dir.exists());
        assert!(moved.exists());
    }

    #[test]
    fn same_volume_move_overwrite_replaces_destination() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "new contents");
        dst.file("a.txt", "old contents");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Native,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(s.errors.is_empty());
        assert!(!file.exists(), "source is gone after a move");
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "new contents"
        );
    }

    #[test]
    fn undo_placement_restores_a_renamed_source_instead_of_deleting_it() {
        // The same-volume move fast path moves the source INTO `staged`; on a
        // later failure `undo_placement` must put it back, not delete the only
        // copy (the data-loss bug the swap-failure path used to have).
        let tmp = TempDir::new();
        let staged = tmp.file("staged.tmp", "the only copy");
        let source = tmp.path().join("source.txt"); // emptied by the rename

        let msg = undo_placement(&staged, &source, true);
        assert!(msg.is_none(), "restore should succeed");
        assert!(!staged.exists(), "staged moved back");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "the only copy");
    }

    #[test]
    fn undo_placement_deletes_a_copied_duplicate() {
        let tmp = TempDir::new();
        let staged = tmp.file("dup.tmp", "disposable");
        let source = tmp.file("source.txt", "original stays");

        let msg = undo_placement(&staged, &source, false);
        assert!(msg.is_none());
        assert!(!staged.exists(), "disposable copy removed");
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "original stays",
            "the real source is never touched on a copy"
        );
    }

    #[test]
    fn undo_placement_preserves_data_when_a_rename_restore_cannot_complete() {
        // If the source location is unreachable (its parent is gone), the data
        // must be left at `staged`, never deleted, and its path surfaced.
        let tmp = TempDir::new();
        let staged = tmp.file("staged.tmp", "irreplaceable");
        let source = tmp.path().join("missing_dir").join("source.txt");

        let msg = undo_placement(&staged, &source, true);
        assert!(msg.is_some(), "must report where the data was kept");
        assert!(msg.unwrap().contains("staged.tmp"));
        assert!(staged.exists(), "data left in place, not destroyed");
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

    // ── Data-loss regressions (from the safety audit) ──────────────────

    #[test]
    fn moving_dir_into_its_own_parent_is_rejected() {
        // Both panels on the same dir: target == source's parent, so
        // dest == source. The source must survive untouched.
        let work = TempDir::new();
        let data = work.dir("data");
        work.file("data/important.txt", "keep me");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Native,
            vec![entry_for(&data)],
            work.path(),
            vec!["data".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(!s.errors.is_empty(), "self-referential move must error");
        assert!(data.exists(), "source directory must survive");
        assert_eq!(
            std::fs::read_to_string(data.join("important.txt")).unwrap(),
            "keep me"
        );
    }

    #[test]
    fn moving_file_onto_itself_keeps_it_buffered() {
        // The buffered path used to truncate-on-create then delete the source.
        let work = TempDir::new();
        let f = work.file("a.txt", "content");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&f)],
            work.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(f.exists(), "file must not be deleted");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "content");
        assert!(!s.errors.is_empty());
    }

    #[test]
    fn failed_overwrite_preserves_existing_destination() {
        use std::os::unix::fs::PermissionsExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("box");
        src.file("box/ok.txt", "new");
        let secret = src.file("box/secret.txt", "x");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        dst.dir("box");
        dst.file("box/existing.txt", "OLD");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec!["box".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        let _ = std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644));

        assert!(!s.errors.is_empty());
        // The pre-existing destination must NOT have been destroyed before
        // the (failing) copy.
        assert_eq!(
            std::fs::read_to_string(dst.path().join("box/existing.txt")).unwrap(),
            "OLD"
        );
    }

    #[test]
    fn successful_overwrite_replaces_directory() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("box");
        src.file("box/new.txt", "NEW");
        dst.dir("box");
        dst.file("box/old.txt", "OLD");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec!["box".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(s.errors.is_empty());
        assert_eq!(
            std::fs::read_to_string(dst.path().join("box/new.txt")).unwrap(),
            "NEW"
        );
        assert!(
            !dst.path().join("box/old.txt").exists(),
            "overwrite replaces the directory"
        );
    }

    #[test]
    fn keep_both_writes_a_copy_and_preserves_the_original() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "NEW");
        dst.file("a.txt", "OLD");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::KeepBoth,
        ));

        assert!(s.errors.is_empty());
        // Original untouched, incoming written under a fresh "copy" name.
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "OLD"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a copy.txt")).unwrap(),
            "NEW"
        );
    }

    #[test]
    fn keep_both_move_keeps_original_and_removes_source() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "NEW");
        dst.file("a.txt", "OLD");

        run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::KeepBoth,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "OLD"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a copy.txt")).unwrap(),
            "NEW"
        );
        assert!(!file.exists(), "move removes the source after keep-both");
    }

    #[test]
    fn native_overwrite_replaces_existing_file() {
        // Native copyfile uses CLONE|EXCL and used to silently fail on an
        // existing destination; staging + swap fixes that.
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "NEW");
        dst.file("a.txt", "OLD");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(s.errors.is_empty(), "native overwrite must not error");
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "NEW"
        );
    }

    #[test]
    fn copying_dir_into_its_own_subdir_is_rejected() {
        // dest = a/sub/a lives inside the source a: must be rejected and
        // must not recurse forever.
        let work = TempDir::new();
        let a = work.dir("a");
        let sub = work.dir("a/sub");
        work.file("a/f.txt", "x");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&a)],
            &sub,
            vec![],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(!s.errors.is_empty(), "copy into own subtree must error");
    }

    #[test]
    fn buffered_copy_recreates_symlink_without_following() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let tree = src.dir("tree");
        src.file("tree/real.txt", "hi");
        // A link pointing back to its own ancestor: following it would loop.
        std::os::unix::fs::symlink(&tree, tree.join("loop")).unwrap();

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&tree)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert!(s.errors.is_empty());
        assert_eq!(
            std::fs::read_to_string(dst.path().join("tree/real.txt")).unwrap(),
            "hi"
        );
        let link_meta = std::fs::symlink_metadata(dst.path().join("tree/loop")).unwrap();
        assert!(
            link_meta.file_type().is_symlink(),
            "the link must be recreated, not followed"
        );
    }

    #[test]
    fn skip_all_progress_reaches_total() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let a = src.file("a.txt", "12345"); // 5 bytes, will be skipped
        let b = src.file("b.txt", "123"); // 3 bytes, will be copied
        dst.file("a.txt", "old");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&a), entry_for(&b)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::SkipAll,
        ));

        assert_eq!(s.total_bytes, 8);
        assert_eq!(
            s.copied_bytes, s.total_bytes,
            "skipped bytes must still advance the bar to 100%"
        );
    }

    #[test]
    fn ask_policy_refuses_surprise_existing_destination() {
        // A destination that appeared after the scan (so it is NOT in the
        // conflict list and the user never confirmed an overwrite) must not be
        // clobbered, and a Move must keep its source.
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "NEW");
        dst.file("a.txt", "OLD-IMPORTANT");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec![], // empty: the engine must rely on a live check, not this
            OverwritePolicy::Ask,
        ));

        assert!(!s.errors.is_empty(), "a surprise existing dest must error");
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "OLD-IMPORTANT",
            "destination must not be clobbered"
        );
        assert!(file.exists(), "move must keep the source when refused");
    }

    #[test]
    fn self_referential_entry_still_advances_progress() {
        let work = TempDir::new();
        let data = work.dir("data");
        work.file("data/x.txt", "12345"); // 5 bytes

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&data)],
            work.path(),
            vec!["data".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(!s.errors.is_empty());
        assert_eq!(
            s.copied_bytes, s.total_bytes,
            "a rejected entry must still advance the bar to 100%"
        );
    }

    #[test]
    fn failed_new_dir_copy_cleans_partial_destination() {
        use std::os::unix::fs::PermissionsExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("box");
        let secret = src.file("box/secret.txt", "x");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        let _ = std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644));

        assert!(!s.errors.is_empty());
        assert!(
            !dst.path().join("box").exists(),
            "partial directory must be cleaned up"
        );
    }
}
