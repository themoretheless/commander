//! Handlers behind `Workspace::execute`: one method per multi-step command arm.

use super::*;

impl Workspace {
    pub(super) fn move_active_tab(&mut self, cmd: Command) {
        let index = self.active_tab_index(self.active);
        let target = if cmd == Command::MoveTabLeft {
            index.checked_sub(1)
        } else {
            index.checked_add(1)
        };
        if let Some(target) = target {
            self.move_tab(self.active, index, target);
        }
    }

    pub(super) fn close_tabs_around_active(&mut self, cmd: Command) {
        let anchor = self.active_tab_index(self.active);
        if cmd == Command::CloseTabsLeft {
            self.close_tabs_left(self.active, anchor);
        } else {
            self.close_tab_group(self.active, anchor, cmd == Command::CloseOtherTabs);
        }
    }

    pub(super) fn open_selected_folders_background_with_notice(&mut self) {
        if let Err(message) = self.open_selected_folders_background() {
            self.emit_ui_request(UiRequest::Notice {
                message,
                error: true,
            });
        }
    }

    pub(super) fn open_cursor_folder_in_tab(&mut self, cmd: Command) {
        let path = self
            .active_panel_ref()
            .cursor_entry()
            .filter(|entry| entry.is_dir)
            .map(|entry| entry.path.clone());
        if let Some(path) = path {
            if cmd == Command::OpenFolderBackgroundTab {
                self.new_background_tab(self.active, path);
            } else {
                self.new_tab(self.active, Some(path));
            }
        }
    }

    pub(super) fn request_metadata_editor(&mut self, cmd: Command) {
        match self.active_panel_ref().permission_targets() {
            Ok(targets) if !targets.is_empty() => {
                self.emit_ui_request(if cmd == Command::EditTimestamps {
                    UiRequest::Timestamps(targets)
                } else {
                    UiRequest::Permissions(targets)
                })
            }
            _ => self.emit_ui_request(UiRequest::Notice {
                message: "The selected items changed. Refresh the panel and select them again"
                    .into(),
                error: true,
            }),
        }
    }

    pub(super) fn select_edge_tab(&mut self, cmd: Command) {
        let index = if cmd == Command::FirstTab {
            0
        } else {
            self.tab_count(self.active).saturating_sub(1)
        };
        self.select_tab(self.active, index);
    }

    pub(super) fn focus_side(&mut self, cmd: Command) {
        if self.pending_op.is_none() && !self.has_any_pending_ui_modal() {
            self.active = if cmd == Command::FocusLeftPanel {
                ActivePanel::Left
            } else {
                ActivePanel::Right
            };
        }
    }

    pub(super) fn cursor_up(&mut self) {
        let panel = self.active_panel();
        if panel.cursor() > 0 {
            panel.set_cursor(panel.cursor() - 1);
            panel.set_scroll_to_cursor(true);
        }
    }

    pub(super) fn cursor_down(&mut self) {
        let panel = self.active_panel();
        let max = panel.filtered_count();
        if panel.cursor() < max {
            panel.set_cursor(panel.cursor() + 1);
            panel.set_scroll_to_cursor(true);
        }
    }

    pub(super) fn cursor_page_down(&mut self) {
        let panel = self.active_panel();
        let page = panel.page_rows().max(1);
        let max = panel.filtered_count();
        panel.set_cursor((panel.cursor() + page).min(max));
        panel.set_scroll_to_cursor(true);
    }

    pub(super) fn move_cursor_by(&mut self, delta: i32) {
        let panel = self.active_panel();
        // A vim count can be up to `i32::MAX`: step in `usize`,
        // saturating, so a huge count lands on the last row.
        let max = panel.filtered_count();
        let step = isize::try_from(delta).unwrap_or(isize::MAX);
        panel.set_cursor(panel.cursor().saturating_add_signed(step).min(max));
        panel.set_scroll_to_cursor(true);
    }

    pub(super) fn extend_select_down(&mut self) {
        let panel = self.active_panel();
        panel.select_cursor();
        let max = panel.filtered_count();
        if panel.cursor() < max {
            panel.set_cursor(panel.cursor() + 1);
        }
        panel.select_cursor();
        panel.set_scroll_to_cursor(true);
    }

    pub(super) fn extend_select_up(&mut self) {
        let panel = self.active_panel();
        panel.select_cursor();
        if panel.cursor() > 1 {
            panel.set_cursor(panel.cursor() - 1);
        }
        panel.select_cursor();
        panel.set_scroll_to_cursor(true);
    }

    pub(super) fn activate_cursor(&mut self) {
        // Cursor 0 is the ".." row, real files start at cursor 1.
        if self.active_panel_ref().cursor() == 0 {
            self.active_panel().go_up_preserving_miller();
            self.mirror_nav_locked_up();
        } else if let Some(entry) = {
            let panel = self.active_panel_ref();
            panel.cursor_entry().cloned()
        } {
            if entry.is_dir {
                let name = entry.name.clone();
                if self.active_panel_ref().browser_mode() == crate::panel::BrowserMode::Miller {
                    self.active_panel().miller_navigate(entry.path, None);
                } else {
                    self.active_panel().navigate_to(entry.path);
                }
                self.mirror_nav_locked_enter(&name);
            } else if crate::archive::is_supported(&entry.path) {
                // Interactive browse stays available; trust gates only
                // automatic inspection (search), not explicit open.
                self.emit_ui_request(UiRequest::Archive(entry.path));
            } else {
                self.emit_ui_request(UiRequest::OpenExternal(
                    crate::ports::OpenRequest::OpenPath(entry.path),
                ));
            }
        }
    }

    pub(super) fn jump_to_slot(&mut self, n: u8) {
        if let Some(path) = self.bookmarks.by_slot(n).map(|b| b.path.clone()) {
            if path.is_dir() {
                self.active_panel().navigate_to(path);
            } else {
                self.emit_ui_request(UiRequest::Notice {
                    message: format!("Quick jump {n} is unavailable: {}", path.display()),
                    error: true,
                });
            }
        } else {
            self.emit_ui_request(UiRequest::Notice {
                message: format!(
                    "Quick jump {n} is empty: Cmd+Shift+{n} assigns the current folder"
                ),
                error: false,
            });
        }
    }

    pub(super) fn assign_slot(&mut self, n: u8) {
        let dir = self.active_panel_ref().current_path.clone();
        let previous = self
            .bookmarks
            .by_slot(n)
            .filter(|held| held.path != dir)
            .map(|held| held.name.clone());
        if let Err(message) = self.bookmark_dir(dir.clone()) {
            self.emit_ui_request(UiRequest::Notice {
                message: message.to_string(),
                error: true,
            });
            return;
        }
        self.bookmarks.assign_slot(&dir, n);
        self.save_bookmarks();
        let name = self
            .bookmarks
            .by_path(&dir)
            .map_or_else(|| dir.display().to_string(), |b| b.name.clone());
        self.emit_ui_request(UiRequest::Notice {
            message: assign_slot_message(n, &name, previous.as_deref()),
            error: false,
        });
    }

    pub(super) fn bookmark_selected_folders_with_notice(&mut self) {
        let result = self.bookmark_selected_folders();
        let (message, error) = match result {
            Ok(added) => (
                format!("Added {added} folder(s) to Favorites; existing entries kept"),
                false,
            ),
            Err(message) => (message, true),
        };
        self.emit_ui_request(UiRequest::Notice { message, error });
    }

    pub(super) fn toggle_current_dir_bookmark(&mut self) {
        // Toggle: bookmark the active directory, or un-bookmark it if it
        // is already a favorite. The Favorites rail may be hidden, so
        // say which way it went.
        let dir = self.active_panel_ref().current_path.clone();
        let (message, error) = if self.bookmarks.contains(&dir) {
            self.bookmarks.remove(&dir);
            ("Removed from Favorites", false)
        } else if let Err(error) = self.bookmark_dir(dir) {
            (error, true)
        } else {
            ("Added to Favorites", false)
        };
        if !error {
            self.save_bookmarks();
        }
        self.emit_ui_request(UiRequest::Notice {
            message: message.to_string(),
            error,
        });
    }

    pub(super) fn toggle_select_and_advance(&mut self) {
        let panel = self.active_panel();
        if let Some(path) = panel.cursor_entry().map(|entry| entry.path.clone()) {
            panel.toggle_select(path);
        }
        let max = panel.filtered_count();
        if panel.cursor() < max {
            panel.set_cursor(panel.cursor() + 1);
        }
    }

    pub(super) fn toggle_preview_pane(&mut self) {
        if self.inactive_panel().preview.is_some() {
            self.inactive_panel_mut().preview = None;
        } else {
            let (preview, declined) = {
                let panel = self.active_panel_ref();
                let entry = panel.cursor_entry();
                let preview = entry.and_then(panel::make_preview);
                // Only an image can be declined; a folder has
                // nothing to preview by design.
                let declined = entry
                    .filter(|entry| preview.is_none() && !entry.is_dir)
                    .map(panel::image_preview_unavailable_message);
                (preview, declined)
            };
            if let Some(message) = declined {
                self.emit_ui_request(UiRequest::Notice {
                    message,
                    error: true,
                });
            }
            if preview.is_some() {
                // Opened from the pane that was showing one (after
                // Tab): only one side previews at a time.
                self.active_panel().preview = None;
            }
            self.inactive_panel_mut().preview = preview;
        }
    }

    pub(super) fn open_cursor_folder_in_other_pane(&mut self) {
        if self.pending_op.is_some() {
            return;
        }
        if let Some(target) = self
            .active_panel_ref()
            .cursor_entry()
            .filter(|entry| entry.is_dir)
            .map(|entry| entry.path.clone())
        {
            let other = self.inactive_panel_mut();
            other.preview = None;
            other.navigate_to(target);
        }
    }

    pub(super) fn go_home_or_root(&mut self, cmd: Command) {
        if self.pending_op.is_some() {
            return;
        }
        let target = if cmd == Command::GoHome {
            dirs::home_dir().filter(|path| path.is_absolute())
        } else {
            let path = &self.active_panel_ref().current_path;
            path.is_absolute()
                .then(|| path.ancestors().last().map(Path::to_path_buf))
                .flatten()
        };
        match target {
            Some(target) => {
                let panel = self.active_panel();
                panel.preview = None;
                if panel.current_path != target {
                    panel.navigate_to(target);
                }
            }
            None => self.notice("The requested home or root folder is unavailable".into()),
        }
    }

    pub(super) fn align_panes(&mut self, cmd: Command) {
        if self.pending_op.is_some() || self.deletes.is_active() || self.has_any_pending_ui_modal()
        {
            return;
        }
        let pull = cmd == Command::MatchOtherPane;
        let target = if pull {
            self.inactive_panel().current_path.clone()
        } else {
            self.active_panel_ref().current_path.clone()
        };
        let destination = if pull {
            self.active_panel()
        } else {
            self.inactive_panel_mut()
        };
        if destination.current_path != target {
            destination.preview = None;
            destination.navigate_to(target);
        }
    }

    pub(super) fn find_cursor_hardlinks(&mut self) {
        let panel = self.active_panel_ref();
        if let Some((device, inode)) = panel
            .cursor_entry()
            .and_then(crate::panel::FileEntry::regular_file_id)
        {
            self.emit_ui_request(UiRequest::FindIdentity {
                root: panel.current_path.clone(),
                device,
                inode,
            });
        }
    }

    pub(super) fn copy_file_details(&mut self) {
        let entries = self
            .active_panel_ref()
            .selected_or_cursor()
            .unwrap_or_default();
        if !entries.is_empty() {
            match crate::clipboard::file_details(entries.iter().map(|entry| {
                [
                    entry.name.as_str(),
                    entry.size_str.as_str(),
                    entry.modified_str.as_str(),
                ]
            })) {
                Ok(text) => self.emit_ui_request(UiRequest::CopyText {
                    text,
                    label: format!("file details for {} item(s)", entries.len()),
                }),
                Err(message) => self.emit_ui_request(UiRequest::Notice {
                    message,
                    error: true,
                }),
            }
        }
    }

    pub(super) fn copy_current_folder(&mut self, cmd: Command) {
        let style = if cmd == Command::CopyFolderUrl {
            crate::clipboard::PathStyle::FileUrl
        } else {
            crate::clipboard::PathStyle::FullPath
        };
        let text = crate::clipboard::format(
            std::slice::from_ref(&self.active_panel_ref().current_path),
            style,
            None,
        );
        let label = if cmd == Command::CopyFolderUrl {
            "current folder URL"
        } else {
            "current folder path"
        }
        .to_string();
        self.emit_ui_request(UiRequest::CopyText { text, label });
    }

    pub(super) fn sort_active_panel(&mut self, cmd: Command) {
        let (column, label) = match cmd {
            Command::SortByStem => (crate::panel::SortColumn::Stem, "name without extension"),
            Command::SortByName => (crate::panel::SortColumn::Name, "name"),
            Command::SortByHardLinks => (crate::panel::SortColumn::HardLinks, "hard-link count"),
            Command::SortByInode => (crate::panel::SortColumn::Inode, "inode"),
            Command::SortByAllocated => (crate::panel::SortColumn::Allocated, "allocated size"),
            Command::SortBySize => (crate::panel::SortColumn::Size, "size"),
            Command::SortByAccessed => (crate::panel::SortColumn::Accessed, "accessed"),
            Command::SortByChanged => (crate::panel::SortColumn::Changed, "metadata changed"),
            Command::SortByCreated => (crate::panel::SortColumn::Created, "created"),
            Command::SortByModified => (crate::panel::SortColumn::Modified, "modified"),
            Command::SortByExtension => (crate::panel::SortColumn::Extension, "extension"),
            _ => (crate::panel::SortColumn::Kind, "kind"),
        };
        let panel = self.active_panel();
        panel.set_sort(column);
        let order = panel.view_config().sort_order();
        self.notice(sort_notice(label, order));
    }

    pub(super) fn select_junk_with_notice(&mut self) {
        let added = self.active_panel().select_junk();
        self.notice(smart_select_message(
            added,
            ("clutter file", "clutter files"),
            "No clutter files to select here",
        ));
    }

    pub(super) fn select_cursor_hardlinks_with_notice(&mut self) {
        let added = self.active_panel().select_cursor_hardlink_group();
        self.notice(smart_select_message(
            added,
            ("hard-link group file", "hard-link group files"),
            "No more visible hard-link aliases to select",
        ));
    }

    pub(super) fn select_largest_allocated_with_notice(&mut self) {
        let added = self.active_panel().select_largest_allocated(10);
        self.notice(smart_select_message(
            added,
            ("file by allocated size", "files by allocated size"),
            "No more files to select by allocated size",
        ));
    }

    pub(super) fn select_largest_with_notice(&mut self) {
        let added = self.active_panel().select_largest(10);
        self.notice(smart_select_message(
            added,
            ("large file", "of the largest files"),
            "No more large files to select here",
        ));
    }

    pub(super) fn select_like_cursor(&mut self) {
        let ext = self
            .active_panel_ref()
            .cursor_entry()
            .filter(|entry| !entry.is_dir && !entry.extension.is_empty())
            .map(|entry| entry.extension.clone());
        let message = match ext {
            Some(ext) => {
                let added = self.active_panel().select_same_extension_as_cursor();
                let (one, many) = (format!(".{ext} file"), format!(".{ext} files"));
                smart_select_message(
                    added,
                    (&one, &many),
                    &format!("No more .{ext} files to select here"),
                )
            }
            None => "Highlight a file with an extension first".to_string(),
        };
        self.notice(message);
    }

    pub(super) fn change_same_stem_selection(&mut self, cmd: Command) {
        let select = matches!(cmd, Command::SelectSameStem);
        let changed = self.active_panel().change_same_stem_as_cursor(select);
        self.notice(format!(
            "{} {changed} visible file(s) with the cursor's name, ignoring extension",
            if select { "Selected" } else { "Deselected" }
        ));
    }

    pub(super) fn invert_visible_kind(&mut self, cmd: Command) {
        let folders = cmd == Command::InvertVisibleFolders;
        let changed = self.active_panel().invert_visible_kind(folders);
        self.notice(format!(
            "Inverted selection for {changed} visible {}",
            if folders { "folder(s)" } else { "file(s)" }
        ));
    }

    pub(super) fn deselect_visible_kind(&mut self, cmd: Command) {
        let folders = cmd == Command::DeselectVisibleFolders;
        let removed = self.active_panel().deselect_visible_kind(folders);
        self.notice(format!(
            "Removed {removed} visible {} from selection",
            if folders { "folder(s)" } else { "file(s)" }
        ));
    }

    pub(super) fn select_visible_kind(&mut self, cmd: Command) {
        let folders = cmd == Command::SelectVisibleFolders;
        let added = self.active_panel().select_visible_kind(folders);
        self.notice(format!(
            "Added {added} visible {} to selection",
            if folders { "folder(s)" } else { "file(s)" }
        ));
    }

    pub(super) fn select_visible_symlinks_with_notice(&mut self) {
        let count = self.active_panel().select_visible_symlinks();
        self.notice(format!(
            "Added {} to selection",
            count_noun(count, "visible symbolic link", "visible symbolic links")
        ));
    }

    pub(super) fn deselect_visible_symlinks_with_notice(&mut self) {
        let count = self.active_panel().deselect_visible_symlinks();
        self.notice(format!(
            "Removed {} from selection",
            count_noun(count, "visible symbolic link", "visible symbolic links")
        ));
    }

    pub(super) fn invert_visible_symlinks_with_notice(&mut self) {
        let count = self.active_panel().invert_visible_symlinks();
        self.notice(format!(
            "Inverted selection for {}",
            count_noun(count, "visible symbolic link", "visible symbolic links")
        ));
    }

    pub(super) fn select_empty_files_with_notice(&mut self) {
        let added = self.active_panel().select_empty_files();
        self.notice(smart_select_message(
            added,
            ("empty file", "empty files"),
            "No empty files to select here",
        ));
    }

    pub(super) fn copy_listing_json(&mut self) {
        match crate::listing_report::json(self.active_panel_ref()) {
            Ok(text) => self.emit_ui_request(UiRequest::CopyText {
                text,
                label: "listing as JSON".into(),
            }),
            Err(error) => self.notice(error),
        }
    }

    pub(super) fn export_favorites(&mut self, cmd: Command) {
        let result = if matches!(cmd, Command::ExportFavoritesXbel) {
            crate::bookmark_exchange::export_xbel(&self.bookmarks)
        } else {
            crate::bookmark_exchange::export(&self.bookmarks)
        };
        match result {
            Ok(text) => self.emit_ui_request(UiRequest::CopyText {
                text,
                label: if matches!(cmd, Command::ExportFavoritesXbel) {
                    "XBEL bookmarks"
                } else {
                    "GTK bookmarks"
                }
                .into(),
            }),
            Err(message) => self.emit_ui_request(UiRequest::Notice {
                message,
                error: true,
            }),
        }
    }

    pub(super) fn verify_checksum_list_at_cursor(&mut self) {
        if let Some(path) = self
            .active_panel_ref()
            .cursor_entry()
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.path.clone())
        {
            self.emit_ui_request(UiRequest::VerifyChecksumList(path));
        }
    }

    pub(super) fn view_hex_at_cursor(&mut self) {
        if let Some(entry) = self
            .active_panel_ref()
            .cursor_entry()
            .filter(|entry| !entry.is_dir && !entry.is_symlink())
        {
            self.emit_ui_request(UiRequest::HexPreview(entry.path.clone()));
        } else {
            self.notice("Hex preview requires a regular file under the cursor".into());
        }
    }

    pub(super) fn toggle_nav_lock(&mut self) {
        self.nav_lock = !self.nav_lock;
        // Back, Forward and quick jumps keep each pane's own history,
        // so say exactly which moves the other pane follows.
        self.notice(if self.nav_lock {
            "Navigation lock on: the other pane follows Enter and Go Up".to_string()
        } else {
            "Navigation lock off".to_string()
        });
    }

    pub(super) fn browse_archive_at_cursor(&mut self) {
        // The `..` row (or an empty folder) has no entry: it is not
        // an archive either, so it gets the same hint.
        match self.active_panel_ref().cursor_entry().cloned() {
            Some(entry) if crate::archive::is_supported(&entry.path) => {
                self.emit_ui_request(UiRequest::Archive(entry.path));
            }
            _ => self.emit_ui_request(UiRequest::Notice {
                message: "Highlight a ZIP or supported TAR archive first".to_string(),
                error: true,
            }),
        }
    }
}
