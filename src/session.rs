//! Persisted session: panel paths, layout and view toggles, so the app
//! reopens where it was left. UI-independent and serde-serialisable.

use crate::panel::{SortColumn, SortOrder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long startup waits for the restored panel paths to answer `is_dir`.
/// A stale hard-mounted NFS/SMB share can block that stat indefinitely, and it
/// runs before the first frame, so the window would never appear.
const RESTORE_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

const STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.session", 1, 8 * 1024 * 1024);

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct PersistedLeftView {
    #[serde(rename = "left_sort_col")]
    sort_column: SortColumn,
    #[serde(rename = "left_sort_order")]
    sort_order: SortOrder,
    #[serde(rename = "left_hidden")]
    show_hidden: bool,
    #[serde(rename = "left_folders_first", default = "default_true")]
    folders_first: bool,
    #[serde(rename = "left_natural_sort", default = "default_true")]
    natural_name_sort: bool,
    #[serde(rename = "left_density", default)]
    density: crate::density::Density,
}

impl PersistedLeftView {
    fn view_config(self) -> crate::panel::ViewConfig {
        persisted_view_config(
            self.sort_column,
            self.sort_order,
            self.show_hidden,
            self.folders_first,
            self.natural_name_sort,
            self.density,
        )
    }
}

impl From<crate::panel::ViewConfig> for PersistedLeftView {
    fn from(config: crate::panel::ViewConfig) -> Self {
        Self {
            sort_column: config.sort_column(),
            sort_order: config.sort_order(),
            show_hidden: config.show_hidden(),
            folders_first: config.folders_first(),
            natural_name_sort: config.natural_name_sort(),
            density: config.density(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct PersistedRightView {
    #[serde(rename = "right_sort_col")]
    sort_column: SortColumn,
    #[serde(rename = "right_sort_order")]
    sort_order: SortOrder,
    #[serde(rename = "right_hidden")]
    show_hidden: bool,
    #[serde(rename = "right_folders_first", default = "default_true")]
    folders_first: bool,
    #[serde(rename = "right_natural_sort", default = "default_true")]
    natural_name_sort: bool,
    #[serde(rename = "right_density", default)]
    density: crate::density::Density,
}

impl PersistedRightView {
    fn view_config(self) -> crate::panel::ViewConfig {
        persisted_view_config(
            self.sort_column,
            self.sort_order,
            self.show_hidden,
            self.folders_first,
            self.natural_name_sort,
            self.density,
        )
    }
}

impl From<crate::panel::ViewConfig> for PersistedRightView {
    fn from(config: crate::panel::ViewConfig) -> Self {
        Self {
            sort_column: config.sort_column(),
            sort_order: config.sort_order(),
            show_hidden: config.show_hidden(),
            folders_first: config.folders_first(),
            natural_name_sort: config.natural_name_sort(),
            density: config.density(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Session {
    pub left_path: PathBuf,
    pub right_path: PathBuf,
    pub active_left: bool,
    pub theme_dark: bool,
    /// Follow the OS light/dark appearance instead of `theme_dark`.
    #[serde(default)]
    pub theme_follow_system: bool,
    pub ui_scale: f32,
    pub show_tree: bool,
    pub tree_width: f32,
    pub show_size_bars: bool,
    pub show_compare: bool,
    /// Keep the F-key bar visible instead of revealing it on modifier hold.
    #[serde(default)]
    pub show_key_bar: bool,
    /// Flattened to the legacy `left_*`/`right_*` JSON keys so old sessions
    /// remain readable while Rust code handles each panel view as one value.
    #[serde(flatten)]
    pub(crate) left_view: PersistedLeftView,
    #[serde(flatten)]
    pub(crate) right_view: PersistedRightView,
    /// Command-palette usage history, for recency/frequency ranking.
    #[serde(default)]
    pub palette_usage: crate::command::UsageStats,
    /// Monotonic counter stamped onto each palette command run.
    #[serde(default)]
    pub palette_tick: u64,
    /// Persisted `Cmd+P` destinations and their frecency metadata.
    #[serde(default)]
    pub recent_paths: Vec<PathBuf>,
    #[serde(default)]
    pub recent_stats: crate::panel::VisitStats,
    #[serde(default)]
    pub recent_order: crate::panel::RecentOrder,
    #[serde(default)]
    pub search_history: crate::search::QueryHistory,
    #[serde(default)]
    pub durability_profile: crate::operation::DurabilityProfile,
    #[serde(default)]
    pub version_retention: crate::operation::VersionRetentionPolicy,
    #[serde(default)]
    pub sync_guard_policy: crate::sync_guard::GuardPolicy,
    #[serde(default)]
    pub name_policy: crate::filesystem_policy::NamePolicy,
    #[serde(default)]
    pub symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    /// Folders expanded in the folder tree, at most [`TREE_EXPANDED_CAP`].
    /// Missing folders are harmless: the tree only walks folders it lists.
    #[serde(default)]
    pub tree_expanded: Vec<PathBuf>,
    /// Window inner size in points, restored at launch; absent in sessions
    /// written before it was remembered.
    #[serde(default)]
    pub window_size: Option<[f32; 2]>,
}

/// Smallest window the app allows (matches the viewport's minimum size).
pub const MIN_WINDOW_SIZE: [f32; 2] = [900.0, 500.0];

/// Largest remembered window size accepted; anything bigger is a corrupt
/// value, not a real display.
const MAX_WINDOW_SIDE: f32 = 16_384.0;

/// A remembered window size worth restoring: finite and clamped to the
/// window's minimum and a sane maximum.
pub fn restorable_window_size(size: Option<[f32; 2]>) -> Option<[f32; 2]> {
    let [width, height] = size?;
    (width.is_finite() && height.is_finite()).then(|| {
        [
            width.clamp(MIN_WINDOW_SIZE[0], MAX_WINDOW_SIDE),
            height.clamp(MIN_WINDOW_SIZE[1], MAX_WINDOW_SIDE),
        ]
    })
}

/// Most expanded tree folders a session keeps.
pub const TREE_EXPANDED_CAP: usize = 256;

/// The expanded-folder set as persisted: shallowest folders first (they
/// matter most for restoring the visible tree), then by path, capped at
/// [`TREE_EXPANDED_CAP`]. The order is deterministic so an unchanged tree
/// writes an unchanged session.
pub fn persisted_tree_expanded<'a>(
    expanded: impl IntoIterator<Item = &'a PathBuf>,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = expanded.into_iter().cloned().collect();
    paths.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    paths.truncate(TREE_EXPANDED_CAP);
    paths
}

/// Serde default for booleans that should restore as `true` (the live
/// default) when a key is absent from an older session file.
const fn default_true() -> bool {
    true
}

impl Session {
    /// Panel paths that still exist as directories. A deleted folder restores
    /// to its nearest surviving ancestor below the filesystem root, else to
    /// `home`. A path that does not answer within [`RESTORE_PROBE_TIMEOUT`] (a
    /// hung mount) also falls back to `home`, so startup never blocks on it.
    pub fn sanitized_paths(&self, home: &Path) -> (PathBuf, PathBuf) {
        self.sanitized_paths_with(home, RESTORE_PROBE_TIMEOUT, |p| p.is_dir())
    }

    /// [`Self::sanitized_paths`] with an injected probe and time budget. Both
    /// probes run concurrently on helper threads against one shared deadline.
    /// A probe still running at the deadline counts as "not a directory"; its
    /// thread is detached, since a stat stuck in the kernel cannot be
    /// cancelled, and it exits on its own if the mount ever answers.
    fn sanitized_paths_with(
        &self,
        home: &Path,
        timeout: Duration,
        probe: impl Fn(&Path) -> bool + Send + Clone + 'static,
    ) -> (PathBuf, PathBuf) {
        let spawn = |path: &Path| {
            let (sender, receiver) = std::sync::mpsc::channel();
            let path = path.to_path_buf();
            let probe = probe.clone();
            std::thread::spawn(move || {
                let _ = sender.send(nearest_surviving_dir(&path, probe));
            });
            receiver
        };
        let left = spawn(&self.left_path);
        let right = spawn(&self.right_path);
        let deadline = Instant::now() + timeout;
        let pick = |receiver: std::sync::mpsc::Receiver<Option<PathBuf>>| {
            let budget = deadline.saturating_duration_since(Instant::now());
            receiver
                .recv_timeout(budget)
                .ok()
                .flatten()
                .unwrap_or_else(|| home.to_path_buf())
        };
        (pick(left), pick(right))
    }

    pub(crate) fn view_configs(&self) -> [crate::panel::ViewConfig; 2] {
        [self.left_view.view_config(), self.right_view.view_config()]
    }
}

/// `path` itself or its nearest ancestor that `is_dir` accepts. A saved path
/// that still exists is always kept, even a drive or filesystem root. Only as
/// an *ancestor* does the bare root not count: a deleted folder restored to
/// `/` or `C:\` is rarely what the user wants, so the caller falls back to
/// home instead.
fn nearest_surviving_dir(path: &Path, is_dir: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    if is_dir(path) {
        return Some(path.to_path_buf());
    }
    path.ancestors()
        .skip(1)
        .filter(|ancestor| ancestor.parent().is_some())
        .find(|ancestor| is_dir(ancestor))
        .map(Path::to_path_buf)
}

fn persisted_view_config(
    sort_column: SortColumn,
    sort_order: SortOrder,
    show_hidden: bool,
    folders_first: bool,
    natural_name_sort: bool,
    density: crate::density::Density,
) -> crate::panel::ViewConfig {
    crate::panel::ViewConfig::default()
        .with_sort(sort_column, sort_order)
        .with_show_hidden(show_hidden)
        .with_folders_first(folders_first)
        .with_natural_name_sort(natural_name_sort)
        .with_density(density)
}

fn session_path() -> PathBuf {
    crate::fs_util::config_dir().join("session.json")
}

pub struct LoadedSession {
    pub value: Option<Session>,
    pub gate: crate::persistence::StoreGate,
}

pub fn load_with(persist: &dyn crate::persistence::Persist) -> LoadedSession {
    load_at(persist, &session_path())
}

fn load_at(persist: &dyn crate::persistence::Persist, path: &Path) -> LoadedSession {
    let loaded = crate::persistence::load_enveloped::<Session>(persist, path, STORE);
    if loaded.value.is_none() && loaded.gate.status().fails_closed() {
        crate::persistence::record_unreadable("Session");
    }
    LoadedSession {
        value: loaded.value,
        gate: loaded.gate,
    }
}

pub fn save_with(
    persist: &dyn crate::persistence::Persist,
    session: &Session,
    gate: &mut crate::persistence::StoreGate,
) -> Result<crate::persistence::AtomicWriteOutcome, crate::persistence::JsonSaveError> {
    save_at(persist, &session_path(), session, gate)
}

fn save_at(
    persist: &dyn crate::persistence::Persist,
    path: &Path,
    session: &Session,
    gate: &mut crate::persistence::StoreGate,
) -> Result<crate::persistence::AtomicWriteOutcome, crate::persistence::JsonSaveError> {
    crate::persistence::save_enveloped(
        persist,
        path,
        STORE,
        session,
        gate,
        crate::persistence::SaveIntent::Automatic,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn sample(left: PathBuf, right: PathBuf) -> Session {
        Session {
            left_path: left,
            right_path: right,
            active_left: true,
            theme_dark: true,
            theme_follow_system: true,
            ui_scale: 1.1,
            show_tree: true,
            tree_width: 220.0,
            show_size_bars: false,
            show_compare: true,
            show_key_bar: false,
            left_view: PersistedLeftView::from(
                crate::panel::ViewConfig::default()
                    .with_sort(SortColumn::Size, SortOrder::Desc)
                    .with_show_hidden(true)
                    .with_natural_name_sort(false)
                    .with_density(crate::density::Density::Compact),
            ),
            right_view: PersistedRightView::from(
                crate::panel::ViewConfig::default()
                    .with_folders_first(false)
                    .with_density(crate::density::Density::Spacious),
            ),
            palette_usage: crate::command::UsageStats::default(),
            palette_tick: 7,
            recent_paths: Vec::new(),
            recent_stats: crate::panel::VisitStats::default(),
            recent_order: crate::panel::RecentOrder::Frecency,
            search_history: crate::search::QueryHistory::default(),
            durability_profile: crate::operation::DurabilityProfile::default(),
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            sync_guard_policy: crate::sync_guard::GuardPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            tree_expanded: vec![PathBuf::from("/a"), PathBuf::from("/a/b")],
            window_size: Some([1400.0, 900.0]),
        }
    }

    #[test]
    fn window_size_round_trips_and_defaults_when_absent() {
        let s = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let mut val = serde_json::to_value(&s).unwrap();
        let back: Session = serde_json::from_value(val.clone()).unwrap();
        assert_eq!(back.window_size, Some([1400.0, 900.0]));
        val.as_object_mut().unwrap().remove("window_size");
        let older: Session = serde_json::from_value(val).unwrap();
        assert_eq!(older.window_size, None);
    }

    #[test]
    fn restorable_window_size_clamps_and_rejects_garbage() {
        assert_eq!(restorable_window_size(None), None);
        assert_eq!(
            restorable_window_size(Some([1400.0, 900.0])),
            Some([1400.0, 900.0])
        );
        assert_eq!(
            restorable_window_size(Some([10.0, 1e9])),
            Some([MIN_WINDOW_SIZE[0], MAX_WINDOW_SIDE])
        );
        assert_eq!(restorable_window_size(Some([f32::NAN, 900.0])), None);
        assert_eq!(restorable_window_size(Some([1400.0, f32::INFINITY])), None);
    }

    #[test]
    fn density_defaults_when_absent_from_json() {
        // A session written before per-panel density existed has neither key.
        let s = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let mut val = serde_json::to_value(&s).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.remove("left_density");
        obj.remove("right_density");
        let back: Session = serde_json::from_value(val).unwrap();
        let [left, right] = back.view_configs();
        assert_eq!(left.density(), crate::density::Density::Comfortable);
        assert_eq!(right.density(), crate::density::Density::Comfortable);
    }

    #[test]
    fn sort_toggles_default_true_when_absent() {
        // A session written before the sort toggles existed has none of the keys.
        let s = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let mut val = serde_json::to_value(&s).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.remove("left_folders_first");
        obj.remove("left_natural_sort");
        obj.remove("right_folders_first");
        obj.remove("right_natural_sort");
        let back: Session = serde_json::from_value(val).unwrap();
        let [left, right] = back.view_configs();
        assert!(left.folders_first());
        assert!(left.natural_name_sort());
        assert!(right.folders_first());
        assert!(right.natural_name_sort());
    }

    #[test]
    fn tree_expansion_persists_shallow_folders_first_and_capped() {
        let mut expanded: std::collections::HashSet<PathBuf> = (0..TREE_EXPANDED_CAP + 10)
            .map(|index| PathBuf::from(format!("/deep/a/f{index}")))
            .collect();
        expanded.insert(PathBuf::from("/deep"));
        expanded.insert(PathBuf::from("/deep/a"));
        let persisted = persisted_tree_expanded(&expanded);
        assert_eq!(persisted.len(), TREE_EXPANDED_CAP);
        assert_eq!(persisted[0], PathBuf::from("/deep"));
        assert_eq!(persisted[1], PathBuf::from("/deep/a"));
        assert_eq!(persisted, persisted_tree_expanded(&expanded));
    }

    #[test]
    fn recent_history_defaults_when_absent() {
        let s = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let mut val = serde_json::to_value(&s).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.remove("recent_paths");
        obj.remove("recent_stats");
        obj.remove("recent_order");
        obj.remove("search_history");
        obj.remove("version_retention");
        obj.remove("name_policy");
        obj.remove("symlink_policy");
        obj.remove("tree_expanded");
        let back: Session = serde_json::from_value(val).unwrap();
        assert!(back.tree_expanded.is_empty());
        assert!(back.recent_paths.is_empty());
        assert_eq!(back.recent_stats, crate::panel::VisitStats::default());
        assert_eq!(back.recent_order, crate::panel::RecentOrder::Frecency);
        assert_eq!(back.search_history, crate::search::QueryHistory::default());
        assert_eq!(
            back.version_retention,
            crate::operation::VersionRetentionPolicy::Recent
        );
        assert_eq!(
            back.name_policy,
            crate::filesystem_policy::NamePolicy::default()
        );
        assert_eq!(
            back.symlink_policy,
            crate::filesystem_policy::SymlinkPolicy::Preserve
        );
    }

    #[test]
    fn round_trips_through_json() {
        let s = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn persisted_panel_views_restore_as_complete_configs() {
        let session = sample(PathBuf::from("/a"), PathBuf::from("/b"));

        let [left, right] = session.view_configs();
        assert_eq!(left.sort_column(), SortColumn::Size);
        assert_eq!(left.sort_order(), SortOrder::Desc);
        assert!(left.show_hidden());
        assert!(left.folders_first());
        assert!(!left.natural_name_sort());
        assert_eq!(left.density(), crate::density::Density::Compact);

        assert_eq!(right.sort_column(), SortColumn::Name);
        assert_eq!(right.sort_order(), SortOrder::Asc);
        assert!(!right.show_hidden());
        assert!(!right.folders_first());
        assert!(right.natural_name_sort());
        assert_eq!(right.density(), crate::density::Density::Spacious);
    }

    #[test]
    fn panel_views_keep_the_legacy_flat_json_schema() {
        let session = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let value = serde_json::to_value(session).unwrap();
        let object = value.as_object().unwrap();

        for key in [
            "left_sort_col",
            "left_sort_order",
            "left_hidden",
            "left_folders_first",
            "left_natural_sort",
            "left_density",
            "right_sort_col",
            "right_sort_order",
            "right_hidden",
            "right_folders_first",
            "right_natural_sort",
            "right_density",
        ] {
            assert!(object.contains_key(key), "missing legacy key {key}");
        }
        assert!(!object.contains_key("left_view"));
        assert!(!object.contains_key("right_view"));
    }

    #[test]
    fn a_session_saved_before_system_theme_keeps_its_fixed_theme() {
        let session = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let mut value = serde_json::to_value(session).unwrap();
        assert_eq!(value["theme_follow_system"], true);
        value.as_object_mut().unwrap().remove("theme_follow_system");
        let old: Session = serde_json::from_value(value).unwrap();
        assert!(!old.theme_follow_system);
        assert!(old.theme_dark);
    }

    #[test]
    fn sanitized_paths_fall_back_to_home_when_missing() {
        let home = TempDir::new();
        let real = home.dir("Documents");
        let s = sample(real.clone(), PathBuf::from("/no/such/dir/xyz"));
        let (l, r) = s.sanitized_paths(home.path());
        assert_eq!(l, real); // exists -> kept
        assert_eq!(r, home.path()); // missing -> home
    }

    #[test]
    fn sanitized_paths_restore_the_nearest_surviving_ancestor() {
        let home = TempDir::new();
        let projects = home.dir("Projects");
        let s = sample(projects.join("deleted/deeper"), projects.clone());
        let (l, r) = s.sanitized_paths(home.path());
        assert_eq!(l, projects);
        assert_eq!(r, projects);
    }

    #[test]
    fn nearest_surviving_dir_keeps_a_saved_root_but_skips_root_ancestors() {
        let root = std::env::current_dir()
            .unwrap()
            .ancestors()
            .last()
            .unwrap()
            .to_path_buf();
        assert_eq!(nearest_surviving_dir(&root, |_| true), Some(root.clone()));
        let gone = root.join("no-such-dir-xyz");
        let only_root = {
            let root = root.clone();
            move |p: &Path| p == root
        };
        assert_eq!(nearest_surviving_dir(&gone, only_root), None);
    }

    #[test]
    fn sanitized_paths_fall_back_to_home_when_a_probe_hangs() {
        use std::sync::{Arc, Mutex};
        let home = TempDir::new();
        let real = home.dir("Documents");
        let hung = PathBuf::from("/mnt/stale-share");
        let s = sample(hung.clone(), real.clone());
        // The probe for `hung` blocks until the test releases it, like a stat
        // on a dead hard mount.
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let probe = {
            let hung = hung.clone();
            move |p: &Path| {
                if p == hung {
                    let _ = gate.lock().map(|g| g.recv());
                    return true;
                }
                p.is_dir()
            }
        };
        let started = Instant::now();
        let (l, r) = s.sanitized_paths_with(home.path(), Duration::from_millis(100), probe);
        assert!(started.elapsed() < Duration::from_secs(5), "did not block");
        assert_eq!(l, home.path()); // unanswered -> home
        assert_eq!(r, real); // answered in time -> kept
        drop(release);
    }

    #[test]
    fn strict_session_rejects_partial_payload_and_blocks_automatic_save() {
        let temp = TempDir::new();
        let path = temp.path().join("session.json");
        let session = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let mut value = serde_json::to_value(&session).unwrap();
        value["ui_scale"] = serde_json::Value::String("large".to_string());
        let original = serde_json::to_vec_pretty(&value).unwrap();
        std::fs::write(&path, &original).unwrap();
        let persist = crate::persistence::FsPersist::default();

        let mut loaded = load_at(&persist, &path);

        assert!(loaded.value.is_none());
        assert_eq!(
            loaded.gate.status(),
            crate::persistence::LoadStatus::Corrupt
        );
        let result = save_at(&persist, &path, &session, &mut loaded.gate);
        assert!(matches!(
            result,
            Err(crate::persistence::JsonSaveError::Blocked(
                crate::persistence::LoadStatus::Corrupt
            ))
        ));
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}
