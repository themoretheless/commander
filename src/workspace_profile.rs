//! Named workspace profiles (research **J008**).
use crate::filesystem_policy::{NamePolicy, SymlinkPolicy};
use crate::operation::DurabilityProfile;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProfile {
    pub name: String,
    pub left_root: PathBuf,
    pub right_root: PathBuf,
    #[serde(default)] pub left_filter: String,
    #[serde(default)] pub right_filter: String,
    #[serde(default)] pub durability: DurabilityProfile,
    #[serde(default)] pub name_policy: NamePolicy,
    #[serde(default)] pub symlink_policy: SymlinkPolicy,
    #[serde(default)] pub trusted_command_templates: Vec<String>,
}
impl WorkspaceProfile {
    pub fn new(name: impl Into<String>, left_root: PathBuf, right_root: PathBuf) -> Self {
        Self { name: name.into(), left_root, right_root, left_filter: String::new(), right_filter: String::new(), durability: DurabilityProfile::default(), name_policy: NamePolicy::default(), symlink_policy: SymlinkPolicy::default(), trusted_command_templates: Vec::new() }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBook { pub profiles: Vec<WorkspaceProfile> }
impl ProfileBook {
    pub fn upsert(&mut self, profile: WorkspaceProfile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == profile.name) { *existing = profile; } else { self.profiles.push(profile); }
    }
    pub fn get(&self, name: &str) -> Option<&WorkspaceProfile> { self.profiles.iter().find(|p| p.name == name) }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_upsert_by_name() {
        let mut book = ProfileBook::default();
        book.upsert(WorkspaceProfile::new("photos", PathBuf::from("/l"), PathBuf::from("/r")));
        book.upsert(WorkspaceProfile::new("photos", PathBuf::from("/l2"), PathBuf::from("/r2")));
        assert_eq!(book.profiles.len(), 1);
        assert_eq!(book.get("photos").unwrap().left_root, PathBuf::from("/l2"));
    }
}
