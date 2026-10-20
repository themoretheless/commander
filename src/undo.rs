//! Undo/redo history plus filesystem-aware replay preflight. An [`Action`]
//! records a completed reversible operation, [`preview`] proves whether its
//! paths can still be replayed, and [`UndoCenter`] advances only after the
//! caller reports a successful filesystem commit.

mod center;

pub use center::{HistoryError, ReplayDirection, ReplayPlan, ReplayReservation, UndoCenter};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A completed operation that can be reversed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    /// A move: each `(from, to)` pair relocated a file from `from` to `to`.
    /// Executing the action forward moves `from` -> `to`; inverting swaps each
    /// pair so executing it moves the files back.
    Move { pairs: Vec<(PathBuf, PathBuf)> },
    /// A batch rename in `dir`: each `(from, to)` pair renamed `from` -> `to`.
    BatchRename {
        dir: PathBuf,
        pairs: Vec<(String, String)>,
        folding: crate::rename_order::NameFolding,
    },
    /// One path renamed in place. Full paths keep the action independent from
    /// whichever panel is active when undo/redo is requested.
    Rename { from: PathBuf, to: PathBuf },
    /// Move selected entries into a newly-created folder. Its inverse is an
    /// [`Action::Ungather`], which removes that folder after moving entries out.
    Gather {
        folder: PathBuf,
        pairs: Vec<(PathBuf, PathBuf)>,
    },
    /// Undo half of [`Action::Gather`]. Kept as a distinct variant so folder
    /// cleanup cannot accidentally apply to an ordinary Move.
    Ungather {
        folder: PathBuf,
        pairs: Vec<(PathBuf, PathBuf)>,
    },
    /// Move-to-Trash with a preserved version-store copy for each item.
    /// Undo restores through `version_store` (not Finder put-back).
    Trash { items: Vec<TrashedItem> },
    /// Inverse of [`Action::Trash`]: restore preserved versions to their
    /// original paths without clobbering an occupied destination.
    RestoreTrash { items: Vec<TrashedItem> },
}

/// One successfully trashed path plus the version-store record that can restore it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TrashedItem {
    pub path: PathBuf,
    pub version: crate::version_store::VersionRecord,
}

impl Action {
    /// How many files the action touched (for status/toast text).
    pub const fn item_count(&self) -> usize {
        match self {
            Self::Move { pairs } => pairs.len(),
            Self::BatchRename { pairs, .. } => pairs.len(),
            Self::Rename { .. } => 1,
            Self::Gather { pairs, .. } | Self::Ungather { pairs, .. } => pairs.len(),
            Self::Trash { items } | Self::RestoreTrash { items } => items.len(),
        }
    }

    /// Past-tense verb for the action, for toast/status text.
    pub const fn verb(&self) -> &'static str {
        match self {
            Self::Move { .. } => "Moved",
            Self::BatchRename { .. } | Self::Rename { .. } => "Renamed",
            Self::Gather { .. } => "Gathered",
            Self::Ungather { .. } => "Ungathered",
            Self::Trash { .. } => "Deleted",
            Self::RestoreTrash { .. } => "Restored",
        }
    }

    /// Where "jump back" should navigate for a receipts/history view: the
    /// directory the action's targets ended up in.
    pub fn jump_to(&self) -> Option<PathBuf> {
        match self {
            Self::Move { pairs } => pairs
                .first()
                .and_then(|(_, to)| to.parent())
                .map(PathBuf::from),
            Self::BatchRename { dir, .. } => Some(dir.clone()),
            Self::Rename { to, .. } => to.parent().map(PathBuf::from),
            Self::Gather { folder, .. } => Some(folder.clone()),
            Self::Ungather { pairs, .. } => pairs
                .first()
                .and_then(|(_, to)| to.parent())
                .map(PathBuf::from),
            Self::Trash { items } | Self::RestoreTrash { items } => items
                .first()
                .and_then(|item| item.path.parent())
                .map(PathBuf::from),
        }
    }

    pub fn description(&self) -> String {
        let count = self.item_count();
        format!(
            "{} ({} item{})",
            self.verb(),
            count,
            if count == 1 { "" } else { "s" }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayEligibility {
    Ready,
    Blocked(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayPath {
    pub from: PathBuf,
    pub to: PathBuf,
    pub eligibility: ReplayEligibility,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayPreview {
    pub action: Action,
    pub paths: Vec<ReplayPath>,
    pub warnings: Vec<String>,
}

impl ReplayPreview {
    pub fn can_execute(&self) -> bool {
        !self.paths.is_empty()
            && self
                .paths
                .iter()
                .all(|path| path.eligibility == ReplayEligibility::Ready)
    }

    pub fn blocked_count(&self) -> usize {
        self.paths
            .iter()
            .filter(|path| matches!(path.eligibility, ReplayEligibility::Blocked(_)))
            .count()
    }
}

fn action_pairs(action: &Action) -> Vec<(PathBuf, PathBuf)> {
    match action {
        Action::Move { pairs } | Action::Gather { pairs, .. } | Action::Ungather { pairs, .. } => {
            pairs.clone()
        }
        Action::BatchRename { dir, pairs, .. } => pairs
            .iter()
            .map(|(from, to)| (dir.join(from), dir.join(to)))
            .collect(),
        Action::Rename { from, to } => vec![(from.clone(), to.clone())],
        Action::Trash { .. } | Action::RestoreTrash { .. } => Vec::new(),
    }
}

/// `Ok(true)` when an entry occupies `path`, `Ok(false)` when nothing does,
/// and a blocking reason when it cannot be inspected (access denied, a stale
/// network mount), so the preview never calls an unreachable entry missing.
fn path_is_taken(path: &Path) -> Result<bool, String> {
    use std::io::ErrorKind;
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Ok(false)
        }
        Err(error) => Err(format!(
            "Cannot inspect {}: {error}; check access or reconnect the volume",
            path.display()
        )),
    }
}

fn same_entry(left: &Path, right: &Path) -> bool {
    let (Ok(left), Ok(right)) = (
        crate::path_identity::PathIdentity::observe(left),
        crate::path_identity::PathIdentity::observe(right),
    ) else {
        return false;
    };
    left.exists
        && right.exists
        && left.volume == right.volume
        && left.file_id.is_some()
        && left.file_id == right.file_id
}

/// Inspect every replay path against the live filesystem. Occupied targets are
/// allowed only when another source in the same atomic rename set vacates them.
pub fn preview(action: &Action) -> ReplayPreview {
    match action {
        Action::Trash { items } => return preview_trash(items, false),
        Action::RestoreTrash { items } => return preview_trash(items, true),
        _ => {}
    }
    let pairs = action_pairs(action);
    let sources = pairs
        .iter()
        .map(|(from, _)| from.clone())
        .collect::<HashSet<_>>();
    let mut target_counts = HashMap::<PathBuf, usize>::new();
    for (_, to) in &pairs {
        *target_counts.entry(to.clone()).or_default() += 1;
    }
    let created_parent = match action {
        Action::Gather { folder, .. } => Some(folder.as_path()),
        _ => None,
    };
    let gather_folder = created_parent
        .map(|folder| match std::fs::symlink_metadata(folder) {
            Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
            Ok(_) => Err("Gather destination is not a directory".to_string()),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(format!("Destination cannot be inspected: {error}")),
        })
        .unwrap_or(Ok(false));
    // A Move or Ungather replays as one transfer into one folder, so every
    // pair must share it (an Ungather of a cross-folder Gather does not).
    let move_parent = match action {
        Action::Move { pairs } | Action::Ungather { pairs, .. } => pairs
            .first()
            .and_then(|(_, to)| to.parent())
            .map(Path::to_path_buf),
        _ => None,
    };

    let paths = pairs
        .into_iter()
        .map(|(from, to)| {
            let blocked = (|| {
                match path_is_taken(&from) {
                    Ok(true) => {}
                    Ok(false) => return Some("Source no longer exists".to_string()),
                    Err(reason) => return Some(reason),
                }
                if target_counts.get(&to).copied().unwrap_or_default() > 1 {
                    return Some("Multiple entries target the same path".to_string());
                }
                match &gather_folder {
                    Ok(_) => {}
                    Err(reason) => return Some(reason.clone()),
                }
                if matches!(action, Action::Move { .. } | Action::Ungather { .. }) {
                    if to.parent() != move_parent.as_deref() {
                        return Some(
                            "Items came from different folders; undo cannot restore them in one step"
                                .to_string(),
                        );
                    }
                    if from.file_name() != to.file_name() {
                        return Some(
                            "Item was renamed when it moved; undo cannot restore its name"
                                .to_string(),
                        );
                    }
                }
                if to
                    .parent()
                    .is_some_and(|parent| !parent.is_dir() && Some(parent) != created_parent)
                {
                    return Some("Destination folder no longer exists".to_string());
                }
                match path_is_taken(&to) {
                    Err(reason) => Some(reason),
                    Ok(true)
                        if !sources.contains(&to)
                            && !sources.iter().any(|source| same_entry(source, &to)) =>
                    {
                        Some("Destination is occupied by another entry".to_string())
                    }
                    Ok(_) => None,
                }
            })();
            ReplayPath {
                from,
                to,
                eligibility: blocked.map_or(ReplayEligibility::Ready, ReplayEligibility::Blocked),
            }
        })
        .collect();

    let mut warnings = Vec::new();
    if let Action::Ungather { folder, pairs } = action
        && let Ok(entries) = std::fs::read_dir(folder)
    {
        let moved = pairs
            .iter()
            .map(|(from, _)| from.clone())
            .collect::<HashSet<_>>();
        let foreign = entries
            .filter_map(Result::ok)
            .filter(|entry| !moved.contains(&entry.path()))
            .count();
        if foreign > 0 {
            warnings.push(unrelated_entries_warning(foreign));
        }
    }

    ReplayPreview {
        action: action.clone(),
        paths,
        warnings,
    }
}

/// Warning for an ungather folder that holds `foreign` entries the gather
/// did not put there.
fn unrelated_entries_warning(foreign: usize) -> String {
    let noun = if foreign == 1 { "entry" } else { "entries" };
    format!("Folder contains {foreign} unrelated {noun} and will be kept")
}

fn preview_trash(items: &[TrashedItem], restoring: bool) -> ReplayPreview {
    let paths = items
        .iter()
        .map(|item| {
            let blocked = if restoring {
                match (
                    path_is_taken(&item.version.stored),
                    path_is_taken(&item.version.original),
                ) {
                    (Err(reason), _) => Some(reason),
                    (Ok(false), _) => Some("Preserved version no longer exists".to_string()),
                    (Ok(true), Err(reason)) => Some(reason),
                    (Ok(true), Ok(true)) => Some("Restore destination is occupied".to_string()),
                    (Ok(true), Ok(false)) => None,
                }
            } else {
                match path_is_taken(&item.path) {
                    Ok(true) => None,
                    Ok(false) => Some("Source no longer exists".to_string()),
                    Err(reason) => Some(reason),
                }
            };
            ReplayPath {
                from: if restoring {
                    item.version.stored.clone()
                } else {
                    item.path.clone()
                },
                to: item.version.original.clone(),
                eligibility: blocked.map_or(ReplayEligibility::Ready, ReplayEligibility::Blocked),
            }
        })
        .collect();
    ReplayPreview {
        action: if restoring {
            Action::RestoreTrash {
                items: items.to_vec(),
            }
        } else {
            Action::Trash {
                items: items.to_vec(),
            }
        },
        paths,
        warnings: Vec::new(),
    }
}

/// The action that reverses `action`, or `None` if it cannot be inverted.
/// Every current variant inverts by swapping its source and destination.
pub fn invert(action: &Action) -> Option<Action> {
    match action {
        Action::Move { pairs } => Some(Action::Move {
            pairs: pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect(),
        }),
        Action::BatchRename {
            dir,
            pairs,
            folding,
        } => Some(Action::BatchRename {
            dir: dir.clone(),
            pairs: pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect(),
            folding: *folding,
        }),
        Action::Rename { from, to } => Some(Action::Rename {
            from: to.clone(),
            to: from.clone(),
        }),
        Action::Gather { folder, pairs } => Some(Action::Ungather {
            folder: folder.clone(),
            pairs: pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect(),
        }),
        Action::Ungather { folder, pairs } => Some(Action::Gather {
            folder: folder.clone(),
            pairs: pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect(),
        }),
        Action::Trash { items } => Some(Action::RestoreTrash {
            items: items.clone(),
        }),
        Action::RestoreTrash { items } => Some(Action::Trash {
            items: items.clone(),
        }),
    }
}

/// A two-stack undo/redo history. Pushing a new action clears the redo stack,
/// so a fresh operation after some undos abandons the redo branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedoInvalidation {
    pub abandoned_actions: usize,
    pub caused_by: String,
}

/// Identity of one recorded history entry. An undo and its redo keep the
/// same id, while recording an equal action again gets a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryEntryId(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryEntry {
    id: HistoryEntryId,
    action: Action,
}

#[derive(Default)]
struct UndoStack {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    redo_invalidation: Option<RedoInvalidation>,
}

impl UndoStack {
    const fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// The action that a Cmd+Z would reverse, without changing the stack.
    fn peek_undo(&self) -> Option<&Action> {
        self.peek_undo_entry().map(|entry| &entry.action)
    }

    fn peek_undo_entry(&self) -> Option<&HistoryEntry> {
        self.undo.last()
    }

    const fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Record a freshly performed action; abandons any redo branch.
    fn push_entry(&mut self, entry: HistoryEntry) {
        if !self.redo.is_empty() {
            self.redo_invalidation = Some(RedoInvalidation {
                abandoned_actions: self.redo.len(),
                caused_by: entry.action.description(),
            });
        }
        self.undo.push(entry);
        self.redo.clear();
    }

    #[cfg(test)]
    fn push(&mut self, action: Action) {
        let next_id = self
            .undo
            .len()
            .saturating_add(self.redo.len())
            .saturating_add(1) as u64;
        self.push_entry(HistoryEntry {
            id: HistoryEntryId(next_id),
            action,
        });
    }

    const fn redo_invalidation(&self) -> Option<&RedoInvalidation> {
        self.redo_invalidation.as_ref()
    }

    fn peek_undo_inverse(&self) -> Option<Action> {
        invert(&self.peek_undo_entry()?.action)
    }

    fn peek_redo_action(&self) -> Option<Action> {
        self.peek_redo_entry().map(|entry| entry.action.clone())
    }

    fn peek_redo_entry(&self) -> Option<&HistoryEntry> {
        self.redo.last()
    }

    fn commit_undo_entry(&mut self, expected: HistoryEntryId) -> bool {
        if self.peek_undo_entry().map(|entry| entry.id) != Some(expected) {
            return false;
        }
        let entry = self
            .undo
            .pop()
            .expect("the expected undo entry was checked above");
        self.redo.push(entry);
        self.redo_invalidation = None;
        true
    }

    fn commit_redo_entry(&mut self, expected: HistoryEntryId) -> bool {
        if self.peek_redo_entry().map(|entry| entry.id) != Some(expected) {
            return false;
        }
        let entry = self
            .redo
            .pop()
            .expect("the expected redo entry was checked above");
        self.undo.push(entry);
        true
    }

    #[cfg(test)]
    fn commit_undo(&mut self) -> bool {
        self.peek_undo_entry()
            .map(|entry| entry.id)
            .is_some_and(|entry| self.commit_undo_entry(entry))
    }

    #[cfg(test)]
    fn commit_redo(&mut self) -> bool {
        self.peek_redo_entry()
            .map(|entry| entry.id)
            .is_some_and(|entry| self.commit_redo_entry(entry))
    }

    /// Pop the most recent action onto the redo stack and return the action to
    /// execute to reverse it. Returns `None` if there is nothing to undo or the
    /// top action cannot be inverted (in which case it is left in place).
    #[cfg(test)]
    pub fn undo(&mut self) -> Option<Action> {
        let inverse = self.peek_undo_inverse()?;
        self.commit_undo();
        Some(inverse)
    }

    /// Pop the most recently undone action back onto the undo stack and return
    /// it to execute (re-applying the original operation).
    #[cfg(test)]
    pub fn redo(&mut self) -> Option<Action> {
        let action = self.peek_redo_action()?;
        self.commit_redo();
        Some(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn mv(from: &str, to: &str) -> Action {
        Action::Move {
            pairs: vec![(PathBuf::from(from), PathBuf::from(to))],
        }
    }

    #[test]
    fn jump_to_is_the_destination_parent_for_a_move() {
        let a = Action::Move {
            pairs: vec![
                (PathBuf::from("/a/x"), PathBuf::from("/b/x")),
                (PathBuf::from("/a/y"), PathBuf::from("/b/y")),
            ],
        };
        assert_eq!(a.jump_to(), Some(PathBuf::from("/b")));
    }

    #[test]
    fn jump_to_is_the_directory_itself_for_a_batch_rename() {
        let a = Action::BatchRename {
            dir: PathBuf::from("/d"),
            pairs: vec![("a.txt".into(), "b.txt".into())],
            folding: crate::rename_order::NameFolding::CaseFolded,
        };
        assert_eq!(a.jump_to(), Some(PathBuf::from("/d")));
    }

    #[test]
    fn invert_move_swaps_each_pair() {
        let a = Action::Move {
            pairs: vec![
                (PathBuf::from("/a/x"), PathBuf::from("/b/x")),
                (PathBuf::from("/a/y"), PathBuf::from("/b/y")),
            ],
        };
        let inv = invert(&a).unwrap();
        assert_eq!(
            inv,
            Action::Move {
                pairs: vec![
                    (PathBuf::from("/b/x"), PathBuf::from("/a/x")),
                    (PathBuf::from("/b/y"), PathBuf::from("/a/y")),
                ],
            }
        );
        // Inverting twice is the identity.
        assert_eq!(invert(&inv).unwrap(), a);
    }

    #[test]
    fn invert_batch_rename_swaps_names() {
        let a = Action::BatchRename {
            dir: PathBuf::from("/d"),
            pairs: vec![("a.txt".into(), "b.txt".into())],
            folding: crate::rename_order::NameFolding::CaseFolded,
        };
        let inv = invert(&a).unwrap();
        assert_eq!(
            inv,
            Action::BatchRename {
                dir: PathBuf::from("/d"),
                pairs: vec![("b.txt".into(), "a.txt".into())],
                folding: crate::rename_order::NameFolding::CaseFolded,
            }
        );
    }

    #[test]
    fn invert_single_rename_swaps_paths() {
        let action = Action::Rename {
            from: PathBuf::from("/d/old.txt"),
            to: PathBuf::from("/d/new.txt"),
        };
        assert_eq!(
            invert(&action),
            Some(Action::Rename {
                from: PathBuf::from("/d/new.txt"),
                to: PathBuf::from("/d/old.txt"),
            })
        );
        assert_eq!(action.item_count(), 1);
        assert_eq!(action.verb(), "Renamed");
        assert_eq!(action.jump_to(), Some(PathBuf::from("/d")));
    }

    #[test]
    fn invert_gather_swaps_paths_and_cleanup_semantics() {
        let folder = PathBuf::from("/d/Photos");
        let action = Action::Gather {
            folder: folder.clone(),
            pairs: vec![(PathBuf::from("/d/a.jpg"), PathBuf::from("/d/Photos/a.jpg"))],
        };
        let inverse = invert(&action).unwrap();
        assert_eq!(
            inverse,
            Action::Ungather {
                folder: folder.clone(),
                pairs: vec![(PathBuf::from("/d/Photos/a.jpg"), PathBuf::from("/d/a.jpg"),)],
            }
        );
        assert_eq!(action.verb(), "Gathered");
        assert_eq!(invert(&inverse), Some(action));
        assert_eq!(inverse.item_count(), 1);
        assert_eq!(inverse.verb(), "Ungathered");
        assert_eq!(inverse.description(), "Ungathered (1 item)");
        assert_eq!(inverse.jump_to(), Some(PathBuf::from("/d")));
    }

    #[test]
    fn invert_trash_restores_through_version_records() {
        let version = crate::version_store::VersionRecord {
            operation_id: crate::operation::OperationId("op".into()),
            key: crate::operation::IdempotencyKey("key".into()),
            original: PathBuf::from("/d/a.txt"),
            stored: PathBuf::from("/versions/a.txt"),
            created_at_secs: 1,
        };
        let action = Action::Trash {
            items: vec![TrashedItem {
                path: PathBuf::from("/d/a.txt"),
                version: version.clone(),
            }],
        };
        let inverse = invert(&action).unwrap();
        assert_eq!(
            inverse,
            Action::RestoreTrash {
                items: vec![TrashedItem {
                    path: PathBuf::from("/d/a.txt"),
                    version,
                }],
            }
        );
        assert_eq!(invert(&inverse), Some(action));
        assert_eq!(inverse.item_count(), 1);
        assert_eq!(inverse.verb(), "Restored");
        assert_eq!(inverse.jump_to(), Some(PathBuf::from("/d")));
    }

    #[test]
    fn undo_returns_inverse_and_enables_redo() {
        let mut s = UndoStack::default();
        s.push(mv("/a/f", "/b/f"));
        assert!(s.can_undo() && !s.can_redo());

        let inv = s.undo().unwrap();
        assert_eq!(inv, mv("/b/f", "/a/f")); // reverse direction
        assert!(!s.can_undo() && s.can_redo());

        let re = s.redo().unwrap();
        assert_eq!(re, mv("/a/f", "/b/f")); // original, re-applied
        assert!(s.can_undo() && !s.can_redo());
    }

    #[test]
    fn pushing_after_undo_truncates_redo() {
        let mut s = UndoStack::default();
        s.push(mv("/a/1", "/b/1"));
        s.push(mv("/a/2", "/b/2"));
        s.undo(); // /a/2 move is now redoable
        assert!(s.can_redo());

        s.push(mv("/a/3", "/b/3")); // a fresh op abandons the redo branch
        assert!(!s.can_redo());
        assert!(s.can_undo());
        assert_eq!(s.redo_invalidation().unwrap().abandoned_actions, 1);
        assert_eq!(s.redo_invalidation().unwrap().caused_by, "Moved (1 item)");
    }

    #[test]
    fn multi_step_walk_back() {
        let mut s = UndoStack::default();
        s.push(mv("/a/1", "/b/1"));
        s.push(mv("/a/2", "/b/2"));
        s.push(mv("/a/3", "/b/3"));

        assert_eq!(s.undo().unwrap(), mv("/b/3", "/a/3"));
        assert_eq!(s.undo().unwrap(), mv("/b/2", "/a/2"));
        assert_eq!(s.undo().unwrap(), mv("/b/1", "/a/1"));
        assert!(!s.can_undo());
        assert!(s.undo().is_none());

        // Redo walks forward in the original order.
        assert_eq!(s.redo().unwrap(), mv("/a/1", "/b/1"));
        assert_eq!(s.redo().unwrap(), mv("/a/2", "/b/2"));
    }

    #[test]
    fn two_phase_history_does_not_advance_before_commit() {
        let mut stack = UndoStack::default();
        stack.push(mv("/a/f", "/b/f"));
        assert_eq!(stack.peek_undo_inverse(), Some(mv("/b/f", "/a/f")));
        assert!(stack.can_undo());
        assert!(!stack.can_redo());

        assert!(stack.commit_undo());
        assert!(!stack.can_undo());
        assert!(stack.can_redo());
    }

    #[test]
    fn preview_blocks_a_missing_source_and_occupied_destination() {
        let temp = TempDir::new();
        let missing = temp.path().join("missing.txt");
        let occupied = temp.file("occupied.txt", "foreign");
        let action = Action::Rename {
            from: missing,
            to: occupied,
        };
        let preview = preview(&action);
        assert!(!preview.can_execute());
        assert_eq!(preview.blocked_count(), 1);
        assert_eq!(
            preview.paths[0].eligibility,
            ReplayEligibility::Blocked("Source no longer exists".to_string())
        );
    }

    #[test]
    fn preview_reports_an_uninspectable_source_instead_of_missing() {
        let temp = TempDir::new();
        // An interior NUL makes the lookup fail with something other than
        // NotFound, standing in for EACCES/ESTALE without needing privileges.
        let unreadable = temp.path().join("bad\0name.txt");
        let action = Action::Rename {
            from: unreadable.clone(),
            to: temp.path().join("target.txt"),
        };
        let preview = preview(&action);
        let ReplayEligibility::Blocked(reason) = &preview.paths[0].eligibility else {
            panic!("uninspectable source must block: {preview:?}");
        };
        assert!(
            reason.starts_with(&format!("Cannot inspect {}: ", unreadable.display())),
            "{reason}"
        );
        // A path under a regular file is simply absent.
        let file = temp.file("plain.txt", "x");
        assert_eq!(path_is_taken(&file.join("child")), Ok(false));
    }

    #[test]
    fn preview_allows_targets_vacated_by_the_same_batch() {
        let temp = TempDir::new();
        temp.file("a.txt", "a");
        temp.file("b.txt", "b");
        let action = Action::BatchRename {
            dir: temp.path().to_path_buf(),
            pairs: vec![
                ("a.txt".into(), "b.txt".into()),
                ("b.txt".into(), "a.txt".into()),
            ],
            folding: crate::rename_order::NameFolding::CaseFolded,
        };
        let preview = preview(&action);
        assert!(preview.can_execute(), "{preview:?}");
    }

    #[test]
    fn cross_folder_move_replay_explains_the_blocker() {
        let temp = TempDir::new();
        temp.dir("one");
        temp.dir("two");
        let action = Action::Move {
            pairs: vec![
                (temp.file("a.txt", "a"), temp.path().join("one/a.txt")),
                (temp.file("b.txt", "b"), temp.path().join("two/b.txt")),
            ],
        };
        let preview = preview(&action);
        assert!(!preview.can_execute());
        assert_eq!(
            preview.paths[1].eligibility,
            ReplayEligibility::Blocked(
                "Items came from different folders; undo cannot restore them in one step"
                    .to_string()
            )
        );
    }

    #[test]
    fn ungather_preview_reports_the_folder_cleanup_gap() {
        let temp = TempDir::new();
        let folder = temp.dir("Gathered");
        let source = temp.file("Gathered/a.txt", "a");
        temp.file("Gathered/foreign.txt", "foreign");
        let destination = temp.path().join("a.txt");
        let action = Action::Ungather {
            folder,
            pairs: vec![(source, destination)],
        };
        let preview = preview(&action);
        assert!(preview.can_execute());
        assert_eq!(preview.warnings.len(), 1);
        assert!(preview.warnings[0].contains("unrelated"));
        assert_eq!(
            preview.warnings[0],
            "Folder contains 1 unrelated entry and will be kept"
        );
        assert_eq!(
            unrelated_entries_warning(2),
            "Folder contains 2 unrelated entries and will be kept"
        );
    }

    #[test]
    fn stack_commit_rejects_a_stale_entry_and_clears_redo_invalidation() {
        let entry = |id, action| HistoryEntry {
            id: HistoryEntryId(id),
            action,
        };
        let mut stack = UndoStack::default();
        stack.push_entry(entry(1, mv("/a/1", "/b/1")));
        assert!(stack.commit_undo_entry(HistoryEntryId(1)));
        stack.push_entry(entry(2, mv("/a/2", "/b/2"))); // abandons the redo branch
        stack.push_entry(entry(3, mv("/a/3", "/b/3")));
        assert!(stack.redo_invalidation().is_some());

        assert!(!stack.commit_undo_entry(HistoryEntryId(2)));
        assert_eq!(stack.peek_undo(), Some(&mv("/a/3", "/b/3")));
        assert!(stack.redo_invalidation().is_some());

        assert!(stack.commit_undo_entry(HistoryEntryId(3)));
        assert!(stack.redo_invalidation().is_none());
        assert_eq!(stack.peek_redo_action(), Some(mv("/a/3", "/b/3")));
    }

    fn blocked(reason: &str) -> ReplayEligibility {
        ReplayEligibility::Blocked(reason.to_string())
    }

    #[test]
    fn preview_blocks_every_pair_targeting_the_same_path() {
        let temp = TempDir::new();
        temp.file("a.txt", "a");
        temp.file("b.txt", "b");
        let action = Action::BatchRename {
            dir: temp.path().to_path_buf(),
            pairs: vec![
                ("a.txt".into(), "c.txt".into()),
                ("b.txt".into(), "c.txt".into()),
            ],
            folding: crate::rename_order::NameFolding::CaseFolded,
        };
        let preview = preview(&action);
        assert_eq!(preview.blocked_count(), 2);
        for path in &preview.paths {
            assert_eq!(
                path.eligibility,
                blocked("Multiple entries target the same path")
            );
        }
    }

    #[test]
    fn gather_preview_allows_a_missing_or_existing_directory() {
        let temp = TempDir::new();
        let a = temp.file("a.txt", "a");
        let b = temp.file("b.txt", "b");
        let folder = temp.path().join("Photos");
        let action = Action::Gather {
            folder: folder.clone(),
            pairs: vec![(a.clone(), folder.join("a.txt")), (b, folder.join("b.txt"))],
        };
        // The folder is created by the replay, so its absence is not a
        // missing destination folder.
        assert!(preview(&action).can_execute(), "{:?}", preview(&action));

        temp.dir("Photos");
        let preview = preview(&action);
        assert!(preview.can_execute(), "{preview:?}");
        assert!(a.exists());
    }

    #[test]
    fn gather_preview_rejects_a_non_directory_destination() {
        let temp = TempDir::new();
        let source = temp.file("a.txt", "a");
        let folder = temp.file("Photos", "occupied");
        let action = Action::Gather {
            folder: folder.clone(),
            pairs: vec![(source, folder.join("a.txt"))],
        };

        let preview = preview(&action);

        assert_eq!(preview.blocked_count(), 1);
        assert_eq!(
            preview.paths[0].eligibility,
            blocked("Gather destination is not a directory")
        );
    }

    #[test]
    fn preview_blocks_a_rename_into_a_vanished_folder() {
        let temp = TempDir::new();
        let action = Action::Rename {
            from: temp.file("x.txt", "x"),
            to: temp.path().join("gone/x.txt"),
        };
        assert_eq!(
            preview(&action).paths[0].eligibility,
            blocked("Destination folder no longer exists")
        );
    }

    #[cfg(unix)]
    #[test]
    fn preview_allows_a_target_that_is_the_same_entry_as_its_source() {
        let temp = TempDir::new();
        let source = temp.file("a.txt", "a");
        let link = temp.path().join("b.txt");
        std::fs::hard_link(&source, &link).unwrap();
        let same = Action::Rename {
            from: source.clone(),
            to: link,
        };
        assert!(preview(&same).can_execute(), "{:?}", preview(&same));

        let foreign = Action::Rename {
            from: source,
            to: temp.file("c.txt", "c"),
        };
        assert_eq!(
            preview(&foreign).paths[0].eligibility,
            blocked("Destination is occupied by another entry")
        );
    }

    fn trashed(path: PathBuf, stored: PathBuf) -> TrashedItem {
        TrashedItem {
            path: path.clone(),
            version: crate::version_store::VersionRecord {
                operation_id: crate::operation::OperationId("op".into()),
                key: crate::operation::IdempotencyKey("key".into()),
                original: path,
                stored,
                created_at_secs: 1,
            },
        }
    }

    #[test]
    fn restore_trash_preview_checks_the_stored_copy_and_destination() {
        let temp = TempDir::new();
        let stored = temp.file("store/a.txt", "saved");
        let missing_store = temp.path().join("store/gone.txt");
        let free = temp.path().join("free.txt");
        let occupied = temp.file("occupied.txt", "new");
        let preview = preview(&Action::RestoreTrash {
            items: vec![
                trashed(free.clone(), missing_store),
                trashed(occupied, stored.clone()),
                trashed(free.clone(), stored.clone()),
            ],
        });
        assert_eq!(
            preview.paths[0].eligibility,
            blocked("Preserved version no longer exists")
        );
        assert_eq!(
            preview.paths[1].eligibility,
            blocked("Restore destination is occupied")
        );
        assert_eq!(preview.paths[2].eligibility, ReplayEligibility::Ready);
        assert_eq!(preview.paths[2].from, stored);
        assert_eq!(preview.paths[2].to, free);
    }

    #[test]
    fn trash_redo_preview_blocks_a_missing_path() {
        let temp = TempDir::new();
        let stored = temp.path().join("store/a.txt");
        let present = temp.file("a.txt", "a");
        let preview = preview(&Action::Trash {
            items: vec![
                trashed(temp.path().join("gone.txt"), stored.clone()),
                trashed(present, stored),
            ],
        });
        assert_eq!(
            preview.paths[0].eligibility,
            blocked("Source no longer exists")
        );
        assert_eq!(preview.paths[1].eligibility, ReplayEligibility::Ready);
    }
}
