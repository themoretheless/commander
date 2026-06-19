//! Pure set algebra over selections. Thin, total wrappers around the std set
//! operations so a multi-pass selection ("everything big, minus videos, plus
//! today's PDFs") can be built and unit-tested without a panel.

use std::collections::HashSet;
use std::path::PathBuf;

/// Members of either set.
pub fn union(a: &HashSet<PathBuf>, b: &HashSet<PathBuf>) -> HashSet<PathBuf> {
    a.union(b).cloned().collect()
}

/// Members of both sets.
pub fn intersect(a: &HashSet<PathBuf>, b: &HashSet<PathBuf>) -> HashSet<PathBuf> {
    a.intersection(b).cloned().collect()
}

/// Members of `a` that are not in `b`.
pub fn difference(a: &HashSet<PathBuf>, b: &HashSet<PathBuf>) -> HashSet<PathBuf> {
    a.difference(b).cloned().collect()
}

/// Members in exactly one of the two sets.
pub fn symmetric_difference(a: &HashSet<PathBuf>, b: &HashSet<PathBuf>) -> HashSet<PathBuf> {
    a.symmetric_difference(b).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn union_is_commutative_with_empty_identity() {
        let a = set(&["a", "b"]);
        let b = set(&["b", "c"]);
        assert_eq!(union(&a, &b), union(&b, &a));
        assert_eq!(union(&a, &b), set(&["a", "b", "c"]));
        assert_eq!(union(&a, &HashSet::new()), a); // empty identity
    }

    #[test]
    fn intersect_is_commutative_and_idempotent() {
        let a = set(&["a", "b", "c"]);
        let b = set(&["b", "c", "d"]);
        assert_eq!(intersect(&a, &b), intersect(&b, &a));
        assert_eq!(intersect(&a, &b), set(&["b", "c"]));
        assert_eq!(intersect(&a, &a), a); // idempotence
    }

    #[test]
    fn difference_removes_and_self_difference_is_empty() {
        let a = set(&["a", "b", "c"]);
        let b = set(&["b"]);
        assert_eq!(difference(&a, &b), set(&["a", "c"]));
        assert!(difference(&a, &a).is_empty()); // a - a == empty
    }

    #[test]
    fn symmetric_difference_self_is_empty_and_mixed_example() {
        let a = set(&["a", "b", "c"]);
        let b = set(&["c", "d"]);
        assert_eq!(symmetric_difference(&a, &b), set(&["a", "b", "d"]));
        assert!(symmetric_difference(&a, &a).is_empty()); // a ^ a == empty
    }
}
