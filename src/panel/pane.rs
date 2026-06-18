//! Pane abstraction trait for left/right symmetry.
//! SRP/ISP: define common interface for panels to reduce dupe in workspace.
//! Inspired by Double Commander and plugin systems in FAR/TC. Allows future "virtual panes".

use std::path::PathBuf;

use super::PanelState;

/// Trait for a "pane" view (could be file panel, tree, etc.).
pub trait Pane {
    fn current_path(&self) -> &PathBuf;
    fn navigate_to(&mut self, path: PathBuf);
    // TODO: more methods: refresh, select, etc. to abstract left/right.
}

/// Example impl for file panel state.
impl Pane for PanelState {
    fn current_path(&self) -> &PathBuf {
        &self.current_path
    }

    fn navigate_to(&mut self, path: PathBuf) {
        // delegate to existing
        // note: this is stub, full would call the nav logic
        self.current_path = path;
    }
}
