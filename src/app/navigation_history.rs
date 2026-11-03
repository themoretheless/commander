//! Frozen per-tab Back/Forward menus; opening them performs no filesystem work.
use super::*;

#[derive(Clone)]
struct Binding {
    owner: PathBuf,
    generation: u64,
    tab_revision: u64,
    trail: crate::jumplist::Snapshot,
}
impl Binding {
    fn matches(&self, panel: &PanelState, revision: u64) -> bool {
        self.owner == panel.current_path
            && self.generation == panel.entries_gen()
            && self.tab_revision == revision
            && self.trail == panel.history.snapshot()
    }
}

pub(super) fn history_menu(
    response: &egui::Response,
    panel: &mut PanelState,
    tab_revision: u64,
    side: &str,
    back: bool,
) {
    let key = response.id.with("history-menu-binding");
    let capture = |panel: &PanelState| Binding {
        owner: panel.current_path.clone(),
        generation: panel.entries_gen(),
        tab_revision,
        trail: panel.history.snapshot(),
    };
    if response.secondary_clicked() {
        response
            .ctx
            .data_mut(|d| d.insert_temp(key, capture(panel)));
    }
    let popup = egui::Popup::context_menu(response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let binding = ui.ctx().data_mut(|d| {
                if let Some(binding) = d.get_temp::<Binding>(key) {
                    binding
                } else {
                    let binding = capture(panel);
                    d.insert_temp(key, binding.clone());
                    binding
                }
            });
            ui.label(if back {
                "Earlier folders"
            } else {
                "Later folders"
            });
            let valid = binding.matches(panel, tab_revision);
            if !valid {
                ui.small("History changed; close and reopen this menu.");
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for index in binding.trail.choices(back) {
                        let path = &binding.trail.paths[index];
                        let label =
                            format!("{}  {}", index + 1, path.to_string_lossy().escape_debug());
                        let row = ui
                            .add_enabled(valid, egui::Button::new(&label).truncate())
                            .on_hover_text(&label);
                        #[cfg(any(test, feature = "visual-qa"))]
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(
                                egui::Id::new(("history-menu-row", side, back, index)),
                                row.rect,
                            )
                        });
                        #[cfg(not(any(test, feature = "visual-qa")))]
                        let _ = side;
                        if row.clicked() && binding.matches(panel, tab_revision) {
                            panel.go_to_history(&binding.trail, index);
                            ui.close();
                        }
                    }
                });
        });
    if popup.is_none() {
        response.ctx.data_mut(|d| d.remove::<Binding>(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn history_menu_binding_rejects_changed_tab_listing_owner_and_trail() {
        let root = TempDir::new();
        let child = root.dir("child");
        let mut panel = PanelState::new(root.path().into());
        panel.refresh();
        panel.navigate_to(child.clone());
        let binding = Binding {
            owner: child.clone(),
            generation: panel.entries_gen(),
            tab_revision: 7,
            trail: panel.history.snapshot(),
        };
        assert!(binding.matches(&panel, 7));
        assert!(!binding.matches(&panel, 8));
        panel.refresh();
        assert!(!binding.matches(&panel, 7));
        let binding = Binding {
            generation: panel.entries_gen(),
            ..binding
        };
        assert!(binding.matches(&panel, 7));
        panel.history.push(root.path());
        assert!(!binding.matches(&panel, 7));
        panel.navigate_to(root.path().into());
        assert!(!binding.matches(&panel, 7));
    }

    #[test]
    fn history_menu_pointer_selects_the_original_slot_and_refuses_a_stale_popup() {
        for stale in [false, true] {
            let root = TempDir::new();
            let a = root.dir("a");
            let b = root.dir("b");
            let changed = root.dir("changed");
            let mut panel = PanelState::new(root.path().into());
            panel.refresh();
            panel.navigate_to(a);
            panel.navigate_to(b.clone());
            let ctx = egui::Context::default();
            let mut time = 0.0;
            let mut frame = |panel: &mut PanelState, events| {
                time += 0.1;
                crate::testutil::discard_egui_output(ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 480.0),
                        )),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let response = ui.button("Back");
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("history-test-button"), response.rect)
                        });
                        history_menu(&response, panel, 7, "left", true);
                    },
                ));
            };
            let pointer = |pos, button, pressed| {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            for _ in 0..3 {
                frame(&mut panel, vec![]);
            }
            let pos = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("history-test-button")))
                .unwrap()
                .center();
            frame(
                &mut panel,
                pointer(pos, egui::PointerButton::Secondary, true),
            );
            frame(
                &mut panel,
                pointer(pos, egui::PointerButton::Secondary, false),
            );
            frame(&mut panel, vec![]);
            let target = ctx
                .data(|d| {
                    d.get_temp::<egui::Rect>(egui::Id::new((
                        "history-menu-row",
                        "left",
                        true,
                        0_usize,
                    )))
                })
                .unwrap()
                .center();
            if stale {
                panel.navigate_to(changed.clone());
            }
            frame(
                &mut panel,
                pointer(target, egui::PointerButton::Primary, true),
            );
            frame(
                &mut panel,
                pointer(target, egui::PointerButton::Primary, false),
            );
            frame(&mut panel, vec![]);
            assert_eq!(
                panel.current_path,
                if stale { changed } else { root.path().into() }
            );
            if !stale {
                assert!(panel.can_go_forward());
                assert_eq!(panel.history.snapshot().paths.last(), Some(&b));
            }
        }
    }
}
