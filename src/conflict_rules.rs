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

impl StoredRelationPolicy {
    /// The policy as the confirmation dialog's button names it.
    pub const fn label(self) -> &'static str {
        match self {
            Self::ReplaceAll => "Overwrite All",
            Self::SkipAll => "Skip Existing",
            Self::KeepBoth => "Keep Both",
            Self::KeepNewer => "Keep Newer",
            Self::KeepLarger => "Keep Larger",
        }
    }
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
        format!("{} would apply to {list}", policy.label())
    }

    /// The "Saved rule: ..." line for the rule matching this root pair and
    /// file kind, if any.
    pub fn saved_rule_preview(
        &self,
        left: &Path,
        right: &Path,
        file_kind: &str,
        names: &[String],
        limit: usize,
    ) -> Option<String> {
        self.find(left, right, file_kind).map(|rule| {
            format!(
                "Saved rule: {}",
                Self::sample_preview(rule.policy, names, limit)
            )
        })
    }

    pub fn kind_for_name(name: &str) -> String {
        Path::new(name)
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
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
    load_store_at(persist(), &store_path())
}

/// A store that exists but cannot be read loads empty and blocks saves, so
/// it is reported through persistence health instead of vanishing silently.
fn load_store_at(persist: &dyn crate::persistence::Persist, path: &Path) -> ConflictRuleBook {
    let loaded = crate::persistence::load_enveloped::<ConflictRuleBook>(persist, path, STORE);
    if loaded.value.is_none() && loaded.gate.status().fails_closed() {
        crate::persistence::record_unreadable("Conflict rules");
    }
    loaded.value.unwrap_or_default()
}

fn save_store(store: &ConflictRuleBook) -> bool {
    save_store_at(persist(), &store_path(), store)
}

/// Failed or blocked saves reach persistence health (and its error toast);
/// `false` when nothing was committed.
fn save_store_at(
    persist: &dyn crate::persistence::Persist,
    path: &Path,
    store: &ConflictRuleBook,
) -> bool {
    let mut loaded = crate::persistence::load_enveloped::<ConflictRuleBook>(persist, path, STORE);
    crate::persistence::report_save(
        "Conflict rules",
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

pub fn load() -> ConflictRuleBook {
    crate::lock_util::recover(cache()).clone()
}

/// Updates the in-memory book and persists it; `false` when the write
/// failed (already reported through persistence health).
pub fn save(book: &ConflictRuleBook) -> bool {
    *crate::lock_util::recover(cache()) = book.clone();
    save_store(book)
}

pub fn upsert(rule: ConflictRule) {
    let mut book = load();
    book.upsert(rule);
    let _ = save(&book);
}

/// The saved policy for this root pair and file kind, if any.
pub fn saved_policy(left: &Path, right: &Path, file_kind: &str) -> Option<RelationPolicy> {
    load()
        .find(left, right, file_kind)
        .map(|rule| rule.policy.into())
}

pub fn preview_for(
    left: &Path,
    right: &Path,
    file_kind: &str,
    names: &[String],
    limit: usize,
) -> Option<String> {
    load().saved_rule_preview(left, right, file_kind, names, limit)
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
        assert!(preview.starts_with("Keep Newer would apply to "));
        assert!(!preview.contains("KeepNewer"));
        assert_eq!(ConflictRuleBook::kind_for_name("photo.PNG"), "png");

        // Persist through a temp store: the real `save` would write into the
        // developer's own config directory and seed the process-wide cache.
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("conflict_rules.json");
        let persist = crate::persistence::FsPersist::default();
        let mut book = ConflictRuleBook::default();
        book.upsert(rule("txt", StoredRelationPolicy::KeepBoth));
        assert!(save_store_at(&persist, &path, &book));
        let loaded = load_store_at(&persist, &path);
        assert_eq!(loaded, book);
        let found = loaded
            .find(Path::new("/a"), Path::new("/b"), "txt")
            .unwrap();
        assert_eq!(RelationPolicy::from(found.policy), RelationPolicy::KeepBoth);
        assert!(
            loaded
                .saved_rule_preview(
                    Path::new("/a"),
                    Path::new("/b"),
                    "txt",
                    &["x.txt".into()],
                    3
                )
                .unwrap()
                .starts_with("Saved rule: Keep Both would apply to x.txt")
        );
        // Rules are scoped: other kinds and swapped roots do not match.
        assert!(
            loaded
                .find(Path::new("/a"), Path::new("/b"), "png")
                .is_none()
        );
        assert!(
            loaded
                .saved_rule_preview(Path::new("/b"), Path::new("/a"), "txt", &[], 3)
                .is_none()
        );
    }

    #[test]
    fn file_kinds_fold_unicode_and_classify_extensionless_edges() {
        assert_eq!(ConflictRuleBook::kind_for_name("фото.ФОТО"), "фото");
        assert_eq!(
            ConflictRuleBook::kind_for_name("фото.ФОТО"),
            ConflictRuleBook::kind_for_name("фото.фото")
        );
        assert_eq!(ConflictRuleBook::kind_for_name(".bashrc"), "other");
        assert_eq!(ConflictRuleBook::kind_for_name("file."), "other");
    }

    fn rule(file_kind: &str, policy: StoredRelationPolicy) -> ConflictRule {
        ConflictRule {
            left_root: PathBuf::from("/a"),
            right_root: PathBuf::from("/b"),
            file_kind: file_kind.into(),
            policy,
        }
    }

    #[test]
    fn upsert_replaces_the_policy_of_an_existing_scope() {
        let mut book = ConflictRuleBook::default();
        book.upsert(rule("txt", StoredRelationPolicy::KeepBoth));
        book.upsert(rule("png", StoredRelationPolicy::SkipAll));
        book.upsert(rule("txt", StoredRelationPolicy::KeepNewer));
        assert_eq!(
            book.rules,
            vec![
                rule("txt", StoredRelationPolicy::KeepNewer),
                rule("png", StoredRelationPolicy::SkipAll),
            ]
        );
    }

    #[test]
    fn unreadable_store_is_reported_and_blocks_saves_visibly() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.file("store.json", "{ not json");
        let persist = crate::persistence::FsPersist::default();
        let before = crate::persistence::health_snapshot();
        assert_eq!(load_store_at(&persist, &path), ConflictRuleBook::default());
        let after_load = crate::persistence::health_snapshot();
        assert!(after_load.unreadable_stores > before.unreadable_stores);
        assert!(!save_store_at(
            &persist,
            &path,
            &ConflictRuleBook::default()
        ));
        assert!(crate::persistence::health_snapshot().save_failures > after_load.save_failures);
    }

    #[test]
    fn missing_store_is_not_reported_as_unreadable() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("store.json");
        let persist = crate::persistence::FsPersist::default();
        assert_eq!(load_store_at(&persist, &path), ConflictRuleBook::default());
        assert!(save_store_at(&persist, &path, &ConflictRuleBook::default()));
    }
}
