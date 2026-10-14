//! Per-root trust labels that gate run-command, external providers, and
//! automatic archive inspection (research **J003**).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.root_trust", 1, 1024 * 1024);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustLabel {
    #[default]
    Trusted,
    Restricted,
    Untrusted,
}

impl TrustLabel {
    pub const ALL: [Self; 3] = [Self::Trusted, Self::Restricted, Self::Untrusted];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Trusted => "Trusted",
            Self::Restricted => "Restricted",
            Self::Untrusted => "Untrusted",
        }
    }

    pub const fn allows_auto_archive_inspect(self) -> bool {
        matches!(self, Self::Trusted)
    }

    pub const fn allows_run_command(self) -> bool {
        !matches!(self, Self::Untrusted)
    }

    pub const fn allows_external_providers(self) -> bool {
        matches!(self, Self::Trusted)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct TrustStore {
    #[serde(default)]
    roots: BTreeMap<String, TrustLabel>,
}

fn store_path() -> PathBuf {
    crate::fs_util::config_dir().join("root_trust.json")
}

fn cache() -> &'static Mutex<TrustStore> {
    static CACHE: OnceLock<Mutex<TrustStore>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(load_store()))
}

fn persist() -> &'static crate::persistence::FsPersist {
    static PERSIST: OnceLock<crate::persistence::FsPersist> = OnceLock::new();
    PERSIST.get_or_init(crate::persistence::FsPersist::default)
}

fn load_store() -> TrustStore {
    load_store_at(persist(), &store_path())
}

/// A store that exists but cannot be read leaves every root Trusted, so the
/// user is warned through persistence health instead of silently losing
/// Restricted/Untrusted labels.
fn load_store_at(persist: &dyn crate::persistence::Persist, path: &Path) -> TrustStore {
    let loaded = crate::persistence::load_enveloped::<TrustStore>(persist, path, STORE);
    if loaded.value.is_none() && loaded.gate.status().fails_closed() {
        crate::persistence::record_unreadable("Trust");
    }
    loaded.value.unwrap_or_default()
}

fn save_store(store: &TrustStore) -> bool {
    save_store_at(persist(), &store_path(), store)
}

/// Whether `store` reached disk. A failure (including a corrupt or
/// unreadable file, which blocks the save so it is not overwritten) is
/// reported through persistence health, which the app shows as an error
/// toast: the label then holds only until restart.
fn save_store_at(
    persist: &dyn crate::persistence::Persist,
    path: &Path,
    store: &TrustStore,
) -> bool {
    let mut loaded = crate::persistence::load_enveloped::<TrustStore>(persist, path, STORE);
    let saved = crate::persistence::save_enveloped(
        persist,
        path,
        STORE,
        store,
        &mut loaded.gate,
        crate::persistence::SaveIntent::Explicit,
    );
    crate::persistence::report_save("Trust labels", saved).is_some()
}

fn normalize_root(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    absolute
        .canonicalize()
        .unwrap_or(absolute)
        .to_string_lossy()
        .into_owned()
}

/// Longest matching registered root wins; unknown roots default to Trusted.
pub fn label_for(path: &Path) -> TrustLabel {
    let needle = normalize_root(path);
    let store = crate::lock_util::recover(cache());
    best_label(&store.roots, &needle)
}

/// Component-wise prefix match, so a label on `/` (or `C:\`) covers its
/// descendants and the platform separator is respected.
fn best_label(roots: &BTreeMap<String, TrustLabel>, needle: &str) -> TrustLabel {
    let needle = Path::new(needle);
    let mut best: Option<(usize, TrustLabel)> = None;
    for (root, label) in roots {
        if needle.starts_with(Path::new(root)) {
            let len = root.len();
            if best.as_ref().is_none_or(|(cur, _)| len >= *cur) {
                best = Some((len, *label));
            }
        }
    }
    best.map_or(TrustLabel::Trusted, |(_, label)| label)
}

/// Apply `label` to `path` for this session and persist it; returns whether
/// it was saved (a failure is already reported to the user as a toast).
pub fn set_label(path: &Path, label: TrustLabel) -> bool {
    // Serializes saves so snapshots reach disk in mutation order, while the
    // cache lock itself is released before any disk I/O so `label_for` on
    // search/provider threads never waits on the write. Order: SAVE, then cache.
    static SAVE: Mutex<()> = Mutex::new(());
    let key = normalize_root(path);
    let _save = crate::lock_util::recover(&SAVE);
    let snapshot = {
        let mut store = crate::lock_util::recover(cache());
        if label == TrustLabel::Trusted {
            store.roots.remove(&key);
        } else {
            store.roots.insert(key, label);
        }
        store.clone()
    };
    save_store(&snapshot)
}

pub fn allows_auto_archive_inspect(path: &Path) -> bool {
    label_for(path).allows_auto_archive_inspect()
}

pub fn allows_run_command(path: &Path) -> bool {
    label_for(path).allows_run_command()
}

pub fn allows_external_providers(path: &Path) -> bool {
    label_for(path).allows_external_providers()
}

/// Clear the in-process trust cache so a test can start from an empty store.
/// Does not rewrite `root_trust.json`; pair with a serial mutex when mutating
/// labels so parallel tests cannot observe a half-updated cache.
#[cfg(test)]
pub fn reset_for_test() {
    let mut store = crate::lock_util::recover(cache());
    *store = TrustStore::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::sync::Mutex;

    static TEST_SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn restricted_blocks_auto_inspect_but_not_run_command() {
        assert!(!TrustLabel::Restricted.allows_auto_archive_inspect());
        assert!(TrustLabel::Restricted.allows_run_command());
        assert!(!TrustLabel::Restricted.allows_external_providers());
        assert!(!TrustLabel::Untrusted.allows_run_command());
    }

    #[test]
    fn assigned_labels_gate_descendant_paths() {
        let _guard = crate::lock_util::recover(&TEST_SERIAL);
        reset_for_test();

        let temp = TempDir::new();
        let root = temp.dir("project");
        // Create the descendant so canonicalize succeeds on macOS (/var →
        // /private/var); otherwise label_for falls back to a non-canonical
        // absolute path that no longer prefixes the stored root.
        let child = temp.file("project/nested/file.zip", "");
        set_label(&root, TrustLabel::Restricted);
        assert_eq!(label_for(&child), TrustLabel::Restricted);
        assert!(!allows_auto_archive_inspect(&child));
        assert!(allows_run_command(&child));
        set_label(&root, TrustLabel::Trusted);
        assert_eq!(label_for(&child), TrustLabel::Trusted);
        reset_for_test();
    }

    #[test]
    fn unreadable_store_is_reported_to_persistence_health() {
        let temp = TempDir::new();
        let path = temp.file("root_trust.json", "{ not json");
        let before = crate::persistence::health_snapshot().unreadable_stores;
        let store = load_store_at(&crate::persistence::FsPersist::default(), &path);
        assert!(store.roots.is_empty());
        assert!(crate::persistence::health_snapshot().unreadable_stores > before);
    }

    #[test]
    fn blocked_label_save_is_reported_and_keeps_the_file() {
        let temp = TempDir::new();
        let path = temp.file("root_trust.json", "{ not json");
        let before = crate::persistence::health_snapshot().save_failures;
        let store = TrustStore {
            roots: BTreeMap::from([("/srv".to_string(), TrustLabel::Untrusted)]),
        };

        assert!(!save_store_at(
            &crate::persistence::FsPersist::default(),
            &path,
            &store
        ));

        assert!(crate::persistence::health_snapshot().save_failures > before);
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    }

    #[test]
    fn missing_store_is_not_reported_as_unreadable() {
        let temp = TempDir::new();
        let path = temp.path().join("root_trust.json");
        let store = load_store_at(&crate::persistence::FsPersist::default(), &path);
        assert!(store.roots.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_root_label_covers_descendants() {
        let roots = BTreeMap::from([
            ("/".to_string(), TrustLabel::Untrusted),
            ("/srv/app".to_string(), TrustLabel::Restricted),
        ]);
        assert_eq!(best_label(&roots, "/home/user"), TrustLabel::Untrusted);
        assert_eq!(best_label(&roots, "/"), TrustLabel::Untrusted);
        assert_eq!(best_label(&roots, "/srv/app/x"), TrustLabel::Restricted);
        assert_eq!(
            best_label(&roots, "/srv/application"),
            TrustLabel::Untrusted
        );
    }
}
