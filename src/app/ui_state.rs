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
pub(crate) struct ContextMenuCandidate {
    pub(crate) target: crate::ports::ContextMenuTarget,
    pub(crate) trigger: crate::ports::ContextMenuTrigger,
    pub(crate) anchor: crate::ports::ContextMenuAnchor,
    pub(crate) focus_id: egui::Id,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ContextMenuRequest {
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
pub(crate) enum ContextMenuPoll {
    Idle,
    AwaitingPaint,
    Ready(Box<ContextMenuRequest>),
}

pub(crate) const fn context_menu_owner(
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
        pub(crate) struct ModalStore {
            $(pub(crate) $field: $ty),+
        }

        pub(crate) const UI_MODAL_REGISTRY: &[(UiModal, ModalSurface)] = &[
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
            pub(crate) fn is_open(&self, modal: UiModal) -> bool {
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
    Recovery => Recovery {
        recovery_open: bool, bool
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
    Sync => Sync {
        sync: Option<SyncState>, option
    },
    Duplicates => Duplicates {
        duplicates: Option<DupState>, option
    },
    Diff => Diff {
        diff: Option<DiffState>, option
    },
    Treemap => Treemap {
        treemap: Option<DiskUsageState>, option
    },
    Find => Find {
        find: Option<FindState>, option
    },
    Archive => Archive {
        archive: Option<ArchiveState>, option
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
    Path => Path {
        path_input: Option<crate::path_probe::PathDialogState>, option
    },
    Recent => Recent {
        recent_input: Option<String>, option
    },
    RunCommand => RunCommand {
        run_command: Option<RunCommandState>, option
    },
    Palette => Palette {
        palette_input: Option<String>, option
    },
);

pub(crate) struct UiState {
    pub(crate) modals: ModalStore,
    /// Type-ahead buffer and the input time of its last keystroke.
    pub(crate) type_ahead: Option<(String, f64)>,
    /// Pending vim-style chord leader and the input time it was pressed.
    pub(crate) chord: Option<(char, f64)>,
    pub(crate) focus_mode: bool,
    pub(crate) focus_started_at: f64,
    /// Escape is read once per frame and consumed by one routed owner.
    pub(crate) escape_request: EscapeRoute,
    /// Identity source for transient widget state across reopenings.
    pub(crate) transient_nonce: u64,
    /// A render barrier between row focus mutation and synchronous AppKit
    /// tracking. Frame B publishes paint/AccessKit; frame C may show the menu.
    pending_context_menu: Option<PendingContextMenu>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            modals: ModalStore::default(),
            type_ahead: None,
            chord: None,
            focus_mode: false,
            focus_started_at: 0.0,
            escape_request: EscapeRoute::None,
            transient_nonce: 0,
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

pub(crate) fn modal_close_requested(window_open: bool, escape_requested: bool) -> bool {
    !window_open || escape_requested
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_all_ui_modals_with_unique_surfaces() {
        assert_eq!(UI_MODAL_REGISTRY.len(), 17);
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
        for surface in [
            ModalSurface::Find,
            ModalSurface::Treemap,
            ModalSurface::Collections,
        ] {
            assert!(
                modal_close_requested(true, true),
                "{surface:?} must close on its routed Escape"
            );
            assert!(!modal_close_requested(true, false));
        }
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
