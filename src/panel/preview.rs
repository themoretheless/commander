use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use super::{FileEntry, format_size};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewIdentity {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl PreviewIdentity {
    pub fn from_entry(entry: &FileEntry) -> Self {
        Self {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
        }
    }

    pub fn matches_entry(&self, entry: &FileEntry) -> bool {
        self.path == entry.path && self.size == entry.size && self.modified == entry.modified
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Pdf(PreviewIdentity),
    Image(PathBuf),
    Pending(PreviewIdentity),
    Books {
        identity: PreviewIdentity,
        books: Arc<crate::fictionbook_preview::BookCollection>,
    },
    Text {
        identity: PreviewIdentity,
        content: Arc<str>,
    },
    Info(Box<InfoCard>),
}

/// Precomputed metadata card for the Get-Info inspector (display-only).
#[derive(Debug, Clone, PartialEq)]
pub struct InfoCard {
    pub image_metadata: Option<crate::image_metadata::Identity>,
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: String,
    pub allocated_size: String,
    pub filesystem_id: String,
    pub hard_links: String,
    pub owner_uid: String,
    pub owner_gid: String,
    pub children: Option<usize>,
    pub modified: String,
    pub accessed: String,
    pub created: String,
    pub changed: String,
    pub permissions: String,
    pub link_status: Option<String>,
    /// The folder whose size or item count was still being measured when
    /// the card was built, so the card can fill it in once it lands.
    pub pending: Option<PathBuf>,
}

fn ownership_text(id: Option<u64>, is_link: bool) -> String {
    id.map_or_else(
        || "Unavailable".into(),
        |id| {
            if is_link {
                format!("Link object: {id}")
            } else {
                id.to_string()
            }
        },
    )
}

/// The size shown while a folder is still being measured.
pub const MEASURING: &str = "\u{2026}";

/// Format the permission bits of a unix mode as "rwxr-xr-x". Like `ls -l`,
/// setuid/setgid show as `s` (`S` without execute) in the owner/group
/// execute slot and the sticky bit as `t` (`T`) in the other slot.
pub fn format_mode(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for (shift, special, mark) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        let triplet = (mode >> shift) & 0o7;
        s.push(if triplet & 0o4 != 0 { 'r' } else { '-' });
        s.push(if triplet & 0o2 != 0 { 'w' } else { '-' });
        let execute = triplet & 0o1 != 0;
        s.push(match (mode & special != 0, execute) {
            (true, true) => mark,
            (true, false) => mark.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    s
}

/// The Get-Info permissions row, from what the listing observed. Rows the
/// listing could not stat say so rather than showing a mode string a real
/// `000` file would also show.
fn permissions_text(permissions: Option<&fs::Permissions>) -> String {
    match permissions {
        #[cfg(unix)]
        Some(permissions) => {
            use std::os::unix::fs::PermissionsExt;
            format_mode(permissions.mode())
        }
        #[cfg(not(unix))]
        Some(permissions) => {
            let text = if permissions.readonly() {
                "read-only"
            } else {
                "read/write"
            };
            text.to_string()
        }
        None => "unavailable".to_string(),
    }
}

/// Toast text for a preview request on an image the preview provider
/// declined, which would otherwise open nothing without saying why.
pub fn image_preview_unavailable_message(entry: &FileEntry) -> String {
    format!(
        "No preview for {}: image preview is turned off or not rolled out here (Developer panel \u{2192} Runtime controls)",
        entry.name
    )
}

/// Build a Get-Info card for `entry`. `dir_size`/`children` come from the
/// panel's already-computed maps (None while still measuring).
pub fn make_info(entry: &FileEntry, dir_size: Option<u64>, children: Option<usize>) -> InfoCard {
    let target_kind = if entry.is_dir {
        "Folder".to_string()
    } else if entry.extension.is_empty() {
        "Document".to_string()
    } else {
        format!("{} file", entry.extension.to_uppercase())
    };
    // A link shows its target's kind and permissions (they are what opening
    // it uses), but must not pass for the target itself.
    let link_target = entry
        .symlink
        .as_ref()
        .and_then(|info| info.target.as_ref().ok());
    let state = entry.symlink.as_ref().map(|info| &info.state);
    let kind = if entry.is_symlink()
        && !matches!(
            state,
            Some(super::LinkTargetState::Available | super::LinkTargetState::Missing)
        ) {
        "Symlink (target unavailable)".into()
    } else {
        info_kind(
            target_kind,
            entry.is_symlink(),
            matches!(state, Some(super::LinkTargetState::Missing)),
        )
    };
    let link_status = entry.is_symlink().then(|| {
        entry.symlink.as_ref().map_or_else(
            || "Target not observed in this listing".into(),
            |info| info.status(),
        )
    });
    let size = if entry.is_symlink() && !matches!(state, Some(super::LinkTargetState::Available)) {
        "Unavailable".into()
    } else if entry.is_dir {
        dir_size.map_or_else(|| MEASURING.to_string(), format_size)
    } else {
        format_size(entry.size)
    };
    let permissions = permissions_text(entry.permissions.as_ref());
    let path = match link_target {
        Some(target) => format!("{} -> {}", entry.path.display(), target.display()),
        None => entry.path.display().to_string(),
    };
    // Folder links are never measured (see `SizeIndex::schedule`).
    let pending =
        (entry.is_dir && !entry.is_symlink() && (dir_size.is_none() || children.is_none()))
            .then(|| entry.path.clone());
    InfoCard {
        image_metadata: crate::image_metadata::Identity::from_entry(entry),
        accessed: if entry.is_symlink() && entry.accessed.is_some() {
            format!("Link object: {}", entry.accessed_str)
        } else {
            entry.accessed_str.clone()
        },
        created: if entry.is_symlink() && entry.created.is_some() {
            format!("Link object: {}", entry.created_str)
        } else {
            entry.created_str.clone()
        },
        changed: if entry.is_symlink() && entry.changed.is_some() {
            format!("Link object: {}", entry.changed_str)
        } else {
            entry.changed_str.clone()
        },
        owner_uid: ownership_text(entry.owner_uid, entry.is_symlink()),
        owner_gid: ownership_text(entry.owner_gid, entry.is_symlink()),
        filesystem_id: entry.filesystem_id().map_or_else(
            || "Unavailable".into(),
            |(device, inode)| {
                let prefix = if entry.is_symlink() {
                    "Link object: "
                } else {
                    ""
                };
                format!("{prefix}Device {device}, inode {inode}")
            },
        ),
        hard_links: if entry.is_dir || entry.is_symlink() {
            "Not measured".into()
        } else {
            entry
                .hard_link_count()
                .map_or_else(|| "Unavailable".into(), |count| count.to_string())
        },
        allocated_size: if entry.is_symlink() || entry.is_dir {
            "Not measured".into()
        } else {
            entry.allocated_size.map_or_else(
                || "Unavailable".into(),
                |bytes| format!("{} ({bytes} bytes)", format_size(bytes)),
            )
        },
        link_status,
        pending,
        name: entry.name.clone(),
        path,
        kind,
        size,
        children: if entry.is_dir { children } else { None },
        modified: entry.modified_str.clone(),
        permissions,
    }
}

/// The Get-Info "Kind" row: a symlink names itself (and a dangling one
/// says so) before the kind of what it points at.
fn info_kind(target_kind: String, is_symlink: bool, dangling: bool) -> String {
    match (is_symlink, dangling) {
        (true, true) => "Broken symlink".to_string(),
        (true, false) => format!("Symlink to {target_kind}"),
        (false, _) => target_kind,
    }
}

/// Create a preview marker without reading file contents. Text is resolved by
/// the app's cancellable background preview pipeline.
pub fn make_preview(entry: &FileEntry) -> Option<PreviewContent> {
    if entry.is_dir {
        return None;
    }
    if entry.extension.eq_ignore_ascii_case("pdf") {
        return Some(PreviewContent::Pdf(PreviewIdentity::from_entry(entry)));
    }
    if !entry.is_image() {
        return Some(PreviewContent::Pending(PreviewIdentity::from_entry(entry)));
    }

    let root = entry.path.parent().unwrap_or(Path::new("/"));
    if !crate::provider_runtime::activate_builtin(
        "native-preview",
        &crate::provider_runtime::ActivationRequest {
            capability: crate::provider_runtime::ProviderCapability::PreviewImage,
            root,
            extension: (!entry.extension.is_empty()).then_some(entry.extension.as_str()),
            bytes: Some(entry.size),
        },
    ) {
        return None;
    }
    Some(PreviewContent::Image(entry.path.clone()))
}

#[cfg(test)]
mod allocation_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn allocation_uses_captured_block_counts_for_sparse_empty_and_written_files() {
        use std::os::unix::fs::MetadataExt;
        let dir = crate::testutil::TempDir::new();
        for (name, size, contents) in [
            ("sparse.bin", 8 * 1024 * 1024, None),
            ("empty.bin", 0, None),
            ("written.bin", 4096, Some(vec![7u8; 4096])),
        ] {
            let path = dir.path().join(name);
            let file = fs::File::create(&path).unwrap();
            if let Some(contents) = contents {
                fs::write(&path, contents).unwrap();
            } else {
                file.set_len(size).unwrap();
            }
            let metadata = fs::metadata(&path).unwrap();
            let entry = FileEntry::from_meta(path.clone(), &metadata).unwrap();
            let bytes = metadata.blocks().checked_mul(512).unwrap();
            assert_eq!(entry.allocated_size, Some(bytes));
            assert_eq!(entry.size, size);
            fs::remove_file(path).unwrap();
            let card = make_info(&entry, None, None);
            assert_eq!(
                card.allocated_size,
                format!("{} ({bytes} bytes)", format_size(bytes))
            );
            assert_eq!(card.size, format_size(size));
        }
    }

    #[test]
    fn unknown_allocation_is_distinct_from_observed_zero_and_folders_are_not_summed() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.file("regular.bin", "abc");
        let mut entry = FileEntry::from_meta(path.clone(), &fs::metadata(path).unwrap()).unwrap();
        entry.allocated_size = None;
        assert_eq!(make_info(&entry, None, None).allocated_size, "Unavailable");
        entry.allocated_size = Some(0);
        assert_eq!(
            make_info(&entry, None, None).allocated_size,
            "0 B (0 bytes)"
        );
        let mut folder =
            FileEntry::from_meta(dir.path().to_path_buf(), &fs::metadata(dir.path()).unwrap())
                .unwrap();
        assert_eq!(folder.allocated_size, None);
        folder.allocated_size = Some(999);
        let card = make_info(&folder, Some(12345), Some(1));
        assert_eq!(card.allocated_size, "Not measured");
        assert_eq!(card.size, format_size(12345));
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_aliases_share_allocation_and_symlinks_do_not_claim_target_blocks() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.file("file.bin", "payload");
        let alias = dir.path().join("alias.bin");
        fs::hard_link(&path, &alias).unwrap();
        let source = FileEntry::observe_lexical(path.clone()).unwrap();
        let hard = FileEntry::observe_lexical(alias).unwrap();
        assert_eq!(source.allocated_size, hard.allocated_size);
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let entry = FileEntry::observe_lexical(link.clone()).unwrap();
        assert_eq!(entry.allocated_bytes(), None);
        assert_eq!(make_info(&entry, None, None).allocated_size, "Not measured");
        fs::remove_file(path).unwrap();
        let broken = FileEntry::observe_lexical(link).unwrap();
        assert_eq!(broken.allocated_size, None);
        assert_eq!(
            make_info(&broken, None, None).allocated_size,
            "Not measured"
        );
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn filesystem_identity_distinguishes_hard_links_copies_and_symlinks_without_restat() {
        let dir = crate::testutil::TempDir::new();
        let original = dir.file("original.bin", "same bytes");
        let alias = dir.path().join("alias.bin");
        let copy = dir.path().join("copy.bin");
        let link = dir.path().join("link.bin");
        fs::hard_link(&original, &alias).unwrap();
        fs::copy(&original, &copy).unwrap();
        std::os::unix::fs::symlink(&original, &link).unwrap();
        let original_entry = FileEntry::observe_lexical(original.clone()).unwrap();
        let alias_entry = FileEntry::observe_lexical(alias.clone()).unwrap();
        let copied = FileEntry::observe_lexical(copy).unwrap();
        let linked = FileEntry::observe_lexical(link).unwrap();
        assert_eq!(original_entry.filesystem_id(), alias_entry.filesystem_id());
        assert_ne!(original_entry.filesystem_id(), copied.filesystem_id());
        assert_ne!(original_entry.filesystem_id(), linked.filesystem_id());
        assert_eq!(original_entry.hard_link_count(), Some(2));
        assert_eq!(copied.hard_link_count(), Some(1));
        assert_eq!(linked.hard_link_count(), None);
        assert_eq!(make_info(&linked, None, None).hard_links, "Not measured");
        assert!(
            make_info(&linked, None, None)
                .filesystem_id
                .starts_with("Link object: ")
        );
        fs::remove_file(original).unwrap();
        let fresh = FileEntry::observe_lexical(alias.clone()).unwrap();
        assert_eq!(fresh.hard_link_count(), Some(1));
        assert_eq!(fresh.filesystem_id(), alias_entry.filesystem_id());
        fs::remove_file(alias).unwrap();
        let card = make_info(&alias_entry, None, None);
        assert_eq!(card.hard_links, "2");
        assert_eq!(
            card.filesystem_id,
            make_info(&original_entry, None, None).filesystem_id
        );
    }
    #[test]
    fn filesystem_identity_formats_full_width_values_and_preserves_unknowns() {
        let dir = crate::testutil::TempDir::new();
        let file = dir.file("file", "x");
        let mut entry = FileEntry::from_meta(file.clone(), &fs::metadata(file).unwrap()).unwrap();
        let crate::panel::ListingIdentity::Captured(identity) = &mut entry.identity else {
            unreachable!()
        };
        identity.volume = Some(u64::MAX);
        identity.file_id = Some(u64::MAX - 1);
        entry.hard_links = Some(u64::MAX);
        let card = make_info(&entry, None, None);
        assert_eq!(
            card.filesystem_id,
            format!("Device {}, inode {}", u64::MAX, u64::MAX - 1)
        );
        assert_eq!(card.hard_links, u64::MAX.to_string());
        let crate::panel::ListingIdentity::Captured(identity) = &mut entry.identity else {
            unreachable!()
        };
        identity.volume = None;
        assert_eq!(make_info(&entry, None, None).filesystem_id, "Unavailable");
        entry.hard_links = None;
        assert_eq!(make_info(&entry, None, None).hard_links, "Unavailable");
        entry.hard_links = Some(0);
        assert_eq!(make_info(&entry, None, None).hard_links, "0");
        entry.identity = crate::panel::ListingIdentity::Unavailable;
        assert_eq!(entry.filesystem_id(), None);
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn owner_metadata_info_preserves_full_width_zero_and_independent_unknowns() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.file("file", "x");
        let mut entry = FileEntry::from_meta(path.clone(), &fs::metadata(&path).unwrap()).unwrap();
        entry.owner_uid = Some(0);
        entry.owner_gid = Some(u64::MAX);
        fs::remove_file(path).unwrap();
        let card = make_info(&entry, None, None);
        assert_eq!(card.owner_uid, "0");
        assert_eq!(card.owner_gid, u64::MAX.to_string());
        entry.owner_uid = None;
        let card = make_info(&entry, None, None);
        assert_eq!(card.owner_uid, "Unavailable");
        assert_eq!(card.owner_gid, u64::MAX.to_string());
        entry.owner_uid = Some(u64::MAX);
        entry.owner_gid = None;
        let card = make_info(&entry, None, None);
        assert_eq!(card.owner_uid, u64::MAX.to_string());
        assert_eq!(card.owner_gid, "Unavailable");
    }
    #[cfg(unix)]
    #[test]
    fn owner_metadata_listing_and_info_retain_lexical_objects_without_restat() {
        use std::os::unix::fs::MetadataExt;
        let dir = crate::testutil::TempDir::new();
        let file = dir.file("file", "x");
        fs::create_dir(dir.path().join("folder")).unwrap();
        std::os::unix::fs::symlink("file", dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink("missing", dir.path().join("dangling")).unwrap();
        let mut panel = crate::panel::PanelState::new(dir.path().to_path_buf());
        panel.refresh();
        for name in ["file", "folder", "link", "dangling"] {
            let path = dir.path().join(name);
            let raw = fs::symlink_metadata(&path).unwrap();
            let observed = FileEntry::observe_lexical(path).unwrap();
            let listed = panel.entries().iter().find(|e| e.name == name).unwrap();
            for entry in [&observed, listed] {
                assert_eq!(entry.owner_uid, Some(u64::from(raw.uid())));
                assert_eq!(entry.owner_gid, Some(u64::from(raw.gid())));
                let prefix = if name == "link" || name == "dangling" {
                    "Link object: "
                } else {
                    ""
                };
                let card = make_info(entry, None, None);
                assert_eq!(card.owner_uid, format!("{prefix}{}", raw.uid()));
                assert_eq!(card.owner_gid, format!("{prefix}{}", raw.gid()));
            }
        }
        let entry = panel
            .entries()
            .iter()
            .find(|e| e.name == "link")
            .unwrap()
            .clone();
        fs::remove_file(file).unwrap();
        fs::remove_file(dir.path().join("link")).unwrap();
        let card = make_info(&entry, None, None);
        assert_eq!(
            card.owner_uid,
            format!("Link object: {}", entry.owner_uid.unwrap())
        );
    }
}

#[cfg(test)]
mod alternate_date_tests {
    use super::*;
    #[test]
    fn alternate_dates_snapshot_formats_unknowns_and_survives_source_removal() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.file("dates", "x");
        let metadata = fs::symlink_metadata(&path).unwrap();
        let entry = FileEntry::observe_lexical(path.clone()).unwrap();
        assert_eq!(entry.accessed, metadata.accessed().ok());
        assert_eq!(entry.created, metadata.created().ok());
        assert_eq!(entry.changed, crate::panel::metadata_change_time(&metadata));
        let card = make_info(&entry, None, None);
        fs::remove_file(path).unwrap();
        assert_eq!(make_info(&entry, None, None), card);
        let mut unknown = entry;
        unknown.set_lexical_dates(None);
        let card = make_info(&unknown, None, None);
        assert_eq!(card.accessed, "Unavailable");
        assert_eq!(card.created, "Unavailable");
        assert_eq!(card.changed, "Unavailable");
    }
    #[cfg(unix)]
    #[test]
    fn alternate_dates_link_card_uses_lexical_observation_instead_of_target() {
        let dir = crate::testutil::TempDir::new();
        let target = dir.file("target", "x");
        let file = fs::File::open(&target).unwrap();
        file.set_times(fs::FileTimes::new().set_accessed(SystemTime::UNIX_EPOCH))
            .unwrap();
        let path = dir.path().join("link");
        std::os::unix::fs::symlink("target", &path).unwrap();
        let raw = fs::symlink_metadata(&path).unwrap();
        let entry = FileEntry::observe_lexical(path.clone()).unwrap();
        assert_eq!(entry.accessed, raw.accessed().ok());
        assert_eq!(entry.created, raw.created().ok());
        assert_ne!(entry.accessed, Some(SystemTime::UNIX_EPOCH));
        let card = make_info(&entry, None, None);
        assert!(card.accessed.starts_with("Link object: "));
        if entry.created.is_some() {
            assert!(card.created.starts_with("Link object: "));
        } else {
            assert_eq!(card.created, "Unavailable");
        }
        fs::remove_file(target).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(make_info(&entry, None, None), card);
    }
}
