//! Selection logic extracted for SRP.
//! Methods for selecting entries, mask, etc. Pure-ish operations on PanelState.

use std::path::PathBuf;

use super::{parse_mask, term_matches, FileEntry, PanelState};

pub fn toggle_select(panel: &mut PanelState, path: PathBuf) {
    if !panel.selected.remove(&path) {
        panel.selected.insert(path);
    }
}

pub fn select_all(panel: &mut PanelState) {
    let all: Vec<PathBuf> = panel
        .filtered_entries()
        .iter()
        .map(|e| e.path.clone())
        .collect();
    let all_selected =
        panel.selected.len() == all.len() && all.iter().all(|p| panel.selected.contains(p));
    if all_selected {
        panel.selected.clear();
    } else {
        panel.selected = all.into_iter().collect();
    }
}

/// Flip selection membership across the filtered view.
pub fn invert_selection(panel: &mut PanelState) {
    let paths: Vec<PathBuf> = panel
        .filtered_entries()
        .iter()
        .map(|e| e.path.clone())
        .collect();
    for p in paths {
        if !panel.selected.remove(&p) {
            panel.selected.insert(p);
        }
    }
}

/// Add `paths` to the current selection.
pub fn extend_selection(panel: &mut PanelState, paths: impl IntoIterator<Item = PathBuf>) {
    panel.selected.extend(paths);
}

pub fn selected_entries(panel: &PanelState) -> Vec<FileEntry> {
    panel.filtered_entries()
        .into_iter()
        .filter(|e| panel.selected.contains(&e.path))
        .cloned()
        .collect()
}

pub fn selected_or_cursor(panel: &PanelState) -> Vec<FileEntry> {
    if panel.selected().is_empty() {
        if panel.cursor() == 0 {
            return vec![];
        }
        match panel.filtered_get(panel.cursor() - 1) {
            Some(entry) => vec![entry.clone()],
            None => vec![],
        }
    } else {
        selected_entries(panel)
    }
}

pub fn total_size_selected(panel: &PanelState) -> u64 {
    let sizes = panel.dir_sizes.lock().ok();
    selected_entries(panel)
        .iter()
        .map(|e| {
            if e.is_dir {
                sizes
                    .as_ref()
                    .and_then(|s| s.get(&e.path).copied())
                    .unwrap_or(0)
            } else {
                e.size
            }
        })
        .sum()
}

pub fn total_dir_size(panel: &PanelState) -> Option<u64> {
    let sizes = panel.dir_sizes.lock().ok()?;
    let dir_count = panel.entries.iter().filter(|e| e.is_dir).count();
    let computed = panel
        .entries
        .iter()
        .filter(|e| e.is_dir)
        .filter_map(|e| sizes.get(&e.path))
        .count();
    if computed == 0 && dir_count > 0 {
        return None;
    }
    let file_total: u64 = panel
        .entries
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.size)
        .sum();
    let dir_total: u64 = panel
        .entries
        .iter()
        .filter(|e| e.is_dir)
        .filter_map(|e| sizes.get(&e.path).copied())
        .sum();
    Some(file_total + dir_total)
}

pub fn select_by_mask(panel: &mut PanelState, mask: &str) -> usize {
    let terms = parse_mask(mask);
    if terms.is_empty() {
        return 0;
    }
    let mut decisions: Vec<(PathBuf, bool)> = Vec::new();
    for e in panel.filtered_entries() {
        let add = terms
            .iter()
            .any(|(t, sub)| !sub && term_matches(t, &e.name_lower, &e.extension));
        let rem = terms
            .iter()
            .any(|(t, sub)| *sub && term_matches(t, &e.name_lower, &e.extension));
        if rem {
            decisions.push((e.path.clone(), false));
        } else if add {
            decisions.push((e.path.clone(), true));
        }
    }
    let mut added = 0;
    for (path, is_add) in decisions {
        if is_add {
            panel.selected.insert(path);
            added += 1;
        } else {
            panel.selected.remove(&path);
        }
    }
    added
}

pub fn mask_match_count(panel: &PanelState, mask: &str) -> usize {
    let terms = parse_mask(mask);
    if terms.is_empty() {
        return 0;
    }
    panel.filtered_entries()
        .iter()
        .filter(|e| {
            terms
                .iter()
                .any(|(t, _)| term_matches(t, &e.name_lower, &e.extension))
        })
        .count()
}

pub fn select_cursor(panel: &mut PanelState) {
    if panel.cursor() == 0 {
        return;
    }
    if let Some(path) = panel.filtered_get(panel.cursor() - 1).map(|e| e.path.clone()) {
        panel.selected.insert(path);
    }
}