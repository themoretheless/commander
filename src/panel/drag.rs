use std::path::PathBuf;

/// Session drag-and-drop state for one panel.
///
/// Entries and the hovered drop target travel together so callers clear,
/// replace, or consume a drag session through one API instead of touching
/// twin fields on [`super::PanelState`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DragState {
    entries: Vec<PathBuf>,
    drop_target: Option<PathBuf>,
}

impl DragState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[PathBuf] {
        &self.entries
    }

    pub fn drop_target(&self) -> Option<&PathBuf> {
        self.drop_target.as_ref()
    }

    pub fn has_drop_target(&self) -> bool {
        self.drop_target.is_some()
    }

    /// Replace the dragged paths. Leaves any existing drop target untouched so
    /// hover feedback from the same frame can still apply.
    pub fn set(&mut self, entries: Vec<PathBuf>) {
        self.entries = entries;
    }

    pub fn set_drop_target(&mut self, target: PathBuf) {
        self.drop_target = Some(target);
    }

    pub fn clear_drop_target(&mut self) {
        self.drop_target = None;
    }

    pub fn take_drop_target(&mut self) -> Option<PathBuf> {
        self.drop_target.take()
    }

    /// Drain entries and drop target together. Returns `None` when both are
    /// already empty.
    pub fn take(&mut self) -> Option<(Vec<PathBuf>, Option<PathBuf>)> {
        if self.entries.is_empty() && self.drop_target.is_none() {
            return None;
        }
        Some((std::mem::take(&mut self.entries), self.drop_target.take()))
    }

    /// Drain only the dragged paths, leaving any drop target in place.
    pub fn take_entries(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.entries)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.drop_target = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_drains_entries_and_target_together() {
        let mut drag = DragState::new();
        drag.set(vec![PathBuf::from("/a")]);
        drag.set_drop_target(PathBuf::from("/dest"));

        let (paths, target) = drag.take().expect("drag session");
        assert_eq!(paths, [PathBuf::from("/a")]);
        assert_eq!(target, Some(PathBuf::from("/dest")));
        assert!(drag.is_empty());
        assert!(!drag.has_drop_target());
        assert!(drag.take().is_none());
    }

    #[test]
    fn clear_zeros_both_fields() {
        let mut drag = DragState::new();
        drag.set(vec![PathBuf::from("/a")]);
        drag.set_drop_target(PathBuf::from("/dest"));
        drag.clear();
        assert!(drag.is_empty());
        assert!(drag.drop_target().is_none());
    }
}
