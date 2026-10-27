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

impl Tree {
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
    fn member(path: &str, is_dir: bool) -> ArchiveMember {
        ArchiveMember {
            index: 0,
            path: path.into(),
            is_dir,
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
