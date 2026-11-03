//! Ordering archive rows keeps their listing indexes (and selection identities) stable.
use crate::archive::{ArchiveKind, ArchiveListing};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Field {
    #[default]
    Name,
    Extension,
    Size,
    Packed,
    Ratio,
    Modified,
    Crc32,
}

impl Field {
    pub const ALL: [Self; 7] = [
        Self::Name,
        Self::Extension,
        Self::Size,
        Self::Packed,
        Self::Ratio,
        Self::Modified,
        Self::Crc32,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Member",
            Self::Extension => "Extension",
            Self::Size => "Size",
            Self::Packed => "Packed",
            Self::Ratio => "Ratio",
            Self::Modified => "Modified",
            Self::Crc32 => "CRC-32",
        }
    }
    pub fn available(self, kind: ArchiveKind) -> bool {
        kind == ArchiveKind::Zip || !matches!(self, Self::Packed | Self::Ratio | Self::Crc32)
    }
    pub fn default_descending(self) -> bool {
        !matches!(self, Self::Name | Self::Extension | Self::Crc32)
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
            self.descending = field.default_descending();
        }
    }

    pub fn apply(self, listing: &ArchiveListing, lower_paths: &[String], visible: &mut [usize]) {
        visible.sort_by(|&left, &right| {
            let a = &listing.members[left];
            let b = &listing.members[right];
            if self.folders_first && a.is_dir != b.is_dir {
                return b.is_dir.cmp(&a.is_dir);
            }
            let crc_a = a.zip_info.as_ref().and_then(|info| info.crc32);
            let crc_b = b.zip_info.as_ref().and_then(|info| info.crc32);
            // Unavailable checksums stay last in either direction. Zero is valid.
            if self.field == Field::Crc32 && crc_a.is_some() != crc_b.is_some() {
                return crc_b.is_some().cmp(&crc_a.is_some());
            }
            let cmp = match self.field {
                Field::Name => self.compare_names(&lower_paths[left], &lower_paths[right]),
                Field::Extension => extension_key(&lower_paths[left], a.is_dir)
                    .cmp(extension_key(&lower_paths[right], b.is_dir)),
                Field::Size => a.size.cmp(&b.size),
                Field::Packed => a.compressed_size.cmp(&b.compressed_size),
                Field::Modified => a.modified.cmp(&b.modified),
                Field::Crc32 => crc_a.cmp(&crc_b),
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

pub fn extension_key(path: &str, is_dir: bool) -> &str {
    if is_dir {
        ""
    } else {
        std::path::Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
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
                    is_regular: index != 2,
                    unix_mode: None,
                    owner_uid: None,
                    owner_gid: None,
                    file_type_bits: 0,
                    is_symlink: false,
                    is_executable: Some(false),
                    size: [60, 4, 0][index],
                    compressed_size: [30, 1, 0][index],
                    zip_info: None,
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
    fn extensions_use_final_component_and_keep_duplicate_member_ids() {
        let mut listing = listing();
        let template = listing.members[0].clone();
        listing.members = [
            "z/file10.TXT",
            "a/file2.txt",
            "archive.tar.gz",
            ".profile",
            "dir.rs",
            "noext",
            "trailing.",
            "a/file2.txt",
        ]
        .iter()
        .enumerate()
        .map(|(index, path)| ArchiveMember {
            index,
            path: (*path).into(),
            is_dir: index == 4,
            ..template.clone()
        })
        .collect();
        let order = Order {
            field: Field::Extension,
            ..Default::default()
        };
        assert_eq!(sorted(&listing, order), [3, 4, 5, 6, 2, 1, 7, 0]);
        assert_eq!(
            sorted(
                &listing,
                Order {
                    descending: true,
                    ..order
                }
            ),
            [1, 7, 0, 2, 3, 4, 5, 6]
        );
        assert_eq!(
            sorted(
                &listing,
                Order {
                    folders_first: true,
                    descending: true,
                    ..order
                }
            ),
            [4, 1, 7, 0, 2, 3, 5, 6]
        );
        assert_eq!(super::extension_key("parent.txt/file", false), "");
        assert_eq!(super::extension_key(".config.json", false), "json");
        assert_eq!(super::extension_key("a.ёж", false), "ёж");
        let mut chosen = Order::default();
        chosen.choose(Field::Extension);
        assert!(!chosen.descending);
        chosen.choose(Field::Extension);
        assert!(chosen.descending);
        for kind in [
            ArchiveKind::Zip,
            ArchiveKind::Tar,
            ArchiveKind::TarGz,
            ArchiveKind::TarBz2,
            ArchiveKind::TarXz,
            ArchiveKind::TarZstd,
        ] {
            assert!(Field::Extension.available(kind));
        }
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
                is_regular: true,
                unix_mode: None,
                owner_uid: None,
                owner_gid: None,
                file_type_bits: 0,
                is_symlink: false,
                is_executable: Some(false),
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
    fn crc_order_is_numeric_keeps_unknown_last_and_preserves_duplicate_identities() {
        let mut listing = listing();
        let template = listing.members[0].clone();
        listing.members = [
            ("file10", Some(7)),
            ("file2", Some(7)),
            ("zero", Some(0)),
            ("unknown", None),
            ("max", Some(u32::MAX)),
            ("file2", Some(7)),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (name, crc32))| ArchiveMember {
            index,
            path: name.into(),
            is_dir: false,
            zip_info: Some(crate::archive::ZipMemberInfo {
                compression: "Stored".into(),
                encrypted: false,
                encryption: "None".into(),
                crc32,
            }),
            ..template.clone()
        })
        .collect();
        let order = Order {
            field: Field::Crc32,
            ..Default::default()
        };
        assert_eq!(sorted(&listing, order), [2, 1, 5, 0, 4, 3]);
        assert_eq!(
            sorted(
                &listing,
                Order {
                    descending: true,
                    ..order
                }
            ),
            [4, 1, 5, 0, 2, 3]
        );
        assert_eq!(
            sorted(
                &listing,
                Order {
                    natural: false,
                    ..order
                }
            ),
            [2, 0, 1, 5, 4, 3]
        );
        listing.members[3].is_dir = true;
        assert_eq!(
            sorted(
                &listing,
                Order {
                    folders_first: true,
                    ..order
                }
            ),
            [3, 2, 1, 5, 0, 4]
        );
        let mut selected = Order::default();
        selected.choose(Field::Crc32);
        assert!(!selected.descending);
        selected.choose(Field::Crc32);
        assert!(selected.descending);
        assert!(Field::Crc32.available(ArchiveKind::Zip));
        assert!(!Field::Crc32.available(ArchiveKind::Tar));
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
