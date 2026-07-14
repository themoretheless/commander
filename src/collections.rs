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
        !self.name.is_empty() && !self.roots.is_empty()
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
    std::fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn save(collections: &ProjectCollections) -> bool {
    save_to(&store_path(), collections)
}

fn save_to(path: &Path, collections: &ProjectCollections) -> bool {
    serde_json::to_string_pretty(collections)
        .ok()
        .is_some_and(|json| crate::fs_util::write_atomic(path, &json))
}

#[derive(Clone, Debug)]
pub struct VirtualEntry {
    pub root: PathBuf,
    pub entry: FileEntry,
}

#[derive(Clone, Debug)]
pub enum ViewEvent {
    Batch(Vec<VirtualEntry>),
    Complete {
        scanned: usize,
        unavailable_roots: Vec<PathBuf>,
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

pub fn spawn_view(collection: ProjectCollection, notify: Notify) -> ViewRun {
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
            let Ok(read_dir) = std::fs::read_dir(&root) else {
                unavailable_roots.push(root);
                continue;
            };
            let mut entries: Vec<_> = read_dir.filter_map(Result::ok).collect();
            entries.sort_by_key(|entry| entry.file_name());
            for dir_entry in entries {
                if worker_cancelled.load(Ordering::Acquire) {
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
                if scanned >= VIEW_CAP {
                    truncated = true;
                    break;
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
        assert_eq!(unavailable, vec![missing]);
    }
}
