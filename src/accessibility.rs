//! Accessibility, input-routing, and responsive-layout contracts.

use std::sync::OnceLock;

#[cfg(target_os = "macos")]
use std::process::Command;

pub const MIN_CONTROL_POINTS: f32 = 24.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Preferences {
    pub reduced_motion: bool,
    pub high_contrast: bool,
}

impl Preferences {
    pub fn system() -> Self {
        static SYSTEM: OnceLock<Preferences> = OnceLock::new();
        *SYSTEM.get_or_init(detect_preferences)
    }
}

fn detect_preferences() -> Preferences {
    let reduced_motion = bool_env("COMMANDER_REDUCED_MOTION")
        .unwrap_or_else(|| macos_universal_access("reduceMotion"));
    let high_contrast = bool_env("COMMANDER_HIGH_CONTRAST")
        .unwrap_or_else(|| macos_universal_access("increaseContrast"));
    Preferences {
        reduced_motion,
        high_contrast,
    }
}

fn bool_env(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|value| parse_bool(&value))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn macos_universal_access(key: &str) -> bool {
    Command::new("defaults")
        .args(["read", "com.apple.universalaccess", key])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| parse_bool(&value))
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn macos_universal_access(_key: &str) -> bool {
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusRegion {
    Toolbar,
    LeftPanel,
    RightPanel,
    Operations,
    Dialog,
}

impl FocusRegion {
    pub fn id(self) -> &'static str {
        match self {
            Self::Toolbar => "focus_toolbar",
            Self::LeftPanel => "focus_left_panel",
            Self::RightPanel => "focus_right_panel",
            Self::Operations => "focus_operations",
            Self::Dialog => "focus_dialog",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FocusLayout {
    pub toolbar_visible: bool,
    pub operations_open: bool,
    pub dialog_open: bool,
}

/// Dialogs trap focus; otherwise traversal follows the stable surface order
/// required by egui's panel carving (toolbar, utility surface, then panes).
pub fn focus_order(layout: FocusLayout) -> Vec<FocusRegion> {
    if layout.dialog_open {
        return vec![FocusRegion::Dialog];
    }
    let mut order = Vec::new();
    if layout.toolbar_visible {
        order.push(FocusRegion::Toolbar);
    }
    if layout.operations_open {
        order.push(FocusRegion::Operations);
    }
    order.extend([FocusRegion::LeftPanel, FocusRegion::RightPanel]);
    order
}

pub fn modal_trap_active(was_open: bool, is_open: bool) -> bool {
    was_open || is_open
}

macro_rules! modal_registry {
    ($($surface:ident),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum ModalSurface {
            $($surface),+
        }

        /// Commander input priority, highest first. This is not a claim about
        /// renderer paint order; the render layer must consume this registry
        /// when it implements modal Escape/focus routing.
        pub const MODAL_REGISTRY: &[ModalSurface] = &[$(ModalSurface::$surface),+];
    };
}

modal_registry!(
    Palette,
    RunCommand,
    Recent,
    Path,
    Mask,
    Collections,
    SavedSearch,
    Archive,
    Find,
    Treemap,
    Diff,
    Duplicates,
    Sync,
    BatchRename,
    Rename,
    DeleteActivity,
    Confirmation,
    History,
    Recovery,
    SafeState,
    Transfer,
);

/// Resolve the highest-priority open surface through the single modal
/// registry. Callers provide only state lookup, never another ordering.
pub fn top_open_modal(mut is_open: impl FnMut(ModalSurface) -> bool) -> Option<ModalSurface> {
    MODAL_REGISTRY
        .iter()
        .copied()
        .find(|surface| is_open(*surface))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextInputMode {
    TextEdit,
    ImeComposition,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextInputState {
    pub text_edit_focused: bool,
    pub ime_composing: bool,
}

impl TextInputState {
    pub const fn mode(self) -> Option<TextInputMode> {
        if self.ime_composing {
            Some(TextInputMode::ImeComposition)
        } else if self.text_edit_focused {
            Some(TextInputMode::TextEdit)
        } else {
            None
        }
    }
}

const IME_COMPOSITION_ID: &str = "commander_ime_composition_active";

pub fn ime_composition_transition(events: &[egui::Event]) -> Option<bool> {
    events
        .iter()
        .filter_map(|event| match event {
            egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => Some(!text.is_empty()),
            egui::Event::Ime(egui::ImeEvent::Commit(_)) => Some(false),
            _ => None,
        })
        .next_back()
}

pub const fn next_ime_composition(
    previous: bool,
    text_edit_focused: bool,
    transition: Option<bool>,
) -> bool {
    match transition {
        Some(active) => active,
        None => previous && text_edit_focused,
    }
}

/// Read only TextEdit focus, not generic egui keyboard focus. IME composition
/// is retained across frames until commit, empty preedit, or focus loss.
pub fn text_input_state(ctx: &egui::Context) -> TextInputState {
    let text_edit_focused = ctx.text_edit_focused();
    let transition = ctx.input(|input| ime_composition_transition(&input.events));
    let ime_composing = ctx.data_mut(|data| {
        let id = egui::Id::new(IME_COMPOSITION_ID);
        let previous = data.get_temp::<bool>(id).unwrap_or(false);
        let active = next_ime_composition(previous, text_edit_focused, transition);
        data.insert_temp(id, active);
        active
    });
    TextInputState {
        text_edit_focused,
        ime_composing,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardRoute {
    Workspace,
    TextInput(TextInputMode),
    Transfer,
    Modal(ModalSurface),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeRoute {
    Modal(ModalSurface),
    TextInput(TextInputMode),
    ActivePreview,
    FocusMode,
    None,
}

/// A render-layer effect that is deliberately not claimed as applied by the
/// headless reducer. Track 8 can consume it while implementing real focus
/// trapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingFocusEffect {
    TrapModal(ModalSurface),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiContractState {
    pub text_input: TextInputState,
    pub modal: Option<ModalSurface>,
    pub active_preview_open: bool,
    pub focus_mode: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiContract {
    pub keyboard: KeyboardRoute,
    pub escape: EscapeRoute,
    pub pending_focus: Option<PendingFocusEffect>,
}

/// Resolve input ownership without an egui frame. IME composition owns Escape
/// before any Commander surface so cancelling a candidate cannot close a
/// dialog. A regular TextEdit inside a modal still leaves Escape to the modal.
pub fn resolve_ui_contract(state: UiContractState) -> UiContract {
    let text_input = state.text_input.mode();
    let keyboard = if let Some(mode) = text_input {
        KeyboardRoute::TextInput(mode)
    } else {
        match state.modal {
            Some(ModalSurface::Transfer) => KeyboardRoute::Transfer,
            Some(surface) => KeyboardRoute::Modal(surface),
            None => KeyboardRoute::Workspace,
        }
    };
    let escape = if text_input == Some(TextInputMode::ImeComposition) {
        EscapeRoute::TextInput(TextInputMode::ImeComposition)
    } else if let Some(surface) = state.modal {
        EscapeRoute::Modal(surface)
    } else if text_input == Some(TextInputMode::TextEdit) {
        EscapeRoute::TextInput(TextInputMode::TextEdit)
    } else if state.active_preview_open {
        EscapeRoute::ActivePreview
    } else if state.focus_mode {
        EscapeRoute::FocusMode
    } else {
        EscapeRoute::None
    };
    UiContract {
        keyboard,
        escape,
        pending_focus: state.modal.map(PendingFocusEffect::TrapModal),
    }
}

/// Consume a routed Escape request only when `owner` is its exact recipient.
/// Resetting the request makes the single-owner guarantee explicit even when
/// several surfaces are rendered during the same frame.
pub fn consume_escape_route(request: &mut EscapeRoute, owner: EscapeRoute) -> bool {
    if owner == EscapeRoute::None || *request != owner {
        return false;
    }
    *request = EscapeRoute::None;
    true
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlSpec {
    pub name: &'static str,
    pub width: f32,
    pub height: f32,
    pub native_exception: Option<&'static str>,
}

pub const CONTROL_CATALOG: [ControlSpec; 8] = [
    ControlSpec {
        name: "icon button",
        width: 24.0,
        height: 24.0,
        native_exception: None,
    },
    ControlSpec {
        name: "path navigation button",
        width: 28.0,
        height: 28.0,
        native_exception: None,
    },
    ControlSpec {
        name: "operations tab",
        width: 68.0,
        height: 26.0,
        native_exception: None,
    },
    ControlSpec {
        name: "filter field",
        width: 80.0,
        height: 26.0,
        native_exception: None,
    },
    ControlSpec {
        name: "compact file row",
        width: 120.0,
        height: 24.0,
        native_exception: None,
    },
    ControlSpec {
        name: "toolbar command",
        width: 42.0,
        height: 24.0,
        native_exception: None,
    },
    ControlSpec {
        name: "native scroll bar",
        width: 6.0,
        height: 80.0,
        native_exception: Some("egui expands the pointer hit target around the painted thumb"),
    },
    ControlSpec {
        name: "panel resize separator",
        width: 8.0,
        height: 80.0,
        native_exception: Some("egui owns an expanded resize interaction zone"),
    },
];

pub fn control_audit_failures(catalog: &[ControlSpec]) -> Vec<&'static str> {
    catalog
        .iter()
        .filter(|control| {
            control.native_exception.is_none()
                && (control.width < MIN_CONTROL_POINTS || control.height < MIN_CONTROL_POINTS)
        })
        .map(|control| control.name)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisualChannel {
    ActivePane,
    KeyboardFocus,
    Cursor,
    Selection,
    Mark,
    Difference,
    Error,
    Disabled,
}

impl VisualChannel {
    pub const ALL: [Self; 8] = [
        Self::ActivePane,
        Self::KeyboardFocus,
        Self::Cursor,
        Self::Selection,
        Self::Mark,
        Self::Difference,
        Self::Error,
        Self::Disabled,
    ];

    pub fn non_color_cue(self) -> &'static str {
        match self {
            Self::ActivePane => "ACTIVE text and top rule",
            Self::KeyboardFocus => "double outline",
            Self::Cursor => "leading chevron and thin outline",
            Self::Selection => "check mark and filled row",
            Self::Mark => "right-edge stripe and diamond",
            Self::Difference => "edge stripe and relation symbol",
            Self::Error => "error icon and text",
            Self::Disabled => "disabled state and explanatory tooltip",
        }
    }
}

pub fn visual_channel_snapshot() -> String {
    VisualChannel::ALL
        .iter()
        .map(|channel| format!("{channel:?}:{}", channel.non_color_cue()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRowSemantics {
    pub label: String,
    pub selected: bool,
    pub expanded: Option<bool>,
}

#[allow(clippy::too_many_arguments)]
pub fn file_row_semantics(
    name: &str,
    kind: &str,
    size: &str,
    modified: &str,
    selected: bool,
    marked: bool,
    cursor: bool,
    _is_directory: bool,
) -> FileRowSemantics {
    let mut states = Vec::new();
    if selected {
        states.push("selected");
    }
    if marked {
        states.push("marked");
    }
    if cursor {
        states.push("cursor");
    }
    let state = if states.is_empty() {
        String::new()
    } else {
        format!("; State: {}", states.join(", "))
    };
    FileRowSemantics {
        label: format!("Name: {name}; Kind: {kind}; Size: {size}; Modified: {modified}{state}"),
        selected,
        // Directory rows navigate to another listing; they do not expose an
        // inline expand/collapse action and must not announce "collapsed".
        expanded: None,
    }
}

pub fn sanitize_text_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(0.8, 2.0)
    } else {
        1.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarMode {
    Full,
    Compact,
}

pub fn toolbar_mode(available_width: f32) -> ToolbarMode {
    if available_width >= 1_180.0 {
        ToolbarMode::Full
    } else {
        ToolbarMode::Compact
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationsPlacement {
    Right,
    Bottom,
}

pub fn operations_placement(available_width: f32) -> OperationsPlacement {
    if available_width >= 760.0 {
        OperationsPlacement::Right
    } else {
        OperationsPlacement::Bottom
    }
}

pub fn pane_min_width(available_width: f32) -> f32 {
    (available_width * 0.28).clamp(100.0, 300.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileRowLayout {
    pub name_width: f32,
    pub metadata_width: f32,
    pub gap: f32,
    pub show_metadata: bool,
}

pub fn file_row_layout(available_width: f32) -> FileRowLayout {
    let width = available_width.max(24.0);
    if width < 160.0 {
        return FileRowLayout {
            name_width: width,
            metadata_width: 0.0,
            gap: 0.0,
            show_metadata: false,
        };
    }
    let gap = 6.0;
    let metadata_width = (width * 0.38).clamp(96.0, 176.0);
    FileRowLayout {
        name_width: (width - metadata_width - gap).max(24.0),
        metadata_width,
        gap,
        show_metadata: true,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn right(self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    fn intersection_area(self, other: Self) -> f32 {
        let width = (self.right().min(other.right()) - self.x.max(other.x)).max(0.0);
        let height = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0.0);
        width * height
    }
}

/// Place a stack near a viewport corner without covering focused rows or
/// active errors. When no candidate is safe, the caller should defer the
/// transient overlay instead of obscuring higher-priority state.
pub fn place_overlay(viewport: Rect, width: f32, height: f32, avoid: &[Rect]) -> Option<Rect> {
    let margin = 12.0;
    let right = viewport.right() - width - margin;
    let bottom = viewport.bottom() - height - margin;
    let left = viewport.x + margin;
    let top = viewport.y + margin;
    let candidates = [
        Rect {
            x: right,
            y: bottom,
            width,
            height,
        },
        Rect {
            x: right,
            y: top,
            width,
            height,
        },
        Rect {
            x: left,
            y: bottom,
            width,
            height,
        },
        Rect {
            x: left,
            y: top,
            width,
            height,
        },
    ];
    candidates.into_iter().find(|candidate| {
        avoid
            .iter()
            .all(|area| candidate.intersection_area(*area) == 0.0)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragWorkflow {
    MoveToHighlightedFolder,
    CopyToHighlightedFolder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DragAlternative {
    pub workflow: DragWorkflow,
    pub keyboard: &'static str,
    pub single_pointer: &'static str,
}

pub const DRAG_ALTERNATIVES: [DragAlternative; 2] = [
    DragAlternative {
        workflow: DragWorkflow::MoveToHighlightedFolder,
        keyboard: "Cmd+Enter",
        single_pointer: "Move In toolbar command",
    },
    DragAlternative {
        workflow: DragWorkflow::CopyToHighlightedFolder,
        keyboard: "Cmd+Shift+Enter",
        single_pointer: "Copy In toolbar command",
    },
];

pub fn drag_alternative(workflow: DragWorkflow) -> DragAlternative {
    DRAG_ALTERNATIVES
        .iter()
        .copied()
        .find(|alternative| alternative.workflow == workflow)
        .expect("every drag workflow has an alternative")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_parser_accepts_explicit_boolean_spellings() {
        assert_eq!(parse_bool(" YES "), Some(true));
        assert_eq!(parse_bool("off"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
    }

    #[test]
    fn focus_order_traps_dialogs_and_covers_every_application_surface() {
        assert_eq!(
            focus_order(FocusLayout {
                toolbar_visible: true,
                operations_open: true,
                dialog_open: false,
            }),
            [
                FocusRegion::Toolbar,
                FocusRegion::Operations,
                FocusRegion::LeftPanel,
                FocusRegion::RightPanel,
            ]
        );
        assert_eq!(
            focus_order(FocusLayout {
                toolbar_visible: true,
                operations_open: true,
                dialog_open: true,
            }),
            [FocusRegion::Dialog]
        );
        assert!(modal_trap_active(true, false));
        assert!(modal_trap_active(false, true));
        assert!(!modal_trap_active(false, false));
    }

    #[test]
    fn every_focus_region_has_a_unique_stable_id() {
        let regions = [
            FocusRegion::Toolbar,
            FocusRegion::LeftPanel,
            FocusRegion::RightPanel,
            FocusRegion::Operations,
            FocusRegion::Dialog,
        ];
        let ids = regions.map(FocusRegion::id);
        let unique = ids.into_iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), regions.len());
    }

    #[test]
    fn compact_control_audit_has_only_documented_native_exceptions() {
        assert!(control_audit_failures(&CONTROL_CATALOG).is_empty());
        assert_eq!(
            CONTROL_CATALOG
                .iter()
                .filter(|control| control.native_exception.is_some())
                .count(),
            2
        );
    }

    #[test]
    fn semantic_colors_always_have_a_non_color_cue_snapshot() {
        let snapshot = visual_channel_snapshot();
        assert_eq!(snapshot.lines().count(), VisualChannel::ALL.len());
        assert!(snapshot.contains("KeyboardFocus:double outline"));
        assert!(snapshot.contains("Error:error icon and text"));
    }

    #[test]
    fn file_rows_name_columns_and_publish_selection_and_expansion() {
        let row = file_row_semantics(
            "Projects", "Folder", "12 items", "Today", true, true, false, true,
        );
        assert_eq!(
            row.label,
            "Name: Projects; Kind: Folder; Size: 12 items; Modified: Today; State: selected, marked"
        );
        assert!(row.selected);
        assert_eq!(row.expanded, None);
    }

    #[test]
    fn two_hundred_percent_scale_keeps_control_and_row_geometry_stable() {
        let scale = sanitize_text_scale(2.0);
        assert_eq!(MIN_CONTROL_POINTS * scale, 48.0);
        let logical_pane_width = 900.0 / scale / 2.0;
        let row = file_row_layout(logical_pane_width);
        assert_eq!(toolbar_mode(900.0 / scale), ToolbarMode::Compact);
        assert_eq!(
            operations_placement(900.0 / scale),
            OperationsPlacement::Bottom
        );
        let pane_min = pane_min_width(900.0 / scale);
        assert!(pane_min * 2.0 + 6.0 <= 900.0 / scale);
        assert!(row.name_width >= 24.0);
        assert!(row.metadata_width >= 96.0);
        assert!(row.name_width + row.metadata_width + row.gap <= logical_pane_width + 0.01);
        let narrow_row = file_row_layout(96.0);
        assert!(!narrow_row.show_metadata);
        assert_eq!(narrow_row.name_width, 96.0);
        assert_eq!(narrow_row.metadata_width, 0.0);
        assert_eq!(sanitize_text_scale(f32::NAN), 1.0);
        assert_eq!(sanitize_text_scale(9.0), 2.0);
    }

    #[test]
    fn overlays_choose_a_corner_that_does_not_cover_focus_or_error() {
        let viewport = Rect {
            width: 1_000.0,
            height: 700.0,
            ..Default::default()
        };
        let focus = Rect {
            x: 700.0,
            y: 560.0,
            width: 260.0,
            height: 80.0,
        };
        let error = Rect {
            x: 350.0,
            y: 220.0,
            width: 300.0,
            height: 220.0,
        };
        let placed = place_overlay(viewport, 280.0, 140.0, &[focus, error])
            .expect("a safe viewport corner remains");
        assert_eq!(placed.intersection_area(focus), 0.0);
        assert_eq!(placed.intersection_area(error), 0.0);
    }

    #[test]
    fn overlays_defer_when_every_safe_candidate_is_obscured() {
        let viewport = Rect {
            width: 320.0,
            height: 240.0,
            ..Default::default()
        };
        assert_eq!(place_overlay(viewport, 280.0, 140.0, &[viewport]), None);
    }

    #[test]
    fn every_drag_workflow_has_keyboard_and_single_pointer_alternatives() {
        assert_eq!(DRAG_ALTERNATIVES.len(), 2);
        assert!(
            DRAG_ALTERNATIVES
                .iter()
                .all(|alternative| !alternative.keyboard.is_empty()
                    && !alternative.single_pointer.is_empty())
        );
    }
}
