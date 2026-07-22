//! Recent directories / visited paths logic.
//! Extracted for SRP: global recent list for Cmd+P switcher, separate from per-panel history.
//! Pure functions testable in isolation. Used by nav and UI.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Session-wide most-recent-first list of visited directories (for the
/// Cmd+P quick switcher), distinct from each panel's linear history.
pub fn visited_log() -> &'static Mutex<Vec<PathBuf>> {
    static V: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(Vec::new()))
}

const VISITED_CAP: usize = 50;

/// Push `path` to the front of `list`, de-duplicating and capping. Pure, so
/// the ordering logic is unit-testable without the global.
pub fn push_visit(list: &mut Vec<PathBuf>, path: &Path, cap: usize) {
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(cap);
}

/// Record a visit to `path` in the global recent list.
pub fn record_visit(path: &Path) {
    if let Ok(mut v) = visited_log().lock() {
        push_visit(&mut v, path, VISITED_CAP);
    }
}

/// Snapshot of recently visited directories, most recent first.
pub fn visited_paths() -> Vec<PathBuf> {
    visited_log().lock().map(|v| v.clone()).unwrap_or_default()
}

/// Filter visited paths by a case-insensitive substring over the full path.
pub fn filter_visited(paths: &[PathBuf], query: &str) -> Vec<PathBuf> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return paths.to_vec();
    }
    paths
        .iter()
        .filter(|p| p.to_string_lossy().to_lowercase().contains(&q))
        .cloned()
        .collect()
}