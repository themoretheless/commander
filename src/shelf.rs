//! A drop stack ("shelf"): an ordered, de-duplicated collection of paths the
//! user gathers across folders, then drains to one destination in a single
//! copy. The drain plan is pure (no I/O), so self-copy dropping and collision
//! resolution are unit-tested directly.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// An ordered, de-duplicated set of staged paths.
#[derive(Default)]
pub struct Shelf {
    items: Vec<PathBuf>,
    membership: HashSet<PathBuf>,
}

impl Shelf {
    /// Add `path` if not already present (keeps first-seen order).
    pub fn add(&mut self, path: PathBuf) {
        if self.membership.insert(path.clone()) {
            self.items.push(path);
        }
    }

    pub fn add_all(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for p in paths {
            self.add(p);
        }
    }

    pub fn remove(&mut self, path: &Path) {
        if self.membership.remove(path) {
            self.items.retain(|item| item != path);
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.membership.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn items(&self) -> &[PathBuf] {
        &self.items
    }

    /// Total size via an injected size lookup, keeping the shelf free of I/O.
    pub fn total_size(&self, size_of: impl Fn(&Path) -> u64) -> u64 {
        self.items.iter().map(|p| size_of(p)).sum()
    }
}

/// Map each shelved source to its destination path under `dest`. Drops
/// self-copies (a source already living in `dest`) and resolves name
/// collisions against `existing_names` plus the names already chosen earlier
/// in this same drain, so two staged files of the same name both land safely.
pub fn drain_plan(
    items: &[PathBuf],
    dest: &Path,
    existing_names: &HashSet<String>,
) -> Vec<(PathBuf, PathBuf)> {
    let mut taken = existing_names.clone();
    let mut plan = Vec::new();
    for src in items {
        // Drop self-copies: the file already lives in the destination folder.
        if src.parent() == Some(dest) {
            continue;
        }
        let Some(name) = src.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let chosen = crate::fs_util::free_name_against(&name, &taken);
        taken.insert(chosen.clone());
        plan.push((src.clone(), dest.join(&chosen)));
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn add_dedupes_and_keeps_order() {
        let mut s = Shelf::default();
        s.add(PathBuf::from("/a/x"));
        s.add(PathBuf::from("/b/y"));
        s.add(PathBuf::from("/a/x")); // dup ignored
        assert_eq!(s.len(), 2);
        assert_eq!(s.items(), &[PathBuf::from("/a/x"), PathBuf::from("/b/y")]);
        s.remove(Path::new("/a/x"));
        assert_eq!(s.items(), &[PathBuf::from("/b/y")]);
        s.add(PathBuf::from("/a/x"));
        assert_eq!(s.items(), &[PathBuf::from("/b/y"), PathBuf::from("/a/x")]);
        s.clear();
        s.add(PathBuf::from("/a/x"));
        assert_eq!(s.items(), &[PathBuf::from("/a/x")]);
    }

    #[test]
    fn total_size_uses_injected_lookup() {
        let mut s = Shelf::default();
        s.add(PathBuf::from("/a"));
        s.add(PathBuf::from("/bb"));
        let total = s.total_size(|p| p.as_os_str().len() as u64);
        assert_eq!(total, 2 + 3);
    }

    #[test]
    fn drain_plan_drops_self_copies() {
        let dest = Path::new("/dest");
        let items = vec![
            PathBuf::from("/dest/already.txt"), // self-copy -> dropped
            PathBuf::from("/src/new.txt"),
        ];
        let plan = drain_plan(&items, dest, &set(&[]));
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].0, PathBuf::from("/src/new.txt"));
        assert_eq!(plan[0].1, PathBuf::from("/dest/new.txt"));
    }

    #[test]
    fn drain_plan_resolves_collisions_against_existing_and_each_other() {
        let dest = Path::new("/dest");
        let items = vec![
            PathBuf::from("/a/file.txt"),
            PathBuf::from("/b/file.txt"), // same name, different source
        ];
        // "file.txt" already exists at the destination.
        let plan = drain_plan(&items, dest, &set(&["file.txt"]));
        assert_eq!(plan[0].1, PathBuf::from("/dest/file copy.txt"));
        assert_eq!(plan[1].1, PathBuf::from("/dest/file copy 2.txt"));
    }

    #[test]
    fn drain_plan_preserves_order_for_unique_names() {
        let dest = Path::new("/dest");
        let items = vec![PathBuf::from("/a/1.txt"), PathBuf::from("/a/2.txt")];
        let plan = drain_plan(&items, dest, &set(&[]));
        assert_eq!(plan[0].1, PathBuf::from("/dest/1.txt"));
        assert_eq!(plan[1].1, PathBuf::from("/dest/2.txt"));
    }
}
