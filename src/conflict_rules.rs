//! Saved conflict rules scoped by root pair and file kind (research **J009**).

use crate::conflict::RelationPolicy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.conflict_rules", 1, 2 * 1024 * 1024);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredRelationPolicy {
    ReplaceAll,
    SkipAll,
    KeepBoth,
    KeepNewer,
    KeepLarger,
}

impl From<StoredRelationPolicy> for RelationPolicy {
    fn from(value: StoredRelationPolicy) -> Self {
        match value {
            StoredRelationPolicy::ReplaceAll => Self::ReplaceAll,
            StoredRelationPolicy::SkipAll => Self::SkipAll,
            StoredRelationPolicy::KeepBoth => Self::KeepBoth,
            StoredRelationPolicy::KeepNewer => Self::KeepNewer,
            StoredRelationPolicy::KeepLarger => Self::KeepLarger,
        }
    }
}

impl From<RelationPolicy> for StoredRelationPolicy {
    fn from(value: RelationPolicy) -> Self {
        match value {
            RelationPolicy::ReplaceAll => Self::ReplaceAll,
            RelationPolicy::SkipAll => Self::SkipAll,
            RelationPolicy::KeepBoth => Self::KeepBoth,
            RelationPolicy::KeepNewer => Self::KeepNewer,
            RelationPolicy::KeepLarger => Self::KeepLarger,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictRule {
    pub left_root: PathBuf,
    pub right_root: PathBuf,
    pub file_kind: String,
    pub policy: StoredRelationPolicy,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictRuleBook {
    pub rules: Vec<ConflictRule>,
}

impl ConflictRuleBook {
    pub fn find(&self, left: &Path, right: &Path, file_kind: &str) -> Option<&ConflictRule> {
        self.rules.iter().find(|rule| {
            rule.left_root == left && rule.right_root == right && rule.file_kind == file_kind
        })
    }

    pub fn upsert(&mut self, rule: ConflictRule) {
        if let Some(existing) = self.rules.iter_mut().find(|candidate| {
            candidate.left_root == rule.left_root
                && candidate.right_root == rule.right_root
                && candidate.file_kind == rule.file_kind
        }) {
            *existing = rule;
        } else {
            self.rules.push(rule);
        }
    }

    pub fn sample_preview(policy: StoredRelationPolicy, names: &[String], limit: usize) -> String {
        let shown: Vec<_> = names.iter().take(limit).cloned().collect();
        let more = names.len().saturating_sub(shown.len());
        let list = if shown.is_empty() {
            "(no sample conflicts)".into()
        } else if more == 0 {
            shown.join(", ")
        } else {
            format!("{}, +{more} more", shown.join(", "))
        };
        format!("{policy:?} would apply to {list}")
    }

    pub fn kind_for_name(name: &str) -> String {
        Path::new(name)
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .unwrap_or_else(|| "other".into())
    }
}

fn store_path() -> PathBuf {
    crate::fs_util::config_dir().join("conflict_rules.json")
}

fn cache() -> &'static Mutex<ConflictRuleBook> {
    static CACHE: OnceLock<Mutex<ConflictRuleBook>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(load_store()))
}

fn persist() -> &'static crate::persistence::FsPersist {
    static PERSIST: OnceLock<crate::persistence::FsPersist> = OnceLock::new();
    PERSIST.get_or_init(crate::persistence::FsPersist::default)
}

fn load_store() -> ConflictRuleBook {
    crate::persistence::load_enveloped::<ConflictRuleBook>(persist(), &store_path(), STORE)
        .value
        .unwrap_or_default()
}

fn save_store(store: &ConflictRuleBook) {
    let mut loaded =
        crate::persistence::load_enveloped::<ConflictRuleBook>(persist(), &store_path(), STORE);
    let _ = crate::persistence::save_enveloped(
        persist(),
        &store_path(),
        STORE,
        store,
        &mut loaded.gate,
        crate::persistence::SaveIntent::Explicit,
    );
}

pub fn load() -> ConflictRuleBook {
    crate::lock_util::recover(cache()).clone()
}

pub fn save(book: &ConflictRuleBook) {
    *crate::lock_util::recover(cache()) = book.clone();
    save_store(book);
}

pub fn upsert(rule: ConflictRule) {
    let mut book = load();
    book.upsert(rule);
    save(&book);
}

pub fn preview_for(
    left: &Path,
    right: &Path,
    file_kind: &str,
    names: &[String],
    limit: usize,
) -> Option<String> {
    load()
        .find(left, right, file_kind)
        .map(|rule| ConflictRuleBook::sample_preview(rule.policy, names, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_preview_is_deterministic_and_rules_persist() {
        let preview = ConflictRuleBook::sample_preview(
            StoredRelationPolicy::KeepNewer,
            &["a.txt".into(), "b.txt".into(), "c.txt".into()],
            2,
        );
        assert!(preview.contains("a.txt"));
        assert!(preview.contains("+1 more"));
        assert_eq!(ConflictRuleBook::kind_for_name("photo.PNG"), "png");

        let mut book = ConflictRuleBook::default();
        book.upsert(ConflictRule {
            left_root: PathBuf::from("/a"),
            right_root: PathBuf::from("/b"),
            file_kind: "txt".into(),
            policy: StoredRelationPolicy::KeepBoth,
        });
        save(&book);
        assert!(
            load()
                .find(Path::new("/a"), Path::new("/b"), "txt")
                .is_some()
        );
        assert!(
            preview_for(
                Path::new("/a"),
                Path::new("/b"),
                "txt",
                &["x.txt".into()],
                3
            )
            .unwrap()
            .contains("KeepBoth")
        );
    }
}
