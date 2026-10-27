//! Non-recursive permission plans anchored to the listing the user selected.
use crate::{panel::ListingIdentity, path_identity::PathIdentity};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub identity: ListingIdentity,
    /// Explicit cross-folder selection has no cached listing row. Observe it
    /// on the worker before presenting a reviewable, identity-bound preview.
    pub capture_on_worker: bool,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub target: Target,
    pub mode: Option<u32>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    rows: Vec<Row>,
    all_bits: u32,
    any_bits: u32,
    unavailable: usize,
    count_cache: std::cell::Cell<Option<(Change, usize)>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub set: u32,
    pub clear: u32,
}

impl Change {
    pub fn apply(self, mode: u32) -> u32 {
        (mode | (self.set & 0o777)) & !(self.clear & 0o777)
    }
    pub fn exact(mode: u32) -> Self {
        Self {
            set: mode & 0o777,
            clear: !mode & 0o777,
        }
    }
    pub fn set_bit(&mut self, bit: u32, enabled: bool) {
        if enabled {
            self.set |= bit;
            self.clear &= !bit;
        } else {
            self.clear |= bit;
            self.set &= !bit;
        }
    }
}

pub fn parse_mode(text: &str) -> Result<u32, &'static str> {
    let text = text.trim();
    let digits = if text.len() == 4 && text.starts_with('0') {
        &text[1..]
    } else {
        text
    };
    if digits.len() != 3 || !digits.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
        return Err("Enter three octal digits, such as 644 or 0755");
    }
    u32::from_str_radix(digits, 8).map_err(|_| "Invalid octal mode")
}

impl Plan {
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
    pub fn ready(&self) -> bool {
        !self.rows.is_empty() && self.unavailable == 0
    }
    pub fn changed_count(&self, change: Change) -> usize {
        if let Some((cached, count)) = self.count_cache.get()
            && cached == change
        {
            return count;
        }
        let count = self
            .rows
            .iter()
            .filter(|row| row.mode.is_some_and(|mode| change.apply(mode) != mode))
            .count();
        self.count_cache.set(Some((change, count)));
        count
    }
    pub fn bit_state(&self, bit: u32) -> (bool, bool) {
        (
            self.all_bits & bit != 0,
            self.any_bits & bit != 0 && self.all_bits & bit == 0,
        )
    }
}

#[cfg(unix)]
fn checked_file(target: &Target) -> Result<(std::fs::File, u32), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if target.capture_on_worker {
        return Err("The selected path must be inspected before commit".into());
    }
    let ListingIdentity::Captured(expected) = &target.identity else {
        return Err("The listing has no identity proof; refresh it first".into());
    };
    if !matches!(
        expected.kind,
        Some(crate::path_identity::PathKind::File | crate::path_identity::PathKind::Directory)
    ) {
        return Err(
            "Only regular files and real directories are supported; links are not followed".into(),
        );
    }
    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY);
    let file = options
        .open(&target.path)
        .map_err(|error| format!("Could not open a protected handle: {error}"))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    let actual = PathIdentity::from_metadata(target.path.clone(), &metadata);
    if !expected.same_shallow_binding(&actual) {
        return Err("Item changed since the listing; refresh and select it again".into());
    }
    Ok((file, metadata.permissions().mode() & 0o7777))
}

#[cfg(not(unix))]
fn checked_file(_target: &Target) -> Result<(std::fs::File, u32), String> {
    Err("POSIX permissions are only supported on Unix hosts".into())
}

pub fn inspect(targets: Vec<Target>, cancelled: &dyn Fn() -> bool) -> Result<Plan, String> {
    let mut plan = Plan::default();
    let mut observed_mode = false;
    for mut target in targets {
        if cancelled() {
            return Err("Permission inspection cancelled".into());
        }
        if target.capture_on_worker {
            match PathIdentity::observe(&target.path) {
                Ok(identity) if identity.exists => {
                    target.identity = ListingIdentity::Captured(identity);
                    target.capture_on_worker = false;
                }
                Ok(_) => {
                    plan.unavailable += 1;
                    plan.rows.push(Row {
                        target,
                        mode: None,
                        error: Some("Selected item no longer exists".into()),
                    });
                    continue;
                }
                Err(error) => {
                    plan.unavailable += 1;
                    plan.rows.push(Row {
                        target,
                        mode: None,
                        error: Some(format!("Could not inspect selected item: {error}")),
                    });
                    continue;
                }
            }
        }
        let (mode, error) = match checked_file(&target) {
            Ok((_, mode)) => (Some(mode), None),
            Err(error) => (None, Some(error)),
        };
        if let Some(mode) = mode {
            if !observed_mode {
                plan.all_bits = mode;
                observed_mode = true;
            } else {
                plan.all_bits &= mode;
            }
            plan.any_bits |= mode;
        }
        if error.is_some() || mode.is_none() {
            plan.unavailable += 1;
        }
        plan.rows.push(Row {
            target,
            mode,
            error,
        });
    }
    Ok(plan)
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub changed: usize,
    pub unchanged: usize,
    pub cancelled: bool,
    pub errors: Vec<String>,
}

pub fn apply(plan: Plan, change: Change, cancelled: &dyn Fn() -> bool) -> Report {
    let mut report = Report::default();
    if !plan.ready() {
        report
            .errors
            .push("The permission preview contains unavailable items; no changes applied".into());
        return report;
    }
    let mut rows = plan.rows;
    // Explicit descendants must be changed before a selected ancestor can lose
    // the search/read bits needed to open their protected handles.
    rows.sort_by_cached_key(|row| std::cmp::Reverse(row.target.path.components().count()));
    for row in rows {
        if cancelled() {
            report.cancelled = true;
            break;
        }
        let before = row.mode.expect("ready plan");
        let desired = change.apply(before);
        if desired == before {
            report.unchanged += 1;
            continue;
        }
        let wrote = std::cell::Cell::new(false);
        let result = checked_file(&row.target).and_then(|(file, mode)| {
            if mode == desired {
                return Ok(());
            }
            if mode != before {
                return Err("Permissions changed after the preview; refresh the preview".into());
            }
            if cancelled() {
                return Err("Permission change cancelled".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(desired))
                    .map_err(|error| error.to_string())?;
                wrote.set(true);
                let observed = file
                    .metadata()
                    .map_err(|error| error.to_string())?
                    .permissions()
                    .mode()
                    & 0o7777;
                if observed != desired {
                    return Err(format!(
                        "Filesystem reported mode {observed:04o}, expected {desired:04o}"
                    ));
                }
                Ok(())
            }
            #[cfg(not(unix))]
            {
                let _ = file;
                Err("POSIX permissions are unavailable".into())
            }
        });
        match result {
            Ok(()) => {
                if wrote.get() {
                    report.changed += 1;
                } else {
                    report.unchanged += 1;
                }
            }
            Err(error) => {
                if wrote.get() {
                    report.changed += 1;
                }
                report
                    .errors
                    .push(format!("{}: {error}", row.target.path.display()));
            }
        }
    }
    report.cancelled |= cancelled();
    report
}

pub struct Run<T> {
    receiver: mpsc::Receiver<T>,
    cancelled: Arc<AtomicBool>,
}
impl<T> Run<T> {
    pub fn poll(&self) -> Result<T, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
impl<T> Drop for Run<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn start<T: Send + 'static>(
    root: PathBuf,
    mutation: bool,
    notify: Arc<dyn Fn() + Send + Sync>,
    work: impl FnOnce(&dyn Fn() -> bool) -> T + Send + 'static,
) -> Result<Run<T>, String> {
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let kind = if mutation {
        crate::workload::TaskKind::Transfer
    } else {
        crate::workload::TaskKind::PathProbe
    };
    crate::workload::submit(
        crate::workload::TaskSpec::new(kind, root, 0)
            .priority(crate::workload::Priority::Interactive),
        move |token| {
            let result = work(&|| token.is_cancelled() || worker_cancelled.load(Ordering::Acquire));
            let _ = sender.send(result);
            notify();
        },
    )
    .map_err(|error| format!("Permission worker could not start: {error:?}"))?;
    Ok(Run {
        receiver,
        cancelled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn octal_and_bit_edits_preserve_unedited_and_special_bits() {
        assert_eq!(parse_mode("0755"), Ok(0o755));
        assert_eq!(parse_mode("640"), Ok(0o640));
        for value in ["", "77", "7777", "888", "-1", "0x64", "abcd"] {
            assert!(parse_mode(value).is_err(), "{value}");
        }
        let mut change = Change::default();
        change.set_bit(0o020, false);
        change.set_bit(0o004, true);
        assert_eq!(change.apply(0o4750), 0o4754);
        assert_eq!(change.apply(0o660), 0o644);
        assert_eq!(Change::exact(0o755).apply(0o4640), 0o4755);
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use crate::testutil::TempDir;
        use std::{
            fs,
            os::unix::fs::{PermissionsExt, symlink},
        };
        fn target(path: &std::path::Path) -> Target {
            Target {
                path: path.into(),
                identity: ListingIdentity::Captured(PathIdentity::observe(path).unwrap()),
                capture_on_worker: false,
            }
        }
        fn mode(path: &std::path::Path) -> u32 {
            fs::metadata(path).unwrap().permissions().mode() & 0o7777
        }
        fn set(path: &std::path::Path, mode: u32) {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        }

        #[test]
        fn permission_edits_only_touch_selected_items_and_keep_contents() {
            let temp = TempDir::new();
            let file = temp.file("file.txt", "original bytes");
            let directory = temp.dir("folder");
            let child = directory.join("child.txt");
            fs::write(&child, "child bytes").unwrap();
            set(&file, 0o640);
            set(&directory, 0o750);
            set(&child, 0o640);
            let plan = inspect(vec![target(&file), target(&directory)], &|| false).unwrap();
            assert!(plan.ready());
            let change = Change::exact(0o700);
            assert_eq!(plan.changed_count(change), 2);
            let report = apply(plan, change, &|| false);
            assert_eq!(report.changed, 2);
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert_eq!(mode(&file), 0o700);
            assert_eq!(mode(&directory), 0o700);
            assert_eq!(mode(&child), 0o640);
            assert_eq!(fs::read_to_string(&file).unwrap(), "original bytes");
        }

        #[test]
        fn foreign_permission_preview_binds_identity_before_commit() {
            let temp = TempDir::new();
            let path = temp.file("foreign", "original");
            set(&path, 0o640);
            let plan = inspect(
                vec![Target {
                    path: path.clone(),
                    identity: ListingIdentity::Unavailable,
                    capture_on_worker: true,
                }],
                &|| false,
            )
            .unwrap();
            assert!(plan.ready());
            assert!(!plan.rows()[0].target.capture_on_worker);
            assert!(matches!(
                plan.rows()[0].target.identity,
                ListingIdentity::Captured(_)
            ));
            fs::rename(&path, temp.path().join("original")).unwrap();
            fs::write(&path, "replacement").unwrap();
            set(&path, 0o600);
            let report = apply(plan, Change::exact(0o755), &|| false);
            assert_eq!(report.changed, 0);
            assert_eq!(report.errors.len(), 1);
            assert_eq!(mode(&path), 0o600);
        }

        #[test]
        fn explicit_descendants_commit_before_ancestors_lose_access() {
            let temp = TempDir::new();
            let directory = temp.dir("selected-folder");
            let child = directory.join("selected-child");
            fs::write(&child, "bytes").unwrap();
            set(&directory, 0o750);
            set(&child, 0o640);
            let plan = inspect(vec![target(&directory), target(&child)], &|| false).unwrap();
            let report = apply(plan, Change::exact(0), &|| false);
            // Restore access before any assertion so a failed test still cleans up.
            let directory_mode = mode(&directory);
            set(&directory, 0o750);
            let child_mode = mode(&child);
            set(&child, 0o640);
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert_eq!(report.changed, 2);
            assert_eq!(directory_mode, 0);
            assert_eq!(child_mode, 0);
        }

        #[test]
        fn replacement_and_symlink_substitution_never_change_the_new_target() {
            let temp = TempDir::new();
            for link in [false, true] {
                let file = temp.file(if link { "link-case" } else { "file-case" }, "old");
                set(&file, 0o640);
                let plan = inspect(vec![target(&file)], &|| false).unwrap();
                fs::rename(&file, file.with_extension("old")).unwrap();
                let replacement = if link {
                    let replacement = temp.file("destination.txt", "other");
                    set(&replacement, 0o600);
                    symlink(&replacement, &file).unwrap();
                    replacement
                } else {
                    fs::write(&file, "replacement").unwrap();
                    set(&file, 0o600);
                    file.clone()
                };
                let report = apply(plan, Change::exact(0o755), &|| false);
                assert_eq!(report.changed, 0);
                assert_eq!(report.errors.len(), 1);
                assert_eq!(mode(&replacement), 0o600);
            }
        }

        #[test]
        fn stale_modes_and_preflight_errors_do_not_silently_apply() {
            let temp = TempDir::new();
            let file = temp.file("file", "bytes");
            set(&file, 0o640);
            let plan = inspect(vec![target(&file)], &|| false).unwrap();
            set(&file, 0o600);
            let report = apply(plan, Change::exact(0o644), &|| false);
            assert_eq!(report.changed, 0);
            assert_eq!(mode(&file), 0o600);
            assert!(report.errors[0].contains("Permissions changed"));
            let link = temp.path().join("link");
            symlink(&file, &link).unwrap();
            let plan = inspect(vec![target(&file), target(&link)], &|| false).unwrap();
            assert!(!plan.ready());
            let report = apply(plan, Change::exact(0o755), &|| false);
            assert_eq!(report.changed, 0);
            assert_eq!(mode(&file), 0o600);
        }

        #[test]
        fn hardlink_aliases_and_mixed_bit_states_are_handled() {
            let temp = TempDir::new();
            let file = temp.file("file", "bytes");
            set(&file, 0o640);
            let alias = temp.path().join("alias");
            fs::hard_link(&file, &alias).unwrap();
            let other = temp.file("other", "other");
            set(&other, 0o600);
            let plan = inspect(vec![target(&file), target(&other)], &|| false).unwrap();
            assert_eq!(plan.bit_state(0o400), (true, false));
            assert_eq!(plan.bit_state(0o040), (false, true));
            let plan = inspect(vec![target(&file), target(&alias)], &|| false).unwrap();
            let report = apply(plan, Change::exact(0o644), &|| false);
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert_eq!(report.changed, 1);
            assert_eq!(report.unchanged, 1);
            assert_eq!(mode(&alias), 0o644);
        }

        #[test]
        fn asynchronous_cancellation_reaches_the_worker_before_commit() {
            let temp = TempDir::new();
            let path = temp.file("async", "bytes");
            set(&path, 0o640);
            let plan = inspect(vec![target(&path)], &|| false).unwrap();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (go_tx, go_rx) = mpsc::channel();
            let run = start(temp.path().into(), true, Arc::new(|| {}), move |cancel| {
                let _ = ready_tx.send(());
                if go_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .is_err()
                {
                    return Report {
                        cancelled: true,
                        ..Default::default()
                    };
                }
                apply(plan, Change::exact(0o755), cancel)
            })
            .unwrap();
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            run.cancel();
            go_tx.send(()).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let report = loop {
                match run.poll() {
                    Ok(report) => break report,
                    Err(mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(1))
                    }
                    other => panic!("worker did not finish: {other:?}"),
                }
            };
            assert!(report.cancelled);
            assert_eq!(report.changed, 0);
            assert_eq!(mode(&path), 0o640);
        }

        #[test]
        fn cancellation_leaves_committed_changes_and_stops_before_the_next_item() {
            let temp = TempDir::new();
            let a = temp.file("a", "a");
            let b = temp.file("b", "b");
            set(&a, 0o640);
            set(&b, 0o640);
            let plan = inspect(vec![target(&a), target(&b)], &|| false).unwrap();
            let report = apply(plan, Change::exact(0o644), &|| mode(&a) == 0o644);
            assert_eq!(report.changed, 1);
            assert!(report.cancelled);
            assert_eq!(mode(&a), 0o644);
            assert_eq!(mode(&b), 0o640);
        }
    }
}
