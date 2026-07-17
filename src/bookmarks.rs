//! Persisted directory bookmarks ("favorites"): a named, ordered,
//! de-duplicated set of directories, each optionally bound to a quick-jump slot
//! (1..=9, e.g. Cmd+1..9). Pure and serde-backed, mirroring [`crate::smart_folder`];
//! load/save touch a JSON file under the config dir.
//!
//! Backs the Favorites rail in the tree sidebar and the Cmd+1..9 quick-jump
//! slots; [`rail_model`] turns the store into a render-ready row list (pure, so
//! the current-dir marker and slot glyphs are unit-tested away from egui).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Highest assignable quick-jump slot (slots are 1..=9).
pub const MAX_SLOT: u8 = 9;

/// A single bookmarked directory.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub path: PathBuf,
    /// Quick-jump slot 1..=9, or `None` when unassigned. At most one bookmark
    /// holds a given slot (see [`Bookmarks::assign_slot`]).
    #[serde(default)]
    pub slot: Option<u8>,
}

/// An ordered, path-unique set of bookmarks.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Bookmarks {
    pub items: Vec<Bookmark>,
}

impl Bookmarks {
    /// Bookmark `path` under `name`. De-duplicated by path: if the path is
    /// already bookmarked the call is a no-op (first-seen entry and order are
    /// kept) and returns `false`; otherwise it is appended and returns `true`.
    pub fn add(&mut self, name: impl Into<String>, path: impl Into<PathBuf>) -> bool {
        let path = path.into();
        if self.contains(&path) {
            return false;
        }
        self.items.push(Bookmark {
            name: name.into(),
            path,
            slot: None,
        });
        true
    }

    /// Remove the bookmark for `path`, if any. Returns whether one was removed.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.items.len();
        self.items.retain(|b| b.path != path);
        self.items.len() != before
    }

    /// Rename the bookmark for `path`. Returns whether one was found.
    // Bookmark-management ops (rename/reorder/clear_slot/by_path) are tested
    // and ready; the favorites context menu that drives them is the next
    // bookmark iteration, so they are not called from non-test code yet.
    #[allow(dead_code)]
    pub fn rename(&mut self, path: &Path, new_name: impl Into<String>) -> bool {
        match self.items.iter_mut().find(|b| b.path == path) {
            Some(b) => {
                b.name = new_name.into();
                true
            }
            None => false,
        }
    }

    /// Move the bookmark at index `from` to index `to`, preserving the relative
    /// order of the rest (a remove-then-insert). `to` is clamped into range.
    /// Returns whether `from` was a valid index.
    #[allow(dead_code)]
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.items.len() {
            return false;
        }
        let to = to.min(self.items.len() - 1);
        if from == to {
            return true;
        }
        let b = self.items.remove(from);
        self.items.insert(to, b);
        true
    }

    /// Bind `path` to quick-jump `slot` (1..=9), stealing the slot from any
    /// other bookmark that currently holds it so slots stay unique. Returns
    /// `false` (changing nothing) for an out-of-range slot or an unknown path.
    pub fn assign_slot(&mut self, path: &Path, slot: u8) -> bool {
        if !(1..=MAX_SLOT).contains(&slot) {
            return false;
        }
        if !self.contains(path) {
            return false;
        }
        // Steal the slot from any prior holder (that is not the target itself).
        for b in self.items.iter_mut() {
            if b.slot == Some(slot) && b.path != path {
                b.slot = None;
            }
        }
        if let Some(b) = self.items.iter_mut().find(|b| b.path == path) {
            b.slot = Some(slot);
        }
        true
    }

    /// Clear any quick-jump slot bound to `path`. Returns whether the bookmark
    /// was found.
    #[allow(dead_code)]
    pub fn clear_slot(&mut self, path: &Path) -> bool {
        match self.items.iter_mut().find(|b| b.path == path) {
            Some(b) => {
                b.slot = None;
                true
            }
            None => false,
        }
    }

    /// The bookmark bound to quick-jump `slot`, if any.
    pub fn by_slot(&self, slot: u8) -> Option<&Bookmark> {
        self.items.iter().find(|b| b.slot == Some(slot))
    }

    /// The bookmark for `path`, if any.
    #[allow(dead_code)]
    pub fn by_path(&self, path: &Path) -> Option<&Bookmark> {
        self.items.iter().find(|b| b.path == path)
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.items.iter().any(|b| b.path == path)
    }
}

/// A render-ready Favorites row: the bookmark plus whether either panel is
/// currently in it (for a "you are here" dot).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RailRow {
    pub name: String,
    pub path: PathBuf,
    pub slot: Option<u8>,
    pub is_current: bool,
}

/// Build the Favorites rail in stored order, marking a row current when either
/// panel (`left`/`right`) is in that directory. Pure; the sidebar renders this.
pub fn rail_model(store: &Bookmarks, left: &Path, right: &Path) -> Vec<RailRow> {
    store
        .items
        .iter()
        .map(|b| RailRow {
            name: b.name.clone(),
            path: b.path.clone(),
            slot: b.slot,
            is_current: b.path == left || b.path == right,
        })
        .collect()
}

fn store_path() -> PathBuf {
    crate::fs_util::config_dir().join("bookmarks.json")
}

/// Load the saved bookmarks, or an empty set if absent/corrupt.
pub fn load() -> Bookmarks {
    Bookmarks {
        items: crate::persistence::load_item_store(&store_path(), "Bookmarks"),
    }
}

/// Save the bookmarks atomically (temp file + rename). Returns `false` if
/// serialization or the atomic write failed, so a caller with UI access can
/// surface the failure instead of letting it pass silently.
pub fn save(store: &Bookmarks) -> bool {
    match serde_json::to_string_pretty(store) {
        Ok(json) => crate::fs_util::write_atomic(&store_path(), &json),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bm() -> Bookmarks {
        let mut b = Bookmarks::default();
        b.add("Code", "/home/me/code");
        b.add("Docs", "/home/me/docs");
        b.add("Downloads", "/home/me/dl");
        b
    }

    #[test]
    fn add_dedupes_by_path_and_keeps_first_seen() {
        let mut b = Bookmarks::default();
        assert!(b.add("Code", "/a"));
        assert!(!b.add("Code again", "/a")); // same path -> rejected
        assert_eq!(b.items.len(), 1);
        // First-seen name is kept.
        assert_eq!(b.items[0].name, "Code");
    }

    #[test]
    fn add_preserves_insertion_order() {
        let b = bm();
        let names: Vec<&str> = b.items.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["Code", "Docs", "Downloads"]);
    }

    #[test]
    fn remove_and_rename_by_path() {
        let mut b = bm();
        assert!(b.rename(Path::new("/home/me/docs"), "Documents"));
        assert_eq!(
            b.by_path(Path::new("/home/me/docs")).unwrap().name,
            "Documents"
        );
        assert!(!b.rename(Path::new("/nope"), "X"));

        assert!(b.remove(Path::new("/home/me/docs")));
        assert!(!b.contains(Path::new("/home/me/docs")));
        assert!(!b.remove(Path::new("/home/me/docs"))); // already gone
        assert_eq!(b.items.len(), 2);
    }

    #[test]
    fn reorder_preserves_other_order_and_clamps() {
        let mut b = bm();
        // Move "Downloads" (index 2) to the front.
        assert!(b.reorder(2, 0));
        let names: Vec<&str> = b.items.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["Downloads", "Code", "Docs"]);
        // Out-of-range target is clamped to the last slot.
        assert!(b.reorder(0, 99));
        let names: Vec<&str> = b.items.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["Code", "Docs", "Downloads"]);
        // Out-of-range source is refused.
        assert!(!b.reorder(9, 0));
    }

    #[test]
    fn assign_slot_steals_from_prior_holder() {
        let mut b = bm();
        assert!(b.assign_slot(Path::new("/home/me/code"), 1));
        assert_eq!(b.by_slot(1).unwrap().name, "Code");
        // Assigning slot 1 to another bookmark steals it from Code.
        assert!(b.assign_slot(Path::new("/home/me/docs"), 1));
        assert_eq!(b.by_slot(1).unwrap().name, "Docs");
        assert_eq!(b.by_path(Path::new("/home/me/code")).unwrap().slot, None);
        // Only one holder of slot 1.
        let holders = b.items.iter().filter(|x| x.slot == Some(1)).count();
        assert_eq!(holders, 1);
    }

    #[test]
    fn assign_slot_rejects_out_of_range_and_unknown_path() {
        let mut b = bm();
        assert!(!b.assign_slot(Path::new("/home/me/code"), 0));
        assert!(!b.assign_slot(Path::new("/home/me/code"), 10));
        assert!(!b.assign_slot(Path::new("/unknown"), 3));
        // Nothing got a slot.
        assert!(b.items.iter().all(|x| x.slot.is_none()));
    }

    #[test]
    fn reassigning_same_path_to_a_new_slot_moves_it() {
        let mut b = bm();
        assert!(b.assign_slot(Path::new("/home/me/code"), 2));
        assert!(b.assign_slot(Path::new("/home/me/code"), 5));
        assert_eq!(b.by_path(Path::new("/home/me/code")).unwrap().slot, Some(5));
        assert!(b.by_slot(2).is_none());
        assert_eq!(b.by_slot(5).unwrap().name, "Code");
    }

    #[test]
    fn clear_slot_unbinds() {
        let mut b = bm();
        b.assign_slot(Path::new("/home/me/code"), 4);
        assert!(b.clear_slot(Path::new("/home/me/code")));
        assert!(b.by_slot(4).is_none());
        assert!(!b.clear_slot(Path::new("/unknown")));
    }

    #[test]
    fn round_trips_through_json_with_slots() {
        let mut b = bm();
        b.assign_slot(Path::new("/home/me/code"), 1);
        b.assign_slot(Path::new("/home/me/dl"), 9);
        let json = serde_json::to_string(&b).unwrap();
        let back: Bookmarks = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn rail_model_marks_current_panels_and_keeps_order_and_slots() {
        let mut b = bm();
        b.assign_slot(Path::new("/home/me/code"), 1);
        // Left panel sits in code, right panel in dl.
        let rows = rail_model(&b, Path::new("/home/me/code"), Path::new("/home/me/dl"));
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Code", "Docs", "Downloads"],
            "stored order kept"
        );
        assert_eq!(rows[0].slot, Some(1));
        assert!(rows[0].is_current, "left panel is in Code");
        assert!(!rows[1].is_current, "no panel is in Docs");
        assert!(rows[2].is_current, "right panel is in Downloads");
    }

    #[test]
    fn slot_defaults_when_absent_from_json() {
        // An older bookmarks file written before slots existed has no `slot` key.
        let mut val = serde_json::json!({ "items": [ { "name": "Code", "path": "/a" } ] });
        let back: Bookmarks = serde_json::from_value(val.take()).unwrap();
        assert_eq!(back.items[0].slot, None);
    }
}
