//! Persisted session: panel paths, layout and view toggles, so the app
//! reopens where it was left. UI-independent and serde-serialisable.

use crate::panel::{SortColumn, SortOrder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub(crate) struct PersistedLeftView {
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
pub(crate) struct PersistedRightView {
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
    pub ui_scale: f32,
    pub show_tree: bool,
    pub tree_width: f32,
    pub show_size_bars: bool,
    pub show_compare: bool,
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
}

/// Serde default for booleans that should restore as `true` (the live
/// default) when a key is absent from an older session file.
fn default_true() -> bool {
    true
}

impl Session {
    /// Panel paths that still exist as directories, falling back to `home`.
    pub fn sanitized_paths(&self, home: &Path) -> (PathBuf, PathBuf) {
        let pick = |p: &Path| {
            if p.is_dir() {
                p.to_path_buf()
            } else {
                home.to_path_buf()
            }
        };
        (pick(&self.left_path), pick(&self.right_path))
    }

    pub(crate) fn view_configs(&self) -> [crate::panel::ViewConfig; 2] {
        [self.left_view.view_config(), self.right_view.view_config()]
    }
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

/// Load the saved session, or None if absent/corrupt.
pub fn load() -> Option<Session> {
    crate::persistence::load_json(&session_path(), "Session")
}

/// Save through a unique private sibling and one atomic replace. There is no
/// cross-process lock or CAS: concurrent session writers are last-writer-wins.
pub fn save(
    session: &Session,
) -> Result<crate::persistence::AtomicWriteOutcome, crate::persistence::PreCommitError> {
    crate::persistence::save_json_atomic(&session_path(), session)
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
            ui_scale: 1.1,
            show_tree: true,
            tree_width: 220.0,
            show_size_bars: false,
            show_compare: true,
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
        }
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
        let back: Session = serde_json::from_value(val).unwrap();
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
    fn sanitized_paths_fall_back_to_home_when_missing() {
        let home = TempDir::new();
        let real = home.dir("Documents");
        let s = sample(real.clone(), PathBuf::from("/no/such/dir/xyz"));
        let (l, r) = s.sanitized_paths(home.path());
        assert_eq!(l, real); // exists -> kept
        assert_eq!(r, home.path()); // missing -> home
    }
}
