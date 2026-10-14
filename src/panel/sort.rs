use std::cmp::Ordering;

use super::{FileEntry, SortColumn, SortOrder, ViewConfig};

/// Natural ("human") ordering: runs of digits compare by numeric value, so
/// "file2" sorts before "file10". Inputs are expected pre-lowercased.
///
/// Works on UTF-8 bytes so sorting a listing does not allocate per
/// comparison: digits are ASCII and never occur inside a multi-byte
/// character, and byte order within UTF-8 matches code point order.
pub(super) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    // Leading-zero count only breaks a tie once the rest of the names compare
    // equal, so "a01b" still sorts before "a1c".
    let mut zero_tiebreak = Ordering::Equal;
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
            let ordering = va.len().cmp(&vb.len()).then_with(|| va.cmp(vb));
            if ordering != Ordering::Equal {
                return ordering;
            }
            if zero_tiebreak == Ordering::Equal {
                zero_tiebreak = (i - si).cmp(&(j - sj));
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
    // One side is exhausted: the longer remainder sorts last.
    (a.len() - i).cmp(&(b.len() - j)).then(zero_tiebreak)
}

fn strip_leading_zeros(run: &[u8]) -> &[u8] {
    let mut first = 0;
    while first + 1 < run.len() && run[first] == b'0' {
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
        // The zero-count tie-break never overrides a later difference.
        assert_eq!(natural_cmp("a01b", "a1c"), Ordering::Less);
        assert_eq!(natural_cmp("a1c", "a01b"), Ordering::Greater);
        assert_eq!(natural_cmp("a01b", "a1b"), Ordering::Greater);
    }

    #[test]
    fn natural_order_of_non_ascii_names_follows_code_points() {
        // Byte-wise comparison must agree with comparing characters.
        assert_eq!(natural_cmp("é", "z"), Ordering::Greater);
        assert_eq!(natural_cmp("z", "é"), Ordering::Less);
        assert_eq!(natural_cmp("ä", "é"), Ordering::Less);
        assert_eq!(natural_cmp("日本2", "日本10"), Ordering::Less);
        assert_eq!(natural_cmp("é2", "é"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
        assert_eq!(natural_cmp("1é", "1a"), Ordering::Greater);
        let mut names = vec!["ü10", "ü9", "u10", "日1", "a"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["a", "u10", "ü9", "ü10", "日1"]);
    }
}
