//! Reviewed, versioned semantic palettes. Draft editing never changes applied colors.
use crate::theme::ThemeColors;
use egui::Color32;
use serde::{Deserialize, Serialize};
const MAX_BYTES: usize = 16 * 1024;
fn color(value: &str) -> Option<Color32> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    Some(Color32::from_rgb(
        u8::from_str_radix(&hex[..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..], 16).ok()?,
    ))
}
fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}
macro_rules! colors {
    ($($field:ident),+)=>{
        #[derive(Serialize,Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Colors {$( $field:String ),+}
        impl Colors {
            fn from_colors(c:ThemeColors)->Self{Self{$($field:hex(c.$field)),+}}
            fn compile(&self)->Option<ThemeColors>{Some(ThemeColors{$($field:color(&self.$field)?),+})}
            fn rows(&mut self)->[(&'static str,&mut String);15]{[$((stringify!($field),&mut self.$field)),+]}
        }
    }
}
colors!(
    bg_deep,
    bg_panel,
    bg_card,
    bg_hover,
    bg_selected,
    bg_toolbar,
    text_primary,
    text_secondary,
    text_muted,
    accent,
    accent_red,
    accent_purple,
    accent_warning,
    folder,
    border
);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: String,
    version: u32,
    colors: Colors,
}
fn parse(text: &str) -> Result<Document, String> {
    if text.len() > MAX_BYTES {
        return Err("Theme JSON is limited to 16 KiB".into());
    }
    let doc: Document = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if doc.format != "commander.theme" || doc.version != 1 {
        return Err("Expected commander.theme version 1".into());
    }
    doc.colors.compile().ok_or("Every color must be #RRGGBB")?;
    Ok(doc)
}
#[derive(Default)]
pub struct State {
    saved: serde_json::Value,
    pub draft: String,
    pub error: Option<String>,
    compiled: Option<ThemeColors>,
}
pub enum Effect {
    Applied,
    Reset,
    Copy(String),
}
impl State {
    pub fn from_saved(saved: serde_json::Value) -> Self {
        let mut state = Self {
            saved,
            ..Default::default()
        };
        if !state.saved.is_null() {
            state.draft = serde_json::to_string_pretty(&state.saved).unwrap_or_default();
            match parse(&state.draft) {
                Ok(doc) => state.compiled = doc.colors.compile(),
                Err(e) => state.error = Some(e),
            }
        }
        state
    }
    pub fn saved(&self) -> serde_json::Value {
        self.saved.clone()
    }
    pub fn colors(&self) -> Option<ThemeColors> {
        self.compiled
    }
    pub fn deactivate(&mut self) {
        self.saved = serde_json::Value::Null;
        self.compiled = None;
    }
    pub fn seed(&mut self, colors: ThemeColors) {
        self.draft = serde_json::to_string_pretty(&Document {
            format: "commander.theme".into(),
            version: 1,
            colors: Colors::from_colors(colors),
        })
        .expect("theme serialization");
        self.error = None;
    }
    pub fn apply(&mut self) -> Result<(), String> {
        let doc = parse(&self.draft)?;
        self.compiled = doc.colors.compile();
        self.saved = serde_json::to_value(doc).expect("theme serialization");
        self.error = None;
        Ok(())
    }
    pub fn editor(&mut self, ui: &mut egui::Ui, current: ThemeColors) -> Option<Effect> {
        let mut effect = None;
        ui.vertical(|ui| {
            ui.small(
                "Edit the draft; Apply activates and saves it. Increase Contrast takes priority.",
            );
            if ui.button("Use current colors as draft").clicked() {
                self.seed(current);
            }
            if let Ok(mut doc) = parse(&self.draft) {
                let mut changed = false;
                egui::ScrollArea::vertical()
                    .id_salt("custom-theme-colors")
                    .max_height(180.)
                    .show(ui, |ui| {
                        egui::Grid::new("custom-theme-grid").show(ui, |ui| {
                            for (label, value) in doc.colors.rows() {
                                ui.label(label);
                                let c = color(value).expect("validated color");
                                let mut rgb = [c.r(), c.g(), c.b()];
                                if ui.color_edit_button_srgb(&mut rgb).changed() {
                                    *value = hex(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                                    changed = true;
                                }
                                ui.end_row();
                            }
                        });
                    });
                if changed {
                    self.draft = serde_json::to_string_pretty(&doc).expect("theme serialization");
                }
            }
            egui::ScrollArea::vertical()
                .id_salt("custom-theme-json")
                .max_height(150.)
                .show(ui, |ui| {
                    let edit = ui.add(
                        egui::TextEdit::multiline(&mut self.draft)
                            .desired_rows(6)
                            .desired_width(320.)
                            .char_limit(MAX_BYTES)
                            .hint_text("Paste Commander theme JSON"),
                    );
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            egui::Id::new("custom-theme-json-input"),
                            edit.rect.intersect(ui.clip_rect()),
                        )
                    });
                    #[cfg(not(feature = "visual-qa"))]
                    let _ = edit;
                });
            ui.horizontal(|ui| {
                let apply = ui.button("Apply custom theme");
                #[cfg(feature = "visual-qa")]
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("apply-custom-theme"), apply.rect));
                if apply.clicked() {
                    match self.apply() {
                        Ok(()) => effect = Some(Effect::Applied),
                        Err(e) => self.error = Some(e),
                    }
                }
                let reset = ui.add_enabled(
                    !self.saved.is_null(),
                    egui::Button::new("Reset custom theme"),
                );
                #[cfg(feature = "visual-qa")]
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("reset-custom-theme"), reset.rect));
                if reset.clicked() {
                    self.deactivate();
                    effect = Some(Effect::Reset);
                }
                let copy = ui.add_enabled(
                    self.compiled.is_some(),
                    egui::Button::new("Copy applied JSON"),
                );
                #[cfg(feature = "visual-qa")]
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("copy-custom-theme"), copy.rect));
                if copy.clicked() {
                    effect = Some(Effect::Copy(
                        serde_json::to_string_pretty(&self.saved).expect("theme serialization"),
                    ));
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
        effect
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_and_invalid_apply_preserve_the_applied_palette() {
        let mut state = State::default();
        state.seed(ThemeColors::dark());
        assert!(state.colors().is_none());
        state.apply().unwrap();
        let saved = state.saved();
        let panel = state.colors().unwrap().bg_panel;
        state.seed(ThemeColors::light());
        assert_eq!(state.colors().unwrap().bg_panel, panel);
        state.draft = "broken".into();
        assert!(state.apply().is_err());
        assert_eq!(state.saved(), saved);
        assert_eq!(state.colors().unwrap().bg_panel, panel);
        let restored = State::from_saved(saved);
        assert_eq!(restored.colors().unwrap().bg_panel, panel);
    }
    #[test]
    fn strict_schema_and_colors_reject_whole_invalid_documents() {
        let mut state = State::default();
        state.seed(ThemeColors::light());
        let original: serde_json::Value = serde_json::from_str(&state.draft).unwrap();
        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("format", serde_json::json!("other")),
            ("extra", serde_json::json!(true)),
        ] {
            let mut bad = original.clone();
            bad[field] = value;
            assert!(parse(&bad.to_string()).is_err());
        }
        for bad_color in ["#fff", "#abcdef00", "red", "#é1234", "#zz0000"] {
            let mut bad = original.clone();
            bad["colors"]["accent"] = bad_color.into();
            assert!(parse(&bad.to_string()).is_err());
        }
        let duplicate =
            state
                .draft
                .replacen("\"accent\":", "\"accent\":\"#112233\",\"accent\":", 1);
        assert!(parse(&duplicate).is_err());
        assert!(parse(&" ".repeat(MAX_BYTES + 1)).is_err());
        let bad = serde_json::json!({"invalid":"preserved"});
        let state = State::from_saved(bad.clone());
        assert_eq!(state.saved(), bad);
        assert!(state.colors().is_none());
        assert!(state.error.is_some());
    }
    #[test]
    fn every_semantic_color_round_trips_and_reset_preserves_draft() {
        let mut state = State::default();
        state.seed(ThemeColors::light());
        let mut doc = parse(&state.draft).unwrap();
        for (_, value) in doc.colors.rows() {
            *value = "#123456".into();
        }
        state.draft = serde_json::to_string(&doc).unwrap();
        state.apply().unwrap();
        let restored = State::from_saved(state.saved());
        let compiled = Colors::from_colors(restored.colors().unwrap());
        assert_eq!(
            serde_json::to_value(compiled).unwrap(),
            serde_json::to_value(doc.colors).unwrap()
        );
        let draft = state.draft.clone();
        state.deactivate();
        assert!(state.saved().is_null());
        assert!(state.colors().is_none());
        assert_eq!(state.draft, draft);
    }
}
