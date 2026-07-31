use std::cmp::Ordering;

use super::{FileEntry, SortColumn, SortOrder, ViewConfig};

/// Natural ("human") ordering: runs of digits compare by numeric value, so
/// "file2" sorts before "file10". Inputs are expected pre-lowercased.
pub(super) fn natural_cmp(a: &str, b: &str) -> Ordering {
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
            let va = strip_leading_zeros(&a[si..i]);
            let vb = strip_leading_zeros(&b[sj..j]);
            let ordering = va
                .len()
                .cmp(&vb.len())
                .then_with(|| va.iter().cmp(vb.iter()))
                .then_with(|| (i - si).cmp(&(j - sj)));
            if ordering != Ordering::Equal {
                return ordering;
            }
        } else {
            match ca.cmp(&cb) {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                ordering => return ordering,
            }
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn strip_leading_zeros(run: &[char]) -> &[char] {
    let mut first = 0;
    while first + 1 < run.len() && run[first] == '0' {
        first += 1;
    }
    &run[first..]
}

pub(super) fn sort_entries(entries: &mut [FileEntry], config: ViewConfig) {
    entries.sort_by(|a, b| {
        if config.folders_first() {
            match (a.is_dir, b.is_dir) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
        }

        let primary = match config.sort_column() {
            SortColumn::Name if config.natural_name_sort() => {
                natural_cmp(&a.name_lower, &b.name_lower)
            }
            SortColumn::Name => a.name_lower.cmp(&b.name_lower),
            SortColumn::Size => a.size.cmp(&b.size),
            SortColumn::Modified => a.modified.cmp(&b.modified),
            SortColumn::Extension => a
                .extension
                .cmp(&b.extension)
                .then_with(|| natural_cmp(&a.name_lower, &b.name_lower)),
            SortColumn::Kind => crate::selection_summary::kind_of(a)
                .cmp(&crate::selection_summary::kind_of(b))
                .then_with(|| natural_cmp(&a.name_lower, &b.name_lower)),
        };
        let ordered = match config.sort_order() {
            SortOrder::Asc => primary,
            SortOrder::Desc => primary.reverse(),
        };
        ordered.then_with(|| a.path.cmp(&b.path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_numbers_are_ordered_by_value_then_leading_zero_count() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("img9", "img09"), Ordering::Less);
        assert_eq!(natural_cmp("v1.2", "v1.10"), Ordering::Less);
    }
}
