//! Pure undo/redo history. An [`Action`] records a completed, reversible
//! operation; [`invert`] turns it into the action that reverses it; and
//! [`UndoStack`] sequences them. No I/O and no UI here, so the stack logic and
//! every inversion are unit-tested directly.

use std::path::PathBuf;

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
}

impl Action {
    /// How many files the action touched (for status/toast text).
    pub fn item_count(&self) -> usize {
        match self {
            Action::Move { pairs } => pairs.len(),
            Action::BatchRename { pairs, .. } => pairs.len(),
            Action::Rename { .. } => 1,
            Action::Gather { pairs, .. } | Action::Ungather { pairs, .. } => pairs.len(),
        }
    }

    /// Past-tense verb for the action, for toast/status text.
    pub fn verb(&self) -> &'static str {
        match self {
            Action::Move { .. } => "Moved",
            Action::BatchRename { .. } | Action::Rename { .. } => "Renamed",
            Action::Gather { .. } | Action::Ungather { .. } => "Gathered",
        }
    }

    /// Where "jump back" should navigate for a receipts/history view: the
    /// directory the action's targets ended up in.
    pub fn jump_to(&self) -> Option<PathBuf> {
        match self {
            Action::Move { pairs } => pairs
                .first()
                .and_then(|(_, to)| to.parent())
                .map(PathBuf::from),
            Action::BatchRename { dir, .. } => Some(dir.clone()),
            Action::Rename { to, .. } => to.parent().map(PathBuf::from),
            Action::Gather { folder, .. } => Some(folder.clone()),
            Action::Ungather { pairs, .. } => pairs
                .first()
                .and_then(|(_, to)| to.parent())
                .map(PathBuf::from),
        }
    }
}

/// The action that reverses `action`, or `None` if it cannot be inverted.
/// Every current variant inverts by swapping its source and destination.
pub fn invert(action: &Action) -> Option<Action> {
    match action {
        Action::Move { pairs } => Some(Action::Move {
            pairs: pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect(),
        }),
        Action::BatchRename { dir, pairs } => Some(Action::BatchRename {
            dir: dir.clone(),
            pairs: pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect(),
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
    }
}

/// A two-stack undo/redo history. Pushing a new action clears the redo stack,
/// so a fresh operation after some undos abandons the redo branch.
#[derive(Default)]
pub struct UndoStack {
    undo: Vec<Action>,
    redo: Vec<Action>,
}

impl UndoStack {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// The action that a Cmd+Z would reverse, without changing the stack.
    pub fn peek_undo(&self) -> Option<&Action> {
        self.undo.last()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Record a freshly performed action; abandons any redo branch.
    pub fn push(&mut self, action: Action) {
        self.undo.push(action);
        self.redo.clear();
    }

    /// Pop the most recent action onto the redo stack and return the action to
    /// execute to reverse it. Returns `None` if there is nothing to undo or the
    /// top action cannot be inverted (in which case it is left in place).
    pub fn undo(&mut self) -> Option<Action> {
        let top = self.undo.last()?;
        let inverse = invert(top)?;
        let action = self.undo.pop().unwrap();
        self.redo.push(action);
        Some(inverse)
    }

    /// Pop the most recently undone action back onto the undo stack and return
    /// it to execute (re-applying the original operation).
    pub fn redo(&mut self) -> Option<Action> {
        let action = self.redo.pop()?;
        self.undo.push(action.clone());
        Some(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let inv = invert(&a).unwrap();
        assert_eq!(
            inv,
            Action::BatchRename {
                dir: PathBuf::from("/d"),
                pairs: vec![("b.txt".into(), "a.txt".into())],
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
        assert_eq!(invert(&inverse), Some(action));
        assert_eq!(inverse.item_count(), 1);
        assert_eq!(inverse.verb(), "Gathered");
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
}
