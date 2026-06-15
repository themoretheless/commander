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
    let dir = dirs::config_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("commander");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("smart_folders.json")
}

/// Load the saved searches, or an empty set if absent/corrupt.
pub fn load() -> SmartFolders {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Save the searches atomically (temp file + rename), best-effort.
pub fn save(store: &SmartFolders) {
    let Ok(json) = serde_json::to_string_pretty(store) else {
        return;
    };
    let path = store_path();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
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
            },
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: Definition = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}
