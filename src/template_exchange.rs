//! Bounded data-only interchange for saved command templates.
use crate::cmdtemplate::{Template, Templates};
use serde::{Deserialize, Serialize};

pub const MAX_BYTES: usize = 64 * 1024;
const MAX_ITEMS: usize = 128;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    format: String,
    version: u32,
    templates: Vec<Record>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    name: String,
    raw: String,
    exts: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    require_all: bool,
    #[serde(
        default,
        skip_serializing_if = "crate::cmdtemplate::WorkingDirectory::is_default"
    )]
    working_dir: crate::cmdtemplate::WorkingDirectory,
    #[serde(
        default,
        skip_serializing_if = "crate::cmdtemplate::ExecutionMode::is_default"
    )]
    execution: crate::cmdtemplate::ExecutionMode,
    #[serde(default, skip_serializing_if = "crate::cmdtemplate::InputKind::is_any")]
    kind: crate::cmdtemplate::InputKind,
    #[serde(
        default,
        skip_serializing_if = "crate::cmdtemplate::SelectionLimits::is_default"
    )]
    limits: crate::cmdtemplate::SelectionLimits,
}
impl From<&Template> for Record {
    fn from(t: &Template) -> Self {
        Self {
            name: t.name.clone(),
            raw: t.raw.clone(),
            exts: t.exts.clone(),
            require_all: t.require_all,
            working_dir: t.working_dir,
            execution: t.execution,
            kind: t.kind,
            limits: t.limits,
        }
    }
}
fn validate(t: &Template) -> Result<(), String> {
    if !t.limits.valid() {
        return Err("Minimum selection cannot exceed maximum".into());
    }
    if t.name.trim().is_empty()
        || t.name.chars().count() > 256
        || t.name.chars().any(char::is_control)
    {
        return Err("Template names must be 1–256 characters without control characters".into());
    }
    if t.raw.trim().is_empty()
        || t.raw.len() > 8192
        || t.raw
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err("Commands must be nonempty, at most 8192 bytes, without control characters except newline/tab".into());
    }
    if t.exts.len() > 32
        || t.exts.iter().any(|ext| {
            ext.chars().any(|c| c.is_whitespace() || c == ',')
                || !matches!(crate::command_recipes::parse_extensions(ext), Ok(parsed) if parsed.len() == 1)
        })
    {
        return Err("Use at most 32 individual extension filters, such as png or tar.gz".into());
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    New,
    Existing,
    Duplicate,
}
#[derive(Clone, Debug)]
pub struct Row {
    pub template: Template,
    pub status: Status,
    pub selected: bool,
}
#[derive(Clone, Debug)]
pub struct Preview {
    source: String,
    pub rows: Vec<Row>,
}
pub fn parse(source: &str, current: &Templates) -> Result<Preview, String> {
    if source.len() > MAX_BYTES {
        return Err("Template exchange is limited to 64 KiB".into());
    }
    let bundle: Bundle =
        serde_json::from_str(source).map_err(|e| format!("Invalid template JSON: {e}"))?;
    if bundle.format != "commander.command-templates" || !matches!(bundle.version, 1..=6) {
        return Err("Expected commander.command-templates version 1, 2, 3, 4, 5 or 6".into());
    }
    if bundle.templates.len() > MAX_ITEMS {
        return Err("Use at most 128 templates per bundle".into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for record in bundle.templates {
        let t = Template {
            name: record.name,
            raw: record.raw,
            exts: record.exts,
            require_all: record.require_all,
            working_dir: record.working_dir,
            execution: record.execution,
            kind: record.kind,
            limits: record.limits,
        };
        if bundle.version == 1 && t.require_all {
            return Err("Whole-selection policy requires bundle version 2".into());
        }
        if bundle.version < 3 && !t.limits.is_default() {
            return Err("Selection count limits require bundle version 3".into());
        }
        if bundle.version < 4 && !t.kind.is_any() {
            return Err("Input kind constraints require bundle version 4".into());
        }
        if bundle.version < 5 && !t.execution.is_default() {
            return Err("Per-item execution requires bundle version 5".into());
        }
        if bundle.version < 6 && !t.working_dir.is_default() {
            return Err("Working-directory policy requires bundle version 6".into());
        }
        validate(&t)?;
        let status = if current.items.iter().any(|v| v.raw == t.raw) {
            Status::Existing
        } else if !seen.insert(t.raw.clone()) {
            Status::Duplicate
        } else {
            Status::New
        };
        let selected = status == Status::New;
        rows.push(Row {
            template: t,
            status,
            selected,
        });
    }
    Ok(Preview {
        source: source.to_string(),
        rows,
    })
}
pub fn merge(
    source: &str,
    preview: &Preview,
    current: &Templates,
) -> Result<(Templates, usize), String> {
    if source != preview.source {
        return Err("Parse the changed JSON before importing".into());
    }
    let checked = parse(source, current)?;
    if checked.rows.len() != preview.rows.len()
        || checked
            .rows
            .iter()
            .zip(&preview.rows)
            .any(|(a, b)| a.template != b.template)
    {
        return Err("The reviewed templates changed; parse again".into());
    }
    let mut next = current.clone();
    let mut added = 0;
    for (row, reviewed) in checked.rows.iter().zip(&preview.rows) {
        if row.status == Status::New && reviewed.status == Status::New && reviewed.selected {
            next.items.push(row.template.clone());
            added += 1;
        }
    }
    Ok((next, added))
}
pub fn export(current: &Templates) -> Result<String, String> {
    if current.items.len() > MAX_ITEMS {
        return Err("Export supports at most 128 templates".into());
    }
    for t in &current.items {
        validate(t)?;
    }
    let text = serde_json::to_string_pretty(&Bundle {
        format: "commander.command-templates".into(),
        version: 6,
        templates: current.items.iter().map(Record::from).collect(),
    })
    .map_err(|e| e.to_string())?;
    if text.len() > MAX_BYTES {
        return Err("Template exchange is limited to 64 KiB".into());
    }
    Ok(text)
}
/// Commit before live replacement. The callback is a persistence seam, never an executor.
pub fn commit(
    source: &str,
    preview: &Preview,
    current: &mut Templates,
    save: impl FnOnce(&Templates) -> bool,
) -> Result<usize, String> {
    let (next, added) = merge(source, preview, current)?;
    if added == 0 {
        return Ok(0);
    }
    if !save(&next) {
        return Err("Could not save imported templates; current list was kept".into());
    }
    *current = next;
    Ok(added)
}

#[derive(Default)]
pub struct Editor {
    pub source: String,
    pub preview: Option<Preview>,
    pub message: Option<String>,
}
impl Editor {
    pub fn rebase(&mut self, current: &Templates) {
        let unchecked: std::collections::HashSet<_> = self
            .preview
            .as_ref()
            .into_iter()
            .flat_map(|p| &p.rows)
            .filter(|r| !r.selected)
            .map(|r| r.template.raw.clone())
            .collect();
        self.preview = parse(&self.source, current).ok().map(|mut preview| {
            for row in &mut preview.rows {
                if unchecked.contains(&row.template.raw) {
                    row.selected = false;
                }
            }
            preview
        });
    }

    /// Returns an import request only on explicit button activation.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        current: &Templates,
        copy: &mut Option<String>,
    ) -> (bool, bool) {
        let mut import = false;
        let response = ui.collapsing("Import / export templates", |ui| {
            ui.small("Commander JSON · review command text before adding · importing does not run commands");
            egui::ScrollArea::vertical().id_salt("template-exchange-input").max_height(100.0).show(ui, |ui| {
                if ui.add(egui::TextEdit::multiline(&mut self.source).code_editor().desired_width(f32::INFINITY).desired_rows(4).char_limit(MAX_BYTES + 1)).changed() {
                    self.preview = None; self.message = None;
                }
            });
            ui.horizontal(|ui| {
                let review = ui.button("Review JSON");
                probe(ui, "review", review.rect);
                if review.clicked() {
                    match parse(&self.source, current) {
                        Ok(preview) => { self.preview = Some(preview); self.message = None; }
                        Err(error) => { self.preview = None; self.message = Some(error); }
                    }
                }
                let export_button = ui.button("Copy all templates as JSON");
                probe(ui, "export", export_button.rect);
                if export_button.clicked() {
                    match export(current) { Ok(text) => *copy = Some(text), Err(error) => self.message = Some(error) }
                }
            });
            if let Some(preview) = &mut self.preview {
                egui::ScrollArea::vertical().id_salt("template-exchange-preview").max_height(150.0).show(ui, |ui| {
                    for row in &mut preview.rows {
                        ui.add_enabled(row.status == Status::New, egui::Checkbox::new(&mut row.selected, format!("{} · {:?}", row.template.name, row.status)));
                        ui.label(egui::RichText::new(&row.template.raw).monospace());
                        ui.small(row.template.execution.label());
                        ui.small(format!("Working folder: {}",row.template.working_dir.label()));
                        if !row.template.kind.is_any() { ui.small(format!("Apply to: {} (every opening item)", row.template.kind.label())); }
                        if !row.template.limits.is_default() { ui.small(format!("Selection: {}", row.template.limits.description())); }
                        ui.small(if row.template.require_all { "Every selected file must match" } else { "Legacy: any selected entry may match" });
                        ui.small(format!("Extensions: {}", if row.template.exts.is_empty() { "All".into() } else { row.template.exts.join(", ") }));
                        ui.separator();
                    }
                });
                let add = ui.add_enabled(preview.rows.iter().any(|r| r.status == Status::New && r.selected), egui::Button::new("Add selected templates"));
                probe(ui, "add", add.rect);
                import = add.clicked();
            }
            if let Some(message) = &self.message { ui.label(message); }
        });
        probe(ui, "toggle", response.header_response.rect);
        (response.body_response.is_some(), import)
    }
}

fn probe(ui: &egui::Ui, name: &str, rect: egui::Rect) {
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new(("template-exchange", name)), rect));
    #[cfg(not(any(test, feature = "visual-qa")))]
    let _ = (ui, name, rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(name: &str, raw: &str) -> Template {
        Template {
            name: name.into(),
            raw: raw.into(),
            exts: vec![".TXT".into(), "tar.gz".into()],
            require_all: false,
            working_dir: Default::default(),
            execution: Default::default(),
            kind: Default::default(),
            limits: Default::default(),
        }
    }
    #[test]
    fn working_directory_survives_v6_and_old_envelopes_cannot_retarget_it() {
        let mut template = t("Other", "pwd");
        template.working_dir = crate::cmdtemplate::WorkingDirectory::OtherPane;
        let store = Templates {
            items: vec![template.clone()],
        };
        let source = export(&store).unwrap();
        let preview = parse(&source, &Templates::default()).unwrap();
        assert_eq!(
            merge(&source, &preview, &Templates::default()).unwrap().0,
            store
        );
        for version in 1..=5 {
            assert!(
                parse(
                    &source.replace("\"version\": 6", &format!("\"version\": {version}")),
                    &Templates::default()
                )
                .is_err()
            );
        }
        assert!(
            parse(
                &source.replace("other_pane", "unknown_directory"),
                &Templates::default()
            )
            .is_err()
        );
        template.working_dir = Default::default();
        template.execution = crate::cmdtemplate::ExecutionMode::PerItemContinue;
        let old = export(&Templates {
            items: vec![template.clone()],
        })
        .unwrap()
        .replace("\"version\": 6", "\"version\": 5");
        assert_eq!(
            parse(&old, &Templates::default()).unwrap().rows[0].template,
            template
        );
    }

    #[test]
    fn execution_mode_survives_v5_exchange_and_older_versions_cannot_change_it() {
        let mut template = t("Sequential", "tool {paths}");
        template.execution = crate::cmdtemplate::ExecutionMode::PerItemContinue;
        let store = Templates {
            items: vec![template.clone()],
        };
        let source = export(&store).unwrap();
        let preview = parse(&source, &Templates::default()).unwrap();
        assert_eq!(
            merge(&source, &preview, &Templates::default()).unwrap().0,
            store
        );
        for version in [1, 2, 3, 4] {
            assert!(
                parse(
                    &source.replace("\"version\": 6", &format!("\"version\": {version}")),
                    &Templates::default()
                )
                .is_err()
            );
        }
        assert!(
            parse(
                &source.replace("per_item_continue", "unknown_mode"),
                &Templates::default()
            )
            .is_err()
        );
        template.execution = Default::default();
        template.kind = crate::cmdtemplate::InputKind::Files;
        let legacy = export(&Templates {
            items: vec![template.clone()],
        })
        .unwrap()
        .replace("\"version\": 6", "\"version\": 4");
        assert_eq!(
            parse(&legacy, &Templates::default()).unwrap().rows[0].template,
            template
        );
    }

    #[test]
    fn input_kind_survives_v4_exchange_and_older_envelopes_cannot_drop_constraints() {
        let mut template = t("Folders", "folder-tool {paths}");
        template.exts.clear();
        template.kind = crate::cmdtemplate::InputKind::Folders;
        let store = Templates {
            items: vec![template.clone()],
        };
        let source = export(&store).unwrap();
        let preview = parse(&source, &Templates::default()).unwrap();
        assert_eq!(
            merge(&source, &preview, &Templates::default()).unwrap().0,
            store
        );
        for version in [1, 2, 3] {
            assert!(
                parse(
                    &source.replace("\"version\": 6", &format!("\"version\": {version}")),
                    &Templates::default()
                )
                .is_err()
            );
        }
        assert!(
            parse(
                &source.replace("\"folders\"", "\"devices\""),
                &Templates::default()
            )
            .is_err()
        );
        template.kind = Default::default();
        template.limits = crate::cmdtemplate::SelectionLimits {
            min: 1,
            max: Some(1),
        };
        let v3 = export(&Templates {
            items: vec![template.clone()],
        })
        .unwrap()
        .replace("\"version\": 6", "\"version\": 3");
        assert_eq!(
            parse(&v3, &Templates::default()).unwrap().rows[0].template,
            template
        );
    }

    #[test]
    fn count_limits_survive_current_exchange_and_old_versions_cannot_silently_drop_them() {
        let mut limited = t("One", "one {paths}");
        limited.limits = crate::cmdtemplate::SelectionLimits {
            min: 1,
            max: Some(1),
        };
        let store = Templates {
            items: vec![limited.clone()],
        };
        let source = export(&store).unwrap();
        let preview = parse(&source, &Templates::default()).unwrap();
        assert_eq!(
            merge(&source, &preview, &Templates::default()).unwrap().0,
            store
        );
        for version in [1, 2] {
            assert!(
                parse(
                    &source.replace("\"version\": 6", &format!("\"version\": {version}")),
                    &Templates::default()
                )
                .is_err()
            );
        }
        limited.limits = Default::default();
        limited.require_all = true;
        let source_v2 = export(&Templates {
            items: vec![limited.clone()],
        })
        .unwrap()
        .replace("\"version\": 6", "\"version\": 2");
        assert!(
            parse(&source_v2, &Templates::default()).unwrap().rows[0]
                .template
                .require_all
        );
        limited.limits = crate::cmdtemplate::SelectionLimits {
            min: 3,
            max: Some(1),
        };
        assert!(
            export(&Templates {
                items: vec![limited]
            })
            .is_err()
        );
        assert!(
            parse(
                &source.replace("\"min\": 1", "\"min\": 2"),
                &Templates::default()
            )
            .is_err()
        );
    }

    #[test]
    fn whole_selection_policy_survives_current_exchange_and_cannot_be_downgraded_to_v1() {
        let mut strict = t("All text", "wc {paths}");
        strict.require_all = true;
        let current = Templates {
            items: vec![strict.clone()],
        };
        let source = export(&current).unwrap();
        let preview = parse(&source, &Templates::default()).unwrap();
        let (restored, added) = merge(&source, &preview, &Templates::default()).unwrap();
        assert_eq!(added, 1);
        assert_eq!(restored, current);
        assert!(
            parse(
                &source.replace("\"version\": 6", "\"version\": 1"),
                &Templates::default()
            )
            .is_err()
        );
        strict.require_all = false;
        let legacy = export(&Templates {
            items: vec![strict],
        })
        .unwrap()
        .replace("\"version\": 6", "\"version\": 1");
        assert!(
            !parse(&legacy, &Templates::default()).unwrap().rows[0]
                .template
                .require_all
        );
        // Existing same-command metadata/policy still wins during append-only merge.
        let legacy_store = Templates {
            items: vec![
                parse(&legacy, &Templates::default()).unwrap().rows[0]
                    .template
                    .clone(),
            ],
        };
        assert_eq!(
            merge(&source, &preview, &legacy_store).unwrap().0,
            legacy_store
        );
    }

    #[test]
    fn template_bundle_roundtrip_preserves_exact_data_without_trust_fields() {
        let current = Templates {
            items: vec![t("Текст", "printf '%s\\n' {paths}\n# comment\t ")],
        };
        let text = export(&current).unwrap();
        let preview = parse(&text, &Templates::default()).unwrap();
        let (next, added) = merge(&text, &preview, &Templates::default()).unwrap();
        assert_eq!(added, 1);
        assert_eq!(next, current);
        assert!(!text.contains("trusted"));
        assert!(parse(&text.replace("\"version\": 6", "\"version\": 7"), &current).is_err());
        assert!(
            parse(
                &text.replacen("\"name\":", "\"enabled\": true, \"name\":", 1),
                &current
            )
            .is_err()
        );
        assert!(
            parse(
                &text.replacen("\"version\": 6", "\"version\": 6, \"trusted\": true", 1),
                &current
            )
            .is_err()
        );
    }
    #[test]
    fn reviewed_merge_rechecks_identity_duplicates_current_store_and_subset() {
        let original = t("Old label", "old {paths}");
        let current = Templates {
            items: vec![original.clone()],
        };
        let source = export(&Templates {
            items: vec![
                t("Replace", "old {paths}"),
                t("New", "new {names}"),
                t("Duplicate", "new {names}"),
                t("Unchecked", "other {dir}"),
            ],
        })
        .unwrap();
        let mut preview = parse(&source, &current).unwrap();
        assert_eq!(
            preview.rows.iter().map(|r| &r.status).collect::<Vec<_>>(),
            [
                &Status::Existing,
                &Status::New,
                &Status::Duplicate,
                &Status::New
            ]
        );
        preview.rows[3].selected = false;
        let (next, added) = merge(&source, &preview, &current).unwrap();
        assert_eq!(added, 1);
        assert_eq!(next.items[0], original);
        assert_eq!(next.items[1].name, "New");
        assert_eq!(merge(&source, &preview, &next).unwrap().1, 0);
        assert!(merge(&(source.clone() + " "), &preview, &current).is_err());
        preview.rows[1].template.raw = "tampered".into();
        assert!(merge(&source, &preview, &current).is_err());
    }
    #[test]
    fn commit_failure_keeps_live_model_zero_additions_do_not_save_and_rebase_keeps_unchecked() {
        let source = export(&Templates {
            items: vec![t("A", "a {paths}"), t("B", "b {names}")],
        })
        .unwrap();
        let mut editor = Editor {
            source: source.clone(),
            preview: Some(parse(&source, &Templates::default()).unwrap()),
            message: None,
        };
        editor.preview.as_mut().unwrap().rows[1].selected = false;
        let mut current = Templates::default();
        assert!(
            commit(
                &source,
                editor.preview.as_ref().unwrap(),
                &mut current,
                |_| false
            )
            .is_err()
        );
        assert!(current.items.is_empty());
        assert_eq!(
            commit(
                &source,
                editor.preview.as_ref().unwrap(),
                &mut current,
                |candidate| candidate.items.len() == 1
            )
            .unwrap(),
            1
        );
        editor.rebase(&current);
        assert_eq!(
            editor.preview.as_ref().unwrap().rows[0].status,
            Status::Existing
        );
        assert!(!editor.preview.as_ref().unwrap().rows[1].selected);
        assert_eq!(
            commit(
                &source,
                editor.preview.as_ref().unwrap(),
                &mut current,
                |_| panic!("unchanged writes are forbidden")
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn bounded_validation_refuses_oversize_controls_empty_and_ambiguous_extensions() {
        for invalid in [
            t("", "ok"),
            t("bad\nlabel", "ok"),
            t("Name", "bad\0command"),
            t("Name", " "),
            t("Name", &"a".repeat(8193)),
        ] {
            assert!(
                export(&Templates {
                    items: vec![invalid]
                })
                .is_err()
            );
        }
        for ext in ["*", "png,jpg", "png,png", " png", "../png", "", "png jpg"] {
            let mut invalid = t("Name", "command");
            invalid.exts = vec![ext.into()];
            assert!(
                export(&Templates {
                    items: vec![invalid]
                })
                .is_err(),
                "{ext}"
            );
        }
        assert!(parse(&" ".repeat(MAX_BYTES + 1), &Templates::default()).is_err());
        assert!(
            export(&Templates {
                items: vec![t("A", "echo"); MAX_ITEMS + 1]
            })
            .is_err()
        );
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    #[test]
    fn review_does_not_import_and_only_explicit_buttons_emit_add_or_copy() {
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
        ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.0);
        let current = Templates {
            items: vec![Template {
                name: "Текст".into(),
                raw: "printf '%s' {paths}".into(),
                exts: vec![],
                require_all: false,
                working_dir: Default::default(),
                execution: Default::default(),
                kind: Default::default(),
                limits: Default::default(),
            }],
        };
        let source = export(&current).unwrap();
        let editor = std::cell::RefCell::new(Editor {
            source: source.clone(),
            ..Default::default()
        });
        let actions = std::cell::RefCell::new(Vec::new());
        let frame = |events| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let mut copy = None;
                    let (_, import) =
                        editor
                            .borrow_mut()
                            .show(ui, &Templates::default(), &mut copy);
                    if import {
                        actions.borrow_mut().push("import".to_string());
                    }
                    if let Some(text) = copy {
                        actions.borrow_mut().push(text);
                    }
                },
            ));
        };
        let click = |name| {
            let rect = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("template-exchange", name))))
                .unwrap();
            for phase in 0..3 {
                let mut events = vec![egui::Event::PointerMoved(rect.center())];
                if phase > 0 {
                    events.push(egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: phase == 1,
                        modifiers: Default::default(),
                    });
                }
                frame(events);
            }
        };
        frame(vec![]);
        click("toggle");
        frame(vec![]);
        frame(vec![]);
        assert!(actions.borrow().is_empty());
        click("review");
        frame(vec![]);
        assert_eq!(editor.borrow().preview.as_ref().unwrap().rows.len(), 1);
        assert!(actions.borrow().is_empty());
        click("add");
        assert_eq!(*actions.borrow(), ["import"]);
        click("export");
        let exported = actions.borrow();
        assert_eq!(
            parse(&exported[1], &Templates::default())
                .unwrap()
                .rows
                .len(),
            0,
            "copy exports the live store, not the pasted draft"
        );
    }
}
