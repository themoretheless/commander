//! Ancestor columns have independent cancellable listing owners. The panel
//! itself remains the sole owner of actionable rows, selection and commands.
use super::{BrowserMode, PanelState, ViewConfig, listing_job::PendingFocus};
use std::path::{Path, PathBuf};
const MAX_ANCESTORS: usize = 8;
#[derive(Default)]
pub struct MillerState {
    pub columns: Vec<PanelState>,
    pub reveal_current: bool,
    path: Option<PathBuf>,
    config: Option<ViewConfig>,
}
const WIDTH_MEMORY_CAPACITY: usize = 64;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Widths(std::collections::VecDeque<(PathBuf, f32)>);
impl Widths {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn get(&self, path: &Path, automatic: f32) -> f32 {
        self.0
            .iter()
            .find(|(p, _)| p == path)
            .map_or(automatic, |(_, w)| *w)
    }
    fn set(&mut self, path: PathBuf, width: f32) -> bool {
        if !width.is_finite() {
            return false;
        }
        let width = width.clamp(180.0, 720.0).round();
        if self.0.iter().any(|(p, w)| *p == path && *w == width) {
            return false;
        }
        self.0.retain(|(p, _)| *p != path);
        self.0.push_back((path, width));
        while self.0.len() > WIDTH_MEMORY_CAPACITY {
            self.0.pop_front();
        }
        true
    }
    fn reset(&mut self, path: &Path) -> bool {
        let old = self.0.len();
        self.0.retain(|(p, _)| p != path);
        self.0.len() != old
    }
}
// Optional geometry never makes an otherwise valid tab unreadable. Native
// paths use platform bytes/units, without lossy UTF-8 conversion or filesystem IO.
impl serde::Serialize for Widths {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = Vec::new();
        let mut budget = 0;
        for (path, width) in self.0.iter().rev() {
            #[cfg(unix)]
            let native = {
                use std::os::unix::ffi::OsStrExt;
                path.as_os_str().as_bytes().to_vec()
            };
            #[cfg(windows)]
            let native = {
                use std::os::windows::ffi::OsStrExt;
                path.as_os_str().encode_wide().collect::<Vec<_>>()
            };
            let row = serde_json::json!({"path": native, "width": width, "platform": std::env::consts::FAMILY});
            let size = serde_json::to_vec(&row)
                .map_err(serde::ser::Error::custom)?
                .len();
            if budget + size > 65536 {
                continue;
            }
            budget += size;
            rows.push(row);
        }
        rows.reverse();
        rows.serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for Widths {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
        let mut result = Self::default();
        let Some(rows) = value.as_array() else {
            return Ok(result);
        };
        let mut accepted = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut budget = 0;
        for row in rows.iter().rev().take(256) {
            if accepted.len() == WIDTH_MEMORY_CAPACITY {
                break;
            }
            let size = serde_json::to_vec(row)
                .map_err(serde::de::Error::custom)?
                .len();
            if budget + size > 65536 {
                continue;
            }
            if row["platform"].as_str() != Some(std::env::consts::FAMILY) {
                continue;
            }
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStringExt;
                let Ok(bytes) = serde_json::from_value::<Vec<u8>>(row["path"].clone()) else {
                    continue;
                };
                if bytes.contains(&0) {
                    continue;
                }
                PathBuf::from(std::ffi::OsString::from_vec(bytes))
            };
            #[cfg(windows)]
            let path = {
                use std::os::windows::ffi::OsStringExt;
                let Ok(units) = serde_json::from_value::<Vec<u16>>(row["path"].clone()) else {
                    continue;
                };
                if units.contains(&0) {
                    continue;
                }
                PathBuf::from(std::ffi::OsString::from_wide(&units))
            };
            let Some(width) = row["width"]
                .as_f64()
                .map(|v| v as f32)
                .filter(|v| v.is_finite())
            else {
                continue;
            };
            if !path.is_absolute() || !seen.insert(path.clone()) {
                continue;
            }
            budget += size;
            accepted.push((path, width));
        }
        for (path, width) in accepted.into_iter().rev() {
            result.set(path, width);
        }
        Ok(result)
    }
}
fn ancestor_paths(path: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = path
        .ancestors()
        .skip(1)
        .filter(|p| !p.as_os_str().is_empty())
        .take(MAX_ANCESTORS)
        .map(Path::to_path_buf)
        .collect();
    paths.reverse();
    paths
}
impl PanelState {
    pub(crate) fn go_up_preserving_miller(&mut self) {
        if self.browser_mode() != BrowserMode::Miller {
            self.go_up();
            return;
        }
        let child = self.current_path.clone();
        if let Some(parent) = child.parent() {
            self.miller_navigate(parent.to_path_buf(), Some(child));
        }
    }

    pub(crate) fn miller_width_snapshot(&self) -> Widths {
        self.miller_widths.clone()
    }
    pub(crate) fn restore_miller_widths(&mut self, widths: Widths) {
        self.miller_widths = widths;
        self.miller.reveal_current = true;
    }

    pub(crate) fn miller_column_width(&self, path: &Path, automatic: f32) -> f32 {
        self.miller_widths.get(path, automatic)
    }
    pub(crate) fn resize_miller_column(&mut self, path: PathBuf, width: f32) -> bool {
        self.miller_widths.set(path, width)
    }
    pub(crate) fn reset_miller_column(&mut self, path: &Path) -> bool {
        self.miller_widths.reset(path)
    }
    /// UI callers must have installed workload + wake-up first. Never falls
    /// back to a synchronous directory read during column rendering.
    pub(crate) fn prepare_miller(&mut self) -> bool {
        if self.browser_mode() != BrowserMode::Miller {
            self.miller = Default::default();
            return false;
        }
        let (Some(workload), Some(notify)) = (self.workload.clone(), self.notify_callback()) else {
            return false;
        };
        let config = self
            .view_config()
            .with_mode(BrowserMode::List)
            .with_sort(super::SortColumn::Name, super::SortOrder::Asc);
        let changed = self.miller.path.as_ref() != Some(&self.current_path);
        if changed || self.miller.config != Some(config) {
            let mut old = std::mem::take(&mut self.miller.columns);
            self.miller.columns = ancestor_paths(&self.current_path)
                .into_iter()
                .map(|path| {
                    if self.miller.config == Some(config)
                        && let Some(i) = old.iter().position(|c| c.current_path == path)
                    {
                        return old.remove(i);
                    }
                    let mut col = Self::new_with_view(path, config);
                    col.miller_context_only = true;
                    col.set_workload(workload.clone());
                    col.set_notify(notify.clone());
                    col.refresh();
                    col
                })
                .collect();
            // Dropped columns cancel their independent listing jobs and watchers.
            self.miller.path = Some(self.current_path.clone());
            self.miller.config = Some(config);
        }
        for col in &mut self.miller.columns {
            col.poll_listing();
            col.poll_fs_changes();
        }
        changed
    }
    /// Promote an ancestor-row location into the action owner before a file
    /// can be opened/operated on. Native path identity survives non-UTF8 names.
    pub(crate) fn miller_navigate(&mut self, folder: PathBuf, focus: Option<PathBuf>) {
        self.navigate_to(folder);
        self.set_browser_mode(BrowserMode::Miller);
        if let Some(path) = focus {
            if self.can_async_list() && self.listing_job.is_awaiting() {
                self.listing_job.set_focus(PendingFocus::Remembered {
                    cursor_path: Some(path),
                    scroll_anchor: 0,
                });
            } else if let Some(i) = self.filtered_position(|e| e.path == path) {
                self.set_cursor(i + 1);
                self.set_scroll_to_cursor(true);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn widths_are_finite_bounded_native_keys_with_recent_change_memory() {
        let mut widths = Widths::default();
        let path = PathBuf::from("/x");
        assert_eq!(widths.get(&path, 220.0), 220.0);
        assert!(widths.set(path.clone(), f32::MIN));
        assert_eq!(widths.get(&path, 0.0), 180.0);
        assert!(widths.set(path.clone(), 10000.0));
        assert_eq!(widths.get(&path, 0.0), 720.0);
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(!widths.set(path.clone(), value));
            assert_eq!(widths.get(&path, 0.0), 720.0);
        }
        assert!(!widths.set(path.clone(), 720.0));
        assert!(widths.reset(&path));
        assert_eq!(widths.get(&path, 360.0), 360.0);
        for i in 0..65 {
            widths.set(PathBuf::from(format!("/{i}")), 200.0 + i as f32);
        }
        assert_eq!(widths.0.len(), 64);
        assert_eq!(widths.get(Path::new("/0"), 220.0), 220.0);
        widths.set("/1".into(), 333.0);
        widths.set("/65".into(), 400.0);
        assert_eq!(widths.get(Path::new("/1"), 0.0), 333.0);
        assert_eq!(widths.get(Path::new("/2"), 220.0), 220.0);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let a = PathBuf::from(std::ffi::OsString::from_vec(b"/a/\xfe".to_vec()));
            let b = PathBuf::from(std::ffi::OsString::from_vec(b"/a/\xff".to_vec()));
            widths.set(a.clone(), 301.0);
            widths.set(b.clone(), 302.0);
            assert_eq!(widths.get(&a, 0.0), 301.0);
            assert_eq!(widths.get(&b, 0.0), 302.0);
        }
    }
    #[test]
    fn resizing_retains_listing_selection_and_live_tab_geometry_without_io() {
        let root = crate::testutil::TempDir::new();
        let a = root.dir("A");
        root.file("A/a.txt", "a");
        let mut p = PanelState::new(a.clone());
        p.refresh();
        p.focus_child("a.txt".into());
        p.select_cursor();
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        p.set_workload(workload.handle());
        p.set_notify(std::sync::Arc::new(|| {}));
        p.set_browser_mode(BrowserMode::Miller);
        p.prepare_miller();
        while workload.run_next() {}
        p.prepare_miller();
        let revisions: Vec<_> = p
            .miller
            .columns
            .iter()
            .map(PanelState::entries_gen)
            .collect();
        let revision = p.entries_gen();
        let cancelled = workload.stats().cancelled;
        let queued = workload.stats().queued;
        p.resize_miller_column(root.path().to_path_buf(), 280.0);
        p.resize_miller_column(a.clone(), 300.0);
        assert_eq!(
            p.miller
                .columns
                .iter()
                .map(PanelState::entries_gen)
                .collect::<Vec<_>>(),
            revisions
        );
        assert_eq!(p.entries_gen(), revision);
        assert_eq!(workload.stats().queued, queued);
        assert_eq!(workload.stats().cancelled, cancelled);
        assert!(p.is_selected(&a.join("a.txt")));
        assert_eq!(p.cursor_entry().unwrap().path, a.join("a.txt"));
        p.set_browser_mode(BrowserMode::List);
        p.suspend_for_tab();
        assert_eq!(p.miller_column_width(&a, 360.0), 300.0);
        assert!(p.reset_miller_column(&a));
        assert_eq!(p.miller_column_width(&a, 360.0), 360.0);
    }
    #[test]
    fn paths_are_lexical_rooted_bounded_and_do_not_read() {
        assert!(ancestor_paths(Path::new("/")).is_empty());
        assert_eq!(
            ancestor_paths(Path::new("/a/b")),
            [PathBuf::from("/"), PathBuf::from("/a")]
        );
        let path = PathBuf::from("/0/1/2/3/4/5/6/7/8/9");
        let paths = ancestor_paths(&path);
        assert_eq!(paths.len(), 8);
        assert_eq!(paths.last().unwrap(), &PathBuf::from("/0/1/2/3/4/5/6/7/8"));
    }
    #[test]
    fn independent_async_columns_reuse_common_ancestors_and_cancel_on_suspend() {
        let tmp = crate::testutil::TempDir::new();
        let a = tmp.path().join("A");
        let b = tmp.path().join("B");
        std::fs::create_dir_all(a.join("inner")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("file.txt"), "owner A").unwrap();
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        let mut panel = PanelState::new(a.join("inner"));
        panel.set_workload(workload.handle());
        panel.set_notify(std::sync::Arc::new(|| {}));
        panel.set_browser_mode(BrowserMode::Miller);
        assert!(panel.prepare_miller());
        assert!(
            panel
                .miller
                .columns
                .iter()
                .all(|c| c.miller_context_only && c.entries().is_empty())
        );
        while workload.run_next() {}
        panel.prepare_miller();
        let col = panel
            .miller
            .columns
            .iter()
            .find(|c| c.current_path == a)
            .unwrap();
        assert!(col.entries().iter().any(|e| e.name == "file.txt"));
        let parent_revision = panel
            .miller
            .columns
            .iter()
            .find(|c| c.current_path == tmp.path())
            .unwrap()
            .entries_gen();
        panel.miller_navigate(b, None);
        assert!(panel.prepare_miller());
        assert!(!panel.miller.columns.iter().any(|c| c.current_path == a));
        assert_eq!(
            panel
                .miller
                .columns
                .iter()
                .find(|c| c.current_path == tmp.path())
                .unwrap()
                .entries_gen(),
            parent_revision
        );
        panel.miller_navigate(a.clone(), Some(a.join("file.txt")));
        while workload.run_next() {}
        panel.poll_listing();
        assert_eq!(panel.cursor_entry().unwrap().path, a.join("file.txt"));
        panel.prepare_miller();
        panel.refresh();
        let before = workload.stats().cancelled;
        panel.suspend_for_tab();
        assert!(panel.miller.columns.is_empty());
        assert!(workload.stats().cancelled > before);
        assert!(panel.entries().iter().all(|e| e.path.starts_with(&a)));
    }
    #[test]
    fn restoring_a_remembered_list_folder_cancels_context_before_publication() {
        let tmp = crate::testutil::TempDir::new();
        let a = tmp.dir("A");
        let b = tmp.dir("B");
        tmp.file("A/a.txt", "a");
        tmp.file("B/b.txt", "b");
        let mut p = PanelState::new(b.clone());
        p.refresh();
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        p.set_workload(workload.handle());
        p.set_notify(std::sync::Arc::new(|| {}));
        p.navigate_to(a);
        p.set_browser_mode(BrowserMode::Miller);
        p.prepare_miller();
        assert!(!p.miller.columns.is_empty());
        let cancelled = workload.stats().cancelled;
        p.navigate_to(b.clone());
        assert_eq!(p.browser_mode(), BrowserMode::List);
        assert!(p.miller.columns.is_empty());
        assert!(workload.stats().cancelled > cancelled);
        while workload.run_next() {}
        p.poll_listing();
        assert_eq!(p.current_path, b);
        assert!(p.entries().iter().any(|e| e.name == "b.txt"));
        assert!(!p.entries().iter().any(|e| e.name == "a.txt"));
    }
    #[test]
    fn two_columns_for_same_path_do_not_supersede_one_another() {
        let tmp = crate::testutil::TempDir::new();
        std::fs::create_dir(tmp.path().join("child")).unwrap();
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        let mut panels: Vec<_> = (0..2)
            .map(|_| {
                let mut p = PanelState::new(tmp.path().join("child"));
                p.set_workload(workload.handle());
                p.set_notify(std::sync::Arc::new(|| {}));
                p.set_browser_mode(BrowserMode::Miller);
                p.prepare_miller();
                p
            })
            .collect();
        // Poll again after draining so columns denied by bounded admission
        // get their turn; a full queue is not a failed directory read.
        for _ in 0..32 {
            for p in &mut panels {
                p.prepare_miller();
            }
            while workload.run_next() {}
        }
        for p in &mut panels {
            p.prepare_miller();
            assert!(
                p.miller
                    .columns
                    .iter()
                    .find(|c| c.current_path == tmp.path())
                    .unwrap()
                    .entries()
                    .iter()
                    .any(|e| e.name == "child")
            );
        }
        assert_eq!(workload.stats().cancelled, 0);
        panels[0].set_browser_mode(BrowserMode::Icons);
        assert!(panels[0].miller.columns.is_empty());
        assert!(!panels[1].miller.columns.is_empty());
    }
    #[test]
    fn no_backend_means_no_sync_listing_and_mode_round_trips() {
        let mut panel = PanelState::new("/not/a/real/path".into());
        panel.set_browser_mode(BrowserMode::Miller);
        assert!(!panel.prepare_miller());
        assert!(panel.miller.columns.is_empty());
        let serialized = serde_json::to_string(&panel.view_config()).unwrap();
        assert_eq!(
            serde_json::from_str::<ViewConfig>(&serialized)
                .unwrap()
                .mode(),
            BrowserMode::Miller
        );
        panel.set_browser_mode(BrowserMode::List);
        assert!(panel.miller.columns.is_empty());
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    #[test]
    fn geometry_roundtrip_is_bounded_tolerant_and_last_wins() {
        let mut widths = Widths::default();
        for n in 0..100 {
            widths.set(PathBuf::from(format!("/column-{n}")), 200.0 + n as f32);
        }
        let encoded = serde_json::to_value(&widths).unwrap();
        let restored: Widths = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(restored, widths);
        assert_eq!(restored.0.len(), 64);
        let mut rows = encoded.as_array().unwrap().clone();
        rows.push(serde_json::json!({"path": false, "width": 300}));
        let mut last = rows[0].clone();
        last["width"] = serde_json::json!(9999);
        rows.push(last);
        let restored: Widths = serde_json::from_value(serde_json::Value::Array(rows)).unwrap();
        assert_eq!(restored.get(Path::new("/column-36"), 0.0), 720.0);
        assert!(
            serde_json::from_value::<Widths>(serde_json::json!("broken"))
                .unwrap()
                .is_empty()
        );
    }
    #[cfg(unix)]
    #[test]
    fn geometry_preserves_non_utf8_native_keys() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/column-\xff".to_vec()));
        let mut widths = Widths::default();
        widths.set(path.clone(), 321.0);
        let restored: Widths =
            serde_json::from_slice(&serde_json::to_vec(&widths).unwrap()).unwrap();
        assert_eq!(restored.get(&path, 0.0), 321.0);
        assert_eq!(restored.get(Path::new("/column-�"), 220.0), 220.0);
    }
}

#[cfg(all(test, unix))]
mod keyboard_native_path_tests {
    use super::*;
    #[test]
    fn miller_parent_focus_keeps_native_child_identity() {
        use std::os::unix::ffi::OsStringExt;
        let root = crate::testutil::TempDir::new();
        let native = root
            .path()
            .join(std::ffi::OsString::from_vec(b"child-\xff".to_vec()));
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        let mut panel = PanelState::new(native.clone());
        panel.set_workload(workload.handle());
        panel.set_notify(std::sync::Arc::new(|| {}));
        panel.set_browser_mode(BrowserMode::Miller);
        panel.go_up_preserving_miller();
        assert_eq!(panel.current_path, root.path());
        assert!(
            matches!(panel.listing_job.pending_focus(), PendingFocus::Remembered { cursor_path: Some(path), .. } if path == &native)
        );
        assert_eq!(panel.browser_mode(), BrowserMode::Miller);
    }
}
