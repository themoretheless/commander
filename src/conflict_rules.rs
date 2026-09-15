//! Saved conflict rules scoped by root pair and file kind (research **J009**).
use crate::conflict::RelationPolicy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredRelationPolicy { ReplaceAll, SkipAll, KeepBoth, KeepNewer, KeepLarger }
impl From<StoredRelationPolicy> for RelationPolicy {
    fn from(v: StoredRelationPolicy) -> Self {
        match v { StoredRelationPolicy::ReplaceAll => Self::ReplaceAll, StoredRelationPolicy::SkipAll => Self::SkipAll, StoredRelationPolicy::KeepBoth => Self::KeepBoth, StoredRelationPolicy::KeepNewer => Self::KeepNewer, StoredRelationPolicy::KeepLarger => Self::KeepLarger }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictRule { pub left_root: PathBuf, pub right_root: PathBuf, pub file_kind: String, pub policy: StoredRelationPolicy }
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictRuleBook { pub rules: Vec<ConflictRule> }
impl ConflictRuleBook {
    pub fn find(&self, left: &Path, right: &Path, file_kind: &str) -> Option<&ConflictRule> {
        self.rules.iter().find(|r| r.left_root == left && r.right_root == right && r.file_kind == file_kind)
    }
    pub fn upsert(&mut self, rule: ConflictRule) {
        if let Some(existing) = self.rules.iter_mut().find(|c| c.left_root == rule.left_root && c.right_root == rule.right_root && c.file_kind == rule.file_kind) { *existing = rule; } else { self.rules.push(rule); }
    }
    pub fn sample_preview(policy: StoredRelationPolicy, names: &[String], limit: usize) -> String {
        let shown: Vec<_> = names.iter().take(limit).cloned().collect();
        let more = names.len().saturating_sub(shown.len());
        let list = if shown.is_empty() { "(no sample conflicts)".into() } else if more == 0 { shown.join(", ") } else { format!("{}, +{more} more", shown.join(", ")) };
        format!("{policy:?} would apply to {list}")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sample_preview_is_deterministic() {
        let preview = ConflictRuleBook::sample_preview(StoredRelationPolicy::KeepNewer, &["a.txt".into(), "b.txt".into(), "c.txt".into()], 2);
        assert!(preview.contains("a.txt"));
        assert!(preview.contains("+1 more"));
    }
}
