//! Cached byte ranges for original text in paired changed lines.
use crate::textdiff::{DiffKind, DiffLine};
use std::{collections::BTreeMap, ops::Range};
const PAIR_CELLS: usize = 65_536;
const TOTAL_CELLS: usize = 2_000_000;

#[derive(Default)]
pub struct Intraline {
    pub enabled: bool,
    pub rows: BTreeMap<usize, Vec<Range<usize>>>,
    pub simplified_pairs: usize,
}
impl Intraline {
    pub fn rebuild(&mut self, lines: &[DiffLine]) {
        self.rows.clear();
        self.simplified_pairs = 0;
        if !self.enabled {
            return;
        }
        let mut budget = TOTAL_CELLS;
        let mut start = 0;
        while start < lines.len() {
            if lines[start].kind == DiffKind::Equal {
                start += 1;
                continue;
            }
            let end = (start..lines.len())
                .find(|&i| lines[i].kind == DiffKind::Equal)
                .unwrap_or(lines.len());
            let deleted = (start..end).filter(|&i| lines[i].kind == DiffKind::Delete);
            let inserted = (start..end).filter(|&i| lines[i].kind == DiffKind::Insert);
            for (a, b) in deleted.zip(inserted) {
                let (minus, plus, simplified) = ranges(&lines[a].text, &lines[b].text, &mut budget);
                self.rows.insert(a, minus);
                self.rows.insert(b, plus);
                self.simplified_pairs += usize::from(simplified);
            }
            start = end;
        }
    }
}
fn push(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    if range.is_empty() {
        return;
    }
    if let Some(last) = ranges.last_mut()
        && last.end == range.start
    {
        last.end = range.end;
    } else {
        ranges.push(range);
    }
}
fn ranges(a: &str, b: &str, budget: &mut usize) -> (Vec<Range<usize>>, Vec<Range<usize>>, bool) {
    let mut prefix = a.bytes().zip(b.bytes()).take_while(|(a, b)| a == b).count();
    while !a.is_char_boundary(prefix) || !b.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let suffix = a[prefix..]
        .chars()
        .rev()
        .zip(b[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let (n, m) = (am.chars().count(), bm.chars().count());
    if n == 0 || m == 0 {
        let mut minus = Vec::new();
        let mut plus = Vec::new();
        push(&mut minus, prefix..a.len() - suffix);
        push(&mut plus, prefix..b.len() - suffix);
        return (minus, plus, false);
    }
    let cells = n.saturating_add(1).saturating_mul(m.saturating_add(1));
    if cells > PAIR_CELLS || cells > *budget {
        let mut minus = Vec::new();
        let mut plus = Vec::new();
        push(&mut minus, prefix..a.len() - suffix);
        push(&mut plus, prefix..b.len() - suffix);
        return (minus, plus, true);
    }
    *budget -= cells;
    let ac: Vec<_> = am.char_indices().collect();
    let bc: Vec<_> = bm.char_indices().collect();
    let cols = m + 1;
    let at = |i: usize, j: usize| i * cols + j;
    let mut dp = vec![0u32; cells];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if ac[i].1 == bc[j].1 {
                dp[at(i + 1, j + 1)] + 1
            } else {
                dp[at(i + 1, j)].max(dp[at(i, j + 1)])
            };
        }
    }
    let (mut minus, mut plus) = (Vec::new(), Vec::new());
    let (mut i, mut j) = (0, 0);
    let add = |out: &mut Vec<Range<usize>>, chars: &[(usize, char)], index: usize| {
        let (byte, ch) = chars[index];
        push(out, prefix + byte..prefix + byte + ch.len_utf8());
    };
    while i < n && j < m {
        if ac[i].1 == bc[j].1 {
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            add(&mut minus, &ac, i);
            i += 1;
        } else {
            add(&mut plus, &bc, j);
            j += 1;
        }
    }
    while i < n {
        add(&mut minus, &ac, i);
        i += 1;
    }
    while j < m {
        add(&mut plus, &bc, j);
        j += 1;
    }
    (minus, plus, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn one(range: Range<usize>) -> Vec<Range<usize>> {
        std::iter::once(range).collect()
    }
    fn compare(a: &str, b: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>, bool) {
        let mut budget = TOTAL_CELLS;
        ranges(a, b, &mut budget)
    }
    #[test]
    fn multiple_unicode_edits_keep_shared_text_and_valid_byte_boundaries() {
        let a = "pre Ж🙂 / old / 12 tail";
        let b = "pre Я🙂 / new / 13 tail";
        let (ar, br, simple) = compare(a, b);
        assert!(!simple);
        let text =
            |s: &str, r: &[Range<usize>]| r.iter().map(|r| &s[r.clone()]).collect::<String>();
        assert_eq!(text(a, &ar), "Жold2");
        assert_eq!(text(b, &br), "Яnew3");
        for (s, rs) in [(a, &ar), (b, &br)] {
            for r in rs {
                assert!(s.is_char_boundary(r.start) && s.is_char_boundary(r.end));
            }
        }
        assert!(ar.len() > 1 && br.len() > 1);
    }
    #[test]
    fn empty_identical_long_and_exhausted_pairs_have_explicit_bounded_fallbacks() {
        assert_eq!(ranges("same", "same", &mut 0), (vec![], vec![], false));
        assert_eq!(compare("", "x"), (vec![], one(0..1), false));
        let a = format!("prefix{}tail", "a".repeat(400));
        let b = format!("prefix{}tail", "b".repeat(400));
        assert_eq!(compare(&a, &b), (one(6..406), one(6..406), true));
        assert_eq!(
            ranges("pre a tail", "pre b tail", &mut 0),
            (one(4..5), one(4..5), true)
        );
        let mut budget = 4;
        assert!(!ranges("a", "b", &mut budget).2);
        assert_eq!(budget, 0);
        assert!(ranges("x", "y", &mut budget).2);
    }
    #[test]
    fn hunk_pairs_never_cross_context_and_unpaired_rows_stay_whole_line() {
        let lines = crate::textdiff::diff_lines(
            "pre old tail\nremoved\ncontext\n",
            "pre new tail\ncontext\nadded\n",
        )
        .unwrap();
        let mut view = Intraline {
            enabled: true,
            ..Default::default()
        };
        view.rebuild(&lines);
        assert_eq!(view.rows.len(), 2);
        assert_eq!(view.rows[&0], one(4..7));
        assert_eq!(view.rows[&2], one(4..7));
        assert!(!view.rows.contains_key(&1));
        assert!(!view.rows.contains_key(&4));
        view.enabled = false;
        view.rebuild(&lines);
        assert!(view.rows.is_empty());
        view.enabled = true;
        view.rebuild(&crate::textdiff::diff_lines("same\n", "same\n").unwrap());
        assert!(view.rows.is_empty());
    }
}
