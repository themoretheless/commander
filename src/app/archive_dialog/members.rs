//! The member list: folder navigation, view filters, copy actions, sortable rows and extraction.

use super::*;

/// What the user asked for while the member list was drawn; applied after the frame.
#[derive(Default)]
pub(super) struct ListingActions {
    pub(super) extract_selected: bool,
    pub(super) extract_all: bool,
    pub(super) copy_members: Option<Result<(String, usize), String>>,
    pub(super) sort_requested: Option<ArchiveField>,
    pub(super) navigate: Option<Option<PathBuf>>,
    pub(super) history_requested: Option<bool>,
}

pub(super) fn listing_ui(
    ui: &mut egui::Ui,
    state: &mut ArchiveState,
    t: crate::theme::ThemeColors,
    cursor_move: Option<MemberCursorMove>,
    toggle_cursor: bool,
    rows_height: f32,
    dest: &std::path::Path,
) -> ListingActions {
    let ctx = &ui.ctx().clone();
    let mut actions = ListingActions::default();
    let Some(listing) = &state.listing else {
        return actions;
    };
    if let Ok(tree) = &listing.tree {
        egui::CollapsingHeader::new("Go to folder")
            .id_salt("archive-goto-folder")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let response = ui.add(egui::TextEdit::singleline(&mut state.member_filter.folder_input)
                        .char_limit(4096).desired_width(280.0).hint_text("From archive root, e.g. src/nested"));
                    if response.changed() { state.member_filter.folder_input_error = None; }
                    let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui.button("Go").clicked() || enter {
                        match tree.resolve_folder(&state.member_filter.folder_input) {
                            Ok(folder) => {
                                state.member_filter.folder_input_error = None;
                                actions.navigate = Some(Some(folder));
                            }
                            Err(error) => { state.member_filter.folder_input_error = Some(error); }
                        }
                    }
                    ui.menu_button("Suggestions", |ui| {
                        ui.set_max_width(420.0);
                        if state.member_filter.folder_completions.as_ref().is_none_or(|(query, _, _)| query != &state.member_filter.folder_input) {
                            let (rows, more) = tree.complete_folders(&state.member_filter.folder_input);
                            state.member_filter.folder_completions = Some((state.member_filter.folder_input.clone(), rows, more));
                        }
                        let (_, rows, more) = state.member_filter.folder_completions.as_ref().unwrap();
                        if rows.is_empty() { ui.label("No indexed folder matches this prefix."); }
                        for folder in rows {
                            let response = ui.add(egui::Label::new(folder.to_string_lossy()).truncate().sense(Sense::click()))
                                .on_hover_text(folder.to_string_lossy());
                            if response.clicked() {
                                state.member_filter.folder_input = folder.to_str().unwrap().to_owned();
                                state.member_filter.folder_input_error = None;
                                actions.navigate = Some(Some(folder.clone()));
                                ui.close();
                            }
                        }
                        if *more { ui.small("More folders match; type a longer prefix (showing first 12)."); }
                    });
                });
                if let Some(error) = &state.member_filter.folder_input_error { ui.colored_label(t.accent_red, error); }
                ui.small("Internal folder path; empty or / opens archive root. Existing filters stay active.");
            });
    }
    ui.horizontal_wrapped(|ui| {
        for (back, label) in [(true, "Back"), (false, "Forward")] {
            let enabled = if back {
                !state.member_filter.past.is_empty()
            } else {
                !state.member_filter.future.is_empty()
            };
            let response = ui
                .add_enabled(enabled, egui::Button::new(label))
                .on_hover_text(if back {
                    "Previous view inside this archive"
                } else {
                    "Next view inside this archive"
                });
            if response.clicked() {
                actions.history_requested = Some(back);
            }
        }

        if ui
            .selectable_label(state.member_filter.folder.is_none(), "All members")
            .clicked()
        {
            actions.navigate = Some(None);
        }
        if ui
            .add_enabled(
                listing.tree.is_ok(),
                egui::Button::new("Folders").selected(state.member_filter.folder.is_some()),
            )
            .clicked()
        {
            actions.navigate = Some(Some(PathBuf::new()));
        }
    });
    if let Err(error) = &listing.tree {
        ui.small(error);
    }
    if let Some(folder) = &state.member_filter.folder {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Select folder contents").on_hover_text("Add all listed members in this folder and its subfolders, including filtered-out members").clicked() {
                select_folder_contents(listing, folder, &mut state.selected);
            }
        });
        egui::ScrollArea::horizontal()
            .id_salt("archive-breadcrumbs")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!folder.as_os_str().is_empty(), egui::Button::new("Up"))
                        .clicked()
                    {
                        actions.navigate = Some(Some(
                            folder.parent().unwrap_or(std::path::Path::new("")).into(),
                        ));
                    }
                    if ui.button("Archive root").clicked() {
                        actions.navigate = Some(Some(PathBuf::new()));
                    }
                    let mut prefix = PathBuf::new();
                    for part in folder.components() {
                        prefix.push(part.as_os_str());
                        if ui.button(part.as_os_str().to_string_lossy()).clicked() {
                            actions.navigate = Some(Some(prefix.clone()));
                        }
                    }
                });
            });
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Narrow to selection")
            .on_hover_text("Show a snapshot of selected real members in All members, keeping the path filter. Empty selection removes narrowing; later deselection does not hide snapshot rows.")
            .clicked()
        {
            state.member_filter.narrow(&state.selected, listing.members.len());
        }
        if ui.add_enabled(state.member_filter.narrowed.is_some(), egui::Button::new("Remove narrowing"))
            .on_hover_text("Show all members allowed by the current folder and path filter; keep selection")
            .clicked()
        {
            state.member_filter.unnarrow();
        }
        if let Some(rows) = &state.member_filter.narrowed {
            ui.label(format!("Narrowed to {} members", rows.len()));
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.menu_button("Types", |ui| {
            let kinds = &mut state.member_filter.kinds;
            ui.checkbox(&mut kinds.files, "Regular files");
            ui.checkbox(&mut kinds.folders, "Directories");
            ui.checkbox(&mut kinds.links, "Symbolic links");
            ui.checkbox(&mut kinds.other, "Other members");
            ui.separator();
            ui.checkbox(&mut kinds.executable_only, "Executable files only")
                .on_hover_text("Require a regular file with explicitly declared executable status. Unknown metadata, directories, links and other records do not match. This intersects the type choices above and does not execute or read payloads.");
            if ui.button("Show all types").clicked() { *kinds = crate::archive_filter::Kinds { hide_dotfiles: kinds.hide_dotfiles, ..Default::default() }; }
            ui.label("Navigation folders stay available. Hidden selections are kept.");
        });
        if state.member_filter.kinds != Default::default() { ui.label("Types / visibility filtered"); }
        ui.label("Sort:");
        let order = &mut state.member_filter.order;
        let previous = order.field;
        egui::ComboBox::from_id_salt("archive-sort")
            .selected_text(order.field.label())
            .show_ui(ui, |ui| {
                for field in ArchiveField::ALL {
                    if field.available(listing.kind) {
                        ui.selectable_value(&mut order.field, field, field.label());
                    }
                }
            });
        if previous != order.field {
            order.descending = order.field.default_descending();
        }
        ui.checkbox(&mut order.descending, "Reverse");
        ui.checkbox(&mut order.natural, "Natural names").on_hover_text("Numbers in names sort by value: file2 before file10. Also breaks ties in other columns.");
        let browsing_folders = state.member_filter.folder.is_some();
        let mut folders_first = browsing_folders || order.folders_first;
        ui.add_enabled(
            !browsing_folders,
            egui::Checkbox::new(&mut folders_first, "Folders first"),
        );
        if !browsing_folders {
            order.folders_first = folders_first;
        }
    });
    state.member_filter.visible(listing, &state.filter);
    if let Some(Err(error)) = &state.member_filter.pattern {
        ui.colored_label(ui.visuals().error_fg_color, format!("Filter: {error}"));
    }
    if let Some(Err(error)) = &state.member_filter.size_pattern {
        ui.colored_label(ui.visuals().error_fg_color, format!("Size: {error}"));
    }
    if let Some(Err(error)) = &state.member_filter.depth_pattern {
        ui.colored_label(ui.visuals().error_fg_color, format!("Depth: {error}"));
    }
    if let Some(Err(error)) = &state.member_filter.exclusion.pattern {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Exclude glob: {error}"),
        );
    }
    if let Some(Err(error)) = &state.member_filter.extensions_pattern {
        ui.colored_label(ui.visuals().error_fg_color, format!("Extensions: {error}"));
    }
    if let Some(Err(error)) = &state.member_filter.modified_pattern {
        ui.colored_label(ui.visuals().error_fg_color, format!("Modified: {error}"));
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Copy:");
        for format in crate::archive_export::Format::ALL {
            let response = ui.selectable_value(&mut state.copy_format, format, format.label());
            let response = response.on_hover_text(match format {
                crate::archive_export::Format::NamesWithoutExtension => "Remove only the final suffix lexically: archive.tar.gz becomes archive.tar; .env stays .env. Applies to directory names too.",
                crate::archive_export::Format::ParentFolders => "Copy distinct internal parent folders in displayed order; . means the archive root. The count on Copy refers to source members.",
                _ => "Copy listing metadata only, without extracting or reading member contents.",
            });
            #[cfg(feature = "visual-qa")]
            ctx.data_mut(|data| data.insert_temp(egui::Id::new(("archive-copy-format", format.label())), response.rect));
            #[cfg(not(feature = "visual-qa"))]
            let _ = response;
        }
        for (selected_only, label) in [(false, "Copy visible"), (true, "Copy selected")] {
            let count = if selected_only { state.selected.iter().filter(|&&id| id < listing.members.len()).count() } else { state.member_filter.visible.iter().filter(|&&id| id < listing.members.len()).count() };
            let response = ui.add_enabled(count > 0, egui::Button::new(format!("{label} ({count})")))
                .on_hover_text(if selected_only { "Copy every selected real member, including outside this folder/filter, in current sort order. No extraction." } else { "Copy visible real members in displayed order; synthetic navigation folders are excluded. No extraction." });
            #[cfg(feature = "visual-qa")]
            ctx.data_mut(|data| data.insert_temp(egui::Id::new(("archive-copy", selected_only)), response.rect));
            if response.clicked() {
                let rows = state.member_filter.copy_rows(listing, &state.selected, selected_only);
                actions.copy_members = Some(crate::archive_export::format(&state.path, listing, &rows, state.copy_format).map(|text| (text, count)));
            }
        }
    });
    let order = state.member_filter.order;
    let folder_mode = state.member_filter.folder.is_some();
    let narrowed_view = state.member_filter.narrowed.is_some();
    let kind_filtered = state.member_filter.kinds != Default::default();
    let path_excluded = state.member_filter.exclude_matches;
    let MemberFilter {
        visible,
        visible_set,
        focused,
        scroll_to_focused,
        range_anchor,
        selection_memory,
        selection_mask,
        selection_stats,
        lower_paths,
        reset_scroll,
        ..
    } = &mut state.member_filter;
    let visible: &[usize] = visible;
    if let Some(cursor) = cursor_move {
        let previous = *focused;
        *focused = step_member_cursor(visible, *focused, !cursor.down, cursor.down, cursor.rows);
        if cursor.extend {
            let anchor = range_anchor.or(previous).or(*focused);
            *range_anchor = anchor;
            select_member_range(
                visible,
                anchor,
                *focused,
                listing.members.len(),
                &mut state.selected,
            );
        } else {
            *range_anchor = *focused;
        }
        *scroll_to_focused = true;
    }
    if toggle_cursor && let Some(index) = *focused {
        if index >= listing.members.len() {
            if let Ok(tree) = &listing.tree {
                actions.navigate = Some(Some(
                    tree.directories[index - listing.members.len()].path.clone(),
                ));
            }
        } else if !state.selected.remove(&index) {
            state.selected.insert(index);
        }
    }
    let declared_bytes = listing
        .declared_uncompressed_bytes
        .min(u128::from(u64::MAX)) as u64;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} · {} rows · {} archive members | {} unpacked",
                listing.kind.label(),
                visible.len(),
                listing.declared_members,
                format_size(declared_bytes)
            ))
            .size(11.0)
            .color(t.text_muted),
        );
        if listing.unsafe_members > 0 {
            ui.label(
                egui::RichText::new(format!(
                    "{} with unsafe paths hidden",
                    listing.unsafe_members
                ))
                .size(11.0)
                .color(t.accent_warning),
            )
            .on_hover_text(
                "These entries have absolute paths or ../ parts that would \
                 write outside the destination, so they are left out",
            );
        }
        if listing.unreadable_members > 0 {
            ui.label(
                egui::RichText::new(format!("{} unreadable", listing.unreadable_members))
                    .size(11.0)
                    .color(t.accent_warning),
            );
        }
        if listing.truncated {
            ui.label(
                egui::RichText::new("Scan limit reached: partial listing")
                    .size(11.0)
                    .color(t.accent_warning),
            );
        }
    });
    ui.add_space(4.0);
    ui.separator();
    let show_modified = ui.available_width() >= 560.0;
    let show_crc =
        listing.kind == crate::archive::ArchiveKind::Zip && ui.available_width() >= 500.0;
    let tail_width =
        230.0 + if show_crc { 86.0 } else { 0.0 } + if show_modified { 126.0 } else { 0.0 };
    let name_width = (ui.available_width() - 8.0 - tail_width).max(40.0);
    let mut columns = vec![
        (ArchiveField::Name, name_width),
        (ArchiveField::Size, 72.0),
        (ArchiveField::Packed, 72.0),
        (ArchiveField::Ratio, 54.0),
    ];
    if show_crc {
        columns.push((ArchiveField::Crc32, 80.0));
    }
    if show_modified {
        columns.push((ArchiveField::Modified, 120.0));
    }
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        for &(field, width) in &columns {
            let label = if order.field == field {
                format!(
                    "{} {}",
                    field.label(),
                    if order.descending { "v" } else { "^" }
                )
            } else {
                field.label().to_string()
            };
            let header = ui.add_enabled(
                field.available(listing.kind),
                egui::Button::new(egui::RichText::new(label).size(10.0).color(t.text_muted))
                    .frame(false)
                    .min_size(egui::vec2(width, 20.0)),
            );
            header.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    header.enabled(),
                    format!("Sort archive by {}", field.label()),
                )
            });
            #[cfg(feature = "visual-qa")]
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::new(("archive-sort-header", field.label())),
                    header.rect,
                )
            });
            if header
                .on_hover_text("Click to sort; click again to reverse")
                .clicked()
            {
                actions.sort_requested = Some(field);
            }
        }
    });
    ui.separator();

    // The outer sheet scrolls controls/details/actions as they grow.
    // Keep member virtualization bounded even in its unbounded content UI.
    #[cfg(feature = "visual-qa")]
    let mut rendered_rows = Vec::new();
    let mut member_scroll = egui::ScrollArea::vertical().id_salt("archive-members");
    if std::mem::take(reset_scroll) {
        member_scroll = member_scroll.vertical_scroll_offset(0.0);
    }
    let member_rows = member_scroll
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .max_height(rows_height)
        .show_rows(ui, 30.0, visible.len(), |ui, rows| {
            // The cursor row may lie outside the rows drawn,
            // so scroll to where it would be.
            if std::mem::take(scroll_to_focused)
                && let Some(row) =
                    focused.and_then(|index| visible.iter().position(|&v| v == index))
            {
                let pitch = 30.0 + ui.spacing().item_spacing.y;
                let top = ui.max_rect().top() + (row as f32 - rows.start as f32) * pitch;
                ui.scroll_to_rect(
                    egui::Rect::from_min_size(
                        egui::pos2(ui.max_rect().left(), top),
                        egui::vec2(1.0, pitch),
                    ),
                    None,
                );
            }
            for row in rows {
                let index = visible[row];
                let synthetic = index >= listing.members.len();
                let member = if synthetic {
                    &listing
                        .tree
                        .as_ref()
                        .expect("folder IDs require tree")
                        .directories[index - listing.members.len()]
                } else {
                    &listing.members[index]
                };
                let selected = state.selected.contains(&index);
                let response = Frame::NONE
                    .fill(if selected {
                        t.bg_selected.linear_multiply(0.55)
                    } else if row % 2 == 0 {
                        t.bg_card.linear_multiply(0.18)
                    } else {
                        Color32::TRANSPARENT
                    })
                    .stroke(if *focused == Some(index) {
                        Stroke::new(1.0_f32, t.accent)
                    } else {
                        Stroke::NONE
                    })
                    .inner_margin(Margin::symmetric(4, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let path = if folder_mode {
                                member
                                    .path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned()
                            } else {
                                member.path.display().to_string()
                            };
                            let path = if synthetic {
                                format!("[Folder] {path}")
                            } else {
                                path
                            };
                            left_cell(
                                ui,
                                name_width,
                                22.0,
                                egui::Label::new(
                                    egui::RichText::new(path).size(11.0).color(t.text_secondary),
                                )
                                .truncate(),
                            );
                            let size = if member.is_dir {
                                String::new()
                            } else {
                                format_size(member.size)
                            };
                            let packed = if member.is_dir {
                                String::new()
                            } else if listing.kind != crate::archive::ArchiveKind::Zip {
                                "n/a".to_string()
                            } else {
                                format_size(member.compressed_size)
                            };
                            let ratio = if member.is_dir || member.compressed_size == 0 {
                                String::new()
                            } else {
                                format!(
                                    "{:.1}x",
                                    member.size as f64 / member.compressed_size as f64
                                )
                            };
                            let modified = crate::search::modified_label(member.modified);
                            let mut values = vec![(size, 72.0), (packed, 72.0), (ratio, 54.0)];
                            if show_crc {
                                values.push((
                                    member
                                        .zip_info
                                        .as_ref()
                                        .and_then(|info| info.crc32)
                                        .map(|crc| format!("{crc:08X}"))
                                        .unwrap_or_else(|| "—".into()),
                                    80.0,
                                ));
                            }
                            if show_modified {
                                values.push((modified, 120.0));
                            }
                            for (value, width) in values {
                                ui.add_sized(
                                    [width, 22.0],
                                    egui::Label::new(
                                        egui::RichText::new(value).size(10.0).color(t.text_muted),
                                    ),
                                );
                            }
                        });
                    })
                    .response
                    .interact(Sense::click());
                #[cfg(feature = "visual-qa")]
                if !synthetic {
                    rendered_rows.push((member.path.clone(), response.rect));
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            egui::Id::new(("archive-member-row", member.path.clone())),
                            response.rect,
                        )
                    });
                }
                response.widget_info(|| {
                    if synthetic {
                        return egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("Open archive folder {}", member.path.display()),
                        );
                    }
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        true,
                        selected,
                        member.path.display().to_string(),
                    )
                });
                if response.clicked() {
                    let previous = *focused;
                    *focused = Some(index);
                    if ui.input(|input| input.modifiers.shift) {
                        let anchor = range_anchor.or(previous).or(Some(index));
                        *range_anchor = anchor;
                        select_member_range(
                            visible,
                            anchor,
                            Some(index),
                            listing.members.len(),
                            &mut state.selected,
                        );
                    } else if synthetic {
                        actions.navigate = Some(Some(member.path.clone()));
                    } else {
                        *range_anchor = Some(index);
                        if !state.selected.remove(&index) {
                            state.selected.insert(index);
                        }
                    }
                }
                if response.double_clicked()
                    && !ui.input(|input| input.modifiers.shift)
                    && member.is_dir
                    && listing.tree.is_ok()
                {
                    actions.navigate = Some(Some(member.path.clone()));
                }
            }
        });
    #[cfg(feature = "visual-qa")]
    ui.ctx().data_mut(|d| {
        d.insert_temp(egui::Id::new("archive-current-member-rows"), rendered_rows);
        d.insert_temp(
            egui::Id::new("archive-member-clip"),
            member_rows.inner_rect.intersect(ui.clip_rect()),
        );
    });
    #[cfg(not(feature = "visual-qa"))]
    let _ = member_rows;
    if visible.is_empty() {
        ui.label(
            egui::RichText::new("No matching members")
                .size(11.0)
                .color(t.text_muted),
        );
    }
    // The member last clicked or moved to, not the lowest
    // selected index.
    if let Some(member) = focused
        .or_else(|| state.selected.iter().next().copied())
        .and_then(|index| listing.members.get(index))
    {
        ui.separator();
        ui.label(
            egui::RichText::new(format!("#{}  |  {}", member.index, member.path.display()))
                .size(10.0)
                .color(t.text_muted),
        );
        let details = egui::CollapsingHeader::new("Member details")
            .id_salt("archive-member-details")
            .show(ui, |ui| {
                #[cfg(feature = "visual-qa")]
                let mut observed = Vec::new();
                egui::Grid::new("archive-member-metadata").num_columns(2).show(ui, |ui| {
                    for (label, value) in member_details(listing.kind, member) {
                        let name = ui.label(label);
                        let text = ui.label(&value);
                        #[cfg(feature = "visual-qa")]
                        observed.push((label.to_owned(), value.clone(), ui.clip_rect().contains_rect(name.rect.shrink(0.5)) && ui.clip_rect().contains_rect(text.rect.shrink(0.5))));
                        #[cfg(not(feature = "visual-qa"))]
                        let _ = (name, text);
                        ui.end_row();
                    }
                });
                #[cfg(feature = "visual-qa")]
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("archive-rendered-details"), (member.path.clone(), observed)));
                ui.small("Declared archive metadata; not verified file contents or extraction permissions.");
            });
        #[cfg(feature = "visual-qa")]
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("archive-details-header"),
                details.header_response.rect,
            )
        });
        #[cfg(not(feature = "visual-qa"))]
        let _ = details;
    }

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!visible_set.is_empty(), egui::Button::new(if folder_mode { "Select visible files" } else { "Select all" }))
            .on_hover_text("Add every visible real member; navigation folders are not extraction entries")
            .clicked()
        {
            state.selected.extend(visible_set.iter().copied());
        }
        for (invert, label) in [(true, "Invert visible"), (false, "Clear visible")] {
            let enabled = if invert { !visible_set.is_empty() } else { visible_set.iter().any(|id| state.selected.contains(id)) };
            if ui.add_enabled(enabled, egui::Button::new(label))
                .on_hover_text(if invert { "Toggle visible real members; keep hidden selections and exclude navigation folders" } else { "Deselect visible real members only; keep hidden selections" })
                .clicked()
            {
                change_visible_selection(visible_set, &mut state.selected, invert);
            }
        }
        let clear = ui.add_enabled(!state.selected.is_empty(), egui::Button::new("Clear")).on_hover_text("Clear the selection");
        #[cfg(feature = "visual-qa")]
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("archive-clear-selection"), clear.rect));
        if clear.clicked() { state.selected.clear(); }
        if listing.kind == crate::archive::ArchiveKind::Zip {
            let repeated = ui.add_enabled(!visible_set.is_empty(), egui::Button::new("Select repeated CRC + size"))
                .on_hover_text("Add visible regular files sharing the same declared CRC-32 and size with another visible entry. Keep hidden selections; ignore unavailable CRCs and navigation folders. Candidates only: this does not verify equal file contents or read payloads.");
            #[cfg(feature = "visual-qa")]
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("archive-repeated-crc"), repeated.rect));
            if repeated.clicked() { state.selected.extend(repeated_crc_rows(listing, visible)); }
        }
        if ui.button("Save selection")
            .on_hover_text("Replace the saved set with all selected archive members, including hidden ones; an empty set can be saved")
            .clicked()
        {
            selection_memory.save(&state.selected, listing.members.len());
        }
        let saved_count = selection_memory.0.as_ref().map_or(0, Vec::len);
        if ui.add_enabled(selection_memory.0.is_some(), egui::Button::new(format!("Restore selection ({saved_count})")))
            .on_hover_text("Replace the current selection with the saved set, including hidden members; no extraction")
            .clicked()
        {
            selection_memory.restore(&mut state.selected, listing.members.len());
            *range_anchor = None;
        }
        let can_match_extension = focused.is_some_and(|id| visible.contains(&id) && listing.members.get(id).is_some_and(|member| member.is_regular && !member.is_dir));
        // A nested enabled UI consumes the remaining row width
        // before the parent can wrap this menu as one button.
        let extension_menu = ui.add_enabled(can_match_extension,
            egui::Button::new("Same extension").wrap_mode(egui::TextWrapMode::Extend));
        egui::Popup::menu(&extension_menu).show(|ui| {
                for (select, label) in [(true, "Select visible matches"), (false, "Deselect visible matches")] {
                    if ui.button(label).on_hover_text("Use the focused regular file's final suffix, ignoring case. Only visible regular files change; hidden selections stay intact.").clicked() {
                        let rows = same_extension_rows(listing, lower_paths.as_deref().unwrap_or(&[]), visible, *focused);
                        for id in rows {
                            if select { state.selected.insert(id); } else { state.selected.remove(&id); }
                        }
                        ui.close();
                    }
                }
        });
        for mode in [SimilarMember::Name, SimilarMember::Folder] {
            if similar_selection_menu(ui, listing, lower_paths.as_deref().unwrap_or(&[]), visible, *focused, &mut state.selected, mode) {
                *range_anchor = None;
            }
        }
    });
    ui.add_space(4.0);
    ui.collapsing("Select by mask", |ui| {
        if selection_mask.ui(
            ui,
            listing,
            lower_paths.as_deref().unwrap_or(&[]),
            visible,
            &mut state.selected,
        ) {
            *range_anchor = None;
        }
    });
    if !state.selected.is_empty() {
        ui.label(selection_stats.get(listing, &state.selected).label())
            .on_hover_text("Declared metadata for all selected real members, including hidden ones. Byte totals cover regular files only; directories do not expand implicitly. Not verified payload sizes or required disk space.");
    }
    if listing.kind == crate::archive::ArchiveKind::Zip
        && listing
            .members
            .iter()
            .any(|member| member.zip_info.as_ref().is_some_and(|info| info.encrypted))
    {
        ui.add_enabled_ui(state.extract.is_none() && state.check.is_none(), |ui| {
            let _ = archive_password_input(ui, &mut state.password);
        });
    }
    let dest_hint = format!("Extract into {}", dest.display());
    ui.horizontal_wrapped(|ui| {
        let n = state.selected.len();
        let hidden = if state.filter.trim().is_empty()
            && !folder_mode
            && !narrowed_view
            && !kind_filtered
            && !path_excluded
        {
            0
        } else {
            state
                .selected
                .iter()
                .filter(|index| !visible_set.contains(*index))
                .count()
        };
        if ui
            .add_enabled(
                n > 0 && state.extract.is_none() && state.check.is_none(),
                super::dialog_button(
                    t,
                    super::ButtonRole::Primary,
                    egui::RichText::new(extract_selected_label(n, hidden)).size(13.0),
                ),
            )
            .on_hover_text(&dest_hint)
            .on_disabled_hover_text(if n == 0 {
                "Click members, or move with Up/Down and press Enter, to \
                 select them"
            } else {
                "An archive operation is running"
            })
            .clicked()
        {
            actions.extract_selected = true;
        }
        let all_hint = if listing.truncated {
            format!(
                "{dest_hint}. Scan limit reached: only the {} listed members \
                 are extracted",
                listing.members.len()
            )
        } else {
            dest_hint.clone()
        };
        let all_button = ui
            .add_enabled(
                !listing.members.is_empty() && state.extract.is_none() && state.check.is_none(),
                super::dialog_button(
                    t,
                    super::ButtonRole::Secondary,
                    egui::RichText::new(extract_all_label(listing)).size(13.0),
                ),
            )
            .on_hover_text(all_hint);
        #[cfg(feature = "visual-qa")]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("archive-extract-all"), all_button.rect)
        });
        if all_button.clicked() {
            actions.extract_all = true;
        }
    });
    actions
}
