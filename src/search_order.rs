//! Presentation-only ordering of retained Find rows; never reads metadata or
//! changes the worker's rank, location ownership or matching result set.
use crate::search::{SearchHit, SearchLocation, TimePivot};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, time::SystemTime};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    #[default]
    Rank,
    Name,
    Path,
    Size,
    Allocated,
    Modified,
    Accessed,
    Created,
    Changed,
    HardLinks,
}
impl Field {
    pub const ALL: [Self; 10] = [
        Self::Rank,
        Self::Name,
        Self::Path,
        Self::Size,
        Self::Allocated,
        Self::Modified,
        Self::Accessed,
        Self::Created,
        Self::Changed,
        Self::HardLinks,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Rank => "Original rank",
            Self::Name => "Name",
            Self::Path => "Full path",
            Self::Size => "Listed size",
            Self::Allocated => "Allocated size",
            Self::Modified => "Modified",
            Self::Accessed => "Accessed",
            Self::Created => "Created",
            Self::Changed => "Metadata changed",
            Self::HardLinks => "Hard-link count",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sort {
    pub field: Field,
    pub descending: bool,
}
fn direction(order: Ordering, descending: bool) -> Ordering {
    if descending { order.reverse() } else { order }
}
fn observed<T: Ord>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => direction(a.cmp(&b), descending),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
/// Date groups always retain their age order; sorting applies within a group.
/// Unknown observations stay last in either direction; ties retain source rank.
pub fn order(hits: &[SearchHit], pivot: TimePivot, sort: Sort, clock: SystemTime) -> Vec<usize> {
    let mut indices: Vec<_> = (0..hits.len()).collect();
    if pivot == TimePivot::None && sort.field == Field::Rank {
        if sort.descending {
            indices.reverse();
        }
        return indices;
    }
    // Compute calendar buckets once rather than in every comparison.
    let buckets: Vec<_> = if pivot == TimePivot::None {
        Vec::new()
    } else {
        hits.iter()
            .map(|h| crate::search::time_bucket(h, pivot, clock))
            .collect()
    };
    indices.sort_by(|&i, &j| {
        let group = if pivot == TimePivot::None {
            Ordering::Equal
        } else {
            buckets[i].cmp(&buckets[j])
        };
        if group != Ordering::Equal {
            return group;
        }
        let (a, b) = (&hits[i], &hits[j]);
        let native = |h: &SearchHit| matches!(h.location, SearchLocation::Native(_));
        let primary = match sort.field {
            Field::Rank => direction(i.cmp(&j), sort.descending),
            Field::Name => direction(
                crate::natural_sort::compare(&a.entry.sort_key, &b.entry.sort_key).then_with(
                    || crate::natural_sort::compare(&a.entry.name_lower, &b.entry.name_lower),
                ),
                sort.descending,
            ),
            Field::Path => direction(a.entry.path.cmp(&b.entry.path), sort.descending),
            Field::Size => direction(a.entry.size.cmp(&b.entry.size), sort.descending),
            Field::Allocated => observed(
                native(a).then(|| a.entry.allocated_bytes()).flatten(),
                native(b).then(|| b.entry.allocated_bytes()).flatten(),
                sort.descending,
            ),
            Field::HardLinks => observed(
                native(a).then(|| a.entry.hard_link_count()).flatten(),
                native(b).then(|| b.entry.hard_link_count()).flatten(),
                sort.descending,
            ),
            Field::Modified => observed(a.entry.modified, b.entry.modified, sort.descending),
            Field::Accessed => observed(a.accessed, b.accessed, sort.descending),
            Field::Created => observed(
                native(a).then_some(a.entry.created).flatten(),
                native(b).then_some(b.entry.created).flatten(),
                sort.descending,
            ),
            Field::Changed => observed(
                native(a).then_some(a.entry.changed).flatten(),
                native(b).then_some(b.entry.changed).flatten(),
                sort.descending,
            ),
        };
        primary.then_with(|| i.cmp(&j))
    });
    indices
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{FileIdentity, MatchExplanation};
    use std::{path::PathBuf, time::Duration};
    fn hit(name: &str, size: u64) -> SearchHit {
        let path = PathBuf::from("/x").join(name);
        SearchHit {
            entry: crate::search::synthetic_entry(path.clone(), false, size, None, "–".into())
                .unwrap(),
            identity: FileIdentity::Path(path.clone()),
            accessed: None,
            explanation: MatchExplanation::default(),
            location: SearchLocation::Native(path),
        }
    }
    fn names(hits: &[SearchHit], pivot: TimePivot, sort: Sort, now: SystemTime) -> Vec<&str> {
        order(hits, pivot, sort, now)
            .into_iter()
            .map(|i| hits[i].entry.name.as_str())
            .collect()
    }
    #[test]
    fn all_fields_keep_exact_values_unknown_last_and_rank_ties_in_both_directions() {
        let mut hits = vec![
            hit("file10", 2),
            hit("file2", 2),
            hit("zero", 0),
            hit("max", u64::MAX),
            hit("unknown", 1),
        ];
        for (i, n) in [2, 2, 0, u64::MAX].into_iter().enumerate() {
            hits[i].entry.allocated_size = Some(n);
            hits[i].entry.hard_links = Some(n);
            let time = SystemTime::UNIX_EPOCH + Duration::from_secs([2, 2, 0, 3][i]);
            hits[i].entry.modified = Some(time);
            hits[i].accessed = Some(time);
            hits[i].entry.created = Some(time);
            hits[i].entry.changed = Some(time);
        }
        hits[4].entry.allocated_size = Some(u64::MAX);
        hits[4].entry.hard_links = Some(u64::MAX);
        hits[4].entry.created = Some(SystemTime::now());
        hits[4].entry.changed = Some(SystemTime::now());
        hits[4].location = SearchLocation::ArchiveMember {
            archive: "/x/a.zip".into(),
            member: "unknown".into(),
            index: 0,
        };
        for field in [
            Field::Allocated,
            Field::HardLinks,
            Field::Modified,
            Field::Accessed,
            Field::Created,
            Field::Changed,
        ] {
            assert_eq!(
                names(
                    &hits,
                    TimePivot::None,
                    Sort {
                        field,
                        descending: false
                    },
                    SystemTime::now()
                ),
                ["zero", "file10", "file2", "max", "unknown"]
            );
            assert_eq!(
                names(
                    &hits,
                    TimePivot::None,
                    Sort {
                        field,
                        descending: true
                    },
                    SystemTime::now()
                ),
                ["max", "file10", "file2", "zero", "unknown"]
            );
        }
        for (field, asc, desc) in [
            (
                Field::Rank,
                vec!["file10", "file2", "zero", "max", "unknown"],
                vec!["unknown", "max", "zero", "file2", "file10"],
            ),
            (
                Field::Name,
                vec!["file2", "file10", "max", "unknown", "zero"],
                vec!["zero", "unknown", "max", "file10", "file2"],
            ),
            (
                Field::Path,
                vec!["file10", "file2", "max", "unknown", "zero"],
                vec!["zero", "unknown", "max", "file2", "file10"],
            ),
            (
                Field::Size,
                vec!["zero", "unknown", "file10", "file2", "max"],
                vec!["max", "file10", "file2", "unknown", "zero"],
            ),
        ] {
            assert_eq!(
                names(
                    &hits,
                    TimePivot::None,
                    Sort {
                        field,
                        descending: false
                    },
                    SystemTime::now()
                ),
                asc
            );
            assert_eq!(
                names(
                    &hits,
                    TimePivot::None,
                    Sort {
                        field,
                        descending: true
                    },
                    SystemTime::now()
                ),
                desc
            );
        }
        for field in Field::ALL {
            for descending in [false, true] {
                let sort = Sort { field, descending };
                assert_eq!(
                    serde_json::from_str::<Sort>(&serde_json::to_string(&sort).unwrap()).unwrap(),
                    sort
                );
            }
        }
    }
    #[test]
    fn age_groups_remain_primary_and_source_rows_never_move() {
        let now = SystemTime::now();
        let mut hits = vec![
            hit("today-large", 7),
            hit("today-small", 1),
            hit("older", u64::MAX),
            hit("unknown", u64::MAX),
        ];
        hits[0].entry.modified = Some(now);
        hits[1].entry.modified = Some(now);
        hits[2].entry.modified = Some(now - Duration::from_secs(400 * 86400));
        let before: Vec<_> = hits.iter().map(|h| h.entry.name.clone()).collect();
        assert_eq!(
            names(
                &hits,
                TimePivot::Modified,
                Sort {
                    field: Field::Size,
                    descending: true
                },
                now
            ),
            ["today-large", "today-small", "older", "unknown"]
        );
        assert_eq!(
            names(
                &hits,
                TimePivot::Modified,
                Sort {
                    field: Field::Size,
                    descending: false
                },
                now
            ),
            ["today-small", "today-large", "older", "unknown"]
        );
        assert_eq!(
            hits.iter()
                .map(|h| h.entry.name.clone())
                .collect::<Vec<_>>(),
            before
        );
    }
    #[cfg(unix)]
    #[test]
    fn full_path_order_retains_non_utf8_distinctions() {
        use std::os::unix::ffi::OsStringExt;
        let mut hits = vec![hit("a", 0), hit("b", 0)];
        hits[0].entry.path = PathBuf::from(std::ffi::OsString::from_vec(b"/x/\xff".to_vec()));
        hits[1].entry.path = PathBuf::from(std::ffi::OsString::from_vec(b"/x/\xfe".to_vec()));
        assert_eq!(
            order(
                &hits,
                TimePivot::None,
                Sort {
                    field: Field::Path,
                    descending: false
                },
                SystemTime::now()
            ),
            [1, 0]
        );
    }
}
