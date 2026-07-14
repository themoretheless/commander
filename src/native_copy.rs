//! Native macOS `copyfile()` with progress callback.
//!
//! Uses APFS clone (instant) when source and dest are on the same APFS volume,
//! falls back to byte-copy with xattr/ACL preservation otherwise.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::transfer::TransferProgress;

/// Build a C string from a path's exact kernel-visible bytes. macOS file names
/// are arbitrary byte sequences; going through `to_string_lossy` would replace
/// invalid UTF-8 with U+FFFD and make copyfile() operate on the wrong path,
/// which for a Move would then delete the real source. Use the raw OS bytes.
fn path_cstring(p: &Path) -> std::io::Result<CString> {
    CString::new(p.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
}

// renamex_np flag: fail with EEXIST rather than clobber an existing dst.
const RENAME_EXCL: c_uint = 0x0000_0004;

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
    fn renamex_np(from: *const c_char, to: *const c_char, flags: c_uint) -> c_int;
    fn clonefile(from: *const c_char, to: *const c_char, flags: c_int) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeCopyOutcome {
    pub bytes: u64,
    pub cloned: bool,
}

/// Move `src` to `dst` with a single atomic rename that fails with `EEXIST`
/// rather than clobbering an existing `dst` (macOS `renamex_np(RENAME_EXCL)`).
///
/// This backs the same-volume move fast path, preserving the no-clobber
/// guarantee the copy path gets from `COPYFILE_EXCL`/`O_EXCL` without a
/// check-then-rename TOCTOU window.
pub fn rename_noreplace(src: &Path, dst: &Path) -> std::io::Result<()> {
    let src_c = path_cstring(src)?;
    let dst_c = path_cstring(dst)?;
    let rc = unsafe { renamex_np(src_c.as_ptr(), dst_c.as_ptr(), RENAME_EXCL) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

struct CallbackCtx {
    state: Arc<Mutex<TransferProgress>>,
    /// Bytes copied before this copyfile() call started.
    base_bytes: u64,
    /// Bytes of files fully completed within this call (recursive copies).
    done_in_call: u64,
}

fn cstr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .to_string(),
    )
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
            let mut s = crate::lock_util::recover(&ctx.state);
            if s.cancelled {
                return COPYFILE_QUIT;
            }
            if let Some(path) = cstr_to_string(src) {
                s.current_file = Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                s.current_file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                s.current_file_copied = 0;
            }
            COPYFILE_CONTINUE
        }
        (COPYFILE_RECURSE_FILE, COPYFILE_FINISH) => {
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
            // Record the failure and keep copying the rest; the caller
            // checks the error list before treating the op as successful.
            let mut s = crate::lock_util::recover(&ctx.state);
            let name = cstr_to_string(src).unwrap_or_else(|| s.current_file.clone());
            s.errors.push(format!("Failed to copy: {}", name));
            COPYFILE_CONTINUE
        }
        (COPYFILE_COPY_DATA, COPYFILE_PROGRESS) => {
            unsafe {
                let mut bytes_copied: i64 = 0;
                copyfile_state_get(
                    cstate,
                    COPYFILE_STATE_COPIED,
                    &mut bytes_copied as *mut _ as *mut std::ffi::c_void,
                );

                let mut s = crate::lock_util::recover(&ctx.state);
                if s.cancelled {
                    return COPYFILE_QUIT;
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

/// Copy a single file using native copyfile() with progress.
pub fn copy_file_native(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
    allow_clone: bool,
) -> std::io::Result<NativeCopyOutcome> {
    let src_c = path_cstring(src)?;
    let dst_c = path_cstring(dst)?;

    let file_size = src.metadata().map(|m| m.len()).unwrap_or(0);

    {
        let mut s = crate::lock_util::recover(state);
        s.current_file = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        s.current_file_size = file_size;
        s.current_file_copied = 0;
    }

    // Try the explicit clone primitive first so telemetry can distinguish a
    // metadata-only APFS clone from copyfile's transparent byte-copy fallback.
    if allow_clone && unsafe { clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) } == 0 {
        let mut progress = crate::lock_util::recover(state);
        progress.current_file_copied = file_size;
        progress.copied_bytes = base_bytes.saturating_add(file_size);
        return Ok(NativeCopyOutcome {
            bytes: file_size,
            cloned: true,
        });
    }

    unsafe {
        let cstate = copyfile_state_alloc();
        if cstate.is_null() {
            return Err(std::io::Error::last_os_error());
        }

        let mut ctx = CallbackCtx {
            state: state.clone(),
            base_bytes,
            done_in_call: 0,
        };

        let cb: copyfile_callback_t = progress_callback;
        copyfile_state_set(
            cstate,
            COPYFILE_STATE_STATUS_CB,
            cb as *const std::ffi::c_void,
        );
        copyfile_state_set(
            cstate,
            COPYFILE_STATE_STATUS_CTX,
            &mut ctx as *mut CallbackCtx as *const std::ffi::c_void,
        );

        // EXCL: the caller always hands us a destination that should not yet
        // exist (a fresh staging path, or a dest believed absent), so refuse
        // to clobber rather than overwrite if one races into being.
        let flags = COPYFILE_ALL | COPYFILE_CLONE | COPYFILE_EXCL;
        let result = copyfile(src_c.as_ptr(), dst_c.as_ptr(), cstate, flags);

        copyfile_state_free(cstate);

        if result != 0 {
            let err = std::io::Error::last_os_error();
            // Check if cancelled
            let s = crate::lock_util::recover(state);
            if s.cancelled {
                // Clean up partial file
                let _ = std::fs::remove_file(dst);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            return Err(err);
        }
    }

    // Finalize progress: a cloned file emits no DATA callbacks at all.
    {
        let mut s = crate::lock_util::recover(state);
        s.current_file_copied = file_size;
        s.copied_bytes = base_bytes.saturating_add(file_size);
    }

    Ok(NativeCopyOutcome {
        bytes: file_size,
        cloned: false,
    })
}

/// Copy a directory recursively using native copyfile() with COPYFILE_RECURSIVE.
pub fn copy_dir_native(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
) -> std::io::Result<u64> {
    let src_c = path_cstring(src)?;
    let dst_c = path_cstring(dst)?;

    {
        let mut s = crate::lock_util::recover(state);
        s.current_file = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
    }

    unsafe {
        let cstate = copyfile_state_alloc();
        if cstate.is_null() {
            return Err(std::io::Error::last_os_error());
        }

        let mut ctx = CallbackCtx {
            state: state.clone(),
            base_bytes,
            done_in_call: 0,
        };

        let cb: copyfile_callback_t = progress_callback;
        copyfile_state_set(
            cstate,
            COPYFILE_STATE_STATUS_CB,
            cb as *const std::ffi::c_void,
        );
        copyfile_state_set(
            cstate,
            COPYFILE_STATE_STATUS_CTX,
            &mut ctx as *mut CallbackCtx as *const std::ffi::c_void,
        );

        let flags = COPYFILE_ALL | COPYFILE_RECURSIVE | COPYFILE_CLONE | COPYFILE_EXCL;
        let result = copyfile(src_c.as_ptr(), dst_c.as_ptr(), cstate, flags);

        copyfile_state_free(cstate);

        if result != 0 {
            let s = crate::lock_util::recover(state);
            if s.cancelled {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            return Err(std::io::Error::last_os_error());
        }

        let copied = ctx.done_in_call;
        let mut s = crate::lock_util::recover(state);
        s.copied_bytes = base_bytes.saturating_add(copied);
        Ok(copied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn rename_noreplace_moves_into_a_free_name() {
        let tmp = TempDir::new();
        let src = tmp.file("a.txt", "hi");
        let dst = tmp.path().join("b.txt");
        rename_noreplace(&src, &dst).unwrap();
        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hi");
    }

    #[test]
    fn rename_noreplace_refuses_to_clobber_an_existing_destination() {
        let tmp = TempDir::new();
        let src = tmp.file("a.txt", "new");
        let dst = tmp.file("b.txt", "old");
        let err = rename_noreplace(&src, &dst).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        // Nothing was moved or clobbered.
        assert!(src.exists());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "old");
    }
}
