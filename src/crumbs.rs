//! Pure breadcrumb segmentation and width-based elision. No egui types, so the
//! split and the collapse are unit-tested directly on paths.

use std::path::{Path, PathBuf};

/// One clickable path segment.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Crumb {
    pub label: String,
    /// Full path up to and including this segment, so a click navigates here.
    pub full_path: PathBuf,
}

/// Split a path into ancestor segments, root first. The root is labelled "/";
/// each deeper segment carries the full path up to and including it.
pub fn crumbs(path: &Path) -> Vec<Crumb> {
    let mut out = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        let label = current
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());
        out.push(Crumb {
            label,
            full_path: current.clone(),
        });
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    out.reverse();
    out
}

/// An elided crumb trail: the always-shown first segment, a collapsed middle
/// group (empty when nothing is hidden), and the trailing segments ending at
/// the current directory.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CrumbLayout {
    pub head: Crumb,
    /// Ancestors hidden behind a "..." chip, in path order.
    pub collapsed: Vec<Crumb>,
    /// Crumbs shown after the head (and the "..." chip, if any).
    pub tail: Vec<Crumb>,
}

impl CrumbLayout {
    /// Whether any segment is hidden behind the "..." chip.
    pub fn is_elided(&self) -> bool {
        !self.collapsed.is_empty()
    }
}

/// Collapse `list` so at most `max_visible` segments are shown (head + tail),
/// always keeping the first and last. The middle is filled from the END so the
/// deepest context stays visible. `max_visible` is clamped to at least 2.
/// An empty `list` yields a root-only layout.
pub fn elide_crumbs(list: &[Crumb], max_visible: usize) -> CrumbLayout {
    let max_visible = max_visible.max(2);
    if list.is_empty() {
        return CrumbLayout {
            head: Crumb {
                label: "/".to_string(),
                full_path: PathBuf::from("/"),
            },
            collapsed: Vec::new(),
            tail: Vec::new(),
        };
    }
    let n = list.len();
    let head = list[0].clone();
    if n <= max_visible {
        return CrumbLayout {
            head,
            collapsed: Vec::new(),
            tail: list[1..].to_vec(),
        };
    }
    // Head takes one slot; the remaining slots go to the deepest segments.
    let tail_count = max_visible - 1;
    let split = n - tail_count;
    CrumbLayout {
        head,
        collapsed: list[1..split].to_vec(),
        tail: list[split..].to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(list: &[Crumb]) -> Vec<&str> {
        list.iter().map(|c| c.label.as_str()).collect()
    }

    #[test]
    fn crumbs_split_absolute_path_root_first() {
        let c = crumbs(Path::new("/Users/me/Documents"));
        assert_eq!(labels(&c), vec!["/", "Users", "me", "Documents"]);
        // Each carries the full path up to it.
        assert_eq!(c[0].full_path, PathBuf::from("/"));
        assert_eq!(c[1].full_path, PathBuf::from("/Users"));
        assert_eq!(c[3].full_path, PathBuf::from("/Users/me/Documents"));
    }

    #[test]
    fn crumbs_of_root_is_single_segment() {
        let c = crumbs(Path::new("/"));
        assert_eq!(labels(&c), vec!["/"]);
        assert_eq!(c[0].full_path, PathBuf::from("/"));
    }

    #[test]
    fn no_elision_when_within_budget() {
        let c = crumbs(Path::new("/Users/me"));
        let layout = elide_crumbs(&c, 5);
        assert!(!layout.is_elided());
        assert_eq!(layout.head.label, "/");
        assert_eq!(labels(&layout.tail), vec!["Users", "me"]);
    }

    #[test]
    fn elision_keeps_first_and_last_and_fills_from_the_end() {
        let c = crumbs(Path::new("/a/b/c/d/e/f"));
        // labels: /, a, b, c, d, e, f  (7 segments)
        let layout = elide_crumbs(&c, 3);
        assert!(layout.is_elided());
        assert_eq!(layout.head.label, "/");
        // Two slots after head go to the deepest: e, f.
        assert_eq!(labels(&layout.tail), vec!["e", "f"]);
        // Everything between head and tail is collapsed, in path order.
        assert_eq!(labels(&layout.collapsed), vec!["a", "b", "c", "d"]);
        // The current directory (last) is always visible.
        assert_eq!(layout.tail.last().unwrap().label, "f");
    }

    #[test]
    fn max_visible_is_clamped_to_two() {
        let c = crumbs(Path::new("/a/b/c/d"));
        let layout = elide_crumbs(&c, 0); // clamped to 2
        assert_eq!(layout.head.label, "/");
        assert_eq!(labels(&layout.tail), vec!["d"]);
        assert_eq!(labels(&layout.collapsed), vec!["a", "b", "c"]);
    }

    #[test]
    fn single_segment_has_empty_tail() {
        let layout = elide_crumbs(&crumbs(Path::new("/")), 4);
        assert_eq!(layout.head.label, "/");
        assert!(layout.tail.is_empty());
        assert!(!layout.is_elided());
    }

    #[test]
    fn empty_list_yields_root_layout() {
        let layout = elide_crumbs(&[], 4);
        assert_eq!(layout.head.label, "/");
        assert!(layout.tail.is_empty());
        assert!(!layout.is_elided());
    }
}
