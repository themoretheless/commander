//! Derived archive navigation only: synthetic folders never become extraction IDs.
use crate::archive::ArchiveMember;
use std::{
    collections::{BTreeSet, HashMap},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

const MAX_FOLDERS: usize = 100_000;
const MAX_PATH_BYTES: usize = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 128;

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub directories: Vec<ArchiveMember>,
    children: HashMap<PathBuf, Vec<usize>>,
}

fn folder_path(input: &str) -> Result<PathBuf, String> {
    if input.chars().count() > 4096 || input.contains('\0') {
        return Err(
            "Archive folder paths are limited to 4096 characters and cannot contain NUL.".into(),
        );
    }
    let mut path = PathBuf::new();
    for component in Path::new(input.trim_start_matches('/')).components() {
        match component {
            Component::Normal(name) => path.push(name),
            Component::CurDir => {}
            _ => {
                return Err(
                    "Use a path from the archive root without parent (..) components.".into(),
                );
            }
        }
    }
    Ok(path)
}

impl Tree {
    /// Resolve a virtual root-relative directory without consulting the host filesystem.
    pub fn resolve_folder(&self, input: &str) -> Result<PathBuf, String> {
        let path = folder_path(input)?;
        if path.as_os_str().is_empty()
            || self
                .directories
                .binary_search_by(|entry| entry.path.cmp(&path))
                .is_ok()
        {
            Ok(path)
        } else {
            Err("Folder is not in the loaded archive index. Use an exact, case-sensitive internal folder path.".into())
        }
    }

    /// Bounded exact-prefix suggestions; results remain resolvable by the typed-path API.
    pub fn complete_folders(&self, input: &str) -> (Vec<PathBuf>, bool) {
        let Ok(path) = folder_path(input) else {
            return (Vec::new(), false);
        };
        let Some(mut prefix) = path.to_str().map(str::to_owned) else {
            return (Vec::new(), false);
        };
        if input.ends_with('/') && !prefix.is_empty() {
            prefix.push('/');
        }
        let mut rows = self
            .directories
            .iter()
            .filter_map(|entry| {
                let text = entry.path.to_str()?;
                if text.starts_with(&prefix) && text.chars().count() <= 4096 {
                    Some(entry.path.clone())
                } else {
                    None
                }
            })
            .take(13)
            .collect::<Vec<_>>();
        let more = rows.len() > 12;
        if more {
            rows.pop();
        }
        (rows, more)
    }

    pub fn build(
        members: &[ArchiveMember],
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, String> {
        let mut paths = BTreeSet::new();
        let mut bytes = 0usize;
        for member in members {
            let mut prefix = PathBuf::new();
            let components: Vec<_> = member.path.components().collect();
            if components.len() > MAX_DEPTH {
                return Err(
                    "Folder view exceeds the 128-level depth limit; use All members".into(),
                );
            }
            for (position, component) in components.iter().enumerate() {
                if cancelled() {
                    return Err("Archive folder indexing cancelled".into());
                }
                let Component::Normal(name) = component else {
                    return Err("Folder view requires enclosed member paths".into());
                };
                prefix.push(name);
                if (member.is_dir || position + 1 < components.len()) && !paths.contains(&prefix) {
                    bytes = bytes.saturating_add(prefix.as_os_str().as_encoded_bytes().len());
                    if paths.len() == MAX_FOLDERS || bytes > MAX_PATH_BYTES {
                        return Err("Folder index limit reached; use All members".into());
                    }
                    paths.insert(prefix.clone());
                }
            }
        }
        let mut tree = Self::default();
        for path in paths {
            if cancelled() {
                return Err("Archive folder indexing cancelled".into());
            }
            let index = tree.directories.len();
            tree.children
                .entry(path.parent().unwrap_or(Path::new("")).into())
                .or_default()
                .push(index);
            tree.directories.push(ArchiveMember {
                index: usize::MAX,
                path,
                is_dir: true,
                is_regular: false,
                unix_mode: None,
                owner_uid: None,
                owner_gid: None,
                file_type_bits: 0,
                is_symlink: false,
                is_executable: Some(false),
                size: 0,
                compressed_size: 0,
                tar_name: None,
                modified: None,
            });
        }
        Ok(Arc::new(tree))
    }
    pub fn children(&self, folder: &Path) -> &[usize] {
        self.children.get(folder).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_completion_normalizes_prefix_and_returns_only_resolvable_indexed_folders() {
        let tree = Tree::build(
            &[
                member("src/deep/a", false),
                member("src/next/b", false),
                member("src-extra/c", false),
                member(" spaced /ёж/a", false),
            ],
            || false,
        )
        .unwrap();
        let (rows, more) = tree.complete_folders("/./src//d");
        assert_eq!(rows, [PathBuf::from("src/deep")]);
        assert!(!more);
        let (rows, _) = tree.complete_folders("src/");
        assert_eq!(rows, [PathBuf::from("src/deep"), PathBuf::from("src/next")]);
        for row in rows {
            assert_eq!(tree.resolve_folder(row.to_str().unwrap()).unwrap(), row);
        }
        assert_eq!(
            tree.complete_folders(" spaced /ё").0,
            [PathBuf::from(" spaced /ёж")]
        );
        for prefix in ["SRC", "src/deep/a", "../src", "x\0", "/etc"] {
            assert!(tree.complete_folders(prefix).0.is_empty(), "{prefix:?}");
        }
    }

    #[test]
    fn folder_completion_caps_results_marks_more_and_rejects_unenterable_paths() {
        let members: Vec<_> = (0..30)
            .map(|i| member(&format!("folder{i:02}/file"), false))
            .collect();
        let tree = Tree::build(&members, || false).unwrap();
        let (rows, more) = tree.complete_folders("");
        assert_eq!(rows.len(), 12);
        assert!(more);
        assert_eq!(rows[0], Path::new("folder00"));
        assert_eq!(rows[11], Path::new("folder11"));
        assert_eq!(
            tree.complete_folders("folder29"),
            (vec![PathBuf::from("folder29")], false)
        );
        assert!(tree.complete_folders(&"x".repeat(4097)).0.is_empty());
        let tree = Tree::build(
            &[member(&format!("{}/file", "x".repeat(4097)), false)],
            || false,
        )
        .unwrap();
        assert!(tree.complete_folders("").0.is_empty());
    }

    #[test]
    fn typed_folder_resolution_uses_index_and_virtual_root_without_host_paths() {
        let tree = Tree::build(
            &[
                member("src/deep/file.txt", false),
                member("space dir/ёж/a", false),
                member("empty", true),
            ],
            || false,
        )
        .unwrap();
        for input in ["", "/", ".", "./", "////"] {
            assert_eq!(tree.resolve_folder(input).unwrap(), PathBuf::new());
        }
        for input in ["src/deep", "/src/deep/", "./src//deep"] {
            assert_eq!(tree.resolve_folder(input).unwrap(), Path::new("src/deep"));
        }
        assert_eq!(
            tree.resolve_folder("space dir/ёж").unwrap(),
            Path::new("space dir/ёж")
        );
        assert_eq!(tree.resolve_folder("empty").unwrap(), Path::new("empty"));
        for input in [
            "src/deep/file.txt",
            "SRC",
            "../src",
            "src/../deep",
            "/etc",
            "~",
            "missing",
            "x\0y",
        ] {
            assert!(tree.resolve_folder(input).is_err(), "{input:?}");
        }
        assert!(tree.resolve_folder(&"a".repeat(4097)).is_err());
    }

    #[test]
    fn typed_folder_resolution_preserves_literal_spaces_and_empty_archive_root() {
        let tree = Tree::build(&[member(" spaced /child", false)], || false).unwrap();
        assert_eq!(
            tree.resolve_folder(" spaced ").unwrap(),
            Path::new(" spaced ")
        );
        assert!(tree.resolve_folder("spaced").is_err());
        let empty = Tree::build(&[], || false).unwrap();
        assert_eq!(empty.resolve_folder("/").unwrap(), PathBuf::new());
        assert!(empty.resolve_folder("src").is_err());
    }

    fn member(path: &str, is_dir: bool) -> ArchiveMember {
        ArchiveMember {
            index: 0,
            path: path.into(),
            is_dir,
            is_regular: !is_dir,
            unix_mode: None,
            owner_uid: None,
            owner_gid: None,
            file_type_bits: 0,
            is_symlink: false,
            is_executable: Some(false),
            size: 0,
            compressed_size: 0,
            tar_name: None,
            modified: None,
        }
    }
    #[test]
    fn implicit_empty_and_duplicate_folders_share_a_navigation_node() {
        let members = [
            member("a/b/one", false),
            member("a/b/two", false),
            member("a", true),
            member("empty", true),
            member("a/b/one", false),
        ];
        let tree = Tree::build(&members, || false).unwrap();
        assert_eq!(
            tree.directories
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            [
                PathBuf::from("a"),
                PathBuf::from("a/b"),
                PathBuf::from("empty")
            ]
        );
        assert_eq!(tree.children(Path::new("")), [0, 2]);
        assert_eq!(tree.children(Path::new("a")), [1]);
        assert!(tree.children(Path::new("empty")).is_empty());
        assert!(
            tree.directories
                .iter()
                .all(|entry| entry.index == usize::MAX)
        );
    }
    #[test]
    fn folder_index_rejects_unsafe_paths_depth_amplification_and_cancellation() {
        for name in ["../outside", "/absolute"] {
            assert!(Tree::build(&[member(name, false)], || false).is_err());
        }
        assert!(Tree::build(&[member(&"a/".repeat(MAX_DEPTH + 1), true)], || false).is_err());
        assert!(Tree::build(&[member("a/b", false)], || true).is_err());
    }
}
