//! Production adapters for desktop and filesystem effects.
//!
//! Keeping native calls here makes the app reducers deterministic and keeps
//! tests away from the user's clipboard, Trash, and launched applications.

use crate::ports::{
    ClipboardOutcome, ClipboardPort, FreeSpacePort, NativeFailure, NativeFailureKind, OpenOutcome,
    OpenRequest, OpenerPort, SpacePrecision, SpaceProbeOutcome, TrashItemOutcome, TrashPort,
    TrashTarget, VolumeRelation,
};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CString;
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;

fn is_main_thread() -> bool {
    unsafe { msg_send![class!(NSThread), isMainThread] }
}

#[derive(Debug)]
pub struct MacOsClipboard {
    _main_thread_only: PhantomData<Rc<()>>,
}

impl MacOsClipboard {
    pub fn new() -> Result<Self, NativeFailure> {
        if !is_main_thread() {
            return Err(NativeFailure {
                kind: NativeFailureKind::Busy,
                message: "clipboard adapter must be created on the main thread".to_string(),
            });
        }
        Ok(Self {
            _main_thread_only: PhantomData,
        })
    }
}

impl ClipboardPort for MacOsClipboard {
    fn write_text(&self, text: &str) -> ClipboardOutcome {
        if !is_main_thread() {
            return ClipboardOutcome::Failed(NativeFailure {
                kind: NativeFailureKind::Busy,
                message: "clipboard write must run on the main thread".to_string(),
            });
        }
        let Ok(c_text) = CString::new(text) else {
            return ClipboardOutcome::Failed(NativeFailure {
                kind: NativeFailureKind::InvalidInput,
                message: "clipboard text contains a NUL byte".to_string(),
            });
        };
        unsafe {
            let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
            let pasteboard: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            if pasteboard.is_null() {
                let _: () = msg_send![pool, drain];
                return ClipboardOutcome::Unsupported(NativeFailure::unsupported(
                    "the system pasteboard is unavailable",
                ));
            }
            let value: *mut Object =
                msg_send![class!(NSString), stringWithUTF8String: c_text.as_ptr()];
            if value.is_null() {
                let _: () = msg_send![pool, drain];
                return ClipboardOutcome::Failed(NativeFailure {
                    kind: NativeFailureKind::InvalidInput,
                    message: "clipboard text could not be represented by NSString".to_string(),
                });
            }
            let values: *mut Object = msg_send![class!(NSArray), arrayWithObject: value];
            let _: () = msg_send![pasteboard, clearContents];
            let committed: bool = msg_send![pasteboard, writeObjects: values];
            let _: () = msg_send![pool, drain];
            if committed {
                ClipboardOutcome::Committed
            } else {
                ClipboardOutcome::Failed(NativeFailure {
                    kind: NativeFailureKind::Unknown,
                    message: "the system pasteboard rejected the write".to_string(),
                })
            }
        }
    }
}

#[derive(Debug)]
pub struct MacOsOpener {
    _main_thread_only: PhantomData<Rc<()>>,
}

impl MacOsOpener {
    pub fn new() -> Result<Self, NativeFailure> {
        if !is_main_thread() {
            return Err(NativeFailure {
                kind: NativeFailureKind::Busy,
                message: "opener adapter must be created on the main thread".to_string(),
            });
        }
        Ok(Self {
            _main_thread_only: PhantomData,
        })
    }
}

impl OpenerPort for MacOsOpener {
    fn open(&self, request: &OpenRequest) -> OpenOutcome {
        if !is_main_thread() {
            return OpenOutcome::Failed(NativeFailure {
                kind: NativeFailureKind::Busy,
                message: "opening an application must run on the main thread".to_string(),
            });
        }
        let result = match request {
            OpenRequest::OpenPath(path) => open::that(path),
            OpenRequest::Reveal(path) => std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn()
                .map(|_| ()),
            OpenRequest::OpenWith { path, application } => std::process::Command::new("open")
                .arg("-a")
                .arg(application)
                .arg(path)
                .spawn()
                .map(|_| ()),
            OpenRequest::QuickLook(path) => std::process::Command::new("qlmanage")
                .arg("-p")
                .arg(path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map(|_| ()),
            OpenRequest::GetInfo(path) => std::process::Command::new("osascript")
                .arg("-e")
                .arg("on run argv")
                .arg("-e")
                .arg(
                    "tell application \"Finder\" to open information window of \
                     (POSIX file (item 1 of argv) as alias)",
                )
                .arg("-e")
                .arg("end run")
                .arg("--")
                .arg(path)
                .spawn()
                .map(|_| ()),
        };
        match result {
            Ok(()) => OpenOutcome::Accepted,
            Err(error) => OpenOutcome::Failed(NativeFailure::from_io(&error)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeTrash;

impl TrashPort for NativeTrash {
    fn move_to_trash(&self, target: &TrashTarget) -> TrashItemOutcome {
        let current = match crate::path_identity::PathIdentity::observe(&target.path) {
            Ok(identity) => identity,
            Err(error) => {
                return TrashItemOutcome::Failed(NativeFailure::from_io(&error));
            }
        };
        if !current.exists {
            return TrashItemOutcome::Missing;
        }
        if !target.expected.same_binding(&current) {
            return TrashItemOutcome::StaleBinding;
        }
        match trash::delete(&target.path) {
            Ok(()) => TrashItemOutcome::Trashed,
            Err(error) => TrashItemOutcome::Failed(classify_trash_failure(error)),
        }
    }
}

fn classify_trash_failure(error: trash::Error) -> NativeFailure {
    match error {
        trash::Error::Os { code, description } => {
            let mut failure = NativeFailure::from_io(&std::io::Error::from_raw_os_error(code));
            if !description.is_empty() {
                failure.message = description;
            }
            failure
        }
        #[cfg(all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        ))]
        trash::Error::FileSystem { source, .. } => NativeFailure::from_io(&source),
        trash::Error::TargetedRoot => NativeFailure {
            kind: NativeFailureKind::InvalidInput,
            message: "filesystem roots cannot be moved to Trash".to_string(),
        },
        trash::Error::CouldNotAccess { target } => NativeFailure {
            kind: NativeFailureKind::Unknown,
            message: format!("Trash could not access {target}"),
        },
        trash::Error::CanonicalizePath { original } => NativeFailure {
            kind: NativeFailureKind::Stale,
            message: format!(
                "Trash could not resolve the parent of {}",
                original.display()
            ),
        },
        trash::Error::ConvertOsString { original } => NativeFailure {
            kind: NativeFailureKind::InvalidInput,
            message: format!("Trash could not represent path {original:?}"),
        },
        trash::Error::Unknown { description } => NativeFailure {
            kind: NativeFailureKind::Unknown,
            message: description,
        },
        other @ (trash::Error::RestoreCollision { .. } | trash::Error::RestoreTwins { .. }) => {
            NativeFailure {
                kind: NativeFailureKind::Unknown,
                message: other.to_string(),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFreeSpace;

impl FreeSpacePort for NativeFreeSpace {
    fn probe(&self, path: &Path) -> SpaceProbeOutcome {
        probe_free_space(path)
    }

    fn volume_relation(&self, source: &Path, target: &Path) -> VolumeRelation {
        volume_relation(source, target)
    }
}

#[cfg(unix)]
fn probe_free_space(path: &Path) -> SpaceProbeOutcome {
    const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    let path = path.to_path_buf();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let spawn = std::thread::Builder::new()
        .name("statvfs-probe".to_string())
        .spawn(move || {
            let _ = sender.send(probe_free_space_blocking(&path));
        });
    if let Err(error) = spawn {
        return SpaceProbeOutcome::Unknown(NativeFailure {
            kind: NativeFailureKind::Unknown,
            message: format!("Could not start free-space probe: {error}"),
        });
    }
    match receiver.recv_timeout(PROBE_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            SpaceProbeOutcome::Unknown(NativeFailure {
                kind: NativeFailureKind::Busy,
                message: format!(
                    "free-space probe timed out after {}s",
                    PROBE_TIMEOUT.as_secs()
                ),
            })
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            SpaceProbeOutcome::Unknown(NativeFailure {
                kind: NativeFailureKind::Unknown,
                message: "free-space probe stopped before publishing a result".to_string(),
            })
        }
    }
}

#[cfg(unix)]
fn probe_free_space_blocking(path: &Path) -> SpaceProbeOutcome {
    use std::os::unix::ffi::OsStrExt;

    let path = match CString::new(path.as_os_str().as_bytes()) {
        Ok(path) => path,
        Err(_) => {
            return SpaceProbeOutcome::Unknown(NativeFailure {
                kind: NativeFailureKind::InvalidInput,
                message: "path contains a NUL byte".to_string(),
            });
        }
    };
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return SpaceProbeOutcome::Unknown(
            NativeFailure::from_io(&std::io::Error::last_os_error()),
        );
    }
    let stats = unsafe { stats.assume_init() };
    classify_space_product(stats.f_bavail as u128, stats.f_frsize as u128)
}

#[cfg(not(unix))]
fn probe_free_space(_path: &Path) -> SpaceProbeOutcome {
    SpaceProbeOutcome::Unknown(NativeFailure::unsupported(
        "free-space probing is not supported on this platform",
    ))
}

fn classify_space_product(blocks: u128, block_size: u128) -> SpaceProbeOutcome {
    let bytes = blocks.saturating_mul(block_size);
    if bytes > u128::from(u64::MAX) {
        SpaceProbeOutcome::Known {
            bytes: u64::MAX,
            precision: SpacePrecision::SaturatedLowerBound,
        }
    } else {
        SpaceProbeOutcome::Known {
            bytes: bytes as u64,
            precision: SpacePrecision::Exact,
        }
    }
}

#[cfg(unix)]
fn volume_relation(source: &Path, target: &Path) -> VolumeRelation {
    use std::os::unix::fs::MetadataExt;

    match (std::fs::metadata(source), std::fs::metadata(target)) {
        (Ok(source), Ok(target)) if source.dev() == target.dev() => VolumeRelation::Same,
        (Ok(_), Ok(_)) => VolumeRelation::Different,
        (Err(error), _) | (_, Err(error)) => {
            VolumeRelation::Unknown(NativeFailure::from_io(&error))
        }
    }
}

#[cfg(not(unix))]
fn volume_relation(_source: &Path, _target: &Path) -> VolumeRelation {
    VolumeRelation::Unknown(NativeFailure::unsupported(
        "volume identity is not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_product_distinguishes_exact_and_saturated_values() {
        assert_eq!(
            classify_space_product(4, 512),
            SpaceProbeOutcome::Known {
                bytes: 2048,
                precision: SpacePrecision::Exact,
            }
        );
        assert_eq!(
            classify_space_product(u128::from(u64::MAX), 2),
            SpaceProbeOutcome::Known {
                bytes: u64::MAX,
                precision: SpacePrecision::SaturatedLowerBound,
            }
        );
    }
}
