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

/// Above this many (query x candidate) cells, `score` keeps the greedy
/// alignment instead of searching for the best one.
const MAX_ALIGNMENT_CELLS: usize = 1 << 16;

const fn is_separator(c: char) -> bool {
    matches!(c, '/' | '\\' | '_' | '-' | ' ' | '.')
}

/// Case-fold one char for matching. Full Unicode lowercase (so `ФАЙЛ` matches
/// `файл` and `É` matches `é`), keeping one char per char so matched indices
/// still line up with the candidate.
fn fold(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// Whether `query` is a case-insensitive subsequence of `candidate`. Allocates
/// nothing and computes no score, for the filter hot path that runs over every
/// entry. An empty (or whitespace-only) query matches everything.
pub fn is_match(query: &str, candidate: &str) -> bool {
    let mut q = query.trim().chars().map(fold).peekable();
    if q.peek().is_none() {
        return true;
    }
    for c in candidate.chars() {
        match q.peek() {
            Some(&qc) => {
                if fold(c) == qc {
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
    let q: Vec<char> = query.trim().chars().map(fold).collect();
    if q.is_empty() {
        return Some(MatchScore {
            score: 0,
            matched_ranges: Vec::new(),
        });
    }
    let cand: Vec<char> = candidate.chars().collect();
    let folded: Vec<char> = cand.iter().map(|&c| fold(c)).collect();

    // Greedy left-to-right subsequence match: a cheap rejection test, and the
    // alignment used when the candidate is too long for the full search.
    let mut qi = 0;
    let mut greedy: Vec<usize> = Vec::with_capacity(q.len());
    for (i, &c) in folded.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            greedy.push(i);
            qi += 1;
        }
    }
    if qi != q.len() {
        return None;
    }
    let matched = if q.len() * cand.len() <= MAX_ALIGNMENT_CELLS {
        best_alignment(&q, &cand, &folded)
    } else {
        greedy
    };

    let mut total = -(matched[0] as i32) * LEADING_PENALTY;
    let mut prev: Option<usize> = None;
    for &p in &matched {
        total += position_bonus(&cand, p);
        if let Some(pp) = prev {
            total += transition(pp, p);
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

/// Score for matching a query char at candidate index `p`, ignoring gaps.
fn position_bonus(cand: &[char], p: usize) -> i32 {
    let mut bonus = MATCH_BASE;
    if p == 0 {
        bonus += START_BONUS;
    } else {
        let before = cand[p - 1];
        if is_separator(before) {
            bonus += BOUNDARY_BONUS;
        } else if cand[p].is_uppercase() && before.is_lowercase() {
            bonus += CAMEL_BONUS;
        }
    }
    bonus
}

/// Score for moving from a match at `pp` to the next one at `p`.
const fn transition(pp: usize, p: usize) -> i32 {
    if p == pp + 1 {
        CONSECUTIVE_BONUS
    } else {
        -((p - pp - 1) as i32) * GAP_PENALTY
    }
}

/// The highest-scoring placement of `q` in `folded` (which must contain it as
/// a subsequence), so `fb` on `fab/foo_bar` takes the word starts f@0 b@8
/// rather than the leftmost b@2. Dynamic programming over (query index,
/// candidate index); ties keep the leftmost placement.
fn best_alignment(q: &[char], cand: &[char], folded: &[char]) -> Vec<usize> {
    let n = folded.len();
    let mut best: Vec<Option<i32>> = vec![None; q.len() * n];
    let mut parent = vec![0_usize; q.len() * n];
    for p in 0..n {
        if folded[p] == q[0] {
            best[p] = Some(position_bonus(cand, p) - p as i32 * LEADING_PENALTY);
        }
    }
    for i in 1..q.len() {
        let (done, rest) = best.split_at_mut(i * n);
        let prev = &done[(i - 1) * n..];
        let row = &mut rest[..n];
        // Best `prev[pp] + pp * GAP_PENALTY` over pp <= p - 2: the gap
        // penalty is linear, so a running maximum covers every gapped pp.
        let mut gapped: Option<(i32, usize)> = None;
        for p in i..n {
            if p >= 2
                && let Some(v) = prev[p - 2]
            {
                let v = v + (p - 2) as i32 * GAP_PENALTY;
                if gapped.is_none_or(|(b, _)| v > b) {
                    gapped = Some((v, p - 2));
                }
            }
            if folded[p] != q[i] {
                continue;
            }
            let mut pick = prev[p - 1].map(|v| (v + transition(p - 1, p), p - 1));
            if let Some((v, pp)) = gapped {
                let v = v - (p - 1) as i32 * GAP_PENALTY;
                if pick.is_none_or(|(b, _)| v > b) {
                    pick = Some((v, pp));
                }
            }
            if let Some((v, pp)) = pick {
                row[p] = Some(v + position_bonus(cand, p));
                parent[i * n + p] = pp;
            }
        }
    }
    let last = (q.len() - 1) * n;
    let mut end = None;
    for p in 0..n {
        if let Some(v) = best[last + p]
            && end.is_none_or(|(b, _)| v > b)
        {
            end = Some((v, p));
        }
    }
    let (_, mut p) = end.expect("caller checked that q is a subsequence");
    let mut matched = vec![p; q.len()];
    for i in (1..q.len()).rev() {
        p = parent[i * n + p];
        matched[i - 1] = p;
    }
    matched
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
    fn folding_covers_non_ascii_letters() {
        assert!(is_match("док", "Документы"));
        assert!(is_match("ФАЙЛ", "файл.txt"));
        assert!(is_match("é", "Éclair"));
        let m = score("ФАЙЛ", "мой файл").unwrap();
        assert_eq!(m.matched_ranges, vec![(4, 8)]);
    }

    #[test]
    fn best_alignment_prefers_word_starts_over_leftmost() {
        // Greedy f@0 b@2 scores 11; f@0 b@8 (after '_') scores 13.
        let m = score("fb", "fab/foo_bar").unwrap();
        assert_eq!(m.score, 13);
        assert_eq!(m.matched_ranges, vec![(0, 1), (8, 9)]);
    }

    #[test]
    fn camel_hump_is_a_boundary() {
        // "sb" matches the S and B humps of "SwapBack".
        let m = score("sb", "SwapBack").unwrap();
        assert_eq!(m.matched_ranges, vec![(0, 1), (4, 5)]);
        assert!(m.score > 0);
    }
}
