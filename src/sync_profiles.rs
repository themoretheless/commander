//! Saved comparison inputs only: plans, edits, stamps and acknowledgements are never persisted.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub name: String,
    pub left: PathBuf,
    pub right: PathBuf,
    pub policy: crate::sync::SyncPolicy,
    pub left_hidden: bool,
    pub right_hidden: bool,
    pub compare_contents: bool,
    pub dry_run: bool,
}
impl Preset {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > 80
            || self.name.chars().any(char::is_control)
        {
            return Err("Use 1–80 characters without controls for a sync profile name".into());
        }
        if !self.left.is_absolute()
            || !self.right.is_absolute()
            || [&self.left, &self.right]
                .iter()
                .any(|p| p.to_string_lossy().contains('\0'))
        {
            return Err("Sync profile folders must be absolute native paths".into());
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Book {
    format: String,
    version: u32,
    profiles: Vec<Preset>,
}
#[derive(Default)]
pub struct State {
    saved: serde_json::Value,
    pub entries: Vec<Preset>,
    pub name: String,
    pub selected: Option<String>,
    pub error: Option<String>,
    valid: bool,
}
impl State {
    pub fn from_saved(saved: serde_json::Value) -> Self {
        let mut s = Self {
            saved,
            valid: true,
            ..Default::default()
        };
        if !s.saved.is_null() {
            match s.decode() {
                Ok(rows) => s.entries = rows,
                Err(error) => {
                    s.valid = false;
                    s.error = Some(error);
                }
            }
        }
        s
    }
    fn decode(&self) -> Result<Vec<Preset>, String> {
        if serde_json::to_vec(&self.saved)
            .map_err(|e| e.to_string())?
            .len()
            > 64 * 1024
        {
            return Err("Saved sync profiles exceed 64 KiB".into());
        }
        let book: Book = serde_json::from_value(self.saved.clone()).map_err(|e| e.to_string())?;
        if book.format != "commander.sync-profiles" || book.version != 1 || book.profiles.len() > 16
        {
            return Err("Unsupported sync profile format, version or count".into());
        }
        let mut names = std::collections::HashSet::new();
        for p in &book.profiles {
            p.validate()?;
            if !names.insert(&p.name) {
                return Err("Duplicate sync profile names".into());
            }
        }
        Ok(book.profiles)
    }
    pub fn saved(&self) -> serde_json::Value {
        self.saved.clone()
    }
    pub fn save(&mut self, preset: Preset, replace: bool) -> Result<(), String> {
        if !self.valid {
            return Err(
                "Saved sync profiles are invalid; Clear saved profiles before replacing them"
                    .into(),
            );
        }
        preset.validate()?;
        let mut next = self.entries.clone();
        let index = next.iter().position(|p| p.name == preset.name);
        if replace {
            let index = index.ok_or("Select an existing sync profile")?;
            next[index] = preset.clone();
        } else {
            if index.is_some() {
                return Err("That sync profile exists; use Replace selected".into());
            }
            if next.len() >= 16 {
                return Err("At most 16 saved sync profiles".into());
            }
            next.push(preset.clone());
        }
        self.publish(next, Some(preset.name))
    }
    fn publish(&mut self, next: Vec<Preset>, selected: Option<String>) -> Result<(), String> {
        let value = serde_json::to_value(Book {
            format: "commander.sync-profiles".into(),
            version: 1,
            profiles: next.clone(),
        })
        .map_err(|e| e.to_string())?;
        if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > 64 * 1024 {
            return Err("Saved sync profiles exceed 64 KiB; previous profiles kept".into());
        }
        self.entries = next;
        self.saved = value;
        self.selected = selected;
        self.error = None;
        Ok(())
    }
    pub fn rename_selected(&mut self) -> Result<(), String> {
        if !self.valid {
            return Err("Saved sync profiles are invalid".into());
        }
        let selected = self.selected.as_deref().ok_or("Select a sync profile")?;
        let index = self
            .entries
            .iter()
            .position(|p| p.name == selected)
            .ok_or("Selected sync profile no longer exists")?;
        let mut next = self.entries.clone();
        next[index].name = self.name.clone();
        next[index].validate()?;
        if next
            .iter()
            .enumerate()
            .any(|(i, p)| i != index && p.name == self.name)
        {
            return Err("That sync profile name already exists".into());
        }
        self.publish(next, Some(self.name.clone()))
    }
    pub fn remove_selected(&mut self) -> Result<(), String> {
        if !self.valid {
            return Err("Saved sync profiles are invalid".into());
        }
        let selected = self.selected.as_deref().ok_or("Select a sync profile")?;
        let index = self
            .entries
            .iter()
            .position(|p| p.name == selected)
            .ok_or("Selected sync profile no longer exists")?;
        let mut next = self.entries.clone();
        next.remove(index);
        self.publish(next, None)
    }
    pub fn clear(&mut self) {
        *self = Self::from_saved(serde_json::Value::Null);
    }
    pub fn editor(&mut self, ui: &mut egui::Ui, current: Preset, ready: bool) -> Option<Preset> {
        let mut load = None;
        let section = ui.collapsing("Saved sync profiles", |ui| {
            ui.small("Saved with the session. Loading compares again; it never copies files.");
            let name = ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .char_limit(80)
                    .hint_text("Profile name (new / rename)"),
            );
            #[cfg(any(test, feature = "visual-qa"))]
            mark(ui, "name", &name);
            #[cfg(not(any(test, feature = "visual-qa")))]
            let _ = name;
            egui::ComboBox::from_id_salt("sync-profile-picker")
                .selected_text(self.selected.as_deref().unwrap_or("Choose saved profile"))
                .show_ui(ui, |ui| {
                    for p in &self.entries {
                        ui.selectable_value(&mut self.selected, Some(p.name.clone()), &p.name);
                    }
                });
            if let Some(p) = self
                .selected
                .as_deref()
                .and_then(|n| self.entries.iter().find(|p| p.name == n))
            {
                for path in [&p.left, &p.right] {
                    ui.add(egui::Label::new(path.to_string_lossy()).truncate())
                        .on_hover_text(path.to_string_lossy());
                }
                ui.small(format!(
                    "{:?}; hidden {} / {}; content {}; dry run {}",
                    p.policy, p.left_hidden, p.right_hidden, p.compare_contents, p.dry_run
                ));
            }
            ui.horizontal_wrapped(|ui| {
                let save = ui.button("Save comparison as new");
                #[cfg(any(test, feature = "visual-qa"))]
                mark(ui, "save", &save);
                if save.clicked() {
                    let mut p = current.clone();
                    p.name = self.name.clone();
                    if let Err(e) = self.save(p, false) {
                        self.error = Some(e);
                    }
                }
                if ui
                    .add_enabled(
                        self.selected.is_some(),
                        egui::Button::new("Replace selected"),
                    )
                    .clicked()
                {
                    let mut p = current.clone();
                    p.name = self.selected.clone().expect("selection");
                    if let Err(e) = self.save(p, true) {
                        self.error = Some(e);
                    }
                }
                let load_button = ui.add_enabled(
                    ready && self.valid && self.selected.is_some(),
                    egui::Button::new("Load and compare"),
                );
                #[cfg(any(test, feature = "visual-qa"))]
                mark(ui, "load", &load_button);
                if load_button.clicked() {
                    load = self
                        .selected
                        .as_deref()
                        .and_then(|n| self.entries.iter().find(|p| p.name == n))
                        .cloned();
                }
                let rename = ui.add_enabled(
                    self.valid && self.selected.is_some(),
                    egui::Button::new("Rename selected to name"),
                );
                #[cfg(any(test, feature = "visual-qa"))]
                mark(ui, "rename", &rename);
                if rename.clicked()
                    && let Err(error) = self.rename_selected()
                {
                    self.error = Some(error);
                }
                let delete = ui.add_enabled(
                    self.valid && self.selected.is_some(),
                    egui::Button::new("Delete selected profile"),
                );
                #[cfg(any(test, feature = "visual-qa"))]
                mark(ui, "delete", &delete);
                if delete.clicked()
                    && let Err(error) = self.remove_selected()
                {
                    self.error = Some(error);
                }
                if ui
                    .add_enabled(
                        !self.saved.is_null(),
                        egui::Button::new("Clear saved profiles"),
                    )
                    .clicked()
                {
                    self.clear();
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
        #[cfg(any(test, feature = "visual-qa"))]
        {
            mark(ui, "header", &section.header_response);
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    egui::Id::new("sync-profile-section-closed"),
                    section.body_response.is_none(),
                )
            });
        }
        #[cfg(not(any(test, feature = "visual-qa")))]
        let _ = section;
        load
    }
}

#[cfg(any(test, feature = "visual-qa"))]
fn mark(ui: &egui::Ui, key: &'static str, response: &egui::Response) {
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new(("sync-profiles", key)), response.rect));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn preset(name: &str) -> Preset {
        Preset {
            name: name.into(),
            left: "/left".into(),
            right: "/right".into(),
            policy: crate::sync::SyncPolicy::TwoWay,
            left_hidden: false,
            right_hidden: true,
            compare_contents: true,
            dry_run: true,
        }
    }
    #[test]
    fn rename_and_delete_preserve_other_profiles_and_comparison_inputs() {
        let mut state = State::from_saved(serde_json::Value::Null);
        let work = preset("Work");
        state.save(work.clone(), false).unwrap();
        state.save(preset("Other"), false).unwrap();
        state.selected = Some("Work".into());
        state.name = "Renamed".into();
        state.rename_selected().unwrap();
        let mut expected = work;
        expected.name = "Renamed".into();
        assert_eq!(state.entries, vec![expected, preset("Other")]);
        assert_eq!(state.selected.as_deref(), Some("Renamed"));
        assert_eq!(State::from_saved(state.saved()).entries, state.entries);
        let before = state.saved();
        for invalid in ["Other", "", "bad\nname"] {
            state.name = invalid.into();
            assert!(state.rename_selected().is_err());
            assert_eq!(state.saved(), before);
        }
        state.remove_selected().unwrap();
        assert_eq!(
            State::from_saved(state.saved()).entries,
            vec![preset("Other")]
        );
        assert!(state.selected.is_none());
        let before = state.saved();
        assert!(state.remove_selected().is_err());
        assert_eq!(state.saved(), before);
        state.selected = Some("Other".into());
        state.remove_selected().unwrap();
        assert!(State::from_saved(state.saved()).entries.is_empty());
    }
    #[test]
    fn saved_inputs_round_trip_and_replacement_is_explicit() {
        let mut s = State::from_saved(serde_json::Value::Null);
        s.save(preset("Work"), false).unwrap();
        let before = s.saved();
        assert!(s.save(preset("Work"), false).is_err());
        assert_eq!(s.saved(), before);
        let mut changed = preset("Work");
        changed.policy = crate::sync::SyncPolicy::MirrorRightToLeft;
        s.save(changed.clone(), true).unwrap();
        assert_eq!(State::from_saved(s.saved()).entries, vec![changed]);
        for n in 1..16 {
            s.save(preset(&n.to_string()), false).unwrap();
        }
        let before = s.saved();
        assert!(s.save(preset("overflow"), false).is_err());
        assert_eq!(s.saved(), before);
    }
    #[test]
    fn invalid_or_oversized_data_does_not_replace_saved_profiles() {
        let raw = serde_json::json!({"version":99});
        let mut s = State::from_saved(raw.clone());
        assert!(s.error.is_some());
        assert!(s.save(preset("Work"), false).is_err());
        assert_eq!(s.saved(), raw);
        s.clear();
        s.save(preset("Work"), false).unwrap();
        let before = s.saved();
        let mut huge = preset("Huge");
        huge.left = PathBuf::from(format!("/{}", "x".repeat(64 * 1024)));
        assert!(s.save(huge, false).is_err());
        assert_eq!(s.saved(), before);
        let mut invalid = preset("bad\nname");
        assert!(s.save(invalid.clone(), false).is_err());
        invalid.name = "Relative".into();
        invalid.left = "relative".into();
        assert!(s.save(invalid, false).is_err());
        assert_eq!(s.saved(), before);
    }
}
