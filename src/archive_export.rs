//! Clipboard-only archive listings. No member reads or filesystem actions.
use crate::archive::{ArchiveKind, ArchiveListing};
use std::path::Path;

const MAX_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    Names,
    NamesWithoutExtension,
    ParentFolders,
    #[default]
    Paths,
    Csv,
}
impl Format {
    pub const ALL: [Self; 5] = [
        Self::Names,
        Self::NamesWithoutExtension,
        Self::Paths,
        Self::ParentFolders,
        Self::Csv,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Names => "Names",
            Self::NamesWithoutExtension => "Names without extension",
            Self::ParentFolders => "Parent folders",
            Self::Paths => "Member paths",
            Self::Csv => "CSV",
        }
    }
}
fn append(out: &mut String, text: &str) -> Result<(), String> {
    if out.len().saturating_add(text.len()) > MAX_BYTES {
        return Err("Archive clipboard output exceeds 4 MiB; select fewer members.".into());
    }
    out.push_str(text);
    Ok(())
}
fn path_text(path: &Path) -> Result<&str, String> {
    let text = path
        .to_str()
        .ok_or("Archive clipboard output requires UTF-8 paths; no lossy names were copied.")?;
    if text.len() > MAX_BYTES {
        return Err("Archive clipboard field exceeds 4 MiB.".into());
    }
    Ok(text)
}
/// IDs address real listing members, never synthetic navigation rows. Preserve
/// caller order, including distinct entries sharing the same path text.
/// Parent folders alone are deduplicated in first-occurrence order.
pub fn format(
    archive: &Path,
    listing: &ArchiveListing,
    rows: &[usize],
    format: Format,
) -> Result<String, String> {
    if rows.is_empty() {
        return Err("No listed archive members to copy.".into());
    }
    let mut out = String::new();
    let mut parents = std::collections::HashSet::new();
    let mut lines = 0;
    if format == Format::Csv {
        append(
            &mut out,
            "Archive,Index,Path,Kind,Size (bytes),Packed (bytes),Modified,Listing truncated\n",
        )?;
    }
    for &index in rows {
        let member = listing
            .members
            .get(index)
            .ok_or("Archive member selection changed; retry copying.")?;
        let path = path_text(&member.path)?;
        if format != Format::Csv {
            let value = match format {
                Format::Names => path_text(
                    member
                        .path
                        .file_name()
                        .map(Path::new)
                        .unwrap_or(&member.path),
                )?,
                Format::NamesWithoutExtension => path_text(
                    member
                        .path
                        .file_stem()
                        .map(Path::new)
                        .unwrap_or(&member.path),
                )?,
                Format::ParentFolders => {
                    let parent = member
                        .path
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new("."));
                    let parent = path_text(parent)?;
                    if !parents.insert(parent) {
                        continue;
                    }
                    parent
                }
                _ => path,
            };
            if lines > 0 {
                append(&mut out, "\n")?;
            }
            append(&mut out, &crate::listing_export::escape_controls(value))?;
            lines += 1;
            continue;
        }
        let kind = if member.is_dir {
            "folder"
        } else if member.is_symlink {
            "symlink"
        } else if member.is_regular {
            "file"
        } else {
            "other"
        };
        let fields = [
            path_text(archive)?.to_owned(),
            member.index.to_string(),
            path.to_owned(),
            kind.to_owned(),
            if member.is_dir {
                String::new()
            } else {
                member.size.to_string()
            },
            if listing.kind == ArchiveKind::Zip && !member.is_dir {
                member.compressed_size.to_string()
            } else {
                String::new()
            },
            member
                .modified
                .map(crate::listing_export::iso_8601_utc)
                .unwrap_or_default(),
            listing.truncated.to_string(),
        ];
        for (column, field) in fields.iter().enumerate() {
            if column > 0 {
                append(&mut out, ",")?;
            }
            append(&mut out, &crate::listing_export::csv_escape(field))?;
        }
        append(&mut out, "\n")?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn listing() -> ArchiveListing {
        let members = [
            "dir/file.txt",
            "dir/file.txt",
            "folder",
            "=formula,\"quoted\"\n.txt",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, path)| crate::archive::ArchiveMember {
            index,
            path: path.into(),
            is_dir: index == 2,
            is_symlink: index == 1,
            is_regular: index != 1 && index != 2,
            unix_mode: None,
            owner_uid: None,
            owner_gid: None,
            file_type_bits: 0,
            is_executable: None,
            size: 12,
            compressed_size: 4,
            tar_name: None,
            modified: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(37)),
        })
        .collect();
        ArchiveListing {
            tree: Err("not needed".into()),
            kind: ArchiveKind::Zip,
            members,
            declared_members: 4,
            declared_uncompressed_bytes: 48,
            unsafe_members: 0,
            unreadable_members: 0,
            truncated: true,
        }
    }
    #[test]
    fn names_and_paths_keep_order_duplicates_and_escape_controls() {
        let listing = listing();
        assert_eq!(
            format(Path::new("a.zip"), &listing, &[1, 0, 2, 3], Format::Names).unwrap(),
            "file.txt\nfile.txt\nfolder\n=formula,\"quoted\"\\n.txt"
        );
        assert_eq!(
            format(Path::new("a.zip"), &listing, &[1, 0], Format::Paths).unwrap(),
            "dir/file.txt\ndir/file.txt"
        );
        assert_eq!(
            listing.members[3].path.to_str().unwrap(),
            "=formula,\"quoted\"\n.txt"
        );
    }
    #[test]
    fn stems_remove_only_final_suffix_and_preserve_duplicate_member_identity() {
        let mut listing = listing();
        for (path, expected) in [
            ("dir/archive.tar.gz", "archive.tar"),
            ("dir/.env", ".env"),
            ("dir/.env.local", ".env"),
            ("dir/name.", "name"),
            ("dir/текст.txt", "текст"),
            ("dir/control\nname.txt", "control\\nname"),
        ] {
            listing.members[0].path = path.into();
            assert_eq!(
                format(
                    Path::new("a.zip"),
                    &listing,
                    &[0, 0],
                    Format::NamesWithoutExtension
                )
                .unwrap(),
                format!("{expected}\n{expected}")
            );
        }
        listing.members[2].path = "folder.with.suffix".into();
        assert_eq!(
            format(
                Path::new("a.zip"),
                &listing,
                &[2],
                Format::NamesWithoutExtension
            )
            .unwrap(),
            "folder.with"
        );
    }

    #[test]
    fn parent_folders_are_internal_distinct_ordered_and_root_is_explicit() {
        let mut listing = listing();
        listing.members[0].path = "src/deep/file.rs".into();
        listing.members[1].path = "src/deep/link.rs".into();
        listing.members[2].path = "folder".into();
        listing.members[3].path = "other\nfolder/file".into();
        assert_eq!(
            format(
                Path::new("/host/a.zip"),
                &listing,
                &[3, 0, 1, 2, 0],
                Format::ParentFolders
            )
            .unwrap(),
            "other\\nfolder\nsrc/deep\n."
        );
        // A late invalid ID must reject the whole result even after a duplicate.
        assert!(
            format(
                Path::new("a.zip"),
                &listing,
                &[0, 1, 99],
                Format::ParentFolders
            )
            .is_err()
        );
        for mode in [Format::NamesWithoutExtension, Format::ParentFolders] {
            listing.members[0].path = format!("{}/name.txt", "x".repeat(MAX_BYTES)).into();
            assert!(format(Path::new("a.zip"), &listing, &[0], mode).is_err());
        }
    }
    #[test]
    fn csv_keeps_identity_raw_fields_metadata_partial_state_and_neutralizes_formulas() {
        let mut listing = listing();
        let text = format(Path::new("a.zip"), &listing, &[1, 2, 3], Format::Csv).unwrap();
        assert_eq!(
            text,
            "Archive,Index,Path,Kind,Size (bytes),Packed (bytes),Modified,Listing truncated\na.zip,1,dir/file.txt,symlink,12,4,1970-01-01T00:00:37Z,true\na.zip,2,folder,folder,,,1970-01-01T00:00:37Z,true\na.zip,3,\"'=formula,\"\"quoted\"\"\n.txt\",file,12,4,1970-01-01T00:00:37Z,true\n"
        );
        listing.kind = ArchiveKind::TarXz;
        listing.members[0].modified = None;
        assert!(
            format(Path::new("a.txz"), &listing, &[0], Format::Csv)
                .unwrap()
                .ends_with("a.txz,0,dir/file.txt,file,12,,,true\n")
        );
    }
    #[test]
    fn output_budget_empty_stale_ids_and_non_utf8_never_return_partial_output() {
        let mut listing = listing();
        assert!(format(Path::new("a.zip"), &listing, &[], Format::Paths).is_err());
        assert!(format(Path::new("a.zip"), &listing, &[0, 100], Format::Paths).is_err());
        listing.members[0].path = "x".repeat(MAX_BYTES).into();
        assert!(format(Path::new("a.zip"), &listing, &[0, 0], Format::Paths).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            listing.members[0].path = std::ffi::OsString::from_vec(vec![b'x', 255]).into();
            for mode in Format::ALL {
                assert!(format(Path::new("a.zip"), &listing, &[1, 0], mode).is_err());
            }
        }
    }
}
