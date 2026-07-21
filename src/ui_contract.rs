//! Headless contract tests for cross-cutting UI input and accessibility rules.

use super::keyboard_command_allowed;
use crate::accessibility::{
    EscapeRoute, KeyboardRoute, ModalSurface, PendingFocusEffect, TextInputMode, TextInputState,
    UiContract, UiContractState, consume_escape_route, file_row_semantics,
    ime_composition_transition, next_ime_composition, resolve_ui_contract, text_input_state,
    top_open_modal,
};
use crate::command::{Command, KeyCode, KeyPress, map_keys};

#[derive(Default)]
struct ContractHarness {
    state: UiContractState,
}

impl ContractHarness {
    fn modal(mut self, surface: ModalSurface) -> Self {
        self.state.modal = Some(surface);
        self
    }

    fn text_input(mut self, state: TextInputState) -> Self {
        self.state.text_input = state;
        self
    }

    fn active_preview(mut self) -> Self {
        self.state.active_preview_open = true;
        self
    }

    fn focus_mode(mut self) -> Self {
        self.state.focus_mode = true;
        self
    }

    fn resolve(&self) -> UiContract {
        resolve_ui_contract(self.state)
    }

    fn dispatched(&self, presses: &[KeyPress]) -> Vec<Command> {
        let route = self.resolve().keyboard;
        map_keys(presses)
            .into_iter()
            .filter(|command| keyboard_command_allowed(route, *command))
            .collect()
    }
}

fn press(code: KeyCode) -> KeyPress {
    KeyPress {
        code,
        command: false,
        shift: false,
    }
}

fn cmd_press(code: KeyCode) -> KeyPress {
    KeyPress {
        code,
        command: true,
        shift: false,
    }
}

#[test]
fn modal_registry_has_deterministic_product_priority() {
    let open = [
        ModalSurface::Transfer,
        ModalSurface::Rename,
        ModalSurface::Palette,
    ];
    assert_eq!(
        top_open_modal(|surface| open.contains(&surface)),
        Some(ModalSurface::Palette)
    );
    assert_eq!(
        top_open_modal(|surface| {
            matches!(surface, ModalSurface::Transfer | ModalSurface::Rename)
        }),
        Some(ModalSurface::Rename)
    );
    assert_eq!(top_open_modal(|_| false), None);
}

#[test]
fn modal_focus_is_an_explicit_pending_render_effect() {
    let contract = ContractHarness::default()
        .modal(ModalSurface::Palette)
        .resolve();
    assert_eq!(
        contract.pending_focus,
        Some(PendingFocusEffect::TrapModal(ModalSurface::Palette))
    );
    assert_eq!(
        contract.keyboard,
        KeyboardRoute::Modal(ModalSurface::Palette)
    );
}

#[test]
fn focused_file_row_does_not_suppress_workspace_commands() {
    let ctx = egui::Context::default();
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.button("file row").request_focus();
    });

    assert!(ctx.egui_wants_keyboard_input());
    assert!(!ctx.text_edit_focused());
    let input = text_input_state(&ctx);
    assert_eq!(input, TextInputState::default());

    let presses = [
        press(KeyCode::F5),
        press(KeyCode::Down),
        cmd_press(KeyCode::A),
    ];
    assert_eq!(
        ContractHarness::default()
            .text_input(input)
            .dispatched(&presses),
        [
            Command::RequestCopy,
            Command::CursorDown,
            Command::SelectAll,
        ]
    );
}

#[test]
fn focused_text_edit_suppresses_workspace_commands() {
    let ctx = egui::Context::default();
    let mut text = String::new();
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.text_edit_singleline(&mut text).request_focus();
    });

    assert!(ctx.text_edit_focused());
    let input = text_input_state(&ctx);
    assert_eq!(input.mode(), Some(TextInputMode::TextEdit));
    assert!(
        ContractHarness::default()
            .text_input(input)
            .dispatched(&[press(KeyCode::F5), press(KeyCode::Down)])
            .is_empty()
    );
}

#[test]
fn ime_composition_persists_until_commit_or_focus_loss() {
    let preedit = egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "候補".to_owned(),
        active_range_chars: Some(0..2),
    });
    let commit = egui::Event::Ime(egui::ImeEvent::Commit("候補".to_owned()));
    assert_eq!(ime_composition_transition(&[preedit]), Some(true));
    assert!(next_ime_composition(false, true, Some(true)));
    assert!(next_ime_composition(true, true, None));
    assert!(!next_ime_composition(
        true,
        true,
        ime_composition_transition(&[commit])
    ));
    assert!(!next_ime_composition(true, false, None));

    let contract = ContractHarness::default()
        .modal(ModalSurface::Rename)
        .text_input(TextInputState {
            text_edit_focused: true,
            ime_composing: true,
        })
        .resolve();
    assert_eq!(
        contract.keyboard,
        KeyboardRoute::TextInput(TextInputMode::ImeComposition)
    );
    assert_eq!(
        contract.escape,
        EscapeRoute::TextInput(TextInputMode::ImeComposition)
    );
}

#[test]
fn escape_routes_only_to_modal_text_input_active_preview_or_focus_mode() {
    let modal = ContractHarness::default()
        .modal(ModalSurface::Rename)
        .text_input(TextInputState {
            text_edit_focused: true,
            ime_composing: false,
        })
        .active_preview()
        .focus_mode()
        .resolve();
    assert_eq!(modal.escape, EscapeRoute::Modal(ModalSurface::Rename));

    assert_eq!(
        ContractHarness::default()
            .text_input(TextInputState {
                text_edit_focused: true,
                ime_composing: false,
            })
            .active_preview()
            .focus_mode()
            .resolve()
            .escape,
        EscapeRoute::TextInput(TextInputMode::TextEdit)
    );
    assert_eq!(
        ContractHarness::default()
            .active_preview()
            .focus_mode()
            .resolve()
            .escape,
        EscapeRoute::ActivePreview
    );
    assert_eq!(
        ContractHarness::default().focus_mode().resolve().escape,
        EscapeRoute::FocusMode
    );
    assert_eq!(
        ContractHarness::default().resolve().escape,
        EscapeRoute::None
    );
}

#[test]
fn routed_escape_is_consumed_once_by_its_exact_owner() {
    let mut request = EscapeRoute::Modal(ModalSurface::BatchRename);
    assert!(!consume_escape_route(
        &mut request,
        EscapeRoute::Modal(ModalSurface::Transfer),
    ));
    assert!(!consume_escape_route(
        &mut request,
        EscapeRoute::ActivePreview,
    ));
    assert!(consume_escape_route(
        &mut request,
        EscapeRoute::Modal(ModalSurface::BatchRename),
    ));
    assert_eq!(request, EscapeRoute::None);
    assert!(!consume_escape_route(&mut request, EscapeRoute::None));
}

#[test]
fn transfer_route_only_keeps_queueable_copy_and_move_commands() {
    let presses = [press(KeyCode::F5), press(KeyCode::F6), press(KeyCode::F8)];
    assert_eq!(
        ContractHarness::default()
            .modal(ModalSurface::Transfer)
            .dispatched(&presses),
        [Command::RequestCopy, Command::RequestMove]
    );
}

#[test]
fn file_row_semantics_reach_the_headless_accesskit_tree() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let semantics = file_row_semantics(
        "Projects", "Folder", "12 items", "Today", true, true, false, true,
    );
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let response = ui.selectable_label(semantics.selected, "Projects");
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                ui.is_enabled(),
                semantics.selected,
                &semantics.label,
            )
        });
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(egui::accesskit::Role::Row);
            node.set_selected(semantics.selected);
            if let Some(expanded) = semantics.expanded {
                node.set_expanded(expanded);
            }
        });
    });

    let update = output
        .platform_output
        .accesskit_update
        .expect("headless AccessKit output");
    let row = update
        .nodes
        .iter()
        .map(|(_, node)| node)
        .find(|node| node.role() == egui::accesskit::Role::Row)
        .expect("file row node");
    assert_eq!(
        row.label(),
        Some(
            "Name: Projects; Kind: Folder; Size: 12 items; Modified: Today; State: selected, marked"
        )
    );
    assert_eq!(row.is_selected(), Some(true));
    assert_eq!(row.is_expanded(), Some(false));
    assert!(row.supports_action(egui::accesskit::Action::Focus));
}
