//! Mask and name-list selection with their match counts.

use super::*;

impl PanelState {
    /// Entries to export/copy as a listing: the current selection if anything
    /// is selected, otherwise the whole filtered view (in display order).
    pub fn listing_entries(&self) -> Vec<&FileEntry> {
        if self.selection.selected().is_empty() {
            self.filtered_entries()
        } else {
            self.visible_selected().collect()
        }
    }

    /// Apply a select-by-mask line to the selection over the filtered view:
    /// add terms select matching entries, `!`/`-` terms deselect them
    /// (subtraction wins per entry). Returns how many entries were added.
    pub fn select_by_mask(&mut self, mask: &str) -> usize {
        self.select_by_mask_target(mask, SelectionTarget::Both)
    }

    pub fn select_by_mask_target(&mut self, mask: &str, target: SelectionTarget) -> usize {
        self.select_by_mask_options(mask, target, false)
    }

    pub fn select_by_mask_options(
        &mut self,
        mask: &str,
        target: SelectionTarget,
        case_sensitive: bool,
    ) -> usize {
        self.select_by_mask_folding(mask, target, case_sensitive, false)
    }

    pub fn select_by_mask_folding(
        &mut self,
        mask: &str,
        target: SelectionTarget,
        case_sensitive: bool,
        ignore_accents: bool,
    ) -> usize {
        self.select_by_mask_compatibility(mask, target, case_sensitive, ignore_accents, false)
    }

    pub fn select_by_mask_compatibility(
        &mut self,
        mask: &str,
        target: SelectionTarget,
        case_sensitive: bool,
        ignore_accents: bool,
        extensionless_star: bool,
    ) -> usize {
        let terms = if ignore_accents {
            parse_mask_folding(mask, case_sensitive, true)
        } else if case_sensitive {
            parse_mask_case(mask, true)
        } else {
            parse_mask(mask)
        };
        if terms.is_empty() {
            return 0;
        }
        let mut added = 0;
        self.update_filtered_selection(|entry, selection| {
            if !target.accepts(entry.is_dir) {
                return;
            }
            match mask_verdict(
                &terms,
                entry,
                case_sensitive,
                ignore_accents,
                extensionless_star,
            ) {
                Some(false) => {
                    selection.remove_selected(&entry.path);
                }
                Some(true) => added += usize::from(selection.insert_selected(entry.path.clone())),
                None => {}
            }
        });
        added
    }

    fn mask_action_paths(
        &self,
        mask: &str,
        options: SelectionMaskOptions,
        action: crate::name_list::Action,
    ) -> Vec<PathBuf> {
        let terms = if options.ignore_accents {
            parse_mask_folding(mask, options.case_sensitive, true)
        } else if options.case_sensitive {
            parse_mask_case(mask, true)
        } else {
            parse_mask(mask)
        };
        self.filtered_iter()
            .filter(|entry| {
                options.target.accepts(entry.is_dir)
                    && action.changes(self.is_selected(&entry.path))
                    && mask_verdict(
                        &terms,
                        entry,
                        options.case_sensitive,
                        options.ignore_accents,
                        options.extensionless_star,
                    ) == Some(true)
            })
            .map(|entry| entry.path.clone())
            .collect()
    }

    pub fn mask_action_count(
        &self,
        mask: &str,
        options: SelectionMaskOptions,
        action: crate::name_list::Action,
    ) -> usize {
        if action == crate::name_list::Action::Add {
            return self.mask_match_count_compatibility(
                mask,
                options.target,
                options.case_sensitive,
                options.ignore_accents,
                options.extensionless_star,
            );
        }
        self.mask_action_paths(mask, options, action).len()
    }

    pub fn apply_mask_action(
        &mut self,
        mask: &str,
        options: SelectionMaskOptions,
        action: crate::name_list::Action,
    ) -> usize {
        if action == crate::name_list::Action::Add {
            return self.select_by_mask_compatibility(
                mask,
                options.target,
                options.case_sensitive,
                options.ignore_accents,
                options.extensionless_star,
            );
        }
        let paths = self.mask_action_paths(mask, options, action);
        let changed = paths.len();
        for path in paths {
            match action {
                crate::name_list::Action::Remove => {
                    self.selection.remove_selected(&path);
                }
                crate::name_list::Action::Invert => self.selection.toggle_selected(path),
                crate::name_list::Action::Add => unreachable!(),
            }
        }
        changed
    }

    /// Add exact UTF-8 basenames from a pasted list, over the current view only.
    pub fn select_by_names(&mut self, text: &str) -> Result<usize, String> {
        self.select_by_names_target(text, SelectionTarget::Both)
    }

    pub fn select_by_names_target(
        &mut self,
        text: &str,
        target: SelectionTarget,
    ) -> Result<usize, String> {
        self.apply_names(text, target, crate::name_list::Action::Add)
    }

    pub fn apply_names(
        &mut self,
        text: &str,
        target: SelectionTarget,
        action: crate::name_list::Action,
    ) -> Result<usize, String> {
        let names = crate::name_list::parse(text)?;
        let mut changed = 0;
        self.update_filtered_selection(|entry, selection| {
            if target.accepts(entry.is_dir)
                && entry
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| names.contains(n))
            {
                match action {
                    crate::name_list::Action::Add => {
                        changed += usize::from(selection.insert_selected(entry.path.clone()))
                    }
                    crate::name_list::Action::Remove => {
                        changed += usize::from(selection.remove_selected(&entry.path))
                    }
                    crate::name_list::Action::Invert => {
                        selection.toggle_selected(entry.path.clone());
                        changed += 1;
                    }
                }
            }
        });
        Ok(changed)
    }

    pub fn selection_revision(&self) -> u64 {
        self.selection.revision()
    }

    pub fn names_match_count(&self, text: &str) -> Result<usize, String> {
        self.names_match_count_target(text, SelectionTarget::Both)
    }

    pub fn names_match_count_target(
        &self,
        text: &str,
        target: SelectionTarget,
    ) -> Result<usize, String> {
        self.names_action_count(text, target, crate::name_list::Action::Add)
    }

    pub fn names_action_count(
        &self,
        text: &str,
        target: SelectionTarget,
        action: crate::name_list::Action,
    ) -> Result<usize, String> {
        let names = crate::name_list::parse(text)?;
        Ok(self
            .filtered_iter()
            .filter(|entry| {
                target.accepts(entry.is_dir)
                    && action.changes(self.is_selected(&entry.path))
                    && entry
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| names.contains(name))
            })
            .count())
    }

    /// How many filtered entries would change selection state if `mask` were
    /// applied. A subtraction-only mask therefore reports zero until it has a
    /// selected match, which keeps the preview aligned with the actual action.
    pub fn mask_match_count(&self, mask: &str) -> usize {
        self.mask_match_count_target(mask, SelectionTarget::Both)
    }

    pub fn mask_match_count_target(&self, mask: &str, target: SelectionTarget) -> usize {
        self.mask_match_count_options(mask, target, false)
    }

    pub fn mask_match_count_options(
        &self,
        mask: &str,
        target: SelectionTarget,
        case_sensitive: bool,
    ) -> usize {
        self.mask_match_count_folding(mask, target, case_sensitive, false)
    }

    pub fn mask_match_count_folding(
        &self,
        mask: &str,
        target: SelectionTarget,
        case_sensitive: bool,
        ignore_accents: bool,
    ) -> usize {
        self.mask_match_count_compatibility(mask, target, case_sensitive, ignore_accents, false)
    }

    pub fn mask_match_count_compatibility(
        &self,
        mask: &str,
        target: SelectionTarget,
        case_sensitive: bool,
        ignore_accents: bool,
        extensionless_star: bool,
    ) -> usize {
        let terms = if ignore_accents {
            parse_mask_folding(mask, case_sensitive, true)
        } else if case_sensitive {
            parse_mask_case(mask, true)
        } else {
            parse_mask(mask)
        };
        if terms.is_empty() {
            return 0;
        }
        let mut changes = 0;
        self.visit_filtered(|_, entry| {
            if !target.accepts(entry.is_dir) {
                return true;
            }
            let selected = self.selection.selected().contains(&entry.path);
            let next = mask_verdict(
                &terms,
                entry,
                case_sensitive,
                ignore_accents,
                extensionless_star,
            )
            .unwrap_or(selected);
            changes += usize::from(next != selected);
            true
        });
        changes
    }
}
