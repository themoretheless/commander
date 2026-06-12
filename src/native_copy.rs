//! Native macOS `copyfile()` with progress callback.
//!
//! Uses APFS clone (instant) when source and dest are on the same APFS volume,
//! falls back to byte-copy with xattr/ACL preservation otherwise.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_uint};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::app::TransferProgress;

// copyfile flags
const COPYFILE_ALL: u32 = 0x000F; // DATA + STAT + ACL + XATTR
const COPYFILE_RECURSIVE: u32 = 0x8000;
const COPYFILE_CLONE: u32 = 1 << 24; // try clone first
const COPYFILE_DATA: u32 = 0x0001;

// Status callback constants
const COPYFILE_COPY: c_int = 3;
const COPYFILE_RECURSE_FILE: c_int = 1;
const COPYFILE_RECURSE_DIR: c_int = 2;
const COPYFILE_RECURSE_DIR_CLEANUP: c_int = 4;
const COPYFILE_RECURSE_ERROR: c_int = 3;
const COPYFILE_PROGRESS: c_int = 7;

const COPYFILE_CONTINUE: c_int = 0;
const COPYFILE_QUIT: c_int = 2;

// copyfile_state keys
const COPYFILE_STATE_STATUS_CB: u32 = 6;
const COPYFILE_STATE_STATUS_CTX: u32 = 7;
const COPYFILE_STATE_COPIED: u32 = 8; // bytes copied so far
const COPYFILE_STATE_SRC_FILENAME: u32 = 10;

#[allow(non_camel_case_types)]
type copyfile_state_t = *mut std::ffi::c_void;
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
    fn copyfile_state_set(
        s: copyfile_state_t,
        flag: u32,
        value: *const std::ffi::c_void,
    ) -> c_int;
    fn copyfile_state_get(
        s: copyfile_state_t,
        flag: u32,
        value: *mut std::ffi::c_void,
    ) -> c_int;
    fn copyfile(
        from: *const c_char,
        to: *const c_char,
        state: copyfile_state_t,
        flags: c_uint,
    ) -> c_int;
}

struct CallbackCtx {
    state: Arc<Mutex<TransferProgress>>,
    file_base_bytes: u64, // bytes copied before current file
}

extern "C" fn progress_callback(
    what: c_int,
    stage: c_int,
    cstate: copyfile_state_t,
    _src: *const c_char,
    _dst: *const c_char,
    ctx: *mut std::ffi::c_void,
) -> c_int {
    let ctx = unsafe { &mut *(ctx as *mut CallbackCtx) };

    match what {
        COPYFILE_RECURSE_FILE => {
            // New file started — update current filename
            unsafe {
                let mut name_ptr: *const c_char = std::ptr::null();
                copyfile_state_get(
                    cstate,
                    COPYFILE_STATE_SRC_FILENAME,
                    &mut name_ptr as *mut _ as *mut std::ffi::c_void,
                );
                if !name_ptr.is_null() {
                    let name = std::ffi::CStr::from_ptr(name_ptr)
                        .to_string_lossy()
                        .to_string();
                    let short = Path::new(&name)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or(name);
                    let mut s = ctx.state.lock().unwrap();
                    s.current_file = short;
                    s.current_file_copied = 0;
                    // Try to get file size
                    if let Ok(meta) = std::fs::metadata(
                        std::ffi::CStr::from_ptr(name_ptr).to_string_lossy().as_ref(),
                    ) {
                        s.current_file_size = meta.len();
                    }
                }
            }
            COPYFILE_CONTINUE
        }
        COPYFILE_RECURSE_DIR => COPYFILE_CONTINUE,
        COPYFILE_RECURSE_DIR_CLEANUP => {
            let mut s = ctx.state.lock().unwrap();
            s.files_done += 1;
            COPYFILE_CONTINUE
        }
        COPYFILE_RECURSE_ERROR => COPYFILE_CONTINUE, // skip errors
        COPYFILE_COPY | COPYFILE_PROGRESS => {
            // Progress update — get bytes copied
            unsafe {
                let mut bytes_copied: i64 = 0;
                copyfile_state_get(
                    cstate,
                    COPYFILE_STATE_COPIED,
                    &mut bytes_copied as *mut _ as *mut std::ffi::c_void,
                );

                let mut s = ctx.state.lock().unwrap();
                if s.cancelled {
                    return COPYFILE_QUIT;
                }

                let current_copied = bytes_copied.max(0) as u64;
                s.current_file_copied = current_copied;
                s.copied_bytes = ctx.file_base_bytes + current_copied;

                if s.started_at.elapsed().as_millis() % 500 < 50 {
                    s.record_sample();
                }
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
) -> std::io::Result<u64> {
    let src_c = CString::new(src.to_string_lossy().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let dst_c = CString::new(dst.to_string_lossy().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let file_size = src.metadata().map(|m| m.len()).unwrap_or(0);

    {
        let mut s = state.lock().unwrap();
        s.current_file = src.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        s.current_file_size = file_size;
        s.current_file_copied = 0;
    }

    unsafe {
        let cstate = copyfile_state_alloc();
        if cstate.is_null() {
            return Err(std::io::Error::last_os_error());
        }

        let mut ctx = CallbackCtx {
            state: state.clone(),
            file_base_bytes: base_bytes,
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

        let flags = COPYFILE_ALL | COPYFILE_CLONE;
        let result = copyfile(src_c.as_ptr(), dst_c.as_ptr(), cstate, flags);

        copyfile_state_free(cstate);

        if result != 0 {
            let err = std::io::Error::last_os_error();
            // Check if cancelled
            let s = state.lock().unwrap();
            if s.cancelled {
                // Clean up partial file
                let _ = std::fs::remove_file(dst);
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
            }
            return Err(err);
        }
    }

    Ok(file_size)
}

/// Copy a directory recursively using native copyfile() with COPYFILE_RECURSIVE.
pub fn copy_dir_native(
    src: &Path,
    dst: &Path,
    state: &Arc<Mutex<TransferProgress>>,
    base_bytes: u64,
) -> std::io::Result<u64> {
    let src_c = CString::new(src.to_string_lossy().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let dst_c = CString::new(dst.to_string_lossy().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    {
        let mut s = state.lock().unwrap();
        s.current_file = src.file_name()
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
            file_base_bytes: base_bytes,
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

        let flags = COPYFILE_ALL | COPYFILE_RECURSIVE | COPYFILE_CLONE;
        let result = copyfile(src_c.as_ptr(), dst_c.as_ptr(), cstate, flags);

        copyfile_state_free(cstate);

        if result != 0 {
            let s = state.lock().unwrap();
            if s.cancelled {
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
            }
            return Err(std::io::Error::last_os_error());
        }
    }

    // Return approximate bytes
    let s = state.lock().unwrap();
    Ok(s.copied_bytes - base_bytes)
}
