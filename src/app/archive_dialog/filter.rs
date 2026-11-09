//! Member filtering: masks, exclusions and saved filters.

use super::*;

pub(super) struct MaskSelection {
    pub(super) query: String,
    pub(super) case_sensitive: bool,
    pub(super) full_path: bool,
    pub(super) applied: Option<(String, bool, bool)>,
    pub(super) pattern: Option<Result<crate::archive_filter::Pattern, String>>,
    pub(super) status: Option<Result<usize, String>>,
}
impl Default for MaskSelection {
    fn default() -> Self {
        Self {
            query: "*".into(),
            case_sensitive: false,
            full_path: false,
            applied: None,
            pattern: None,
            status: None,
        }
    }
}
impl MaskSelection {
    fn pattern(&mut self) -> &Result<crate::archive_filter::Pattern, String> {
        if self.applied.as_ref().is_none_or(|(query, case, path)| {
            query != &self.query || *case != self.case_sensitive || *path != self.full_path
        }) {
            self.pattern = Some(if self.query.is_empty() {
                Err("Enter a mask; * matches every visible real member.".into())
            } else {
                crate::archive_filter::Pattern::compile(
                    &self.query,
                    crate::archive_filter::Options {
                        glob: true,
                        basename_only: !self.full_path,
                        case_sensitive: self.case_sensitive,
                        ..Default::default()
                    },
                )
            });
            self.applied = Some((self.query.clone(), self.case_sensitive, self.full_path));
            self.status = None;
        }
        self.pattern.as_ref().unwrap()
    }

    pub(super) fn apply(
        &mut self,
        listing: &crate::archive::ArchiveListing,
        lower_paths: &[String],
        visible: &[usize],
        selected: &mut std::collections::BTreeSet<usize>,
        select: bool,
    ) -> Result<usize, String> {
        if lower_paths.len() != listing.members.len() {
            return Err("Archive listing changed; retry selection.".into());
        }
        let pattern = self.pattern().as_ref().map_err(Clone::clone)?;
        let mut count = 0;
        for &id in visible {
            let Some(member) = listing.members.get(id) else {
                continue;
            };
            if pattern.matches(&member.path.to_string_lossy(), &lower_paths[id]) {
                if select {
                    selected.insert(id);
                } else {
                    selected.remove(&id);
                }
                count += 1;
            }
        }
        Ok(count)
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        listing: &crate::archive::ArchiveListing,
        lower_paths: &[String],
        visible: &[usize],
        selected: &mut std::collections::BTreeSet<usize>,
    ) -> bool {
        ui.horizontal_wrapped(|ui| {
            ui.label("Mask");
            let input = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id(egui::Id::new("archive-selection-mask-input"))
                    .char_limit(256)
                    .desired_width(220.0),
            );
            input.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::TextEdit,
                    input.enabled(),
                    "Archive selection glob mask",
                )
            });
            ui.checkbox(&mut self.case_sensitive, "Match case");
            ui.checkbox(&mut self.full_path, "Full paths");
        });
        let valid = self.pattern().is_ok();
        let mut applied = false;
        ui.horizontal_wrapped(|ui| {
            for (select, label) in [(true, "Select matching"), (false, "Deselect matching")] {
                let response = ui.add_enabled(valid && !visible.is_empty(), egui::Button::new(label))
                    .on_hover_text("Only visible real archive records change. Hidden selections stay intact; navigation rows are excluded. Directory subtrees do not expand.");
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("archive-selection-mask-action", select)), response.rect));
                if response.clicked() {
                    let result = self.apply(listing, lower_paths, visible, selected, select);
                    applied = result.is_ok();
                    self.status = Some(result);
                }
            }
        });
        if let Some(Err(error)) = &self.pattern {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(result) = &self.status {
            match result {
                Ok(count) => {
                    ui.label(format!("Last action: {count} visible members matched"));
                }
                Err(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
        }
        ui.small("Glob masks: *, ?, classes and {alternatives}; ? matches a UTF-8 byte. Full paths use **/ across folders.");
        applied
    }
}

pub(super) fn matching_modes_ui(
    ui: &mut egui::Ui,
    options: &mut crate::archive_filter::Options,
    enabled: bool,
) {
    for (index, label, mut checked, hint) in [
        (
            0,
            "Regex",
            options.regex,
            "Filter internal paths or filenames with a bounded regular expression. Literal substring is the default.",
        ),
        (
            1,
            "Exact",
            options.exact,
            "Match the complete path/name literally, preserving spaces.",
        ),
        (
            2,
            "Glob",
            options.glob,
            "Match a whole filename/path with *, ?, classes and {alternatives}. In full paths * stays within a component; **/ spans directories. Backslash escapes metacharacters. ? matches one UTF-8 byte.",
        ),
        (
            3,
            "Fuzzy",
            options.fuzzy,
            "Match query characters in order with gaps. Outer spaces are trimmed; inner spaces are literal. Match case, Smart case and Filename only apply; the current sort order is preserved.",
        ),
    ] {
        let response = ui
            .add_enabled(enabled, egui::Checkbox::new(&mut checked, label))
            .on_hover_text(hint);
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("archive-match-mode", label)), response.rect)
        });
        if response.changed() {
            if checked {
                options.regex = false;
                options.exact = false;
                options.glob = false;
                options.fuzzy = false;
            }
            match index {
                0 => options.regex = checked,
                1 => options.exact = checked,
                2 => options.glob = checked,
                _ => options.fuzzy = checked,
            }
        }
    }
}

pub(super) fn hidden_entries_ui(
    ui: &mut egui::Ui,
    kinds: &mut crate::archive_filter::Kinds,
    enabled: bool,
) {
    let response = ui.add_enabled(enabled, egui::Checkbox::new(&mut kinds.hide_dotfiles, "Hide dotfiles"))
        .on_hover_text("Hide real members and navigation folders with any dot-prefixed internal path component, including descendants of hidden folders. Does not inspect host attributes or follow links. Hidden selected members remain selected.");
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new("archive-hide-dotfiles"), response.rect));
    let _ = response;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ExclusionSettings {
    pub(super) query: String,
    pub(super) full_path: bool,
}
impl ExclusionSettings {
    fn compile(&self) -> Result<crate::archive_filter::Pattern, String> {
        crate::archive_filter::Pattern::compile(
            &self.query,
            crate::archive_filter::Options {
                glob: true,
                basename_only: !self.full_path,
                ..Default::default()
            },
        )
    }
}

#[derive(Default)]
pub(super) struct ExclusionGlob {
    pub(super) settings: ExclusionSettings,
    applied: Option<ExclusionSettings>,
    pub(super) pattern: Option<Result<crate::archive_filter::Pattern, String>>,
}
impl ExclusionGlob {
    fn refresh(&mut self) -> bool {
        if self.applied.as_ref() == Some(&self.settings) {
            return false;
        }
        self.pattern = Some(self.settings.compile());
        self.applied = Some(self.settings.clone());
        true
    }
    fn accepts(&self, lower_path: &str) -> bool {
        let Some(Ok(pattern)) = &self.pattern else {
            return false;
        };
        if self.settings.query.is_empty() {
            return true;
        }
        let mut candidate = lower_path.trim_end_matches('/');
        loop {
            if pattern.matches(candidate, candidate) {
                return false;
            }
            let Some((parent, _)) = candidate.rsplit_once('/') else {
                return true;
            };
            candidate = parent;
        }
    }
    pub(super) fn ui(&mut self, ui: &mut egui::Ui, enabled: bool) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Exclude glob");
            let response = ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.settings.query)
                .id(egui::Id::new("archive-exclusion-query")).char_limit(256).desired_width(260.0)
                .hint_text("*.bak or {*.bak,*.tmp}"))
                .on_hover_text("Independent case-insensitive glob exclusion. Filename matching includes each ancestor directory; excluding a directory hides its descendants. Full paths compares archive-relative prefixes. Blank disables it. ? counts UTF-8 comparison bytes; **/ crosses folders.");
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, response.enabled(), "Archive exclusion glob"));
            ui.add_enabled(enabled, egui::Checkbox::new(&mut self.settings.full_path, "Full paths"));
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("archive-exclusion-query-rect"), response.rect));
        });
    }
}

/// Query settings only; never captures a folder or member IDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SavedMemberFilter {
    exclusion: ExclusionSettings,
    extensions_input: String,
    modified_input: String,
    depth_input: String,
    pub(super) query: String,
    size_input: String,
    pub(super) options: crate::archive_filter::Options,
    pub(super) kinds: crate::archive_filter::Kinds,
    exclude_matches: bool,
}

impl SavedMemberFilter {
    pub(super) fn label(&self) -> String {
        let kinds = [
            (self.kinds.files, "Files"),
            (self.kinds.folders, "Folders"),
            (self.kinds.links, "Links"),
            (self.kinds.other, "Other"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect::<Vec<_>>()
        .join(", ");
        let kinds = if self.kinds.hide_dotfiles {
            format!("{kinds} · Hide dotfiles")
        } else {
            kinds
        };
        let kinds = if self.kinds.executable_only {
            format!(
                "{} · Executable files only",
                if kinds.is_empty() { "No types" } else { &kinds }
            )
        } else {
            kinds
        };
        format!(
            "{} · {} · {}{}{}{} · {} · Size: {} · Depth: {} · Modified UTC: {} · Extensions: {} · Exclude glob ({}): {}",
            if self.query.is_empty() {
                "(empty)".into()
            } else {
                self.query.escape_debug().to_string()
            },
            if self.options.basename_only {
                "Filename only"
            } else {
                "Full path"
            },
            if self.options.fuzzy {
                "Fuzzy"
            } else if self.options.glob {
                "Glob"
            } else if self.options.exact {
                "Exact"
            } else if self.options.regex {
                "Regex"
            } else {
                "Literal"
            },
            if self.options.case_sensitive {
                " · Match case"
            } else {
                ""
            },
            if self.options.smart_case {
                " · Smart case"
            } else {
                ""
            },
            if self.exclude_matches {
                " · Exclude matches"
            } else {
                ""
            },
            if kinds.is_empty() { "No types" } else { &kinds },
            if self.size_input.trim().is_empty() {
                "Any"
            } else {
                &self.size_input
            },
            if self.depth_input.trim().is_empty() {
                "Any"
            } else {
                &self.depth_input
            },
            if self.modified_input.trim().is_empty() {
                "Any"
            } else {
                &self.modified_input
            },
            if self.extensions_input.trim().is_empty() {
                "Any"
            } else {
                &self.extensions_input
            },
            if self.exclusion.full_path {
                "Full path"
            } else {
                "Filename"
            },
            if self.exclusion.query.is_empty() {
                "None".into()
            } else {
                self.exclusion.query.escape_debug().to_string()
            }
        )
    }
}

/// Cached filtered rows, cursor and bounded navigation history, owned by one listing.
#[derive(Default)]
pub struct MemberFilter {
    pub(super) exclusion: ExclusionGlob,
    pub(super) extensions_input: String,
    pub(super) applied_extensions_input: Option<String>,
    pub(super) extensions_pattern: Option<Result<crate::archive_filter::ExtensionFilter, String>>,
    pub(super) selection_mask: MaskSelection,
    pub(super) modified_input: String,
    pub(super) applied_modified_input: Option<String>,
    pub(super) modified_pattern: Option<Result<crate::archive_filter::ModifiedFilter, String>>,
    pub(super) depth_input: String,
    pub(super) applied_depth_input: Option<String>,
    pub(super) depth_pattern: Option<Result<crate::archive_filter::DepthFilter, String>>,
    pub(super) size_input: String,
    pub(super) applied_size_input: Option<String>,
    pub(super) size_pattern: Option<Result<crate::archive_filter::SizeFilter, String>>,
    pub(super) saved_filter_search: String,
    pub(super) saved_filters: std::collections::VecDeque<SavedMemberFilter>,
    pub(super) past: std::collections::VecDeque<ArchiveLocation>,
    pub(super) future: std::collections::VecDeque<ArchiveLocation>,
    pub(super) options: crate::archive_filter::Options,
    pub(super) exclude_matches: bool,
    pub(super) applied_exclude_matches: Option<bool>,
    pub(super) kinds: crate::archive_filter::Kinds,
    pub(super) applied_kinds: Option<crate::archive_filter::Kinds>,
    pub(super) applied_options: Option<crate::archive_filter::Options>,
    pub(super) pattern: Option<Result<crate::archive_filter::Pattern, String>>,
    pub(super) lower_paths: Option<Vec<String>>,
    pub(super) folder: Option<PathBuf>,
    pub(super) applied_folder: Option<Option<PathBuf>>,
    pub(super) reset_scroll: bool,
    pub(super) needle: Option<String>,
    // Real IDs index listing.members; navigation IDs index tree.directories
    // after that range and are never admitted to selected/extraction.
    pub(super) visible: Vec<usize>,
    pub(super) visible_set: std::collections::HashSet<usize>,
    pub(crate) order: ArchiveOrder,
    pub(super) applied_order: Option<ArchiveOrder>,
    pub(super) focused: Option<usize>,
    pub(super) range_anchor: Option<usize>,
    pub(super) selection_memory: SelectionMemory,
    pub(super) selection_stats: SelectionStatsCache,
    pub(super) narrowed: Option<std::collections::BTreeSet<usize>>,
    pub(super) folder_input: String,
    pub(super) folder_input_error: Option<String>,
    pub(super) folder_completions: Option<(String, Vec<PathBuf>, bool)>,
    /// The cursor moved by key and its row should scroll into view.
    pub(super) scroll_to_focused: bool,
}

impl MemberFilter {
    fn snapshot(&self, query: &str) -> SavedMemberFilter {
        SavedMemberFilter {
            exclusion: self.exclusion.settings.clone(),
            extensions_input: self.extensions_input.clone(),
            modified_input: self.modified_input.clone(),
            depth_input: self.depth_input.clone(),
            query: query.to_owned(),
            size_input: self.size_input.clone(),
            options: self.options,
            kinds: self.kinds,
            exclude_matches: self.exclude_matches,
        }
    }

    pub(super) fn save_filter(&mut self, query: &str) -> bool {
        if (query.is_empty()
            && self.options == Default::default()
            && self.kinds == Default::default()
            && !self.exclude_matches
            && self.size_input.trim().is_empty()
            && self.depth_input.trim().is_empty()
            && self.modified_input.trim().is_empty()
            && self.extensions_input.trim().is_empty()
            && self.exclusion.settings == ExclusionSettings::default())
            || crate::archive_filter::Pattern::compile(query, self.options).is_err()
            || crate::archive_filter::SizeFilter::compile(&self.size_input).is_err()
            || crate::archive_filter::DepthFilter::compile(&self.depth_input).is_err()
            || crate::archive_filter::ModifiedFilter::compile(&self.modified_input).is_err()
            || crate::archive_filter::ExtensionFilter::compile(&self.extensions_input).is_err()
            || self.exclusion.settings.compile().is_err()
        {
            return false;
        }
        let saved = self.snapshot(query);
        self.saved_filters.retain(|entry| entry != &saved);
        self.saved_filters.push_front(saved);
        while self.saved_filters.len() > 20
            || self
                .saved_filters
                .iter()
                .map(|entry| {
                    entry.query.len()
                        + entry.size_input.len()
                        + entry.depth_input.len()
                        + entry.modified_input.len()
                        + entry.extensions_input.len()
                        + entry.exclusion.query.len()
                })
                .sum::<usize>()
                > 16 * 1024
        {
            self.saved_filters.pop_back();
        }
        true
    }

    pub(super) fn restore_filter(&mut self, saved: SavedMemberFilter, query: &mut String) {
        *query = saved.query;
        self.exclusion.settings = saved.exclusion;
        self.extensions_input = saved.extensions_input;
        self.modified_input = saved.modified_input;
        self.depth_input = saved.depth_input;
        self.size_input = saved.size_input;
        self.options = saved.options;
        self.kinds = saved.kinds;
        self.exclude_matches = saved.exclude_matches;
        self.needle = None;
    }

    pub(super) fn remove_saved_filter(&mut self, saved: &SavedMemberFilter) -> bool {
        let before = self.saved_filters.len();
        self.saved_filters.retain(|entry| entry != saved);
        self.saved_filters.len() != before
    }

    pub(super) fn saved_filters_ui(
        &mut self,
        ui: &mut egui::Ui,
        query: &mut String,
        enabled: bool,
    ) -> bool {
        let valid = self.needle.as_deref() == Some(query.as_str())
            && self.applied_options == Some(self.options)
            && self.pattern.as_ref().is_some_and(Result::is_ok)
            && self.applied_size_input.as_deref() == Some(self.size_input.as_str())
            && self.size_pattern.as_ref().is_some_and(Result::is_ok)
            && self.applied_depth_input.as_deref() == Some(self.depth_input.as_str())
            && self.depth_pattern.as_ref().is_some_and(Result::is_ok)
            && self.applied_modified_input.as_deref() == Some(self.modified_input.as_str())
            && self.modified_pattern.as_ref().is_some_and(Result::is_ok)
            && self.applied_extensions_input.as_deref() == Some(self.extensions_input.as_str())
            && self.extensions_pattern.as_ref().is_some_and(Result::is_ok)
            && self.exclusion.applied.as_ref() == Some(&self.exclusion.settings)
            && self.exclusion.pattern.as_ref().is_some_and(Result::is_ok)
            && (!query.is_empty()
                || self.options != Default::default()
                || self.kinds != Default::default()
                || self.exclude_matches
                || !self.size_input.trim().is_empty()
                || !self.depth_input.trim().is_empty()
                || !self.modified_input.trim().is_empty()
                || !self.extensions_input.trim().is_empty()
                || self.exclusion.settings != ExclusionSettings::default());
        let save = ui.add_enabled(enabled && valid, egui::Button::new("Save filter"))
            .on_hover_text("Save query, filename/path scope, size/depth/dates/extensions, Regex/case/exclusion and type filters for this loaded archive. Folder, narrowing, sort and selections are not saved.");
        if save.clicked() {
            self.save_filter(query);
        }
        let menu = ui.add_enabled(
            enabled && !self.saved_filters.is_empty(),
            egui::Button::new("Saved filters").wrap_mode(egui::TextWrapMode::Extend),
        );
        let mut restore = None;
        let mut remove = None;
        let mut clear = false;
        egui::Popup::menu(&menu).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_max_width(420.0);
            ui.horizontal(|ui| {
                let response = ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.saved_filter_search)
                    .id(egui::Id::new("archive-saved-filter-search")).char_limit(128)
                    .desired_width(280.0).hint_text("Search saved filters"));
                response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, response.enabled(), "Search saved archive filter descriptions"));
                if ui.add_enabled(enabled && !self.saved_filter_search.is_empty(), egui::Button::new("Clear search")).clicked() {
                    self.saved_filter_search.clear();
                }
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("archive-saved-filter-search-rect"), response.rect));
            });
            let search = self.saved_filter_search.trim().to_lowercase();
            let mut matched = 0usize;
            for (index, entry) in self.saved_filters.iter().enumerate() {
                let label = entry.label();
                if !label.to_lowercase().contains(&search) { continue; }
                matched += 1;
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 80.0).clamp(1.0, 340.0);
                    let response = ui.allocate_ui_with_layout(
                        egui::vec2(width, ui.spacing().interact_size.y),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| ui.add_enabled(enabled, egui::Button::new(&label).truncate())
                    ).inner.on_hover_text(format!("{label}\nApply to current folder and narrowing; keep sort and selection."));
                    let action = ui.add_enabled(enabled, egui::Button::new("Remove"))
                        .on_hover_text("Remove only this saved filter; keep the active query and settings.");
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("archive-remove-saved-filter", index)), action.rect));
                    if action.clicked() {
                        remove = Some(entry.clone());
                        ui.close();
                    }
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("archive-saved-filter", index)), response.rect));
                    #[cfg(not(any(test, feature = "visual-qa")))]
                    let _ = index;
                    if response.clicked() {
                        restore = Some(entry.clone());
                        ui.close();
                    }
                });
            }
            ui.label(format!("{matched} of {} saved filters", self.saved_filters.len()));
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("archive-saved-filter-matches"), matched));
            ui.separator();
            if ui
                .add_enabled(enabled, egui::Button::new("Clear all saved filters"))
                .on_hover_text("Remove every saved filter, including entries hidden by this menu search. Keep the active query and settings.")
                .clicked()
            {
                clear = true;
                ui.close();
            }
        });
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("archive-save-filter"), save.rect);
            data.insert_temp(egui::Id::new("archive-saved-filters-menu"), menu.rect);
        });
        if clear && enabled {
            self.saved_filters.clear();
        }
        if let Some(saved) = remove.filter(|_| enabled) {
            self.remove_saved_filter(&saved);
            return false;
        }
        if let Some(saved) = restore.filter(|_| enabled) {
            self.restore_filter(saved, query);
            return true;
        }
        false
    }

    pub(super) fn has_filters(&self, query: &str) -> bool {
        !query.is_empty()
            || self.options != Default::default()
            || self.kinds != Default::default()
            || self.exclude_matches
            || !self.size_input.is_empty()
            || !self.depth_input.is_empty()
            || !self.modified_input.is_empty()
            || !self.extensions_input.is_empty()
            || self.exclusion.settings != ExclusionSettings::default()
            || self.narrowed.is_some()
    }

    pub(super) fn reset_filters(&mut self, query: &mut String) {
        query.clear();
        self.exclusion.settings = ExclusionSettings::default();
        self.extensions_input.clear();
        self.modified_input.clear();
        self.depth_input.clear();
        self.size_input.clear();
        self.options = Default::default();
        self.kinds = Default::default();
        self.exclude_matches = false;
        self.narrowed = None;
        self.needle = None;
        self.applied_folder = None;
    }

    pub(super) fn narrow(
        &mut self,
        selected: &std::collections::BTreeSet<usize>,
        real_count: usize,
    ) {
        let rows: std::collections::BTreeSet<_> = selected
            .iter()
            .copied()
            .filter(|&id| id < real_count)
            .collect();
        if !rows.is_empty() {
            self.navigate(None);
        }
        self.narrowed = (!rows.is_empty()).then_some(rows);
        self.applied_folder = None;
    }

    pub(super) fn unnarrow(&mut self) {
        self.narrowed = None;
        self.applied_folder = None;
    }

    fn location(&self) -> ArchiveLocation {
        ArchiveLocation {
            folder: self.folder.clone(),
            focused: self.focused,
        }
    }
    fn trim_history(&mut self) {
        while self.past.len() + self.future.len() > MAX_ARCHIVE_HISTORY
            || self
                .past
                .iter()
                .chain(self.future.iter())
                .fold(0usize, |bytes, point| bytes.saturating_add(point.bytes()))
                > MAX_ARCHIVE_HISTORY_BYTES
        {
            if self.past.pop_front().is_none() {
                self.future.pop_front();
            }
        }
    }
    pub(crate) fn navigate(&mut self, folder: Option<PathBuf>) {
        if self.folder != folder {
            self.past.push_back(self.location());
            self.future.clear();
            self.trim_history();
            self.folder = folder;
        }
        self.focused = None;
        self.range_anchor = None;
        self.reset_scroll = true;
    }
    pub(super) fn history_step(&mut self, back: bool) -> bool {
        let point = if back {
            self.past.pop_back()
        } else {
            self.future.pop_back()
        };
        let Some(point) = point else {
            return false;
        };
        let current = self.location();
        if back {
            self.future.push_back(current);
        } else {
            self.past.push_back(current);
        }
        self.trim_history();
        self.folder = point.folder;
        self.focused = point.focused;
        self.range_anchor = None;
        self.reset_scroll = true;
        self.scroll_to_focused = self.focused.is_some();
        true
    }

    #[cfg(any(test, feature = "visual-qa"))]
    pub fn row_position(&self, index: usize) -> Option<usize> {
        self.visible.iter().position(|&row| row == index)
    }

    #[cfg(feature = "visual-qa")]
    pub fn applied_order_matches(&self) -> bool {
        self.applied_order == Some(self.order)
    }
    #[cfg(feature = "visual-qa")]
    pub fn current_folder(&self) -> Option<&std::path::Path> {
        self.folder.as_deref()
    }

    #[cfg(feature = "visual-qa")]
    pub fn metadata_filters_are(&self, values: [&str; 4]) -> bool {
        [
            self.size_input.as_str(),
            self.depth_input.as_str(),
            self.modified_input.as_str(),
            self.extensions_input.as_str(),
        ] == values
    }
    #[cfg(feature = "visual-qa")]
    pub fn saved_filter_matches_current(&self, query: &str) -> bool {
        self.saved_filters.len() == 1 && self.saved_filters.front() == Some(&self.snapshot(query))
    }
    #[cfg(feature = "visual-qa")]
    pub fn filters_are_clear(&self, query: &str) -> bool {
        !self.has_filters(query) && self.saved_filters.len() == 1
    }
    #[cfg(feature = "visual-qa")]
    pub fn fuzzy_filter_applied(&self) -> bool {
        self.options.fuzzy && self.applied_options == Some(self.options)
    }
    #[cfg(feature = "visual-qa")]
    pub fn exclusion_glob_is(&self, query: &str) -> bool {
        self.exclusion.settings.query == query
            && self.exclusion.applied.as_ref() == Some(&self.exclusion.settings)
    }
    #[cfg(feature = "visual-qa")]
    pub fn dotfiles_hidden(&self) -> bool {
        self.kinds.hide_dotfiles
    }
    #[cfg(feature = "visual-qa")]
    pub fn first_visible_member(&self) -> Option<usize> {
        self.visible.iter().copied().find(|&index| {
            self.lower_paths
                .as_ref()
                .is_some_and(|paths| index < paths.len())
        })
    }

    pub(super) fn copy_rows(
        &self,
        listing: &crate::archive::ArchiveListing,
        selected: &std::collections::BTreeSet<usize>,
        selected_only: bool,
    ) -> Vec<usize> {
        if !selected_only {
            return self
                .visible
                .iter()
                .copied()
                .filter(|&id| id < listing.members.len())
                .collect();
        }
        let mut rows = selected
            .iter()
            .copied()
            .filter(|&id| id < listing.members.len())
            .collect::<Vec<_>>();
        let lower: std::borrow::Cow<'_, [String]> = self.lower_paths.as_ref().map_or_else(
            || {
                std::borrow::Cow::Owned(
                    listing
                        .members
                        .iter()
                        .map(|m| m.path.to_string_lossy().to_lowercase())
                        .collect(),
                )
            },
            |paths| std::borrow::Cow::Borrowed(paths.as_slice()),
        );
        self.order.apply(listing, &lower, &mut rows);
        rows
    }

    pub(super) fn visible(
        &mut self,
        listing: &crate::archive::ArchiveListing,
        filter: &str,
    ) -> &[usize] {
        let exclusion_changed = self.exclusion.refresh();
        let needle = filter.to_owned();
        let query_changed = self.needle.as_deref() != Some(needle.as_str())
            || self.applied_options != Some(self.options);
        if query_changed {
            self.pattern = Some(crate::archive_filter::Pattern::compile(
                &needle,
                self.options,
            ));
        }
        let pattern = self.pattern.as_ref().unwrap();
        let size_changed = self.applied_size_input.as_deref() != Some(self.size_input.as_str());
        if size_changed {
            self.size_pattern = Some(crate::archive_filter::SizeFilter::compile(&self.size_input));
            self.applied_size_input = Some(self.size_input.clone());
        }
        let size_pattern = self.size_pattern.as_ref().unwrap();
        let depth_changed = self.applied_depth_input.as_deref() != Some(self.depth_input.as_str());
        if depth_changed {
            self.depth_pattern = Some(crate::archive_filter::DepthFilter::compile(
                &self.depth_input,
            ));
            self.applied_depth_input = Some(self.depth_input.clone());
        }
        let depth_pattern = self.depth_pattern.as_ref().unwrap();
        let modified_changed =
            self.applied_modified_input.as_deref() != Some(self.modified_input.as_str());
        if modified_changed {
            self.modified_pattern = Some(crate::archive_filter::ModifiedFilter::compile(
                &self.modified_input,
            ));
            self.applied_modified_input = Some(self.modified_input.clone());
        }
        let modified_pattern = self.modified_pattern.as_ref().unwrap();
        let extensions_changed =
            self.applied_extensions_input.as_deref() != Some(self.extensions_input.as_str());
        if extensions_changed {
            self.extensions_pattern = Some(crate::archive_filter::ExtensionFilter::compile(
                &self.extensions_input,
            ));
            self.applied_extensions_input = Some(self.extensions_input.clone());
        }
        let extensions_pattern = self.extensions_pattern.as_ref().unwrap();
        let changed = query_changed
            || exclusion_changed
            || extensions_changed
            || modified_changed
            || depth_changed
            || size_changed
            || self.applied_folder.as_ref() != Some(&self.folder)
            || self.applied_kinds != Some(self.kinds)
            || self.applied_exclude_matches != Some(self.exclude_matches);
        let lower = self.lower_paths.get_or_insert_with(|| {
            listing
                .members
                .iter()
                .map(|member| member.path.to_string_lossy().to_lowercase())
                .collect()
        });
        if changed {
            self.visible = lower
                .iter()
                .enumerate()
                .filter(|(index, path)| {
                    self.kinds.matches(&listing.members[*index])
                        && self.exclusion.accepts(path)
                        && extensions_pattern
                            .as_ref()
                            .is_ok_and(|extensions| extensions.matches(path))
                        && modified_pattern
                            .as_ref()
                            .is_ok_and(|dates| dates.matches(listing.members[*index].modified))
                        && depth_pattern
                            .as_ref()
                            .is_ok_and(|depth| depth.matches(&listing.members[*index].path))
                        && size_pattern
                            .as_ref()
                            .is_ok_and(|size| size.matches(&listing.members[*index]))
                        && self
                            .narrowed
                            .as_ref()
                            .is_none_or(|rows| rows.contains(index))
                        && pattern.as_ref().is_ok_and(|pattern| {
                            pattern.matches(&listing.members[*index].path.to_string_lossy(), path)
                                != self.exclude_matches
                        })
                        && self.folder.as_ref().is_none_or(|folder| {
                            !listing.members[*index].is_dir
                                && listing.members[*index].path.parent() == Some(folder.as_path())
                        })
                })
                .map(|(index, _)| index)
                .collect();
            self.visible_set = self.visible.iter().copied().collect();
            self.needle = Some(needle.clone());
            self.applied_folder = Some(self.folder.clone());
            self.applied_options = Some(self.options);
            self.applied_kinds = Some(self.kinds);
            self.applied_exclude_matches = Some(self.exclude_matches);
            self.reset_scroll = true;
        }
        if changed || self.applied_order != Some(self.order) {
            // Rebuild on sorting too: synthetic navigation IDs must never enter
            // the extraction member sort or selection.
            self.visible.retain(|&index| index < listing.members.len());
            self.order.apply(listing, lower, &mut self.visible);
            if let (Some(folder), Ok(tree)) = (&self.folder, &listing.tree)
                && self.narrowed.is_none()
            {
                let mut keyed: Vec<_> = tree
                    .children(folder)
                    .iter()
                    .copied()
                    .map(|index| {
                        (
                            index,
                            tree.directories[index]
                                .path
                                .to_string_lossy()
                                .to_lowercase(),
                        )
                    })
                    .filter(|(index, path)| {
                        self.exclusion.accepts(path)
                            && (!self.kinds.hide_dotfiles
                                || !crate::archive_filter::hidden_path(
                                    &tree.directories[*index].path,
                                ))
                            && extensions_pattern.is_ok()
                            && modified_pattern.is_ok()
                            && depth_pattern.is_ok()
                            && size_pattern.is_ok()
                            && pattern.as_ref().is_ok_and(|pattern| {
                                pattern
                                    .matches(&tree.directories[*index].path.to_string_lossy(), path)
                                    != self.exclude_matches
                            })
                    })
                    .collect();
                keyed.sort_by(|(left, a), (right, b)| {
                    let cmp = self.order.compare_names(a, b);
                    let cmp = if self.order.descending && self.order.field == ArchiveField::Name {
                        cmp.reverse()
                    } else {
                        cmp
                    };
                    cmp.then_with(|| {
                        tree.directories[*left]
                            .path
                            .cmp(&tree.directories[*right].path)
                    })
                });
                let mut folders: Vec<_> = keyed
                    .into_iter()
                    .map(|(index, _)| index + listing.members.len())
                    .collect();
                folders.append(&mut self.visible);
                self.visible = folders;
            }
            self.visible_set = self
                .visible
                .iter()
                .copied()
                .filter(|&index| index < listing.members.len())
                .collect();
            if self
                .range_anchor
                .is_some_and(|id| !self.visible.contains(&id))
            {
                self.range_anchor = None;
            }
            if self.focused.is_some_and(|id| !self.visible.contains(&id)) {
                self.focused = None;
                self.scroll_to_focused = false;
            }
            self.applied_order = Some(self.order);
            self.scroll_to_focused |= self.focused.is_some();
        }
        &self.visible
    }
}
