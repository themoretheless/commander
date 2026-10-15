//! Named workspace profiles (research **J008**).

use crate::filesystem_policy::{NamePolicy, SymlinkPolicy};
use crate::operation::DurabilityProfile;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.workspace_profiles", 1, 4 * 1024 * 1024);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProfile {
    pub name: String,
    pub left_root: PathBuf,
    pub right_root: PathBuf,
    #[serde(default)]
    pub left_filter: String,
    #[serde(default)]
    pub right_filter: String,
    #[serde(default)]
    pub durability: DurabilityProfile,
    #[serde(default)]
    pub name_policy: NamePolicy,
    #[serde(default)]
    pub symlink_policy: SymlinkPolicy,
    #[serde(default)]
    pub trusted_command_templates: Vec<String>,
}

impl WorkspaceProfile {
    pub fn new(name: impl Into<String>, left_root: PathBuf, right_root: PathBuf) -> Self {
        Self {
            name: name.into(),
            left_root,
            right_root,
            left_filter: String::new(),
            right_filter: String::new(),
            durability: DurabilityProfile::default(),
            name_policy: NamePolicy::default(),
            symlink_policy: SymlinkPolicy::default(),
            trusted_command_templates: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBook {
    pub profiles: Vec<WorkspaceProfile>,
}

impl ProfileBook {
    pub fn upsert(&mut self, profile: WorkspaceProfile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == profile.name) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    pub fn get(&self, name: &str) -> Option<&WorkspaceProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }
}

fn store_path() -> PathBuf {
    crate::fs_util::config_dir().join("workspace_profiles.json")
}

fn cache() -> &'static Mutex<ProfileBook> {
    static CACHE: OnceLock<Mutex<ProfileBook>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(load_store()))
}

fn persist() -> &'static crate::persistence::FsPersist {
    static PERSIST: OnceLock<crate::persistence::FsPersist> = OnceLock::new();
    PERSIST.get_or_init(crate::persistence::FsPersist::default)
}

fn load_store() -> ProfileBook {
    load_store_at(persist(), &store_path())
}

/// A store that exists but cannot be read loads empty and blocks saves, so
/// it is reported through persistence health instead of vanishing silently.
fn load_store_at(persist: &dyn crate::persistence::Persist, path: &Path) -> ProfileBook {
    let loaded = crate::persistence::load_enveloped::<ProfileBook>(persist, path, STORE);
    if loaded.value.is_none() && loaded.gate.status().fails_closed() {
        crate::persistence::record_unreadable("Workspace profiles");
    }
    loaded.value.unwrap_or_default()
}

fn save_store(store: &ProfileBook) -> bool {
    save_store_at(persist(), &store_path(), store)
}

/// Failed or blocked saves reach persistence health (and its error toast);
/// `false` when nothing was committed.
fn save_store_at(
    persist: &dyn crate::persistence::Persist,
    path: &Path,
    store: &ProfileBook,
) -> bool {
    let mut loaded = crate::persistence::load_enveloped::<ProfileBook>(persist, path, STORE);
    crate::persistence::report_save(
        "Workspace profiles",
        crate::persistence::save_enveloped(
            persist,
            path,
            STORE,
            store,
            &mut loaded.gate,
            crate::persistence::SaveIntent::Explicit,
        ),
    )
    .is_some()
}

pub fn load() -> ProfileBook {
    crate::lock_util::recover(cache()).clone()
}

/// Updates the in-memory book and persists it; `false` when the write
/// failed (already reported through persistence health).
pub fn save(book: &ProfileBook) -> bool {
    *crate::lock_util::recover(cache()) = book.clone();
    save_store(book)
}

/// Adds or replaces `profile` and persists the book; `false` when the
/// write failed (already reported through persistence health).
pub fn upsert(profile: WorkspaceProfile) -> bool {
    let mut book = load();
    book.upsert(profile);
    save(&book)
}

#[derive(Clone, Debug)]
pub struct CaptureParams<'a> {
    pub name: String,
    pub left_root: &'a Path,
    pub right_root: &'a Path,
    pub left_filter: String,
    pub right_filter: String,
    pub durability: DurabilityProfile,
    pub name_policy: NamePolicy,
    pub symlink_policy: SymlinkPolicy,
    pub trusted_command_templates: Vec<String>,
}

pub fn capture(params: CaptureParams<'_>) -> WorkspaceProfile {
    let mut profile = WorkspaceProfile::new(
        params.name,
        params.left_root.to_path_buf(),
        params.right_root.to_path_buf(),
    );
    profile.left_filter = params.left_filter;
    profile.right_filter = params.right_filter;
    profile.durability = params.durability;
    profile.name_policy = params.name_policy;
    profile.symlink_policy = params.symlink_policy;
    profile.trusted_command_templates = params.trusted_command_templates;
    // The failure is already surfaced as an error toast by `save`. Callers
    // that show a success notice (workspace.rs, developer_panel.rs) do not
    // see this outcome yet; returning it from `capture` is follow-up work.
    let _saved = upsert(profile.clone());
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_upsert_by_name_and_persist() {
        let mut book = ProfileBook::default();
        book.upsert(WorkspaceProfile::new(
            "photos",
            PathBuf::from("/l"),
            PathBuf::from("/r"),
        ));
        book.upsert(WorkspaceProfile::new(
            "photos",
            PathBuf::from("/l2"),
            PathBuf::from("/r2"),
        ));
        assert_eq!(book.profiles.len(), 1);
        assert_eq!(book.get("photos").unwrap().left_root, PathBuf::from("/l2"));
        // Round-trip through a temp store: `save()` would overwrite the
        // developer's real workspace_profiles.json.
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("workspace_profiles.json");
        let persist = crate::persistence::FsPersist::default();
        assert!(save_store_at(&persist, &path, &book));
        assert_eq!(load_store_at(&persist, &path), book);
    }

    #[test]
    fn unreadable_store_is_reported_and_blocks_saves_visibly() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.file("store.json", "{ not json");
        let persist = crate::persistence::FsPersist::default();
        let before = crate::persistence::health_snapshot();
        assert_eq!(load_store_at(&persist, &path), ProfileBook::default());
        let after_load = crate::persistence::health_snapshot();
        assert!(after_load.unreadable_stores > before.unreadable_stores);
        assert!(!save_store_at(&persist, &path, &ProfileBook::default()));
        assert!(crate::persistence::health_snapshot().save_failures > after_load.save_failures);
    }

    #[test]
    fn missing_store_is_not_reported_as_unreadable() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("store.json");
        let persist = crate::persistence::FsPersist::default();
        assert_eq!(load_store_at(&persist, &path), ProfileBook::default());
        assert!(save_store_at(&persist, &path, &ProfileBook::default()));
    }
}
