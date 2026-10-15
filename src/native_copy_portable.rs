//! Portable native-copy backend for Linux and Windows.
//!
//! Mirrors the macOS `native_copy` API: exclusive (never-clobbering) copies
//! with progress and cancellation. Bytes move in 1 MiB steps so progress and
//! cancel stay responsive: on Linux through `copy_file_range` (in-kernel, and
//! a reflink or server-side copy where Btrfs, XFS or NFS can do one), falling
//! back to a userspace read/write loop where the kernel declines.

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

/// `read` that retries a signal interruption (possible on network and FUSE
/// mounts). Cancellation is the progress flag, never `ErrorKind::Interrupted`.
fn read_retrying(input: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        match input.read(buf) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

/// Move up to `len` bytes from `input` to `output` inside the kernel, at both
/// files' current offsets. `Ok(None)` means the kernel cannot copy between
/// these files and the caller should use read/write from here on.
#[cfg(target_os = "linux")]
fn copy_range_chunk(
    input: &fs::File,
    output: &fs::File,
    len: usize,
) -> std::io::Result<Option<usize>> {
    use std::os::fd::AsRawFd;

    loop {
        let n = unsafe {
            libc::copy_file_range(
                input.as_raw_fd(),
                std::ptr::null_mut(),
                output.as_raw_fd(),
                std::ptr::null_mut(),
                len,
                0,
            )
        };
        if let Ok(n) = usize::try_from(n) {
            return Ok(Some(n));
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINTR) => {}
            Some(
                libc::EXDEV
                | libc::ENOSYS
                | libc::EOPNOTSUPP
                | libc::EINVAL
                | libc::EPERM
                | libc::EBADF,
            ) => return Ok(None),
            _ => return Err(error),
        }
    }
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
    let report = |copied: u64| {
        if report_progress {
            let mut s = crate::lock_util::recover(state);
            s.current_file_copied = copied;
            s.copied_bytes = base_bytes.saturating_add(copied);
            s.maybe_sample();
        }
    };
    let mut buf = Vec::new();
    let mut copied = 0u64;
    // Both paths advance the shared file offsets, so a fallback mid-file
    // continues exactly where the kernel copy stopped.
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut kernel_copy = cfg!(target_os = "linux");
    let result = (|| {
        loop {
            crate::transfer::check_cancelled(state)?;
            #[cfg(target_os = "linux")]
            if kernel_copy {
                match copy_range_chunk(&input, &output, CHUNK)? {
                    // A zero before any byte may be a pseudo-file whose size
                    // the kernel cannot see; let read() decide EOF.
                    Some(0) if copied == 0 => kernel_copy = false,
                    Some(0) => break,
                    Some(n) => {
                        copied = copied.saturating_add(n as u64);
                        report(copied);
                        continue;
                    }
                    None => kernel_copy = false,
                }
            }
            if buf.is_empty() {
                buf = vec![0u8; CHUNK];
            }
            let n = read_retrying(&mut input, &mut buf)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n])?;
            copied = copied.saturating_add(n as u64);
            report(copied);
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
        crate::lock_util::recover(state).begin_file(src, file_size);
    }
    crate::transfer::check_cancelled(state)?;
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
    copy_dir_attributes(src, dst);
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
        crate::transfer::check_cancelled(state)?;
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        let outcome = if file_type.is_symlink() {
            crate::fs_util::copy_symlink(&from, &to)
        } else if file_type.is_dir() {
            fs::create_dir(&to)
                .and_then(|()| copy_tree(&from, &to, state, base_bytes, copied))
                .map(|()| copy_dir_attributes(&from, &to))
        } else if !file_type.is_file() {
            // Opening a FIFO blocks until a writer appears, with no cancel
            // check; devices and sockets have no bytes to copy either.
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "special file (FIFO, socket or device) skipped",
            ))
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
            Err(_) if crate::lock_util::recover(state).cancelled => {
                return Err(crate::transfer::cancelled_error());
            }
            Err(error) => {
                crate::lock_util::recover(state)
                    .errors
                    .push(format!("Failed to copy: {} ({error})", from.display()));
            }
        }
    }
    Ok(())
}

/// Best-effort copy of a folder's mode and modification time, applied after
/// its children so a read-only folder does not block their creation.
fn copy_dir_attributes(src: &Path, dst: &Path) {
    let Ok(metadata) = fs::metadata(src) else {
        return;
    };
    // Directory handles for `futimens` need no write access on Unix; Windows
    // cannot open a folder through `File::open`, so it keeps the copy time.
    #[cfg(unix)]
    if let (Ok(modified), Ok(dir)) = (metadata.modified(), fs::File::open(dst)) {
        let _ = dir.set_modified(modified);
    }
    let _ = fs::set_permissions(dst, metadata.permissions());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    #[cfg(unix)]
    use std::ffi::CString;

    fn progress() -> Arc<Mutex<TransferProgress>> {
        Arc::new(Mutex::new(TransferProgress::new(0, 1)))
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

    #[test]
    fn tree_copy_records_a_child_failure_and_still_copies_its_siblings() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "aa").unwrap();
        fs::write(src.join("b.txt"), "new").unwrap();
        fs::write(src.join("c.txt"), "ccc").unwrap();
        // A destination child that already exists makes the exclusive create
        // fail for that one file only.
        let dst = tmp.path().join("dst");
        fs::create_dir(&dst).unwrap();
        fs::write(dst.join("b.txt"), "old").unwrap();
        let state = progress();

        let mut copied = 0;
        copy_tree(&src, &dst, &state, 0, &mut copied).unwrap();

        assert_eq!(copied, 5);
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "aa");
        assert_eq!(fs::read_to_string(dst.join("b.txt")).unwrap(), "old");
        assert_eq!(fs::read_to_string(dst.join("c.txt")).unwrap(), "ccc");
        let errors = crate::lock_util::recover(&state).errors.clone();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("b.txt"), "{errors:?}");
    }

    #[test]
    fn dir_copy_ignores_errors_recorded_before_it_started() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "aa").unwrap();
        let dst = tmp.path().join("dst");
        let state = progress();
        // Only failures recorded by this copy fail the folder.
        crate::lock_util::recover(&state)
            .errors
            .push("earlier transfer error".into());
        copy_dir_native(&src, &dst, &state, 0).unwrap();
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "aa");
    }

    #[test]
    fn file_copy_samples_speed_while_copying() {
        let tmp = TempDir::new();
        let src = tmp.file("a.txt", "hello");
        let dst = tmp.path().join("b.txt");
        let state = progress();
        {
            let mut s = crate::lock_util::recover(&state);
            s.started_at = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(2))
                .unwrap();
        }
        copy_file_native(&src, &dst, &state, 0, false).unwrap();
        let samples = crate::lock_util::recover(&state).speed_samples.clone();
        assert!(samples.len() >= 2, "{samples:?}");
        assert_eq!(samples.last().unwrap().1, 5.0);
    }

    #[test]
    fn read_retrying_retries_a_signal_interruption() {
        struct Flaky(bool);
        impl Read for Flaky {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if std::mem::replace(&mut self.0, false) {
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                buf[0] = 7;
                Ok(1)
            }
        }
        let mut buf = [0u8; 4];
        assert_eq!(read_retrying(&mut Flaky(true), &mut buf).unwrap(), 1);
        assert_eq!(buf[0], 7);
    }

    #[test]
    fn tree_copy_records_child_errors_and_stops_only_on_a_user_cancel() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("a")).unwrap();
        fs::write(src.join("b.txt"), "bb").unwrap();
        let dst = tmp.path().join("dst");
        fs::create_dir(&dst).unwrap();
        // The pre-existing "a" makes create_dir fail; any error that is not a
        // user cancel must be recorded and the walk continued.
        fs::create_dir(dst.join("a")).unwrap();
        let state = progress();
        let mut copied = 0;
        copy_tree(&src, &dst, &state, 0, &mut copied).unwrap();
        assert_eq!(fs::read_to_string(dst.join("b.txt")).unwrap(), "bb");
        assert_eq!(crate::lock_util::recover(&state).errors.len(), 1);

        crate::lock_util::recover(&state).cancelled = true;
        let error = copy_tree(&src, &tmp.path().join("other"), &state, 0, &mut copied).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    }

    #[cfg(unix)]
    #[test]
    fn dir_copy_restores_nested_folder_modes_and_times() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        let nested = src.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("f.txt"), "abc").unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::File::open(&nested).unwrap().set_modified(old).unwrap();
        let dst = tmp.path().join("dst");

        copy_dir_native(&src, &dst, &progress(), 0).unwrap();

        let copied = fs::metadata(dst.join("nested")).unwrap();
        assert_eq!(copied.permissions().mode() & 0o777, 0o700);
        assert_eq!(copied.modified().unwrap(), old);
        assert_eq!(fs::read_to_string(dst.join("nested/f.txt")).unwrap(), "abc");
    }

    #[cfg(unix)]
    #[test]
    fn dir_copy_skips_fifos_with_an_error_instead_of_blocking() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), "aa").unwrap();
        let fifo = CString::new(src.join("pipe").as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
        let dst = tmp.path().join("dst");
        let state = progress();

        let error = copy_dir_native(&src, &dst, &state, 0).unwrap_err();

        assert!(error.to_string().contains("per-file failures"));
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "aa");
        assert!(!dst.join("pipe").exists());
        let errors = crate::lock_util::recover(&state).errors.clone();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("pipe") && e.contains("special file")),
            "{errors:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_copy_falls_back_to_read_for_files_the_kernel_cannot_copy() {
        // procfs reports size 0 and copy_file_range copies nothing, yet the
        // file has content: the read/write fallback must still copy it.
        let tmp = TempDir::new();
        let dst = tmp.path().join("status");
        let state = progress();
        let outcome =
            copy_file_native(Path::new("/proc/self/status"), &dst, &state, 0, false).unwrap();
        let copied = fs::read_to_string(&dst).unwrap();
        assert!(copied.starts_with("Name:"), "{copied}");
        assert_eq!(outcome.bytes, copied.len() as u64);
    }

    #[test]
    fn large_file_copy_is_byte_exact_across_chunks() {
        let tmp = TempDir::new();
        let src = tmp.path().join("big.bin");
        let bytes: Vec<u8> = (0..(CHUNK * 2 + 4321)).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &bytes).unwrap();
        let dst = tmp.path().join("copy.bin");
        let state = progress();
        let outcome = copy_file_native(&src, &dst, &state, 10, false).unwrap();
        assert_eq!(outcome.bytes, bytes.len() as u64);
        assert_eq!(fs::read(&dst).unwrap(), bytes);
        let s = crate::lock_util::recover(&state);
        assert_eq!(s.copied_bytes, 10 + bytes.len() as u64);
        assert_eq!(s.current_file_copied, bytes.len() as u64);
    }
}
