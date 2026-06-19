//! Pure batch-rename planning: turn a list of file names plus a rule into a
//! list of proposed new names, flagging anything invalid or colliding. There
//! is no I/O and no UI here, so the whole transform is unit-tested directly on
//! fabricated name lists.

use std::collections::{HashMap, HashSet};

/// Case transform applied to the name stem (the extension is left untouched).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CaseMode {
    #[default]
    Keep,
    Lower,
    Upper,
}

/// Optional sequential counter appended to each stem, numbered by input order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Numbering {
    pub start: u32,
    pub step: u32,
    pub pad: usize,
}

/// A composable rename rule. Operations apply in a fixed, documented order:
/// literal find/replace over the whole name, then split off the extension,
/// then case, prefix, suffix, and finally the optional counter on the stem.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RenameRule {
    pub find: String,
    pub replace: String,
    pub prefix: String,
    pub suffix: String,
    pub case: CaseMode,
    pub numbering: Option<Numbering>,
}

/// Outcome of planning one entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlanStatus {
    /// Valid and different from the source name.
    Ok,
    /// The result equals the source name (nothing to do).
    Unchanged,
    /// Empty, contains '/', or is "." / "..".
    Invalid,
    /// Two entries map to the same target, or the target collides with a
    /// sibling that is not itself being renamed away.
    Collision,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenamePlan {
    pub from: String,
    pub to: String,
    pub status: PlanStatus,
}

/// Split a file name into (stem, extension-with-dot). A leading dot (dotfiles
/// like `.gitignore`) is part of the stem, not an extension; the split uses
/// the last interior dot, so `archive.tar.gz` -> (`archive.tar`, `.gz`).
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn apply_case(stem: &str, mode: CaseMode) -> String {
    match mode {
        CaseMode::Keep => stem.to_string(),
        CaseMode::Lower => stem.to_lowercase(),
        CaseMode::Upper => stem.to_uppercase(),
    }
}

/// Compute the proposed new name for a single entry at position `index`.
fn rename_one(name: &str, index: usize, rule: &RenameRule) -> String {
    // 1) literal find/replace over the full name (may touch the extension).
    let replaced = if rule.find.is_empty() {
        name.to_string()
    } else {
        name.replace(&rule.find, &rule.replace)
    };
    // 2) split off the extension; structural edits operate on the stem.
    let (stem, ext) = split_ext(&replaced);
    let mut stem = apply_case(stem, rule.case);
    stem = format!("{}{}{}", rule.prefix, stem, rule.suffix);
    if let Some(n) = rule.numbering {
        let value = u64::from(n.start) + index as u64 * u64::from(n.step);
        stem = format!("{stem}{value:0width$}", width = n.pad);
    }
    format!("{stem}{ext}")
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && name != "." && name != ".."
}

/// Plan a batch rename. `names` are the entries to rename (their order drives
/// the counter); `existing` is every name currently in the directory
/// (including the ones being renamed) so collisions with untouched siblings
/// are caught. Comparison is case-insensitive, matching the default macOS FS.
pub fn plan_batch_rename(
    names: &[String],
    existing: &HashSet<String>,
    rule: &RenameRule,
) -> Vec<RenamePlan> {
    let batch_lower: HashSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let existing_lower: HashSet<String> = existing.iter().map(|n| n.to_lowercase()).collect();

    let targets: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, n)| rename_one(n, i, rule))
        .collect();

    // How many targets share each lowercased name (intra-batch clashes).
    let mut counts: HashMap<String, usize> = HashMap::new();
    for t in &targets {
        *counts.entry(t.to_lowercase()).or_insert(0) += 1;
    }

    names
        .iter()
        .zip(targets.iter())
        .map(|(from, to)| {
            let to_lower = to.to_lowercase();
            let status = if !is_valid_name(to) {
                PlanStatus::Invalid
            } else if to == from {
                PlanStatus::Unchanged
            } else if counts.get(&to_lower).copied().unwrap_or(0) > 1 {
                PlanStatus::Collision
            } else if existing_lower.contains(&to_lower) && !batch_lower.contains(&to_lower) {
                // Clashes with a sibling that is not being renamed away.
                PlanStatus::Collision
            } else {
                PlanStatus::Ok
            };
            RenamePlan {
                from: from.clone(),
                to: to.clone(),
                status,
            }
        })
        .collect()
}

/// Whether a plan set is safe to apply: at least one effective change and no
/// invalid or colliding rows.
pub fn plan_is_applicable(plans: &[RenamePlan]) -> bool {
    let any_change = plans.iter().any(|p| p.status == PlanStatus::Ok);
    let any_bad = plans
        .iter()
        .any(|p| matches!(p.status, PlanStatus::Invalid | PlanStatus::Collision));
    any_change && !any_bad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn existing(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn find_replace_touches_whole_name_then_splits_extension() {
        let rule = RenameRule {
            find: "IMG".into(),
            replace: "Photo".into(),
            ..Default::default()
        };
        let n = names(&["IMG_01.jpg", "IMG_02.jpg"]);
        let ex = existing(&["IMG_01.jpg", "IMG_02.jpg"]);
        let plans = plan_batch_rename(&n, &ex, &rule);
        assert_eq!(plans[0].to, "Photo_01.jpg");
        assert_eq!(plans[1].to, "Photo_02.jpg");
        assert_eq!(plans[0].status, PlanStatus::Ok);
    }

    #[test]
    fn prefix_suffix_and_case_apply_to_the_stem_only() {
        let rule = RenameRule {
            prefix: "v_".into(),
            suffix: "_final".into(),
            case: CaseMode::Upper,
            ..Default::default()
        };
        let n = names(&["report.txt"]);
        let plans = plan_batch_rename(&n, &existing(&["report.txt"]), &rule);
        // stem REPORT -> v_REPORT_final, extension untouched.
        assert_eq!(plans[0].to, "v_REPORT_final.txt");
    }

    #[test]
    fn numbering_uses_input_order_with_padding() {
        let rule = RenameRule {
            prefix: "shot_".into(),
            numbering: Some(Numbering {
                start: 5,
                step: 5,
                pad: 3,
            }),
            ..Default::default()
        };
        // Counter follows input order; it is appended to the stem, ahead of
        // the extension. The third name has no extension.
        let n = names(&["a.png", "b.png", "c"]);
        let plans = plan_batch_rename(&n, &existing(&[]), &rule);
        assert_eq!(plans[0].to, "shot_a005.png");
        assert_eq!(plans[1].to, "shot_b010.png");
        assert_eq!(plans[2].to, "shot_c015");
    }

    #[test]
    fn dotfiles_keep_their_leading_dot_as_stem() {
        let rule = RenameRule {
            prefix: "x".into(),
            ..Default::default()
        };
        let plans = plan_batch_rename(&names(&[".gitignore"]), &existing(&[".gitignore"]), &rule);
        assert_eq!(plans[0].to, "x.gitignore");
    }

    #[test]
    fn unchanged_when_rule_is_a_noop_for_the_name() {
        let rule = RenameRule {
            find: "zzz".into(),
            replace: "q".into(),
            ..Default::default()
        };
        let plans = plan_batch_rename(&names(&["a.txt"]), &existing(&["a.txt"]), &rule);
        assert_eq!(plans[0].status, PlanStatus::Unchanged);
        assert_eq!(plans[0].to, "a.txt");
    }

    #[test]
    fn two_entries_to_same_target_are_collisions() {
        // Both lose their unique digits, colliding on "img.jpg".
        let rule = RenameRule {
            find: "1".into(),
            replace: "".into(),
            ..Default::default()
        };
        let n = names(&["img1.jpg", "img11.jpg"]);
        let plans = plan_batch_rename(
            &n,
            &existing(&n.iter().map(String::as_str).collect::<Vec<_>>()),
            &rule,
        );
        assert_eq!(plans[0].to, "img.jpg");
        assert_eq!(plans[1].to, "img.jpg");
        assert_eq!(plans[0].status, PlanStatus::Collision);
        assert_eq!(plans[1].status, PlanStatus::Collision);
        assert!(!plan_is_applicable(&plans));
    }

    #[test]
    fn collision_with_an_untouched_sibling_is_flagged() {
        let rule = RenameRule {
            find: "draft".into(),
            replace: "final".into(),
            ..Default::default()
        };
        // "final.txt" already exists and is NOT in the rename batch.
        let n = names(&["draft.txt"]);
        let ex = existing(&["draft.txt", "final.txt"]);
        let plans = plan_batch_rename(&n, &ex, &rule);
        assert_eq!(plans[0].status, PlanStatus::Collision);
    }

    #[test]
    fn vacate_guard_suppresses_sibling_check_not_duplicate_check() {
        // "a.txt" -> "b.txt"; the existing "b.txt" is a collision only when it
        // is NOT part of the batch. When "b.txt" is also selected (and so will
        // be vacated), the guard lets the rename through.
        let rule = RenameRule {
            find: "a".into(),
            replace: "b".into(),
            ..Default::default()
        };
        let ex = existing(&["a.txt", "b.txt"]);
        // b.txt not in batch -> collision with an untouched sibling.
        assert_eq!(
            plan_batch_rename(&names(&["a.txt"]), &ex, &rule)[0].status,
            PlanStatus::Collision
        );
        // b.txt also in the batch -> a.txt's target is no longer a sibling
        // clash. (b.txt has no 'a', so it stays put and is Unchanged.)
        let plans = plan_batch_rename(&names(&["a.txt", "b.txt"]), &ex, &rule);
        assert_eq!(plans[0].to, "b.txt");
        assert_eq!(plans[1].status, PlanStatus::Unchanged);
        // With b.txt staying, both want "b.txt" -> the duplicate-target rule
        // still flags a.txt. The vacate guard only suppresses the *sibling*
        // check, not genuine duplicates.
        assert_eq!(plans[0].status, PlanStatus::Collision);
    }

    #[test]
    fn invalid_target_names_are_flagged() {
        let rule = RenameRule {
            find: "a".into(),
            replace: "/".into(),
            ..Default::default()
        };
        let plans = plan_batch_rename(&names(&["a"]), &existing(&["a"]), &rule);
        assert_eq!(plans[0].status, PlanStatus::Invalid);
        assert!(!plan_is_applicable(&plans));
    }

    #[test]
    fn applicable_requires_a_change_and_no_problems() {
        // All unchanged -> not applicable (nothing to do).
        let noop = plan_batch_rename(
            &names(&["a.txt"]),
            &existing(&["a.txt"]),
            &RenameRule::default(),
        );
        assert!(!plan_is_applicable(&noop));
        // One clean change -> applicable.
        let rule = RenameRule {
            prefix: "p_".into(),
            ..Default::default()
        };
        let ok = plan_batch_rename(&names(&["a.txt"]), &existing(&["a.txt"]), &rule);
        assert!(plan_is_applicable(&ok));
        assert_eq!(ok.iter().filter(|p| p.status == PlanStatus::Ok).count(), 1);
    }
}
