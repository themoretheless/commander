//! Pure fuzzy subsequence matching and ranking. No egui types, so the scorer
//! and the ranker are unit-tested directly. Shared by the command palette and,
//! later, the quick-filter and quick-jump.

/// The result of matching a query against a candidate: a relevance score and
/// the half-open char-index ranges of the matched characters (for highlight).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MatchScore {
    pub score: i32,
    pub matched_ranges: Vec<(usize, usize)>,
}

const MATCH_BASE: i32 = 1;
const START_BONUS: i32 = 10;
const BOUNDARY_BONUS: i32 = 8;
const CAMEL_BONUS: i32 = 7;
const CONSECUTIVE_BONUS: i32 = 5;
const GAP_PENALTY: i32 = 1;
const LEADING_PENALTY: i32 = 1;

fn is_separator(c: char) -> bool {
    matches!(c, '/' | '\\' | '_' | '-' | ' ' | '.')
}

/// Whether `query` is a case-insensitive subsequence of `candidate`. Allocates
/// nothing and computes no score, for the filter hot path that runs over every
/// entry. An empty (or whitespace-only) query matches everything.
pub fn is_match(query: &str, candidate: &str) -> bool {
    let mut q = query
        .trim()
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .peekable();
    if q.peek().is_none() {
        return true;
    }
    for c in candidate.chars() {
        match q.peek() {
            Some(&qc) => {
                if c.to_ascii_lowercase() == qc {
                    q.next();
                }
            }
            None => return true,
        }
    }
    q.peek().is_none()
}

/// Score `candidate` against `query` with a case-insensitive subsequence match.
/// Returns `None` when `query` is not a subsequence of `candidate`. An empty
/// query matches everything with score 0 (so ranking keeps the input order).
///
/// Bonuses reward consecutive runs, matches at word starts (index 0 or after a
/// separator) and camelCase humps, while gaps and a later first match are
/// penalised, so "fb" ranks "foo/bar" above "fabricate".
pub fn score(query: &str, candidate: &str) -> Option<MatchScore> {
    let q: Vec<char> = query
        .trim()
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if q.is_empty() {
        return Some(MatchScore {
            score: 0,
            matched_ranges: Vec::new(),
        });
    }
    let cand: Vec<char> = candidate.chars().collect();

    // Greedy left-to-right subsequence match, recording matched indices.
    let mut qi = 0;
    let mut matched: Vec<usize> = Vec::with_capacity(q.len());
    for (i, &c) in cand.iter().enumerate() {
        if qi < q.len() && c.to_ascii_lowercase() == q[qi] {
            matched.push(i);
            qi += 1;
        }
    }
    if qi != q.len() {
        return None;
    }

    let mut total = -(matched[0] as i32) * LEADING_PENALTY;
    let mut prev: Option<usize> = None;
    for &p in &matched {
        total += MATCH_BASE;
        if p == 0 {
            total += START_BONUS;
        } else {
            let before = cand[p - 1];
            if is_separator(before) {
                total += BOUNDARY_BONUS;
            } else if cand[p].is_uppercase() && before.is_lowercase() {
                total += CAMEL_BONUS;
            }
        }
        if let Some(pp) = prev {
            if p == pp + 1 {
                total += CONSECUTIVE_BONUS;
            } else {
                total -= (p - pp - 1) as i32 * GAP_PENALTY;
            }
        }
        prev = Some(p);
    }

    // Merge adjacent matched indices into half-open ranges.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &p in &matched {
        match ranges.last_mut() {
            Some(last) if last.1 == p => last.1 = p + 1,
            _ => ranges.push((p, p + 1)),
        }
    }

    Some(MatchScore {
        score: total,
        matched_ranges: ranges,
    })
}

/// An item that matched a query, carrying its score and highlight ranges.
pub struct Ranked<T> {
    pub item: T,
    pub score: i32,
    pub matched_ranges: Vec<(usize, usize)>,
}

/// Keep only the items whose `key` matches `query`, sorted by score descending.
/// The sort is stable, so equal scores (and an empty query) preserve the input
/// order.
pub fn rank<T>(query: &str, items: Vec<T>, key: impl Fn(&T) -> &str) -> Vec<Ranked<T>> {
    let mut scored: Vec<Ranked<T>> = items
        .into_iter()
        .filter_map(|item| {
            let ms = score(query, key(&item))?;
            Some(Ranked {
                score: ms.score,
                matched_ranges: ms.matched_ranges,
                item,
            })
        })
        .collect();
    // Stable sort by score descending; ties keep their input order.
    scored.sort_by_key(|r| std::cmp::Reverse(r.score));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(q: &str, c: &str) -> i32 {
        score(q, c).expect("should match").score
    }

    #[test]
    fn fb_ranks_foo_bar_above_fabricate() {
        assert!(sc("fb", "foo/bar") > sc("fb", "fabricate"));
    }

    #[test]
    fn is_match_agrees_with_score_existence() {
        for (q, c) in [
            ("scn", "scanner.rs"),
            ("fb", "foo/bar"),
            ("", "anything"),
            ("  ", "x"),
            ("xyz", "abc"),
            ("Cargo", "cargo.toml"),
            ("zzz", "zz"),
        ] {
            assert_eq!(
                is_match(q, c),
                score(q, c).is_some(),
                "is_match disagreed with score for ({q:?}, {c:?})"
            );
        }
        assert!(is_match("scn", "scanner.rs"));
        assert!(!is_match("xyz", "abc"));
    }

    #[test]
    fn non_subsequence_returns_none() {
        assert!(score("xyz", "abc").is_none());
        assert!(score("abc", "ab").is_none()); // query longer than any subsequence
    }

    #[test]
    fn empty_query_matches_with_zero_score() {
        let m = score("", "anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.matched_ranges.is_empty());
        // Whitespace-only query is treated as empty.
        assert_eq!(score("   ", "x").unwrap().score, 0);
    }

    #[test]
    fn matched_ranges_cover_matched_chars_merged() {
        // "foobar": f@0, b@3 -> ranges [(0,1),(3,4)].
        let m = score("fb", "foobar").unwrap();
        assert_eq!(m.matched_ranges, vec![(0, 1), (3, 4)]);
        // Consecutive matches merge into one range.
        let m2 = score("foo", "foobar").unwrap();
        assert_eq!(m2.matched_ranges, vec![(0, 3)]);
    }

    #[test]
    fn consecutive_beats_scattered() {
        assert!(sc("ab", "abc") > sc("ab", "axb"));
    }

    #[test]
    fn word_start_beats_mid_word() {
        // 'c' at a word start scores higher than 'c' buried mid-word.
        assert!(sc("c", "cat") > sc("c", "arc"));
        assert!(sc("p", "go/path") > sc("p", "depth"));
    }

    #[test]
    fn camel_hump_is_a_boundary() {
        // "sb" matches the S and B humps of "SwapBack".
        let m = score("sb", "SwapBack").unwrap();
        assert_eq!(m.matched_ranges, vec![(0, 1), (4, 5)]);
        assert!(m.score > 0);
    }

    #[test]
    fn rank_keeps_input_order_for_empty_query() {
        let items = vec!["beta", "alpha", "gamma"];
        let ranked = rank("", items.clone(), |s| s);
        let order: Vec<&str> = ranked.iter().map(|r| r.item).collect();
        assert_eq!(order, items);
    }

    #[test]
    fn rank_filters_and_orders_by_relevance() {
        let items = vec!["fabricate", "foo/bar", "nope"];
        let ranked = rank("fb", items, |s| s);
        let order: Vec<&str> = ranked.iter().map(|r| r.item).collect();
        assert_eq!(order, vec!["foo/bar", "fabricate"]); // "nope" filtered out
    }
}
