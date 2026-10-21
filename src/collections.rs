//! Named project collections: persisted non-owning groups of directory roots
//! plus an asynchronous virtual view over their direct children.

use crate::panel::FileEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const VIEW_CAP: usize = 5_000;
const STORE_NAME: &str = "Project collections";
pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCollection {
    pub name: String,
    pub roots: Vec<PathBuf>,
}

impl ProjectCollection {
    pub fn new(name: impl Into<String>, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut seen = HashSet::new();
        let roots = roots
            .into_iter()
            .filter(|root| seen.insert(root.clone()))
            .collect();
        Self {
            name: name.into().trim().to_string(),
            roots,
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.name.is_empty()
            && !self.roots.is_empty()
            && self.roots.iter().all(|root| root.to_str().is_some())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCollections {
    pub items: Vec<ProjectCollection>,
}

impl ProjectCollections {
    pub fn add(&mut self, collection: ProjectCollection) -> bool {
        if !collection.is_valid() {
            return false;
        }
        if let Some(existing) = self
            .items
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&collection.name))
        {
            *existing = collection;
        } else {
            self.items.push(collection);
        }
        true
    }

    pub fn remove(&mut self, name: &str) {
        self.items
            .retain(|collection| !collection.name.eq_ignore_ascii_case(name));
    }

    pub fn get(&self, name: &str) -> Option<&ProjectCollection> {
        self.items
            .iter()
            .find(|collection| collection.name.eq_ignore_ascii_case(name))
    }
}

fn store_path() -> PathBuf {
    crate::fs_util::config_dir().join("project_collections.json")
}

pub fn load() -> ProjectCollections {
    load_from(&store_path())
}

fn load_from(path: &Path) -> ProjectCollections {
    let loaded: Vec<ProjectCollection> = crate::persistence::load_item_store(path, STORE_NAME);
    // Re-admit each item through `new` so a hand-edited file cannot smuggle in
    // untrimmed names, duplicate roots or empty sets. Same-name entries are
    // merged into the first one (its name and root order win, the later
    // entries' extra roots are appended) so no root is silently dropped.
    // Entries that cannot be kept at all are reported as a recovery.
    let mut collections = ProjectCollections::default();
    let mut rejected = 0usize;
    for item in loaded {
        let item = ProjectCollection::new(item.name, item.roots);
        if !item.is_valid() {
            rejected = rejected.saturating_add(1);
            continue;
        }
        if let Some(existing) = collections
            .items
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&item.name))
        {
            let roots = std::mem::take(&mut existing.roots);
            *existing = ProjectCollection::new(
                std::mem::take(&mut existing.name),
                roots.into_iter().chain(item.roots),
            );
        } else {
            collections.items.push(item);
        }
    }
    if rejected > 0 {
        crate::persistence::record_recovery(STORE_NAME, collections.items.len(), rejected);
        crate::persistence::mark_lossy_load(path);
    }
    collections
}

pub fn save(collections: &ProjectCollections) -> bool {
    save_to(&store_path(), collections)
}

fn save_to(path: &Path, collections: &ProjectCollections) -> bool {
    crate::persistence::save_item_store(path, STORE_NAME, collections)
}

#[derive(Clone, Debug)]
pub struct VirtualEntry {
    pub root: PathBuf,
    pub entry: FileEntry,
}

/// A root the view could not list, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableRoot {
    pub path: PathBuf,
    pub kind: std::io::ErrorKind,
}

impl UnavailableRoot {
    /// A short reason for the dialog: missing vs denied vs anything else.
    pub fn reason(&self) -> &'static str {
        use std::io::ErrorKind;
        match self.kind {
            ErrorKind::NotFound => "missing",
            ErrorKind::PermissionDenied => "permission denied",
            ErrorKind::NotADirectory => "not a folder",
            _ => "cannot be read",
        }
    }
}

#[derive(Clone, Debug)]
pub enum ViewEvent {
    Batch(Vec<VirtualEntry>),
    Complete {
        scanned: usize,
        unavailable_roots: Vec<UnavailableRoot>,
        truncated: bool,
        cancelled: bool,
    },
}

pub struct ViewRun {
    receiver: Receiver<ViewEvent>,
    cancelled: Arc<AtomicBool>,
}

impl ViewRun {
    pub fn try_recv(&self) -> Result<ViewEvent, TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for ViewRun {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Whether the virtual view lists `name`: dot-names only when hidden files
/// are shown, the same rule the panels use.
fn listed(name: &std::ffi::OsStr, show_hidden: bool) -> bool {
    show_hidden || !name.to_string_lossy().starts_with('.')
}

/// Stream every root's entries on a worker thread. Hidden (dot-name) entries
/// follow `show_hidden`, like the panels.
pub fn spawn_view(collection: ProjectCollection, show_hidden: bool, notify: Notify) -> ViewRun {
    spawn_view_with_cap(collection, show_hidden, VIEW_CAP, notify)
}

/// [`spawn_view`] stopping after `cap` scanned entries across all roots.
/// `truncated` is reported only when an entry was actually left unscanned.
fn spawn_view_with_cap(
    collection: ProjectCollection,
    show_hidden: bool,
    cap: usize,
    notify: Notify,
) -> ViewRun {
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut scanned = 0usize;
        let mut unavailable_roots = Vec::new();
        let mut truncated = false;
        let mut batch = Vec::with_capacity(64);

        for root in collection.roots {
            if worker_cancelled.load(Ordering::Acquire) {
                break;
            }
            let read_dir = match std::fs::read_dir(&root) {
                Ok(read_dir) => read_dir,
                Err(error) => {
                    unavailable_roots.push(UnavailableRoot {
                        path: root,
                        kind: error.kind(),
                    });
                    continue;
                }
            };
            let mut entries: Vec<_> = read_dir.filter_map(Result::ok).collect();
            entries.retain(|entry| listed(&entry.file_name(), show_hidden));
            entries.sort_by_key(|entry| entry.file_name());
            for dir_entry in entries {
                if worker_cancelled.load(Ordering::Acquire) {
                    break;
                }
                if scanned >= cap {
                    truncated = true;
                    break;
                }
                scanned += 1;
                let path = dir_entry.path();
                let Ok(metadata) = dir_entry.metadata() else {
                    continue;
                };
                let Some(entry) = FileEntry::from_meta(path, &metadata) else {
                    continue;
                };
                batch.push(VirtualEntry {
                    root: root.clone(),
                    entry,
                });
                if batch.len() == 64 {
                    if sender
                        .send(ViewEvent::Batch(std::mem::take(&mut batch)))
                        .is_err()
                    {
                        return;
                    }
                    notify();
                }
            }
            if truncated {
                break;
            }
        }
        let was_cancelled = worker_cancelled.load(Ordering::Acquire);
        if !batch.is_empty() && !was_cancelled {
            if sender.send(ViewEvent::Batch(batch)).is_err() {
                return;
            }
            notify();
        }
        let _ = sender.send(ViewEvent::Complete {
            scanned,
            unavailable_roots,
            truncated,
            cancelled: was_cancelled,
        });
        notify();
    });
    ViewRun {
        receiver,
        cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::time::{Duration, Instant};

    #[test]
    fn collection_deduplicates_roots_and_replaces_names_case_insensitively() {
        let a = PathBuf::from("/a");
        let mut collections = ProjectCollections::default();
        assert!(collections.add(ProjectCollection::new("Work", [a.clone(), a.clone()])));
        assert_eq!(collections.items[0].roots, vec![a]);
        assert!(collections.add(ProjectCollection::new("work", [PathBuf::from("/b")])));
        assert_eq!(collections.items.len(), 1);
        assert_eq!(collections.items[0].roots, vec![PathBuf::from("/b")]);
    }

    #[test]
    fn invalid_collection_is_rejected() {
        let mut collections = ProjectCollections::default();
        assert!(!collections.add(ProjectCollection::new("", [PathBuf::from("/a")])));
        assert!(!collections.add(ProjectCollection::new("Empty", [])));
        assert!(collections.items.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_root_is_rejected_before_it_can_break_saves() {
        use std::os::unix::ffi::OsStringExt;
        let root = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', b'x', 0xff]));
        let mut collections = ProjectCollections::default();

        assert!(!collections.add(ProjectCollection::new("Raw", [root])));
        assert!(collections.items.is_empty());
        assert!(serde_json::to_vec(&collections).is_ok());
    }

    #[test]
    fn collections_round_trip_through_atomic_store() {
        let temp = TempDir::new();
        let path = temp.path().join("collections.json");
        let mut collections = ProjectCollections::default();
        assert!(collections.add(ProjectCollection::new(
            "Client work",
            [PathBuf::from("/projects/client")],
        )));

        assert!(save_to(&path, &collections));
        assert_eq!(load_from(&path), collections);
    }

    #[test]
    fn load_normalizes_hand_edited_collections() {
        let temp = TempDir::new();
        let path = temp.file(
            "collections.json",
            r#"{"items":[
                {"name":"  Work ","roots":["/a","/a","/b"]},
                {"name":"work","roots":["/c"]},
                {"name":"   ","roots":["/d"]},
                {"name":"Empty","roots":[]}
            ]}"#,
        );
        let loaded = load_from(&path);
        assert_eq!(
            loaded.items,
            vec![ProjectCollection {
                name: "Work".into(),
                roots: vec![
                    PathBuf::from("/a"),
                    PathBuf::from("/b"),
                    PathBuf::from("/c")
                ],
            }]
        );
    }

    fn unreadable_backups(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".unreadable-"))
            .collect()
    }

    #[test]
    fn save_keeps_a_store_that_lost_entries_while_normalizing() {
        let temp = TempDir::new();
        let original = r#"{"items":[{"name":"   ","roots":["/d"]},{"name":"A","roots":["/a"]}]}"#;
        let path = temp.file("collections.json", original);
        let collections = load_from(&path);
        assert_eq!(collections.items.len(), 1);
        assert!(save_to(&path, &collections));

        let backups = unreadable_backups(temp.path());
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
        // Only the first save after the lossy load sets the file aside.
        assert!(save_to(&path, &collections));
        assert_eq!(unreadable_backups(temp.path()).len(), 1);
    }

    #[test]
    fn save_keeps_a_store_that_was_unreadable_at_load_even_if_readable_now() {
        let temp = TempDir::new();
        let path = temp.path().join("collections.json");
        // Simulate a transient read failure at load time.
        crate::persistence::mark_lossy_load(&path);
        let original = r#"{"items":[{"name":"A","roots":["/a"]}]}"#;
        std::fs::write(&path, original).unwrap();
        assert!(save_to(&path, &ProjectCollections::default()));

        let backups = unreadable_backups(temp.path());
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
    }

    #[test]
    fn save_keeps_an_unreadable_store_instead_of_overwriting_it() {
        let temp = TempDir::new();
        let path = temp.file("collections.json", "{ not json");
        let mut collections = load_from(&path);
        assert!(collections.items.is_empty());
        assert!(collections.add(ProjectCollection::new("New", [PathBuf::from("/n")])));
        assert!(save_to(&path, &collections));

        assert_eq!(load_from(&path), collections);
        let backups: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".unreadable-"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), "{ not json");
    }

    #[test]
    fn virtual_view_streams_entries_with_root_provenance() {
        let left = TempDir::new();
        let right = TempDir::new();
        left.file("left.txt", "l");
        right.file("right.txt", "r");
        let run = spawn_view(
            ProjectCollection::new(
                "Both",
                [left.path().to_path_buf(), right.path().to_path_buf()],
            ),
            false,
            Arc::new(|| {}),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut rows = Vec::new();
        loop {
            assert!(Instant::now() < deadline, "collection view timed out");
            match run.receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(ViewEvent::Batch(batch)) => rows.extend(batch),
                Ok(ViewEvent::Complete {
                    unavailable_roots, ..
                }) => {
                    assert!(unavailable_roots.is_empty());
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("view channel closed: {error}"),
            }
        }
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|row| { row.root == left.path() && row.entry.name == "left.txt" })
        );
        assert!(
            rows.iter()
                .any(|row| { row.root == right.path() && row.entry.name == "right.txt" })
        );
    }

    #[test]
    fn missing_roots_are_reported_without_hiding_available_ones() {
        let root = TempDir::new();
        root.file("ok.txt", "x");
        let missing = root.path().join("missing");
        let run = spawn_view(
            ProjectCollection::new("Mixed", [root.path().to_path_buf(), missing.clone()]),
            false,
            Arc::new(|| {}),
        );
        let mut unavailable = Vec::new();
        while let Ok(event) = run.receiver.recv_timeout(Duration::from_secs(5)) {
            if let ViewEvent::Complete {
                unavailable_roots, ..
            } = event
            {
                unavailable = unavailable_roots;
                break;
            }
        }
        assert_eq!(
            unavailable,
            vec![UnavailableRoot {
                path: missing,
                kind: std::io::ErrorKind::NotFound,
            }]
        );
        assert_eq!(unavailable[0].reason(), "missing");
    }

    #[test]
    fn unavailable_reasons_tell_missing_from_denied() {
        let reason = |kind| {
            UnavailableRoot {
                path: PathBuf::from("/x"),
                kind,
            }
            .reason()
        };
        assert_eq!(reason(std::io::ErrorKind::NotFound), "missing");
        assert_eq!(
            reason(std::io::ErrorKind::PermissionDenied),
            "permission denied"
        );
        assert_eq!(reason(std::io::ErrorKind::NotADirectory), "not a folder");
        assert_eq!(reason(std::io::ErrorKind::Other), "cannot be read");
    }

    fn view_names(root: &Path, show_hidden: bool) -> Vec<String> {
        let run = spawn_view(
            ProjectCollection::new("Hidden", [root.to_path_buf()]),
            show_hidden,
            Arc::new(|| {}),
        );
        let mut names = Vec::new();
        while let Ok(event) = run.receiver.recv_timeout(Duration::from_secs(5)) {
            match event {
                ViewEvent::Batch(batch) => {
                    names.extend(batch.into_iter().map(|row| row.entry.name))
                }
                ViewEvent::Complete { .. } => break,
            }
        }
        names
    }

    #[test]
    fn hidden_entries_follow_the_show_hidden_setting() {
        let root = TempDir::new();
        root.file("visible.txt", "v");
        root.file(".DS_Store", "x");
        root.dir(".git");
        assert_eq!(view_names(root.path(), false), vec!["visible.txt"]);
        assert_eq!(
            view_names(root.path(), true),
            vec![".DS_Store", ".git", "visible.txt"]
        );
    }

    /// Every row and the final `(scanned, truncated)` of a capped view.
    fn capped_view(roots: [&Path; 2], cap: usize) -> (Vec<VirtualEntry>, usize, bool) {
        let run = spawn_view_with_cap(
            ProjectCollection::new("Capped", roots.map(Path::to_path_buf)),
            false,
            cap,
            Arc::new(|| {}),
        );
        let mut rows = Vec::new();
        while let Ok(event) = run.receiver.recv_timeout(Duration::from_secs(5)) {
            match event {
                ViewEvent::Batch(batch) => rows.extend(batch),
                ViewEvent::Complete {
                    scanned, truncated, ..
                } => return (rows, scanned, truncated),
            }
        }
        panic!("collection view did not complete");
    }

    #[test]
    fn the_view_cap_spans_roots_and_reports_truncation_only_when_rows_are_left() {
        let left = TempDir::new();
        let right = TempDir::new();
        for name in ["a.txt", "b.txt"] {
            left.file(name, "l");
            right.file(name, "r");
        }
        let roots = [left.path(), right.path()];

        let (rows, scanned, truncated) = capped_view(roots, 3);
        assert_eq!((rows.len(), scanned, truncated), (3, 3, true));
        assert_eq!(
            rows.iter().filter(|row| row.root == right.path()).count(),
            1
        );

        // Exactly the cap: everything was listed, nothing was cut off.
        let (rows, scanned, truncated) = capped_view(roots, 4);
        assert_eq!((rows.len(), scanned, truncated), (4, 4, false));

        // The first root alone fills the cap; the second still has rows.
        let (rows, _, truncated) = capped_view(roots, 2);
        assert!(truncated);
        assert!(rows.iter().all(|row| row.root == left.path()));
    }
}
