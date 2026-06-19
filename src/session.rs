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
    /// List density. Defaulted for sessions written before density existed.
    #[serde(default)]
    pub density: crate::density::Density,
    /// Command-palette usage history, for recency/frequency ranking.
    #[serde(default)]
    pub palette_usage: crate::command::UsageStats,
    /// Monotonic counter stamped onto each palette command run.
    #[serde(default)]
    pub palette_tick: u64,
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

/// Save the session atomically (temp file + rename), best-effort.
pub fn save(session: &Session) {
    let Ok(json) = serde_json::to_string_pretty(session) else {
        return;
    };
    let path = session_path();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
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
            density: crate::density::Density::Compact,
            palette_usage: crate::command::UsageStats::default(),
            palette_tick: 7,
        }
    }

    #[test]
    fn density_defaults_when_absent_from_json() {
        // A session written before density existed has no `density` key.
        let s = sample(PathBuf::from("/a"), PathBuf::from("/b"));
        let mut val = serde_json::to_value(&s).unwrap();
        val.as_object_mut().unwrap().remove("density");
        let back: Session = serde_json::from_value(val).unwrap();
        assert_eq!(back.density, crate::density::Density::Comfortable);
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
