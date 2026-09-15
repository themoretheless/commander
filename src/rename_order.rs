//! Safe execution order for a batch rename.
//!
//! The batch-rename studio ([`crate::rename`]) decides WHAT each file becomes
//! but only flags collisions and refuses the batch; it computes no execution
//! order, so it cannot perform swaps, rotations, or the common case-only
//! `Foo` -> `foo` rename (a no-op on a case-insensitive volume unless staged
//! through a temporary). This module turns an `(old, new)` map into an ordered,
//! mid-batch-safe sequence of single renames.
//!
//! Pure and standalone: [`safe_rename_order`] computes the plan and
//! [`apply_steps`] executes it through an injected rename, so the ordering and
//! rollback are unit-tested without touching disk while the studio runs it for
//! real.

use std::collections::{HashMap, HashSet};

/// One atomic rename. A cycle (a->b->a, longer rotations, or a case-only
/// self-rename) is broken by staging one member aside with [`ToTemp`], draining
/// the rest as [`Direct`]s, then moving the staged member into place with
/// [`FromTemp`]. The two temp halves deliberately bracket the cycle's other
/// steps, which a single bundled "via temp" step could not express.
///
/// [`Direct`]: RenameStep::Direct
/// [`ToTemp`]: RenameStep::ToTemp
/// [`FromTemp`]: RenameStep::FromTemp
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RenameStep {
    /// Rename `from` to `to`. `to` is guaranteed free at this point.
    Direct { from: String, to: String },
    /// Stage `from` aside to the scratch name `tmp` (first half of a cycle break).
    ToTemp { from: String, tmp: String },
    /// Move a previously-staged scratch name into its final place.
    FromTemp { tmp: String, to: String },
}

impl RenameStep {
    /// The `(source, destination)` names this step renames, forward.
    fn endpoints(&self) -> (&str, &str) {
        match self {
            RenameStep::Direct { from, to } => (from, to),
            RenameStep::ToTemp { from, tmp } => (from, tmp),
            RenameStep::FromTemp { tmp, to } => (tmp, to),
        }
    }
}

#[derive(Debug)]
pub struct RenameRollbackFailure<E> {
    pub from: String,
    pub to: String,
    pub error: E,
}

/// A failed forward rename together with every reverse step that could not be
/// completed. Keeping both error layers is essential: the original failure
/// explains why the batch stopped, while rollback failures identify names
/// that may still be at an intermediate location.
#[derive(Debug)]
pub struct ApplyStepsError<E> {
    pub failed_from: String,
    pub failed_to: String,
    pub operation: E,
    pub rollback_failures: Vec<RenameRollbackFailure<E>>,
}

impl<E: std::fmt::Display> std::fmt::Display for ApplyStepsError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "rename {} to {} failed: {}",
            self.failed_from, self.failed_to, self.operation
        )?;
        for failure in &self.rollback_failures {
            write!(
                formatter,
                "; rollback {} to {} failed: {}",
                failure.from, failure.to, failure.error
            )?;
        }
        Ok(())
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ApplyStepsError<E> {}

/// Execute an ordered plan via an injected `rename(from, to)` (so the same
/// logic drives `std::fs::rename` in the app and an in-memory map in tests).
/// On the first failure, the already-applied steps are reversed best-effort
/// (newest first), so a partial OS error never strands a file at a temp name.
/// Returns the number of entries landed in their final place on success.
pub fn apply_steps<E>(
    steps: &[RenameStep],
    mut rename: impl FnMut(&str, &str) -> Result<(), E>,
) -> Result<usize, ApplyStepsError<E>> {
    for (i, step) in steps.iter().enumerate() {
        let (from, to) = step.endpoints();
        if let Err(operation) = rename(from, to) {
            let mut rollback_failures = Vec::new();
            for prev in steps[..i].iter().rev() {
                let (pf, pt) = prev.endpoints();
                if let Err(error) = rename(pt, pf) {
                    rollback_failures.push(RenameRollbackFailure {
                        from: pt.to_string(),
                        to: pf.to_string(),
                        error,
                    });
                }
            }
            return Err(ApplyStepsError {
                failed_from: from.to_string(),
                failed_to: to.to_string(),
                operation,
                rollback_failures,
            });
        }
    }
    // Every step except a bare ToTemp lands a name in its final place; a cycle
    // of k entries is (k-1) Direct + 1 FromTemp, so this is exactly k per cycle.
    Ok(steps
        .iter()
        .filter(|s| !matches!(s, RenameStep::ToTemp { .. }))
        .count())
}

/// The result of ordering a rename batch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RenameOrder {
    /// An ordered sequence of single renames, safe to apply top to bottom: no
    /// step ever overwrites a name still occupied by a not-yet-moved file.
    Steps(Vec<RenameStep>),
    /// The batch cannot be ordered safely (a target collides with an untouched
    /// existing file, or two renames clash on one name). Carries a reason.
    Conflict(String),
}

fn key(s: &str) -> String {
    s.to_lowercase()
}

/// Order `map` (each `(old, new)`) into a mid-batch-safe rename sequence, or
/// report a [`RenameOrder::Conflict`]. `existing` is every name currently in the
/// directory (so a target landing on an untouched sibling is caught).
///
/// Occupancy is compared case-insensitively, matching the default macOS volume,
/// while the steps carry the exact cased names. Exact no-op pairs (`old == new`)
/// are dropped; a case-only change (`Foo` -> `foo`) is kept and routed through a
/// temporary so the case actually changes.
pub fn safe_rename_order(map: &[(String, String)], existing: &HashSet<String>) -> RenameOrder {
    // Drop exact no-ops; keep case-only changes (they still need a temp).
    let pairs: Vec<(String, String)> = map.iter().filter(|(f, t)| f != t).cloned().collect();
    let n = pairs.len();
    if n == 0 {
        return RenameOrder::Steps(Vec::new());
    }

    // Source key -> pair index. Two sources sharing a key cannot both exist in
    // one directory, so treat it as malformed input.
    let mut source_idx: HashMap<String, usize> = HashMap::new();
    for (i, (f, _)) in pairs.iter().enumerate() {
        if source_idx.insert(key(f), i).is_some() {
            return RenameOrder::Conflict(format!(
                "two renames share the source \u{201c}{f}\u{201d}"
            ));
        }
    }
    let source_keys: HashSet<String> = source_idx.keys().cloned().collect();
    let existing_keys: HashSet<String> = existing.iter().map(|s| key(s)).collect();

    // Conflict: two renames want the same target.
    let mut target_count: HashMap<String, usize> = HashMap::new();
    for (_, t) in &pairs {
        *target_count.entry(key(t)).or_insert(0) += 1;
    }
    for (_, t) in &pairs {
        if target_count[&key(t)] > 1 {
            return RenameOrder::Conflict(format!(
                "two renames target the same name \u{201c}{t}\u{201d}"
            ));
        }
    }
    // Conflict: a target lands on a file that exists and is NOT being renamed
    // away. (A target whose name is also a source is fine: that source vacates.)
    for (_, t) in &pairs {
        let tk = key(t);
        if existing_keys.contains(&tk) && !source_keys.contains(&tk) {
            return RenameOrder::Conflict(format!(
                "\u{201c}{t}\u{201d} already exists and is not being renamed"
            ));
        }
    }

    // blocker[i] = the pair whose source currently occupies pair i's target (so
    // pair i must wait for it to vacate). Targets are unique, so each pair is
    // the blocker of at most one other: the dependency graph is a disjoint set
    // of simple paths and simple cycles.
    let blocker: Vec<Option<usize>> = pairs
        .iter()
        .map(|(_, t)| source_idx.get(&key(t)).copied())
        .collect();
    // succ_of[j] = the unique pair blocked by j (reverse of `blocker`).
    let mut succ_of: HashMap<usize, usize> = HashMap::new();
    for (i, b) in blocker.iter().enumerate() {
        if let Some(j) = b {
            succ_of.insert(*j, i);
        }
    }

    let mut emitted = vec![false; n];
    let mut steps: Vec<RenameStep> = Vec::new();

    // Drain every path: emit a pair as soon as its target is free (no blocker)
    // or its blocker has already vacated. Self-cycles (case-only) never qualify.
    loop {
        let mut progress = false;
        for i in 0..n {
            if emitted[i] {
                continue;
            }
            let ready = match blocker[i] {
                None => true,
                Some(j) if j != i => emitted[j],
                Some(_) => false,
            };
            if ready {
                emitted[i] = true;
                steps.push(RenameStep::Direct {
                    from: pairs[i].0.clone(),
                    to: pairs[i].1.clone(),
                });
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }

    // Whatever remains is a set of simple cycles; break each with one temp.
    // Occupancy for temps is the same case-folded set used for conflict checks:
    // never mix original-case names into this set (that was the D8 mismatch).
    let mut reserved: HashSet<String> = existing_keys.clone();
    for (f, t) in &pairs {
        reserved.insert(key(f));
        reserved.insert(key(t));
    }
    let mut tmp_seq = 0;
    for start in 0..n {
        if emitted[start] {
            continue;
        }
        let tmp = mint_temp_name(&mut tmp_seq, &mut reserved);

        steps.push(RenameStep::ToTemp {
            from: pairs[start].0.clone(),
            tmp: tmp.clone(),
        });
        emitted[start] = true;
        // Drain the rest of the cycle now that `start`'s source has vacated.
        let mut node = succ_of.get(&start).copied();
        while let Some(cur) = node {
            if cur == start {
                break;
            }
            steps.push(RenameStep::Direct {
                from: pairs[cur].0.clone(),
                to: pairs[cur].1.clone(),
            });
            emitted[cur] = true;
            node = succ_of.get(&cur).copied();
        }
        steps.push(RenameStep::FromTemp {
            tmp,
            to: pairs[start].1.clone(),
        });
    }

    RenameOrder::Steps(steps)
}

/// Next free `.cmdr-rename-N` scratch name against a case-folded reserved set.
/// Increments `seq` past every collision so two cycles never share a temp, even
/// when an existing sibling already occupies a mixed-case variant of the base.
fn mint_temp_name(seq: &mut usize, reserved: &mut HashSet<String>) -> String {
    loop {
        let candidate = format!(".cmdr-rename-{seq}");
        *seq += 1;
        let folded = key(&candidate);
        if reserved.insert(folded) {
            return candidate;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    fn existing(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| key(s)).collect()
    }

    fn steps(order: &RenameOrder) -> &[RenameStep] {
        match order {
            RenameOrder::Steps(s) => s,
            RenameOrder::Conflict(why) => panic!("expected Steps, got Conflict: {why}"),
        }
    }

    /// Apply a plan to a case-insensitive name set, asserting no step ever
    /// lands on a name still occupied. Returns the final set of (lowercased)
    /// names.
    fn simulate(order: &RenameOrder, present: &[&str]) -> HashSet<String> {
        let mut set: HashSet<String> = present.iter().map(|s| s.to_lowercase()).collect();
        for step in steps(order) {
            match step {
                RenameStep::Direct { from, to } => {
                    let (f, t) = (from.to_lowercase(), to.to_lowercase());
                    assert!(set.contains(&f), "Direct source {from} missing");
                    assert!(!set.contains(&t), "Direct clobbers occupied {to}");
                    set.remove(&f);
                    set.insert(t);
                }
                RenameStep::ToTemp { from, tmp } => {
                    let (f, t) = (from.to_lowercase(), tmp.to_lowercase());
                    assert!(set.contains(&f), "ToTemp source {from} missing");
                    assert!(!set.contains(&t), "ToTemp clobbers occupied {tmp}");
                    set.remove(&f);
                    set.insert(t);
                }
                RenameStep::FromTemp { tmp, to } => {
                    let (t, d) = (tmp.to_lowercase(), to.to_lowercase());
                    assert!(set.contains(&t), "FromTemp temp {tmp} missing");
                    assert!(!set.contains(&d), "FromTemp clobbers occupied {to}");
                    set.remove(&t);
                    set.insert(d);
                }
            }
        }
        set
    }

    fn expected_final(present: &[&str], pairs: &[(&str, &str)]) -> HashSet<String> {
        let sources: HashSet<String> = pairs.iter().map(|(f, _)| f.to_lowercase()).collect();
        let mut set: HashSet<String> = present
            .iter()
            .map(|s| s.to_lowercase())
            .filter(|k| !sources.contains(k))
            .collect();
        for (_, t) in pairs {
            set.insert(t.to_lowercase());
        }
        set
    }

    #[test]
    fn independent_renames_are_all_direct() {
        let m = map(&[("a.txt", "x.txt"), ("b.txt", "y.txt")]);
        let order = safe_rename_order(&m, &existing(&["a.txt", "b.txt"]));
        let s = steps(&order);
        assert!(s.iter().all(|st| matches!(st, RenameStep::Direct { .. })));
        assert_eq!(s.len(), 2);
        assert_eq!(
            simulate(&order, &["a.txt", "b.txt"]),
            expected_final(
                &["a.txt", "b.txt"],
                &[("a.txt", "x.txt"), ("b.txt", "y.txt")]
            )
        );
    }

    #[test]
    fn dependent_chain_is_ordered_vacate_first() {
        // a->b needs b to vacate first (b->c). Order must put b->c before a->b.
        let pairs = [("a", "b"), ("b", "c")];
        let order = safe_rename_order(&map(&pairs), &existing(&["a", "b"]));
        let s = steps(&order);
        let pos = |from: &str| {
            s.iter()
                .position(|st| matches!(st, RenameStep::Direct { from: f, .. } if f == from))
                .unwrap()
        };
        assert!(pos("b") < pos("a"), "b->c must run before a->b");
        assert_eq!(
            simulate(&order, &["a", "b"]),
            expected_final(&["a", "b"], &pairs)
        );
    }

    #[test]
    fn two_cycle_is_broken_with_a_temp() {
        let pairs = [("a", "b"), ("b", "a")];
        let order = safe_rename_order(&map(&pairs), &existing(&["a", "b"]));
        let s = steps(&order);
        assert!(s.iter().any(|st| matches!(st, RenameStep::ToTemp { .. })));
        assert!(s.iter().any(|st| matches!(st, RenameStep::FromTemp { .. })));
        assert_eq!(
            simulate(&order, &["a", "b"]),
            expected_final(&["a", "b"], &pairs) // {a, b} swapped -> still {a, b}
        );
    }

    #[test]
    fn three_cycle_rotation_resolves() {
        let pairs = [("a", "b"), ("b", "c"), ("c", "a")];
        let order = safe_rename_order(&map(&pairs), &existing(&["a", "b", "c"]));
        // Exactly one temp pair brackets the rotation.
        let s = steps(&order);
        assert_eq!(
            s.iter()
                .filter(|st| matches!(st, RenameStep::ToTemp { .. }))
                .count(),
            1
        );
        assert_eq!(
            simulate(&order, &["a", "b", "c"]),
            expected_final(&["a", "b", "c"], &pairs)
        );
    }

    #[test]
    fn case_only_rename_goes_through_a_temp() {
        // Foo -> foo: a self-cycle on a case-insensitive volume; must stage.
        let pairs = [("Foo", "foo")];
        let order = safe_rename_order(&map(&pairs), &existing(&["Foo"]));
        let s = steps(&order);
        assert!(matches!(s[0], RenameStep::ToTemp { .. }));
        assert!(matches!(s[s.len() - 1], RenameStep::FromTemp { .. }));
        // Final set is the lowercased name.
        let final_set = simulate(&order, &["Foo"]);
        assert_eq!(final_set, existing(&["foo"]));
    }

    #[test]
    fn target_on_an_untouched_sibling_is_a_conflict() {
        // final.txt exists and is not being renamed away.
        let order = safe_rename_order(
            &map(&[("draft.txt", "final.txt")]),
            &existing(&["draft.txt", "final.txt"]),
        );
        assert!(matches!(order, RenameOrder::Conflict(_)));
    }

    #[test]
    fn target_on_a_sibling_being_vacated_is_not_a_conflict() {
        // b is also being renamed (b->c), so a->b resolves by ordering.
        let pairs = [("a", "b"), ("b", "c")];
        let order = safe_rename_order(&map(&pairs), &existing(&["a", "b"]));
        assert!(matches!(order, RenameOrder::Steps(_)));
    }

    #[test]
    fn duplicate_targets_are_a_conflict() {
        let order = safe_rename_order(&map(&[("a", "x"), ("b", "x")]), &existing(&["a", "b"]));
        assert!(matches!(order, RenameOrder::Conflict(_)));
    }

    #[test]
    fn no_op_pairs_are_dropped() {
        let order = safe_rename_order(&map(&[("a", "a")]), &existing(&["a"]));
        assert_eq!(order, RenameOrder::Steps(Vec::new()));
    }

    #[test]
    fn mixed_batch_directs_and_a_cycle_never_clobbers() {
        // One independent rename, one chain, and one 2-cycle, all at once.
        let pairs = [
            ("free.txt", "moved.txt"), // independent
            ("a", "b"),                // chain head (needs b to vacate)
            ("b", "c"),                // chain tail
            ("p", "q"),                // 2-cycle
            ("q", "p"),
        ];
        let present = ["free.txt", "a", "b", "p", "q"];
        let order = safe_rename_order(&map(&pairs), &existing(&present));
        assert_eq!(simulate(&order, &present), expected_final(&present, &pairs));
    }

    #[test]
    fn apply_steps_executes_a_swap_through_a_temp() {
        let pairs = [("a", "b"), ("b", "a")];
        let order = safe_rename_order(&map(&pairs), &existing(&["a", "b"]));
        // Track file contents so we can prove the swap actually happened.
        let mut fs: HashMap<String, String> = HashMap::from([
            ("a".to_string(), "A".to_string()),
            ("b".to_string(), "B".to_string()),
        ]);
        let done = apply_steps(steps(&order), |from, to| {
            let v = fs.remove(from).ok_or("missing source")?;
            fs.insert(to.to_string(), v);
            Ok::<(), &str>(())
        })
        .unwrap();
        assert_eq!(done, 2);
        assert_eq!(
            fs.get("a"),
            Some(&"B".to_string()),
            "a now holds b's content"
        );
        assert_eq!(
            fs.get("b"),
            Some(&"A".to_string()),
            "b now holds a's content"
        );
        assert_eq!(fs.len(), 2, "no temp left behind");
    }

    #[test]
    fn apply_steps_rolls_back_on_failure() {
        // Two independent renames; fail the second and assert the first reverts.
        let pairs = [("a", "x"), ("b", "y")];
        let order = safe_rename_order(&map(&pairs), &existing(&["a", "b"]));
        let mut fs: HashSet<String> = existing(&["a", "b"]);
        let res = apply_steps(steps(&order), |from, to| {
            if to == "y" {
                return Err("boom");
            }
            assert!(fs.remove(from));
            fs.insert(to.to_string());
            Ok::<(), &str>(())
        });
        assert!(res.is_err());
        // a->x was applied then rolled back; nothing landed.
        assert_eq!(fs, existing(&["a", "b"]));
    }

    #[test]
    fn apply_steps_reports_a_failed_rollback_without_hiding_the_original_error() {
        let pairs = [("a", "x"), ("b", "y")];
        let order = safe_rename_order(&map(&pairs), &existing(&["a", "b"]));
        let mut fs: HashSet<String> = existing(&["a", "b"]);

        let error = apply_steps(steps(&order), |from, to| {
            if (from, to) == ("b", "y") {
                return Err("forward failure");
            }
            if (from, to) == ("x", "a") {
                return Err("rollback failure");
            }
            assert!(fs.remove(from));
            fs.insert(to.to_string());
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.operation, "forward failure");
        assert_eq!(error.rollback_failures.len(), 1);
        assert_eq!(error.rollback_failures[0].from, "x");
        assert_eq!(error.rollback_failures[0].to, "a");
        assert_eq!(error.rollback_failures[0].error, "rollback failure");
        assert_eq!(fs, existing(&["x", "b"]));
    }

    #[test]
    fn temp_name_avoids_a_collision_with_an_existing_scratch() {
        // The first scratch base is already taken on disk; the minted temp must
        // dodge it, and the plan must still apply cleanly.
        let pairs = [("a", "b"), ("b", "a")];
        let order = safe_rename_order(&map(&pairs), &existing(&["a", "b", ".cmdr-rename-0"]));
        let s = steps(&order);
        if let Some(RenameStep::ToTemp { tmp, .. }) =
            s.iter().find(|st| matches!(st, RenameStep::ToTemp { .. }))
        {
            assert_ne!(
                tmp, ".cmdr-rename-0",
                "temp must dodge the existing scratch"
            );
            assert_eq!(tmp, ".cmdr-rename-1");
        } else {
            panic!("expected a ToTemp step");
        }
        // The untouched scratch file survives.
        let mut final_set = expected_final(&["a", "b"], &pairs);
        final_set.insert(".cmdr-rename-0".to_string());
        assert_eq!(simulate(&order, &["a", "b", ".cmdr-rename-0"]), final_set);
    }

    #[test]
    fn temp_name_dodges_a_case_variant_of_an_existing_scratch() {
        // Reserved occupancy is case-folded; a differently-cased sibling must
        // still block the same temp base on a case-insensitive volume.
        let pairs = [("a", "b"), ("b", "a")];
        let order = safe_rename_order(&map(&pairs), &existing(&["a", "b", ".CMDR-RENAME-0"]));
        let s = steps(&order);
        let RenameStep::ToTemp { tmp, .. } = s
            .iter()
            .find(|st| matches!(st, RenameStep::ToTemp { .. }))
            .expect("expected a ToTemp step")
        else {
            unreachable!();
        };
        assert_eq!(tmp, ".cmdr-rename-1");
        assert_ne!(key(tmp), key(".CMDR-RENAME-0"));
    }

    #[test]
    fn mint_temp_name_skips_every_case_folded_collision() {
        let mut reserved = existing(&[".cmdr-rename-0", ".CMDR-RENAME-1"]);
        let mut seq = 0;
        assert_eq!(mint_temp_name(&mut seq, &mut reserved), ".cmdr-rename-2");
        assert!(reserved.contains(&key(".cmdr-rename-2")));
    }
}
