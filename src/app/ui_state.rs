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
        }
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
}
