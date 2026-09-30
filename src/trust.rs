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
    crate::persistence::load_enveloped::<TrustStore>(persist(), &store_path(), STORE)
        .value
        .unwrap_or_default()
}

fn save_store(store: &TrustStore) {
    let mut loaded =
        crate::persistence::load_enveloped::<TrustStore>(persist(), &store_path(), STORE);
    let _ = crate::persistence::save_enveloped(
        persist(),
        &store_path(),
        STORE,
        store,
        &mut loaded.gate,
        crate::persistence::SaveIntent::Explicit,
    );
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
    let mut best: Option<(usize, TrustLabel)> = None;
    for (root, label) in &store.roots {
        if needle == *root || needle.starts_with(&format!("{root}/")) {
            let len = root.len();
            if best.as_ref().is_none_or(|(cur, _)| len >= *cur) {
                best = Some((len, *label));
            }
        }
    }
    best.map_or(TrustLabel::Trusted, |(_, label)| label)
}

pub fn set_label(path: &Path, label: TrustLabel) {
    let key = normalize_root(path);
    let mut store = crate::lock_util::recover(cache());
    if label == TrustLabel::Trusted {
        store.roots.remove(&key);
    } else {
        store.roots.insert(key, label);
    }
    save_store(&store);
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
}
