//! Transient application-shell state.
//!
//! Durable stores, workspace state, services, and caches intentionally stay on
//! [`App`](super::App). This module owns only per-interaction state and modal
//! slots.

use super::{
    ArchiveState, BatchRenameState, CollectionsDialogState, DiffState, DiskUsageState, DupState,
    FindState, HistoryPreviewState, RenameState, RunCommandState, SyncState, UiModal,
};
use crate::accessibility::{EscapeRoute, ModalSurface};

#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenuCandidate {
    pub(crate) target: crate::ports::ContextMenuTarget,
    pub(crate) trigger: crate::ports::ContextMenuTrigger,
    pub(crate) anchor: crate::ports::ContextMenuAnchor,
    pub(crate) focus_id: egui::Id,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenuRequest {
    pub(crate) panel: crate::workspace::ActivePanel,
    pub(crate) invocation: crate::ports::ContextMenuInvocation,
    pub(crate) focus_id: egui::Id,
}

#[derive(Clone, Debug, PartialEq)]
struct PendingContextMenu {
    request: ContextMenuRequest,
    render_passes_remaining: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContextMenuPoll {
    Idle,
    AwaitingPaint,
    Ready(Box<ContextMenuRequest>),
}

pub const fn context_menu_owner(
    _current: crate::workspace::ActivePanel,
    requested: crate::workspace::ActivePanel,
) -> crate::workspace::ActivePanel {
    requested
}

macro_rules! slot_is_open {
    ($slot:expr, option) => {
        $slot.is_some()
    };
    ($slot:expr, bool) => {
        *$slot
    };
}

macro_rules! define_modal_store {
    (
        $(
            $modal:ident => $surface:ident {
                $field:ident: $ty:ty, $kind:ident
            }
        ),+ $(,)?
    ) => {
        #[derive(Default)]
        pub struct ModalStore {
            $(pub(crate) $field: $ty),+
        }

        pub const UI_MODAL_REGISTRY: &[(UiModal, ModalSurface)] = &[
            $((UiModal::$modal, ModalSurface::$surface)),+
        ];

        impl UiModal {
            pub(crate) const fn surface(self) -> ModalSurface {
                match self {
                    $(Self::$modal => ModalSurface::$surface),+
                }
            }

            pub(crate) const fn from_surface(surface: ModalSurface) -> Option<Self> {
                match surface {
                    $(ModalSurface::$surface => Some(Self::$modal)),+,
                    _ => None,
                }
            }
        }

        impl ModalStore {
            pub(crate) const fn is_open(&self, modal: UiModal) -> bool {
                match modal {
                    $(UiModal::$modal => slot_is_open!(&self.$field, $kind)),+
                }
            }

            pub(crate) fn is_surface_open(&self, surface: ModalSurface) -> bool {
                UiModal::from_surface(surface).is_some_and(|modal| self.is_open(modal))
            }

            pub(crate) fn any_open(&self) -> bool {
                UI_MODAL_REGISTRY
                    .iter()
                    .any(|(modal, _)| self.is_open(*modal))
            }
        }
    };
}

define_modal_store!(
    Permissions => Permissions {
        permissions: Option<super::permissions_dialog::PermissionState>, option
    },
    KeyboardHelp => KeyboardHelp {
        keyboard_help: bool, bool
    },
    History => History {
        history_preview: Option<HistoryPreviewState>, option
    },
    Rename => Rename {
        renaming: Option<RenameState>, option
    },
    BatchRename => BatchRename {
        batch_rename: Option<BatchRenameState>, option
    },
    Find => Find {
        find: Option<FindState>, option
    },
    SavedSearch => SavedSearch {
        saved_search_open: bool, bool
    },
    Collections => Collections {
        collections_dialog: Option<CollectionsDialogState>, option
    },
    Mask => Mask {
        mask_input: Option<String>, option
    },
    RunCommand => RunCommand {
        run_command: Option<RunCommandState>, option
    },
    Palette => Palette {
        palette_input: Option<super::omnibar_dialog::OmnibarState>, option
    },
);

/// Tools that take over the inactive pane instead of floating over the
/// workspace: disk usage, diff, synchronize, duplicates and archive. At most
/// one is open; the panes stay interactive while it is.
#[derive(Default)]
pub struct PaneTools {
    /// The pane the open tool replaces, fixed when it opens.
    pub(crate) side: Option<crate::workspace::ActivePanel>,
    pub(crate) treemap: Option<DiskUsageState>,
    pub(crate) diff: Option<DiffState>,
    pub(crate) sync: Option<SyncState>,
    pub(crate) duplicates: Option<DupState>,
    pub(crate) archive: Option<ArchiveState>,
}

impl PaneTools {
    pub(crate) const fn any_open(&self) -> bool {
        self.treemap.is_some()
            || self.diff.is_some()
            || self.sync.is_some()
            || self.duplicates.is_some()
            || self.archive.is_some()
    }

    /// Close whatever tool is open and claim `side` for the next one.
    pub(crate) fn replace_on(&mut self, side: crate::workspace::ActivePanel) {
        *self = Self {
            side: Some(side),
            ..Self::default()
        };
    }
}

pub struct UiState {
    pub(crate) modals: ModalStore,
    pub(crate) tools: PaneTools,
    /// Type-ahead buffer and the input time of its last keystroke.
    pub(crate) type_ahead: Option<(String, f64)>,
    /// The numeric type-ahead buffer has already jumped as a name.
    pub(crate) type_ahead_jumped: bool,
    /// The last type-ahead jump found no matching name.
    pub(crate) type_ahead_miss: bool,
    /// Pending vim-style chord leader and the input time it was pressed.
    pub(crate) chord: Option<(char, f64)>,
    /// Input time a modifier key started being held (key bar reveal).
    pub(crate) modifier_held_since: Option<f64>,
    pub(crate) focus_mode: bool,
    pub(crate) focus_started_at: f64,
    /// Pointer distance accumulated since focus mode was armed.
    pub(crate) focus_moved: f32,
    /// Escape is read once per frame and consumed by one routed owner.
    pub(crate) escape_request: EscapeRoute,
    /// Identity source for transient widget state across reopenings.
    pub(crate) transient_nonce: u64,
    /// The last committed select-by-mask text, prefilled on the next open.
    pub(crate) last_mask: String,
    /// A render barrier between row focus mutation and synchronous `AppKit`
    /// tracking. Frame B publishes paint/AccessKit; frame C may show the menu.
    pending_context_menu: Option<PendingContextMenu>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            modals: ModalStore::default(),
            tools: PaneTools::default(),
            type_ahead: None,
            type_ahead_jumped: false,
            type_ahead_miss: false,
            chord: None,
            modifier_held_since: None,
            focus_mode: false,
            focus_started_at: 0.0,
            focus_moved: 0.0,
            escape_request: EscapeRoute::None,
            transient_nonce: 0,
            last_mask: String::new(),
            pending_context_menu: None,
        }
    }
}

impl UiState {
    pub(crate) fn queue_context_menu(
        &mut self,
        panel: crate::workspace::ActivePanel,
        invocation: crate::ports::ContextMenuInvocation,
        focus_id: egui::Id,
    ) {
        self.pending_context_menu = Some(PendingContextMenu {
            request: ContextMenuRequest {
                panel,
                invocation,
                focus_id,
            },
            render_passes_remaining: 1,
        });
    }

    pub(crate) fn cancel_context_menu(&mut self) {
        self.pending_context_menu = None;
    }

    pub(crate) fn poll_context_menu(&mut self) -> ContextMenuPoll {
        let Some(pending) = self.pending_context_menu.as_mut() else {
            return ContextMenuPoll::Idle;
        };
        if pending.render_passes_remaining > 0 {
            pending.render_passes_remaining -= 1;
            return ContextMenuPoll::AwaitingPaint;
        }
        let ready = self
            .pending_context_menu
            .take()
            .expect("pending request exists");
        ContextMenuPoll::Ready(Box::new(ready.request))
    }
}

pub const fn modal_close_requested(window_open: bool, escape_requested: bool) -> bool {
    !window_open || escape_requested
}

/// Escape first belongs to an open combo/menu popup. A containing modal or
/// pane tool may close only when no popup was open for this frame.
pub fn escape_closes_surface(ctx: &egui::Context, escape_requested: bool) -> bool {
    escape_requested && !egui::Popup::is_any_open(ctx)
}

/// Two-click confirmation for a destructive row action (delete a saved
/// search, remove a project). The first click on `key` arms it; a second
/// click on the same key confirms and disarms. Clicking another row arms that
/// row instead. Returns true when the action should run.
pub fn confirm_on_second_click(armed: &mut Option<String>, key: &str) -> bool {
    if armed.as_deref() == Some(key) {
        *armed = None;
        true
    } else {
        *armed = Some(key.to_string());
        false
    }
}

/// The row armed by [`confirm_on_second_click`] in the dialog keyed `id`.
/// Kept in egui temp memory: it is per-interaction and never persisted.
pub fn armed_row(ctx: &egui::Context, id: egui::Id) -> Option<String> {
    ctx.data(|data| data.get_temp::<String>(id))
}

pub fn set_armed_row(ctx: &egui::Context, id: egui::Id, armed: Option<String>) {
    ctx.data_mut(|data| {
        if let Some(key) = armed {
            data.insert_temp(id, key);
        } else {
            data.remove::<String>(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_row_actions_need_a_second_click_on_the_same_row() {
        let mut armed = None;
        assert!(!confirm_on_second_click(&mut armed, "a"));
        assert_eq!(armed.as_deref(), Some("a"));
        // Another row re-arms instead of confirming.
        assert!(!confirm_on_second_click(&mut armed, "b"));
        assert_eq!(armed.as_deref(), Some("b"));
        assert!(confirm_on_second_click(&mut armed, "b"));
        assert_eq!(armed, None);
    }

    #[test]
    fn registry_covers_all_ui_modals_with_unique_surfaces() {
        assert_eq!(UI_MODAL_REGISTRY.len(), 11);
        for (index, (modal, surface)) in UI_MODAL_REGISTRY.iter().enumerate() {
            assert_eq!(modal.surface(), *surface);
            assert_eq!(UiModal::from_surface(*surface), Some(*modal));
            assert!(
                UI_MODAL_REGISTRY[index + 1..]
                    .iter()
                    .all(|(other_modal, other_surface)| {
                        modal != other_modal && surface != other_surface
                    })
            );
        }
    }

    #[test]
    fn modal_store_reports_individual_and_any_open_slots() {
        let mut store = ModalStore::default();
        assert!(!store.any_open());
        assert!(!store.is_open(UiModal::SavedSearch));
        assert!(!store.is_surface_open(ModalSurface::Mask));

        store.saved_search_open = true;
        assert!(store.is_open(UiModal::SavedSearch));
        assert!(store.is_surface_open(ModalSurface::SavedSearch));
        assert!(store.any_open());

        store.saved_search_open = false;
        store.mask_input = Some("*.rs".to_string());
        assert!(store.is_open(UiModal::Mask));
        assert!(store.any_open());
    }

    #[test]
    fn async_modals_close_when_their_escape_route_is_consumed() {
        use crate::accessibility::{
            TextInputState, UiContractState, consume_escape_route, resolve_ui_contract,
        };
        for surface in [ModalSurface::Find, ModalSurface::Collections] {
            // Escape reaches the modal even while its query field has focus.
            let mut escape = resolve_ui_contract(UiContractState {
                text_input: TextInputState {
                    text_edit_focused: true,
                    ime_composing: false,
                },
                modal: Some(surface),
                pane_tool_open: true,
                ..UiContractState::default()
            })
            .escape;
            assert_eq!(escape, EscapeRoute::Modal(surface));
            // Another surface cannot take it; the owner takes it once.
            assert!(!consume_escape_route(
                &mut escape,
                EscapeRoute::Modal(ModalSurface::Palette)
            ));
            let consumed = consume_escape_route(&mut escape, EscapeRoute::Modal(surface));
            assert!(consumed, "{surface:?} must receive its routed Escape");
            assert_eq!(escape, EscapeRoute::None);
            assert!(!consume_escape_route(
                &mut escape,
                EscapeRoute::Modal(surface)
            ));
            assert!(
                modal_close_requested(true, consumed),
                "{surface:?} must close on its routed Escape"
            );
            assert!(!modal_close_requested(true, false));
        }
    }

    #[test]
    fn an_open_popup_consumes_escape_before_its_surface() {
        let ctx = egui::Context::default();
        assert!(escape_closes_surface(&ctx, true));
        egui::Popup::open_id(&ctx, egui::Id::new("surface_combo_popup"));
        assert!(!escape_closes_surface(&ctx, true));
        assert!(!escape_closes_surface(&ctx, false));
    }

    #[test]
    fn opening_a_pane_tool_replaces_the_previous_one() {
        let mut tools = PaneTools::default();
        assert!(!tools.any_open());
        tools.replace_on(crate::workspace::ActivePanel::Right);
        tools.diff = Some(DiffState {
            name_a: "a".into(),
            name_b: "b".into(),
            lines: Vec::new(),
            message: None,
            keyboard_focused: false,
            scroll_offset: egui::Vec2::ZERO,
        });
        assert!(tools.any_open());
        tools.replace_on(crate::workspace::ActivePanel::Left);
        assert!(!tools.any_open(), "the old tool closes");
        assert_eq!(tools.side, Some(crate::workspace::ActivePanel::Left));
    }

    #[test]
    fn context_menu_request_waits_for_one_published_frame() {
        let mut state = UiState::default();
        let path = std::path::PathBuf::from("/tmp/context-target");
        let invocation = crate::ports::ContextMenuInvocation {
            target: crate::ports::ContextMenuTarget {
                expected: crate::path_identity::PathIdentity::missing(&path),
                path,
            },
            trigger: crate::ports::ContextMenuTrigger::Keyboard,
            anchor: crate::ports::ContextMenuAnchor::ViewRect(crate::ports::ContextMenuViewRect {
                min_x: 10.0,
                min_y: 20.0,
                max_x: 110.0,
                max_y: 44.0,
                native_points_per_ui_point: 1.0,
            }),
        };
        let focus_id = egui::Id::new("context-target");
        state.queue_context_menu(
            crate::workspace::ActivePanel::Right,
            invocation.clone(),
            focus_id,
        );
        assert_eq!(state.poll_context_menu(), ContextMenuPoll::AwaitingPaint);
        assert_eq!(
            state.poll_context_menu(),
            ContextMenuPoll::Ready(Box::new(ContextMenuRequest {
                panel: crate::workspace::ActivePanel::Right,
                invocation,
                focus_id,
            }))
        );
        assert_eq!(state.poll_context_menu(), ContextMenuPoll::Idle);
    }

    #[test]
    fn latest_context_menu_request_wins_and_cancel_retires_it() {
        let mut state = UiState::default();
        let make = |path: &str| crate::ports::ContextMenuInvocation {
            target: crate::ports::ContextMenuTarget {
                path: path.into(),
                expected: crate::path_identity::PathIdentity::missing(std::path::Path::new(path)),
            },
            trigger: crate::ports::ContextMenuTrigger::Pointer,
            anchor: crate::ports::ContextMenuAnchor::GlobalScreen(crate::ports::ContextMenuPoint {
                x: 1.0,
                y: 2.0,
            }),
        };
        state.queue_context_menu(
            crate::workspace::ActivePanel::Left,
            make("/tmp/first"),
            egui::Id::new("first"),
        );
        state.queue_context_menu(
            crate::workspace::ActivePanel::Right,
            make("/tmp/latest"),
            egui::Id::new("latest"),
        );
        assert_eq!(state.poll_context_menu(), ContextMenuPoll::AwaitingPaint);
        state.cancel_context_menu();
        assert_eq!(state.poll_context_menu(), ContextMenuPoll::Idle);
    }

    #[test]
    fn inactive_panel_accessibility_request_becomes_the_context_menu_owner() {
        assert_eq!(
            context_menu_owner(
                crate::workspace::ActivePanel::Left,
                crate::workspace::ActivePanel::Right
            ),
            crate::workspace::ActivePanel::Right
        );
        assert_eq!(
            context_menu_owner(
                crate::workspace::ActivePanel::Right,
                crate::workspace::ActivePanel::Left
            ),
            crate::workspace::ActivePanel::Left
        );
    }
}
