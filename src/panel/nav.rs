//! Navigation and history stack for a panel (back/forward, go up, navigate).
//! SRP: all "where am I and how did I get here" logic in a small dedicated file.
//! Inspired by classic dual-pane FMs (mc, FAR, Total Commander) location stacks.
//! Extracted per SOLID plan to make PanelState thinner and nav testable in isolation.

use std::path::PathBuf;

use super::{PanelState, record_visit};

/// Core navigation: push new path to history (trimming forward), update current,
/// clear filter, record visit, and refresh.
pub fn navigate_to(panel: &mut PanelState, path: PathBuf) {
    // Trim forward history when navigating to a new path
    if panel.history_pos() + 1 < panel.history().len() {
        panel.truncate_history(panel.history_pos() + 1);
    }
    panel.push_history(path.clone());
    panel.set_history_pos(panel.history().len() - 1);
    record_visit(&path);
    panel.set_current_path(path);
    panel.set_search("".to_string(), false);
    panel.refresh();
}

/// Go up one directory level. Remembers the child name so cursor lands on it
/// in the parent (classic dual-pane behaviour, like in Total Commander / mc).
pub fn go_up(panel: &mut PanelState) {
    let child = panel
        .current_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string());
    if let Some(parent) = panel.current_path().parent().map(|p| p.to_path_buf()) {
        navigate_to(panel, parent);
        if let Some(name) = child
            && let Some(idx) = panel.filtered_entries().iter().position(|e| e.name == name)
        {
            panel.set_cursor_and_scroll(idx + 1);
        }
    }
}

/// Type-ahead navigation (prefix or contains match on filtered names).
/// Returns true if a match was found and cursor moved.
pub fn type_ahead(panel: &mut PanelState, buffer: &str) -> bool {
    if buffer.is_empty() {
        return false;
    }
    let q = buffer.to_lowercase();
    let pos = {
        let entries = panel.filtered_entries();
        entries
            .iter()
            .position(|e| e.name_lower.starts_with(&q))
            .or_else(|| entries.iter().position(|e| e.name_lower.contains(&q)))
    };
    if let Some(idx) = pos {
        panel.set_cursor_and_scroll(idx + 1);
        true
    } else {
        false
    }
}

pub fn can_go_back(panel: &PanelState) -> bool {
    panel.history_pos() > 0
}

pub fn can_go_forward(panel: &PanelState) -> bool {
    panel.history_pos() + 1 < panel.history().len()
}

pub fn go_back(panel: &mut PanelState) {
    if can_go_back(panel) {
        let new_pos = panel.history_pos() - 1;
        panel.set_history_pos(new_pos);
        panel.set_current_path(panel.history()[new_pos].clone());
        panel.set_search("".to_string(), false);
        panel.refresh();
    }
}

pub fn go_forward(panel: &mut PanelState) {
    if can_go_forward(panel) {
        let new_pos = panel.history_pos() + 1;
        panel.set_history_pos(new_pos);
        panel.set_current_path(panel.history()[new_pos].clone());
        panel.set_search("".to_string(), false);
        panel.refresh();
    }
}