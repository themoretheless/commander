//! Keyboard shortcuts read before the hex window is drawn.

use super::*;

/// Navigation and focus requests taken from this frame's keyboard input.
pub(super) struct HexShortcuts {
    pub(super) open_characters: bool,
    pub(super) search: Option<crate::hex_search::Direction>,
    pub(super) copy: Option<String>,
    pub(super) navigate: Option<u64>,
    pub(super) history_navigation: Option<bool>,
    pub(super) focus_offset: bool,
    pub(super) focus_search: bool,
}

impl HexViewer {
    /// Consume the viewer's shortcuts; `busy` blocks those that start reads.
    pub(super) fn take_shortcuts(&mut self, ctx: &egui::Context, busy: bool) -> HexShortcuts {
        let mut search = None;
        let mut copy = None;
        let mut navigate = None;
        let mut history_navigation = None;
        let open_characters = if !egui::Popup::is_any_open(ctx) && ctx.input(character_shortcut) {
            let menu = ctx.input(|input| input.modifiers.shift);
            ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                input.consume_key(modifiers, egui::Key::F8);
            });
            if !menu {
                self.select_character_encoding(self.character_encoding.next());
            }
            menu
        } else {
            false
        };

        if !busy
            && !egui::Popup::is_any_open(ctx)
            && !ctx.egui_wants_keyboard_input()
            && let Some(direction) = ctx.input(repeat_search_shortcut)
        {
            ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                input.consume_key(modifiers, egui::Key::F7);
                input.consume_key(modifiers, egui::Key::Space);
            });
            search = Some(direction);
        }
        if !busy
            && !egui::Popup::is_any_open(ctx)
            && !ctx.egui_wants_keyboard_input()
            && !ctx
                .plugin::<egui::text_selection::LabelSelectionState>()
                .lock()
                .has_selection()
            && ctx.input(copy_match_shortcut)
            && let Some(bytes) = self.matched_bytes()
        {
            copy = Some(byte_text(bytes, ByteFormat::Hex));
            ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                input.consume_key(modifiers, egui::Key::C);
                input.consume_key(modifiers, egui::Key::Insert);
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Copy));
            });
        }
        if !busy
            && !egui::Popup::is_any_open(ctx)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(clear_match_shortcut)
        {
            ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::CTRL, egui::Key::U);
            });
            self.clear_match();
        }
        if !busy
            && !egui::Popup::is_any_open(ctx)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(page_navigation_key)
            && let Some(page) = &self.page
        {
            navigate = ctx
                .input_mut(|input| {
                    let modifiers = input.modifiers;
                    if modifiers.alt || modifiers.shift || modifiers.mac_cmd {
                        return None;
                    }
                    if input.consume_key(modifiers, egui::Key::Home) {
                        Some(0)
                    } else if input.consume_key(modifiers, egui::Key::End) {
                        Some(page.last_offset())
                    } else if modifiers.ctrl && input.consume_key(modifiers, egui::Key::ArrowLeft) {
                        page.offset.checked_sub(1)
                    } else if modifiers.ctrl && input.consume_key(modifiers, egui::Key::ArrowRight)
                    {
                        page.offset
                            .checked_add(1)
                            .filter(|offset| *offset <= page.stamp.identity.size)
                    } else if !modifiers.ctrl && input.consume_key(modifiers, egui::Key::PageUp) {
                        Some(page.previous_offset())
                    } else if !modifiers.ctrl && input.consume_key(modifiers, egui::Key::PageDown) {
                        page.next_offset()
                    } else {
                        None
                    }
                })
                .filter(|offset| *offset != page.offset);
        }
        if !busy && !egui::Popup::is_any_open(ctx) && !ctx.egui_wants_keyboard_input() {
            history_navigation = ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                if modifiers.mac_cmd {
                    return None;
                }
                if modifiers.ctrl && !modifiers.alt && input.consume_key(modifiers, egui::Key::Z) {
                    Some(!modifiers.shift)
                } else if modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.shift
                    && input.consume_key(modifiers, egui::Key::Backspace)
                {
                    Some(true)
                } else {
                    None
                }
            });
        }
        if !busy && !egui::Popup::is_any_open(ctx) && !ctx.egui_wants_keyboard_input() {
            let adjustment = ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                if !modifiers.alt || modifiers.shift || modifiers.mac_cmd {
                    return None;
                }
                if input.consume_key(modifiers, egui::Key::ArrowLeft) {
                    Some((false, modifiers.ctrl))
                } else if input.consume_key(modifiers, egui::Key::ArrowRight) {
                    Some((true, modifiers.ctrl))
                } else {
                    None
                }
            });
            if let Some((increase, block)) = adjustment {
                let width = self.row_width.0;
                self.row_width = RowWidth::new(match (increase, block) {
                    (true, false) => width.saturating_add(1),
                    (false, false) => width.saturating_sub(1),
                    (true, true) => (width / 16 + 1) * 16,
                    (false, true) => (width.saturating_sub(1) / 16) * 16,
                });
            }
        }
        if !busy && !egui::Popup::is_any_open(ctx) {
            let bookmark_key = ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                if !modifiers.ctrl || modifiers.alt || modifiers.mac_cmd {
                    return None;
                }
                [
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                    egui::Key::Num6,
                    egui::Key::Num7,
                    egui::Key::Num8,
                    egui::Key::Num9,
                    egui::Key::Num0,
                ]
                .into_iter()
                .enumerate()
                .find_map(|(slot, key)| {
                    input
                        .consume_key(modifiers, key)
                        .then_some((slot, modifiers.shift))
                })
            });
            if let Some((slot, set)) = bookmark_key {
                if set {
                    if self.error.is_none() && self.page.is_some() {
                        self.bookmark_slot = slot;
                        self.set_bookmark();
                    }
                } else if let Some(offset) = self.bookmarks[slot] {
                    self.bookmark_slot = slot;
                    navigate = Some(offset);
                }
            }
        }
        let (focus_offset, mut focus_search) =
            if !busy && !egui::Popup::is_any_open(ctx) && ctx.input(focus_shortcut) {
                ctx.input_mut(|input| {
                    if !input.modifiers.is_none() {
                        return (false, false);
                    }
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::F5),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::F7),
                    )
                })
            } else {
                (false, false)
            };
        if self.restore_search_focus && !busy {
            self.restore_search_focus = false;
            let focus = ctx.memory(|memory| memory.focused());
            if !egui::Popup::is_any_open(ctx)
                && !focus_offset
                && focus.is_none_or(|id| id == egui::Id::new("hex-search-query"))
                && !ctx.input(|input| input.pointer.any_pressed())
            {
                focus_search = true;
            }
        }
        HexShortcuts {
            open_characters,
            search,
            copy,
            navigate,
            history_navigation,
            focus_offset,
            focus_search,
        }
    }
}
