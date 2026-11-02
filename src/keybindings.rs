//! User-defined workspace shortcuts, independently adapting Kiorg's sequences.
use crate::command::{CATALOG, Command};
use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub keys: String,
    pub command: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stroke {
    key: Key,
    shift: bool,
    alt: bool,
    ctrl: bool,
    command: bool,
}
impl Stroke {
    fn event(key: Key, m: Modifiers) -> Self {
        Self {
            key,
            shift: m.shift,
            alt: m.alt,
            ctrl: m.ctrl && cfg!(target_os = "macos"),
            command: m.command || (!cfg!(target_os = "macos") && m.ctrl),
        }
    }
    fn label(&self) -> String {
        let mut label = String::new();
        for (yes, name) in [
            (self.command, "Cmd+"),
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
        ] {
            if yes {
                label.push_str(name);
            }
        }
        label.push_str(self.key.name());
        label
    }
    fn parse(text: &str) -> Result<Self, String> {
        let mut parts: Vec<_> = text.split('+').collect();
        let name = parts.pop().ok_or("Missing key")?;
        let key = Key::from_name(name)
            .or_else(|| {
                Key::ALL
                    .iter()
                    .copied()
                    .find(|k| k.name().eq_ignore_ascii_case(name))
            })
            .ok_or_else(|| format!("Unknown key: {name}"))?;
        if key == Key::Escape {
            return Err("Escape is reserved for cancelling a sequence".into());
        }
        let mut result = Self {
            key,
            shift: false,
            alt: false,
            ctrl: false,
            command: false,
        };
        for part in parts {
            match part.to_ascii_lowercase().as_str() {
                "shift" if !result.shift => result.shift = true,
                "alt" if !result.alt => result.alt = true,
                "cmd" | "command" if !result.command => result.command = true,
                "ctrl" | "control" if cfg!(target_os = "macos") && !result.ctrl => {
                    result.ctrl = true
                }
                "ctrl" | "control" if !cfg!(target_os = "macos") && !result.command => {
                    result.command = true
                }
                _ => return Err(format!("Unknown or repeated modifier: {part}")),
            }
        }
        Ok(result)
    }
}
const NAVIGATION: &[(&str, Command)] = &[
    ("Move cursor up", Command::CursorUp),
    ("Move cursor down", Command::CursorDown),
    ("Move to first item", Command::CursorHome),
    ("Move to last item", Command::CursorEnd),
    ("Move one page up", Command::CursorPageUp),
    ("Move one page down", Command::CursorPageDown),
    ("Extend selection up", Command::ExtendSelectUp),
    ("Extend selection down", Command::ExtendSelectDown),
    ("Extend selection to first item", Command::ExtendSelectHome),
    ("Extend selection to last item", Command::ExtendSelectEnd),
    ("Extend selection one page up", Command::ExtendSelectPageUp),
    (
        "Extend selection one page down",
        Command::ExtendSelectPageDown,
    ),
    ("Switch active pane", Command::SwitchPanel),
    ("Open highlighted entry", Command::Activate),
    ("Go to parent folder", Command::GoUp),
    ("Toggle selection and move down", Command::ToggleSelect),
];
fn choices() -> impl Iterator<Item = (&'static str, Command)> {
    NAVIGATION
        .iter()
        .copied()
        .chain(CATALOG.iter().map(|e| (e.label, e.command)))
}
fn repeatable(command: Command) -> bool {
    matches!(
        command,
        Command::CursorUp
            | Command::CursorDown
            | Command::CursorHome
            | Command::CursorEnd
            | Command::CursorPageUp
            | Command::CursorPageDown
            | Command::ExtendSelectUp
            | Command::ExtendSelectDown
            | Command::ExtendSelectHome
            | Command::ExtendSelectEnd
            | Command::ExtendSelectPageUp
            | Command::ExtendSelectPageDown
    )
}
#[derive(Clone)]
struct Compiled {
    strokes: Vec<Stroke>,
    command: Command,
}
fn compile(rows: &[Binding]) -> Result<Vec<Compiled>, String> {
    if rows.len() > 64 {
        return Err("At most 64 custom shortcuts".into());
    }
    let mut out: Vec<Compiled> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if row.keys.len() > 128 || row.command.len() > 160 {
            return Err(format!("Shortcut {} exceeds its text limit", index + 1));
        }
        let strokes = row
            .keys
            .split_whitespace()
            .map(Stroke::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if strokes.is_empty() || strokes.len() > 4 {
            return Err("Use one to four space-separated keys".into());
        }
        let command = choices()
            .find(|(label, _)| *label == row.command)
            .ok_or_else(|| format!("Unknown command: {}", row.command))?
            .1;
        for old in &out {
            if old.strokes.starts_with(&strokes) || strokes.starts_with(&old.strokes) {
                return Err(format!(
                    "Shortcut {} duplicates or prefixes another shortcut",
                    index + 1
                ));
            }
        }
        out.push(Compiled { strokes, command });
    }
    Ok(out)
}
const EXCHANGE_LIMIT: usize = 64 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
    format: String,
    version: u32,
    bindings: Vec<Binding>,
}
fn decode_exchange(text: &str) -> Result<Vec<Binding>, String> {
    if text.len() > EXCHANGE_LIMIT {
        return Err("Shortcut exchange is limited to 64 KiB".into());
    }
    let data: Exchange =
        serde_json::from_str(text).map_err(|e| format!("Invalid shortcut JSON: {e}"))?;
    if data.format != "commander.shortcuts" || data.version != 1 {
        return Err("Unsupported shortcut exchange format/version".into());
    }
    compile(&data.bindings)?;
    Ok(data.bindings)
}
#[derive(Default)]
pub struct State {
    raw: serde_json::Value,
    compiled: Vec<Compiled>,
    pub draft: Vec<Binding>,
    pub error: Option<String>,
    pub exchange_text: String,
    exchange_feedback: Option<String>,
    pending: Vec<Stroke>,
    last: f64,
    owner: Option<(bool, usize, u64, std::path::PathBuf)>,
}
impl State {
    pub fn from_saved(raw: serde_json::Value) -> Self {
        if raw.is_null() {
            return Self::default();
        }
        let mut result = Self {
            raw: raw.clone(),
            ..Default::default()
        };
        match serde_json::from_value::<Vec<Binding>>(raw) {
            Ok(rows) => {
                if rows.len() > 64 {
                    result.error=Some("Saved shortcut list exceeds 64 entries; edit the bounded draft to replace it".into());
                    result.draft = rows.into_iter().take(64).collect();
                    return result;
                }
                result.draft = rows;
                match compile(&result.draft) {
                    Ok(c) => result.compiled = c,
                    Err(e) => {
                        result.error = Some(e);
                        for row in &mut result.draft {
                            row.keys = row.keys.chars().take(128).collect();
                            row.command = row.command.chars().take(160).collect();
                        }
                    }
                }
            }
            Err(e) => result.error = Some(format!("Saved shortcuts could not be read: {e}")),
        };
        result
    }
    pub fn is_empty(&self) -> bool {
        self.compiled.is_empty()
    }
    pub fn saved(&self) -> serde_json::Value {
        self.raw.clone()
    }
    pub fn cancel(&mut self) {
        self.pending.clear();
        self.owner = None;
    }
    pub fn apply(&mut self) -> Result<(), String> {
        let compiled = compile(&self.draft)?;
        self.raw = serde_json::to_value(&self.draft).map_err(|e| e.to_string())?;
        self.compiled = compiled;
        self.cancel();
        self.error = None;
        Ok(())
    }
    fn step(&mut self, stroke: Stroke, now: f64) -> (bool, Option<Command>) {
        if !now.is_finite() || now < self.last || now - self.last > 1.0 {
            self.pending.clear();
        }
        self.last = now;
        if stroke.key == Key::Escape {
            let claimed = !self.pending.is_empty();
            self.pending.clear();
            return (claimed, None);
        }
        self.pending.push(stroke.clone());
        let find = |pending: &[Stroke]| {
            self.compiled
                .iter()
                .find(|c| c.strokes.starts_with(pending))
        };
        if find(&self.pending).is_none() {
            self.pending.clear();
            self.pending.push(stroke);
        }
        if let Some(found) = find(&self.pending) {
            if found.strokes.len() == self.pending.len() {
                let command = found.command;
                self.pending.clear();
                (true, Some(command))
            } else {
                (true, None)
            }
        } else {
            self.pending.clear();
            (false, None)
        }
    }
    /// Only called by the app while the unoccluded file workspace owns input.
    pub fn input(
        &mut self,
        ctx: &egui::Context,
        owner: (bool, usize, u64, std::path::PathBuf),
    ) -> (bool, Option<Command>) {
        if self.compiled.is_empty() {
            self.cancel();
            return (false, None);
        }
        if self.owner.as_ref() != Some(&owner) {
            self.cancel();
            self.owner = Some(owner);
        }
        let (events, now) = ctx.input(|i| (i.events.clone(), i.time));
        let mut claimed = false;
        let mut command = None;
        for event in events {
            if let egui::Event::Key {
                key,
                pressed: true,
                repeat,
                modifiers,
                ..
            } = event
            {
                let stroke = Stroke::event(key, modifiers);
                if repeat {
                    if !self.pending.is_empty() {
                        claimed = true;
                        continue;
                    }
                    if let Some(binding) = self
                        .compiled
                        .iter()
                        .find(|c| c.strokes.first() == Some(&stroke))
                    {
                        claimed = true;
                        if binding.strokes.len() == 1 && repeatable(binding.command) {
                            command = Some(binding.command);
                            break;
                        }
                    }
                    continue;
                }
                let (used, action) = self.step(stroke, now);
                claimed |= used;
                if action.is_some() {
                    command = action;
                    break;
                }
            }
        }
        if claimed {
            ctx.input_mut(|i| {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Text(_) | egui::Event::Key { .. }))
            });
        }
        (claimed, command)
    }
    /// Effective workspace keys; an intercepted default leader cannot run its old action.
    pub fn hints(&self, command: Command, default: &str) -> Vec<String> {
        let mut keys: Vec<String> = self
            .compiled
            .iter()
            .filter(|c| c.command == command)
            .map(|c| {
                c.strokes
                    .iter()
                    .map(Stroke::label)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        let shadowed = Stroke::parse(default).ok().is_some_and(|stroke| {
            self.compiled
                .iter()
                .any(|c| c.strokes.first() == Some(&stroke))
        });
        if !default.is_empty() && !shadowed && !keys.iter().any(|s| s == default) {
            keys.push(default.into());
        }
        keys
    }
    pub fn reference(&self) -> Vec<(&'static str, Command, Vec<String>)> {
        let defaults = [
            "Up",
            "Down",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Shift+Up",
            "Shift+Down",
            "Shift+Home",
            "Shift+End",
            "Shift+PageUp",
            "Shift+PageDown",
            "Tab",
            "Enter",
            "Backspace",
            "Space",
        ];
        let mut rows = Vec::new();
        for ((label, command), default) in NAVIGATION
            .iter()
            .copied()
            .zip(defaults)
            .chain(CATALOG.iter().map(|e| ((e.label, e.command), e.shortcut)))
        {
            if rows.iter().any(|(_, cmd, _)| *cmd == command) {
                continue;
            }
            let hints = self.hints(command, default);
            if !default.is_empty() || !hints.is_empty() {
                rows.push((label, command, hints));
            }
        }
        rows
    }
    fn pending_hint(&self, now: f64) -> Option<String> {
        if self.pending.is_empty() || !now.is_finite() || now < self.last || now - self.last > 1. {
            return None;
        }
        Some(
            self.pending
                .iter()
                .map(Stroke::label)
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
    pub fn show_pending(&self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let Some(keys) = self.pending_hint(now) else {
            return;
        };
        egui::Area::new(egui::Id::new("custom-shortcut-pending"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0., -45.))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(format!("{keys} …  · Escape cancels"));
                });
            });
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
            (1.001 - (now - self.last)).max(0.001),
        ));
    }
    pub fn export_applied(&self) -> Result<String, String> {
        let rows: Vec<Binding> = if self.raw.is_null() {
            Vec::new()
        } else {
            serde_json::from_value(self.raw.clone())
                .map_err(|e| format!("Applied shortcuts cannot be exported: {e}"))?
        };
        compile(&rows)?;
        let text = serde_json::to_string_pretty(&Exchange {
            format: "commander.shortcuts".into(),
            version: 1,
            bindings: rows,
        })
        .map_err(|e| e.to_string())?;
        if text.len() > EXCHANGE_LIMIT {
            return Err("Shortcut exchange is limited to 64 KiB".into());
        }
        Ok(text)
    }
    pub fn import_draft(&mut self) -> Result<(), String> {
        let rows = decode_exchange(&self.exchange_text)?;
        self.draft = rows;
        self.error = None;
        self.exchange_feedback = Some(format!(
            "Loaded {} shortcuts into the draft. Apply shortcuts to activate them.",
            self.draft.len()
        ));
        Ok(())
    }
    pub fn editor(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let mut copy = None;
        ui.separator();
        ui.strong("Custom workspace shortcuts");
        ui.small("One to four keys, separated by spaces: Alt+G H. Overrides built-in keys only while the file list owns input. Prefixes expire after one second; Escape cancels.");
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("custom-shortcuts")
            .max_height(160.)
            .show(ui, |ui| {
                for (i, row) in self.draft.iter_mut().enumerate() {
                    ui.push_id(i, |ui| {
                        ui.horizontal(|ui| {
                            let key = ui.add(
                                egui::TextEdit::singleline(&mut row.keys)
                                    .desired_width(100.)
                                    .hint_text("Alt+G H")
                                    .char_limit(128),
                            );
                            #[cfg(any(test, feature = "visual-qa"))]
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new(("custom-shortcut-keys", i)), key.rect)
                            });
                            #[cfg(not(any(test, feature = "visual-qa")))]
                            let _ = key;
                            egui::ComboBox::from_id_salt("command")
                                .selected_text(if row.command.is_empty() {
                                    "Choose command"
                                } else {
                                    &row.command
                                })
                                .width(240.)
                                .height(220.)
                                .show_ui(ui, |ui| {
                                    for (label, _) in choices() {
                                        ui.selectable_value(
                                            &mut row.command,
                                            label.to_owned(),
                                            label,
                                        );
                                    }
                                });
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
            });
        if let Some(i) = remove {
            self.draft.remove(i);
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.draft.len() < 64, egui::Button::new("Add shortcut"))
                .clicked()
            {
                self.draft.push(Binding::default());
            }
            if ui.button("Clear draft").clicked() {
                self.draft.clear();
            }
            let apply = ui.button("Apply shortcuts");
            #[cfg(feature="visual-qa")]{let state=serde_json::json!({"hovered":apply.hovered(),"clicked":apply.clicked(),"down":apply.is_pointer_button_down_on(),"id":format!("{:?}",apply.id)});ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("apply-shortcuts-response"),state));}
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("custom-shortcut-clip"), ui.clip_rect()));
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("apply-custom-shortcuts"), apply.rect));
            if apply.clicked()
                && let Err(e) = self.apply()
            {
                self.error = Some(e);
            }
        });
        let exchange=egui::CollapsingHeader::new("Import / export shortcuts").show(ui,|ui|{
            let export=ui.button("Copy applied set");
            #[cfg(any(test,feature="visual-qa"))]ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("export-custom-shortcuts"),export.rect));
            if export.clicked(){match self.export_applied(){Ok(text)=>copy=Some(text),Err(e)=>self.exchange_feedback=Some(e)}}
            ui.small("Paste a Commander shortcut set below. Loading replaces only the editable draft; Apply activates it.");
            let input=ui.add(egui::TextEdit::multiline(&mut self.exchange_text).desired_rows(4).desired_width(f32::INFINITY).char_limit(EXCHANGE_LIMIT).code_editor());
            if input.changed(){self.exchange_feedback=None;}
            let import=ui.button("Load into draft");
            #[cfg(any(test,feature="visual-qa"))]ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new("import-custom-shortcuts"),import.rect);d.insert_temp(egui::Id::new("shortcut-exchange-clip"),ui.clip_rect());});
            if import.clicked()&&let Err(error)=self.import_draft(){self.exchange_feedback=Some(error);}
            if let Some(feedback)=&self.exchange_feedback{ui.label(feedback);}
        });
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("shortcut-exchange-header"),
                exchange.header_response.rect,
            )
        });
        #[cfg(not(any(test, feature = "visual-qa")))]
        let _ = exchange;
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.small(format!(
            "{} applied shortcuts · changes save with the session",
            self.compiled.len()
        ));
        copy
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn row(keys: &str) -> Binding {
        Binding {
            keys: keys.into(),
            command: "Reverse sort order".into(),
        }
    }
    fn press(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }
    #[test]
    fn exchange_exports_applied_snapshot_and_round_trips_without_draft_changes() {
        let mut s =
            State::from_saved(serde_json::json!([{"keys":"F5","command":"Reverse sort order"}]));
        let snapshot = s.saved();
        s.draft = vec![row("Escape")];
        let json = s.export_applied().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["format"], "commander.shortcuts");
        assert_eq!(value["version"], 1);
        assert_eq!(value["bindings"], snapshot);
        assert!(json.len() < EXCHANGE_LIMIT);
        assert_eq!(decode_exchange(&json).unwrap(), vec![row("F5")]);
        assert_eq!(s.saved(), snapshot);
        assert_eq!(s.draft[0].keys, "Escape");
        assert!(
            decode_exchange(&State::default().export_applied().unwrap())
                .unwrap()
                .is_empty()
        );
        assert!(
            State::from_saved(serde_json::json!({"future":"keys"}))
                .export_applied()
                .is_err()
        );
    }
    #[test]
    fn valid_import_replaces_draft_only_until_explicit_apply() {
        let mut s =
            State::from_saved(serde_json::json!([{"keys":"F5","command":"Reverse sort order"}]));
        let old = s.saved();
        s.exchange_text=serde_json::json!({"format":"commander.shortcuts","version":1,"bindings":[{"keys":"N","command":"Move cursor down"}]}).to_string();
        s.import_draft().unwrap();
        assert_eq!(s.saved(), old);
        assert_eq!(s.draft[0].keys, "N");
        assert_eq!(s.hints(Command::ReverseSort, ""), vec!["F5"]);
        assert!(s.hints(Command::CursorDown, "").is_empty());
        s.apply().unwrap();
        assert_eq!(s.hints(Command::CursorDown, ""), vec!["N"]);
        assert!(s.hints(Command::ReverseSort, "").is_empty());
    }
    #[test]
    fn invalid_exchange_preserves_draft_and_active_preferences() {
        let mut s =
            State::from_saved(serde_json::json!([{"keys":"F5","command":"Reverse sort order"}]));
        let old = s.saved();
        let draft = s.draft.clone();
        let pack = |bindings| {
            serde_json::json!({"format":"commander.shortcuts","version":1,"bindings":bindings})
                .to_string()
        };
        let mut bad=vec!["{}".into(),r#"{"format":"commander.shortcuts","version":1,"version":1,"bindings":[]}"#.into(),serde_json::json!({"format":"commander.shortcuts","version":2,"bindings":[]}).to_string(),serde_json::json!({"format":"commander.shortcuts","version":1,"bindings":[],"unknown":true}).to_string(),pack(serde_json::json!([{"keys":"N","command":"missing"}])),pack(serde_json::json!([{"keys":"N","command":"Move cursor down","unknown":0}])),pack(serde_json::json!([{"keys":"N","command":"Move cursor down"},{"keys":"N H","command":"Move cursor up"}]))];
        bad.push(" ".repeat(EXCHANGE_LIMIT + 1));
        bad.push(pack(serde_json::to_value(vec![row("F5"); 65]).unwrap()));
        for text in bad {
            s.exchange_text = text;
            assert!(s.import_draft().is_err());
            assert_eq!(s.saved(), old);
            assert_eq!(s.draft, draft);
        }
    }
    #[test]
    fn effective_hints_use_applied_bindings_and_remove_shadowed_defaults() {
        let mut state = State::from_saved(
            serde_json::json!([{"keys":"F5","command":"Reverse sort order"},{"keys":"Alt+G H","command":"Toggle hidden files"},{"keys":"N","command":"Move cursor down"}]),
        );
        assert!(state.hints(Command::RequestCopy, "F5").is_empty());
        assert_eq!(state.hints(Command::ReverseSort, ""), vec!["F5"]);
        assert_eq!(
            state.hints(Command::ToggleHidden, "Cmd+H"),
            vec!["Alt+G H", "Cmd+H"]
        );
        let rows = state.reference();
        assert!(
            rows.iter()
                .any(|(label, _, keys)| *label == "Move cursor down" && keys == &["N", "Down"])
        );
        assert!(
            rows.iter()
                .any(|(label, _, keys)| *label == "Copy to other panel" && keys.is_empty())
        );
        state.draft[0].command = "Copy to other panel".into();
        assert!(state.hints(Command::RequestCopy, "F5").is_empty());
        state.apply().unwrap();
        assert_eq!(state.hints(Command::RequestCopy, "F5"), vec!["F5"]);
        let prefix =
            State::from_saved(serde_json::json!([{"keys":"F5 H","command":"Reverse sort order"}]));
        assert!(prefix.hints(Command::RequestCopy, "F5").is_empty());
        for entry in CATALOG.iter().filter(|e| !e.shortcut.is_empty()) {
            assert!(Stroke::parse(entry.shortcut).is_ok(), "{}", entry.shortcut);
        }
        assert_eq!(
            State::from_saved(serde_json::json!({"future":"keys"}))
                .hints(Command::RequestCopy, "F5"),
            vec!["F5"]
        );
    }
    #[test]
    fn palette_search_uses_active_keys_without_losing_labels_keywords_or_usage() {
        let state = State::from_saved(
            serde_json::json!([{"keys":"F5","command":"Reverse sort order"},{"keys":"Alt+G H","command":"Toggle hidden files"}]),
        );
        let usage = crate::command::UsageStats::default();
        let search = |q| {
            crate::command::rank_with_shortcuts(q, &usage, 0, |command, default| {
                state.hints(command, default)
            })
        };
        let found = search("F5");
        assert_eq!(found[0].command, Command::ReverseSort);
        assert!(!found.iter().any(|m| m.command == Command::RequestCopy));
        assert_eq!(search("alt g h")[0].command, Command::ToggleHidden);
        assert_eq!(
            search("Copy to other panel")[0].command,
            Command::RequestCopy
        );
        assert!(
            search("clipboard")
                .iter()
                .any(|m| m.command == Command::CopyPath)
        );
        assert_eq!(
            crate::command::rank("F5", &usage, 0)[0].command,
            Command::RequestCopy
        );
    }
    #[test]
    fn navigation_choices_are_unique_and_survive_restoration() {
        let mut labels = std::collections::HashSet::new();
        for (label, _) in choices() {
            assert!(labels.insert(label), "{label}");
        }
        for (label, command) in NAVIGATION {
            let raw = serde_json::json!([{"keys":"Alt+N","command":label}]);
            let mut s = State::from_saved(raw.clone());
            assert!(s.error.is_none(), "{label}");
            assert_eq!(s.compiled[0].command, *command);
            s.apply().unwrap();
            assert_eq!(s.saved(), raw);
        }
    }
    #[test]
    fn held_navigation_repeats_but_activation_and_sequences_do_not() {
        let ctx = egui::Context::default();
        let mut s = State::from_saved(
            serde_json::json!([{"keys":"N","command":"Move cursor down"},{"keys":"O","command":"Open highlighted entry"},{"keys":"Alt+G H","command":"Move cursor up"}]),
        );
        assert!(s.error.is_none());
        let send = |s: &mut State, key, mods, repeat, time| {
            let mut out = (false, None);
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat,
                        modifiers: mods,
                    }],
                    ..Default::default()
                },
                |ui| {
                    out = s.input(ui.ctx(), (true, 0, 0, "/fixture".into()));
                },
            ));
            out
        };
        assert_eq!(
            send(&mut s, Key::N, Modifiers::NONE, false, 1.),
            (true, Some(Command::CursorDown))
        );
        assert_eq!(
            send(&mut s, Key::N, Modifiers::NONE, true, 1.1),
            (true, Some(Command::CursorDown))
        );
        assert_eq!(
            send(&mut s, Key::O, Modifiers::NONE, false, 1.2),
            (true, Some(Command::Activate))
        );
        assert_eq!(
            send(&mut s, Key::O, Modifiers::NONE, true, 1.3),
            (true, None)
        );
        assert_eq!(
            send(&mut s, Key::G, Modifiers::ALT, false, 1.4),
            (true, None)
        );
        assert_eq!(
            send(&mut s, Key::G, Modifiers::ALT, true, 1.5),
            (true, None)
        );
        assert_eq!(
            send(&mut s, Key::H, Modifiers::NONE, false, 1.6),
            (true, Some(Command::CursorUp))
        );
        assert_eq!(
            send(&mut s, Key::H, Modifiers::NONE, true, 1.7),
            (false, None)
        );
    }
    #[test]
    fn raw_input_claims_text_and_resets_on_owner_changes() {
        let ctx = egui::Context::default();
        let mut s = State {
            draft: vec![row("Alt+G H")],
            ..Default::default()
        };
        s.apply().unwrap();
        let owner = (true, 0, 1, std::path::PathBuf::from("/fixture"));
        let send = |s: &mut State, time, events, owner: (bool, usize, u64, std::path::PathBuf)| {
            let events: Vec<egui::Event> = [Key::G, Key::H]
                .into_iter()
                .map(|key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                })
                .chain(events)
                .collect();
            let mut result = (false, None);
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    result = s.input(ui.ctx(), owner.clone());
                    if result.0 {
                        assert!(ui.input(|i| i.events.iter().all(|e| !matches!(
                            e,
                            egui::Event::Text(_) | egui::Event::Key { .. }
                        ))));
                    }
                },
            ));
            result
        };
        assert_eq!(
            send(
                &mut s,
                1.,
                vec![press(Key::G, Modifiers::ALT)],
                owner.clone()
            ),
            (true, None)
        );
        let mut other = owner.clone();
        other.1 = 1;
        assert_eq!(
            send(
                &mut s,
                1.1,
                vec![press(Key::H, Modifiers::NONE)],
                other.clone()
            ),
            (false, None)
        );
        assert_eq!(
            send(
                &mut s,
                2.,
                vec![
                    press(Key::G, Modifiers::ALT),
                    press(Key::H, Modifiers::NONE),
                    egui::Event::Text("h".into())
                ],
                other.clone()
            ),
            (true, Some(Command::ReverseSort))
        );
        send(
            &mut s,
            3.,
            vec![press(Key::G, Modifiers::ALT)],
            other.clone(),
        );
        s.cancel();
        assert_eq!(
            send(&mut s, 3.1, vec![press(Key::H, Modifiers::NONE)], other),
            (false, None)
        );
    }
    #[test]
    fn apply_button_commits_only_a_valid_draft() {
        let ctx = egui::Context::default();
        let mut s = State {
            draft: vec![row("Alt+G H")],
            ..Default::default()
        };
        let frame = |s: &mut State, events| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700., 500.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let _ = s.editor(ui);
                },
            ))
        };
        frame(&mut s, vec![]);
        let rect = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("apply-custom-shortcuts")))
            .unwrap();
        for down in [true, false] {
            frame(
                &mut s,
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: down,
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(s.compiled.len(), 1);
        let applied = s.saved();
        s.draft.push(row("Alt+G"));
        frame(&mut s, vec![]);
        let rect = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("apply-custom-shortcuts")))
            .unwrap();
        for down in [true, false] {
            frame(
                &mut s,
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: down,
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(s.saved(), applied);
        assert!(s.error.is_some());
    }
    #[test]
    fn validates_commands_aliases_prefixes_and_limits() {
        for rows in [
            vec![row("")],
            vec![row("G G G G G")],
            vec![row("Alt+Alt+G")],
            vec![row("Escape")],
            vec![row("G"), row("G H")],
            vec![row("G H"), row("G")],
            vec![row("G H"), row("g h")],
            vec![row("G"); 65],
            vec![Binding {
                keys: "G".into(),
                command: "missing".into(),
            }],
        ] {
            assert!(compile(&rows).is_err(), "{rows:?}");
        }
        assert!(compile(&[row("Alt+G H"), row("Alt+G J")]).is_ok());
    }
    #[test]
    fn sequence_timeout_mismatch_modifiers_and_escape() {
        let mut s = State {
            draft: vec![row("Alt+G H")],
            ..Default::default()
        };
        s.apply().unwrap();
        let g = Stroke::parse("Alt+G").unwrap();
        let h = Stroke::parse("H").unwrap();
        assert_eq!(s.step(g.clone(), 1.), (true, None));
        assert_eq!(s.pending_hint(1.1).as_deref(), Some("Alt+G"));
        assert!(s.pending_hint(2.1).is_none());
        assert_eq!(s.step(h.clone(), 1.5), (true, Some(Command::ReverseSort)));
        assert_eq!(s.step(g.clone(), 2.), (true, None));
        assert_eq!(s.step(h.clone(), 3.1), (false, None));
        s.step(g.clone(), 4.);
        assert_eq!(
            s.step(Stroke::parse("Shift+H").unwrap(), 4.1),
            (false, None)
        );
        s.step(g.clone(), 5.);
        let esc = Stroke {
            key: Key::Escape,
            ..h.clone()
        };
        assert_eq!(s.step(esc, 5.1), (true, None));
        assert_eq!(s.step(h.clone(), 5.2), (false, None));
        s.step(g, 6.);
        assert_eq!(s.step(h, 5.9), (false, None));
    }
    #[test]
    fn failed_apply_preserves_applied_and_unreadable_saved_data() {
        let mut s = State {
            draft: vec![row("Alt+G H")],
            ..Default::default()
        };
        s.apply().unwrap();
        let saved = s.saved();
        s.draft.push(row("Alt+G"));
        assert!(s.apply().is_err());
        assert_eq!(s.saved(), saved);
        let restored = State::from_saved(saved);
        assert_eq!(restored.compiled.len(), 1);
        let invalid = serde_json::json!({"future":"format"});
        let s = State::from_saved(invalid.clone());
        assert_eq!(s.saved(), invalid);
        assert!(s.error.is_some());
        assert!(s.compiled.is_empty());
    }
}
