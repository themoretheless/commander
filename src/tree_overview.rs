//! Cancellable disk-usage scan and compressed directory-tree model.

use crate::panel::FileEntry;
use std::cmp::{Ordering as CmpOrdering, Reverse};
use std::collections::BinaryHeap;
use std::fs::{self, ReadDir};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub const NODE_CAP: usize = 50_000;
pub const TOP_LEVEL_CAP: usize = 512;
const INACCESSIBLE_PATH_CAP: usize = 256;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(80);

pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, Default)]
pub struct ScanProgress {
    pub entries: usize,
    pub files: usize,
    pub directories: usize,
    pub bytes: u64,
    pub current: PathBuf,
}

#[derive(Clone, Debug)]
pub struct OverviewNode {
    pub path: PathBuf,
    pub label: String,
    pub depth: usize,
    pub bytes: u64,
    pub files: usize,
    pub unreadable: bool,
}

#[derive(Clone, Debug)]
pub struct OverviewSnapshot {
    pub root: PathBuf,
    pub top_level: Vec<(FileEntry, u64)>,
    pub top_level_truncated: bool,
    pub nodes: Vec<OverviewNode>,
    pub progress: ScanProgress,
    pub inaccessible_paths: Vec<PathBuf>,
    pub inaccessible_count: usize,
    pub truncated: bool,
    pub cancelled: bool,
    pub elapsed: Duration,
}

#[derive(Clone, Debug)]
pub enum OverviewEvent {
    Progress(ScanProgress),
    Complete(OverviewSnapshot),
}

pub struct OverviewRun {
    receiver: Receiver<OverviewEvent>,
    cancelled: Arc<AtomicBool>,
}

impl OverviewRun {
    pub fn try_recv(&self) -> Result<OverviewEvent, TryRecvError> {
        self.receiver.try_recv()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl Drop for OverviewRun {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn spawn(root: PathBuf, notify: Notify) -> OverviewRun {
    spawn_with_cap(root, NODE_CAP, notify)
}

fn spawn_with_cap(root: PathBuf, node_cap: usize, notify: Notify) -> OverviewRun {
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        run_worker(root, node_cap.max(1), worker_cancelled, sender, notify);
    });
    OverviewRun {
        receiver,
        cancelled,
    }
}

struct Frame {
    path: PathBuf,
    depth: usize,
    entries: ReadDir,
    top_entry: Option<FileEntry>,
    bytes: u64,
    files: usize,
    direct_files: usize,
    child_nodes: Vec<usize>,
    children_truncated: bool,
}

#[derive(Clone, Debug)]
struct RawNode {
    path: PathBuf,
    bytes: u64,
    files: usize,
    direct_files: usize,
    children: Vec<usize>,
    children_truncated: bool,
    unreadable: bool,
}

struct TopLevelItem {
    entry: FileEntry,
    bytes: u64,
}

impl PartialEq for TopLevelItem {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes && self.entry.path == other.entry.path
    }
}

impl Eq for TopLevelItem {}

impl Ord for TopLevelItem {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.bytes
            .cmp(&other.bytes)
            .then_with(|| self.entry.path.cmp(&other.entry.path))
    }
}

impl PartialOrd for TopLevelItem {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

#[derive(Default)]
struct TopLevelSet {
    items: BinaryHeap<Reverse<TopLevelItem>>,
    truncated: bool,
}

impl TopLevelSet {
    fn push(&mut self, entry: FileEntry, bytes: u64) {
        let candidate = Reverse(TopLevelItem { entry, bytes });
        if self.items.len() < TOP_LEVEL_CAP {
            self.items.push(candidate);
            return;
        }
        self.truncated = true;
        if self
            .items
            .peek()
            .is_some_and(|smallest| candidate.0 > smallest.0)
        {
            self.items.pop();
            self.items.push(candidate);
        }
    }

    fn into_sorted(self) -> (Vec<(FileEntry, u64)>, bool) {
        let mut items: Vec<_> = self
            .items
            .into_iter()
            .map(|item| (item.0.entry, item.0.bytes))
            .collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
        (items, self.truncated)
    }
}

struct ScanState {
    progress: ScanProgress,
    inaccessible_paths: Vec<PathBuf>,
    inaccessible_count: usize,
    truncated: bool,
    last_progress: Instant,
}

impl ScanState {
    fn new(root: &Path) -> Self {
        Self {
            progress: ScanProgress {
                directories: 1,
                current: root.to_path_buf(),
                ..Default::default()
            },
            inaccessible_paths: Vec::new(),
            inaccessible_count: 0,
            truncated: false,
            last_progress: Instant::now(),
        }
    }

    fn inaccessible(&mut self, path: PathBuf) {
        self.inaccessible_count = self.inaccessible_count.saturating_add(1);
        if self.inaccessible_paths.len() < INACCESSIBLE_PATH_CAP {
            self.inaccessible_paths.push(path);
        }
    }

    fn send_progress(&mut self, sender: &mpsc::Sender<OverviewEvent>, notify: &Notify) -> bool {
        if self.last_progress.elapsed() < PROGRESS_INTERVAL {
            return true;
        }
        if sender
            .send(OverviewEvent::Progress(self.progress.clone()))
            .is_err()
        {
            return false;
        }
        self.last_progress = Instant::now();
        notify();
        true
    }
}

fn run_worker(
    root: PathBuf,
    node_cap: usize,
    cancelled: Arc<AtomicBool>,
    sender: mpsc::Sender<OverviewEvent>,
    notify: Notify,
) {
    let started = Instant::now();
    let mut state = ScanState::new(&root);
    let Ok(root_entries) = fs::read_dir(&root) else {
        state.inaccessible(root.clone());
        finish(
            root,
            Vec::new(),
            None,
            TopLevelSet::default(),
            state,
            false,
            started.elapsed(),
            &sender,
            &notify,
        );
        return;
    };

    let mut stack = vec![Frame {
        path: root.clone(),
        depth: 0,
        entries: root_entries,
        top_entry: None,
        bytes: 0,
        files: 0,
        direct_files: 0,
        child_nodes: Vec::new(),
        children_truncated: false,
    }];
    let mut raw_nodes = Vec::new();
    let mut top_level = TopLevelSet::default();
    let mut root_node = None;
    let mut was_cancelled = false;

    while !stack.is_empty() {
        if cancelled.load(Ordering::Acquire) {
            was_cancelled = true;
            break;
        }

        let next = stack.last_mut().and_then(|frame| frame.entries.next());
        match next {
            Some(Ok(dir_entry)) => {
                let path = dir_entry.path();
                state.progress.entries = state.progress.entries.saturating_add(1);
                state.progress.current = path.clone();
                let Ok(file_type) = dir_entry.file_type() else {
                    state.inaccessible(path);
                    continue;
                };
                if file_type.is_dir() {
                    state.progress.directories = state.progress.directories.saturating_add(1);
                    let metadata = dir_entry.metadata().ok();
                    let top_entry = (stack.len() == 1)
                        .then(|| {
                            metadata
                                .as_ref()
                                .and_then(|meta| FileEntry::from_meta(path.clone(), meta))
                        })
                        .flatten();
                    match fs::read_dir(&path) {
                        Ok(entries) => stack.push(Frame {
                            path,
                            depth: stack.len(),
                            entries,
                            top_entry,
                            bytes: 0,
                            files: 0,
                            direct_files: 0,
                            child_nodes: Vec::new(),
                            children_truncated: false,
                        }),
                        Err(_) => {
                            state.inaccessible(path.clone());
                            let node = RawNode {
                                path,
                                bytes: 0,
                                files: 0,
                                direct_files: 0,
                                children: Vec::new(),
                                children_truncated: false,
                                unreadable: true,
                            };
                            let child_index = push_raw_node(
                                &mut raw_nodes,
                                node,
                                node_cap,
                                false,
                                &mut state.truncated,
                            );
                            if let Some(parent) = stack.last_mut() {
                                if let Some(index) = child_index {
                                    parent.child_nodes.push(index);
                                } else {
                                    parent.children_truncated = true;
                                }
                            }
                            if let Some(entry) = top_entry {
                                top_level.push(entry, 0);
                            }
                        }
                    }
                } else {
                    let Ok(metadata) = fs::symlink_metadata(&path) else {
                        state.inaccessible(path);
                        continue;
                    };
                    let bytes = metadata.len();
                    state.progress.files = state.progress.files.saturating_add(1);
                    state.progress.bytes = state.progress.bytes.saturating_add(bytes);
                    if let Some(parent) = stack.last_mut() {
                        parent.bytes = parent.bytes.saturating_add(bytes);
                        parent.files = parent.files.saturating_add(1);
                        parent.direct_files = parent.direct_files.saturating_add(1);
                        if parent.depth == 0
                            && let Some(entry) = FileEntry::from_meta(path, &metadata)
                        {
                            top_level.push(entry, bytes);
                        }
                    }
                }
                if !state.send_progress(&sender, &notify) {
                    return;
                }
            }
            Some(Err(_)) => {
                if let Some(frame) = stack.last() {
                    state.inaccessible(frame.path.clone());
                }
            }
            None => {
                let frame = stack.pop().expect("non-empty scan stack");
                let is_root = frame.depth == 0;
                let node = RawNode {
                    path: frame.path,
                    bytes: frame.bytes,
                    files: frame.files,
                    direct_files: frame.direct_files,
                    children: frame.child_nodes,
                    children_truncated: frame.children_truncated,
                    unreadable: false,
                };
                let bytes = node.bytes;
                let files = node.files;
                let node_index = push_raw_node(
                    &mut raw_nodes,
                    node,
                    node_cap,
                    is_root,
                    &mut state.truncated,
                );
                if is_root {
                    root_node = node_index;
                } else {
                    if let Some(entry) = frame.top_entry {
                        top_level.push(entry, bytes);
                    }
                    if let Some(parent) = stack.last_mut() {
                        parent.bytes = parent.bytes.saturating_add(bytes);
                        parent.files = parent.files.saturating_add(files);
                        if let Some(index) = node_index {
                            parent.child_nodes.push(index);
                        } else {
                            parent.children_truncated = true;
                        }
                    }
                }
            }
        }
    }

    if was_cancelled && let Some(root_frame) = stack.first() {
        let partial_root = RawNode {
            path: root.clone(),
            bytes: root_frame.bytes,
            files: root_frame.files,
            direct_files: root_frame.direct_files,
            children: root_frame.child_nodes.clone(),
            children_truncated: true,
            unreadable: false,
        };
        root_node = push_raw_node(
            &mut raw_nodes,
            partial_root,
            node_cap,
            true,
            &mut state.truncated,
        );
    }

    finish(
        root,
        raw_nodes,
        root_node,
        top_level,
        state,
        was_cancelled,
        started.elapsed(),
        &sender,
        &notify,
    );
}

fn push_raw_node(
    nodes: &mut Vec<RawNode>,
    node: RawNode,
    cap: usize,
    is_root: bool,
    truncated: &mut bool,
) -> Option<usize> {
    if !is_root && nodes.len() >= cap.saturating_sub(1) {
        *truncated = true;
        return None;
    }
    let index = nodes.len();
    nodes.push(node);
    Some(index)
}

#[allow(clippy::too_many_arguments)]
fn finish(
    root: PathBuf,
    raw_nodes: Vec<RawNode>,
    root_node: Option<usize>,
    top_level: TopLevelSet,
    state: ScanState,
    cancelled: bool,
    elapsed: Duration,
    sender: &mpsc::Sender<OverviewEvent>,
    notify: &Notify,
) {
    let nodes = root_node.map_or_else(Vec::new, |root| compress_tree(&raw_nodes, root));
    let (top_level, top_level_truncated) = top_level.into_sorted();
    let snapshot = OverviewSnapshot {
        root,
        top_level,
        top_level_truncated,
        nodes,
        progress: state.progress,
        inaccessible_paths: state.inaccessible_paths,
        inaccessible_count: state.inaccessible_count,
        truncated: state.truncated,
        cancelled,
        elapsed,
    };
    let _ = sender.send(OverviewEvent::Complete(snapshot));
    notify();
}

fn compress_tree(nodes: &[RawNode], root: usize) -> Vec<OverviewNode> {
    let mut output = Vec::new();
    let mut pending = sorted_children(nodes, root)
        .into_iter()
        .rev()
        .map(|index| (index, 0usize))
        .collect::<Vec<_>>();

    while let Some((start, depth)) = pending.pop() {
        let mut index = start;
        let mut labels = vec![path_label(&nodes[index].path)];
        while !nodes[index].unreadable
            && nodes[index].direct_files == 0
            && nodes[index].children.len() == 1
            && !nodes[index].children_truncated
        {
            index = nodes[index].children[0];
            labels.push(path_label(&nodes[index].path));
        }

        let node = &nodes[index];
        output.push(OverviewNode {
            path: node.path.clone(),
            label: labels.join("/"),
            depth,
            bytes: node.bytes,
            files: node.files,
            unreadable: node.unreadable,
        });
        for child in sorted_children(nodes, index).into_iter().rev() {
            pending.push((child, depth.saturating_add(1)));
        }
    }
    output
}

fn sorted_children(nodes: &[RawNode], index: usize) -> Vec<usize> {
    let mut children = nodes[index].children.clone();
    children.sort_by(|a, b| {
        nodes[*b]
            .bytes
            .cmp(&nodes[*a].bytes)
            .then_with(|| nodes[*a].path.cmp(&nodes[*b].path))
    });
    children
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::time::Duration;

    fn completed(run: &OverviewRun) -> OverviewSnapshot {
        loop {
            match run.receiver.recv_timeout(Duration::from_secs(5)) {
                Ok(OverviewEvent::Progress(_)) => {}
                Ok(OverviewEvent::Complete(snapshot)) => return snapshot,
                Err(error) => panic!("overview scan did not complete: {error}"),
            }
        }
    }

    #[test]
    fn single_child_directories_are_compressed_without_losing_totals() {
        let temp = TempDir::new();
        temp.file("a/b/c/data.bin", "12345");

        let run = spawn(temp.path().to_path_buf(), Arc::new(|| {}));
        let snapshot = completed(&run);

        assert_eq!(snapshot.progress.bytes, 5);
        assert_eq!(snapshot.top_level[0].0.name, "a");
        assert_eq!(snapshot.top_level[0].1, 5);
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.nodes[0].label, "a/b/c");
        assert_eq!(snapshot.nodes[0].files, 1);
        assert_eq!(snapshot.nodes[0].bytes, 5);
    }

    #[test]
    fn branch_points_remain_visible_and_largest_children_sort_first() {
        let temp = TempDir::new();
        temp.file("project/small/a.txt", "a");
        temp.file("project/large/b.txt", "123456");

        let run = spawn(temp.path().to_path_buf(), Arc::new(|| {}));
        let snapshot = completed(&run);
        let labels: Vec<_> = snapshot
            .nodes
            .iter()
            .map(|node| (node.label.as_str(), node.depth))
            .collect();

        assert_eq!(labels[0], ("project", 0));
        assert_eq!(labels[1], ("large", 1));
        assert_eq!(labels[2], ("small", 1));
    }

    #[test]
    fn node_cap_reports_truncation_but_keeps_a_root_summary() {
        let temp = TempDir::new();
        for index in 0..12 {
            temp.file(&format!("d{index}/file.txt"), "x");
        }

        let run = spawn_with_cap(temp.path().to_path_buf(), 4, Arc::new(|| {}));
        let snapshot = completed(&run);

        assert!(snapshot.truncated);
        assert_eq!(snapshot.progress.files, 12);
        assert_eq!(snapshot.progress.bytes, 12);
        assert!(snapshot.nodes.len() <= 3);
    }

    #[test]
    fn top_level_map_keeps_only_the_largest_bounded_set() {
        let temp = TempDir::new();
        for index in 0..(TOP_LEVEL_CAP + 8) {
            temp.file(&format!("{index:04}.txt"), &"x".repeat(index + 1));
        }

        let run = spawn(temp.path().to_path_buf(), Arc::new(|| {}));
        let snapshot = completed(&run);

        assert!(snapshot.top_level_truncated);
        assert_eq!(snapshot.top_level.len(), TOP_LEVEL_CAP);
        assert_eq!(snapshot.top_level[0].1, (TOP_LEVEL_CAP + 8) as u64);
        assert_eq!(snapshot.top_level.last().unwrap().1, 9);
    }

    #[test]
    fn cancellation_finishes_with_an_explicit_partial_snapshot() {
        let temp = TempDir::new();
        for index in 0..2_000 {
            temp.file(&format!("files/{index}.txt"), "x");
        }

        let run = spawn(temp.path().to_path_buf(), Arc::new(|| {}));
        run.cancel();
        let snapshot = completed(&run);

        assert!(snapshot.cancelled);
        assert!(snapshot.progress.entries < 2_001);
    }
}
