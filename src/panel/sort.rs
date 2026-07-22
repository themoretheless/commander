//! Sorting logic for panel entries.
//! SRP: natural sort, column/order state, sort methods extracted.
//! DRY from main panel.rs. Matches patterns in other file managers (e.g. Total Commander natural sort, mc sorting).

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

use super::FileEntry;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SortColumn {
    Name,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// Natural ("human") ordering: runs of digits compare by numeric value, so
/// "file2" sorts before "file10". Non-digit runs compare by char. Inputs are
/// expected pre-lowercased (we sort on `name_lower`).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        let (ca, cb) = (a[i], b[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let si = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            // Compare by numeric value: drop leading zeros, then longer run
            // wins, then lexically; finally fewer leading zeros sorts first.
            let va = strip_leading_zeros(&a[si..i]);
            let vb = strip_leading_zeros(&b[sj..j]);
            let ord = va
                .len()
                .cmp(&vb.len())
                .then_with(|| va.iter().cmp(vb.iter()))
                .then_with(|| (i - si).cmp(&(j - sj)));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            match ca.cmp(&cb) {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                ord => return ord,
            }
        }
    }
    // One ran out: the shorter string sorts first.
    (a.len() - i).cmp(&(b.len() - j))
}

fn strip_leading_zeros(s: &[char]) -> &[char] {
    let mut k = 0;
    while k + 1 < s.len() && s[k] == '0' {
        k += 1;
    }
    &s[k..]
}

/// Sort the panel's entries in place using current sort_col and sort_order.
/// Dirs always first. Updates entries_gen.
pub fn sort_entries(panel: &mut super::PanelState) {
    let col = panel.sort_col;
    let order = panel.sort_order;

    panel.entries.sort_by(|a, b| {
        // Dirs always first
        match (a.is_dir, b.is_dir) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }

        let cmp = match col {
            // Natural order over the precomputed lowercase name, so
            // "file2" sorts before "file10".
            SortColumn::Name => natural_cmp(&a.name_lower, &b.name_lower),
            SortColumn::Size => a.size.cmp(&b.size),
            SortColumn::Modified => a.modified.cmp(&b.modified),
        };

        match order {
            SortOrder::Asc => cmp,
            SortOrder::Desc => cmp.reverse(),
        }
    });
    // Content/order changed: filtered indices must be rebuilt.
    panel.entries_gen = panel.entries_gen.wrapping_add(1);
}

pub fn set_sort(panel: &mut super::PanelState, col: SortColumn) {
    if panel.sort_col == col {
        panel.sort_order = match panel.sort_order {
            SortOrder::Asc => SortOrder::Desc,
            SortOrder::Desc => SortOrder::Asc,
        };
    } else {
        panel.sort_col = col;
        panel.sort_order = SortOrder::Asc;
    }
    sort_entries(panel);
}

pub fn sort_indicator(panel: &super::PanelState, col: SortColumn) -> &str {
    if panel.sort_col == col {
        match panel.sort_order {
            SortOrder::Asc => " ▲",
            SortOrder::Desc => " ▼",
        }
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn natural_cmp_orders_numbers_by_value() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("img9", "img09"), Ordering::Less);
        assert_eq!(natural_cmp("v1.2", "v1.10"), Ordering::Less);
    }
}