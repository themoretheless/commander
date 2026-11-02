//! Optional name-component constraints on content-verified duplicate groups.
use crate::dedup::DupGroup;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum Component {
    #[default]
    Any,
    Basename,
    Stem,
    ParentName,
    RelativePath,
}
impl Component {
    pub const ALL: [Self; 5] = [
        Self::Any,
        Self::Basename,
        Self::Stem,
        Self::ParentName,
        Self::RelativePath,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "Any name",
            Self::Basename => "Same full name",
            Self::Stem => "Same name without extension",
            Self::ParentName => "Same parent folder name",
            Self::RelativePath => "Same relative path",
        }
    }
}
pub fn partition(
    roots: &[std::path::PathBuf],
    groups: Vec<DupGroup>,
    component: Component,
    ignore_ascii_case: bool,
    minimum: usize,
    cancelled: impl FnMut() -> bool,
) -> Vec<DupGroup> {
    if component == Component::Any {
        return crate::duplicate_extension::retain_groups(groups, minimum, cancelled);
    }
    crate::duplicate_extension::partition_by(groups, minimum, cancelled, |path| {
        let name = match component {
            Component::Any => return Some(Vec::new()),
            Component::Basename => path.file_name(),
            Component::Stem => path.file_stem(),
            Component::ParentName => path.parent().and_then(|parent| parent.file_name()),
            Component::RelativePath => relative_path(path, roots).map(std::path::Path::as_os_str),
        }?;
        let mut key = name.as_encoded_bytes().to_vec();
        if ignore_ascii_case {
            key.make_ascii_lowercase();
        }
        Some(key)
    })
}
/// Same most-specific-root ownership as cross-root membership and UI labels.
/// No canonicalization, target following or guessed parent is allowed.
pub fn relative_path<'a>(
    path: &'a std::path::Path,
    roots: &[std::path::PathBuf],
) -> Option<&'a std::path::Path> {
    let owner = crate::dedup_scan::root_owner(path, roots.iter().map(std::path::PathBuf::as_path))?;
    let relative = path.strip_prefix(&roots[owner]).ok()?;
    (!relative.as_os_str().is_empty()
        && relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_))))
    .then_some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn group(paths: &[&str]) -> DupGroup {
        DupGroup {
            size: 7,
            files: paths
                .iter()
                .map(|p| crate::dedup::FileKey {
                    path: (*p).into(),
                    identity: crate::panel::ListingIdentity::Unavailable,
                    size: 7,
                    hash: 1,
                    modified: None,
                })
                .collect(),
        }
    }
    fn names(groups: Vec<DupGroup>) -> Vec<Vec<std::path::PathBuf>> {
        groups
            .into_iter()
            .map(|g| g.files.into_iter().map(|f| f.path).collect())
            .collect()
    }
    #[test]
    fn relative_path_grouping_uses_captured_longest_root_and_literal_boundaries() {
        let roots = vec![
            "/backupA".into(),
            "/backupB".into(),
            "/backupA/nested".into(),
        ];
        let input = group(&[
            "/backupA/docs/file.bin",
            "/backupB/docs/file.bin",
            "/backupA/nested/docs/file.bin",
            "/backupA/other/file.bin",
            "/backupB/DOCS/FILE.BIN",
            "/backupAX/docs/file.bin",
            "/backupA/../backupB/docs/file.bin",
            "/backupA",
        ]);
        assert_eq!(
            relative_path(&input.files[2].path, &roots),
            Some(std::path::Path::new("docs/file.bin"))
        );
        assert_eq!(relative_path(&input.files[5].path, &roots), None);
        assert_eq!(relative_path(&input.files[6].path, &roots), None);
        assert_eq!(relative_path(&input.files[7].path, &roots), None);
        let exact = partition(
            &roots,
            vec![input.clone()],
            Component::RelativePath,
            false,
            3,
            || false,
        );
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].files.len(), 3);
        let folded = partition(
            &roots,
            vec![input.clone()],
            Component::RelativePath,
            true,
            4,
            || false,
        );
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].files.len(), 4);
        assert!(
            partition(
                &roots,
                vec![input.clone()],
                Component::RelativePath,
                false,
                4,
                || false
            )
            .is_empty()
        );
        assert!(
            partition(
                &[],
                vec![input.clone()],
                Component::RelativePath,
                true,
                2,
                || false
            )
            .is_empty()
        );
        let mut reversed = roots.clone();
        reversed.reverse();
        assert_eq!(
            partition(
                &reversed,
                vec![input.clone()],
                Component::RelativePath,
                false,
                3,
                || false
            ),
            exact
        );
        let mut ticks = 0;
        assert!(
            partition(
                &roots,
                vec![input.clone(), input],
                Component::RelativePath,
                true,
                2,
                || {
                    ticks += 1;
                    ticks == 11
                }
            )
            .is_empty()
        );
    }
    #[cfg(unix)]
    #[test]
    fn relative_path_grouping_preserves_non_utf8_keys_and_unicode_case() {
        use std::os::unix::ffi::OsStringExt;
        let roots = vec!["/a".into(), "/b".into()];
        let mut input = group(&["/a/�/x", "/b/�/x", "/a/�/x", "/b/�/x", "/a/É/x", "/b/é/x"]);
        input.files[0].path = std::ffi::OsString::from_vec(b"/a/\xff/x".to_vec()).into();
        input.files[1].path = std::ffi::OsString::from_vec(b"/b/\xff/x".to_vec()).into();
        let result = partition(
            &roots,
            vec![input],
            Component::RelativePath,
            true,
            2,
            || false,
        );
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0].files[0].path.as_os_str().as_encoded_bytes(),
            b"/a/\xff/x"
        );
        assert_eq!(result[1].files[0].path, std::path::Path::new("/a/�/x"));
    }

    #[test]
    fn name_components_partition_stably_and_compose_with_extensions() {
        let input = group(&[
            "/a/album/report.txt",
            "/b/Album/REPORT.TXT",
            "/c/album/report.bin",
            "/d/album/report.txt",
            "/e/other/report.pdf",
        ]);
        let exact = partition(
            &[],
            vec![input.clone()],
            Component::Basename,
            false,
            2,
            || false,
        );
        assert_eq!(
            names(exact),
            vec![vec![
                std::path::PathBuf::from("/a/album/report.txt"),
                "/d/album/report.txt".into()
            ]]
        );
        let folded = partition(
            &[],
            vec![input.clone()],
            Component::Basename,
            true,
            2,
            || false,
        );
        assert_eq!(folded[0].files.len(), 3);
        assert_eq!(
            partition(&[], vec![input.clone()], Component::Stem, false, 2, || {
                false
            })[0]
                .files
                .len(),
            4
        );
        let folded = partition(&[], vec![input.clone()], Component::Stem, true, 2, || false);
        assert_eq!(folded[0].files.len(), 5);
        assert_eq!(
            partition(
                &[],
                vec![input.clone()],
                Component::ParentName,
                false,
                2,
                || { false }
            )[0]
            .files
            .len(),
            3
        );
        assert_eq!(
            partition(
                &[],
                vec![input.clone()],
                Component::ParentName,
                true,
                2,
                || { false }
            )[0]
            .files
            .len(),
            4
        );
        let combined = crate::duplicate_extension::partition(
            folded,
            crate::duplicate_extension::Mode::CaseSensitive,
            2,
            || false,
        );
        assert_eq!(combined.len(), 1);
        assert_eq!(combined[0].files.len(), 2);
        assert!(
            partition(
                &[],
                vec![input.clone()],
                Component::Basename,
                false,
                3,
                || false
            )
            .is_empty()
        );
        let mut ticks = 0;
        assert!(
            partition(
                &[],
                vec![input.clone(), input],
                Component::Stem,
                true,
                2,
                || {
                    ticks += 1;
                    ticks == 9
                }
            )
            .is_empty()
        );
    }
    #[test]
    fn name_components_keep_dotfiles_unicode_and_missing_parents_literal() {
        let input = group(&[
            "/a/.config",
            "/b/.config",
            "/c/.config.txt",
            "/d/archive.tar.gz",
            "/e/archive.tar.zip",
            "/f/É.txt",
            "/g/é.txt",
        ]);
        let result = partition(&[], vec![input], Component::Stem, true, 2, || false);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].files.len(), 3);
        assert_eq!(result[1].files.len(), 2);
        assert!(
            partition(
                &[],
                vec![group(&["/one.txt", "/two.txt", "plain.txt"])],
                Component::ParentName,
                true,
                2,
                || false
            )
            .is_empty()
        );
    }
    #[cfg(unix)]
    #[test]
    fn name_components_do_not_merge_invalid_bytes_with_replacement_characters() {
        use std::os::unix::ffi::OsStringExt;
        let mut input = group(&["/a/�.txt", "/b/�.txt", "/c/�.txt", "/d/�.txt"]);
        for (f, p) in input.files.iter_mut().zip([
            b"/a/\xff.txt".as_slice(),
            b"/b/\xff.txt",
            b"/c/\xef\xbf\xbd.txt",
            b"/d/\xef\xbf\xbd.txt",
        ]) {
            f.path = std::ffi::OsString::from_vec(p.to_vec()).into();
        }
        for component in [Component::Basename, Component::Stem] {
            let groups = partition(&[], vec![input.clone()], component, true, 2, || false);
            assert_eq!(groups.len(), 2);
            assert_eq!(groups[0].files[0].path, input.files[0].path);
            assert_eq!(groups[1].files[0].path, input.files[2].path);
        }
    }
}
