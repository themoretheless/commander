//! Ordering archive rows keeps their listing indexes (and selection identities) stable.
use crate::archive::{ArchiveKind, ArchiveListing};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Field {
    #[default]
    Name,
    Size,
    Packed,
    Ratio,
    Modified,
}

impl Field {
    pub const ALL: [Self; 5] = [
        Self::Name,
        Self::Size,
        Self::Packed,
        Self::Ratio,
        Self::Modified,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Member",
            Self::Size => "Size",
            Self::Packed => "Packed",
            Self::Ratio => "Ratio",
            Self::Modified => "Modified",
        }
    }
    pub fn available(self, kind: ArchiveKind) -> bool {
        kind == ArchiveKind::Zip || !matches!(self, Self::Packed | Self::Ratio)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    pub field: Field,
    pub descending: bool,
    pub folders_first: bool,
    pub natural: bool,
}

impl Default for Order {
    fn default() -> Self {
        Self {
            field: Field::Name,
            descending: false,
            folders_first: false,
            natural: true,
        }
    }
}

impl Order {
    /// Inputs are the cached lowercase display paths; no per-comparison allocation.
    pub fn compare_names(self, a: &str, b: &str) -> std::cmp::Ordering {
        if self.natural {
            crate::natural_sort::compare(a, b)
        } else {
            a.cmp(b)
        }
    }

    pub fn choose(&mut self, field: Field) {
        if self.field == field {
            self.descending = !self.descending;
        } else {
            self.field = field;
            self.descending = field != Field::Name;
        }
    }

    pub fn apply(self, listing: &ArchiveListing, lower_paths: &[String], visible: &mut [usize]) {
        visible.sort_by(|&left, &right| {
            let a = &listing.members[left];
            let b = &listing.members[right];
            if self.folders_first && a.is_dir != b.is_dir {
                return b.is_dir.cmp(&a.is_dir);
            }
            let cmp = match self.field {
                Field::Name => self.compare_names(&lower_paths[left], &lower_paths[right]),
                Field::Size => a.size.cmp(&b.size),
                Field::Packed => a.compressed_size.cmp(&b.compressed_size),
                Field::Modified => a.modified.cmp(&b.modified),
                Field::Ratio => match (a.compressed_size, b.compressed_size) {
                    (0, 0) => std::cmp::Ordering::Equal,
                    (0, _) => std::cmp::Ordering::Less,
                    (_, 0) => std::cmp::Ordering::Greater,
                    (a_packed, b_packed) => (u128::from(a.size) * u128::from(b_packed))
                        .cmp(&(u128::from(b.size) * u128::from(a_packed))),
                },
            };
            let cmp = if self.descending { cmp.reverse() } else { cmp };
            // Ties are deterministic and preserve each distinct archive entry.
            cmp.then_with(|| self.compare_names(&lower_paths[left], &lower_paths[right]))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.index.cmp(&b.index))
                .then_with(|| left.cmp(&right))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::ArchiveMember;
    use std::path::PathBuf;

    fn listing() -> ArchiveListing {
        ArchiveListing {
            tree: Err("Not indexed".into()),
            kind: ArchiveKind::Zip,
            members: ["z.txt", "a.txt", "folder"]
                .into_iter()
                .enumerate()
                .map(|(index, name)| ArchiveMember {
                    index,
                    path: PathBuf::from(name),
                    is_dir: index == 2,
                    size: [60, 4, 0][index],
                    compressed_size: [30, 1, 0][index],
                    tar_name: None,
                    modified: (index < 2).then(|| {
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs([10, 20, 0][index])
                    }),
                })
                .collect(),
            declared_members: 3,
            declared_uncompressed_bytes: 64,
            unsafe_members: 0,
            unreadable_members: 0,
            truncated: false,
        }
    }

    fn sorted(listing: &ArchiveListing, order: Order) -> Vec<usize> {
        let lower = listing
            .members
            .iter()
            .map(|member| member.path.to_string_lossy().to_lowercase())
            .collect::<Vec<_>>();
        let mut rows = (0..listing.members.len()).collect::<Vec<_>>();
        order.apply(listing, &lower, &mut rows);
        rows
    }

    #[test]
    fn natural_archive_names_ties_and_duplicate_ids_are_deterministic() {
        let mut listing = listing();
        let template = listing.members[0].clone();
        listing.members = ["file10", "file2", "file02", "file2", "ёж10", "ёж2"]
            .iter()
            .enumerate()
            .map(|(index, name)| ArchiveMember {
                index,
                path: (*name).into(),
                is_dir: false,
                size: 1,
                ..template.clone()
            })
            .collect();
        assert_eq!(sorted(&listing, Order::default()), [1, 3, 2, 0, 5, 4]);
        assert_eq!(
            sorted(
                &listing,
                Order {
                    natural: false,
                    ..Default::default()
                }
            ),
            [2, 0, 1, 3, 4, 5]
        );
        assert_eq!(
            sorted(
                &listing,
                Order {
                    descending: true,
                    ..Default::default()
                }
            ),
            [4, 5, 0, 2, 1, 3]
        );
        // Equal metadata uses ascending name order, including in reverse sorts.
        assert_eq!(
            sorted(
                &listing,
                Order {
                    field: Field::Size,
                    descending: true,
                    ..Default::default()
                }
            ),
            [1, 3, 2, 0, 5, 4]
        );
        listing.members.truncate(2);
        listing.members[0].path = format!("file{}", "9".repeat(1000)).into();
        listing.members[1].path = format!("file1{}", "0".repeat(1000)).into();
        assert_eq!(sorted(&listing, Order::default()), [0, 1]);
    }

    #[test]
    fn metadata_orders_and_reversal_keep_folders_first_when_requested() {
        let listing = listing();
        assert_eq!(sorted(&listing, Order::default()), [1, 2, 0]);
        for field in [Field::Size, Field::Packed] {
            let order = Order {
                field,
                descending: true,
                folders_first: false,
                ..Order::default()
            };
            assert_eq!(sorted(&listing, order), [0, 1, 2]);
            assert_eq!(
                sorted(
                    &listing,
                    Order {
                        folders_first: true,
                        ..order
                    }
                ),
                [2, 0, 1]
            );
            assert_eq!(
                sorted(
                    &listing,
                    Order {
                        descending: false,
                        folders_first: true,
                        ..order
                    }
                ),
                [2, 1, 0]
            );
        }
        assert_eq!(
            sorted(
                &listing,
                Order {
                    field: Field::Modified,
                    descending: true,
                    folders_first: false,
                    ..Order::default()
                }
            ),
            [1, 0, 2]
        );
        assert_eq!(
            sorted(
                &listing,
                Order {
                    field: Field::Ratio,
                    descending: true,
                    folders_first: false,
                    ..Order::default()
                }
            ),
            [1, 0, 2]
        );
    }

    #[test]
    fn compression_ratio_sort_is_exact_without_floating_point_or_overflow() {
        let mut listing = listing();
        listing.members.truncate(2);
        listing.members[0].size = u64::MAX;
        listing.members[0].compressed_size = u64::MAX - 1;
        listing.members[1].size = u64::MAX - 1;
        listing.members[1].compressed_size = u64::MAX - 2;
        assert_eq!(
            sorted(
                &listing,
                Order {
                    field: Field::Ratio,
                    descending: true,
                    folders_first: false,
                    ..Order::default()
                }
            ),
            [1, 0]
        );
    }

    #[test]
    fn toggling_field_and_unavailable_tar_metadata_are_explicit() {
        let mut order = Order::default();
        order.choose(Field::Size);
        assert!(order.descending);
        order.choose(Field::Size);
        assert!(!order.descending);
        order.choose(Field::Name);
        assert!(!order.descending);
        assert!(!Field::Ratio.available(ArchiveKind::Tar));
        assert!(!Field::Packed.available(ArchiveKind::TarGz));
        assert!(Field::Modified.available(ArchiveKind::Tar));
    }
}
