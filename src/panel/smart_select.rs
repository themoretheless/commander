//! Smart selections over the visible entries: junk, largest, hard links, extensions, kinds.

use super::*;

impl PanelState {
    /// Add the well-known clutter files in the filtered view to the selection
    /// (`.DS_Store`, `.localized`, `Thumbs.db`, `desktop.ini`, the custom-icon
    /// `Icon\r`). Folders are never matched. Returns how many were added.
    pub fn select_junk(&mut self) -> usize {
        // Lowercased: case-insensitive volumes carry variants such as
        // `thumbs.db` or `Desktop.ini`. `Icon\r` stays an exact match.
        const JUNK_NAMES_LOWER: &[&str] = &[".ds_store", ".localized", "thumbs.db", "desktop.ini"];
        self.select_filtered_where(|entry| {
            !entry.is_dir
                && (entry.name == "Icon\r" || JUNK_NAMES_LOWER.contains(&entry.name_lower.as_str()))
        })
    }

    /// Select the `n` largest files in the filtered view (folders excluded).
    /// Returns how many entries were newly added to the selection.
    pub fn select_largest(&mut self, n: usize) -> usize {
        self.select_largest_by(n, |entry| (!entry.is_dir).then_some(entry.size))
    }

    pub fn select_largest_allocated(&mut self, n: usize) -> usize {
        self.select_largest_by(n, FileEntry::allocated_bytes)
    }

    fn select_largest_by(&mut self, n: usize, metric: impl Fn(&FileEntry) -> Option<u64>) -> usize {
        if n == 0 {
            return 0;
        }
        // Top-n needs sizes before mutation; keep a bounded heap instead of a
        // full filtered `Vec<&FileEntry>` plus a second path list.
        let mut heap = std::collections::BinaryHeap::<std::cmp::Reverse<(u64, PathBuf)>>::new();
        for entry in self.filtered_iter() {
            let Some(size) = metric(entry) else {
                continue;
            };
            heap.push(std::cmp::Reverse((size, entry.path.clone())));
            if heap.len() > n {
                heap.pop();
            }
        }
        let mut added = 0;
        while let Some(std::cmp::Reverse((_, path))) = heap.pop() {
            added += usize::from(self.selection.insert_selected(path));
        }
        added
    }

    /// Focus the first/last visible non-directory in current display order.
    pub fn jump_to_file_edge(&mut self, last: bool) -> bool {
        let row = {
            let mut rows = self
                .filtered_iter()
                .enumerate()
                .filter(|(_, entry)| !entry.is_dir)
                .map(|(index, _)| index + 1);
            if last { rows.last() } else { rows.next() }
        };
        let Some(row) = row else {
            return false;
        };
        self.set_cursor(row);
        self.set_scroll_to_cursor(true);
        true
    }

    /// Move among visible selected rows without modifying picks or wrapping.
    pub fn jump_to_selected(&mut self, forward: bool) -> bool {
        let row = {
            let cursor = self.cursor();
            let mut rows = self
                .filtered_iter()
                .enumerate()
                .filter(|(_, entry)| self.is_selected(&entry.path))
                .map(|(index, _)| index + 1)
                .filter(|&row| if forward { row > cursor } else { row < cursor });
            if forward { rows.next() } else { rows.last() }
        };
        let Some(row) = row else {
            return false;
        };
        self.set_cursor(row);
        self.set_scroll_to_cursor(true);
        true
    }

    /// Add the cursor's visible hard-link group, including the cursor, only
    /// when another visible path has the same retained regular-file identity.
    pub fn select_cursor_hardlink_group(&mut self) -> usize {
        let Some(cursor) = self.cursor_entry() else {
            return 0;
        };
        let Some(key) = cursor.regular_file_id() else {
            return 0;
        };
        let mut has_peer = false;
        self.visit_filtered(|_, entry| {
            has_peer |= entry.path != cursor.path && entry.regular_file_id() == Some(key);
            !has_peer
        });
        if !has_peer {
            return 0;
        }
        self.select_filtered_where(|entry| entry.regular_file_id() == Some(key))
    }

    /// Select every filtered file sharing the cursor file's extension. Does
    /// nothing if the cursor is on `..`, a folder, or an extension-less file.
    /// Returns how many entries were added.
    pub fn select_same_extension_as_cursor(&mut self) -> usize {
        let ext = match self.cursor_entry() {
            Some(e) if !e.is_dir && !e.extension.is_empty() => e.extension.clone(),
            _ => return 0,
        };
        self.select_filtered_where(|entry| !entry.is_dir && entry.extension == ext)
    }

    /// Change visible files sharing the cursor's final-extension-free name.
    /// Match case-insensitively, like the existing same-extension action.
    pub fn change_same_stem_as_cursor(&mut self, select: bool) -> usize {
        let stem = match self.cursor_entry() {
            Some(entry) if !entry.is_dir => Path::new(&entry.name_lower)
                .file_stem()
                .map(|stem| stem.to_owned()),
            _ => return 0,
        };
        let Some(stem) = stem else {
            return 0;
        };
        let mut changed = 0;
        self.update_filtered_selection(|entry, selection| {
            if !entry.is_dir && Path::new(&entry.name_lower).file_stem() == Some(stem.as_os_str()) {
                changed += usize::from(if select {
                    selection.insert_selected(entry.path.clone())
                } else {
                    selection.remove_selected(&entry.path)
                });
            }
        });
        changed
    }

    /// Add one kind from the current filtered view; existing picks are retained.
    pub fn select_visible_kind(&mut self, folders: bool) -> usize {
        self.select_filtered_where(|entry| entry.is_dir == folders)
    }

    /// Remove only this kind in the filtered view, retaining all other picks.
    pub fn deselect_visible_kind(&mut self, folders: bool) -> usize {
        self.deselect_filtered_where(|entry| entry.is_dir == folders)
    }

    pub fn clear_visible_selection(&mut self) -> usize {
        self.deselect_filtered_where(|_| true)
    }

    fn deselect_filtered_where(&mut self, predicate: impl Fn(&FileEntry) -> bool) -> usize {
        let mut removed = 0;
        self.update_filtered_selection(|entry, selection| {
            if predicate(entry) {
                removed += usize::from(selection.remove_selected(&entry.path));
            }
        });
        removed
    }

    /// Subtract only visible matching files, leaving hidden picks and folders intact.
    pub fn deselect_same_extension_as_cursor(&mut self) -> usize {
        let ext = match self.cursor_entry() {
            Some(entry) if !entry.is_dir && !entry.extension.is_empty() => entry.extension.clone(),
            _ => return 0,
        };
        let mut removed = 0;
        self.update_filtered_selection(|entry, selection| {
            if !entry.is_dir && entry.extension == ext {
                removed += usize::from(selection.remove_selected(&entry.path));
            }
        });
        removed
    }

    /// Symlink selection uses captured lexical kinds, independent of target
    /// type/existence. Hidden picks and nonlinks are never changed.
    pub fn select_visible_symlinks(&mut self) -> usize {
        self.select_filtered_where(FileEntry::is_symlink)
    }
    pub fn deselect_visible_symlinks(&mut self) -> usize {
        self.deselect_filtered_where(FileEntry::is_symlink)
    }
    pub fn invert_visible_symlinks(&mut self) -> usize {
        let mut changed = 0;
        self.update_filtered_selection(|entry, selection| {
            if entry.is_symlink() {
                selection.toggle_selected(entry.path.clone());
                changed += 1;
            }
        });
        changed
    }

    /// Select the zero-byte files in the filtered view (folders excluded).
    /// Returns how many entries were added.
    pub fn select_empty_files(&mut self) -> usize {
        self.select_filtered_where(|entry| !entry.is_dir && entry.size == 0)
    }
}
