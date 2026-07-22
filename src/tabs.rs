//! Tab model extracted for SRP (each tab owns its PanelState independently).
//! Small piece for easier understanding. DRY from workspace.

pub use crate::panel::PanelState;

pub struct Tab {
    pub state: PanelState,
}

impl Tab {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self {
            state: PanelState::new(path),
        }
    }
}
