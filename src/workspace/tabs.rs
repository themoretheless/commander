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
        skip_serializing_if = "crate::persistence::is_false",
        deserialize_with = "deserialize_protection"
    )]
    pub close_protected: bool,
    #[serde(
        default,
        skip_serializing_if = "crate::persistence::is_false",
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

    /// Admit only a captured lexical path component, including the current folder/root.
    pub(crate) fn open_breadcrumb_tab(
        &mut self,
        side: ActivePanel,
        owner: &std::path::Path,
        generation: u64,
        revision: u64,
        folder: PathBuf,
        activate: bool,
    ) -> bool {
        let panel = match side {
            ActivePanel::Left => &self.left,
            ActivePanel::Right => &self.right,
        };
        if self.tab_revision() != revision
            || panel.current_path != owner
            || panel.entries_gen() != generation
            || !folder.is_absolute()
            || !owner.ancestors().any(|path| path == folder)
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
mod tests;

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
