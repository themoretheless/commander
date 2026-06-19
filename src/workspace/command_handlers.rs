//! Command handlers extracted to avoid giant match in execute.
//! SRP: each group of commands (tabs, navigation, file ops, etc.) has its handler.
//! Inspired by VSCode command registry and FAR key handlers. Dispatch from Workspace::execute.
//! Full CommandHandler trait for plugins/extensibility.

use crate::command::Command;
use crate::workspace::Workspace;

/// Trait for command handlers. Plugins can implement to add commands.
pub trait CommandHandler {
    fn handle(&mut self, ws: &mut Workspace, cmd: Command) -> bool;
}

// Blanket for free fn handlers (back compat).
impl CommandHandler for fn(&mut Workspace, Command) -> bool {
    fn handle(&mut self, ws: &mut Workspace, cmd: Command) -> bool {
        self(ws, cmd)
    }
}

/// Handler for tab and git commands. Full impl extracted (SRP).
/// Does the real work (moved/adapted from Workspace::handle_tab_and_git).
pub fn handle_tab_commands(ws: &mut Workspace, cmd: Command) -> bool {
    match cmd {
        Command::NewTab => {
            match ws.active {
                crate::workspace::ActivePanel::Left => ws.left.duplicate_active(),
                crate::workspace::ActivePanel::Right => ws.right.duplicate_active(),
            }
            true
        }
        Command::CloseTab => {
            match ws.active {
                crate::workspace::ActivePanel::Left => ws.left.close_tab(ws.left.active),
                crate::workspace::ActivePanel::Right => ws.right.close_tab(ws.right.active),
            }
            true
        }
        Command::NextTab => {
            let side = match ws.active {
                crate::workspace::ActivePanel::Left => &mut ws.left,
                crate::workspace::ActivePanel::Right => &mut ws.right,
            };
            if !side.tabs.is_empty() {
                side.set_active((side.active + 1) % side.tabs.len());
            }
            true
        }
        Command::PrevTab => {
            let side = match ws.active {
                crate::workspace::ActivePanel::Left => &mut ws.left,
                crate::workspace::ActivePanel::Right => &mut ws.right,
            };
            if !side.tabs.is_empty() {
                side.set_active((side.active + side.tabs.len() - 1) % side.tabs.len());
            }
            true
        }
        Command::SaveTabSet => {
            crate::workspace::tab_sets::save_tab_set(ws);
            true
        }
        Command::RestoreTabSet => {
            crate::workspace::tab_sets::restore_tab_set(ws);
            true
        }
        Command::ToggleShowGit => {
            ws.requests.toggle_show_git_request = true;
            true
        }
        Command::OpenTerminal => {
            let dir = ws.active_panel_ref().current_path().clone();
            let _ = std::process::Command::new("open")
                .arg("-a")
                .arg("Terminal")
                .arg(&dir)
                .spawn();
            true
        }
        Command::GitDiff => {
            let panel = ws.active_panel_ref();
            if let Some(entry) = panel.filtered_get(panel.cursor().saturating_sub(1)) {
                let dir = panel.current_path().clone();
                let _ = std::process::Command::new("git")
                    .arg("-C").arg(&dir)
                    .arg("diff")
                    .arg("--").arg(&entry.path)
                    .spawn();
                ws.requests.git_toast = Some("Opened git diff".to_string());
            }
            true
        }
        Command::GitStage => {
            let paths: Vec<_> = ws.active_panel_ref().selected_or_cursor().into_iter().map(|e| e.path).collect();
            if !paths.is_empty() {
                let dir = ws.active_panel_ref().current_path().clone();
                let _ = std::process::Command::new("git")
                    .arg("-C").arg(&dir)
                    .arg("add")
                    .arg("--")
                    .args(&paths)
                    .status();
                ws.active_panel().refresh();
                ws.requests.git_toast = Some(format!("Staged {} file(s)", paths.len()));
            }
            true
        }
        Command::GitDiscard => {
            let paths: Vec<_> = ws.active_panel_ref().selected_or_cursor().into_iter().map(|e| e.path).collect();
            if !paths.is_empty() {
                let dir = ws.active_panel_ref().current_path().clone();
                let _ = std::process::Command::new("git")
                    .arg("-C").arg(&dir)
                    .arg("checkout")
                    .arg("--")
                    .args(&paths)
                    .spawn();
                ws.active_panel().refresh();
                ws.requests.git_toast = Some(format!("Discarded changes for {} file(s)", paths.len()));
            }
            true
        }
        Command::ShowPermissions => {
            let panel = ws.active_panel_ref();
            if let Some(entry) = panel.filtered_get(panel.cursor().saturating_sub(1)) {
                ws.requests.git_toast = Some(format!("Perms: {} (rwx stub, idea #83)", entry.name));
            }
            true
        }
        Command::BrowseArchive => {
            let panel = ws.active_panel_ref();
            if let Some(entry) = panel.filtered_get(panel.cursor().saturating_sub(1)) {
                let n = entry.name.to_lowercase();
                if n.ends_with(".zip") || n.ends_with(".tar") || n.ends_with(".tgz") || n.ends_with(".tar.gz") {
                    ws.requests.git_toast = Some(format!("Archive browse stub: {}", entry.name));
                } else {
                    ws.requests.git_toast = Some("Not an archive (stub)".into());
                }
            }
            true
        }
        _ => false,
    }
}

/// Handler for navigation commands (cursor movement).
pub fn handle_nav_commands(ws: &mut Workspace, cmd: Command) -> bool {
    match cmd {
        Command::CursorUp => {
            let panel = ws.active_panel();
            if panel.cursor() > 0 {
                panel.set_cursor(panel.cursor() - 1);
                panel.set_scroll_to_cursor(true);
            }
            true
        }
        Command::CursorDown => {
            let panel = ws.active_panel();
            let max = panel.filtered_count();
            if panel.cursor() < max {
                panel.set_cursor(panel.cursor() + 1);
                panel.set_scroll_to_cursor(true);
            }
            true
        }
        Command::CursorHome => {
            let panel = ws.active_panel();
            panel.set_cursor_and_scroll(0);
            true
        }
        Command::CursorEnd => {
            let panel = ws.active_panel();
            panel.set_cursor_and_scroll(panel.filtered_count());
            true
        }
        Command::CursorPageUp => {
            let panel = ws.active_panel();
            let page = panel.page_rows().max(1);
            let newc = panel.cursor().saturating_sub(page);
            panel.set_cursor_and_scroll(newc);
            true
        }
        Command::CursorPageDown => {
            let panel = ws.active_panel();
            let page = panel.page_rows().max(1);
            let max = panel.filtered_count();
            let newc = (panel.cursor() + page).min(max);
            panel.set_cursor_and_scroll(newc);
            true
        }
        _ => false,
    }
}

/// Handler for selection commands.
pub fn handle_selection_commands(ws: &mut Workspace, cmd: Command) -> bool {
    match cmd {
        Command::ExtendSelectDown => {
            let panel = ws.active_panel();
            panel.select_cursor();
            let max = panel.filtered_count();
            if panel.cursor() < max {
                panel.set_cursor(panel.cursor() + 1);
            }
            panel.select_cursor();
            panel.set_scroll_to_cursor(true);
            true
        }
        Command::ExtendSelectUp => {
            let panel = ws.active_panel();
            panel.select_cursor();
            if panel.cursor() > 1 {
                panel.set_cursor(panel.cursor() - 1);
            }
            panel.select_cursor();
            panel.set_scroll_to_cursor(true);
            true
        }
        _ => false,
    }
}

// More handlers can be added. Dispatch is currently explicit in workspace::execute.

