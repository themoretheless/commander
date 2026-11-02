//! Search projection retains full-store identities and order; no execution or I/O.
use crate::cmdtemplate::Templates;
use crate::panel::FileEntry;

#[derive(Default)]
pub struct Browser {
    pub query: String,
    pub show_all: bool,
    pub indices: Vec<usize>,
    pub applicable: usize,
    key: Option<Key>,
}
struct Key {
    store: Templates,
    names: Vec<(String, bool)>,
    query: String,
    show_all: bool,
}
impl Browser {
    /// Cache against the complete template content and immutable opening names,
    /// not just row count: edits, moves, imports and a changed selection invalidate it.
    pub fn refresh(&mut self, store: &Templates, selection: &[FileEntry]) -> bool {
        if self.key.as_ref().is_some_and(|key| {
            key.store == *store
                && key.query == self.query
                && key.show_all == self.show_all
                && key.names.len() == selection.len()
                && key
                    .names
                    .iter()
                    .zip(selection)
                    .all(|(a, b)| a.0 == b.name_lower && a.1 == b.is_dir)
        }) {
            return false;
        }
        let query = self.query.trim().to_lowercase();
        let too_long = self.query.chars().take(129).count() > 128;
        let words: Vec<_> = query.split_whitespace().collect();
        self.applicable = 0;
        self.indices.clear();
        for (index, template) in store.items.iter().enumerate() {
            let applies = template.matches(selection);
            self.applicable += usize::from(applies);
            if too_long || (!self.show_all && !applies) {
                continue;
            }
            // Keep field boundaries: a word cannot match across unrelated fields.
            let fields: Vec<_> = [&template.name, &template.raw]
                .into_iter()
                .chain(&template.exts)
                .map(|s| s.to_lowercase())
                .collect();
            if words
                .iter()
                .all(|word| fields.iter().any(|field| field.contains(word)))
            {
                self.indices.push(index);
            }
        }
        self.key = Some(Key {
            store: store.clone(),
            names: selection
                .iter()
                .map(|e| (e.name_lower.clone(), e.is_dir))
                .collect(),
            query: self.query.clone(),
            show_all: self.show_all,
        });
        true
    }
    pub fn show(&mut self, ui: &mut egui::Ui, store: &Templates, selection: &[FileEntry]) -> bool {
        let mut keyboard_active = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Find templates");
            let search = ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(180.0).hint_text("Name, command or extension").char_limit(129));
            let clear = ui.button("Clear");
            if clear.clicked() { self.query.clear(); }
            let all = ui.checkbox(&mut self.show_all, "All templates").on_hover_text("Include templates for other file types. Choosing a row only fills the command draft.");
            keyboard_active = search.has_focus() || search.lost_focus() || search.changed()
                || clear.clicked() || clear.has_focus() || all.changed() || all.has_focus();
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| {
                for (name, rect) in [("query", search.rect), ("clear", clear.rect), ("all", all.rect)] {
                    data.insert_temp(egui::Id::new(("template-browser", name)), rect);
                }
            });
        });
        self.refresh(store, selection);
        ui.small(format!(
            "{} shown / {} saved · {} applicable to opening selection",
            self.indices.len(),
            store.items.len(),
            self.applicable
        ));
        if self.query.chars().take(129).count() > 128 {
            ui.small("Use at most 128 search characters");
        } else if self.indices.is_empty() && !store.items.is_empty() {
            ui.small("No matching templates. Clear the search or enable All templates.");
        }
        keyboard_active
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmdtemplate::Template;
    fn store() -> Templates {
        Templates {
            items: vec![
                Template {
                    name: "Архивы".into(),
                    raw: "unzip {paths}".into(),
                    exts: vec!["zip".into()],
                    require_all: false,
                    working_dir: Default::default(),
                    execution: Default::default(),
                    kind: Default::default(),
                    limits: Default::default(),
                },
                Template {
                    name: "Text".into(),
                    raw: "wc -l {paths}".into(),
                    exts: vec!["txt".into()],
                    require_all: false,
                    working_dir: Default::default(),
                    execution: Default::default(),
                    kind: Default::default(),
                    limits: Default::default(),
                },
                Template {
                    name: "Archive stats".into(),
                    raw: "wc -c {paths}".into(),
                    exts: vec![],
                    require_all: false,
                    working_dir: Default::default(),
                    execution: Default::default(),
                    kind: Default::default(),
                    limits: Default::default(),
                },
            ],
        }
    }
    fn entry(name: &str) -> FileEntry {
        let tmp = crate::testutil::TempDir::new();
        let path = tmp.file(name, "data");
        FileEntry::from_meta(path.clone(), &std::fs::metadata(path).unwrap()).unwrap()
    }
    #[test]
    fn selected_folder_working_directory_affects_applicability_and_invalidates_cache() {
        let mut store = store();
        store.items[1].working_dir = crate::cmdtemplate::WorkingDirectory::SelectedFolder;
        let mut selected = vec![entry("notes.txt")];
        let mut browser = Browser::default();
        browser.refresh(&store, &selected);
        assert_eq!(browser.indices, [2]);
        selected[0].is_dir = true;
        assert!(browser.refresh(&store, &selected));
        assert_eq!(browser.indices, [1, 2]);
        selected.push(entry("another.txt"));
        browser.refresh(&store, &selected);
        assert_eq!(browser.indices, [2]);
        browser.show_all = true;
        browser.refresh(&store, &selected);
        assert_eq!(browser.indices, [0, 1, 2]);
        assert!(!store.items[1].allows_selection(&selected));
        store.items[1].working_dir = crate::cmdtemplate::WorkingDirectory::OtherPane;
        browser.show_all = false;
        assert!(browser.refresh(&store, &selected));
        assert_eq!(browser.indices, [1, 2]);
    }

    #[test]
    fn kind_policy_changes_invalidate_same_length_store_and_all_scope_never_bypasses_it() {
        let mut store = store();
        store.items[1].kind = crate::cmdtemplate::InputKind::Folders;
        let mut selection = vec![entry("notes.txt")];
        let mut browser = Browser::default();
        browser.refresh(&store, &selection);
        assert_eq!(browser.indices, [2]);
        store.items[1].kind = crate::cmdtemplate::InputKind::Files;
        assert!(browser.refresh(&store, &selection));
        assert_eq!(browser.indices, [1, 2]);
        selection[0].is_dir = true;
        assert!(browser.refresh(&store, &selection));
        assert_eq!(browser.indices, [2]);
        browser.show_all = true;
        browser.refresh(&store, &selection);
        assert_eq!(browser.indices, [0, 1, 2]);
        assert!(!store.items[1].allows_selection(&selection));
    }

    #[test]
    fn count_limit_changes_and_selection_size_invalidate_projection_without_bypassing_run_policy() {
        let mut store = store();
        store.items[1].limits = crate::cmdtemplate::SelectionLimits {
            min: 1,
            max: Some(1),
        };
        let mut selection = vec![entry("one.txt")];
        let mut browser = Browser::default();
        browser.refresh(&store, &selection);
        assert_eq!(browser.indices, [1, 2]);
        selection.push(entry("other.png"));
        assert!(browser.refresh(&store, &selection));
        assert_eq!(browser.indices, [2]);
        browser.show_all = true;
        browser.refresh(&store, &selection);
        assert_eq!(browser.indices, [0, 1, 2]);
        assert!(!store.items[1].allows_selection(&selection));
        store.items[1].limits.max = Some(2);
        browser.show_all = false;
        assert!(browser.refresh(&store, &selection));
        assert_eq!(browser.indices, [1, 2]);
        assert!(store.items[1].allows_selection(&selection));
    }

    #[test]
    fn strict_policy_and_same_name_directory_changes_invalidate_cached_applicability() {
        let mut store = store();
        store.items[1].require_all = true;
        let mut selected = vec![entry("one.txt")];
        let mut browser = Browser::default();
        browser.refresh(&store, &selected);
        assert_eq!(browser.indices, [1, 2]);
        selected[0].is_dir = true;
        assert!(browser.refresh(&store, &selected));
        assert_eq!(browser.indices, [2]);
        store.items[1].require_all = false;
        assert!(browser.refresh(&store, &selected));
        assert_eq!(browser.indices, [1, 2]);
        store.items[1].require_all = true;
        selected[0].is_dir = false;
        selected.push(entry("image.png"));
        browser.refresh(&store, &selected);
        assert_eq!(browser.indices, [2]);
        browser.show_all = true;
        browser.refresh(&store, &selected);
        assert_eq!(browser.indices, [0, 1, 2]);
        assert!(!store.items[1].allows_selection(&selected));
    }

    #[test]
    fn search_composes_fields_applicability_and_full_store_identity_without_reordering() {
        let store = store();
        let selection = [entry("FILE.TXT")];
        let mut browser = Browser::default();
        assert!(browser.refresh(&store, &selection));
        assert_eq!(browser.indices, [1, 2]);
        assert_eq!(browser.applicable, 2);
        browser.query = "TEXT WC txt".into();
        browser.refresh(&store, &selection);
        assert_eq!(browser.indices, [1]);
        browser.query = "АРХИВ".into();
        browser.refresh(&store, &selection);
        assert!(browser.indices.is_empty());
        browser.show_all = true;
        browser.refresh(&store, &selection);
        assert_eq!(browser.indices, [0]);
        browser.query.clear();
        browser.refresh(&store, &selection);
        assert_eq!(browser.indices, [0, 1, 2]);
        browser.query = "z".repeat(129);
        browser.refresh(&store, &selection);
        assert!(browser.indices.is_empty());
        assert_eq!(store.items[0].name, "Архивы");
    }
    #[test]
    fn cache_invalidates_same_length_metadata_moves_and_selection_and_clears_stale_rows() {
        let mut store = store();
        let selection = [entry("one.txt")];
        let mut browser = Browser::default();
        assert!(browser.refresh(&store, &selection));
        assert!(!browser.refresh(&store, &selection));
        browser.query = "wc".into();
        assert!(browser.refresh(&store, &selection));
        assert_eq!(browser.indices, [1, 2]);
        store.items.swap(0, 2);
        assert!(browser.refresh(&store, &selection));
        assert_eq!(browser.indices, [0, 1]);
        store.items[1].raw = "file {paths}".into();
        assert!(browser.refresh(&store, &selection));
        assert_eq!(browser.indices, [0]);
        assert!(browser.refresh(&store, &[entry("one.zip")]));
        assert_eq!(browser.applicable, 2);
        store.items.clear();
        assert!(browser.refresh(&store, &selection));
        assert!(browser.indices.is_empty());
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    #[test]
    fn real_query_input_and_clear_refresh_immediately_and_claim_keyboard_activity() {
        let ctx = egui::Context::default();
        let browser = std::cell::RefCell::new(Browser::default());
        let store = Templates {
            items: vec![crate::cmdtemplate::Template {
                name: "Текст".into(),
                raw: "wc {paths}".into(),
                exts: vec![],
                require_all: false,
                working_dir: Default::default(),
                execution: Default::default(),
                kind: Default::default(),
                limits: Default::default(),
            }],
        };
        let active = std::cell::Cell::new(false);
        let frame = |events| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    active.set(browser.borrow_mut().show(ui, &store, &[]));
                },
            ));
        };
        let click = |name| {
            let rect = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("template-browser", name))))
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
        assert_eq!(browser.borrow().indices, [0]);
        click("query");
        frame(vec![
            egui::Event::Text("missing".into()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            },
        ]);
        assert!(browser.borrow().indices.is_empty());
        assert!(
            active.get(),
            "query text/Enter must not fall through to Run"
        );
        frame(vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Default::default(),
        }]);
        click("clear");
        assert!(active.get());
        assert!(browser.borrow().query.is_empty());
        assert_eq!(browser.borrow().indices, [0]);
        click("all");
        assert!(browser.borrow().show_all);
        assert!(active.get());
    }
}
