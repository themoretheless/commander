//! Explicit timestamp edits over the metadata editor's protected, captured targets.
use crate::permissions::{Binding, Plan, Report, checked_metadata};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub modified: SystemTime,
    pub accessed: SystemTime,
}
impl Snapshot {
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
        Some(Self {
            modified: metadata.modified().ok()?,
            accessed: metadata.accessed().ok()?,
        })
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
}
impl Change {
    pub fn parse(modified: &str, accessed: &str) -> Result<Self, String> {
        let parse = |value: &str| {
            if value.trim().is_empty() {
                return Ok(None);
            }
            if value.len() > 64 {
                return Err("A timestamp is limited to 64 bytes".into());
            }
            crate::query::parse_absolute_time(value.trim())
                .map(Some)
                .map_err(|_| {
                    "Use YYYY-MM-DD (midnight UTC) or RFC 3339 with an explicit timezone"
                        .to_string()
                })
        };
        Ok(Self {
            modified: parse(modified)?,
            accessed: parse(accessed)?,
        })
    }
    pub fn changed(self, snapshot: Snapshot) -> bool {
        self.modified.is_some_and(|t| t != snapshot.modified)
            || self.accessed.is_some_and(|t| t != snapshot.accessed)
    }
    fn conflicts(self, before: Snapshot, current: Snapshot) -> bool {
        self.modified
            .is_some_and(|t| current.modified != before.modified && current.modified != t)
            || self
                .accessed
                .is_some_and(|t| current.accessed != before.accessed && current.accessed != t)
    }
    fn file_times(self) -> std::fs::FileTimes {
        let mut times = std::fs::FileTimes::new();
        if let Some(t) = self.modified {
            times = times.set_modified(t);
        }
        if let Some(t) = self.accessed {
            times = times.set_accessed(t);
        }
        times
    }
}
pub fn ready(plan: &Plan) -> bool {
    plan.times_ready()
}
pub fn changed_count(plan: &Plan, change: Change) -> usize {
    plan.changed_times_count(change)
}

/// Stable explicit UTC display, including nanoseconds and pre-epoch timestamps.
pub fn format(time: SystemTime) -> String {
    let (secs, nanos) = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (i64::try_from(d.as_secs()).ok(), d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            let s = i64::try_from(d.as_secs()).ok().and_then(i64::checked_neg);
            if d.subsec_nanos() == 0 {
                (s, 0)
            } else {
                (
                    s.and_then(|s| s.checked_sub(1)),
                    1_000_000_000 - d.subsec_nanos(),
                )
            }
        }
    };
    secs.and_then(|s| chrono::DateTime::<chrono::Utc>::from_timestamp(s, nanos))
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        .unwrap_or_else(|| "Outside display range".into())
}

pub fn apply(plan: Plan, change: Change, cancelled: &dyn Fn() -> bool) -> Report {
    let mut report = Report::default();
    if !ready(&plan) {
        report
            .errors
            .push("The timestamp preview contains unavailable items; no changes applied".into());
        return report;
    }
    if change == Change::default() {
        report.unchanged = plan.rows().len();
        return report;
    }
    // Do not skip a row merely because it matched during preview: every edited
    // field and pathname binding must still be checked at commit.
    for row in plan.rows() {
        if cancelled() {
            report.cancelled = true;
            break;
        }
        let wrote = std::cell::Cell::new(false);
        let result=checked_metadata(&row.target,Binding::Object).and_then(|(file,metadata)| {
            let current=Snapshot::from_metadata(&metadata).ok_or("Current file times are unavailable")?;
            let before=row.times.expect("ready timestamp plan");
            if change.conflicts(before,current){return Err("An edited timestamp changed after preview; reload before applying".into());}
            if !change.changed(current){return Ok(());}
            if cancelled(){return Err("Timestamp change cancelled".into());}
            file.set_times(change.file_times()).map_err(|e|e.to_string())?;wrote.set(true);
            let actual=file.metadata().map_err(|e|e.to_string()).and_then(|m|Snapshot::from_metadata(&m).ok_or_else(||"Could not verify changed timestamps".into()))?;
            if change.changed(actual){return Err("Filesystem timestamp precision or reported value differs from the requested value".into());}
            Ok(())
        });
        if wrote.get() {
            report.changed += 1;
        }
        match result {
            Ok(()) if !wrote.get() => report.unchanged += 1,
            Ok(()) => {}
            Err(error) => report
                .errors
                .push(format!("{}: {error}", row.target.path.display())),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_parse_offsets_fractional_seconds_and_pre_epoch_without_local_time_guessing() {
        assert_eq!(Change::parse(" ", "").unwrap(), Change::default());
        for text in [
            "1969-12-31T23:59:59.123456789Z",
            "2026-10-31",
            "2026-10-31T14:30:00+04:00",
        ] {
            let t = Change::parse(text, "").unwrap().modified.unwrap();
            assert_eq!(Change::parse(&format(t), "").unwrap().modified, Some(t));
        }
        assert_eq!(
            Change::parse("2026-10-31T14:30:00+04:00", "").unwrap(),
            Change::parse("2026-10-31T10:30:00Z", "").unwrap()
        );
        for bad in [
            "now",
            "2026-02-30",
            "2026-10-31T14:30:00",
            "2016-12-31T23:59:60Z",
            "99999999-01-01",
        ] {
            assert!(Change::parse(bad, "").is_err());
        }
        assert!(Change::parse(&"x".repeat(65), "").is_err());
    }
    #[cfg(unix)]
    mod unix {
        use super::*;
        use crate::{
            panel::ListingIdentity,
            path_identity::PathIdentity,
            permissions::{self, Target},
            testutil::TempDir,
        };
        use std::{fs, path::Path};
        fn target(path: &Path) -> Target {
            Target {
                path: path.to_owned(),
                identity: ListingIdentity::Captured(PathIdentity::observe(path).unwrap()),
                capture_on_worker: false,
            }
        }
        fn snapshot(path: &Path) -> Snapshot {
            Snapshot::from_metadata(&fs::metadata(path).unwrap()).unwrap()
        }
        fn seed(path: &Path) {
            fs::File::open(path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(time("2020-01-02T03:04:05Z"))
                        .set_accessed(time("2021-02-03T04:05:06Z")),
                )
                .unwrap();
        }
        fn time(value: &str) -> SystemTime {
            crate::query::parse_absolute_time(value).unwrap()
        }
        fn plan(paths: &[&Path]) -> Plan {
            permissions::inspect(paths.iter().map(|p| target(p)).collect(), &|| false).unwrap()
        }
        fn change() -> Change {
            Change::parse("2026-10-31T10:30:00.123456789Z", "2025-01-02T03:04:05Z").unwrap()
        }

        #[test]
        fn timestamp_edits_touch_only_selected_files_and_folders_and_preserve_payloads() {
            let temp = TempDir::new();
            let a = temp.file("a", "payload");
            let child = temp.file("folder/child", "child");
            let folder = temp.path().join("folder");
            for p in [&a, &folder, &child] {
                seed(p);
            }
            let untouched = snapshot(&child);
            let plan = plan(&[&a, &folder]);
            assert!(ready(&plan));
            assert_eq!(changed_count(&plan, change()), 2);
            let report = apply(plan, change(), &|| false);
            assert_eq!(
                (report.changed, report.unchanged, report.errors.len()),
                (2, 0, 0)
            );
            for p in [&a, &folder] {
                assert_eq!(
                    snapshot(p),
                    Snapshot {
                        modified: change().modified.unwrap(),
                        accessed: change().accessed.unwrap()
                    }
                );
            }
            assert_eq!(snapshot(&child), untouched);
            assert_eq!(fs::read_to_string(&a).unwrap(), "payload");
        }
        #[test]
        fn edited_field_conflicts_are_rejected_while_unedited_changes_are_retained() {
            let temp = TempDir::new();
            let a = temp.file("a", "payload");
            seed(&a);
            let p = plan(&[&a]);
            let before = snapshot(&a);
            let external = time("2022-01-01");
            fs::File::open(&a)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(external))
                .unwrap();
            let c = Change {
                modified: None,
                accessed: change().accessed,
            };
            let report = apply(p, c, &|| false);
            assert_eq!((report.changed, report.errors.len()), (1, 0));
            assert_eq!(snapshot(&a).modified, external);
            // A row equal to the desired value in preview is still checked.
            seed(&a);
            let p = plan(&[&a]);
            fs::File::open(&a)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(external))
                .unwrap();
            let report = apply(
                p,
                Change {
                    modified: Some(before.modified),
                    accessed: None,
                },
                &|| false,
            );
            assert_eq!((report.changed, report.errors.len()), (0, 1));
            assert_eq!(snapshot(&a).modified, external);
            let p = plan(&[&a]);
            let old_access = snapshot(&a).accessed;
            fs::File::open(&a)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_accessed(external))
                .unwrap();
            let report = apply(
                p,
                Change {
                    modified: None,
                    accessed: Some(old_access),
                },
                &|| false,
            );
            assert_eq!((report.changed, report.errors.len()), (0, 1));
            assert_eq!(snapshot(&a).accessed, external);
        }
        #[test]
        fn timestamp_hardlink_aliases_are_idempotent_and_unchanged_fields_stay_current() {
            let temp = TempDir::new();
            let a = temp.file("a", "payload");
            let b = temp.path().join("b");
            fs::hard_link(&a, &b).unwrap();
            seed(&a);
            let p = plan(&[&a, &b]);
            let report = apply(p, change(), &|| false);
            assert_eq!(
                (report.changed, report.unchanged, report.errors.len()),
                (1, 1, 0)
            );
            assert_eq!(snapshot(&a), snapshot(&b));
        }
        #[test]
        fn timestamp_replacement_symlink_and_unavailable_preview_never_mutate_a_substitute() {
            let temp = TempDir::new();
            let a = temp.file("a", "original");
            let b = temp.file("b", "target");
            seed(&a);
            seed(&b);
            let before = snapshot(&b);
            let p = plan(&[&a]);
            fs::rename(&a, temp.path().join("old")).unwrap();
            std::os::unix::fs::symlink(&b, &a).unwrap();
            let report = apply(p, change(), &|| false);
            assert_eq!((report.changed, report.errors.len()), (0, 1));
            assert_eq!(snapshot(&b), before);
            let p = plan(&[&a, &b]);
            assert!(!ready(&p));
            let report = apply(p, change(), &|| false);
            assert_eq!(report.changed, 0);
            assert_eq!(snapshot(&b), before);
            fs::remove_file(&a).unwrap();
            let held = target(&b);
            let old_b = temp.path().join("held-b");
            fs::rename(&b, &old_b).unwrap();
            fs::write(&b, "replacement target").unwrap();
            seed(&b);
            let replacement = snapshot(&b);
            let stale_plan =
                permissions::inspect_with_binding(vec![held], Binding::Object, &|| false).unwrap();
            assert!(!ready(&stale_plan));
            let report = apply(stale_plan, change(), &|| false);
            assert_eq!(report.changed, 0);
            assert_eq!(snapshot(&b), replacement);
            fs::write(&a, "replacement").unwrap();
            seed(&a);
            let before = snapshot(&a);
            let stale = permissions::inspect(vec![target(&b)], &|| true);
            assert!(stale.is_err());
            assert_eq!(snapshot(&a), before);
        }
        #[test]
        fn timestamp_cancellation_retains_applied_items_and_stops_before_the_next() {
            let temp = TempDir::new();
            let a = temp.file("a", "a");
            let b = temp.file("b", "b");
            seed(&a);
            seed(&b);
            let before = snapshot(&b);
            let p = plan(&[&a, &b]);
            let report = apply(p, change(), &|| {
                snapshot(&a).modified == change().modified.unwrap()
            });
            assert!(report.cancelled);
            assert_eq!(report.changed, 1);
            assert_eq!(snapshot(&b), before);
            let p = plan(&[&b]);
            let report = apply(p, change(), &|| true);
            assert!(report.cancelled);
            assert_eq!(report.changed, 0);
            assert_eq!(snapshot(&b), before);
        }
        #[test]
        fn timestamp_reload_rebinds_the_same_object_after_its_version_changes() {
            let temp = TempDir::new();
            let a = temp.file("a", "payload");
            seed(&a);
            let selected = target(&a);
            let first = permissions::inspect(vec![selected.clone()], &|| false).unwrap();
            let report = apply(first, change(), &|| false);
            assert_eq!(report.changed, 1);
            assert!(!ready(
                &permissions::inspect(vec![selected.clone()], &|| false).unwrap()
            ));
            let refreshed =
                permissions::inspect_with_binding(vec![selected], Binding::Object, &|| false)
                    .unwrap();
            assert!(ready(&refreshed));
            assert_eq!(refreshed.rows()[0].times, Some(snapshot(&a)));
            assert_eq!(changed_count(&refreshed, change()), 0);
            let next = Change::parse("2024-01-01", "").unwrap();
            let report = apply(refreshed, next, &|| false);
            assert_eq!((report.changed, report.errors.len()), (1, 0));
            assert_eq!(snapshot(&a).modified, next.modified.unwrap());
        }

        #[test]
        fn timestamp_foreign_paths_are_bound_before_commit_and_empty_edits_do_nothing() {
            let temp = TempDir::new();
            let a = temp.file("a", "payload");
            seed(&a);
            let before = snapshot(&a);
            let p = permissions::inspect(
                vec![Target {
                    path: a.clone(),
                    identity: ListingIdentity::Unavailable,
                    capture_on_worker: true,
                }],
                &|| false,
            )
            .unwrap();
            assert!(ready(&p));
            assert!(!p.rows()[0].target.capture_on_worker);
            assert_eq!(changed_count(&p, Change::default()), 0);
            let report = apply(p, Change::default(), &|| false);
            assert_eq!(
                (report.changed, report.unchanged, report.errors.len()),
                (0, 1, 0)
            );
            assert_eq!(snapshot(&a), before);
        }
    }
}
