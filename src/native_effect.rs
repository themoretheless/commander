//! Production adapters for desktop and filesystem effects.
//!
//! Keeping native calls here makes the app reducers deterministic and keeps
//! tests away from the user's clipboard, Trash, and launched applications.

use crate::ports::{
    ClipboardOutcome, ClipboardPort, FreeSpacePort, NativeFailure, NativeFailureKind, OpenOutcome,
    OpenRequest, OpenerPort, SpacePrecision, SpaceProbeOutcome, TrashItemOutcome, TrashPort,
    TrashTarget, VolumeRelation,
};
use std::path::Path;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CString;
    use std::marker::PhantomData;
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
                OpenRequest::Reveal(path) => spawn_reaped_logging_failure(
                    std::process::Command::new("open").arg("-R").arg(path),
                    format!("Reveal in Finder for {}", path.display()),
                ),
                OpenRequest::OpenWith { path, application } => spawn_reaped_logging_failure(
                    std::process::Command::new("open")
                        .arg("-a")
                        .arg(application)
                        .arg(path),
                    format!("Opening {} with {application:?}", path.display()),
                ),
                OpenRequest::QuickLook(path) => spawn_reaped(
                    std::process::Command::new("qlmanage")
                        .arg("-p")
                        .arg(path)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null()),
                ),
                OpenRequest::GetInfo(path) => spawn_reaped_logging_failure(
                    std::process::Command::new("osascript")
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
                        .arg(path),
                    format!(
                        "Get Info for {} (if Finder automation was denied, allow it in System Settings > Privacy & Security > Automation)",
                        path.display()
                    ),
                ),
            };
            match result {
                Ok(()) => OpenOutcome::Accepted,
                Err(error) => OpenOutcome::Failed(NativeFailure::from_io(&error)),
            }
        }
    }
}
#[cfg(target_os = "macos")]
pub use appkit::{MacOsClipboard, MacOsOpener};

/// Launch a helper process without leaving a zombie: dropping a `Child` does
/// not reap it, so a detached thread waits for it to exit.
#[cfg(any(target_os = "macos", test))]
fn spawn_reaped(command: &mut std::process::Command) -> std::io::Result<()> {
    spawn_reaped_with_handle(command).map(drop)
}

#[cfg(any(target_os = "macos", test))]
fn spawn_reaped_with_handle(
    command: &mut std::process::Command,
) -> std::io::Result<Option<std::thread::JoinHandle<()>>> {
    let mut child = command.spawn()?;
    // The process is already running; if no reaper thread can start, the
    // launch still succeeded and only the exit status goes uncollected.
    Ok(std::thread::Builder::new()
        .name("child-reaper".to_string())
        .spawn(move || {
            let _ = child.wait();
        })
        .ok())
}

/// Launch a helper like [`spawn_reaped`], but have the reaper log a failed
/// exit with the helper's stderr: the launch itself is reported as accepted,
/// so without this a helper that fails (Finder automation denied, disk full)
/// leaves no trace. `what` names the operation and its path.
pub fn spawn_reaped_logging_failure(
    command: &mut std::process::Command,
    what: String,
) -> std::io::Result<()> {
    spawn_reaped_reporting(command, what, |message| {
        log::warn!(target: "commander::native_effect", "{message}");
    })
    .map(drop)
}

fn spawn_reaped_reporting(
    command: &mut std::process::Command,
    what: String,
    on_failure: impl FnOnce(String) + Send + 'static,
) -> std::io::Result<Option<std::thread::JoinHandle<()>>> {
    let mut child = command.stderr(std::process::Stdio::piped()).spawn()?;
    Ok(std::thread::Builder::new()
        .name("child-reaper".to_string())
        .spawn(move || {
            let stderr = child
                .stderr
                .take()
                .map(read_bounded_stderr)
                .unwrap_or_default();
            let message = match child.wait() {
                Ok(status) => helper_failure_message(&what, status, &stderr),
                Err(error) => Some(format!(
                    "{what}: could not collect the exit status: {error}"
                )),
            };
            if let Some(message) = message {
                on_failure(message);
            }
        })
        .ok())
}

/// Keep the head of a helper's stderr but drain the rest, so a chatty helper
/// cannot block on a full pipe before it exits.
fn read_bounded_stderr(mut stderr: impl std::io::Read) -> Vec<u8> {
    const KEEP: usize = 4 * 1024;
    let mut kept = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let room = KEEP.saturating_sub(kept.len()).min(read);
                kept.extend_from_slice(&buffer[..room]);
            }
        }
    }
    kept
}

fn helper_failure_message(
    what: &str,
    status: std::process::ExitStatus,
    stderr: &[u8],
) -> Option<String> {
    if status.success() {
        return None;
    }
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    Some(if stderr.is_empty() {
        format!("{what} failed ({status})")
    } else {
        format!("{what} failed ({status}): {stderr}")
    })
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
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A filesystem probe that may block on a hung mount, keyed so a repeat of
/// one still stuck in the kernel does not start another thread.
#[cfg(any(unix, test))]
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum ProbeKey {
    FreeSpace(std::path::PathBuf),
    Relation(std::path::PathBuf, std::path::PathBuf),
}

#[cfg(any(unix, test))]
#[cfg_attr(not(unix), allow(dead_code))]
fn in_flight_probes() -> &'static std::sync::Mutex<std::collections::HashSet<ProbeKey>> {
    static IN_FLIGHT: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<ProbeKey>>> =
        std::sync::OnceLock::new();
    IN_FLIGHT.get_or_init(Default::default)
}

/// Why [`run_bounded`] produced no result.
#[cfg(any(unix, test))]
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
enum BoundedFailure {
    /// The same probe is still blocked from an earlier call.
    StillBlocked,
    TimedOut,
    SpawnFailed(String),
    Disconnected,
}

/// Run `work` on a helper thread and wait at most `timeout`. A timed-out
/// thread is abandoned (a blocked `stat`/`statvfs` cannot be interrupted) but
/// stays registered under `key` until it returns, so repeated calls on a hung
/// mount fail fast instead of accumulating blocked threads.
#[cfg(any(unix, test))]
#[cfg_attr(not(unix), allow(dead_code))]
fn run_bounded<T: Send + 'static>(
    key: ProbeKey,
    timeout: std::time::Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, BoundedFailure> {
    if !crate::lock_util::recover(in_flight_probes()).insert(key.clone()) {
        return Err(BoundedFailure::StillBlocked);
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let worker_key = key.clone();
    let spawn = std::thread::Builder::new()
        .name("fs-probe".to_string())
        .spawn(move || {
            // Released on return and on unwind, so a panicking probe does not
            // leave its key marked busy for the rest of the process.
            struct Release(ProbeKey);
            impl Drop for Release {
                fn drop(&mut self) {
                    crate::lock_util::recover(in_flight_probes()).remove(&self.0);
                }
            }
            let release = Release(worker_key);
            let outcome = work();
            drop(release);
            let _ = sender.send(outcome);
        });
    if let Err(error) = spawn {
        crate::lock_util::recover(in_flight_probes()).remove(&key);
        return Err(BoundedFailure::SpawnFailed(error.to_string()));
    }
    match receiver.recv_timeout(timeout) {
        Ok(outcome) => Ok(outcome),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(BoundedFailure::TimedOut),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(BoundedFailure::Disconnected),
    }
}

#[cfg(unix)]
fn bounded_failure(what: &str, failure: BoundedFailure) -> NativeFailure {
    match failure {
        BoundedFailure::StillBlocked => NativeFailure {
            kind: NativeFailureKind::Busy,
            message: format!("{what} is still blocked from an earlier attempt"),
        },
        BoundedFailure::TimedOut => NativeFailure {
            kind: NativeFailureKind::Busy,
            message: format!("{what} timed out after {}s", PROBE_TIMEOUT.as_secs()),
        },
        BoundedFailure::SpawnFailed(error) => NativeFailure {
            kind: NativeFailureKind::Unknown,
            message: format!("Could not start {what}: {error}"),
        },
        BoundedFailure::Disconnected => NativeFailure {
            kind: NativeFailureKind::Unknown,
            message: format!("{what} stopped before publishing a result"),
        },
    }
}

#[cfg(unix)]
fn probe_free_space(path: &Path) -> SpaceProbeOutcome {
    let owned = path.to_path_buf();
    run_bounded(
        ProbeKey::FreeSpace(path.to_path_buf()),
        PROBE_TIMEOUT,
        move || probe_free_space_blocking(&owned),
    )
    .unwrap_or_else(|failure| {
        SpaceProbeOutcome::Unknown(bounded_failure("free-space probe", failure))
    })
}

#[cfg(unix)]
fn probe_free_space_blocking(path: &Path) -> SpaceProbeOutcome {
    use std::os::unix::ffi::OsStrExt;

    let path = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
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

/// Stats both paths under the same timeout as the free-space probe, so a hung
/// target mount cannot block the space-probe worker before `probe` runs.
#[cfg(unix)]
fn volume_relation(source: &Path, target: &Path) -> VolumeRelation {
    let (owned_source, owned_target) = (source.to_path_buf(), target.to_path_buf());
    run_bounded(
        ProbeKey::Relation(source.to_path_buf(), target.to_path_buf()),
        PROBE_TIMEOUT,
        move || volume_relation_blocking(&owned_source, &owned_target),
    )
    .unwrap_or_else(|failure| VolumeRelation::Unknown(bounded_failure("volume probe", failure)))
}

#[cfg(unix)]
fn volume_relation_blocking(source: &Path, target: &Path) -> VolumeRelation {
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
    use std::time::Duration;

    #[test]
    fn a_still_blocked_probe_is_not_spawned_again() {
        let key = ProbeKey::FreeSpace("/test/hung-mount".into());
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let first = run_bounded(key.clone(), Duration::from_millis(20), move || {
            let _ = blocked.recv();
            1
        });
        assert_eq!(first, Err(BoundedFailure::TimedOut));
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran_in_worker = ran.clone();
        let second = run_bounded(key.clone(), Duration::from_secs(5), move || {
            ran_in_worker.store(true, std::sync::atomic::Ordering::SeqCst);
            2
        });
        assert_eq!(second, Err(BoundedFailure::StillBlocked));
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));

        // Once the stuck call returns, the key is free again.
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match run_bounded(key.clone(), Duration::from_secs(5), || 3) {
                Ok(value) => {
                    assert_eq!(value, 3);
                    break;
                }
                Err(BoundedFailure::StillBlocked) if std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn a_panicking_probe_releases_its_key() {
        let key = ProbeKey::FreeSpace("/test/panicking-probe".into());
        let first: Result<u8, _> = run_bounded(key.clone(), Duration::from_secs(5), || {
            panic!("probe failed")
        });
        assert_eq!(first, Err(BoundedFailure::Disconnected));
        assert_eq!(run_bounded(key, Duration::from_secs(5), || 4), Ok(4));
    }

    #[cfg(unix)]
    #[test]
    fn volume_relation_still_classifies_same_volume() {
        let temp = crate::testutil::TempDir::new();
        let a = temp.file("a", "");
        let b = temp.file("b", "");
        assert_eq!(volume_relation(&a, &b), VolumeRelation::Same);
    }

    #[cfg(unix)]
    #[test]
    fn spawned_helpers_are_reaped() {
        let handle = spawn_reaped_with_handle(&mut std::process::Command::new("true"))
            .unwrap()
            .expect("reaper thread starts");
        // The reaper finishes only after wait() has collected the child.
        handle.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_helpers_report_status_and_stderr() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = spawn_reaped_reporting(
            std::process::Command::new("sh").args(["-c", "echo denied >&2; exit 3"]),
            "Get Info for /tmp/x".to_string(),
            move |message| sender.send(message).unwrap(),
        )
        .unwrap()
        .expect("reaper thread starts");
        handle.join().unwrap();

        let message = receiver.try_recv().expect("a failed exit is reported");
        assert!(
            message.starts_with("Get Info for /tmp/x failed"),
            "{message}"
        );
        assert!(message.contains('3'), "{message}");
        assert!(message.ends_with(": denied"), "{message}");

        let (sender, receiver) = std::sync::mpsc::channel();
        spawn_reaped_reporting(
            &mut std::process::Command::new("true"),
            "ok".to_string(),
            move |message| sender.send(message).unwrap(),
        )
        .unwrap()
        .expect("reaper thread starts")
        .join()
        .unwrap();
        assert!(receiver.try_recv().is_err(), "success is not reported");
    }

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
