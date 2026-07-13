//! Persisted session: panel paths, layout and view toggles, so the app
//! reopens where it was left. UI-independent and serde-serialisable.

use crate::panel::{SortColumn, SortOrder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    pub left_sort_col: SortColumn,
    pub left_sort_order: SortOrder,
    pub left_hidden: bool,
    pub right_sort_col: SortColumn,
    pub right_sort_order: SortOrder,
    pub right_hidden: bool,
    /// Sort toggles, defaulted true for sessions written before they existed.
    #[serde(default = "default_true")]
    pub left_folders_first: bool,
    #[serde(default = "default_true")]
    pub left_natural_sort: bool,
    #[serde(default = "default_true")]
    pub right_folders_first: bool,
    #[serde(default = "default_true")]
    pub right_natural_sort: bool,
    /// List density, per panel. Defaulted for sessions written before
    /// density existed (and before it moved from a single app-wide tier to
    /// one per panel).
    #[serde(default)]
    pub left_density: crate::density::Density,
    #[serde(default)]
    pub right_density: crate::density::Density,
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
    pub sync_guard_policy: crate::sync_guard::GuardPolicy,
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
}

fn session_path() -> PathBuf {
    crate::fs_util::config_dir().join("session.json")
}

/// Load the saved session, or None if absent/corrupt.
pub fn load() -> Option<Session> {
    let data = std::fs::read_to_string(session_path()).ok()?;
    serde_json::from_str(&data).ok()
}

/// Save the session atomically (temp file + rename). Returns `false` if
/// serialization or the atomic write failed. The autosave caller treats this
/// as best-effort; an explicit caller could surface the failure.
pub fn save(session: &Session) -> bool {
    match serde_json::to_string_pretty(session) {
        Ok(json) => crate::fs_util::write_atomic(&session_path(), &json),
        Err(_) => false,
    }
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
            left_sort_col: SortColumn::Size,
            left_sort_order: SortOrder::Desc,
            left_hidden: true,
            right_sort_col: SortColumn::Name,
            right_sort_order: SortOrder::Asc,
            right_hidden: false,
            left_folders_first: true,
            left_natural_sort: false,
            right_folders_first: false,
            right_natural_sort: true,
            left_density: crate::density::Density::Compact,
            right_density: crate::density::Density::Spacious,
            palette_usage: crate::command::UsageStats::default(),
            palette_tick: 7,
            recent_paths: Vec::new(),
            recent_stats: crate::panel::VisitStats::default(),
            recent_order: crate::panel::RecentOrder::Frecency,
            search_history: crate::search::QueryHistory::default(),
            durability_profile: crate::operation::DurabilityProfile::default(),
            sync_guard_policy: crate::sync_guard::GuardPolicy::default(),
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
        assert_eq!(back.left_density, crate::density::Density::Comfortable);
        assert_eq!(back.right_density, crate::density::Density::Comfortable);
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
        assert!(back.left_folders_first);
        assert!(back.left_natural_sort);
        assert!(back.right_folders_first);
        assert!(back.right_natural_sort);
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
        let back: Session = serde_json::from_value(val).unwrap();
        assert!(back.recent_paths.is_empty());
        assert_eq!(back.recent_stats, crate::panel::VisitStats::default());
        assert_eq!(back.recent_order, crate::panel::RecentOrder::Frecency);
        assert_eq!(back.search_history, crate::search::QueryHistory::default());
    }

    #[test]
    fn round_trips_through_json() {
        let s = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
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
