//! Cached filtering of a completed/stopped disk-usage tree; no filesystem I/O.
use crate::tree_overview::OverviewNode;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub query: String,
    pub fuzzy: bool,
    pub match_case: bool,
    pub collapsed: BTreeSet<PathBuf>,
}
#[derive(Default)]
pub struct View {
    pub options: Options,
    pub indices: Vec<usize>,
    pub direct: Vec<bool>,
    pub matches: usize,
    /// Nearest represented ancestor in the full snapshot, independent of filtering.
    pub parents: Vec<Option<usize>>,
    pub has_children: Vec<bool>,
    pub hidden_by_collapse: usize,
    ready: bool,
    applied: Options,
}
impl View {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            ..Default::default()
        }
    }
    pub fn invalidate(&mut self) {
        self.ready = false;
    }
    pub fn search_changed(&self) -> bool {
        !self.ready
            || self.options.query != self.applied.query
            || self.options.fuzzy != self.applied.fuzzy
            || self.options.match_case != self.applied.match_case
    }
    pub fn is_current(&self) -> bool {
        self.ready && self.options == self.applied
    }
    #[cfg(feature = "visual-qa")]
    pub fn is_ready(&self) -> bool {
        self.is_current()
    }
    /// Returns true when the view is rebuilt, including an empty result.
    pub fn refresh(&mut self, root: &Path, nodes: &[OverviewNode]) -> bool {
        if self.ready && self.options == self.applied {
            return false;
        }
        self.applied = self.options.clone();
        self.ready = true;
        self.indices.clear();
        self.direct = vec![false; nodes.len()];
        self.matches = 0;
        let by_path: HashMap<_, _> = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.path.as_path(), index))
            .collect();
        self.parents = nodes
            .iter()
            .map(|node| {
                node.path
                    .ancestors()
                    .skip(1)
                    .take_while(|path| *path != root && path.starts_with(root))
                    .find_map(|path| by_path.get(path).copied())
            })
            .collect();
        self.has_children = vec![false; nodes.len()];
        for parent in self.parents.iter().flatten() {
            self.has_children[*parent] = true;
        }
        self.hidden_by_collapse = 0;
        if self.options.query.chars().take(257).count() > 256 {
            return true;
        }
        let query = self.options.query.trim();
        let lower = (!self.options.match_case && !self.options.fuzzy).then(|| query.to_lowercase());
        for (index, node) in nodes.iter().enumerate() {
            let path = node
                .path
                .strip_prefix(root)
                .unwrap_or(&node.path)
                .to_string_lossy();
            self.direct[index] = query.is_empty()
                || if self.options.fuzzy {
                    crate::fuzzy::is_match_case(query, &path, self.options.match_case)
                } else if let Some(lower) = &lower {
                    path.to_lowercase().contains(lower)
                } else {
                    path.contains(query)
                };
        }
        self.matches = self.direct.iter().filter(|&&matched| matched).count();
        let mut visible = self.direct.clone();
        if !query.is_empty() && self.matches > 0 {
            let mut seen = HashSet::new();
            for (index, node) in nodes.iter().enumerate() {
                if !self.direct[index] {
                    continue;
                }
                let mut parent = node.path.parent();
                while let Some(path) = parent.filter(|path| path.starts_with(root)) {
                    if !seen.insert(path) {
                        break;
                    }
                    if let Some(&index) = by_path.get(path) {
                        visible[index] = true;
                    }
                    parent = path.parent();
                }
            }
        }
        // Searching reveals matches without changing the saved collapse choices.
        if query.is_empty() {
            for (index, node) in nodes.iter().enumerate() {
                if visible[index]
                    && node
                        .path
                        .ancestors()
                        .skip(1)
                        .take_while(|path| *path != root && path.starts_with(root))
                        .any(|path| self.options.collapsed.contains(path))
                {
                    visible[index] = false;
                    self.hidden_by_collapse += 1;
                }
            }
        }
        self.indices = visible
            .iter()
            .enumerate()
            .filter_map(|(index, &shown)| shown.then_some(index))
            .collect();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(path: &str, depth: usize) -> OverviewNode {
        OverviewNode {
            measurement: Default::default(),
            path: path.into(),
            label: path.rsplit('/').next().unwrap().into(),
            depth,
            bytes: 10,
            files: 1,
            unreadable: false,
        }
    }
    #[test]
    fn filtering_keeps_only_real_ancestors_and_source_order_without_changing_totals() {
        let nodes = vec![
            node("/root/branch", 0),
            node("/root/branch/other", 1),
            node("/root/branch/compressed/target", 1),
            node("/root/branch-extra", 0),
        ];
        let mut view = View::default();
        view.options.query = "target".into();
        assert!(view.refresh(Path::new("/root"), &nodes));
        assert_eq!(view.indices, [0, 2]);
        assert_eq!(view.parents, [None, Some(0), Some(0), None]);
        assert_eq!(view.matches, 1);
        assert_eq!(view.direct, [false, false, true, false]);
        assert_eq!(nodes[0].bytes, 10);
        assert_eq!(nodes[2].depth, 1);
        assert!(!view.refresh(Path::new("/root"), &nodes));
    }
    #[test]
    fn collapse_uses_physical_ancestry_search_reveals_and_rescan_keeps_path_choices() {
        let mut nodes = vec![
            node("/root/branch", 0),
            node("/root/branch/chain/target", 1),
            node("/root/branch-extra", 0),
            node("/root/branch-extra/child", 1),
        ];
        let mut view = View::default();
        view.options.collapsed.insert("/root/branch".into());
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [0, 2, 3]);
        assert_eq!(view.has_children, [true, false, true, false]);
        assert_eq!(view.hidden_by_collapse, 1);
        assert!(!view.refresh(Path::new("/root"), &nodes));
        view.options.query = "target".into();
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [0, 1]);
        assert_eq!(view.hidden_by_collapse, 0);
        assert!(view.options.collapsed.contains(Path::new("/root/branch")));
        view.options.query.clear();
        nodes[1].path = "/root/branch/new-child".into();
        view.invalidate();
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [0, 2, 3]);
        view.options.collapsed.insert("/root/branch-extra".into());
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [0, 2]);
        assert_eq!(view.hidden_by_collapse, 2);
        view.options.collapsed.clear();
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [0, 1, 2, 3]);
        assert_eq!(nodes[0].bytes, 10);
    }
    #[test]
    fn unicode_case_fuzzy_and_relative_path_queries_compose() {
        let nodes = vec![
            node("/root/projects/target", 0),
            node("/root/папка/Пример", 0),
        ];
        let mut view = View::default();
        view.options.query = "pjtg".into();
        view.refresh(Path::new("/root"), &nodes);
        assert!(view.indices.is_empty());
        view.options.fuzzy = true;
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [0]);
        view.options.query = "Pjtg".into();
        view.options.match_case = true;
        view.refresh(Path::new("/root"), &nodes);
        assert!(view.indices.is_empty());
        view.options.fuzzy = false;
        view.options.match_case = false;
        view.options.query = "пример".into();
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [1]);
        view.options.query = "projects/target".into();
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [0]);
        view.options.query = "root".into();
        view.refresh(Path::new("/root"), &nodes);
        assert!(
            view.indices.is_empty(),
            "root prefix must not match every row"
        );
    }
    #[test]
    fn empty_results_are_cached_and_new_snapshots_require_explicit_invalidation() {
        let mut nodes = vec![node("/root/target", 0)];
        let mut view = View::default();
        view.options.query = "target".into();
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [0]);
        nodes[0].path = "/root/renamed".into();
        view.invalidate();
        view.refresh(Path::new("/root"), &nodes);
        assert!(view.indices.is_empty());
        assert!(!view.refresh(Path::new("/root"), &nodes));
        view.options.query = "x".repeat(257);
        view.refresh(Path::new("/root"), &nodes);
        assert!(view.indices.is_empty());
        view.options.query = "  ".into();
        view.refresh(Path::new("/root"), &nodes);
        assert_eq!(view.indices, [0]);
        assert_eq!(view.matches, 1);
    }
}
