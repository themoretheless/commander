//! Saved tab sets logic.
//! SRP extraction for saved/restore tab configurations (left/right paths).
//! From workspace handle_tab_and_git. Inspired by session management in editors like VSCode workspaces or Total Commander tab sets.

use std::path::PathBuf;

use crate::workspace::{PanelTab, Workspace};
use crate::panel::PanelState;

/// Simple representation of a saved set of tabs. Supports named workspaces (idea #6).
#[derive(Clone, Debug)]
pub struct TabSet {
    pub name: String,
    pub left: Vec<PathBuf>,
    pub right: Vec<PathBuf>,
}

impl TabSet {
    pub fn from_workspace(ws: &Workspace, name: impl Into<String>) -> Self {
        let left = ws.left.tabs.iter().map(|t| t.state.current_path().clone()).collect();
        let right = ws.right.tabs.iter().map(|t| t.state.current_path().clone()).collect();
        TabSet { name: name.into(), left, right }
    }

    /// Restore into the workspace (basic version, refreshes tabs).
    pub fn restore_to(&self, ws: &mut Workspace) {
        if !self.left.is_empty() {
            ws.left.tabs.clear();
            for p in &self.left {
                let mut st = PanelState::new(p.clone());
                ws.left.tabs.push(PanelTab { state: st });
            }
            ws.left.active = 0;
            for t in &mut ws.left.tabs {
                t.state.refresh();
            }
        }
        if !self.right.is_empty() {
            ws.right.tabs.clear();
            for p in &self.right {
                let mut st = PanelState::new(p.clone());
                ws.right.tabs.push(PanelTab { state: st });
            }
            ws.right.active = 0;
            for t in &mut ws.right.tabs {
                t.state.refresh();
            }
        }
    }
}

pub fn save_tab_set(ws: &mut Workspace) {
    let set = TabSet::from_workspace(ws, "Saved");
    ws.saved_tab_sets.push((set.name, set.left, set.right));
}

pub fn restore_tab_set(ws: &mut Workspace) {
    if let Some(last) = ws.saved_tab_sets.last().cloned() {
        let set = TabSet { name: last.0, left: last.1, right: last.2 };
        set.restore_to(ws);
        ws.requests.git_toast = Some(format!("Restored '{}' ({}L/{}R)", set.name, set.left.len(), set.right.len()));
    }
}