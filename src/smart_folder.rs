//! Saved searches ("smart folders"): a named [`Query`] over a root, persisted
//! so a recurring hunt can be re-run with one click. The store is pure and
//! serde-backed; load/save touch a JSON file under the config dir.

use crate::query::Query;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Save the searches atomically (temp file + rename). Returns `false` if
/// serialization or the atomic write failed, so the caller can surface it.
pub fn save(store: &SmartFolders) -> bool {
    match serde_json::to_string_pretty(store) {
        Ok(json) => crate::fs_util::write_atomic(&store_path(), &json),
        Err(_) => false,
    }
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
        let d = Definition {
            name: "Everything".into(),
            root: PathBuf::from("/root"),
            query: Query {
                predicates: vec![
                    Predicate::NameContains("report".into()),
                    Predicate::Kind(Kind::Document),
                    Predicate::MinSize(1024),
                    Predicate::MaxAgeDays(30),
                ],
                mode: crate::query::MatchMode::Exact,
            },
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: Definition = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
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
