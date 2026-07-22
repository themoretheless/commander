//! Pane abstraction trait for left/right symmetry.
//! SRP/ISP: define common interface for panels to reduce dupe in workspace.
//! Inspired by Double Commander and plugin systems in FAR/TC. Allows future "virtual panes".

use std::path::PathBuf;

use super::{nav, PanelState};

/// Trait for a "pane" view (file panel or future virtual/tree panes).
pub trait Pane {
    fn current_path(&self) -> &PathBuf;
    fn navigate_to(&mut self, path: PathBuf);
    fn refresh(&mut self);
    fn cursor(&self) -> usize;
    fn set_cursor(&mut self, c: usize);
    fn filtered_count(&self) -> usize;
}

/// Full impl for PanelState (was stubby).
impl Pane for PanelState {
    fn current_path(&self) -> &PathBuf {
        self.current_path()
    }

    fn navigate_to(&mut self, path: PathBuf) {
        nav::navigate_to(self, path);
    }

    fn refresh(&mut self) {
        self.refresh();
    }

    fn cursor(&self) -> usize {
        self.cursor()
    }

    fn set_cursor(&mut self, c: usize) {
        self.set_cursor(c);
    }

    fn filtered_count(&self) -> usize {
        self.filtered_count()
    }
}
