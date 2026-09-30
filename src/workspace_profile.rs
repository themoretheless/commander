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
    crate::persistence::load_enveloped::<ProfileBook>(persist(), &store_path(), STORE)
        .value
        .unwrap_or_default()
}

fn save_store(store: &ProfileBook) {
    let mut loaded =
        crate::persistence::load_enveloped::<ProfileBook>(persist(), &store_path(), STORE);
    let _ = crate::persistence::save_enveloped(
        persist(),
        &store_path(),
        STORE,
        store,
        &mut loaded.gate,
        crate::persistence::SaveIntent::Explicit,
    );
}

pub fn load() -> ProfileBook {
    crate::lock_util::recover(cache()).clone()
}

pub fn save(book: &ProfileBook) {
    *crate::lock_util::recover(cache()) = book.clone();
    save_store(book);
}

pub fn upsert(profile: WorkspaceProfile) {
    let mut book = load();
    book.upsert(profile);
    save(&book);
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
    upsert(profile.clone());
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
        save(&book);
        assert!(load().get("photos").is_some());
    }
}
