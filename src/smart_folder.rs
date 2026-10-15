//! Saved searches ("smart folders"): a named [`Query`] over a root, persisted
//! so a recurring hunt can be re-run with one click. The store is pure and
//! serde-backed; load/save touch a JSON file under the config dir.

use crate::query::Query;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const STORE_NAME: &str = "Saved searches";

/// A named, persisted search.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Definition {
    pub name: String,
    pub root: PathBuf,
    pub query: Query,
}

/// An ordered set of saved searches, unique by name.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct SmartFolders {
    pub items: Vec<Definition>,
}

impl SmartFolders {
    /// Add `def`, replacing any existing entry with the same name (so
    /// re-saving a name updates it in place rather than duplicating).
    pub fn add(&mut self, def: Definition) {
        if let Some(slot) = self.items.iter_mut().find(|d| d.name == def.name) {
            *slot = def;
        } else {
            self.items.push(def);
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.items.retain(|d| d.name != name);
    }
}

fn store_path() -> PathBuf {
    crate::fs_util::config_dir().join("smart_folders.json")
}

/// Load the saved searches, or an empty set if absent/corrupt.
pub fn load() -> SmartFolders {
    load_from(&store_path())
}

fn load_from(path: &Path) -> SmartFolders {
    SmartFolders {
        items: crate::persistence::load_item_store(path, STORE_NAME),
    }
}

/// Save the searches atomically (temp file + rename). Returns `false` if
/// serialization or the atomic write failed, so the caller can surface it.
/// A store that loaded as empty/partial because it was unreadable is moved
/// aside before being replaced (see [`crate::persistence::save_item_store`]).
pub fn save(store: &SmartFolders) -> bool {
    save_to(&store_path(), store)
}

fn save_to(path: &Path, store: &SmartFolders) -> bool {
    crate::persistence::save_item_store(path, STORE_NAME, store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Predicate;
    use crate::selection_summary::Kind;

    fn def(name: &str, root: &str) -> Definition {
        Definition {
            name: name.to_string(),
            root: PathBuf::from(root),
            query: Query {
                predicates: vec![Predicate::NameContains("x".into())],
                mode: crate::query::MatchMode::Exact,
            },
        }
    }

    #[test]
    fn add_replaces_same_name_no_dupes() {
        let mut s = SmartFolders::default();
        s.add(def("Logs", "/a"));
        s.add(def("Photos", "/b"));
        s.add(def("Logs", "/c")); // same name, new root -> replace in place
        assert_eq!(s.items.len(), 2);
        let logs = s.items.iter().find(|d| d.name == "Logs").unwrap();
        assert_eq!(logs.root, PathBuf::from("/c"));
        // Order is preserved (Logs stays first).
        assert_eq!(s.items[0].name, "Logs");
    }

    #[test]
    fn remove_drops_by_name() {
        let mut s = SmartFolders::default();
        s.add(def("Logs", "/a"));
        assert!(s.items.iter().any(|d| d.name == "Logs"));
        s.remove("Logs");
        assert!(s.items.is_empty());
    }

    #[test]
    fn round_trips_every_predicate_through_json() {
        for mode in crate::query::MatchMode::ALL {
            let d = Definition {
                name: "Everything".into(),
                root: PathBuf::from("/root"),
                query: Query {
                    predicates: vec![
                        Predicate::NameContains("report".into()),
                        Predicate::PathContains("Finance/2026".into()),
                        Predicate::ContentContains("revenue".into()),
                        Predicate::Kind(Kind::Document),
                        Predicate::MinSize(1024),
                        Predicate::MaxSize(1 << 30),
                        Predicate::MaxAgeDays(30),
                        Predicate::MinAgeDays(365),
                    ],
                    mode,
                },
            };
            let json = serde_json::to_string(&d).unwrap();
            let back: Definition = serde_json::from_str(&json).unwrap();
            assert_eq!(d, back);
        }
    }

    #[test]
    fn save_preserves_a_partly_unreadable_store() {
        let temp = crate::testutil::TempDir::new();
        let mut stored = SmartFolders::default();
        stored.add(def("Logs", "/a"));
        let mut value = serde_json::to_value(&stored).unwrap();
        value["items"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "name": 7 }));
        let original = value.to_string();
        let path = temp.file("smart_folders.json", &original);

        let mut loaded = load_from(&path);
        assert_eq!(loaded, stored, "the valid search still loads");
        loaded.add(def("Photos", "/b"));
        assert!(save_to(&path, &loaded));
        assert_eq!(load_from(&path), loaded);

        let backup = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|p| p.to_string_lossy().contains(".unreadable-"))
            .expect("original moved aside");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), original);

        // A clean store is replaced in place without another backup.
        assert!(save_to(&path, &loaded));
        let backups = std::fs::read_dir(temp.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".unreadable-")
            })
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn old_saved_search_defaults_to_exact_mode() {
        let definition = def("Legacy", "/root");
        let mut value = serde_json::to_value(&definition).unwrap();
        value
            .get_mut("query")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("mode");
        let back: Definition = serde_json::from_value(value).unwrap();
        assert_eq!(back.query.mode, crate::query::MatchMode::Exact);
    }
}
