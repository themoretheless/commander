//! Each visible pane is the active slot; inactive slots own suspended panels.
use super::{ActivePanel, PanelState, Workspace};
use crate::panel::ViewConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MAX_TABS: usize = 8;
pub const MAX_CLOSED_TABS: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "pinned_path")]
    pub pinned_path: Option<PathBuf>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_title"
    )]
    pub title: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "is_false",
        deserialize_with = "deserialize_protection"
    )]
    pub close_protected: bool,
    #[serde(
        default,
        skip_serializing_if = "is_false",
        deserialize_with = "deserialize_protection"
    )]
    pub full_path_label: bool,
    #[serde(default)]
    pub view: ViewConfig,
    #[serde(default, skip_serializing_if = "crate::panel::MillerWidths::is_empty")]
    pub miller_widths: crate::panel::MillerWidths,
    #[serde(
        default,
        skip_serializing_if = "crate::selection_snapshot::Snapshot::is_none"
    )]
    pub selection_snapshot: crate::selection_snapshot::Snapshot,
}

// Optional pinned roots retain native identity and never trigger filesystem IO.
mod pinned_path {
    use super::*;
    pub(super) fn valid(path: &std::path::Path) -> bool {
        path.is_absolute()
            && path.as_os_str().as_encoded_bytes().len() <= 16 * 1024
            && !path.as_os_str().as_encoded_bytes().contains(&0)
    }
    pub fn serialize<S: serde::Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let value = path.as_ref().filter(|p| valid(p)).map(|path| {
            #[cfg(unix)]
            let native = {
                use std::os::unix::ffi::OsStrExt;
                path.as_os_str().as_bytes().to_vec()
            };
            #[cfg(windows)]
            let native = {
                use std::os::windows::ffi::OsStrExt;
                path.as_os_str().encode_wide().collect::<Vec<_>>()
            };
            serde_json::json!({"platform":std::env::consts::FAMILY,"path":native})
        });
        value.serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value["platform"].as_str() != Some(std::env::consts::FAMILY) {
            return Ok(None);
        }
        let Some(units) = value["path"].as_array().filter(|a| a.len() <= 16 * 1024) else {
            return Ok(None);
        };
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStringExt;
            let Ok(bytes) =
                serde_json::from_value::<Vec<u8>>(serde_json::Value::Array(units.clone()))
            else {
                return Ok(None);
            };
            PathBuf::from(std::ffi::OsString::from_vec(bytes))
        };
        #[cfg(windows)]
        let path = {
            use std::os::windows::ffi::OsStringExt;
            let Ok(units) =
                serde_json::from_value::<Vec<u16>>(serde_json::Value::Array(units.clone()))
            else {
                return Ok(None);
            };
            PathBuf::from(std::ffi::OsString::from_wide(&units))
        };
        Ok(valid(&path).then_some(path))
    }
}

fn is_false(value: &bool) -> bool {
    !value
}
fn deserialize_protection<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(false))
}

pub fn valid_tab_title(title: &str) -> bool {
    title.trim().chars().count() <= 64 && !title.chars().any(char::is_control)
}

fn clean_title(title: &str) -> Option<String> {
    if !valid_tab_title(title) {
        return None;
    }
    let title = title.trim();
    (!title.is_empty() && valid_tab_title(title)).then(|| title.to_string())
}

fn deserialize_title<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let title = serde_json::Value::deserialize(deserializer)?;
    Ok(title.as_str().and_then(clean_title))
}

impl SavedTab {
    pub fn display_title(&self) -> std::borrow::Cow<'_, str> {
        if self.full_path_label {
            self.path.to_string_lossy()
        } else if let Some(title) = &self.title {
            std::borrow::Cow::Borrowed(title)
        } else {
            self.path
                .file_name()
                .unwrap_or(self.path.as_os_str())
                .to_string_lossy()
        }
    }

    fn from_panel(panel: &PanelState) -> Self {
        Self {
            path: panel.current_path.clone(),
            pinned_path: panel.tab_pinned_path.clone(),
            title: panel.tab_title.clone(),
            close_protected: panel.tab_close_protected,
            full_path_label: panel.tab_full_path_label,
            view: panel.view_config(),
            miller_widths: panel.miller_width_snapshot(),
            selection_snapshot: panel.tab_selection.clone(),
        }
    }

    fn panel(&self) -> PanelState {
        let mut panel = PanelState::new_with_view(self.path.clone(), self.view);
        panel.restore_miller_widths(self.miller_widths.clone());
        panel.tab_selection = self.selection_snapshot.clone();
        panel.tab_pinned_path = self.pinned_path.clone().filter(|p| pinned_path::valid(p));
        panel.tab_title = self.title.as_deref().and_then(clean_title);
        panel.tab_close_protected = self.close_protected;
        panel.tab_full_path_label = self.full_path_label;
        panel
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SavedClosedTab {
    pub tab: SavedTab,
    pub position: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SavedTabs {
    pub entries: Vec<SavedTab>,
    pub active: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<SavedClosedTab>,
}

impl<'de> Deserialize<'de> for SavedTabs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let mut saved = Self::default();
        let requested = value
            .get("active")
            .and_then(|v| v.as_u64())
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0);
        if let Some(entries) = value.get("entries").and_then(|v| v.as_array()) {
            for (index, entry) in entries.iter().enumerate() {
                if saved.entries.len() == MAX_TABS {
                    break;
                }
                if let Ok(tab) = serde_json::from_value::<SavedTab>(entry.clone())
                    && tab.path.is_absolute()
                {
                    if index == requested {
                        saved.active = saved.entries.len();
                    }
                    saved.entries.push(tab);
                }
            }
        }
        if let Some(rows) = value.get("closed").and_then(|v| v.as_array()) {
            // Recent valid entries win, without inspecting paths on disk.
            for row in rows.iter().rev().take(256) {
                if saved.closed.len() == MAX_CLOSED_TABS {
                    break;
                }
                if let Some(tab) = row
                    .get("tab")
                    .and_then(|v| serde_json::from_value::<SavedTab>(v.clone()).ok())
                    && tab.path.is_absolute()
                {
                    let position = row
                        .get("position")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                        .min(MAX_TABS as u64) as usize;
                    saved.closed.push(SavedClosedTab { tab, position });
                }
            }
            saved.closed.reverse();
        }
        Ok(saved)
    }
}

/// Exact-path duplicate choices: retain every protected tab; otherwise prefer a
/// custom title, the active tab among ties, then the leftmost equal-priority tab.
impl SavedTabs {
    /// Count unprotected peers on each side of an anchor, excluding the anchor.
    pub fn closable_sides(&self, anchor: usize) -> (usize, usize) {
        if anchor >= self.entries.len() {
            return (0, 0);
        }
        let left = self.entries[..anchor]
            .iter()
            .filter(|tab| !tab.close_protected)
            .count();
        let right = self.entries[anchor + 1..]
            .iter()
            .filter(|tab| !tab.close_protected)
            .count();
        (left, right)
    }
}

fn duplicate_removals(saved: &SavedTabs) -> Vec<usize> {
    let mut groups: std::collections::HashMap<&PathBuf, Vec<usize>> =
        std::collections::HashMap::new();
    for (index, tab) in saved.entries.iter().enumerate() {
        groups.entry(&tab.path).or_default().push(index);
    }
    let mut removals = Vec::new();
    for indices in groups.values() {
        if indices
            .iter()
            .any(|&index| saved.entries[index].close_protected)
        {
            removals.extend(
                indices
                    .iter()
                    .copied()
                    .filter(|&index| !saved.entries[index].close_protected),
            );
        } else {
            let winner = indices
                .iter()
                .copied()
                .max_by_key(|&index| {
                    (
                        saved.entries[index].title.is_some(),
                        index == saved.active,
                        std::cmp::Reverse(index),
                    )
                })
                .unwrap();
            removals.extend(indices.iter().copied().filter(|&index| index != winner));
        }
    }
    removals.sort_unstable();
    removals
}

struct ClosedTab {
    panel: PanelState,
    position: usize,
}

pub(super) struct PaneTabs {
    slots: Vec<Option<PanelState>>,
    active: usize,
    closed: Vec<ClosedTab>,
}

impl Default for PaneTabs {
    fn default() -> Self {
        Self {
            slots: vec![None],
            active: 0,
            closed: Vec::new(),
        }
    }
}

impl PaneTabs {
    fn remember_closed(&mut self, panel: PanelState, position: usize) {
        if self.closed.len() == MAX_CLOSED_TABS {
            self.closed.remove(0);
        }
        self.closed.push(ClosedTab { panel, position });
    }

    fn activate(&mut self, visible: &mut PanelState, index: usize) -> bool {
        if index == self.active || index >= self.slots.len() {
            return false;
        }
        let Some(next) = self.slots[index].take() else {
            return false;
        };
        visible.suspend_for_tab();
        self.slots[self.active] = Some(std::mem::replace(visible, next));
        self.active = index;
        // The app wires the shared worker lane and watcher before reading rows.
        visible.prepare_tab_activation();
        true
    }

    fn saved(&self, visible: &PanelState) -> SavedTabs {
        SavedTabs {
            entries: self
                .slots
                .iter()
                .map(|slot| slot.as_ref().unwrap_or(visible))
                .map(SavedTab::from_panel)
                .collect(),
            active: self.active,
            closed: self
                .closed
                .iter()
                .skip(self.closed.len().saturating_sub(MAX_CLOSED_TABS))
                .map(|closed| SavedClosedTab {
                    tab: SavedTab::from_panel(&closed.panel),
                    position: closed.position,
                })
                .collect(),
        }
    }

    fn restore(&mut self, saved: &SavedTabs) {
        self.closed = saved
            .closed
            .iter()
            .skip(saved.closed.len().saturating_sub(MAX_CLOSED_TABS))
            .map(|closed| ClosedTab {
                panel: closed.tab.panel(),
                position: closed.position.min(MAX_TABS),
            })
            .collect();
        if saved.entries.is_empty() {
            return;
        }
        self.active = saved.active.min(saved.entries.len().min(MAX_TABS) - 1);
        self.slots = saved
            .entries
            .iter()
            .take(MAX_TABS)
            .enumerate()
            .map(|(index, tab)| (index != self.active).then(|| tab.panel()))
            .collect();
    }
}

impl Workspace {
    fn tabs_and_panel(&mut self, side: ActivePanel) -> (&mut PaneTabs, &mut PanelState) {
        match side {
            ActivePanel::Left => (&mut self.tabs[0], &mut self.left),
            ActivePanel::Right => (&mut self.tabs[1], &mut self.right),
        }
    }

    pub(crate) fn apply_profile_tab_layout(
        &mut self,
        layout: &crate::workspace_profile::TabLayout,
    ) -> Result<(), String> {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            if self
                .tab_snapshot(side)
                .entries
                .iter()
                .any(|tab| tab.close_protected)
            {
                return Err("Unprotect current tabs before replacing the tab set".into());
            }
        }
        let mut left = layout.left.clone();
        let mut right = layout.right.clone();
        left.closed = self.tab_snapshot(ActivePanel::Left).closed;
        right.closed = self.tab_snapshot(ActivePanel::Right).closed;
        let mut new_left = left.entries[left.active].panel();
        let mut new_right = right.entries[right.active].panel();
        if new_left.tab_pinned_path.is_some() {
            new_left.prepare_tab_activation();
        }
        if new_right.tab_pinned_path.is_some() {
            new_right.prepare_tab_activation();
        }
        self.tabs[0].restore(&left);
        self.tabs[1].restore(&right);
        self.left = new_left;
        self.right = new_right;
        self.active = if layout.active == 0 {
            ActivePanel::Left
        } else {
            ActivePanel::Right
        };
        self.reset_tab_presentation();
        Ok(())
    }

    /// Header-only snapshots: no filesystem work, inactive activation or history cloning.
    pub fn tab_choices(&self) -> Vec<crate::tab_search::Entry> {
        let mut entries = Vec::with_capacity(2 * MAX_TABS);
        for (side, tabs, visible) in [
            (ActivePanel::Left, &self.tabs[0], &self.left),
            (ActivePanel::Right, &self.tabs[1], &self.right),
        ] {
            for (index, slot) in tabs.slots.iter().enumerate() {
                let p = slot.as_ref().unwrap_or(visible);
                entries.push(crate::tab_search::Entry {
                    side,
                    index,
                    revision: self.tab_revision(),
                    path: p.current_path.clone(),
                    name: p.tab_title.clone().unwrap_or_else(|| {
                        p.current_path
                            .file_name()
                            .unwrap_or(p.current_path.as_os_str())
                            .to_string_lossy()
                            .into_owned()
                    }),
                    current: index == tabs.active,
                    focused: index == tabs.active && self.active == side,
                    protected: p.tab_close_protected,
                    pin: p.tab_pinned_path.clone(),
                });
            }
        }
        entries
    }

    pub fn activate_tab_captured(
        &mut self,
        side: ActivePanel,
        index: usize,
        revision: u64,
        path: &std::path::Path,
    ) -> bool {
        if !self.tabs_can_change()
            || self.tab_revision() != revision
            || self
                .tab_snapshot(side)
                .entries
                .get(index)
                .is_none_or(|tab| tab.path != path)
        {
            return false;
        }
        if self.active_tab_index(side) != index && !self.select_tab(side, index) {
            return false;
        }
        self.active = side;
        true
    }

    pub fn tab_snapshot(&self, side: ActivePanel) -> SavedTabs {
        match side {
            ActivePanel::Left => self.tabs[0].saved(&self.left),
            ActivePanel::Right => self.tabs[1].saved(&self.right),
        }
    }

    /// Set presentation metadata on a pointed tab without activating or reloading it.
    pub fn set_tab_title(&mut self, side: ActivePanel, index: usize, title: &str) -> bool {
        if !self.tabs_can_change() || index >= self.tab_count(side) || !valid_tab_title(title) {
            return false;
        }
        let title = clean_title(title);
        let (tabs, visible) = self.tabs_and_panel(side);
        let panel = tabs.slots[index].as_mut().unwrap_or(visible);
        if panel.tab_title == title {
            return false;
        }
        panel.tab_title = title;
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    /// Change a pointed tab's label style without activating or rescanning it.
    pub fn set_tab_full_path_label(
        &mut self,
        side: ActivePanel,
        index: usize,
        full_path: bool,
    ) -> bool {
        if !self.tabs_can_change() || index >= self.tab_count(side) {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        let panel = tabs.slots[index].as_mut().unwrap_or(visible);
        if panel.tab_full_path_label == full_path {
            return false;
        }
        panel.tab_full_path_label = full_path;
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    /// Pin the pointed tab's current native folder; ordinary navigation stays allowed.
    pub fn set_tab_pinned(&mut self, side: ActivePanel, index: usize, pinned: bool) -> bool {
        if !self.tabs_can_change() || index >= self.tab_count(side) {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        let panel = tabs.slots[index].as_mut().unwrap_or(visible);
        if pinned == panel.tab_pinned_path.is_some()
            || (pinned && !pinned_path::valid(&panel.current_path))
        {
            return false;
        }
        panel.tab_pinned_path = pinned.then(|| panel.current_path.clone());
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    pub fn set_tab_close_protection(
        &mut self,
        side: ActivePanel,
        index: usize,
        protect: bool,
    ) -> bool {
        if !self.tabs_can_change() || index >= self.tab_count(side) {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        let panel = tabs.slots[index].as_mut().unwrap_or(visible);
        if panel.tab_close_protected == protect {
            return false;
        }
        panel.tab_close_protected = protect;
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    pub fn tab_count(&self, side: ActivePanel) -> usize {
        self.tabs[if side == ActivePanel::Left { 0 } else { 1 }]
            .slots
            .len()
    }

    pub fn active_tab_index(&self, side: ActivePanel) -> usize {
        self.tabs[if side == ActivePanel::Left { 0 } else { 1 }].active
    }

    pub fn restore_tabs(&mut self, left: &SavedTabs, right: &SavedTabs) {
        let overridden = |saved: &SavedTabs, path: &std::path::Path| {
            saved
                .entries
                .get(
                    saved
                        .active
                        .min(saved.entries.len().saturating_sub(1))
                        .min(MAX_TABS - 1),
                )
                .is_some_and(|tab| tab.path != path)
        };
        self.restore_tabs_with_launch_overrides(
            left,
            right,
            [
                overridden(left, &self.left.current_path),
                overridden(right, &self.right.current_path),
            ],
        );
    }

    pub(crate) fn restore_tabs_with_launch_overrides(
        &mut self,
        left: &SavedTabs,
        right: &SavedTabs,
        launch_overrides: [bool; 2],
    ) {
        // Visible paths have already been sanitized and may be overridden by CLI.
        self.tabs[0].restore(left);
        self.tabs[1].restore(right);
        for (side, (saved, panel)) in [(left, &mut self.left), (right, &mut self.right)]
            .into_iter()
            .enumerate()
        {
            if saved.entries.is_empty() {
                continue;
            }
            if let Some(tab) = saved
                .entries
                .get(saved.active.min(saved.entries.len() - 1).min(MAX_TABS - 1))
                .filter(|tab| {
                    tab.path == panel.current_path
                        || (!launch_overrides[side] && tab.pinned_path.is_some())
                })
            {
                panel.restore_miller_widths(tab.miller_widths.clone());
                panel.tab_selection = tab.selection_snapshot.clone();
                panel.tab_pinned_path = tab.pinned_path.clone().filter(|p| pinned_path::valid(p));
            }
            panel.tab_title = saved
                .entries
                .get(saved.active.min(saved.entries.len() - 1).min(MAX_TABS - 1))
                .filter(|tab| {
                    tab.path == panel.current_path
                        || (!launch_overrides[side] && tab.pinned_path.is_some())
                })
                .and_then(|tab| tab.title.as_deref())
                .and_then(clean_title);
            panel.tab_close_protected = saved
                .entries
                .get(saved.active.min(saved.entries.len() - 1).min(MAX_TABS - 1))
                .is_some_and(|tab| {
                    (tab.path == panel.current_path
                        || (!launch_overrides[side] && tab.pinned_path.is_some()))
                        && tab.close_protected
                });
            panel.tab_full_path_label = saved
                .entries
                .get(saved.active.min(saved.entries.len() - 1).min(MAX_TABS - 1))
                .is_some_and(|tab| {
                    (tab.path == panel.current_path
                        || (!launch_overrides[side] && tab.pinned_path.is_some()))
                        && tab.full_path_label
                });
            if panel.tab_pinned_path.is_some() && !launch_overrides[side] {
                panel.prepare_tab_activation();
            }
        }
    }

    fn tabs_can_change(&self) -> bool {
        self.pending_op.is_none() && !self.deletes.is_active() && !self.has_any_pending_ui_modal()
    }

    pub fn tab_revision(&self) -> u64 {
        self.tab_revision
    }

    fn reset_tab_presentation(&mut self) {
        self.tab_revision = self.tab_revision.wrapping_add(1);
        self.left.preview = None;
        self.right.preview = None;
        self.preview_declined = None;
        self.left.drag.clear();
        self.right.drag.clear();
        *self.command_capabilities.borrow_mut() = None;
    }

    pub fn select_tab(&mut self, side: ActivePanel, index: usize) -> bool {
        if !self.tabs_can_change() {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        if !tabs.activate(visible, index) {
            return false;
        }
        self.reset_tab_presentation();
        true
    }

    /// Open an existing favorite without consulting or modifying the cursor selection.
    /// Uses the same pending-operation/modal gate and capacity owner as normal tabs.
    pub fn open_favorite(&mut self, path: &std::path::Path, target: super::FavoriteTarget) -> bool {
        self.open_favorite_at(self.active, path, target)
    }
    /// Bind a picker action to the panel that opened it, even if active focus changed.
    pub fn open_favorite_at(
        &mut self,
        source: ActivePanel,
        path: &std::path::Path,
        target: super::FavoriteTarget,
    ) -> bool {
        if !self.tabs_can_change() || !path.is_absolute() || !self.bookmarks.contains(path) {
            return false;
        }
        match target {
            super::FavoriteTarget::NewTab => self.new_tab(source, Some(path.to_path_buf())),
            super::FavoriteTarget::BackgroundTab => {
                self.new_background_tab(source, path.to_path_buf())
            }
            super::FavoriteTarget::CurrentPanel | super::FavoriteTarget::OtherPanel => {
                let destination = if target == super::FavoriteTarget::CurrentPanel {
                    source
                } else {
                    source.other()
                };
                let (_, panel) = self.tabs_and_panel(destination);
                if panel.current_path != path {
                    panel.preview = None;
                    panel.navigate_to(path.to_path_buf());
                    self.tab_revision = self.tab_revision.wrapping_add(1);
                    self.preview_declined = None;
                    self.left.drag.clear();
                    self.right.drag.clear();
                    *self.command_capabilities.borrow_mut() = None;
                }
                true
            }
        }
    }

    pub fn new_tab(&mut self, side: ActivePanel, path: Option<PathBuf>) -> bool {
        if !self.tabs_can_change() || self.tab_count(side) >= MAX_TABS {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        let path = path.unwrap_or_else(|| visible.current_path.clone());
        let next = PanelState::new_with_view(path, visible.view_config());
        let index = tabs.active + 1;
        tabs.slots.insert(index, Some(next));
        tabs.activate(visible, index);
        self.reset_tab_presentation();
        true
    }

    /// Open only selected visible directories, in display order, as one lazy group.
    pub fn open_selected_folders_background(&mut self) -> Result<usize, String> {
        if !self.tabs_can_change() {
            return Err("Finish the pending operation before opening folder tabs".into());
        }
        let source = self.active;
        let remaining = MAX_TABS.saturating_sub(self.tab_count(source));
        let active = self.active_panel_ref();
        let mut folders = Vec::new();
        active.visit_filtered(|_, entry| {
            if entry.is_dir && active.is_selected(&entry.path) {
                folders.push(entry.path.clone());
            }
            folders.len() <= remaining
        });
        if folders.is_empty() {
            return Err("Select at least one visible folder".into());
        }
        if folders.len() > MAX_TABS.saturating_sub(self.tab_count(source)) {
            return Err(
                "Not enough free tabs for all selected visible folders; no tabs opened".into(),
            );
        }
        let mut seen = std::collections::HashSet::new();
        if folders
            .iter()
            .any(|path| !path.is_absolute() || !seen.insert(path.clone()))
        {
            return Err("Folder selection changed; refresh and select the folders again".into());
        }
        let count = folders.len();
        let (tabs, visible) = self.tabs_and_panel(source);
        let config = visible.view_config();
        let prepared = folders
            .into_iter()
            .map(|path| Some(PanelState::new_with_view(path, config)))
            .collect::<Vec<_>>();
        tabs.slots
            .splice(tabs.active + 1..tabs.active + 1, prepared);
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        Ok(count)
    }

    /// Validate a captured current-list folder or its synthetic parent row.
    pub(crate) fn open_listed_folder_tab(
        &mut self,
        side: ActivePanel,
        owner: &std::path::Path,
        generation: u64,
        folder: PathBuf,
        activate: bool,
    ) -> bool {
        let panel = match side {
            ActivePanel::Left => &self.left,
            ActivePanel::Right => &self.right,
        };
        if panel.current_path != owner
            || panel.entries_gen() != generation
            || !(folder.is_absolute() && panel.current_path.parent() == Some(folder.as_path())
                || panel.filtered_indices().iter().any(|&index| {
                    panel
                        .entries()
                        .get(index)
                        .is_some_and(|entry| entry.is_dir && entry.path == folder)
                }))
        {
            return false;
        }
        if activate {
            self.new_tab(side, Some(folder))
        } else {
            self.new_background_tab(side, folder)
        }
    }

    /// Only a currently displayed Miller ancestor directory may request a tab.
    #[cfg(test)]
    fn open_miller_background_tab(
        &mut self,
        side: ActivePanel,
        owner: &std::path::Path,
        generation: u64,
        folder: PathBuf,
    ) -> bool {
        self.open_miller_tab(side, owner, generation, folder, false)
    }
    pub(crate) fn open_miller_tab(
        &mut self,
        side: ActivePanel,
        owner: &std::path::Path,
        generation: u64,
        folder: PathBuf,
        activate: bool,
    ) -> bool {
        let panel = match side {
            ActivePanel::Left => &self.left,
            ActivePanel::Right => &self.right,
        };
        if panel.current_path != owner
            || panel.entries_gen() != generation
            || panel.browser_mode() != crate::panel::BrowserMode::Miller
            || !panel.miller.columns.iter().any(|column| {
                column.current_path == folder
                    || column
                        .entries()
                        .iter()
                        .any(|entry| entry.is_dir && entry.path == folder)
            })
        {
            return false;
        }
        if activate {
            self.new_tab(side, Some(folder))
        } else {
            self.new_background_tab(side, folder)
        }
    }

    /// Insert beside the current tab, leaving its listing, watcher and preview live.
    pub fn new_background_tab(&mut self, side: ActivePanel, path: PathBuf) -> bool {
        self.insert_background_tab(side, path, None)
    }

    /// Reveal a target in a lazy background tab without moving the live pane.
    pub fn reveal_in_background_tab(&mut self, side: ActivePanel, target: PathBuf) -> bool {
        let Some(parent) = target.parent().map(std::path::Path::to_path_buf) else {
            return false;
        };
        let Some(name) = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            return false;
        };
        self.insert_background_tab(side, parent, Some(name))
    }

    /// Open an entire result group, preserving order and rejecting it before mutation
    /// if any target is invalid, an operation blocks tab changes, or capacity is short.
    pub fn reveal_in_background_tabs(&mut self, side: ActivePanel, targets: Vec<PathBuf>) -> bool {
        if targets.is_empty()
            || !self.tabs_can_change()
            || targets.len() > MAX_TABS.saturating_sub(self.tab_count(side))
        {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        let prepared: Option<Vec<_>> = targets
            .into_iter()
            .map(|target| {
                let parent = target.parent()?.to_path_buf();
                let name = target.file_name()?.to_string_lossy().into_owned();
                let config = if name.starts_with('.') {
                    visible.view_config().with_show_hidden(true)
                } else {
                    visible.view_config()
                };
                let mut panel = PanelState::new_with_view(parent, config);
                panel.queue_initial_focus_child(name);
                Some(Some(panel))
            })
            .collect();
        let Some(prepared) = prepared else {
            return false;
        };
        let index = tabs.active + 1;
        tabs.slots.splice(index..index, prepared);
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    fn insert_background_tab(
        &mut self,
        side: ActivePanel,
        path: PathBuf,
        focus: Option<String>,
    ) -> bool {
        if !self.tabs_can_change() || self.tab_count(side) >= MAX_TABS {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        let config = if focus.as_ref().is_some_and(|name| name.starts_with('.')) {
            visible.view_config().with_show_hidden(true)
        } else {
            visible.view_config()
        };
        let mut next = PanelState::new_with_view(path, config);
        if let Some(name) = focus {
            next.queue_initial_focus_child(name);
        }
        tabs.slots.insert(tabs.active + 1, Some(next));
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    /// Copy a pointed tab's path/view beside it, leaving the visible panel live.
    pub fn duplicate_tab_at(&mut self, side: ActivePanel, index: usize) -> bool {
        if !self.tabs_can_change() || self.tab_count(side) >= MAX_TABS {
            return false;
        }
        let saved = self.tab_snapshot(side);
        let Some(source) = saved.entries.get(index) else {
            return false;
        };
        let panel = source.panel();
        let (tabs, _) = self.tabs_and_panel(side);
        let position = index + 1;
        tabs.slots.insert(position, Some(panel));
        if position <= tabs.active {
            tabs.active += 1;
        }
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    /// Move the owned runtime panel into the other pane's background tab group.
    pub fn move_tab_to_other(&mut self, source: ActivePanel, index: usize) -> bool {
        let position = self.active_tab_index(source.other()) + 1;
        self.move_tab_to_other_at(source, index, position)
    }
    /// Transfer the exact captured tab into a destination insertion boundary.
    pub(crate) fn transfer_tab_captured(
        &mut self,
        source: ActivePanel,
        index: usize,
        position: usize,
        revision: u64,
        path: &std::path::Path,
    ) -> bool {
        if self.tab_revision() != revision
            || self
                .tab_snapshot(source)
                .entries
                .get(index)
                .is_none_or(|tab| tab.path != path)
        {
            return false;
        }
        self.move_tab_to_other_at(source, index, position)
    }
    fn move_tab_to_other_at(&mut self, source: ActivePanel, index: usize, position: usize) -> bool {
        if !self.tabs_can_change()
            || self.tab_count(source) <= 1
            || index >= self.tab_count(source)
            || self.tab_count(source.other()) >= MAX_TABS
            || position > self.tab_count(source.other())
        {
            return false;
        }
        let visible_changed = index == self.active_tab_index(source);
        let (tabs, visible) = self.tabs_and_panel(source);
        if visible_changed {
            let next = if index > 0 { index - 1 } else { 1 };
            tabs.activate(visible, next);
        }
        let mut panel = tabs.slots.remove(index).expect("moved slot is suspended");
        if index < tabs.active {
            tabs.active -= 1;
        }
        panel.suspend_for_tab();
        let (destination, _) = self.tabs_and_panel(source.other());
        destination.slots.insert(position, Some(panel));
        if position <= destination.active {
            destination.active += 1;
        }
        if visible_changed {
            let (_, visible) = self.tabs_and_panel(source);
            visible.preview = None;
            self.preview_declined = None;
            self.left.drag.clear();
            self.right.drag.clear();
        }
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    pub fn can_copy_tabs_to_other(&self, source: ActivePanel) -> bool {
        self.tab_count(source)
            .saturating_add(self.tab_count(source.other()))
            <= MAX_TABS
    }

    /// Copy paths and view settings as a lazy group. Neither visible panel moves.
    pub fn copy_tabs_to_other(&mut self, source: ActivePanel) -> bool {
        if !self.tabs_can_change() || !self.can_copy_tabs_to_other(source) {
            return false;
        }
        let saved = self.tab_snapshot(source);
        let panels: Vec<_> = saved
            .entries
            .into_iter()
            .map(|tab| Some(tab.panel()))
            .collect();
        let (tabs, _) = self.tabs_and_panel(source.other());
        let position = tabs.active + 1;
        tabs.slots.splice(position..position, panels);
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    pub fn close_tab(&mut self, side: ActivePanel) -> bool {
        if !self.tabs_can_change()
            || self.tab_count(side) <= 1
            || self.tab_snapshot(side).entries[self.active_tab_index(side)].close_protected
        {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        let old = tabs.active;
        let next = if old > 0 { old - 1 } else { 1 };
        tabs.activate(visible, next);
        if let Some(panel) = tabs.slots.remove(old) {
            tabs.remember_closed(panel, old);
        }
        if tabs.active > old {
            tabs.active -= 1;
        }
        self.reset_tab_presentation();
        true
    }

    /// Close a specific slot without activating an inactive panel first.
    pub fn close_tab_at(&mut self, side: ActivePanel, index: usize) -> bool {
        if !self.tabs_can_change() || self.tab_count(side) <= 1 || index >= self.tab_count(side) {
            return false;
        }
        if self.tab_snapshot(side).entries[index].close_protected {
            return false;
        }
        if index == self.active_tab_index(side) {
            return self.close_tab(side);
        }
        let (tabs, _) = self.tabs_and_panel(side);
        let Some(panel) = tabs.slots.remove(index) else {
            unreachable!("inactive tab owns its panel");
        };
        tabs.remember_closed(panel, index);
        if index < tabs.active {
            tabs.active -= 1;
        }
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    /// Close a group relative to the menu's tab, without cycling through its peers.
    /// `others` keeps only the anchor; otherwise keep its entire left prefix.
    pub fn close_tab_group(&mut self, side: ActivePanel, anchor: usize, others: bool) -> bool {
        let count = self.tab_count(side);
        if !self.tabs_can_change()
            || anchor >= count
            || (if others {
                count <= 1
            } else {
                anchor + 1 == count
            })
        {
            return false;
        }
        let saved = self.tab_snapshot(side);
        let removable: Vec<_> = (0..count)
            .filter(|&index| {
                ((others && index != anchor) || (!others && index > anchor))
                    && !saved.entries[index].close_protected
            })
            .collect();
        if removable.is_empty() {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        if others || removable.contains(&tabs.active) {
            tabs.activate(visible, anchor);
        }
        // Descending removal preserves saved positions. LIFO reopen reconstructs
        // the original order, including tabs on both sides of the survivor.
        for index in removable.into_iter().rev() {
            {
                if let Some(panel) = tabs.slots.remove(index) {
                    tabs.remember_closed(panel, index);
                }
                if tabs.active > index {
                    tabs.active -= 1;
                }
            }
        }
        self.reset_tab_presentation();
        true
    }

    /// Close the prefix before a pointed tab; keep a surviving visible tab when possible.
    pub fn close_tabs_left(&mut self, side: ActivePanel, anchor: usize) -> bool {
        if !self.tabs_can_change() || anchor == 0 || anchor >= self.tab_count(side) {
            return false;
        }
        let saved = self.tab_snapshot(side);
        let removable: Vec<_> = (0..anchor)
            .filter(|&index| !saved.entries[index].close_protected)
            .collect();
        if removable.is_empty() {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        if removable.contains(&tabs.active) {
            tabs.activate(visible, anchor);
        }
        for index in removable.into_iter().rev() {
            let panel = tabs.slots.remove(index).expect("removed tab is inactive");
            tabs.remember_closed(panel, index);
            if tabs.active > index {
                tabs.active -= 1;
            }
        }
        self.reset_tab_presentation();
        true
    }

    /// Format the pointed tab rather than the visible panel or its selected entries.
    pub fn tab_path_for_copy(&self, side: ActivePanel, index: usize) -> Option<String> {
        let snapshot = self.tab_snapshot(side);
        let tab = snapshot.entries.get(index)?;
        Some(crate::clipboard::format(
            std::slice::from_ref(&tab.path),
            crate::clipboard::PathStyle::FullPath,
            None,
        ))
    }

    pub fn closed_tab_count(&self, side: ActivePanel) -> usize {
        self.tabs[if side == ActivePanel::Left { 0 } else { 1 }]
            .closed
            .len()
    }

    /// Oldest first, bounded to eight panels per pane; presentation persists in the session.
    #[cfg(any(test, feature = "visual-qa"))]
    pub fn closed_tab_paths(&self, side: ActivePanel) -> Vec<PathBuf> {
        self.tabs[if side == ActivePanel::Left { 0 } else { 1 }]
            .closed
            .iter()
            .map(|closed| closed.panel.current_path.clone())
            .collect()
    }

    /// Forget presentation history only; never switch panes or disturb live previews.
    pub fn forget_closed_tab_at(&mut self, side: ActivePanel, index: usize) -> bool {
        if !self.tabs_can_change() || index >= self.closed_tab_count(side) {
            return false;
        }
        self.tabs[if side == ActivePanel::Left { 0 } else { 1 }]
            .closed
            .remove(index);
        self.tab_revision = self.tab_revision.wrapping_add(1);
        true
    }

    pub fn clear_closed_tabs(&mut self, side: ActivePanel) -> bool {
        if !self.tabs_can_change() || self.closed_tab_count(side) == 0 {
            return false;
        }
        self.tabs[if side == ActivePanel::Left { 0 } else { 1 }]
            .closed
            .clear();
        self.tab_revision = self.tab_revision.wrapping_add(1);
        true
    }

    pub fn reopen_closed_tab(&mut self, side: ActivePanel) -> bool {
        let Some(index) = self.closed_tab_count(side).checked_sub(1) else {
            return false;
        };
        self.reopen_closed_tab_at(side, index)
    }

    pub fn reopen_closed_tab_at(&mut self, side: ActivePanel, closed_index: usize) -> bool {
        if !self.tabs_can_change()
            || self.tab_count(side) >= MAX_TABS
            || closed_index >= self.closed_tab_count(side)
        {
            return false;
        }
        let (tabs, visible) = self.tabs_and_panel(side);
        let closed = tabs.closed.remove(closed_index);
        let index = closed.position.min(tabs.slots.len());
        tabs.slots.insert(index, Some(closed.panel));
        if index <= tabs.active {
            tabs.active += 1;
        }
        tabs.activate(visible, index);
        self.reset_tab_presentation();
        true
    }

    /// A drag may only reorder the exact tab captured at gesture start.
    pub(crate) fn move_tab_captured(
        &mut self,
        side: ActivePanel,
        from: usize,
        to: usize,
        revision: u64,
        path: &std::path::Path,
    ) -> bool {
        if self.tab_revision() != revision
            || self
                .tab_snapshot(side)
                .entries
                .get(from)
                .is_none_or(|tab| tab.path != path)
        {
            return false;
        }
        self.move_tab(side, from, to)
    }

    /// Reorder slots without switching or suspending the visible panel.
    pub fn move_tab(&mut self, side: ActivePanel, from: usize, to: usize) -> bool {
        if !self.tabs_can_change()
            || from == to
            || from >= self.tab_count(side)
            || to >= self.tab_count(side)
        {
            return false;
        }
        let (tabs, _) = self.tabs_and_panel(side);
        let slot = tabs.slots.remove(from);
        tabs.slots.insert(to, slot);
        tabs.active = if tabs.active == from {
            to
        } else if from < tabs.active && tabs.active <= to {
            tabs.active - 1
        } else if to <= tabs.active && tabs.active < from {
            tabs.active + 1
        } else {
            tabs.active
        };
        self.tab_revision = self.tab_revision.wrapping_add(1);
        *self.command_capabilities.borrow_mut() = None;
        true
    }

    pub fn duplicate_tab_count(&self, side: ActivePanel) -> usize {
        duplicate_removals(&self.tab_snapshot(side)).len()
    }

    /// Preserve protected/named tabs and remember every discarded runtime panel.
    pub fn close_duplicate_tabs(&mut self, side: ActivePanel) -> bool {
        if !self.tabs_can_change() {
            return false;
        }
        let saved = self.tab_snapshot(side);
        let removals = duplicate_removals(&saved);
        if removals.is_empty() {
            return false;
        }
        let active_path = &saved.entries[saved.active].path;
        let survivor = saved
            .entries
            .iter()
            .enumerate()
            .filter(|(index, tab)| !removals.contains(index) && &tab.path == active_path)
            .max_by_key(|(index, tab)| {
                (
                    tab.close_protected,
                    tab.title.is_some(),
                    std::cmp::Reverse(*index),
                )
            })
            .map(|(index, _)| index)
            .unwrap();
        let (tabs, visible) = self.tabs_and_panel(side);
        if removals.contains(&tabs.active) {
            tabs.activate(visible, survivor);
        }
        for index in removals.into_iter().rev() {
            let panel = tabs
                .slots
                .remove(index)
                .expect("removed duplicate is inactive");
            tabs.remember_closed(panel, index);
            if tabs.active > index {
                tabs.active -= 1;
            }
        }
        self.reset_tab_presentation();
        true
    }

    pub fn step_tab(&mut self, side: ActivePanel, forward: bool) -> bool {
        let count = self.tab_count(side);
        let active = self.active_tab_index(side);
        let next = if forward {
            (active + 1) % count
        } else {
            (active + count - 1) % count
        };
        self.select_tab(side, next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    #[test]
    fn selected_folder_tabs_open_visible_directories_atomically_in_display_order() {
        use crate::command::{Command, availability};
        for mode in [
            crate::panel::BrowserMode::List,
            crate::panel::BrowserMode::Icons,
            crate::panel::BrowserMode::Miller,
        ] {
            let temp = TempDir::new();
            let two = temp.dir("folder-2");
            let ten = temp.dir("folder-10");
            let skip = temp.dir("skip");
            let other = temp.dir("other");
            let file = temp.file("file.txt", "keep");
            let mut ws = Workspace::new(other.clone(), temp.path().into());
            ws.active = ActivePanel::Right;
            ws.right.refresh();
            ws.right.set_browser_mode(mode);
            ws.right.focus_child("file.txt".into());
            for path in [&ten, &skip, &two, &file, &other.join("foreign")] {
                ws.right.toggle_select(path.clone());
            }
            ws.right.set_search_query("folder- | file");
            assert_eq!(ws.right.selected_entries().len(), 3);
            let before = ws.tab_snapshot(ActivePanel::Right).entries[0].clone();
            let revision = ws.right.entries_gen();
            let tab_revision = ws.tab_revision();
            let cursor = ws.right.cursor_entry().unwrap().path.clone();
            let selection = ws.right.selected_paths().clone();
            assert!(
                availability(
                    Command::OpenSelectedFoldersBackgroundTabs,
                    &ws.command_context()
                )
                .enabled
            );
            ws.execute(Command::OpenSelectedFoldersBackgroundTabs);
            assert!(ws.drain_ui_requests().is_empty());
            let tabs = ws.tab_snapshot(ActivePanel::Right);
            assert_eq!(tabs.active, 0);
            assert_eq!(tabs.entries.len(), 3);
            assert_eq!(tabs.entries[0], before);
            assert_eq!(tabs.entries[1].path, two);
            assert_eq!(tabs.entries[2].path, ten);
            assert_eq!(ws.tab_revision(), tab_revision.wrapping_add(1));
            assert_eq!(ws.right.entries_gen(), revision);
            assert_eq!(ws.right.cursor_entry().unwrap().path, cursor);
            assert_eq!(ws.right.selected_paths(), &selection);
            assert_eq!(ws.left.current_path, other);
            assert_eq!(ws.active, ActivePanel::Right);
            assert_eq!(ws.right.search_query(), "folder- | file");
            for index in [1, 2] {
                let pending = ws.tabs[1].slots[index].as_ref().unwrap();
                assert!(pending.entries().is_empty());
                assert!(!pending.has_notify());
                assert_eq!(pending.view_config(), before.view);
            }
        }
    }
    #[test]
    fn selected_folder_tabs_refuse_short_capacity_pending_modal_and_empty_folder_selection() {
        use crate::command::{Command, availability};
        let temp = TempDir::new();
        let a = temp.dir("A");
        let b = temp.dir("B");
        let file = temp.file("file.txt", "keep");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.left.refresh();
        ws.left.focus_child("A".into());
        assert!(
            !availability(
                Command::OpenSelectedFoldersBackgroundTabs,
                &ws.command_context()
            )
            .enabled
        );
        assert!(ws.open_selected_folders_background().is_err());
        ws.left.toggle_select(file.clone());
        assert!(
            !availability(
                Command::OpenSelectedFoldersBackgroundTabs,
                &ws.command_context()
            )
            .enabled
        );
        ws.left.toggle_select(a.clone());
        ws.left.toggle_select(b.clone());
        ws.emit_ui_request(crate::ui_request::UiRequest::Rename(file));
        assert!(ws.open_selected_folders_background().is_err());
        assert_eq!(ws.tab_count(ActivePanel::Left), 1);
        ws.drain_ui_requests();
        while ws.tab_count(ActivePanel::Left) < MAX_TABS - 1 {
            assert!(ws.new_background_tab(ActivePanel::Left, a.clone()));
        }
        let before = ws.tab_snapshot(ActivePanel::Left);
        let revision = ws.tab_revision();
        let disabled = availability(
            Command::OpenSelectedFoldersBackgroundTabs,
            &ws.command_context(),
        );
        assert!(!disabled.enabled);
        assert!(disabled.reason.unwrap().contains("free tabs"));
        assert!(ws.open_selected_folders_background().is_err());
        assert_eq!(ws.tab_revision(), revision);
        assert_eq!(ws.tab_snapshot(ActivePanel::Left), before);
        ws.execute(Command::OpenSelectedFoldersBackgroundTabs);
        assert!(matches!(
            &ws.drain_ui_requests()[..],
            [crate::ui_request::UiRequest::Notice { error: true, .. }]
        ));
        assert_eq!(ws.tab_snapshot(ActivePanel::Left), before);
    }
    #[test]
    fn open_tab_choices_cover_both_groups_without_loading_and_revalidate_before_activation() {
        let temp = TempDir::new();
        let a = temp.dir("A");
        let b = temp.dir("B");
        let mut ws = Workspace::new(a.clone(), a.clone());
        for side in [ActivePanel::Left, ActivePanel::Right] {
            while ws.tab_count(side) < MAX_TABS {
                assert!(ws.new_background_tab(side, b.clone()));
            }
        }
        assert!(ws.set_tab_title(ActivePanel::Right, 1, "Work"));
        assert!(ws.set_tab_close_protection(ActivePanel::Right, 1, true));
        assert!(ws.set_tab_pinned(ActivePanel::Right, 1, true));
        let revision = ws.tab_revision();
        let choices = ws.tab_choices();
        assert_eq!(choices.len(), 16);
        assert_eq!(choices.iter().filter(|e| e.current).count(), 2);
        assert_eq!(choices.iter().filter(|e| e.focused).count(), 1);
        assert_eq!(ws.tab_revision(), revision);
        let choice = choices
            .iter()
            .find(|e| e.side == ActivePanel::Right && e.index == 1)
            .unwrap();
        assert_eq!(choice.name, "Work");
        assert!(choice.protected);
        assert_eq!(choice.pin.as_ref(), Some(&b));
        assert!(ws.tabs[1].slots[1].as_ref().unwrap().entries().is_empty());
        assert!(!ws.tabs[1].slots[1].as_ref().unwrap().has_notify());
        assert!(ws.activate_tab_captured(choice.side, choice.index, choice.revision, &choice.path));
        assert_eq!(ws.active, ActivePanel::Right);
        assert_eq!(ws.right.current_path, b);
        assert_eq!(ws.active_tab_index(ActivePanel::Right), 1);
        let same = ws.tab_choices().into_iter().find(|e| e.focused).unwrap();
        let generation = ws.right.entries_gen();
        assert!(ws.activate_tab_captured(same.side, same.index, same.revision, &same.path));
        assert_eq!(ws.right.entries_gen(), generation);
        assert!(!ws.activate_tab_captured(
            choice.side,
            choice.index,
            choice.revision,
            &choice.path
        ));
        let current = ws.tab_choices()[0].clone();
        ws.left.navigate_to(b.clone());
        assert!(!ws.activate_tab_captured(
            current.side,
            current.index,
            current.revision,
            &current.path
        ));
        let current = ws.tab_choices()[0].clone();
        ws.emit_ui_request(crate::ui_request::UiRequest::Rename(b.clone()));
        assert!(!ws.activate_tab_captured(
            current.side,
            current.index,
            current.revision,
            &current.path
        ));
        ws.drain_ui_requests();
        assert!(ws.set_tab_title(ActivePanel::Left, 0, "Changed"));
        assert!(!ws.activate_tab_captured(
            current.side,
            current.index,
            current.revision,
            &current.path
        ));
    }
    #[test]
    fn open_tab_activation_distinguishes_same_path_slots_and_preserves_native_paths() {
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        assert!(ws.new_background_tab(ActivePanel::Right, temp.path().into()));
        let choices = ws.tab_choices();
        let duplicate = choices
            .iter()
            .find(|e| e.side == ActivePanel::Right && e.index == 1)
            .unwrap();
        assert!(ws.activate_tab_captured(
            duplicate.side,
            duplicate.index,
            duplicate.revision,
            &duplicate.path
        ));
        assert_eq!(ws.active_tab_index(ActivePanel::Right), 1);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let native = temp
                .path()
                .join(std::ffi::OsString::from_vec(vec![b'x', 255]));
            // A native path must survive even when the host filesystem rejects its bytes.
            assert!(ws.new_background_tab(ActivePanel::Left, native.clone()));
            let choice = ws
                .tab_choices()
                .into_iter()
                .find(|e| e.path == native)
                .unwrap();
            assert!(ws.activate_tab_captured(
                choice.side,
                choice.index,
                choice.revision,
                &choice.path
            ));
            assert_eq!(ws.left.current_path, native);
        }
    }

    #[test]
    fn parent_row_tabs_keep_source_state_and_activate_only_the_requested_destination() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            for mode in [
                crate::panel::BrowserMode::List,
                crate::panel::BrowserMode::Icons,
                crate::panel::BrowserMode::Miller,
            ] {
                for activate in [false, true] {
                    let temp = TempDir::new();
                    let owner = temp.dir("owner");
                    let other = temp.dir("other");
                    let keep = temp.file("owner/keep.txt", "kept");
                    let mut ws = if side == ActivePanel::Left {
                        Workspace::new(owner.clone(), other.clone())
                    } else {
                        Workspace::new(other.clone(), owner.clone())
                    };
                    {
                        let (_, p) = ws.tabs_and_panel(side);
                        p.refresh();
                        p.set_browser_mode(mode);
                        p.set_search_query("keep");
                        p.focus_child("keep.txt".into());
                        p.select_path(keep.clone());
                    }
                    assert!(ws.set_tab_title(side, 0, "Keep source"));
                    assert!(ws.set_tab_close_protection(side, 0, true));
                    ws.left.preview = Some(crate::panel::PreviewContent::Image(keep.clone()));
                    ws.right.preview = ws.left.preview.clone();
                    let before = ws.tab_snapshot(side);
                    let opposite = ws.tab_snapshot(side.other());
                    let generation = ws.tabs_and_panel(side).1.entries_gen();
                    ws.active = side.other();
                    assert!(ws.open_listed_folder_tab(
                        side,
                        &owner,
                        generation,
                        temp.path().into(),
                        activate
                    ));
                    assert_eq!(ws.active, side.other());
                    assert_eq!(ws.tab_count(side), 2);
                    assert_eq!(ws.active_tab_index(side), usize::from(activate));
                    assert_eq!(ws.tab_snapshot(side).entries[0], before.entries[0]);
                    assert_eq!(ws.tab_snapshot(side.other()), opposite);
                    if activate {
                        let (_, p) = ws.tabs_and_panel(side);
                        assert_eq!(p.current_path, temp.path());
                        assert!(p.entries().is_empty());
                        assert!(!p.has_notify());
                        assert!(ws.close_tab(side));
                        ws.tabs_and_panel(side).1.refresh();
                    } else {
                        assert!(ws.left.preview.is_some() && ws.right.preview.is_some());
                        let (tabs, p) = ws.tabs_and_panel(side);
                        assert_eq!(p.entries_gen(), generation);
                        let background = tabs.slots[1].as_ref().unwrap();
                        assert_eq!(background.current_path, temp.path());
                        assert!(background.entries().is_empty());
                        assert!(!background.has_notify());
                    }
                    let (_, p) = ws.tabs_and_panel(side);
                    assert_eq!(p.current_path, owner);
                    assert_eq!(p.cursor_entry().unwrap().path, keep);
                    assert!(p.is_selected(&keep));
                    assert_eq!(p.search_query(), "keep");
                    assert_eq!(p.browser_mode(), mode);
                    assert_eq!(std::fs::read(keep).unwrap(), b"kept");
                }
            }
        }
    }

    #[test]
    fn parent_row_tabs_refuse_stale_owner_revision_root_modal_and_capacity() {
        let temp = TempDir::new();
        let owner = temp.dir("owner");
        let other = temp.dir("other");
        let file = temp.file("owner/keep.txt", "kept");
        let mut ws = Workspace::new(owner.clone(), other.clone());
        ws.left.refresh();
        let generation = ws.left.entries_gen();
        let before = ws.tab_snapshot(ActivePanel::Left);
        for (captured, revision, target) in [
            (other.clone(), generation, temp.path().into()),
            (
                owner.clone(),
                generation.wrapping_add(1),
                temp.path().into(),
            ),
            (owner.clone(), generation, other),
            (owner.clone(), generation, owner.clone()),
            (owner.clone(), generation, file.clone()),
        ] {
            assert!(!ws.open_listed_folder_tab(
                ActivePanel::Left,
                &captured,
                revision,
                target,
                false
            ));
            assert_eq!(ws.tab_snapshot(ActivePanel::Left), before);
        }
        ws.emit_ui_request(crate::ui_request::UiRequest::Rename(file));
        assert!(!ws.open_listed_folder_tab(
            ActivePanel::Left,
            &owner,
            generation,
            temp.path().into(),
            false
        ));
        ws.drain_ui_requests();
        while ws.tab_count(ActivePanel::Left) < MAX_TABS {
            assert!(ws.new_background_tab(ActivePanel::Left, temp.path().into()));
        }
        let full = ws.tab_snapshot(ActivePanel::Left);
        for activate in [false, true] {
            assert!(!ws.open_listed_folder_tab(
                ActivePanel::Left,
                &owner,
                generation,
                temp.path().into(),
                activate
            ));
            assert_eq!(ws.tab_snapshot(ActivePanel::Left), full);
        }
        let mut root = Workspace::new(PathBuf::from("/"), PathBuf::from("/"));
        assert!(!root.open_listed_folder_tab(
            ActivePanel::Left,
            std::path::Path::new("/"),
            root.left.entries_gen(),
            PathBuf::from("/"),
            false
        ));
        assert_eq!(root.tab_count(ActivePanel::Left), 1);
    }

    #[test]
    fn listed_folder_tabs_revalidate_visible_directory_and_preserve_source_for_all_views() {
        for mode in [
            crate::panel::BrowserMode::List,
            crate::panel::BrowserMode::Icons,
            crate::panel::BrowserMode::Miller,
        ] {
            let temp = TempDir::new();
            let folder = temp.dir("Target");
            let file = temp.file("keep.txt", "keep");
            let other = temp.dir("Other");
            let mut ws = Workspace::new(temp.path().into(), other.clone());
            ws.left.refresh();
            ws.left.focus_child("keep.txt".into());
            ws.left.select_cursor();
            ws.left.set_browser_mode(mode);
            ws.active = ActivePanel::Right;
            let revision = ws.left.entries_gen();
            for (owner, generation, target) in [
                (other.clone(), revision, folder.clone()),
                (temp.path().into(), revision.wrapping_add(1), folder.clone()),
                (temp.path().into(), revision, file.clone()),
                (temp.path().into(), revision, temp.path().join("missing")),
            ] {
                assert!(!ws.open_listed_folder_tab(
                    ActivePanel::Left,
                    &owner,
                    generation,
                    target,
                    false
                ));
                assert_eq!(ws.tab_count(ActivePanel::Left), 1);
            }
            ws.left.set_search_query("keep");
            assert!(!ws.open_listed_folder_tab(
                ActivePanel::Left,
                temp.path(),
                ws.left.entries_gen(),
                folder.clone(),
                false
            ));
            ws.left.clear_filters();
            let revision = ws.left.entries_gen();
            assert!(ws.open_listed_folder_tab(
                ActivePanel::Left,
                temp.path(),
                revision,
                folder.clone(),
                false
            ));
            assert_eq!(ws.tab_count(ActivePanel::Left), 2);
            assert_eq!(ws.active_tab_index(ActivePanel::Left), 0);
            assert_eq!(ws.tab_snapshot(ActivePanel::Left).entries[1].path, folder);
            assert_eq!(ws.left.current_path, temp.path());
            assert_eq!(ws.left.entries_gen(), revision);
            assert_eq!(ws.left.cursor_entry().unwrap().path, file);
            assert_eq!(ws.left.selected_entries()[0].path, file);
            assert_eq!(ws.right.current_path, other);
            assert_eq!(ws.active, ActivePanel::Right);
            while ws.tab_count(ActivePanel::Left) < MAX_TABS {
                assert!(ws.new_background_tab(ActivePanel::Left, folder.clone()));
            }
            assert!(!ws.open_listed_folder_tab(
                ActivePanel::Left,
                temp.path(),
                revision,
                folder,
                false
            ));
        }
    }
    #[test]
    fn miller_foreground_tab_activates_the_captured_folder_and_preserves_source_slot() {
        let temp = TempDir::new();
        let a = temp.dir("A");
        let b = temp.dir("B");
        temp.file("A/a.txt", "keep");
        let mut ws = Workspace::new(a.clone(), temp.path().into());
        ws.left.refresh();
        ws.left.set_browser_mode(crate::panel::BrowserMode::Miller);
        let mut column = PanelState::new(temp.path().into());
        column.refresh();
        ws.left.miller.columns.push(column);
        let revision = ws.left.entries_gen();
        assert!(!ws.open_miller_tab(
            ActivePanel::Left,
            &a,
            revision.wrapping_add(1),
            b.clone(),
            true
        ));
        assert_eq!(ws.tab_count(ActivePanel::Left), 1);
        assert!(ws.open_miller_tab(ActivePanel::Left, &a, revision, b.clone(), true));
        assert_eq!(ws.left.current_path, b);
        assert_eq!(ws.left.browser_mode(), crate::panel::BrowserMode::Miller);
        let tabs = ws.tab_snapshot(ActivePanel::Left);
        assert_eq!(tabs.active, 1);
        assert_eq!(tabs.entries[0].path, a);
        assert_eq!(tabs.entries[1].path, b);
        assert_eq!(ws.right.current_path, temp.path());
        assert_eq!(ws.active, ActivePanel::Left);
        assert!(!ws.open_miller_tab(ActivePanel::Left, &a, revision, temp.path().into(), true));
        assert_eq!(ws.tab_count(ActivePanel::Left), 2);
    }
    #[test]
    fn miller_background_tabs_validate_displayed_owner_and_preserve_live_state() {
        let temp = TempDir::new();
        let a = temp.dir("A");
        let b = temp.dir("B");
        let file = temp.file("A/a.txt", "keep");
        let control = temp.file("control.txt", "control");
        let mut ws = Workspace::new(a.clone(), temp.path().into());
        ws.left.refresh();
        ws.left.focus_child("a.txt".into());
        ws.left.select_cursor();
        ws.left.set_browser_mode(crate::panel::BrowserMode::Miller);
        let mut column = PanelState::new(temp.path().into());
        column.refresh();
        ws.left.miller.columns.push(column);
        let generation = ws.left.entries_gen();
        let cursor = ws.left.cursor_entry().unwrap().path.clone();
        for (owner, revision, folder) in [
            (temp.path().to_path_buf(), generation, b.clone()),
            (a.clone(), generation.wrapping_add(1), b.clone()),
            (a.clone(), generation, control),
            (a.clone(), generation, temp.path().join("unlisted")),
        ] {
            assert!(!ws.open_miller_background_tab(ActivePanel::Left, &owner, revision, folder));
            assert_eq!(ws.tab_count(ActivePanel::Left), 1);
        }
        assert!(ws.open_miller_background_tab(ActivePanel::Left, &a, generation, b.clone()));
        assert_eq!(ws.tab_snapshot(ActivePanel::Left).entries[1].path, b);
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 0);
        assert_eq!(ws.left.current_path, a);
        assert_eq!(ws.left.entries_gen(), generation);
        assert_eq!(ws.left.cursor_entry().unwrap().path, cursor);
        assert_eq!(ws.left.selected_entries()[0].path, file);
        assert_eq!(ws.right.current_path, temp.path());
        assert_eq!(ws.left.miller.columns.len(), 1);
        assert!(ws.open_miller_background_tab(
            ActivePanel::Left,
            &a,
            generation,
            temp.path().into()
        ));
        while ws.tab_count(ActivePanel::Left) < MAX_TABS {
            assert!(ws.new_background_tab(ActivePanel::Left, b.clone()));
        }
        assert!(!ws.open_miller_background_tab(ActivePanel::Left, &a, generation, b));
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 0);
        assert_eq!(ws.left.current_path, a);
        ws.left.set_browser_mode(crate::panel::BrowserMode::List);
        assert!(!ws.open_miller_background_tab(
            ActivePanel::Left,
            &a,
            generation,
            temp.path().into()
        ));
    }
    #[test]
    fn favorite_picker_binding_uses_its_opening_panel_and_revalidates_removed_paths() {
        use crate::workspace::FavoriteTarget;
        let temp = TempDir::new();
        let favorite = temp.dir("favorite");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.bookmarks.add("Pinned", favorite.clone());
        ws.active = ActivePanel::Right;
        assert!(ws.open_favorite_at(ActivePanel::Left, &favorite, FavoriteTarget::CurrentPanel));
        assert_eq!(ws.left.current_path, favorite);
        assert_eq!(ws.right.current_path, temp.path());
        assert_eq!(ws.active, ActivePanel::Right);
        assert!(ws.open_favorite_at(ActivePanel::Left, &favorite, FavoriteTarget::BackgroundTab));
        assert_eq!(ws.tab_count(ActivePanel::Left), 2);
        assert_eq!(ws.tab_count(ActivePanel::Right), 1);
        ws.bookmarks.remove(&favorite);
        let before = [
            ws.tab_snapshot(ActivePanel::Left),
            ws.tab_snapshot(ActivePanel::Right),
        ];
        assert!(!ws.open_favorite_at(ActivePanel::Left, &favorite, FavoriteTarget::NewTab));
        assert_eq!(
            [
                ws.tab_snapshot(ActivePanel::Left),
                ws.tab_snapshot(ActivePanel::Right)
            ],
            before
        );
    }

    #[test]
    fn favorite_destinations_preserve_source_state_and_bookmarks_for_both_active_sides() {
        use crate::workspace::FavoriteTarget;
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let source = temp.dir("source");
            let other = temp.dir("other");
            let favorite = temp.dir("favorite");
            let mut ws = Workspace::new(source.clone(), other.clone());
            ws.active = side;
            ws.bookmarks.add("Pinned", favorite.clone());
            ws.bookmarks.assign_slot(&favorite, 3);
            let original = ws.bookmarks.clone();
            let source_path = ws.active_panel_ref().current_path.clone();
            ws.active_panel().set_search_query("keep-filter");
            let preview = crate::panel::PreviewContent::Image(temp.path().join("preview.png"));
            ws.active_panel().preview = Some(preview.clone());
            assert!(ws.open_favorite(&favorite, FavoriteTarget::BackgroundTab));
            assert_eq!(ws.active_panel_ref().current_path, source_path);
            assert_eq!(ws.active_tab_index(side), 0);
            assert_eq!(ws.tab_count(side), 2);
            assert_eq!(ws.active_panel_ref().search_query(), "keep-filter");
            assert_eq!(ws.tab_snapshot(side).entries[1].path, favorite);
            assert_eq!(ws.active_panel_ref().preview, Some(preview.clone()));
            assert!(ws.open_favorite(&favorite, FavoriteTarget::OtherPanel));
            assert_eq!(ws.active_panel_ref().current_path, source_path);
            assert_eq!(ws.active_panel_ref().search_query(), "keep-filter");
            assert_eq!(ws.inactive_panel().current_path, favorite);
            assert_eq!(ws.active_panel_ref().preview, Some(preview));
            assert_eq!(ws.active, side);
            assert!(ws.open_favorite(&favorite, FavoriteTarget::NewTab));
            assert_eq!(ws.active_tab_index(side), 1);
            assert_eq!(ws.tab_count(side), 3);
            assert_eq!(ws.active_panel_ref().current_path, favorite);
            assert!(ws.select_tab(side, 0));
            assert_eq!(ws.active_panel_ref().current_path, source_path);
            assert_eq!(ws.active_panel_ref().search_query(), "keep-filter");
            assert_eq!(ws.bookmarks, original);
        }
    }

    #[test]
    fn favorite_open_rejects_unknown_paths_modal_and_capacity_without_partial_changes() {
        use crate::workspace::FavoriteTarget;
        let temp = TempDir::new();
        let favorite = temp.dir("favorite");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.bookmarks.add("Pinned", favorite.clone());
        for target in [
            FavoriteTarget::OtherPanel,
            FavoriteTarget::NewTab,
            FavoriteTarget::BackgroundTab,
        ] {
            assert!(!ws.open_favorite(&temp.path().join("unknown"), target));
            ws.emit_ui_request(crate::ui_request::UiRequest::Rename(favorite.clone()));
            let before = [
                ws.tab_snapshot(ActivePanel::Left),
                ws.tab_snapshot(ActivePanel::Right),
            ];
            assert!(!ws.open_favorite(&favorite, target));
            assert_eq!(
                [
                    ws.tab_snapshot(ActivePanel::Left),
                    ws.tab_snapshot(ActivePanel::Right)
                ],
                before
            );
            ws.drain_ui_requests();
        }
        for _ in 1..MAX_TABS {
            assert!(ws.new_background_tab(ActivePanel::Left, favorite.clone()));
        }
        let before = [
            ws.tab_snapshot(ActivePanel::Left),
            ws.tab_snapshot(ActivePanel::Right),
        ];
        assert!(!ws.open_favorite(&favorite, FavoriteTarget::NewTab));
        assert!(!ws.open_favorite(&favorite, FavoriteTarget::BackgroundTab));
        assert_eq!(
            [
                ws.tab_snapshot(ActivePanel::Left),
                ws.tab_snapshot(ActivePanel::Right)
            ],
            before
        );
        assert!(ws.open_favorite(&favorite, FavoriteTarget::OtherPanel));
        assert_eq!(ws.tab_count(ActivePanel::Left), MAX_TABS);
    }

    #[test]
    fn full_path_labels_preserve_inactive_tab_state_and_custom_titles() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.new_tab(side, Some(temp.dir("work")));
            ws.active = side;
            ws.active_panel().set_search_query("retained");
            ws.set_tab_title(side, 0, "Project");
            let before = ws.tab_snapshot(side);
            let other = ws.tab_snapshot(side.other());
            let revision = ws.tab_revision();
            let generation = ws.active_panel_ref().entries_gen();
            assert!(ws.set_tab_full_path_label(side, 0, true));
            let after = ws.tab_snapshot(side);
            assert_eq!(after.active, before.active);
            assert_eq!(
                after.entries[0].display_title(),
                temp.path().to_string_lossy()
            );
            assert_eq!(after.entries[0].title.as_deref(), Some("Project"));
            assert_eq!(ws.active, side);
            assert_eq!(ws.active_panel_ref().search_query(), "retained");
            assert_eq!(ws.active_panel_ref().entries_gen(), generation);
            assert_eq!(ws.tab_snapshot(side.other()), other);
            assert_eq!(ws.closed_tab_count(side), 0);
            assert!(ws.tab_revision() > revision);
            let revision = ws.tab_revision();
            assert!(!ws.set_tab_full_path_label(side, 0, true));
            assert!(!ws.set_tab_full_path_label(side, usize::MAX, true));
            assert_eq!(ws.tab_revision(), revision);
            assert!(ws.set_tab_full_path_label(side, 0, false));
            assert_eq!(ws.tab_snapshot(side), before);
            assert_eq!(ws.tab_snapshot(side).entries[0].display_title(), "Project");
            ws.emit_ui_request(crate::ui_request::UiRequest::Rename(
                temp.file("modal.txt", "x"),
            ));
            let revision = ws.tab_revision();
            assert!(!ws.set_tab_full_path_label(side, 0, true));
            assert_eq!(ws.tab_revision(), revision);
        }
    }

    #[test]
    fn full_path_label_restore_does_not_leak_to_overridden_visible_paths() {
        let temp = TempDir::new();
        let original = temp.dir("original");
        let replacement = temp.dir("replacement");
        let mut source = Workspace::new(original.clone(), original);
        source.set_tab_full_path_label(ActivePanel::Left, 0, true);
        source.set_tab_full_path_label(ActivePanel::Right, 0, true);
        let left = source.tab_snapshot(ActivePanel::Left);
        let right = source.tab_snapshot(ActivePanel::Right);
        let mut restored = Workspace::new(replacement.clone(), replacement);
        restored.restore_tabs(&left, &right);
        assert!(!restored.tab_snapshot(ActivePanel::Left).entries[0].full_path_label);
        assert!(!restored.tab_snapshot(ActivePanel::Right).entries[0].full_path_label);
    }

    #[test]
    fn full_path_label_metadata_defaults_and_malformed_fields_keep_tabs() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!("yes"),
            serde_json::json!(1),
            serde_json::json!({}),
        ] {
            let saved: SavedTabs = serde_json::from_value(serde_json::json!({"entries":[{"path":"/a","title":"Project","full_path_label":value}],"active":0})).unwrap();
            assert_eq!(saved.entries.len(), 1);
            assert!(!saved.entries[0].full_path_label);
            assert_eq!(saved.entries[0].display_title(), "Project");
        }
        let legacy: SavedTabs =
            serde_json::from_str(r#"{"entries":[{"path":"/a"}],"active":0}"#).unwrap();
        assert!(!legacy.entries[0].full_path_label);
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("full_path_label")
        );
        let mut saved = legacy;
        saved.entries[0].full_path_label = true;
        assert_eq!(
            serde_json::from_str::<SavedTabs>(&serde_json::to_string(&saved).unwrap()).unwrap(),
            saved
        );
    }

    #[test]
    fn duplicate_priority_keeps_named_or_protected_runtime_state_and_recovers_closed_tabs() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            for protected in [false, true] {
                let temp = TempDir::new();
                let picked = temp.file("picked.txt", "payload");
                let mut ws = Workspace::new(temp.path().into(), temp.path().into());
                ws.new_tab(side, None);
                ws.set_tab_title(side, 1, "Work");
                ws.set_tab_close_protection(side, 1, protected);
                match side {
                    ActivePanel::Left => {
                        ws.left.refresh();
                        ws.left.toggle_select(picked.clone());
                        ws.left.set_search_query("picked");
                    }
                    ActivePanel::Right => {
                        ws.right.refresh();
                        ws.right.toggle_select(picked.clone());
                        ws.right.set_search_query("picked");
                    }
                }
                ws.select_tab(side, 0);
                let before = ws.tab_snapshot(side);
                assert_eq!(ws.duplicate_tab_count(side), 1);
                assert!(ws.close_duplicate_tabs(side));
                assert_eq!(ws.tab_count(side), 1);
                let current = match side {
                    ActivePanel::Left => &ws.left,
                    ActivePanel::Right => &ws.right,
                };
                assert_eq!(current.tab_title.as_deref(), Some("Work"));
                assert_eq!(current.tab_close_protected, protected);
                assert_eq!(current.search_query(), "picked");
                assert!(current.is_selected(&picked));
                assert_eq!(current.current_path, temp.path());
                assert_eq!(ws.closed_tab_count(side), 1);
                assert!(ws.reopen_closed_tab(side));
                assert_eq!(ws.tab_snapshot(side).entries, before.entries);
            }
        }
    }

    #[test]
    fn duplicate_priority_prefers_active_among_equal_titles_and_first_other_group() {
        let temp = TempDir::new();
        let child = temp.dir("child");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.set_tab_title(ActivePanel::Left, 0, "first");
        ws.new_tab(ActivePanel::Left, None);
        ws.set_tab_title(ActivePanel::Left, 1, "active");
        ws.new_background_tab(ActivePanel::Left, child.clone());
        ws.new_background_tab(ActivePanel::Left, child);
        ws.set_tab_title(ActivePanel::Left, 3, "Named child");
        assert!(ws.close_duplicate_tabs(ActivePanel::Left));
        let saved = ws.tab_snapshot(ActivePanel::Left);
        assert_eq!(saved.entries[0].title.as_deref(), Some("active"));
        assert_eq!(saved.entries[1].title.as_deref(), Some("Named child"));
        assert_eq!(saved.active, 0);
        assert!(!ws.close_duplicate_tabs(ActivePanel::Left));
    }

    #[test]
    fn protected_tabs_survive_all_close_groups_and_remain_navigable() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            for active in 0..4 {
                for mode in 0..3 {
                    let temp = TempDir::new();
                    let mut ws = Workspace::new(temp.path().into(), temp.path().into());
                    for name in ["a", "b", "c"] {
                        ws.new_tab(side, Some(temp.dir(name)));
                    }
                    assert!(ws.set_tab_close_protection(side, 2, true));
                    ws.select_tab(side, 2);
                    ws.active = side;
                    assert!(
                        !crate::command::availability(
                            crate::command::Command::CloseTab,
                            &ws.command_context()
                        )
                        .enabled
                    );
                    assert!(
                        crate::command::availability(
                            crate::command::Command::NextTab,
                            &ws.command_context()
                        )
                        .enabled
                    );
                    assert!(!ws.close_tab(side));
                    assert!(!ws.close_tab_at(side, 2));
                    ws.select_tab(side, active);
                    let before = ws.tab_snapshot(side);
                    assert!(match mode {
                        0 => ws.close_tab_group(side, 1, true),
                        1 => ws.close_tab_group(side, 0, false),
                        _ => ws.close_tabs_left(side, 3),
                    });
                    let after = ws.tab_snapshot(side);
                    assert_eq!(after.entries.len(), 2);
                    assert!(
                        after
                            .entries
                            .iter()
                            .any(|tab| tab.path == before.entries[2].path && tab.close_protected)
                    );
                    assert_eq!(ws.tab_count(side.other()), 1);
                    for (index, tab) in after.entries.iter().enumerate() {
                        ws.select_tab(side, index);
                        let visible = match side {
                            ActivePanel::Left => &ws.left,
                            ActivePanel::Right => &ws.right,
                        };
                        assert_eq!(visible.current_path, tab.path);
                    }
                    while ws.reopen_closed_tab(side) {}
                    assert_eq!(ws.tab_snapshot(side).entries, before.entries);
                }
            }
        }
    }

    #[test]
    fn protected_duplicates_are_retained_and_protection_follows_tab_lifecycle() {
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        for _ in 0..3 {
            ws.new_tab(ActivePanel::Left, None);
        }
        ws.select_tab(ActivePanel::Left, 0);
        ws.set_tab_close_protection(ActivePanel::Left, 1, true);
        ws.set_tab_close_protection(ActivePanel::Left, 2, true);
        assert_eq!(ws.duplicate_tab_count(ActivePanel::Left), 2);
        assert!(ws.close_duplicate_tabs(ActivePanel::Left));
        assert_eq!(ws.tab_count(ActivePanel::Left), 2);
        assert_eq!(ws.duplicate_tab_count(ActivePanel::Left), 0);
        assert!(!ws.close_duplicate_tabs(ActivePanel::Left));
        assert!(ws.duplicate_tab_at(ActivePanel::Left, 1));
        assert!(ws.tab_snapshot(ActivePanel::Left).entries[2].close_protected);
        assert!(ws.copy_tabs_to_other(ActivePanel::Left));
        assert!(ws.tab_snapshot(ActivePanel::Right).entries[2].close_protected);
        assert!(ws.move_tab_to_other(ActivePanel::Left, 1));
        assert!(ws.tab_snapshot(ActivePanel::Right).entries[1].close_protected);
        ws.select_tab(ActivePanel::Right, 1);
        let saved = ws.tab_snapshot(ActivePanel::Right);
        let decoded: SavedTabs =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        let mut restored = Workspace::new(temp.path().into(), temp.path().into());
        restored.restore_tabs(&SavedTabs::default(), &decoded);
        assert_eq!(restored.tab_snapshot(ActivePanel::Right), decoded);
        assert!(!restored.close_tab(ActivePanel::Right));
        for index in 0..restored.tab_count(ActivePanel::Right) {
            restored.set_tab_close_protection(ActivePanel::Right, index, true);
        }
        let protected_snapshot = restored.tab_snapshot(ActivePanel::Right);
        let revision = restored.tab_revision();
        assert!(!restored.close_tab_group(ActivePanel::Right, 0, true));
        assert!(!restored.close_tab_group(ActivePanel::Right, 0, false));
        assert!(!restored.close_tabs_left(ActivePanel::Right, 2));
        assert_eq!(
            restored.tab_snapshot(ActivePanel::Right),
            protected_snapshot
        );
        assert_eq!(restored.tab_revision(), revision);
        assert!(restored.set_tab_close_protection(ActivePanel::Right, decoded.active, false));
        assert!(restored.close_tab(ActivePanel::Right));
        let legacy: SavedTabs = serde_json::from_value(serde_json::json!({"entries":[{"path":"/a","close_protected":"invalid"},{"path":"/b"}]})).unwrap();
        assert_eq!(legacy.entries.len(), 2);
        assert!(legacy.entries.iter().all(|tab| !tab.close_protected));
    }

    #[test]
    fn custom_tab_titles_survive_duplicate_copy_move_navigation_close_and_restore() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let a = temp.dir("a");
            let nested = temp.dir("nested");
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.new_tab(side, Some(a.clone()));
            let focus = ws.active;
            assert!(ws.set_tab_title(side, 0, "  Проект  "));
            assert!(ws.set_tab_full_path_label(side, 0, true));
            assert_eq!(ws.active, focus);
            assert_eq!(ws.active_tab_index(side), 1);
            assert_eq!(
                ws.tab_snapshot(side).entries[0].title.as_deref(),
                Some("Проект")
            );
            assert_eq!(
                ws.tab_path_for_copy(side, 0),
                Some(temp.path().to_string_lossy().into_owned())
            );
            assert!(ws.duplicate_tab_at(side, 0));
            assert_eq!(
                ws.tab_snapshot(side).entries[1].title.as_deref(),
                Some("Проект")
            );
            assert!(ws.tab_snapshot(side).entries[1].full_path_label);
            assert!(ws.copy_tabs_to_other(side));
            assert!(ws.tab_snapshot(side.other()).entries[1].full_path_label);
            assert_eq!(
                ws.tab_snapshot(side.other()).entries[1].title.as_deref(),
                Some("Проект")
            );
            ws.select_tab(side, 0);
            match side {
                ActivePanel::Left => ws.left.navigate_to(nested.clone()),
                ActivePanel::Right => ws.right.navigate_to(nested.clone()),
            }
            assert_eq!(
                ws.tab_snapshot(side).entries[0].title.as_deref(),
                Some("Проект")
            );
            assert!(ws.move_tab_to_other(side, 0));
            assert_eq!(
                ws.tab_snapshot(side.other()).entries[1].title.as_deref(),
                Some("Проект")
            );
            assert!(ws.tab_snapshot(side.other()).entries[1].full_path_label);
            assert_eq!(
                ws.tab_snapshot(side.other()).entries[1].display_title(),
                nested.to_string_lossy()
            );
            assert_eq!(ws.tab_snapshot(side.other()).entries[1].path, nested);
            assert!(ws.close_tab_at(side.other(), 1));
            assert!(ws.reopen_closed_tab(side.other()));
            assert_eq!(
                ws.tab_snapshot(side.other()).entries[1].title.as_deref(),
                Some("Проект")
            );
            assert!(ws.tab_snapshot(side.other()).entries[1].full_path_label);
            let left = ws.tab_snapshot(ActivePanel::Left);
            let right = ws.tab_snapshot(ActivePanel::Right);
            let left: SavedTabs =
                serde_json::from_str(&serde_json::to_string(&left).unwrap()).unwrap();
            let right: SavedTabs =
                serde_json::from_str(&serde_json::to_string(&right).unwrap()).unwrap();
            let mut restored = Workspace::new(
                left.entries[left.active].path.clone(),
                right.entries[right.active].path.clone(),
            );
            restored.restore_tabs(&left, &right);
            assert_eq!(restored.tab_snapshot(ActivePanel::Left), left);
            assert_eq!(restored.tab_snapshot(ActivePanel::Right), right);
            assert!(restored.set_tab_title(side.other(), 1, ""));
            assert_eq!(restored.tab_snapshot(side.other()).entries[1].title, None);
            assert!(restored.tab_snapshot(side.other()).entries[1].full_path_label);
            assert!(restored.set_tab_full_path_label(side.other(), 1, false));
            assert_eq!(
                restored.tab_snapshot(side.other()).entries[1].display_title(),
                "nested"
            );
            assert_eq!(restored.tab_snapshot(side.other()).entries[1].path, nested);
        }
    }

    #[test]
    fn tab_title_validation_keeps_legacy_tabs_and_rejects_invalid_metadata() {
        let legacy: SavedTabs =
            serde_json::from_value(serde_json::json!({"entries":[{"path":"/a"}],"active":0}))
                .unwrap();
        assert!(legacy.entries[0].title.is_none());
        assert!(!serde_json::to_string(&legacy).unwrap().contains("title"));
        let saved: SavedTabs = serde_json::from_value(serde_json::json!({"entries":[
            {"path":"/a", "title":"  Work  "}, {"path":"/b", "title":123},
            {"path":"/c", "title":"bad\nline"}, {"path":"/d", "title":"x".repeat(65)}
        ], "active":1}))
        .unwrap();
        assert_eq!(saved.entries.len(), 4);
        assert_eq!(saved.entries[0].title.as_deref(), Some("Work"));
        assert!(saved.entries[1..].iter().all(|tab| tab.title.is_none()));
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        let before = ws.tab_revision();
        assert!(!ws.set_tab_title(ActivePanel::Left, 0, "bad\tlabel"));
        assert!(!ws.set_tab_title(ActivePanel::Left, 0, &"x".repeat(65)));
        assert!(!ws.set_tab_title(ActivePanel::Left, 99, "Work"));
        assert_eq!(ws.tab_revision(), before);
        ws.restore_tabs(&saved, &SavedTabs::default());
        assert!(
            ws.tab_snapshot(ActivePanel::Left).entries[1]
                .title
                .is_none()
        );
    }

    #[test]
    fn copy_tab_groups_preserves_order_views_active_slots_and_capacity_atomically() {
        use crate::command::{Command, availability};
        let root = TempDir::new();
        let paths: Vec<_> = (0..MAX_TABS)
            .map(|index| root.dir(&format!("folder-{index}")))
            .collect();
        for source in [ActivePanel::Left, ActivePanel::Right] {
            for source_count in 1..=MAX_TABS {
                for destination_count in 1..=MAX_TABS {
                    let source_saved = SavedTabs {
                        entries: paths[..source_count]
                            .iter()
                            .enumerate()
                            .map(|(index, path)| SavedTab {
                                path: path.clone(),
                                pinned_path: None,
                                title: None,
                                close_protected: false,
                                full_path_label: false,
                                miller_widths: Default::default(),
                                selection_snapshot: Default::default(),
                                view: ViewConfig::default().with_mode(if index % 2 == 0 {
                                    crate::panel::BrowserMode::Icons
                                } else {
                                    crate::panel::BrowserMode::List
                                }),
                            })
                            .collect(),
                        active: source_count - 1,
                        closed: Vec::new(),
                    };
                    let target_saved = SavedTabs {
                        entries: paths[..destination_count]
                            .iter()
                            .map(|path| SavedTab {
                                path: path.clone(),
                                pinned_path: None,
                                title: None,
                                close_protected: false,
                                full_path_label: false,
                                miller_widths: Default::default(),
                                selection_snapshot: Default::default(),
                                view: ViewConfig::default(),
                            })
                            .collect(),
                        active: destination_count / 2,
                        closed: Vec::new(),
                    };
                    let mut ws = Workspace::new(paths[0].clone(), paths[0].clone());
                    if source == ActivePanel::Left {
                        ws.restore_tabs(&source_saved, &target_saved);
                    } else {
                        ws.restore_tabs(&target_saved, &source_saved);
                    }
                    ws.active = source;
                    let source_before = ws.tab_snapshot(source);
                    let destination_before = ws.tab_snapshot(source.other());
                    let revision = ws.tab_revision();
                    let fits = source_count + destination_count <= MAX_TABS;
                    assert_eq!(ws.can_copy_tabs_to_other(source), fits);
                    assert_eq!(
                        availability(Command::CopyTabsToOther, &ws.command_context()).enabled,
                        fits
                    );
                    assert_eq!(ws.copy_tabs_to_other(source), fits);
                    assert_eq!(ws.tab_snapshot(source), source_before);
                    assert_eq!(ws.active, source);
                    let after = ws.tab_snapshot(source.other());
                    assert_eq!(after.active, destination_before.active);
                    let mut expected = destination_before.entries.clone();
                    if fits {
                        expected.splice(
                            destination_before.active + 1..destination_before.active + 1,
                            source_before.entries.clone(),
                        );
                    }
                    assert_eq!(after.entries, expected);
                    assert_eq!(ws.tab_revision(), revision.wrapping_add(u64::from(fits)));
                    assert_eq!(ws.closed_tab_count(source), 0);
                    assert_eq!(ws.closed_tab_count(source.other()), 0);
                    if fits {
                        let (tabs, _) = ws.tabs_and_panel(source.other());
                        for slot in &tabs.slots[destination_before.active + 1
                            ..destination_before.active + 1 + source_count]
                        {
                            let panel = slot.as_ref().unwrap();
                            assert!(panel.entries().is_empty());
                            assert!(!panel.has_notify());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn copy_tabs_command_keeps_selections_and_new_tabs_observe_later_files() {
        let root = TempDir::new();
        let source_root = root.dir("source");
        let target_root = root.dir("target");
        root.file("source/a.txt", "source");
        root.file("target/b.txt", "target");
        let mut ws = Workspace::new(source_root.clone(), target_root.clone());
        ws.left.refresh();
        ws.right.refresh();
        ws.left.toggle_select(source_root.join("a.txt"));
        ws.right.toggle_select(target_root.join("b.txt"));
        ws.execute(crate::command::Command::CopyTabsToOther);
        assert!(ws.left.is_selected(&source_root.join("a.txt")));
        assert!(ws.right.is_selected(&target_root.join("b.txt")));
        assert_eq!(ws.left.current_path, source_root);
        assert_eq!(ws.right.current_path, target_root);
        root.file("source/later.txt", "fresh");
        assert!(ws.select_tab(ActivePanel::Right, 1));
        ws.right.refresh();
        assert!(
            ws.right
                .entries()
                .iter()
                .any(|entry| entry.name == "later.txt")
        );
        assert!(ws.right.selection_is_empty());
    }

    #[test]
    fn background_open_keeps_visible_state_and_lazily_loads_only_on_activation() {
        use crate::command::{Command, availability};
        let root = TempDir::new();
        let child = root.dir("child");
        root.file("selected.txt", "payload");
        let mut ws = Workspace::new(root.path().into(), root.path().into());
        ws.left.refresh();
        ws.left.focus_child("child".into());
        ws.left.toggle_select(root.path().join("selected.txt"));
        ws.left.set_browser_mode(crate::panel::BrowserMode::Icons);
        let snapshot = ws.tab_snapshot(ActivePanel::Left);
        let rows = ws.left.entries().len();
        let cursor = ws.left.cursor_entry().unwrap().path.clone();
        let revision = ws.tab_revision();
        assert!(availability(Command::OpenFolderBackgroundTab, &ws.command_context()).enabled);
        ws.execute(Command::OpenFolderBackgroundTab);
        assert_eq!(ws.left.current_path, root.path());
        assert_eq!(ws.left.cursor_entry().unwrap().path, cursor);
        assert_eq!(ws.left.entries().len(), rows);
        assert!(ws.left.is_selected(&root.path().join("selected.txt")));
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 0);
        assert_eq!(ws.tab_revision(), revision.wrapping_add(1));
        let saved = ws.tab_snapshot(ActivePanel::Left);
        assert_eq!(saved.entries[0], snapshot.entries[0]);
        assert_eq!(saved.entries[1].path, child);
        assert_eq!(saved.entries[1].view, saved.entries[0].view);
        let suspended = ws.tabs[0].slots[1].as_ref().unwrap();
        assert!(suspended.entries().is_empty());
        assert!(!suspended.has_notify());
        assert_eq!(ws.closed_tab_count(ActivePanel::Left), 0);
        root.file("child/created-later.txt", "fresh");
        assert!(ws.select_tab(ActivePanel::Left, 1));
        ws.left.refresh();
        assert!(
            ws.left
                .entries()
                .iter()
                .any(|entry| entry.name == "created-later.txt")
        );
        assert!(ws.select_tab(ActivePanel::Left, 0));
        assert!(ws.left.is_selected(&root.path().join("selected.txt")));
    }

    #[test]
    fn background_result_group_preserves_order_lazy_focus_and_rejects_partial_opening() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let keep = temp.file("keep.txt", "keep");
            let first = temp.file("one/a.txt", "a");
            let second = temp.file("two/.hidden.txt", "b");
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.left.refresh();
            ws.right.refresh();
            ws.left.toggle_select(keep.clone());
            ws.right.toggle_select(keep.clone());
            let active = ws.active;
            let revision = ws.tab_revision;
            assert!(!ws.reveal_in_background_tabs(side, vec![first.clone(), PathBuf::from("/")]));
            assert_eq!(ws.tab_revision, revision);
            assert_eq!(ws.tab_count(side), 1);
            assert!(!ws.reveal_in_background_tabs(side, vec![]));
            ws.emit_ui_request(crate::ui_request::UiRequest::Rename(first.clone()));
            assert!(!ws.reveal_in_background_tabs(side, vec![first.clone(), second.clone()]));
            assert_eq!(ws.tab_count(side), 1);
            assert_eq!(ws.tab_revision, revision);
            ws.drain_ui_requests();
            assert!(ws.reveal_in_background_tabs(
                side,
                vec![first.clone(), second.clone(), first.clone()]
            ));
            assert_eq!(ws.active, active);
            assert_eq!(ws.active_tab_index(side), 0);
            assert_eq!(ws.tab_count(side), 4);
            assert_eq!(ws.left.current_path, temp.path());
            assert_eq!(ws.right.current_path, temp.path());
            assert!(ws.left.is_selected(&keep));
            assert!(ws.right.is_selected(&keep));
            let saved = ws.tab_snapshot(side);
            let entries = &saved.entries;
            let slots = match side {
                ActivePanel::Left => &ws.tabs[0],
                ActivePanel::Right => &ws.tabs[1],
            };
            for slot in slots.slots.iter().skip(1) {
                let panel = slot.as_ref().unwrap();
                assert!(panel.entries().is_empty());
                assert!(!panel.has_notify());
            }
            assert_eq!(entries[1].path, first.parent().unwrap());
            assert_eq!(entries[2].path, second.parent().unwrap());
            assert_eq!(entries[3].path, first.parent().unwrap());
            assert!(entries[2].view.show_hidden());
            for (index, expected) in [(1, first.clone()), (2, second.clone()), (3, first.clone())] {
                assert!(ws.select_tab(side, index));
                let panel = match side {
                    ActivePanel::Left => &mut ws.left,
                    ActivePanel::Right => &mut ws.right,
                };
                panel.refresh();
                assert_eq!(panel.cursor_entry().unwrap().path, expected);
                assert!(ws.select_tab(side, 0));
            }
            while ws.tab_count(side) < MAX_TABS - 1 {
                assert!(ws.new_background_tab(side, temp.path().into()));
            }
            let count = ws.tab_count(side);
            let revision = ws.tab_revision;
            assert!(!ws.reveal_in_background_tabs(side, vec![first.clone(), second.clone()]));
            assert_eq!(ws.tab_count(side), count);
            assert_eq!(ws.tab_revision, revision);
            assert!(ws.reveal_in_background_tabs(side, vec![first]));
            assert_eq!(ws.tab_count(side), MAX_TABS);
        }
    }

    #[test]
    fn background_result_reveal_defers_focus_and_preserves_both_live_panes() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let selected = temp.file("keep.txt", "keep");
            let target = temp.file("found/target.txt", "target");
            temp.file("found/other.txt", "other");
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.left.refresh();
            ws.right.refresh();
            ws.left.toggle_select(selected.clone());
            ws.right.toggle_select(selected.clone());
            let active = ws.active;
            assert!(ws.reveal_in_background_tab(side, target.clone()));
            assert_eq!(ws.active, active);
            assert_eq!(ws.active_tab_index(side), 0);
            assert_eq!(ws.left.current_path, temp.path());
            assert_eq!(ws.right.current_path, temp.path());
            assert!(ws.left.is_selected(&selected) && ws.right.is_selected(&selected));
            let slot = match side {
                ActivePanel::Left => &ws.tabs[0],
                ActivePanel::Right => &ws.tabs[1],
            }
            .slots[1]
                .as_ref()
                .unwrap();
            assert!(slot.entries().is_empty());
            assert!(!slot.has_notify());
            assert!(ws.select_tab(side, 1));
            let panel = match side {
                ActivePanel::Left => &mut ws.left,
                ActivePanel::Right => &mut ws.right,
            };
            panel.refresh();
            assert_eq!(panel.cursor_entry().unwrap().path, target);
            panel.focus_child("other.txt".into());
            panel.refresh();
            assert_eq!(panel.cursor_entry().unwrap().name, "other.txt");
            while ws.tab_count(side) < MAX_TABS {
                assert!(ws.new_background_tab(side, temp.path().into()));
            }
            let before = ws.tab_snapshot(side);
            assert!(!ws.reveal_in_background_tab(side, target));
            assert_eq!(ws.tab_snapshot(side), before);
            assert!(!ws.reveal_in_background_tab(side, PathBuf::from("/")));
        }
    }

    #[test]
    fn background_result_reveal_shows_hidden_targets_without_changing_the_live_view() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let target = temp.file("found/.hidden.txt", "hidden");
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.emit_ui_request(crate::ui_request::UiRequest::Rename(target.clone()));
            assert!(!ws.reveal_in_background_tab(side, target.clone()));
            assert_eq!(ws.tab_count(side), 1);
            ws.drain_ui_requests();
            assert!(ws.reveal_in_background_tab(side, target.clone()));
            let saved = ws.tab_snapshot(side);
            assert!(!saved.entries[0].view.show_hidden());
            assert!(saved.entries[1].view.show_hidden());
            assert!(ws.select_tab(side, 1));
            let panel = match side {
                ActivePanel::Left => &mut ws.left,
                ActivePanel::Right => &mut ws.right,
            };
            panel.refresh();
            assert_eq!(panel.cursor_entry().unwrap().path, target);
        }
    }

    #[test]
    fn background_open_inserts_next_to_current_tab_and_respects_cap_and_modal() {
        use crate::command::{Command, availability};
        let root = TempDir::new();
        root.dir("child");
        let mut ws = Workspace::new(root.path().into(), root.path().into());
        ws.right.refresh();
        ws.right.focus_child("child".into());
        for index in 1..MAX_TABS {
            let path = root.path().join(format!("lazy-{index}"));
            assert!(ws.new_background_tab(ActivePanel::Right, path.clone()));
            assert_eq!(ws.tab_snapshot(ActivePanel::Right).entries[1].path, path);
            assert_eq!(ws.active_tab_index(ActivePanel::Right), 0);
        }
        ws.active = ActivePanel::Right;
        let before = ws.tab_snapshot(ActivePanel::Right);
        assert!(!availability(Command::OpenFolderBackgroundTab, &ws.command_context()).enabled);
        assert!(!ws.new_background_tab(ActivePanel::Right, root.path().join("overflow")));
        ws.execute(Command::OpenFolderBackgroundTab);
        assert_eq!(ws.tab_snapshot(ActivePanel::Right), before);
        assert_eq!(ws.tab_count(ActivePanel::Left), 1);
    }

    #[test]
    fn grouped_closure_keeps_anchor_or_prefix_and_reopens_original_order() {
        let temp = TempDir::new();
        let paths: Vec<_> = (0..4).map(|i| temp.dir(&format!("tab-{i}"))).collect();
        for side in [ActivePanel::Left, ActivePanel::Right] {
            for anchor in 0..4 {
                for active in 0..4 {
                    for others in [false, true] {
                        let mut ws = Workspace::new(paths[0].clone(), paths[0].clone());
                        for path in &paths[1..] {
                            assert!(ws.new_tab(side, Some(path.clone())));
                        }
                        assert!(ws.select_tab(side, active) || active == 3);
                        let before = ws.tab_snapshot(side);
                        let other = if side == ActivePanel::Left {
                            ActivePanel::Right
                        } else {
                            ActivePanel::Left
                        };
                        let untouched = ws.tab_snapshot(other);
                        let revision = ws.tab_revision();
                        let changed = others || anchor < 3;
                        assert_eq!(ws.close_tab_group(side, anchor, others), changed);
                        if !changed {
                            assert_eq!(ws.tab_snapshot(side), before);
                            continue;
                        }
                        let after = ws.tab_snapshot(side);
                        let expected: Vec<_> = if others {
                            vec![paths[anchor].clone()]
                        } else {
                            paths[..=anchor].to_vec()
                        };
                        assert_eq!(
                            after
                                .entries
                                .iter()
                                .map(|tab| tab.path.clone())
                                .collect::<Vec<_>>(),
                            expected
                        );
                        assert_eq!(after.active, if others { 0 } else { active.min(anchor) });
                        assert_eq!(ws.tab_snapshot(other), untouched);
                        assert_eq!(ws.tab_revision(), revision.wrapping_add(1));
                        while ws.reopen_closed_tab(side) {}
                        assert_eq!(ws.tab_snapshot(side).entries, before.entries);
                        assert_eq!(ws.closed_tab_count(side), 0);
                    }
                }
            }
        }
    }

    #[test]
    fn grouped_closure_rejects_invalid_anchors_without_mutation() {
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        for others in [false, true] {
            for anchor in [0, 1, usize::MAX] {
                let revision = ws.tab_revision();
                assert!(!ws.close_tab_group(ActivePanel::Left, anchor, others));
                assert_eq!(ws.tab_count(ActivePanel::Left), 1);
                assert_eq!(ws.closed_tab_count(ActivePanel::Left), 0);
                assert_eq!(ws.tab_revision(), revision);
            }
        }
    }

    #[test]
    fn reopening_restores_runtime_state_and_revalidates_the_directory() {
        let temp = TempDir::new();
        let child = temp.dir("child");
        let file = temp.file("a.txt", "a");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.left.refresh();
        ws.left.navigate_to(child.clone());
        ws.left.go_up();
        ws.left.refresh();
        ws.left.toggle_select(file.clone());
        ws.left.set_search_query("a");
        ws.left.focus_child("a.txt".into());
        ws.left.set_browser_mode(crate::panel::BrowserMode::Icons);
        ws.new_tab(ActivePanel::Left, Some(child));
        ws.select_tab(ActivePanel::Left, 0);
        assert!(ws.close_tab(ActivePanel::Left));
        assert_eq!(ws.closed_tab_paths(ActivePanel::Left), [temp.path()]);
        temp.file("a-new.txt", "new");
        assert!(ws.reopen_closed_tab(ActivePanel::Left));
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 0);
        assert!(ws.left.can_go_back());
        assert!(ws.left.is_selected(&file));
        assert_eq!(ws.left.search_query(), "a");
        assert_eq!(ws.left.browser_mode(), crate::panel::BrowserMode::Icons);
        assert!(
            ws.left.entries().is_empty(),
            "stale rows must not be published"
        );
        assert!(!ws.left.has_notify());
        ws.left.refresh();
        assert!(
            ws.left
                .entries()
                .iter()
                .any(|entry| entry.name == "a-new.txt")
        );
        assert_eq!(ws.left.cursor_entry().map(|entry| &entry.path), Some(&file));
        assert_eq!(ws.closed_tab_count(ActivePanel::Left), 0);
    }

    #[test]
    fn closed_tab_history_is_bounded_and_can_restore_a_specific_entry() {
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        for index in 0..12 {
            ws.new_tab(ActivePanel::Left, Some(temp.dir(&format!("dir-{index}"))));
            ws.close_tab(ActivePanel::Left);
        }
        assert_eq!(ws.closed_tab_count(ActivePanel::Left), MAX_CLOSED_TABS);
        assert_eq!(ws.closed_tab_count(ActivePanel::Right), 0);
        assert_eq!(
            ws.closed_tab_paths(ActivePanel::Left)[0],
            temp.path().join("dir-4")
        );
        assert!(ws.reopen_closed_tab_at(ActivePanel::Left, 2));
        assert_eq!(ws.left.current_path, temp.path().join("dir-6"));
        assert_eq!(ws.closed_tab_count(ActivePanel::Left), 7);
        assert!(!ws.reopen_closed_tab_at(ActivePanel::Left, 99));
        while ws.tab_count(ActivePanel::Left) < MAX_TABS {
            ws.new_tab(ActivePanel::Left, None);
        }
        let before = ws.closed_tab_paths(ActivePanel::Left);
        assert!(!ws.reopen_closed_tab(ActivePanel::Left));
        assert_eq!(
            ws.closed_tab_paths(ActivePanel::Left),
            before,
            "failed reopen must not consume history"
        );
        assert!(
            !crate::command::availability(
                crate::command::Command::ReopenClosedTab,
                &ws.command_context()
            )
            .enabled
        );
    }

    #[test]
    fn deduplicated_interleaved_tabs_reopen_in_original_order_and_keep_filters() {
        let temp = TempDir::new();
        let a = temp.path().to_path_buf();
        let b = temp.dir("b");
        let c = temp.dir("c");
        let paths = [a.clone(), a.clone(), b.clone(), a.clone(), b, c, a];
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        for (index, path) in paths.iter().enumerate() {
            if index > 0 {
                ws.new_tab(ActivePanel::Left, Some(path.clone()));
            }
            ws.left.set_search_query(format!("tab-{index}"));
        }
        ws.select_tab(ActivePanel::Left, 1);
        assert!(ws.close_duplicate_tabs(ActivePanel::Left));
        assert_eq!(ws.tab_count(ActivePanel::Left), 3);
        assert_eq!(ws.closed_tab_count(ActivePanel::Left), 4);
        for _ in 0..4 {
            assert!(ws.reopen_closed_tab(ActivePanel::Left));
        }
        let actual = ws
            .tab_snapshot(ActivePanel::Left)
            .entries
            .into_iter()
            .map(|tab| tab.path)
            .collect::<Vec<_>>();
        assert_eq!(actual, paths);
        for index in 0..paths.len() {
            ws.select_tab(ActivePanel::Left, index);
            assert_eq!(ws.left.search_query(), format!("tab-{index}"));
        }
    }

    #[test]
    fn reopening_uses_normal_command_guards_and_history_moves_with_its_pane() {
        use crate::command::{Command, availability};
        let temp = TempDir::new();
        let child = temp.dir("child");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        assert!(!availability(Command::ReopenClosedTab, &ws.command_context()).enabled);
        ws.new_tab(ActivePanel::Left, Some(child.clone()));
        ws.close_tab(ActivePanel::Left);
        assert!(availability(Command::ReopenClosedTab, &ws.command_context()).enabled);
        ws.execute(Command::SwapPanels);
        assert_eq!(ws.closed_tab_count(ActivePanel::Left), 0);
        assert_eq!(
            ws.closed_tab_paths(ActivePanel::Right).as_slice(),
            std::slice::from_ref(&child)
        );
        ws.active = ActivePanel::Right;
        ws.execute(Command::ReopenClosedTab);
        assert_eq!(ws.right.current_path, child);
        ws.close_tab(ActivePanel::Right);
        ws.execute(Command::CreateDir);
        assert!(!ws.reopen_closed_tab(ActivePanel::Right));
        assert_eq!(ws.closed_tab_count(ActivePanel::Right), 1);
    }

    #[test]
    fn copying_pointed_tab_path_keeps_active_bindings_and_escapes_controls() {
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        let child = temp.dir("folder name");
        ws.new_tab(ActivePanel::Left, Some(child.clone()));
        ws.select_tab(ActivePanel::Left, 0);
        let before = ws.tab_snapshot(ActivePanel::Left);
        let focus = ws.active;
        assert_eq!(
            ws.tab_path_for_copy(ActivePanel::Left, 1),
            Some(child.to_string_lossy().into_owned())
        );
        assert_eq!(ws.tab_snapshot(ActivePanel::Left).entries, before.entries);
        assert_eq!(ws.active_tab_index(ActivePanel::Left), before.active);
        assert_eq!(ws.active, focus);
        assert_eq!(
            ws.tab_path_for_copy(ActivePanel::Right, 0),
            Some(temp.path().to_string_lossy().into_owned())
        );
        assert!(ws.tab_path_for_copy(ActivePanel::Right, 1).is_none());
        #[cfg(unix)]
        {
            let controls = temp.dir("line\nbreak\t");
            ws.new_background_tab(ActivePanel::Right, controls.clone());
            let copied = ws.tab_path_for_copy(ActivePanel::Right, 1).unwrap();
            assert_eq!(
                copied,
                controls
                    .to_string_lossy()
                    .replace('\n', "\\n")
                    .replace('\t', "\\t")
            );
            assert!(!copied.contains('\n') && !copied.contains('\t'));
        }
    }

    #[test]
    fn close_left_keeps_survivors_bindings_and_reopens_in_original_order() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            for active in 0..3 {
                for anchor in 0..3 {
                    let temp = TempDir::new();
                    let mut ws = Workspace::new(temp.path().into(), temp.path().into());
                    ws.new_tab(side, Some(temp.dir("a")));
                    ws.new_tab(side, Some(temp.dir("b")));
                    ws.select_tab(side, active);
                    let before = ws.tab_snapshot(side);
                    let focus = ws.active;
                    assert_eq!(ws.close_tabs_left(side, anchor), anchor > 0);
                    let after = ws.tab_snapshot(side);
                    assert_eq!(after.entries, before.entries[anchor..]);
                    assert_eq!(after.active, active.saturating_sub(anchor));
                    assert_eq!(ws.active, focus);
                    assert_eq!(ws.tab_count(side.other()), 1);
                    assert_eq!(ws.closed_tab_count(side), anchor);
                    for _ in 0..anchor {
                        assert!(ws.reopen_closed_tab(side));
                    }
                    assert_eq!(ws.tab_snapshot(side).entries, before.entries);
                    assert!(!ws.close_tabs_left(side, 99));
                }
            }
        }
    }

    #[test]
    fn pinned_tab_paths_are_native_bounded_and_optional_for_legacy_records() {
        let tmp = TempDir::new();
        let mut ws = Workspace::new(tmp.path().into(), tmp.path().into());
        #[cfg(unix)]
        let home = {
            use std::os::unix::ffi::OsStringExt;
            tmp.path()
                .join(std::ffi::OsString::from_vec(vec![b'h', 255]))
        };
        #[cfg(windows)]
        let home = tmp.path().join("native-home");
        ws.left.current_path = home.clone();
        assert!(ws.set_tab_pinned(ActivePanel::Left, 0, true));
        ws.left.current_path = tmp.path().into();
        let value = serde_json::to_value(ws.tab_snapshot(ActivePanel::Left)).unwrap();
        let parsed: SavedTabs = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed.entries[0].pinned_path, Some(home));
        for malformed in [
            serde_json::Value::Null,
            serde_json::json!(123),
            serde_json::json!({"platform":"alien","path":[47,97]}),
            serde_json::json!({"platform":std::env::consts::FAMILY,"path":[97]}),
            serde_json::json!({"platform":std::env::consts::FAMILY,"path":[47,0]}),
            serde_json::json!({"platform":std::env::consts::FAMILY,"path":vec![47;16385]}),
        ] {
            let mut bad = value.clone();
            bad["entries"][0]["pinned_path"] = malformed;
            let parsed: SavedTabs = serde_json::from_value(bad).unwrap();
            assert_eq!(parsed.entries.len(), 1);
            assert!(parsed.entries[0].pinned_path.is_none());
        }
    }

    #[test]
    fn pinned_tabs_reset_lazily_on_reactivation_and_unpin_retains_temporary_navigation() {
        use crate::command::{Command, availability};
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let tmp = TempDir::new();
            let home = tmp.dir("home");
            let child = tmp.dir("home/child");
            let picked = tmp.file("home/keep.txt", "kept");
            let mut ws = Workspace::new(home.clone(), home.clone());
            ws.active = side;
            {
                let (_, panel) = ws.tabs_and_panel(side);
                panel.refresh();
                panel.set_browser_mode(crate::panel::BrowserMode::Icons);
                panel.set_search_query("keep");
                panel.focus_child("keep.txt".into());
                panel.select_path(picked.clone());
            }
            assert!(availability(Command::PinTab, &ws.command_context()).enabled);
            ws.execute(Command::PinTab);
            assert!(availability(Command::UnpinTab, &ws.command_context()).enabled);
            assert!(!availability(Command::PinTab, &ws.command_context()).enabled);
            assert!(!ws.set_tab_pinned(side, 0, true));
            assert!(!ws.set_tab_pinned(side, 99, true));
            assert!(ws.new_background_tab(side, tmp.path().into()));
            ws.tabs_and_panel(side).1.navigate_to(child.clone());
            assert_eq!(ws.tabs_and_panel(side).1.current_path, child);
            assert!(ws.select_tab(side, 1));
            assert!(ws.select_tab(side, 0));
            {
                let (_, panel) = ws.tabs_and_panel(side);
                assert_eq!(panel.current_path, home);
                assert!(panel.entries().is_empty());
                assert!(!panel.has_notify());
                let workload = crate::workload::DeterministicWorkload::new(
                    crate::workload::SchedulerLimits::default(),
                );
                panel.set_workload(workload.handle());
                panel.set_notify(std::sync::Arc::new(|| {}));
                panel.refresh();
                panel.ensure_cursor_valid();
                assert!(workload.run_next());
                assert!(panel.poll_listing());
                assert_eq!(panel.cursor_entry().unwrap().path, picked);
                assert!(panel.is_selected(&picked));
                assert_eq!(panel.search_query(), "keep");
                assert_eq!(panel.browser_mode(), crate::panel::BrowserMode::Icons);
                // Suspend the test worker before a synchronous temporary navigation.
                panel.suspend_for_tab();
                panel.navigate_to(child.clone());
            }
            ws.execute(Command::UnpinTab);
            assert!(ws.select_tab(side, 1));
            assert!(ws.select_tab(side, 0));
            assert_eq!(ws.tabs_and_panel(side).1.current_path, child);
            ws.emit_ui_request(crate::ui_request::UiRequest::Rename(picked.clone()));
            assert!(!ws.set_tab_pinned(side, 0, true));
            ws.drain_ui_requests();
            assert_eq!(std::fs::read(picked).unwrap(), b"kept");
        }
    }

    #[test]
    fn pinned_tab_identity_survives_duplicate_transfer_close_and_missing_home() {
        let tmp = TempDir::new();
        let home = tmp.dir("home");
        let away = tmp.dir("away");
        let mut ws = Workspace::new(home.clone(), away.clone());
        assert!(ws.set_tab_pinned(ActivePanel::Left, 0, true));
        ws.left.navigate_to(away.clone());
        assert!(ws.duplicate_tab_at(ActivePanel::Left, 0));
        assert!(ws.move_tab_to_other(ActivePanel::Left, 1));
        assert!(ws.select_tab(ActivePanel::Right, 1));
        assert_eq!(ws.right.current_path, home);
        assert_eq!(ws.right.tab_pinned_path, Some(home.clone()));
        assert!(ws.close_tab(ActivePanel::Right));
        assert!(ws.reopen_closed_tab(ActivePanel::Right));
        assert_eq!(ws.right.current_path, home);
        assert!(ws.new_background_tab(ActivePanel::Right, away.clone()));
        ws.right.navigate_to(away);
        assert!(ws.select_tab(ActivePanel::Right, 2));
        std::fs::remove_dir(&home).unwrap();
        assert!(ws.select_tab(ActivePanel::Right, 1));
        assert_eq!(ws.right.current_path, home);
        ws.right.refresh();
        assert!(ws.right.entries().is_empty());
        assert!(!matches!(
            ws.right.dir_status(),
            crate::panel::DirStatus::Listed
        ));
        assert_eq!(ws.right.tab_pinned_path, Some(home));
    }

    #[test]
    fn captured_cross_pane_transfer_keeps_runtime_state_at_every_insertion_boundary() {
        for source in [ActivePanel::Left, ActivePanel::Right] {
            for move_visible in [false, true] {
                for position in 0..=2 {
                    let temp = TempDir::new();
                    let root = temp.dir("source");
                    let b = temp.dir("B");
                    let a = temp.dir("A");
                    let dest = temp.dir("destination");
                    let extra = temp.dir("extra");
                    let keep = temp.file("B/keep.txt", "moved");
                    let live = temp.file("destination/live.txt", "destination");
                    let mut ws = Workspace::new(root.clone(), dest.clone());
                    if source == ActivePanel::Right {
                        ws = Workspace::new(dest.clone(), root.clone());
                    }
                    assert!(ws.new_background_tab(source, a));
                    assert!(ws.new_background_tab(source, b.clone()));
                    assert!(ws.select_tab(source, 1));
                    {
                        let (_, panel) = ws.tabs_and_panel(source);
                        panel.refresh();
                        panel.set_browser_mode(crate::panel::BrowserMode::Icons);
                        panel.set_search_query("keep");
                        panel.focus_child("keep.txt".into());
                        panel.select_path(keep.clone());
                    }
                    assert!(ws.set_tab_title(source, 1, "Transfer me"));
                    assert!(ws.set_tab_close_protection(source, 1, true));
                    if !move_visible {
                        assert!(ws.select_tab(source, 0));
                    }
                    assert!(ws.new_background_tab(source.other(), extra));
                    let preview = crate::panel::PreviewContent::Image(live.clone());
                    let destination_generation = {
                        let (_, panel) = ws.tabs_and_panel(source.other());
                        panel.refresh();
                        panel.focus_child("live.txt".into());
                        panel.select_path(live.clone());
                        panel.preview = Some(preview.clone());
                        panel.entries_gen()
                    };
                    ws.active = source;
                    let before = ws.tab_snapshot(source);
                    let destination_before = ws.tab_snapshot(source.other());
                    let revision = ws.tab_revision();
                    assert!(ws.transfer_tab_captured(source, 1, position, revision, &b));
                    let mut expected = destination_before.entries.clone();
                    expected.insert(position, before.entries[1].clone());
                    let after = ws.tab_snapshot(source.other());
                    assert_eq!(after.entries, expected);
                    assert_eq!(after.active, usize::from(position == 0));
                    assert_eq!(ws.tab_count(source), 2);
                    assert_eq!(ws.active, source);
                    assert_eq!(ws.closed_tab_count(source), before.closed.len());
                    assert_eq!(
                        ws.closed_tab_count(source.other()),
                        destination_before.closed.len()
                    );
                    {
                        let (tabs, panel) = ws.tabs_and_panel(source.other());
                        assert_eq!(panel.current_path, dest);
                        assert_eq!(panel.entries_gen(), destination_generation);
                        assert_eq!(panel.preview, Some(preview));
                        assert!(panel.is_selected(&live));
                        let moved = tabs.slots[position].as_ref().unwrap();
                        assert_eq!(moved.current_path, b);
                        assert_eq!(moved.search_query(), "keep");
                        assert!(moved.is_selected(&keep));
                        assert!(!moved.has_notify());
                    }
                    assert!(ws.select_tab(source.other(), position));
                    {
                        let (_, panel) = ws.tabs_and_panel(source.other());
                        let workload = crate::workload::DeterministicWorkload::new(
                            crate::workload::SchedulerLimits::default(),
                        );
                        panel.set_workload(workload.handle());
                        panel.set_notify(std::sync::Arc::new(|| {}));
                        panel.refresh();
                        panel.ensure_cursor_valid();
                        assert!(workload.run_next());
                        assert!(panel.poll_listing());
                        assert!(panel.is_selected(&keep));
                        assert_eq!(panel.cursor_entry().unwrap().path, keep);
                    }
                    assert_eq!(std::fs::read(&keep).unwrap(), b"moved");
                    assert_eq!(std::fs::read(&live).unwrap(), b"destination");
                }
            }
        }
    }
    #[test]
    fn captured_cross_pane_transfer_refuses_stale_capacity_last_tab_and_pending_modal() {
        let temp = TempDir::new();
        let b = temp.dir("B");
        let file = temp.file("keep.txt", "keep");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        let revision = ws.tab_revision();
        assert!(!ws.transfer_tab_captured(ActivePanel::Left, 0, 0, revision, temp.path()));
        assert!(ws.new_background_tab(ActivePanel::Left, b.clone()));
        let revision = ws.tab_revision();
        let left = ws.tab_snapshot(ActivePanel::Left);
        let right = ws.tab_snapshot(ActivePanel::Right);
        for (index, position, version, path) in [
            (1, 0, revision.wrapping_add(1), b.clone()),
            (1, 0, revision, temp.path().into()),
            (9, 0, revision, b.clone()),
            (1, 2, revision, b.clone()),
        ] {
            assert!(!ws.transfer_tab_captured(ActivePanel::Left, index, position, version, &path));
            assert_eq!(ws.tab_snapshot(ActivePanel::Left), left);
            assert_eq!(ws.tab_snapshot(ActivePanel::Right), right);
            assert_eq!(ws.tab_revision(), revision);
        }
        ws.emit_ui_request(crate::ui_request::UiRequest::Rename(file));
        assert!(!ws.transfer_tab_captured(ActivePanel::Left, 1, 0, revision, &b));
        ws.drain_ui_requests();
        ws.left.navigate_to(b.clone());
        assert!(!ws.transfer_tab_captured(ActivePanel::Left, 0, 0, revision, temp.path()));
        while ws.tab_count(ActivePanel::Right) < MAX_TABS {
            assert!(ws.new_background_tab(ActivePanel::Right, b.clone()));
        }
        let revision = ws.tab_revision();
        let left = ws.tab_snapshot(ActivePanel::Left);
        let right = ws.tab_snapshot(ActivePanel::Right);
        assert!(!ws.transfer_tab_captured(ActivePanel::Left, 1, 0, revision, &b));
        assert_eq!(ws.tab_snapshot(ActivePanel::Left), left);
        assert_eq!(ws.tab_snapshot(ActivePanel::Right), right);
    }
    #[test]
    fn captured_tab_reorder_keeps_both_previews_selection_and_rejects_changed_bindings() {
        let temp = TempDir::new();
        let a = temp.dir("A");
        let b = temp.dir("B");
        let file = temp.file("keep.txt", "keep");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.left.refresh();
        ws.left.focus_child("keep.txt".into());
        ws.left.select_cursor();
        assert!(ws.new_background_tab(ActivePanel::Left, a));
        assert!(ws.new_background_tab(ActivePanel::Left, b.clone()));
        assert!(ws.set_tab_close_protection(ActivePanel::Left, 1, true));
        let preview = crate::panel::PreviewContent::Image(file.clone());
        ws.left.preview = Some(preview.clone());
        ws.right.preview = Some(preview.clone());
        let generation = ws.left.entries_gen();
        let revision = ws.tab_revision();
        let before = ws.tab_snapshot(ActivePanel::Left);
        assert!(!ws.move_tab_captured(ActivePanel::Left, 1, 0, revision.wrapping_sub(1), &b));
        assert!(!ws.move_tab_captured(ActivePanel::Left, 1, 0, revision, temp.path()));
        assert_eq!(ws.tab_snapshot(ActivePanel::Left), before);
        assert!(ws.move_tab_captured(ActivePanel::Left, 1, 0, revision, &b));
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 1);
        assert!(ws.tab_snapshot(ActivePanel::Left).entries[0].close_protected);
        assert_eq!(ws.left.current_path, temp.path());
        assert_eq!(ws.left.entries_gen(), generation);
        assert!(ws.left.is_selected(&file));
        assert_eq!(ws.left.preview, Some(preview.clone()));
        assert_eq!(ws.right.preview, Some(preview));
        let revision = ws.tab_revision();
        ws.emit_ui_request(crate::ui_request::UiRequest::Rename(file));
        assert!(!ws.move_tab_captured(ActivePanel::Left, 0, 2, revision, &b));
        ws.drain_ui_requests();
        ws.left.navigate_to(b.clone());
        assert!(!ws.move_tab_captured(ActivePanel::Left, 1, 0, revision, temp.path()));
    }
    #[test]
    fn reordering_keeps_visible_panel_and_all_slot_bindings() {
        for active in 0..3 {
            for from in 0..3 {
                for to in 0..3 {
                    let temp = TempDir::new();
                    let mut ws = Workspace::new(temp.path().into(), temp.path().into());
                    ws.new_tab(ActivePanel::Left, Some(temp.dir("a")));
                    ws.new_tab(ActivePanel::Left, Some(temp.dir("b")));
                    ws.select_tab(ActivePanel::Left, active);
                    ws.left.set_search_query("kept");
                    let visible = ws.left.current_path.clone();
                    let mut expected = ws.tab_snapshot(ActivePanel::Left).entries;
                    let moved = expected.remove(from);
                    expected.insert(to, moved);
                    assert_eq!(ws.move_tab(ActivePanel::Left, from, to), from != to);
                    assert_eq!(ws.left.current_path, visible);
                    assert_eq!(ws.left.search_query(), "kept");
                    assert_eq!(ws.tab_snapshot(ActivePanel::Left).entries, expected);
                    let tabs = &ws.tabs[0];
                    assert_eq!(tabs.slots.iter().filter(|slot| slot.is_none()).count(), 1);
                    assert!(tabs.slots[tabs.active].is_none());
                    assert_eq!(ws.tab_count(ActivePanel::Right), 1);
                    for (index, tab) in expected.iter().enumerate() {
                        ws.select_tab(ActivePanel::Left, index);
                        assert_eq!(ws.left.current_path, tab.path);
                    }
                }
            }
        }
    }

    #[test]
    fn deduplication_keeps_active_state_and_first_other_path() {
        let temp = TempDir::new();
        let child = temp.dir("child");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.new_tab(ActivePanel::Left, Some(child.clone()));
        ws.left.set_search_query("first child");
        ws.new_tab(ActivePanel::Left, Some(temp.path().into()));
        ws.left.set_search_query("active state");
        ws.left.set_browser_mode(crate::panel::BrowserMode::Icons);
        ws.new_tab(ActivePanel::Left, Some(child.clone()));
        ws.select_tab(ActivePanel::Left, 2);
        assert_eq!(ws.duplicate_tab_count(ActivePanel::Left), 2);
        assert!(ws.close_duplicate_tabs(ActivePanel::Left));
        assert_eq!(ws.tab_count(ActivePanel::Left), 2);
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 1);
        assert_eq!(ws.left.search_query(), "active state");
        assert_eq!(ws.left.browser_mode(), crate::panel::BrowserMode::Icons);
        assert_eq!(ws.tab_count(ActivePanel::Right), 1);
        assert!(!ws.close_duplicate_tabs(ActivePanel::Left));
        ws.select_tab(ActivePanel::Left, 0);
        assert_eq!(ws.left.current_path, child);
        assert_eq!(ws.left.search_query(), "first child");
    }

    #[test]
    fn tab_organization_commands_obey_boundaries_and_modal_guards() {
        use crate::command::{Command, availability};
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        assert!(!availability(Command::MoveTabLeft, &ws.command_context()).enabled);
        assert!(!availability(Command::MoveTabRight, &ws.command_context()).enabled);
        assert!(!availability(Command::CloseDuplicateTabs, &ws.command_context()).enabled);
        ws.new_tab(ActivePanel::Left, None);
        assert!(availability(Command::CloseDuplicateTabs, &ws.command_context()).enabled);
        ws.execute(Command::MoveTabLeft);
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 0);
        ws.execute(Command::MoveTabRight);
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 1);
        assert!(!ws.move_tab(ActivePanel::Left, 0, 8));
        ws.execute(Command::CreateDir);
        assert!(!ws.move_tab(ActivePanel::Left, 1, 0));
        assert!(!ws.close_duplicate_tabs(ActivePanel::Left));
        assert_eq!(ws.tab_count(ActivePanel::Left), 2);
    }

    #[test]
    fn tabs_preserve_selection_filter_history_and_view_per_pane() {
        let temp = TempDir::new();
        let child = temp.dir("child");
        let file = temp.file("a.txt", "a");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.left.refresh();
        ws.left.navigate_to(child.clone());
        ws.left.go_up();
        ws.left.toggle_select(file.clone());
        ws.left.set_search_query("a");
        ws.left.focus_child("a.txt".to_string());
        ws.left.set_browser_mode(crate::panel::BrowserMode::Icons);
        assert!(ws.new_tab(ActivePanel::Left, Some(child.clone())));
        assert_eq!(ws.left.current_path, child);
        assert!(!ws.left.can_go_back());
        assert_eq!(ws.tab_count(ActivePanel::Right), 1);
        ws.left.set_browser_mode(crate::panel::BrowserMode::List);
        assert!(ws.select_tab(ActivePanel::Left, 0));
        assert!(ws.left.is_selected(&file));
        assert!(ws.left.can_go_back());
        assert!(!ws.left.has_notify());
        ws.left.refresh();
        assert_eq!(ws.left.cursor_entry().map(|entry| &entry.path), Some(&file));
        assert_eq!(ws.left.search_query(), "a");
        assert_eq!(ws.left.browser_mode(), crate::panel::BrowserMode::Icons);
        assert!(!ws.left.has_notify());
        assert!(ws.select_tab(ActivePanel::Left, 1));
        assert_eq!(ws.left.browser_mode(), crate::panel::BrowserMode::List);
    }
    #[test]
    fn move_tab_between_panes_preserves_runtime_state_and_history_without_copying_files() {
        for source in [ActivePanel::Left, ActivePanel::Right] {
            for index in 0..3 {
                for move_visible in [false, true] {
                    let temp = TempDir::new();
                    let a = temp.dir("a");
                    let b = temp.dir("b");
                    let dest = temp.dir("dest");
                    let history = temp.dir("history");
                    for directory in [temp.path().to_path_buf(), a.clone(), b.clone()] {
                        std::fs::write(directory.join("keep.txt"), "payload").unwrap();
                    }
                    temp.file("dest/live.txt", "destination");
                    let mut ws = Workspace::new(temp.path().into(), temp.path().into());
                    ws.tabs_and_panel(source.other())
                        .1
                        .navigate_to(dest.clone());
                    ws.new_tab(source, Some(a));
                    ws.new_tab(source, Some(b));
                    ws.select_tab(source, index);
                    let (_, panel) = ws.tabs_and_panel(source);
                    let moved_path = panel.current_path.clone();
                    panel.navigate_to(history.clone());
                    panel.go_back();
                    panel.refresh();
                    panel.set_browser_mode(crate::panel::BrowserMode::Icons);
                    panel.set_search_query("keep");
                    panel.focus_child("keep.txt".into());
                    panel.select_path(moved_path.join("keep.txt"));
                    if !move_visible {
                        ws.select_tab(source, if index == 1 { 2 } else { 1 });
                    }
                    let old_visible = ws.tab_snapshot(source).entries[ws.active_tab_index(source)]
                        .path
                        .clone();
                    let (_, target) = ws.tabs_and_panel(source.other());
                    target.refresh();
                    target.set_search_query("live");
                    target.focus_child("live.txt".into());
                    target.select_path(dest.join("live.txt"));
                    ws.active = source;
                    assert!(ws.move_tab_to_other(source, index));
                    let (_, target) = ws.tabs_and_panel(source.other());
                    assert_eq!(target.search_query(), "live");
                    assert_eq!(target.cursor_entry().unwrap().name, "live.txt");
                    assert!(target.is_selected(&dest.join("live.txt")));
                    assert_eq!(ws.active, source);
                    assert_eq!(ws.tab_count(source), 2);
                    assert_eq!(ws.tab_count(source.other()), 2);
                    assert_eq!(ws.closed_tab_count(source), 0);
                    assert_eq!(ws.closed_tab_count(source.other()), 0);
                    assert_eq!(ws.tab_snapshot(source.other()).entries[0].path, dest);
                    assert_eq!(ws.active_tab_index(source.other()), 0);
                    if !move_visible {
                        assert_eq!(
                            ws.tab_snapshot(source).entries[ws.active_tab_index(source)].path,
                            old_visible
                        );
                    }
                    let (tabs, _) = ws.tabs_and_panel(source.other());
                    let moved = tabs.slots[1].as_ref().unwrap();
                    assert_eq!(moved.current_path, moved_path);
                    assert_eq!(moved.search_query(), "keep");
                    assert_eq!(moved.browser_mode(), crate::panel::BrowserMode::Icons);
                    assert!(moved.is_selected(&moved_path.join("keep.txt")));
                    assert!(moved.history.can_forward());
                    assert!(!moved.has_notify());
                    assert!(ws.select_tab(source.other(), 1));
                    let (_, moved) = ws.tabs_and_panel(source.other());
                    moved.refresh();
                    assert_eq!(moved.cursor_entry().unwrap().name, "keep.txt");
                    assert!(moved.is_selected(&moved_path.join("keep.txt")));
                    moved.go_forward();
                    assert_eq!(moved.current_path, history);
                    assert_eq!(
                        std::fs::read(moved_path.join("keep.txt")).unwrap(),
                        b"payload"
                    );
                    assert_eq!(
                        std::fs::read(dest.join("live.txt")).unwrap(),
                        b"destination"
                    );
                }
            }
        }
    }

    #[test]
    fn move_tab_between_panes_rejects_last_tab_invalid_index_and_capacity_atomically() {
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        assert!(!ws.move_tab_to_other(ActivePanel::Left, 0));
        ws.new_background_tab(ActivePanel::Left, temp.path().into());
        assert!(!ws.move_tab_to_other(ActivePanel::Left, 99));
        for _ in 1..MAX_TABS {
            ws.new_background_tab(ActivePanel::Right, temp.path().into());
        }
        let left = ws.tab_snapshot(ActivePanel::Left);
        let right = ws.tab_snapshot(ActivePanel::Right);
        let revision = ws.tab_revision();
        assert!(!ws.move_tab_to_other(ActivePanel::Left, 0));
        assert_eq!(ws.tab_snapshot(ActivePanel::Left), left);
        assert_eq!(ws.tab_snapshot(ActivePanel::Right), right);
        assert_eq!(ws.tab_revision(), revision);
    }

    #[test]
    fn duplicate_pointed_tab_copies_its_view_lazily_and_keeps_the_live_panel() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            for source_index in 0..3 {
                let temp = TempDir::new();
                let a = temp.dir("a");
                let b = temp.dir("b");
                temp.file("a/keep.txt", "payload");
                let mut ws = Workspace::new(temp.path().into(), temp.path().into());
                ws.new_tab(side, Some(a.clone()));
                ws.new_tab(side, Some(b));
                ws.select_tab(side, source_index);
                ws.tabs_and_panel(side)
                    .1
                    .set_browser_mode(crate::panel::BrowserMode::Icons);
                ws.select_tab(side, 1);
                ws.active = side.other();
                let (_, live) = ws.tabs_and_panel(side);
                live.refresh();
                live.focus_child("keep.txt".into());
                live.select_path(a.join("keep.txt"));
                live.set_search_query("keep");
                live.preview = live.cursor_entry().and_then(crate::panel::make_preview);
                let snapshot = ws.tab_snapshot(side);
                let revision = ws.tab_revision();
                assert!(ws.duplicate_tab_at(side, source_index));
                assert_eq!(ws.active, side.other());
                assert_eq!(ws.tab_revision(), revision.wrapping_add(1));
                let after = ws.tab_snapshot(side);
                assert_eq!(
                    after.entries[source_index + 1],
                    snapshot.entries[source_index]
                );
                assert_eq!(
                    after.active,
                    snapshot.active + usize::from(source_index < snapshot.active)
                );
                let mut expected = snapshot.entries.clone();
                expected.insert(source_index + 1, snapshot.entries[source_index].clone());
                assert_eq!(after.entries, expected);
                let (tabs, live) = ws.tabs_and_panel(side);
                assert_eq!(live.current_path, a);
                assert_eq!(live.search_query(), "keep");
                assert_eq!(live.cursor_entry().unwrap().name, "keep.txt");
                assert!(live.is_selected(&a.join("keep.txt")));
                assert!(live.preview.is_some());
                let copied = tabs.slots[source_index + 1].as_ref().unwrap();
                assert!(copied.entries().is_empty());
                assert!(!copied.has_notify());
                assert!(copied.selection_is_empty());
                assert_eq!(ws.closed_tab_count(side), 0);
            }
        }
    }

    #[test]
    fn duplicate_pointed_tab_rejects_invalid_index_and_full_capacity_atomically() {
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        let before = ws.tab_snapshot(ActivePanel::Left);
        assert!(!ws.duplicate_tab_at(ActivePanel::Left, 1));
        assert_eq!(ws.tab_snapshot(ActivePanel::Left), before);
        for _ in 1..MAX_TABS {
            assert!(ws.duplicate_tab_at(ActivePanel::Left, 0));
        }
        let before = ws.tab_snapshot(ActivePanel::Left);
        let revision = ws.tab_revision();
        assert!(!ws.duplicate_tab_at(ActivePanel::Left, 0));
        assert_eq!(ws.tab_snapshot(ActivePanel::Left), before);
        assert_eq!(ws.tab_revision(), revision);
    }

    #[test]
    fn close_specific_inactive_tab_keeps_live_panel_and_can_restore_closed_state() {
        for side in [ActivePanel::Left, ActivePanel::Right] {
            for index in [0, 2] {
                let temp = TempDir::new();
                let mut ws = Workspace::new(temp.path().into(), temp.path().into());
                let a = temp.dir("a");
                let b = temp.dir("b");
                temp.file("a/keep.txt", "payload");
                ws.new_tab(side, Some(a.clone()));
                ws.new_tab(side, Some(b));
                ws.select_tab(side, 1);
                ws.active = side.other();
                let (_, live) = ws.tabs_and_panel(side);
                live.refresh();
                live.focus_child("keep.txt".into());
                live.select_path(a.join("keep.txt"));
                live.set_search_query("keep");
                live.preview = live.cursor_entry().and_then(crate::panel::make_preview);
                assert!(live.preview.is_some());
                let before = ws.tab_snapshot(side);
                let revision = ws.tab_revision();
                assert!(ws.close_tab_at(side, index));
                assert_eq!(ws.active, side.other());
                assert_eq!(ws.tab_count(side), 2);
                assert_eq!(ws.tab_revision(), revision.wrapping_add(1));
                let (_, live) = ws.tabs_and_panel(side);
                assert_eq!(live.current_path, a);
                assert_eq!(live.search_query(), "keep");
                assert!(live.preview.is_some());
                assert!(live.is_selected(&a.join("keep.txt")));
                assert_eq!(live.cursor_entry().unwrap().name, "keep.txt");
                assert_eq!(
                    ws.closed_tab_paths(side),
                    vec![before.entries[index].path.clone()]
                );
                assert!(ws.reopen_closed_tab(side));
                assert_eq!(ws.tab_snapshot(side).entries, before.entries);
            }
        }
    }

    #[test]
    fn close_specific_active_tab_and_invalid_requests_preserve_slot_invariants() {
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        assert!(!ws.close_tab_at(ActivePanel::Left, 0));
        let a = temp.dir("a");
        let b = temp.dir("b");
        ws.new_tab(ActivePanel::Left, Some(a));
        ws.new_tab(ActivePanel::Left, Some(b));
        let before = ws.tab_snapshot(ActivePanel::Left);
        assert!(!ws.close_tab_at(ActivePanel::Left, 99));
        assert_eq!(ws.tab_snapshot(ActivePanel::Left), before);
        assert!(ws.close_tab_at(ActivePanel::Left, 2));
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 1);
        assert_eq!(
            ws.tabs[0]
                .slots
                .iter()
                .filter(|slot| slot.is_none())
                .count(),
            1
        );
        assert!(ws.tabs[0].slots[ws.tabs[0].active].is_none());
    }

    #[test]
    fn close_first_middle_last_and_wrap_preserve_slot_invariant() {
        for close in 0..3 {
            let temp = TempDir::new();
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            let a = temp.dir("a");
            let b = temp.dir("b");
            ws.new_tab(ActivePanel::Left, Some(a));
            ws.new_tab(ActivePanel::Left, Some(b));
            ws.select_tab(ActivePanel::Left, close);
            assert!(ws.close_tab(ActivePanel::Left));
            assert_eq!(ws.tab_count(ActivePanel::Left), 2);
            for _ in 0..4 {
                assert!(ws.step_tab(ActivePanel::Left, true));
            }
            let tabs = &ws.tabs[0];
            assert_eq!(tabs.slots.iter().filter(|s| s.is_none()).count(), 1);
            assert!(tabs.slots[tabs.active].is_none());
            assert!(ws.close_tab(ActivePanel::Left));
            assert!(!ws.close_tab(ActivePanel::Left));
        }
    }
    #[test]
    fn restore_is_lazy_bounded_and_retains_resolved_visible_path() {
        let temp = TempDir::new();
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        let saved = SavedTabs {
            entries: (0..20)
                .map(|n| SavedTab {
                    path: PathBuf::from(format!("/missing-{n}")),
                    pinned_path: None,
                    title: None,
                    close_protected: false,
                    full_path_label: false,
                    miller_widths: Default::default(),
                    selection_snapshot: Default::default(),
                    view: ViewConfig::default(),
                })
                .collect(),
            active: 2,
            closed: Vec::new(),
        };
        ws.restore_tabs(&saved, &SavedTabs::default());
        assert_eq!(ws.left.current_path, temp.path());
        assert_eq!(ws.tab_count(ActivePanel::Left), MAX_TABS);
        assert!(!ws.new_tab(ActivePanel::Left, None));
        assert!(ws.select_tab(ActivePanel::Left, 0));
        assert_eq!(ws.left.current_path, PathBuf::from("/missing-0"));
        assert!(ws.left.entries().is_empty());
    }
    #[test]
    fn malformed_saved_tabs_keep_valid_siblings_and_active_binding() {
        let saved:SavedTabs=serde_json::from_value(serde_json::json!({"entries":[{"path":"/a"},{"path":42},{"path":"/b"},{"path":"relative"}],"active":2})).unwrap();
        assert_eq!(saved.entries.len(), 2);
        assert_eq!(saved.active, 1);
        assert_eq!(saved.entries[saved.active].path, PathBuf::from("/b"));
    }
    #[test]
    fn switching_clears_cross_pane_preview_and_revalidates_background_changes() {
        let temp = TempDir::new();
        let child = temp.dir("child");
        let file = temp.file("old.txt", "old");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.left.refresh();
        ws.left.set_cursor(1);
        ws.new_tab(ActivePanel::Left, Some(child));
        std::fs::remove_file(file).unwrap();
        temp.file("new.txt", "new");
        ws.right.preview = Some(crate::panel::PreviewContent::Image(
            temp.path().join("unrelated.png"),
        ));
        ws.select_tab(ActivePanel::Left, 0);
        assert!(ws.right.preview.is_none());
        ws.left.refresh();
        assert!(
            ws.left
                .entries()
                .iter()
                .any(|entry| entry.name == "new.txt")
        );
        assert!(
            !ws.left
                .entries()
                .iter()
                .any(|entry| entry.name == "old.txt")
        );
    }
    #[test]
    fn swap_panels_moves_tab_groups_and_active_slots_together() {
        let temp = TempDir::new();
        let child = temp.dir("child");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.new_tab(ActivePanel::Left, Some(child.clone()));
        ws.execute(crate::command::Command::SwapPanels);
        assert_eq!(ws.tab_count(ActivePanel::Left), 1);
        assert_eq!(ws.tab_count(ActivePanel::Right), 2);
        assert_eq!(ws.right.current_path, child);
        assert!(ws.select_tab(ActivePanel::Right, 0));
        assert_eq!(ws.right.current_path, temp.path());
    }

    #[test]
    fn folder_name_command_enables_grouping_and_preserves_cursor_and_tab_settings() {
        use crate::command::Command;
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            temp.dir("Zulu");
            temp.dir("Alpha");
            let file = temp.file("large.txt", "12345");
            temp.file("small.txt", "1");
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.active = side;
            ws.active_panel().refresh();
            ws.active_panel().replace_selection([file.clone()]);
            let row = ws
                .active_panel_ref()
                .entries()
                .iter()
                .position(|e| e.path == file)
                .unwrap()
                + 1;
            ws.active_panel().set_cursor(row);
            let other = ws.tab_snapshot(side.other());
            ws.execute(Command::ToggleFoldersFirst);
            assert!(!ws.active_panel_ref().view_config().folders_first());
            ws.execute(Command::SortBySize);
            ws.execute(Command::SortBySize);
            ws.execute(Command::ToggleFoldersByName);
            assert!(ws.active_panel_ref().view_config().folders_first());
            assert!(ws.active_panel_ref().view_config().folders_by_name());
            assert_eq!(
                ws.active_panel_ref()
                    .entries()
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>(),
                ["Alpha", "Zulu", "large.txt", "small.txt"]
            );
            assert_eq!(ws.active_panel_ref().cursor_entry().unwrap().path, file);
            assert!(ws.active_panel_ref().is_selected(&file));
            assert_eq!(ws.tab_snapshot(side.other()), other);
            assert!(ws.duplicate_tab_at(side, 0));
            assert!(ws.tab_snapshot(side).entries[1].view.folders_by_name());
            let saved: SavedTabs =
                serde_json::from_str(&serde_json::to_string(&ws.tab_snapshot(side)).unwrap())
                    .unwrap();
            assert!(saved.entries[1].view.folders_by_name());
            ws.execute(Command::ToggleFoldersFirst);
            assert!(!ws.active_panel_ref().view_config().folders_by_name());
            assert!(!ws.active_panel_ref().view_config().folders_first());
            assert!(ws.tab_snapshot(side).entries[1].view.folders_by_name());
            assert_eq!(ws.active, side);
        }
    }

    #[test]
    fn case_sensitive_sort_command_preserves_focus_and_follows_tab_view_copies() {
        use crate::command::Command;
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let alpha = temp.file("alpha.txt", "x");
            temp.file("Beta.txt", "x");
            temp.file("Zulu.txt", "x");
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.active = side;
            ws.active_panel().refresh();
            ws.active_panel().replace_selection([alpha.clone()]);
            ws.active_panel().set_cursor(1);
            let other = ws.tab_snapshot(side.other());
            ws.execute(Command::ToggleCaseSensitiveSort);
            assert_eq!(
                ws.active_panel_ref()
                    .entries()
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>(),
                ["Beta.txt", "Zulu.txt", "alpha.txt"]
            );
            assert_eq!(ws.active_panel_ref().cursor_entry().unwrap().path, alpha);
            assert!(ws.active_panel_ref().is_selected(&alpha));
            assert_eq!(ws.tab_snapshot(side.other()), other);
            assert!(ws.duplicate_tab_at(side, 0));
            assert!(
                ws.tab_snapshot(side).entries[1]
                    .view
                    .case_sensitive_name_sort()
            );
            ws.execute(Command::ToggleCaseSensitiveSort);
            assert_eq!(ws.active_panel_ref().entries()[0].path, alpha);
            assert!(
                !ws.active_panel_ref()
                    .view_config()
                    .case_sensitive_name_sort()
            );
            assert!(
                ws.tab_snapshot(side).entries[1]
                    .view
                    .case_sensitive_name_sort()
            );
            let saved: SavedTabs =
                serde_json::from_str(&serde_json::to_string(&ws.tab_snapshot(side)).unwrap())
                    .unwrap();
            assert!(saved.entries[1].view.case_sensitive_name_sort());
            assert_eq!(ws.active, side);
        }
    }

    #[test]
    fn stem_sort_command_preserves_cursor_selection_and_saved_view() {
        use crate::{
            command::Command,
            panel::{SortColumn, SortOrder},
        };
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let a = temp.file("a.txt", "x");
            temp.file("a-z.txt", "x");
            temp.file("b.txt", "x");
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.active = side;
            ws.active_panel().refresh();
            ws.active_panel().replace_selection([a.clone()]);
            let row = ws
                .active_panel_ref()
                .entries()
                .iter()
                .position(|e| e.path == a)
                .unwrap()
                + 1;
            ws.active_panel().set_cursor(row);
            let other = ws.tab_snapshot(side.other());
            ws.execute(Command::SortByStem);
            assert_eq!(
                ws.active_panel_ref()
                    .entries()
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>(),
                ["a.txt", "a-z.txt", "b.txt"]
            );
            assert_eq!(ws.active_panel_ref().sort_column(), SortColumn::Stem);
            assert_eq!(
                ws.active_panel_ref().view_config().sort_order(),
                SortOrder::Asc
            );
            ws.execute(Command::SortByStem);
            assert_eq!(
                ws.active_panel_ref()
                    .entries()
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>(),
                ["b.txt", "a-z.txt", "a.txt"]
            );
            assert_eq!(ws.active_panel_ref().cursor_entry().unwrap().path, a);
            assert!(ws.active_panel_ref().is_selected(&a));
            assert_eq!(ws.tab_snapshot(side.other()), other);
            let saved: SavedTabs =
                serde_json::from_str(&serde_json::to_string(&ws.tab_snapshot(side)).unwrap())
                    .unwrap();
            assert_eq!(saved.entries[0].view.sort_column(), SortColumn::Stem);
            assert_eq!(saved.entries[0].view.sort_order(), SortOrder::Desc);
            assert_eq!(ws.active, side);
        }
    }

    #[test]
    fn persistent_tab_selection_follows_closed_tabs_and_survives_optional_corruption() {
        use crate::command::Command;
        let temp = crate::testutil::TempDir::new();
        let a = temp.dir("a");
        let b = temp.dir("b");
        let file = a.join("selected.txt");
        std::fs::write(&file, "content").unwrap();
        let mut ws = Workspace::new(a.clone(), b.clone());
        ws.left.refresh();
        ws.left.select_path(file.clone());
        ws.execute(Command::SaveTabSelection);
        assert!(ws.new_background_tab(ActivePanel::Left, b.clone()));
        assert!(ws.close_tab(ActivePanel::Left));
        let saved = ws.tab_snapshot(ActivePanel::Left);
        let encoded = serde_json::to_vec(&saved).unwrap();
        let restored: SavedTabs = serde_json::from_slice(&encoded).unwrap();
        let mut fresh = Workspace::new(b.clone(), b);
        fresh.restore_tabs(&restored, &Default::default());
        fresh.execute(Command::ReopenClosedTab);
        assert_eq!(fresh.left.current_path, a);
        assert_eq!(fresh.left.selected_count(), 0);
        fresh.left.refresh();
        fresh.execute(Command::RestoreTabSelection);
        assert!(fresh.left.is_selected(&file));
        let mut bad = serde_json::to_value(fresh.tab_snapshot(ActivePanel::Left)).unwrap();
        bad["entries"][0]["selection_snapshot"] = serde_json::json!({"bad":"value"});
        let recovered: SavedTabs = serde_json::from_value(bad).unwrap();
        assert!(!recovered.entries.is_empty());
        assert!(recovered.entries[0].selection_snapshot.is_none());
    }
    #[test]
    fn selection_memory_commands_follow_owned_tabs_and_respect_modals() {
        use crate::command::{Command, availability};
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let file = temp.file("keep.txt", "x");
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.active = side;
            ws.active_panel().refresh();
            ws.active_panel().replace_selection([file.clone()]);
            assert!(availability(Command::SaveSelection, &ws.command_context()).enabled);
            assert!(!availability(Command::RestoreSelection, &ws.command_context()).enabled);
            ws.execute(Command::SaveSelection);
            ws.active_panel().clear_selection();
            ws.new_tab(side, None);
            ws.active_panel().refresh();
            assert!(!availability(Command::RestoreSelection, &ws.command_context()).enabled);
            ws.select_tab(side, 0);
            ws.active_panel().refresh();
            assert!(availability(Command::RestoreSelection, &ws.command_context()).enabled);
            ws.execute(Command::RestoreSelection);
            assert!(ws.active_panel_ref().is_selected(&file));
            ws.close_tab(side);
            ws.reopen_closed_tab(side);
            ws.active_panel().refresh();
            ws.active_panel().clear_selection();
            ws.execute(Command::RestoreSelection);
            assert!(ws.active_panel_ref().is_selected(&file));
            ws.active_panel().clear_selection();
            ws.emit_ui_request(crate::ui_request::UiRequest::Rename(file.clone()));
            ws.execute(Command::SaveSelection);
            ws.execute(Command::RestoreSelection);
            assert!(ws.active_panel_ref().selection_is_empty());
            ws.drain_ui_requests();
            ws.execute(Command::RestoreSelection);
            assert!(ws.active_panel_ref().is_selected(&file));
            assert_eq!(ws.tab_count(side.other()), 1);
            assert_eq!(ws.active, side);
            let other = if side == ActivePanel::Left {
                &ws.right
            } else {
                &ws.left
            };
            assert!(other.selection_is_empty());
            assert!(!other.can_restore_selection());
        }
    }

    #[test]
    fn palette_group_closing_matches_protection_counts_and_recovers_original_order() {
        use crate::command::{Command, availability};
        for side in [ActivePanel::Left, ActivePanel::Right] {
            for protected_mask in 0..16 {
                for anchor in 0..4 {
                    for command in [
                        Command::CloseOtherTabs,
                        Command::CloseTabsLeft,
                        Command::CloseTabsRight,
                    ] {
                        let temp = TempDir::new();
                        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
                        ws.active = side;
                        ws.active_panel().set_search_query("state 0");
                        for index in 1..4 {
                            ws.new_tab(side, Some(temp.dir(&format!("tab-{index}"))));
                            ws.active_panel().set_search_query(format!("state {index}"));
                        }
                        for index in 0..4 {
                            ws.set_tab_close_protection(
                                side,
                                index,
                                protected_mask & (1 << index) != 0,
                            );
                        }
                        ws.select_tab(side, anchor);
                        let before = ws.tab_snapshot(side);
                        let other = ws.tab_snapshot(side.other());
                        let removed = |index: usize| {
                            !before.entries[index].close_protected
                                && match command {
                                    Command::CloseTabsLeft => index < anchor,
                                    Command::CloseTabsRight => index > anchor,
                                    _ => index != anchor,
                                }
                        };
                        let expected: Vec<_> = before
                            .entries
                            .iter()
                            .enumerate()
                            .filter(|(index, _)| !removed(*index))
                            .map(|(_, tab)| tab.clone())
                            .collect();
                        let count = 4 - expected.len();
                        assert_eq!(
                            availability(command, &ws.command_context()).enabled,
                            count > 0
                        );
                        let revision = ws.tab_revision();
                        ws.execute(command);
                        assert_eq!(ws.tab_snapshot(side).entries, expected);
                        assert_eq!(ws.closed_tab_count(side), count);
                        assert_eq!(ws.tab_revision() != revision, count > 0);
                        assert_eq!(
                            ws.active_panel_ref().search_query(),
                            format!("state {anchor}")
                        );
                        assert_eq!(ws.tab_snapshot(side.other()), other);
                        assert_eq!(ws.active, side);
                        while ws.reopen_closed_tab(side) {}
                        assert_eq!(ws.tab_snapshot(side).entries, before.entries);
                        for index in 0..4 {
                            ws.select_tab(side, index);
                            assert_eq!(
                                ws.active_panel_ref().search_query(),
                                format!("state {index}")
                            );
                        }
                        ws.select_tab(side, 1);
                        ws.emit_ui_request(crate::ui_request::UiRequest::Rename(
                            temp.file("modal.txt", "x"),
                        ));
                        let before = ws.tab_snapshot(side);
                        let revision = ws.tab_revision();
                        ws.execute(command);
                        assert_eq!(ws.tab_snapshot(side), before);
                        assert_eq!(ws.tab_revision(), revision);
                    }
                }
            }
        }
    }

    #[test]
    fn numbered_tab_commands_restore_every_slot_without_retargeting_other_pane() {
        use crate::command::{Command, availability};
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.active = side;
            ws.active_panel().set_search_query("slot 0");
            assert!(!availability(Command::GoToTab(1), &ws.command_context()).enabled);
            let revision = ws.tab_revision();
            ws.execute(Command::GoToTab(1));
            assert_eq!(ws.tab_revision(), revision);
            for index in 1..MAX_TABS {
                ws.new_tab(side, Some(temp.dir(&format!("slot-{index}"))));
                ws.active_panel().set_search_query(format!("slot {index}"));
                ws.set_tab_title(side, index, &format!("Title {index}"));
                ws.set_tab_close_protection(side, index, index % 2 == 0);
            }
            let other = ws.tab_snapshot(side.other());
            ws.last_command = Some(Command::ReverseSort);
            for index in 0..MAX_TABS {
                assert!(availability(Command::GoToTab(index), &ws.command_context()).enabled);
                ws.execute(Command::GoToTab(index));
                assert_eq!(ws.active_tab_index(side), index);
                assert_eq!(
                    ws.active_panel_ref().search_query(),
                    format!("slot {index}")
                );
                assert!(!availability(Command::GoToTab(index), &ws.command_context()).enabled);
                let revision = ws.tab_revision();
                ws.execute(Command::GoToTab(index));
                assert_eq!(ws.tab_revision(), revision);
            }
            for index in [MAX_TABS, usize::MAX] {
                assert!(!availability(Command::GoToTab(index), &ws.command_context()).enabled);
                let revision = ws.tab_revision();
                ws.execute(Command::GoToTab(index));
                assert_eq!(ws.tab_revision(), revision);
            }
            assert_eq!(ws.tab_snapshot(side.other()), other);
            assert_eq!(ws.last_command, Some(Command::ReverseSort));
            assert_eq!(ws.active, side);
            assert_eq!(ws.closed_tab_count(side), 0);
            ws.emit_ui_request(crate::ui_request::UiRequest::Rename(
                temp.file("modal.txt", "x"),
            ));
            let revision = ws.tab_revision();
            ws.execute(Command::GoToTab(0));
            assert_eq!(ws.active_tab_index(side), MAX_TABS - 1);
            assert_eq!(ws.tab_revision(), revision);
        }
    }

    #[test]
    fn tab_edge_commands_restore_runtime_state_and_respect_boundaries() {
        use crate::command::{Command, availability};
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.active = side;
            assert!(!availability(Command::FirstTab, &ws.command_context()).enabled);
            assert!(!availability(Command::LastTab, &ws.command_context()).enabled);
            ws.active_panel().set_search_query("first state");
            ws.set_tab_close_protection(side, 0, true);
            ws.new_tab(side, Some(temp.dir("middle")));
            ws.active_panel().set_search_query("middle state");
            ws.new_tab(side, Some(temp.dir("last")));
            ws.active_panel().set_search_query("last state");
            let other = ws.tab_snapshot(side.other());
            ws.last_command = Some(Command::ReverseSort);
            assert!(availability(Command::FirstTab, &ws.command_context()).enabled);
            assert!(!availability(Command::LastTab, &ws.command_context()).enabled);
            ws.execute(Command::FirstTab);
            assert_eq!(ws.active_tab_index(side), 0);
            assert_eq!(ws.active_panel_ref().search_query(), "first state");
            assert!(ws.active_panel_ref().tab_close_protected);
            assert!(!availability(Command::FirstTab, &ws.command_context()).enabled);
            assert!(availability(Command::LastTab, &ws.command_context()).enabled);
            let revision = ws.tab_revision();
            ws.execute(Command::FirstTab);
            assert_eq!(ws.tab_revision(), revision);
            ws.execute(Command::LastTab);
            assert_eq!(ws.active_tab_index(side), 2);
            assert_eq!(ws.active_panel_ref().search_query(), "last state");
            assert_eq!(ws.active, side);
            assert_eq!(ws.tab_snapshot(side.other()), other);
            ws.select_tab(side, 1);
            assert_eq!(ws.active_panel_ref().search_query(), "middle state");
            assert_eq!(ws.closed_tab_count(side), 0);
            assert_eq!(ws.last_command, Some(Command::ReverseSort));
            ws.emit_ui_request(crate::ui_request::UiRequest::Rename(
                temp.file("modal.txt", "x"),
            ));
            let revision = ws.tab_revision();
            for command in [Command::FirstTab, Command::LastTab] {
                ws.execute(command);
                assert_eq!(ws.active_tab_index(side), 1);
                assert_eq!(ws.tab_revision(), revision);
                assert_eq!(ws.active_panel_ref().search_query(), "middle state");
            }
        }
    }

    #[test]
    fn protection_palette_commands_follow_active_tab_and_preserve_runtime_state() {
        use crate::command::Command;
        for side in [ActivePanel::Left, ActivePanel::Right] {
            let temp = TempDir::new();
            let mut ws = Workspace::new(temp.path().into(), temp.path().into());
            ws.new_tab(side, Some(temp.dir("work")));
            ws.active = side;
            ws.active_panel().set_search_query("retained");
            let before = ws.tab_snapshot(side);
            let other = ws.tab_snapshot(side.other());
            assert!(
                crate::command::availability(Command::ProtectTab, &ws.command_context()).enabled
            );
            assert!(
                !crate::command::availability(Command::UnprotectTab, &ws.command_context()).enabled
            );
            ws.execute(Command::ProtectTab);
            assert!(
                !crate::command::availability(Command::ProtectTab, &ws.command_context()).enabled
            );
            assert!(
                crate::command::availability(Command::UnprotectTab, &ws.command_context()).enabled
            );
            assert!(
                !crate::command::availability(Command::CloseTab, &ws.command_context()).enabled
            );
            assert!(ws.active_panel_ref().tab_close_protected);
            assert_eq!(ws.active_panel_ref().search_query(), "retained");
            assert_eq!(ws.active_tab_index(side), before.active);
            assert_eq!(ws.tab_snapshot(side.other()), other);
            ws.execute(Command::UnprotectTab);
            assert!(crate::command::availability(Command::CloseTab, &ws.command_context()).enabled);
            assert_eq!(ws.tab_snapshot(side), before);
            assert_eq!(ws.active_panel_ref().search_query(), "retained");
            assert_eq!(ws.active, side);
        }
    }

    #[test]
    fn pending_modal_prevents_tab_changes_and_cannot_retarget_rename() {
        let temp = TempDir::new();
        let file = temp.file("a.txt", "a");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.left.refresh();
        ws.new_tab(ActivePanel::Left, None);
        ws.left.refresh();
        ws.left.set_cursor(1);
        ws.execute(crate::command::Command::BeginRename);
        assert!(!ws.select_tab(ActivePanel::Left, 0));
        assert!(!ws.new_tab(ActivePanel::Left, None));
        assert!(!ws.copy_tabs_to_other(ActivePanel::Left));
        assert!(!ws.new_background_tab(ActivePanel::Left, temp.path().join("blocked")));
        assert!(!ws.close_tab(ActivePanel::Left));
        assert!(!ws.close_tab_at(ActivePanel::Left, 0));
        assert!(!ws.close_tab_at(ActivePanel::Left, 1));
        assert!(!ws.duplicate_tab_at(ActivePanel::Left, 0));
        assert!(!ws.move_tab_to_other(ActivePanel::Left, 0));
        assert!(!ws.close_tabs_left(ActivePanel::Left, 1));
        assert!(!ws.set_tab_title(ActivePanel::Left, 0, "blocked"));
        assert!(!ws.set_tab_close_protection(ActivePanel::Left, 0, true));
        assert!(!ws.close_tab_group(ActivePanel::Left, 0, true));
        assert!(!ws.close_tab_group(ActivePanel::Left, 0, false));
        ws.execute(crate::command::Command::FirstTab);
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 1);
        ws.execute(crate::command::Command::LastTab);
        assert_eq!(ws.active_tab_index(ActivePanel::Left), 1);
        ws.execute(crate::command::Command::ProtectTab);
        assert!(!ws.left.tab_close_protected);
        let requests = ws.drain_ui_requests();
        assert!(requests.contains(&crate::ui_request::UiRequest::Rename(file)));
    }

    #[test]
    fn restored_tabs_round_trip_preserves_views_and_original_active_index() {
        let temp = TempDir::new();
        let child = temp.dir("child");
        let mut ws = Workspace::new(temp.path().into(), temp.path().into());
        ws.left.set_browser_mode(crate::panel::BrowserMode::Icons);
        ws.new_tab(ActivePanel::Left, Some(child));
        ws.left.set_browser_mode(crate::panel::BrowserMode::List);
        let saved = ws.tab_snapshot(ActivePanel::Left);
        let decoded: SavedTabs =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(decoded, saved);
        let mut restored =
            Workspace::new(saved.entries[saved.active].path.clone(), temp.path().into());
        restored.restore_tabs(&decoded, &SavedTabs::default());
        assert!(restored.select_tab(ActivePanel::Left, 0));
        assert_eq!(
            restored.left.browser_mode(),
            crate::panel::BrowserMode::Icons
        );
    }
}

#[cfg(test)]
mod miller_session_tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn fresh_workspace_restores_geometry_and_respects_cli_path_override() {
        let mut original = Workspace::new(PathBuf::from("/tmp"), PathBuf::from("/"));
        original
            .left
            .resize_miller_column(PathBuf::from("/"), 281.0);
        let saved = original.tab_snapshot(ActivePanel::Left);
        let saved: SavedTabs =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        let mut fresh = Workspace::new(PathBuf::from("/tmp"), PathBuf::from("/"));
        fresh.restore_tabs(&saved, &SavedTabs::default());
        assert_eq!(fresh.left.miller_column_width(Path::new("/"), 220.0), 281.0);
        let mut overridden = Workspace::new(PathBuf::from("/"), PathBuf::from("/"));
        overridden.restore_tabs(&saved, &SavedTabs::default());
        assert_eq!(
            overridden.left.miller_column_width(Path::new("/"), 220.0),
            220.0
        );
        let mut malformed = serde_json::to_value(&saved).unwrap();
        malformed["entries"][0]["miller_widths"] = serde_json::json!({"bad": true});
        let restored: SavedTabs = serde_json::from_value(malformed).unwrap();
        assert_eq!(restored.entries.len(), 1);
        assert!(restored.entries[0].miller_widths.is_empty());
    }
}

#[cfg(test)]
mod closed_history_persistence_tests {
    use super::*;
    #[test]
    fn closed_history_is_optional_tolerant_recent_bounded_and_lazy() {
        let mut rows: Vec<_> = (0..20)
            .map(|n| serde_json::json!({"tab":{"path":format!("/missing-{n}")},"position":999}))
            .collect();
        rows.push(serde_json::json!({"tab":{"path":"relative"}}));
        rows.push(serde_json::json!({"tab":false}));
        let saved: SavedTabs = serde_json::from_value(serde_json::json!({"closed":rows})).unwrap();
        assert_eq!(saved.closed.len(), MAX_CLOSED_TABS);
        assert_eq!(saved.closed[0].tab.path, PathBuf::from("/missing-12"));
        assert_eq!(saved.closed[7].tab.path, PathBuf::from("/missing-19"));
        assert!(saved.closed.iter().all(|c| c.position == MAX_TABS));
        let mut ws = Workspace::new("/".into(), "/".into());
        ws.restore_tabs(&saved, &SavedTabs::default());
        assert_eq!(ws.left.current_path, PathBuf::from("/"));
        assert_eq!(ws.closed_tab_count(ActivePanel::Left), MAX_CLOSED_TABS);
        assert!(ws.reopen_closed_tab(ActivePanel::Left));
        assert_eq!(ws.left.current_path, PathBuf::from("/missing-19"));
        assert!(ws.left.entries().is_empty());
        let legacy: SavedTabs =
            serde_json::from_value(serde_json::json!({"entries":[{"path":"/"}],"closed":false}))
                .unwrap();
        assert!(legacy.closed.is_empty());
        assert_eq!(legacy.entries.len(), 1);
        assert!(!serde_json::to_string(&legacy).unwrap().contains("closed"));
    }
}

#[cfg(test)]
mod closed_history_management_tests {
    use super::*;
    #[test]
    fn forgetting_duplicate_locations_is_exact_and_preserves_live_owners() {
        let root = crate::testutil::TempDir::new();
        let file = root.file("a.txt", "data");
        let mut ws = Workspace::new(root.path().into(), root.path().into());
        for side in [ActivePanel::Left, ActivePanel::Right] {
            for title in ["First", "Second"] {
                assert!(ws.new_tab(side, None));
                let index = ws.active_tab_index(side);
                assert!(ws.set_tab_title(side, index, title));
                assert!(ws.close_tab(side));
            }
        }
        ws.left.refresh();
        ws.left.focus_child("a.txt".into());
        ws.left.select_cursor();
        ws.right.preview = Some(crate::panel::PreviewContent::Text {
            identity: crate::panel::PreviewIdentity {
                path: file.clone(),
                size: 4,
                modified: None,
            },
            content: std::sync::Arc::from("preview"),
        });
        let preview = ws.right.preview.clone();
        let revision = ws.left.entries_gen();
        let other = ws.tab_snapshot(ActivePanel::Right);
        let tab_revision = ws.tab_revision();
        assert!(!ws.forget_closed_tab_at(ActivePanel::Left, 99));
        assert_eq!(ws.tab_revision(), tab_revision);
        assert!(ws.forget_closed_tab_at(ActivePanel::Left, 0));
        assert_eq!(
            ws.tab_snapshot(ActivePanel::Left).closed[0]
                .tab
                .title
                .as_deref(),
            Some("Second")
        );
        assert_eq!(ws.tab_revision(), tab_revision + 1);
        assert_eq!(ws.tab_snapshot(ActivePanel::Right), other);
        assert_eq!(ws.right.preview, preview);
        assert_eq!(ws.left.entries_gen(), revision);
        assert_eq!(ws.left.cursor_entry().unwrap().path, file);
        assert!(ws.left.is_selected(&file));
        assert!(ws.clear_closed_tabs(ActivePanel::Left));
        assert!(!ws.clear_closed_tabs(ActivePanel::Left));
        assert_eq!(ws.right.preview, preview);
        assert!(file.exists());
        let restored: SavedTabs = serde_json::from_slice(
            &serde_json::to_vec(&ws.tab_snapshot(ActivePanel::Left)).unwrap(),
        )
        .unwrap();
        assert!(restored.closed.is_empty());
        ws.emit_ui_request(crate::ui_request::UiRequest::Rename(file));
        assert!(!ws.clear_closed_tabs(ActivePanel::Right));
        assert!(!ws.forget_closed_tab_at(ActivePanel::Right, 0));
        assert_eq!(ws.tab_snapshot(ActivePanel::Right), other);
    }
}
