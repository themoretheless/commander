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

/// Name order as the user configured it: natural ("file2" before "file10")
/// or plain A-Z. Every other column breaks its ties with this, always
/// ascending whatever the sort direction, so equal keys read alphabetically
/// ("alpha" before "Zeta") instead of in byte path order.
fn name_cmp(a: &FileEntry, b: &FileEntry, config: ViewConfig) -> Ordering {
    if config.natural_name_sort() {
        natural_cmp(&a.name_lower, &b.name_lower)
    } else {
        a.name_lower.cmp(&b.name_lower)
    }
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
            SortColumn::Name => name_cmp(a, b, config),
            SortColumn::Size => a.size.cmp(&b.size),
            SortColumn::Modified => a.modified.cmp(&b.modified),
            SortColumn::Extension => a.extension.cmp(&b.extension),
            SortColumn::Kind => {
                crate::selection_summary::kind_of(a).cmp(&crate::selection_summary::kind_of(b))
            }
        };
        let ordered = match config.sort_order() {
            SortOrder::Asc => primary,
            SortOrder::Desc => primary.reverse(),
        };
        // Name is the primary key of its own column; elsewhere it is an
        // ascending tie-break that does not flip with the direction.
        ordered
            .then_with(|| name_cmp(a, b, config))
            .then_with(|| a.path.cmp(&b.path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, is_dir: bool, size: u64) -> FileEntry {
        FileEntry {
            permissions: None,
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: std::path::PathBuf::from(format!("/t/{name}")),
            identity: super::super::ListingIdentity::Unavailable,
            is_dir,
            size,
            extension: name
                .rsplit_once('.')
                .map(|(_, e)| e.to_string())
                .unwrap_or_default(),
            modified: None,
            modified_str: String::new(),
            size_str: String::new(),
        }
    }

    fn sorted(mut entries: Vec<FileEntry>, config: ViewConfig) -> Vec<String> {
        sort_entries(&mut entries, config);
        entries.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn equal_keys_break_ties_by_name_not_byte_path() {
        let config = ViewConfig::default().with_sort(SortColumn::Size, SortOrder::Asc);
        let names = sorted(
            vec![
                named("Zeta", true, 0),
                named("alpha", true, 0),
                named("beta", true, 0),
            ],
            config,
        );
        assert_eq!(names, ["alpha", "beta", "Zeta"]);
    }

    #[test]
    fn descending_sorts_keep_ties_in_ascending_name_order() {
        let config = ViewConfig::default().with_sort(SortColumn::Size, SortOrder::Desc);
        let names = sorted(
            vec![
                named("Zeta", false, 1),
                named("alpha", false, 1),
                named("big", false, 9),
            ],
            config,
        );
        assert_eq!(names, ["big", "alpha", "Zeta"]);

        let by_name = ViewConfig::default().with_sort(SortColumn::Name, SortOrder::Desc);
        let names = sorted(
            vec![named("alpha", false, 1), named("Zeta", false, 1)],
            by_name,
        );
        assert_eq!(names, ["Zeta", "alpha"]);
    }

    #[test]
    fn extension_ties_follow_the_natural_sort_setting() {
        let entries = || vec![named("f10.txt", false, 1), named("f2.txt", false, 1)];
        let natural = ViewConfig::default().with_sort(SortColumn::Extension, SortOrder::Asc);
        assert!(natural.natural_name_sort());
        assert_eq!(sorted(entries(), natural), ["f2.txt", "f10.txt"]);
        let plain = natural.with_natural_name_sort(false);
        assert_eq!(sorted(entries(), plain), ["f10.txt", "f2.txt"]);
    }

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

    #[test]
    fn natural_digit_runs_longer_than_u64_and_all_zero_runs() {
        assert_eq!(
            natural_cmp("f99999999999999999999", "f100000000000000000000"),
            Ordering::Less
        );
        assert_eq!(
            natural_cmp("f100000000000000000001", "f100000000000000000000"),
            Ordering::Greater
        );
        // All-zero runs are the value 0; the shorter run sorts first.
        assert_eq!(natural_cmp("a0", "a00"), Ordering::Less);
        assert_eq!(natural_cmp("a00", "a0"), Ordering::Greater);
        assert_eq!(natural_cmp("a00", "a1"), Ordering::Less);
    }

    #[test]
    fn natural_prefixes_and_digits_before_letters() {
        assert_eq!(natural_cmp("file", "file1"), Ordering::Less);
        assert_eq!(natural_cmp("a1b", "a"), Ordering::Greater);
        assert_eq!(natural_cmp("1a", "a"), Ordering::Less);
        assert_eq!(natural_cmp("a", "1a"), Ordering::Greater);
    }

    #[test]
    fn modified_sort_puts_unknown_dates_first_ascending_and_last_descending() {
        use std::time::{Duration, UNIX_EPOCH};
        let dated = |name: &str, secs: Option<u64>| {
            let mut entry = named(name, false, 1);
            entry.modified = secs.map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
            entry
        };
        let entries = || {
            vec![
                dated("new", Some(300)),
                dated("unknown", None),
                dated("old", Some(100)),
            ]
        };
        let asc = ViewConfig::default().with_sort(SortColumn::Modified, SortOrder::Asc);
        assert_eq!(sorted(entries(), asc), ["unknown", "old", "new"]);
        let desc = ViewConfig::default().with_sort(SortColumn::Modified, SortOrder::Desc);
        assert_eq!(sorted(entries(), desc), ["new", "old", "unknown"]);
    }
}
