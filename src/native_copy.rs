//! Native macOS `copyfile()` with progress callback.
//!
//! Uses APFS clone (instant) when source and dest are on the same APFS volume,
//! falls back to byte-copy with xattr/ACL preservation otherwise.

use std::ffi::{CString, OsStr};
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::transfer::TransferProgress;

/// Build a C string from a path's exact kernel-visible bytes. macOS file names
/// are arbitrary byte sequences; going through `to_string_lossy` would replace
/// invalid UTF-8 with U+FFFD and make `copyfile()` operate on the wrong path,
/// which for a Move would then delete the real source. Use the raw OS bytes.
fn path_cstring(p: &Path) -> std::io::Result<CString> {
    CString::new(p.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
}

// copyfile flags
const COPYFILE_ALL: u32 = 0x000F; // DATA + STAT + ACL + XATTR
const COPYFILE_EXCL: u32 = 1 << 17; // fail if the destination already exists
const COPYFILE_RECURSIVE: u32 = 0x8000;
const COPYFILE_CLONE: u32 = 1 << 24; // try clone first

// Status callback "what" values (copyfile.h)
const COPYFILE_RECURSE_ERROR: c_int = 0;
const COPYFILE_RECURSE_FILE: c_int = 1;
const COPYFILE_RECURSE_DIR: c_int = 2;
const COPYFILE_COPY_DATA: c_int = 4;

// Status callback "stage" values (copyfile.h)
const COPYFILE_START: c_int = 1;
const COPYFILE_FINISH: c_int = 2;
const COPYFILE_ERR: c_int = 3;
const COPYFILE_PROGRESS: c_int = 4;

const COPYFILE_CONTINUE: c_int = 0;
const COPYFILE_QUIT: c_int = 2;

// copyfile_state keys
const COPYFILE_STATE_STATUS_CB: u32 = 6;
const COPYFILE_STATE_STATUS_CTX: u32 = 7;
const COPYFILE_STATE_COPIED: u32 = 8; // bytes copied so far

#[allow(non_camel_case_types)]
type copyfile_state_t = *mut std::ffi::c_void;
#[allow(non_camel_case_types)]
type copyfile_callback_t = extern "C" fn(
    what: c_int,
    stage: c_int,
    state: copyfile_state_t,
    src: *const c_char,
    dst: *const c_char,
    ctx: *mut std::ffi::c_void,
) -> c_int;

unsafe extern "C" {
    fn copyfile_state_alloc() -> copyfile_state_t;
    fn copyfile_state_free(s: copyfile_state_t) -> c_int;
    fn copyfile_state_set(s: copyfile_state_t, flag: u32, value: *const std::ffi::c_void) -> c_int;
    fn copyfile_state_get(s: copyfile_state_t, flag: u32, value: *mut std::ffi::c_void) -> c_int;
    fn copyfile(
        from: *const c_char,
        to: *const c_char,
        state: copyfile_state_t,
        flags: c_uint,
    ) -> c_int;
    fn clonefile(from: *const c_char, to: *const c_char, flags: c_int) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeCopyOutcome {
    pub bytes: u64,
    pub cloned: bool,
}

struct CallbackCtx {
    state: Arc<Mutex<TransferProgress>>,
    /// Bytes copied before this `copyfile()` call started.
    base_bytes: u64,
    /// Bytes of files fully completed within this call (recursive copies).
    done_in_call: u64,
    report_progress: bool,
}

/// The exact path behind a callback's C string. Built from the raw bytes, so
/// `metadata()` works for names that are not valid UTF-8.
fn cstr_to_path(ptr: *const c_char) -> Option<PathBuf> {
    if ptr.is_null() {
        return None;
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_bytes();
    Some(PathBuf::from(OsStr::from_bytes(bytes)))
}

/// Leaf name shown for a callback path (lossy only for display).
fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .to_string()
}

/// Per-file failure line, with the OS reason when `copyfile` set one.
fn failure_message(name: &str, os_error: &std::io::Error) -> String {
    match os_error.raw_os_error() {
        Some(0) | None => format!("Failed to copy: {name}"),
        Some(_) => format!("Failed to copy: {name} ({os_error})"),
    }
}

extern "C" fn progress_callback(
    what: c_int,
    stage: c_int,
    cstate: copyfile_state_t,
    src: *const c_char,
    _dst: *const c_char,
    ctx: *mut std::ffi::c_void,
) -> c_int {
    let ctx = unsafe { &mut *(ctx as *mut CallbackCtx) };

    match (what, stage) {
        (COPYFILE_RECURSE_FILE, COPYFILE_START) => {
            // stat before locking: the UI locks this progress every frame, and
            // a slow mount must not stall it.
            let started = if ctx.report_progress {
                cstr_to_path(src).map(|path| {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    (display_name(&path), size)
                })
            } else {
                None
            };
            let mut s = crate::lock_util::recover(&ctx.state);
            if s.cancelled {
                return COPYFILE_QUIT;
            }
            if let Some((name, size)) = started {
                s.current_file = name;
                s.current_file_size = size;
                s.current_file_copied = 0;
            }
            COPYFILE_CONTINUE
        }
        (COPYFILE_RECURSE_FILE, COPYFILE_FINISH) => {
            if !ctx.report_progress {
                return COPYFILE_CONTINUE;
            }
            let mut s = crate::lock_util::recover(&ctx.state);
            // Count the whole file as done: cloned files produce no DATA callbacks.
            ctx.done_in_call = ctx.done_in_call.saturating_add(s.current_file_size);
            s.current_file_copied = s.current_file_size;
            s.copied_bytes = ctx.base_bytes.saturating_add(ctx.done_in_call);
            s.maybe_sample();
            COPYFILE_CONTINUE
        }
        (COPYFILE_RECURSE_FILE, COPYFILE_ERR)
        | (COPYFILE_RECURSE_DIR, COPYFILE_ERR)
        | (COPYFILE_RECURSE_ERROR, _) => {
            // Fail fast: leave already-copied siblings on disk and surface the
            // failure so `copy_dir_native` returns Err before the executor can
            // treat the tree as disposable.
            let os_error = std::io::Error::last_os_error();
            let name = cstr_to_path(src).map(|path| path.display().to_string());
            let mut s = crate::lock_util::recover(&ctx.state);
            let name = name.unwrap_or_else(|| s.current_file.clone());
            s.errors.push(failure_message(&name, &os_error));
            COPYFILE_QUIT
        }
        (COPYFILE_COPY_DATA, COPYFILE_PROGRESS) => {
            unsafe {
                if !ctx.report_progress {
                    return if crate::lock_util::recover(&ctx.state).cancelled {
                        COPYFILE_QUIT
                    } else {
                        COPYFILE_CONTINUE
                    };
                }
                let mut bytes_copied: i64 = 0;
                let got = copyfile_state_get(
                    cstate,
                    COPYFILE_STATE_COPIED,
                    &mut bytes_copied as *mut _ as *mut std::ffi::c_void,
                );

                let mut s = crate::lock_util::recover(&ctx.state);
                if s.cancelled {
                    return COPYFILE_QUIT;
                }
                // An unknown count must not move this file's progress back to 0.
                if got != 0 {
                    return COPYFILE_CONTINUE;
                }

                let current_copied = bytes_copied.max(0) as u64;
                s.current_file_copied = current_copied;
                s.copied_bytes = ctx
                    .base_bytes
                    .saturating_add(ctx.done_in_call)
                    .saturating_add(current_copied);
                s.maybe_sample();
            }
            COPYFILE_CONTINUE
        }
        _ => COPYFILE_CONTINUE,
    }
}

/// Copy a single file using native `copyfile()` with progress.
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
    allow_clone: bool,
    report_progress: bool,
) -> std::io::Result<NativeCopyOutcome> {
    let src_c = path_cstring(src)?;
    let dst_c = path_cstring(dst)?;

    let file_size = src.metadata().map(|m| m.len()).unwrap_or(0);

    if report_progress {
        crate::lock_util::recover(state).begin_file(src, file_size);
    }

    // Try the explicit clone primitive first so telemetry can distinguish a
    // metadata-only APFS clone from copyfile's transparent byte-copy fallback.
    crate::transfer::check_cancelled(state)?;
    if allow_clone {
        if unsafe { clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) } == 0 {
            if report_progress {
                let mut progress = crate::lock_util::recover(state);
                progress.current_file_copied = file_size;
                progress.copied_bytes = base_bytes.saturating_add(file_size);
            }
            return Ok(NativeCopyOutcome {
                bytes: file_size,
                cloned: true,
            });
        }
        // Fallback is legal only when the declined clone left no artifact.
        // Otherwise COPYFILE_EXCL would fail ambiguously and a caller could
        // mistake an unverified partial for resumable staging.
        if crate::fs_util::path_is_taken(dst) {
            return Err(std::io::Error::other(
                "clone backend declined after creating a staging artifact",
            ));
        }
    }

    let mut ctx = CallbackCtx {
        state: state.clone(),
        base_bytes,
        done_in_call: 0,
        report_progress,
    };
    // EXCL: the caller always hands us a destination that should not yet
    // exist (a fresh staging path, or a dest believed absent), so refuse to
    // clobber rather than overwrite if one races into being.
    let (result, err) = run_copyfile(&src_c, &dst_c, COPYFILE_ALL | COPYFILE_EXCL, &mut ctx)?;
    if result != 0 {
        let s = crate::lock_util::recover(state);
        if s.cancelled {
            // Clean up partial file
            let _ = std::fs::remove_file(dst);
            return Err(crate::transfer::cancelled_error());
        }
        return Err(err);
    }

    // Finalize progress: a cloned file emits no DATA callbacks at all.
    if report_progress {
        let mut s = crate::lock_util::recover(state);
        s.current_file_copied = file_size;
        s.copied_bytes = base_bytes.saturating_add(file_size);
    }

    Ok(NativeCopyOutcome {
        bytes: file_size,
        cloned: false,
    })
}

/// Copy a directory recursively using native `copyfile()` with `COPYFILE_RECURSIVE`.
pub fn copy_dir_native(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
) -> std::io::Result<u64> {
    let src_c = path_cstring(src)?;
    let dst_c = path_cstring(dst)?;

    let errors_before = {
        let mut s = crate::lock_util::recover(state);
        s.current_file = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        s.errors.len()
    };

    let mut ctx = CallbackCtx {
        state: state.clone(),
        base_bytes,
        done_in_call: 0,
        report_progress: true,
    };
    let flags = COPYFILE_ALL | COPYFILE_RECURSIVE | COPYFILE_CLONE | COPYFILE_EXCL;
    let (result, os_error) = run_copyfile(&src_c, &dst_c, flags, &mut ctx)?;
    finish_dir_copy(
        state,
        base_bytes,
        errors_before,
        result,
        os_error,
        ctx.done_in_call,
    )
}

/// Run one `copyfile()` with [`progress_callback`] reporting into `ctx`.
///
/// Returns the call's result code together with errno captured right after
/// it, before `copyfile_state_free` can disturb errno. `Err` means the state
/// could not be set up and nothing was copied.
fn run_copyfile(
    src: &CString,
    dst: &CString,
    flags: u32,
    ctx: &mut CallbackCtx,
) -> std::io::Result<(c_int, std::io::Error)> {
    let cstate = unsafe { copyfile_state_alloc() };
    if cstate.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let cb: copyfile_callback_t = progress_callback;
    // Without the callback a copy could not be cancelled or reported, so a
    // refused state setting fails the copy instead of running blind.
    let configured = unsafe {
        copyfile_state_set(
            cstate,
            COPYFILE_STATE_STATUS_CB,
            cb as *const std::ffi::c_void,
        ) == 0
            && copyfile_state_set(
                cstate,
                COPYFILE_STATE_STATUS_CTX,
                ctx as *mut CallbackCtx as *const std::ffi::c_void,
            ) == 0
    };
    if !configured {
        let error = std::io::Error::last_os_error();
        unsafe { copyfile_state_free(cstate) };
        return Err(error);
    }
    let result = unsafe { copyfile(src.as_ptr(), dst.as_ptr(), cstate, flags) };
    let os_error = std::io::Error::last_os_error();
    unsafe { copyfile_state_free(cstate) };
    Ok((result, os_error))
}

/// Finalize a recursive native directory copy. Per-file callback failures must
/// become `Err` even if `copyfile()` itself returned success, so the executor
/// never treats a partial tree as a clean placement candidate.
fn finish_dir_copy(
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
    errors_before: usize,
    copyfile_rc: c_int,
    os_error: std::io::Error,
    copied: u64,
) -> std::io::Result<u64> {
    if copyfile_rc != 0 {
        let s = crate::lock_util::recover(state);
        if s.cancelled {
            return Err(crate::transfer::cancelled_error());
        }
        return Err(os_error);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn no_error() -> std::io::Error {
        std::io::Error::from_raw_os_error(0)
    }

    #[test]
    fn finish_dir_copy_reports_the_errno_captured_after_copyfile() {
        let progress = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
        let os_error = std::io::Error::from_raw_os_error(libc::ENOSPC);
        let err = finish_dir_copy(&progress, 0, 0, -1, os_error, 0).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ENOSPC));

        crate::lock_util::recover(&progress).cancelled = true;
        let os_error = std::io::Error::from_raw_os_error(libc::ENOSPC);
        let err = finish_dir_copy(&progress, 0, 0, -1, os_error, 0).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
    }

    #[test]
    fn callback_paths_keep_non_utf8_bytes_and_failures_name_the_reason() {
        let raw = CString::new(b"/tmp/bad-\xff.bin".to_vec()).unwrap();
        let path = cstr_to_path(raw.as_ptr()).unwrap();
        assert_eq!(path.as_os_str().as_bytes(), b"/tmp/bad-\xff.bin");
        assert_eq!(display_name(&path), "bad-\u{fffd}.bin");
        assert!(cstr_to_path(std::ptr::null()).is_none());

        let denied = std::io::Error::from_raw_os_error(libc::EACCES);
        assert!(failure_message("a.txt", &denied).starts_with("Failed to copy: a.txt ("));
        assert_eq!(
            failure_message("a.txt", &no_error()),
            "Failed to copy: a.txt"
        );
    }

    #[test]
    fn finish_dir_copy_returns_err_when_callback_recorded_child_failures() {
        let progress = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
        {
            let mut state = crate::lock_util::recover(&progress);
            state.errors.push("Failed to copy: bad.txt".to_string());
        }
        let err = finish_dir_copy(&progress, 0, 0, 0, no_error(), 12).unwrap_err();
        assert!(err.to_string().contains("per-file failures"));
        // Successful siblings' byte progress must not be committed as clean.
        assert_eq!(crate::lock_util::recover(&progress).copied_bytes, 0);
    }

    #[test]
    fn finish_dir_copy_accepts_a_clean_recursive_success() {
        let progress = Arc::new(Mutex::new(TransferProgress::new(12, 1)));
        let copied = finish_dir_copy(&progress, 0, 0, 0, no_error(), 12).unwrap();
        assert_eq!(copied, 12);
        assert_eq!(crate::lock_util::recover(&progress).copied_bytes, 12);
    }
}
