//! Portable native-copy backend for Linux and Windows.
//!
//! Mirrors the macOS `native_copy` API: exclusive (never-clobbering) copies
//! with progress, cancellation, and a no-replace rename. `std::fs::copy`
//! already uses the platform's best primitive (`copy_file_range`, which can
//! reflink on Btrfs/XFS, or `CopyFileExW` on Windows).

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::transfer::TransferProgress;

const CHUNK: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeCopyOutcome {
    pub bytes: u64,
    pub cloned: bool,
}

/// Rename `src` to `dst`, failing with `AlreadyExists` instead of replacing
/// an occupied destination.
pub fn rename_noreplace(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let src_c = CString::new(src.as_os_str().as_bytes())?;
        let dst_c = CString::new(dst.as_os_str().as_bytes())?;
        // SAFETY: both pointers are valid NUL-terminated paths for the call.
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                src_c.as_ptr(),
                libc::AT_FDCWD,
                dst_c.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        let unsupported = error
            .raw_os_error()
            .is_some_and(|code| code == libc::EINVAL || code == libc::ENOSYS);
        if !unsupported {
            return Err(error);
        }
    }
    rename_noreplace_fallback(src, dst)
}

/// Best-effort no-clobber rename where the OS has no exclusive rename.
fn rename_noreplace_fallback(src: &Path, dst: &Path) -> std::io::Result<()> {
    if crate::fs_util::path_is_taken(dst) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    fs::rename(src, dst)
}

fn cancelled() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")
}

fn is_cancelled(state: &Arc<Mutex<TransferProgress>>) -> bool {
    crate::lock_util::recover(state).cancelled
}

/// Copy one file's bytes into a destination that must not exist yet.
fn copy_bytes(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
    report_progress: bool,
) -> std::io::Result<u64> {
    let mut input = fs::File::open(src)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(dst)?;
    let mut buf = vec![0u8; CHUNK];
    let mut copied = 0u64;
    let result = (|| {
        loop {
            if is_cancelled(state) {
                return Err(cancelled());
            }
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n])?;
            copied += n as u64;
            if report_progress {
                let mut s = crate::lock_util::recover(state);
                s.current_file_copied = copied;
                s.copied_bytes = base_bytes.saturating_add(copied);
            }
        }
        output.flush()?;
        if let Ok(metadata) = fs::metadata(src) {
            let _ = fs::set_permissions(dst, metadata.permissions());
            if let Ok(modified) = metadata.modified() {
                let _ = output.set_modified(modified);
            }
        }
        Ok(copied)
    })();
    if result.is_err() {
        drop(output);
        let _ = fs::remove_file(dst);
    }
    result
}

pub fn copy_file_native(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
    allow_clone: bool,
) -> std::io::Result<NativeCopyOutcome> {
    copy_file_native_inner(src, dst, state, base_bytes, allow_clone, true)
}

pub fn seed_file_native(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    allow_clone: bool,
) -> std::io::Result<NativeCopyOutcome> {
    copy_file_native_inner(src, dst, state, 0, allow_clone, false)
}

fn copy_file_native_inner(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
    _allow_clone: bool,
    report_progress: bool,
) -> std::io::Result<NativeCopyOutcome> {
    let file_size = src.metadata().map(|m| m.len()).unwrap_or(0);
    if report_progress {
        let mut s = crate::lock_util::recover(state);
        s.current_file = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        s.current_file_size = file_size;
        s.current_file_copied = 0;
    }
    if is_cancelled(state) {
        return Err(cancelled());
    }
    let bytes = copy_bytes(src, dst, state, base_bytes, report_progress)?;
    Ok(NativeCopyOutcome {
        bytes,
        cloned: false,
    })
}

/// Copy a directory tree into a destination that must not exist yet.
/// Per-file failures are recorded and turn the whole copy into an error, so
/// a partial tree is never treated as a clean result.
pub fn copy_dir_native(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
) -> std::io::Result<u64> {
    let errors_before = crate::lock_util::recover(state).errors.len();
    fs::create_dir(dst)?;
    let mut copied = 0u64;
    copy_tree(src, dst, state, base_bytes, &mut copied)?;
    if let Ok(metadata) = fs::metadata(src) {
        let _ = fs::set_permissions(dst, metadata.permissions());
    }
    let mut s = crate::lock_util::recover(state);
    if s.errors.len() > errors_before {
        return Err(std::io::Error::other(
            "native directory copy reported per-file failures",
        ));
    }
    s.copied_bytes = base_bytes.saturating_add(copied);
    Ok(copied)
}

fn copy_tree(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
    copied: &mut u64,
) -> std::io::Result<()> {
    for entry in fs::read_dir(src)? {
        if is_cancelled(state) {
            return Err(cancelled());
        }
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        let outcome = if file_type.is_symlink() {
            copy_symlink(&from, &to)
        } else if file_type.is_dir() {
            fs::create_dir(&to).and_then(|()| copy_tree(&from, &to, state, base_bytes, copied))
        } else {
            {
                let mut s = crate::lock_util::recover(state);
                s.current_file = entry.file_name().to_string_lossy().to_string();
                s.current_file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
            copy_bytes(&from, &to, state, base_bytes + *copied, true).map(|bytes| {
                *copied += bytes;
            })
        };
        match outcome {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => return Err(error),
            Err(error) => {
                crate::lock_util::recover(state)
                    .errors
                    .push(format!("Failed to copy: {} ({error})", from.display()));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}

#[cfg(windows)]
fn copy_symlink(from: &Path, to: &Path) -> std::io::Result<()> {
    let target = fs::read_link(from)?;
    if fs::metadata(from).is_ok_and(|m| m.is_dir()) {
        std::os::windows::fs::symlink_dir(target, to)
    } else {
        std::os::windows::fs::symlink_file(target, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn progress() -> Arc<Mutex<TransferProgress>> {
        Arc::new(Mutex::new(TransferProgress::new(0, 1)))
    }

    #[test]
    fn rename_noreplace_refuses_to_clobber() {
        let tmp = TempDir::new();
        let src = tmp.file("a.txt", "new");
        let dst = tmp.file("b.txt", "old");
        let err = rename_noreplace(&src, &dst).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&dst).unwrap(), "old");
    }

    #[test]
    fn file_copy_is_exclusive_and_reports_bytes() {
        let tmp = TempDir::new();
        let src = tmp.file("a.txt", "hello");
        let dst = tmp.path().join("b.txt");
        let state = progress();
        let outcome = copy_file_native(&src, &dst, &state, 0, true).unwrap();
        assert_eq!(outcome.bytes, 5);
        assert_eq!(fs::read_to_string(&dst).unwrap(), "hello");
        assert!(copy_file_native(&src, &dst, &state, 0, true).is_err());
    }

    #[test]
    fn dir_copy_recurses() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("nested/f.txt"), "abc").unwrap();
        let dst = tmp.path().join("dst");
        let copied = copy_dir_native(&src, &dst, &progress(), 0).unwrap();
        assert_eq!(copied, 3);
        assert_eq!(fs::read_to_string(dst.join("nested/f.txt")).unwrap(), "abc");
    }
}
