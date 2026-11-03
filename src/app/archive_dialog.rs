//! Read-only, bounded ZIP/TAR browser. Archive parsing stays on a worker thread.

use super::*;
use crate::archive_order::{Field as ArchiveField, Order as ArchiveOrder};

const CONTENT_GROUPS_PER_PAGE: usize = 100;

const CONTENT_MEMBERS_PER_PAGE: usize = 10;

/// Clamp stale pages before multiplying, including empty and extreme inputs.
fn bounded_page_range(count: usize, page: &mut usize, size: usize) -> std::ops::Range<usize> {
    *page = (*page).min(count.saturating_sub(1) / size);
    let start = *page * size;
    start..start.saturating_add(size).min(count)
}

#[cfg(test)]
fn content_group_page_range(count: usize, page: &mut usize) -> std::ops::Range<usize> {
    bounded_page_range(count, page, CONTENT_GROUPS_PER_PAGE)
}

fn content_pager(
    ui: &mut egui::Ui,
    count: usize,
    page: &mut usize,
    size: usize,
    labels: [&str; 3],
) -> std::ops::Range<usize> {
    let mut range = bounded_page_range(count, page, size);
    if count > size {
        ui.horizontal_wrapped(|ui| {
            for (forward, label, enabled) in [
                (false, labels[0], range.start > 0),
                (true, labels[1], range.end < count),
            ] {
                let button = ui.add_enabled(enabled, egui::Button::new(label));
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new(label), button.rect);
                    d.insert_temp(ui.make_persistent_id(label), button.rect);
                });
                if button.clicked() {
                    if forward {
                        *page += 1;
                    } else {
                        *page -= 1;
                    }
                }
            }
            range = bounded_page_range(count, page, size);
            ui.label(format!(
                "{} {}–{} of {}",
                labels[2],
                range.start + 1,
                range.end,
                count
            ));
        });
    }
    range
}

fn content_group_pager(
    ui: &mut egui::Ui,
    count: usize,
    page: &mut usize,
) -> std::ops::Range<usize> {
    content_pager(
        ui,
        count,
        page,
        CONTENT_GROUPS_PER_PAGE,
        ["Previous groups", "Next groups", "Groups"],
    )
}

const MAX_ARCHIVE_HISTORY: usize = 64;
const MAX_ARCHIVE_HISTORY_BYTES: usize = 256 * 1024;
#[derive(Clone)]
struct ArchiveLocation {
    folder: Option<PathBuf>,
    focused: Option<usize>,
}
impl ArchiveLocation {
    fn bytes(&self) -> usize {
        self.folder
            .as_ref()
            .map_or(0, |path| path.as_os_str().as_encoded_bytes().len())
    }
}

/// One explicit snapshot, owned by the immutable archive listing.
#[derive(Default)]
struct SelectionMemory(Option<Vec<usize>>);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SelectionStats {
    files: usize,
    folders: usize,
    links: usize,
    other: usize,
    unpacked: u128,
    packed: Option<u128>,
}
impl SelectionStats {
    fn compute(
        listing: &crate::archive::ArchiveListing,
        ids: &std::collections::BTreeSet<usize>,
    ) -> Self {
        let mut stats = Self {
            packed: (listing.kind == crate::archive::ArchiveKind::Zip).then_some(0),
            ..Default::default()
        };
        for member in ids.iter().filter_map(|&id| listing.members.get(id)) {
            if member.is_symlink {
                stats.links += 1;
            } else if member.is_dir {
                stats.folders += 1;
            } else if member.is_regular {
                stats.files += 1;
                stats.unpacked += u128::from(member.size);
                if let Some(packed) = &mut stats.packed {
                    *packed += u128::from(member.compressed_size);
                }
            } else {
                stats.other += 1;
            }
        }
        stats
    }
    fn label(&self) -> String {
        let packed = self.packed.map_or_else(
            || "packed size unavailable".to_owned(),
            |bytes| format!("{bytes} packed bytes"),
        );
        format!(
            "Selected: {} files · {} directories · {} links · {} other | {} unpacked bytes · {packed}",
            self.files, self.folders, self.links, self.other, self.unpacked
        )
    }
}

#[derive(Default)]
struct SelectionStatsCache {
    ids: Vec<usize>,
    stats: SelectionStats,
    initialized: bool,
}
impl SelectionStatsCache {
    fn get(
        &mut self,
        listing: &crate::archive::ArchiveListing,
        selected: &std::collections::BTreeSet<usize>,
    ) -> &SelectionStats {
        if !self.initialized || !self.ids.iter().eq(selected.iter()) {
            self.ids = selected.iter().copied().collect();
            self.stats = SelectionStats::compute(listing, selected);
            self.initialized = true;
        }
        &self.stats
    }
}

impl SelectionMemory {
    fn save(&mut self, selected: &std::collections::BTreeSet<usize>, real_count: usize) {
        self.0 = Some(
            selected
                .iter()
                .copied()
                .filter(|&id| id < real_count)
                .collect(),
        );
    }

    fn restore(&self, selected: &mut std::collections::BTreeSet<usize>, real_count: usize) {
        if let Some(saved) = &self.0 {
            selected.clear();
            selected.extend(saved.iter().copied().filter(|&id| id < real_count));
        }
    }
}

struct MaskSelection {
    query: String,
    case_sensitive: bool,
    full_path: bool,
    applied: Option<(String, bool, bool)>,
    pattern: Option<Result<crate::archive_filter::Pattern, String>>,
    status: Option<Result<usize, String>>,
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

    fn apply(
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

    fn ui(
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

fn matching_modes_ui(
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

fn hidden_entries_ui(ui: &mut egui::Ui, kinds: &mut crate::archive_filter::Kinds, enabled: bool) {
    let response = ui.add_enabled(enabled, egui::Checkbox::new(&mut kinds.hide_dotfiles, "Hide dotfiles"))
        .on_hover_text("Hide real members and navigation folders with any dot-prefixed internal path component, including descendants of hidden folders. Does not inspect host attributes or follow links. Hidden selected members remain selected.");
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new("archive-hide-dotfiles"), response.rect));
    let _ = response;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ExclusionSettings {
    query: String,
    full_path: bool,
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
struct ExclusionGlob {
    settings: ExclusionSettings,
    applied: Option<ExclusionSettings>,
    pattern: Option<Result<crate::archive_filter::Pattern, String>>,
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
    fn ui(&mut self, ui: &mut egui::Ui, enabled: bool) {
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
struct SavedMemberFilter {
    exclusion: ExclusionSettings,
    extensions_input: String,
    modified_input: String,
    depth_input: String,
    query: String,
    size_input: String,
    options: crate::archive_filter::Options,
    kinds: crate::archive_filter::Kinds,
    exclude_matches: bool,
}

impl SavedMemberFilter {
    fn label(&self) -> String {
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
    exclusion: ExclusionGlob,
    extensions_input: String,
    applied_extensions_input: Option<String>,
    extensions_pattern: Option<Result<crate::archive_filter::ExtensionFilter, String>>,
    selection_mask: MaskSelection,
    modified_input: String,
    applied_modified_input: Option<String>,
    modified_pattern: Option<Result<crate::archive_filter::ModifiedFilter, String>>,
    depth_input: String,
    applied_depth_input: Option<String>,
    depth_pattern: Option<Result<crate::archive_filter::DepthFilter, String>>,
    size_input: String,
    applied_size_input: Option<String>,
    size_pattern: Option<Result<crate::archive_filter::SizeFilter, String>>,
    saved_filter_search: String,
    saved_filters: std::collections::VecDeque<SavedMemberFilter>,
    past: std::collections::VecDeque<ArchiveLocation>,
    future: std::collections::VecDeque<ArchiveLocation>,
    options: crate::archive_filter::Options,
    exclude_matches: bool,
    applied_exclude_matches: Option<bool>,
    kinds: crate::archive_filter::Kinds,
    applied_kinds: Option<crate::archive_filter::Kinds>,
    applied_options: Option<crate::archive_filter::Options>,
    pattern: Option<Result<crate::archive_filter::Pattern, String>>,
    lower_paths: Option<Vec<String>>,
    folder: Option<PathBuf>,
    applied_folder: Option<Option<PathBuf>>,
    reset_scroll: bool,
    needle: Option<String>,
    // Real IDs index listing.members; navigation IDs index tree.directories
    // after that range and are never admitted to selected/extraction.
    visible: Vec<usize>,
    visible_set: std::collections::HashSet<usize>,
    pub(crate) order: ArchiveOrder,
    applied_order: Option<ArchiveOrder>,
    focused: Option<usize>,
    range_anchor: Option<usize>,
    selection_memory: SelectionMemory,
    selection_stats: SelectionStatsCache,
    narrowed: Option<std::collections::BTreeSet<usize>>,
    folder_input: String,
    folder_input_error: Option<String>,
    folder_completions: Option<(String, Vec<PathBuf>, bool)>,
    /// The cursor moved by key and its row should scroll into view.
    scroll_to_focused: bool,
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

    fn save_filter(&mut self, query: &str) -> bool {
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

    fn restore_filter(&mut self, saved: SavedMemberFilter, query: &mut String) {
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

    fn remove_saved_filter(&mut self, saved: &SavedMemberFilter) -> bool {
        let before = self.saved_filters.len();
        self.saved_filters.retain(|entry| entry != saved);
        self.saved_filters.len() != before
    }

    fn saved_filters_ui(&mut self, ui: &mut egui::Ui, query: &mut String, enabled: bool) -> bool {
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

    fn has_filters(&self, query: &str) -> bool {
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

    fn reset_filters(&mut self, query: &mut String) {
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

    fn narrow(&mut self, selected: &std::collections::BTreeSet<usize>, real_count: usize) {
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

    fn unnarrow(&mut self) {
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
    fn history_step(&mut self, back: bool) -> bool {
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

    fn copy_rows(
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

    fn visible(&mut self, listing: &crate::archive::ArchiveListing, filter: &str) -> &[usize] {
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

fn select_folder_contents(
    listing: &crate::archive::ArchiveListing,
    folder: &std::path::Path,
    selected: &mut std::collections::BTreeSet<usize>,
) {
    selected.extend(
        listing
            .members
            .iter()
            .enumerate()
            .filter(|(_, member)| member.path.starts_with(folder))
            .map(|(index, _)| index),
    );
}

/// Add a displayed range without admitting synthetic folders or hidden members.
fn select_member_range(
    visible: &[usize],
    anchor: Option<usize>,
    target: Option<usize>,
    real_count: usize,
    selected: &mut std::collections::BTreeSet<usize>,
) {
    let Some(end) = target.and_then(|id| visible.iter().position(|&row| row == id)) else {
        return;
    };
    let start = anchor
        .and_then(|id| visible.iter().position(|&row| row == id))
        .unwrap_or(end);
    selected.extend(
        visible[start.min(end)..=start.max(end)]
            .iter()
            .copied()
            .filter(|&id| id < real_count),
    );
}

/// Change only the real members admitted by the current folder and filter.
fn change_visible_selection(
    visible: &std::collections::HashSet<usize>,
    selected: &mut std::collections::BTreeSet<usize>,
    invert: bool,
) {
    for &id in visible {
        if !selected.remove(&id) && invert {
            selected.insert(id);
        }
    }
}

fn content_group_rows(
    report: &crate::archive_check::Report,
    listing: &crate::archive::ArchiveListing,
) -> Result<Vec<usize>, String> {
    content_groups_rows(
        report.content_groups.as_deref().unwrap_or_default(),
        listing,
    )
}
fn content_groups_rows(
    groups: &[crate::archive_check::ContentGroup],
    listing: &crate::archive::ArchiveListing,
) -> Result<Vec<usize>, String> {
    let mut by_index = std::collections::HashMap::new();
    for (id, member) in listing.members.iter().enumerate() {
        if by_index.insert(member.index, (id, member)).is_some() {
            return Err(
                "Archive listing has ambiguous entry indexes; reopen before selecting.".into(),
            );
        }
    }
    let mut rows = Vec::new();
    for group in groups {
        for scanned in &group.members {
            let &(id, member) = by_index.get(&scanned.index).ok_or(
                "Archive listing differs from scanned identities; reopen before selecting.",
            )?;
            if member.path != scanned.path
                || member.size != scanned.size
                || !member.is_regular
                || member.is_dir
                || member.is_symlink
            {
                return Err(
                    "Archive listing differs from scanned identities; reopen before selecting."
                        .into(),
                );
            }
            rows.push(id);
        }
    }
    Ok(rows)
}

fn change_content_group_selection(
    report: &crate::archive_check::Report,
    listing: &crate::archive::ArchiveListing,
    group: Option<usize>,
    select: bool,
    selected: &mut std::collections::BTreeSet<usize>,
) -> Result<(), String> {
    let rows = if let Some(index) = group {
        let group = report
            .content_groups
            .as_ref()
            .and_then(|groups| groups.get(index))
            .ok_or("Content group is no longer available; run the scan again.")?;
        content_groups_rows(std::slice::from_ref(group), listing)?
    } else {
        content_group_rows(report, listing)?
    };
    // Resolve every identity first, so refusal cannot partly alter selection.
    if select {
        selected.extend(rows);
    } else {
        for id in rows {
            selected.remove(&id);
        }
    }
    Ok(())
}

/// Runtime-only replica policy shared by the per-group and global actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContentKeepOrder {
    #[default]
    First,
    Last,
    Newest,
    Oldest,
    Shallowest,
    Deepest,
}
impl ContentKeepOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::First => "First entries",
            Self::Last => "Last entries",
            Self::Newest => "Newest timestamps",
            Self::Oldest => "Oldest timestamps",
            Self::Shallowest => "Shallowest paths",
            Self::Deepest => "Deepest paths",
        }
    }
    fn short_label(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Last => "last",
            Self::Newest => "newest",
            Self::Oldest => "oldest",
            Self::Shallowest => "shallowest",
            Self::Deepest => "deepest",
        }
    }
}
pub struct ContentKeep {
    pub count: usize,
    pub order: ContentKeepOrder,
    pub protected_glob: String,
    pub protect_basename: bool,
    pub protect_case_sensitive: bool,
}
impl Default for ContentKeep {
    fn default() -> Self {
        Self {
            count: 1,
            order: ContentKeepOrder::First,
            protected_glob: String::new(),
            protect_basename: false,
            protect_case_sensitive: true,
        }
    }
}
impl ContentKeep {
    fn label(&self) -> String {
        format!(
            "{}{} {}",
            if self.protected_glob.lines().all(str::is_empty) {
                ""
            } else {
                "at least "
            },
            self.count,
            self.order.short_label()
        )
    }
    fn protection(&self) -> Result<Vec<crate::archive_filter::Pattern>, String> {
        if self.protected_glob.chars().count() > 8192 {
            return Err("Protection list exceeds 8192 characters.".into());
        }
        let mut patterns = Vec::new();
        for (line, glob) in self.protected_glob.lines().enumerate() {
            if glob.is_empty() {
                continue;
            }
            if patterns.len() == 16 {
                return Err("Use at most 16 nonempty protection patterns.".into());
            }
            patterns.push(
                crate::archive_filter::Pattern::compile(
                    glob,
                    crate::archive_filter::Options {
                        glob: true,
                        basename_only: self.protect_basename,
                        case_sensitive: self.protect_case_sensitive,
                        ..Default::default()
                    },
                )
                .map_err(|error| format!("Protection line {}: {error}", line + 1))?,
            );
        }
        Ok(patterns)
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        ui.horizontal_wrapped(|ui| {
            ui.label("Keep copies per group:");
            ui.add(egui::DragValue::new(&mut self.count).range(1..=100_000).speed(1));
            for order in [ContentKeepOrder::First,ContentKeepOrder::Last,ContentKeepOrder::Newest,ContentKeepOrder::Oldest,ContentKeepOrder::Shallowest,ContentKeepOrder::Deepest] {
                let label=order.label();
                let button=ui.selectable_value(&mut self.order,order,label).on_hover_text("First/last use original entry numbers. Newest/oldest use member timestamps declared in the archive: known dates first, missing dates last, ties by lowest original entry number. Shallowest/deepest use the number of parent folders inside the archive, with lowest original entry number for equal depths. Groups with fewer members keep them all. Apply with Select copies; changing this policy leaves selection unchanged.");
                #[cfg(feature="visual-qa")]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-keep-order",label)),button.rect));
                let _=button;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Protect paths (globs):");
            let input = ui.add(
                egui::TextEdit::multiline(&mut self.protected_glob)
                    .desired_rows(2)
                    .id(egui::Id::new("archive-protected-path-glob"))
                    .desired_width(260.0)
                    .char_limit(8192)
                    .hint_text("originals/**\n*.bak"),
            );
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("archive-protect-glob"), input.rect));
            let _ = input;
            let basename=ui.checkbox(&mut self.protect_basename,"Filename only").on_hover_text("Match the final filename at any folder depth instead of the complete archive-relative path. Editing this option leaves selection unchanged until Select copies.");
            let case=ui.checkbox(&mut self.protect_case_sensitive,"Match case").on_hover_text("When off, use the archive filter's lowercase-based matching for both pattern and filenames. Editing this option leaves selection unchanged until Select copies.");
            #[cfg(feature="visual-qa")]
            ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new("archive-protect-basename"),basename.rect);d.insert_temp(egui::Id::new("archive-protect-case"),case.rect);});
            let _=(basename,case);
        });
        ui.add(egui::Label::new(egui::RichText::new("One glob per line; all matches are kept; copy count is a minimum.").small()).wrap()).on_hover_text("One archive-relative glob per line; up to 16 nonempty patterns, 256 characters each. Any matching pattern protects the member. Empty lines are ignored; spaces are literal. Filename only matches the final name at any depth; Match case controls letter case. In full-path mode, * stays within a folder and ** crosses folders. Empty disables protection. Editing the pattern leaves selection unchanged until Select copies.");
        let error = self.protection().err();
        if let Some(error) = &error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        error.is_none()
    }
}

/// Lexical archive-relative parent depth; no filesystem or link resolution.
fn content_path_depth(path: &std::path::Path) -> usize {
    path.components()
        .filter(|part| matches!(part, std::path::Component::Normal(_)))
        .count()
        .saturating_sub(1)
}

/// Resolve the entire proposal before removing keepers from the selection.
fn select_redundant_content_groups(
    report: &crate::archive_check::Report,
    listing: &crate::archive::ArchiveListing,
    group: Option<usize>,
    keep: &ContentKeep,
    selected: &mut std::collections::BTreeSet<usize>,
) -> Result<(), String> {
    if keep.count == 0 {
        return Err("Keep at least one representative per group.".into());
    }
    let protection = keep.protection()?;
    let groups = report
        .content_groups
        .as_deref()
        .ok_or("No completed content-group scan is available.")?;
    let groups = if let Some(rank) = group {
        std::slice::from_ref(
            groups
                .get(rank)
                .ok_or("Content group is no longer available; run the scan again.")?,
        )
    } else {
        groups
    };
    let rows = content_groups_rows(groups, listing)?;
    let mut seen = std::collections::HashSet::new();
    let mut keepers = Vec::new();
    let mut offset = 0;
    for group in groups {
        if group.members.len() < 2 {
            return Err("Content group has fewer than two members; run the scan again.".into());
        }
        let mut ranked = group.members.iter().enumerate().collect::<Vec<_>>();
        ranked.sort_unstable_by(|(left, a), (right, b)| match keep.order {
            ContentKeepOrder::First => a.index.cmp(&b.index),
            ContentKeepOrder::Last => b.index.cmp(&a.index),
            ContentKeepOrder::Shallowest | ContentKeepOrder::Deepest => {
                let depth = content_path_depth(&a.path).cmp(&content_path_depth(&b.path));
                let depth = if keep.order == ContentKeepOrder::Deepest {
                    depth.reverse()
                } else {
                    depth
                };
                depth.then_with(|| a.index.cmp(&b.index))
            }
            ContentKeepOrder::Newest | ContentKeepOrder::Oldest => {
                let left = listing.members[rows[offset + left]].modified;
                let right = listing.members[rows[offset + right]].modified;
                let time = match (left, right) {
                    (Some(a), Some(b)) => {
                        if keep.order == ContentKeepOrder::Newest {
                            b.cmp(&a)
                        } else {
                            a.cmp(&b)
                        }
                    }
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                time.then_with(|| a.index.cmp(&b.index))
            }
        });
        let mut protected = vec![false; group.members.len()];
        if !protection.is_empty() {
            for (rank, member) in group.members.iter().enumerate() {
                let path=member.path.to_str().ok_or("Protected path matching requires UTF-8 member paths; selection was not changed.")?;
                let lowercase = if keep.protect_case_sensitive {
                    std::borrow::Cow::Borrowed(path)
                } else {
                    std::borrow::Cow::Owned(path.to_lowercase())
                };
                if protection
                    .iter()
                    .any(|pattern| pattern.matches(path, &lowercase))
                {
                    protected[rank] = true;
                    keepers.push(rows[offset + rank]);
                }
            }
        }
        let additional = keep
            .count
            .saturating_sub(protected.iter().filter(|&&p| p).count());
        keepers.extend(
            ranked
                .into_iter()
                .filter(|(rank, _)| !protected[*rank])
                .take(additional)
                .map(|(rank, _)| rows[offset + rank]),
        );
        offset += group.members.len();
    }
    if rows.iter().any(|id| !seen.insert(*id)) {
        return Err("Content groups overlap; run the scan again.".into());
    }
    selected.extend(rows);
    for keeper in keepers {
        selected.remove(&keeper);
    }
    Ok(())
}

/// Visible regular-file candidates only; declared CRC/size is not byte equality.
fn repeated_crc_rows(listing: &crate::archive::ArchiveListing, visible: &[usize]) -> Vec<usize> {
    if listing.kind != crate::archive::ArchiveKind::Zip {
        return Vec::new();
    }
    let mut counts = std::collections::HashMap::new();
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for &id in visible {
        if !seen.insert(id) {
            continue;
        }
        let Some(member) = listing
            .members
            .get(id)
            .filter(|m| m.is_regular && !m.is_dir && !m.is_symlink)
        else {
            continue;
        };
        let Some(crc) = member.zip_info.as_ref().and_then(|info| info.crc32) else {
            continue;
        };
        let key = (member.size, crc);
        *counts.entry(key).or_insert(0usize) += 1;
        rows.push((id, key));
    }
    rows.into_iter()
        .filter_map(|(id, key)| (counts[&key] > 1).then_some(id))
        .collect()
}

fn same_extension_rows(
    listing: &crate::archive::ArchiveListing,
    lower_paths: &[String],
    visible: &[usize],
    focused: Option<usize>,
) -> Vec<usize> {
    let Some(id) = focused.filter(|id| visible.contains(id)) else {
        return Vec::new();
    };
    if !listing
        .members
        .get(id)
        .is_some_and(|member| member.is_regular && !member.is_dir)
    {
        return Vec::new();
    }
    if lower_paths.len() != listing.members.len() {
        return Vec::new();
    }
    let key = crate::archive_order::extension_key(&lower_paths[id], false);
    visible
        .iter()
        .copied()
        .filter(|&row| {
            listing
                .members
                .get(row)
                .is_some_and(|member| member.is_regular && !member.is_dir)
                && crate::archive_order::extension_key(&lower_paths[row], false) == key
        })
        .collect()
}

#[derive(Clone, Copy)]
enum SimilarMember {
    Name,
    Folder,
}

fn similar_selection_menu(
    ui: &mut egui::Ui,
    listing: &crate::archive::ArchiveListing,
    lower_paths: &[String],
    visible: &[usize],
    focused: Option<usize>,
    selected: &mut std::collections::BTreeSet<usize>,
    mode: SimilarMember,
) -> bool {
    let label = match mode {
        SimilarMember::Name => "Same name",
        SimilarMember::Folder => "Same folder",
    };
    let enabled = focused.is_some_and(|id| {
        visible.contains(&id)
            && listing.members.get(id).is_some_and(|member| {
                matches!(mode, SimilarMember::Folder)
                    || (member.is_regular && !member.is_dir && !member.is_symlink)
            })
    });
    let menu = ui.add_enabled(
        enabled,
        egui::Button::new(label).wrap_mode(egui::TextWrapMode::Extend),
    );
    let mut applied = false;
    egui::Popup::menu(&menu).show(|ui| {
        for (select, action) in [(true, "Select visible matches"), (false, "Deselect visible matches")] {
            let response = ui.add_enabled(enabled, egui::Button::new(action)).on_hover_text(match mode {
                SimilarMember::Name => "Match regular-file names without their final extension, ignoring case. Keep hidden selections; no subtree expansion.",
                SimilarMember::Folder => "Match the exact internal parent folder of the focused real member. Include visible real records only; keep hidden selections and do not expand directories.",
            });
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("archive-similar-action", label, select)), response.rect));
            if response.clicked() && enabled {
                for id in similar_member_rows(listing, lower_paths, visible, focused, mode) {
                    if select { selected.insert(id); } else { selected.remove(&id); }
                }
                applied = true;
                ui.close();
            }
        }
    });
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::new(("archive-similar-menu", label)), menu.rect)
    });
    applied
}

fn similar_member_rows(
    listing: &crate::archive::ArchiveListing,
    lower_paths: &[String],
    visible: &[usize],
    focused: Option<usize>,
    mode: SimilarMember,
) -> Vec<usize> {
    let Some(id) = focused.filter(|id| visible.contains(id)) else {
        return Vec::new();
    };
    let Some(reference) = listing.members.get(id) else {
        return Vec::new();
    };
    let regular = |member: &crate::archive::ArchiveMember| {
        member.is_regular && !member.is_dir && !member.is_symlink
    };
    if matches!(mode, SimilarMember::Name)
        && (!regular(reference) || lower_paths.len() != listing.members.len())
    {
        return Vec::new();
    }
    let stem = |index: usize| {
        std::path::Path::new(&lower_paths[index])
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&lower_paths[index])
    };
    visible
        .iter()
        .copied()
        .filter(|&index| {
            let Some(member) = listing.members.get(index) else {
                return false;
            };
            match mode {
                SimilarMember::Name => regular(member) && stem(index) == stem(id),
                SimilarMember::Folder => {
                    reference.path.parent().is_some()
                        && member.path.parent() == reference.path.parent()
                }
            }
        })
        .collect()
}

fn member_details(
    kind: crate::archive::ArchiveKind,
    member: &crate::archive::ArchiveMember,
) -> [(&'static str, String); 9] {
    let member_kind = if member.is_symlink {
        "Symbolic link"
    } else if member.is_dir {
        "Directory"
    } else if member.is_regular {
        "Regular file"
    } else {
        "Other archive record"
    };
    [
        ("Kind", member_kind.to_owned()),
        (
            "Declared size",
            if member.is_dir {
                "Not applicable (directory)".into()
            } else {
                format!("{} bytes", member.size)
            },
        ),
        (
            "Packed size",
            if kind == crate::archive::ArchiveKind::Zip && !member.is_dir {
                format!("{} bytes", member.compressed_size)
            } else {
                "Unavailable".into()
            },
        ),
        (
            "Modified (UTC)",
            member
                .modified
                .map(crate::listing_export::iso_8601_utc)
                .unwrap_or_else(|| "Unavailable".into()),
        ),
        (
            "Unix permissions",
            member
                .unix_mode
                .map(|mode| format!("{:04o}", mode & 0o7777))
                .unwrap_or_else(|| "Unavailable".into()),
        ),
        (
            "Executable",
            match member.is_executable {
                Some(true) => "Yes",
                Some(false) => "No",
                None => "Unavailable",
            }
            .to_owned(),
        ),
        (
            "Compression method",
            member
                .zip_info
                .as_ref()
                .map(|info| info.compression.clone())
                .unwrap_or_else(|| "Unavailable (not recorded per member)".into()),
        ),
        (
            "Encrypted",
            member
                .zip_info
                .as_ref()
                .map(|info| {
                    if info.encrypted {
                        format!("Yes ({}, password required)", info.encryption)
                    } else {
                        "No".to_owned()
                    }
                })
                .unwrap_or_else(|| "Unavailable (not recorded per member)".into()),
        ),
        (
            "Declared CRC-32",
            member
                .zip_info
                .as_ref()
                .and_then(|info| info.crc32)
                .map(|crc| format!("{crc:08X}"))
                .unwrap_or_else(|| "Unavailable (not recorded per member)".into()),
        ),
    ]
}

/// Inline extract errors shown at most; the rest are summarized as a count.
const MAX_INLINE_ERRORS: usize = 5;

/// Toast and inline text for a partly failed extraction. A read-only
/// destination fails every member, so the toast names only the count and
/// the first error and the inline list stays capped.
fn extract_error_summary(errors: &[String]) -> (String, String) {
    let toast = match errors {
        [only] => only.clone(),
        [first, ..] => format!("{} members failed: {first}", errors.len()),
        [] => String::new(),
    };
    let mut inline = errors
        .iter()
        .take(MAX_INLINE_ERRORS)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if errors.len() > MAX_INLINE_ERRORS {
        inline.push_str(&format!(
            "\n… and {} more",
            errors.len() - MAX_INLINE_ERRORS
        ));
    }
    (toast, inline)
}

/// Label of the extract-selection button. Selected members the filter hides
/// are still extracted, so their count is named.
fn extract_selected_label(selected: usize, hidden: usize) -> String {
    match (selected, hidden) {
        (0, _) => "Extract selected to other panel".to_string(),
        (n, 0) => format!("Extract {n} to other panel"),
        (n, hidden) => format!("Extract {n} ({hidden} hidden) to other panel"),
    }
}

/// Label of the extract-everything button: a capped listing only holds the
/// first members, and only those are extracted.
fn extract_all_label(listing: &crate::archive::ArchiveListing) -> String {
    if listing.truncated {
        format!("Extract {} listed", listing.members.len())
    } else {
        "Extract all".to_string()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct MemberCursorMove {
    down: bool,
    rows: usize,
    extend: bool,
}

fn member_cursor_input(
    ctx: &egui::Context,
    filter_id: egui::Id,
    page_rows: usize,
) -> Option<MemberCursorMove> {
    if !ctx.memory(|memory| memory.has_focus(filter_id)) {
        return None;
    }
    ctx.input_mut(|input| {
        let modifiers = input.modifiers;
        if modifiers.alt || modifiers.ctrl || modifiers.command || modifiers.mac_cmd {
            return None;
        }
        for (key, down, rows) in [
            (egui::Key::PageUp, false, page_rows.max(1)),
            (egui::Key::PageDown, true, page_rows.max(1)),
            (egui::Key::ArrowUp, false, 1),
            (egui::Key::ArrowDown, true, 1),
        ] {
            if input.consume_key(modifiers, key) {
                return Some(MemberCursorMove {
                    down,
                    rows,
                    extend: modifiers.shift,
                });
            }
        }
        None
    })
}

/// Move the member cursor through `visible` (member indexes in list
/// order). A cursor the filter hides restarts at the first visible row.
fn step_member_cursor(
    visible: &[usize],
    focused: Option<usize>,
    up: bool,
    down: bool,
    rows: usize,
) -> Option<usize> {
    if !(up || down) || visible.is_empty() {
        return focused;
    }
    let Some(row) = focused.and_then(|index| visible.iter().position(|&v| v == index)) else {
        return visible.first().copied();
    };
    let row = if down {
        row.saturating_add(rows).min(visible.len() - 1)
    } else {
        row.saturating_sub(rows)
    };
    Some(visible[row])
}

impl App {
    pub(crate) fn open_archive(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.open_archive_on(path, self.ws.active.other(), ctx);
    }

    /// Open the archive tool over pane `side`. Retry passes the tool's own
    /// side: clicking in the tool makes its pane active, so recomputing it
    /// from focus would flip the tool to the other pane.
    fn open_archive_on(&mut self, path: PathBuf, side: ActivePanel, ctx: &egui::Context) {
        let repaint = ctx.clone();
        let run = crate::archive::start_listing(
            path.clone(),
            std::sync::Arc::new(move || repaint.request_repaint()),
        );
        self.ui.tools.replace_on(side);
        self.ui.tools.archive = Some(ArchiveState {
            content_group_page: 0,
            content_keep: ContentKeep::default(),
            content_member_pages: std::collections::BTreeMap::new(),
            strict_zip_headers: false,
            password: zeroize::Zeroizing::new(String::new()),
            check: None,
            check_result: None,
            check_strict_snapshot: None,
            check_recorded_at: None,
            copy_format: crate::archive_export::Format::default(),
            path,
            filter: String::new(),
            run: Some(run),
            extract: None,
            listing: None,
            member_filter: MemberFilter::default(),
            selected: std::collections::BTreeSet::new(),
            status: None,
            error: None,
        });
        Self::mark_tool_opened(ctx, "archive");
    }

    /// The visible pane beside the tool, which extraction writes into.
    fn archive_extract_dest(&self) -> PathBuf {
        let side = self.ui.tools.side.unwrap_or(self.ws.active).other();
        match side {
            ActivePanel::Left => self.ws.left.current_path.clone(),
            ActivePanel::Right => self.ws.right.current_path.clone(),
        }
    }

    fn poll_archive(&mut self, now: f64) {
        let event = self.ui.tools.archive.as_ref().and_then(|state| {
            let run = state.run.as_ref()?;
            match run.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Archive worker stopped before completion".to_string()))
                }
            }
        });
        if let Some(result) = event
            && let Some(state) = self.ui.tools.archive.as_mut()
        {
            state.run = None;
            match result {
                Ok(listing) => {
                    state.listing = Some(listing);
                    state.member_filter = MemberFilter::default();
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
        }

        if let Some(state) = self.ui.tools.archive.as_mut() {
            let result = state.check.as_ref().and_then(|run| match run.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(
                    "Archive check worker stopped; no complete result.".into(),
                )),
            });
            if let Some(result) = result {
                state.check = None;
                state.check_result = Some(result);
                state.content_group_page = 0;
                state.content_member_pages.clear();
                state.check_recorded_at = Some(chrono::Utc::now());
            }
        }

        let extract_event = self.ui.tools.archive.as_ref().and_then(|state| {
            let run = state.extract.as_ref()?;
            match run.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Extract worker stopped before completion".to_string()))
                }
            }
        });
        if let Some(result) = extract_event
            && let Some(state) = self.ui.tools.archive.as_mut()
        {
            state.extract = None;
            match result {
                Ok(report) => {
                    let status = format!(
                        "Extracted {} · skipped existing {} · dirs {}",
                        report.extracted, report.skipped_existing, report.skipped_dirs
                    );
                    state.status = Some(status.clone());
                    if report.errors.is_empty() {
                        state.error = None;
                        self.toast_at(now, crate::toasts::ToastKind::Info, status, false);
                    } else {
                        let (toast, inline) = extract_error_summary(&report.errors);
                        state.error = Some(inline);
                        self.toast_at(now, crate::toasts::ToastKind::Error, toast, false);
                    }
                    self.ws.left.refresh();
                    self.ws.right.refresh();
                }
                Err(error) => {
                    // Drop the "Extracting…" line so it is not left beside the error.
                    state.status = None;
                    state.error = Some(error.clone());
                    self.toast_at(now, crate::toasts::ToastKind::Error, error, false);
                }
            }
        }
    }

    pub(crate) fn show_archive_tool(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let escape_requested =
            self.take_escape_request(crate::accessibility::EscapeRoute::PaneTool);
        if self.ui.tools.archive.is_none() {
            return;
        }
        let now = ctx.input(|input| input.time);
        self.poll_archive(now);

        let sheet_height = ui.available_height().max(0.0);
        let rows_height = (sheet_height * 0.35).clamp(72.0, 280.0);
        let page_rows = (rows_height / (30.0 + ui.spacing().item_spacing.y))
            .floor()
            .max(1.0) as usize;
        let t = self.colors;
        let window_open = true;
        let mut reveal = false;
        let mut retry = false;
        let mut extract_selected = false;
        let mut extract_all = false;
        let mut stop_extract = false;
        let mut start_check = false;
        let mut start_content_groups = false;
        let mut select_content_groups = false;
        let mut select_content_group = None;
        let mut select_redundant_groups = None;
        let mut stop_check = false;
        let mut copy_members = None;
        let mut copy_check_report = None;
        let mut sort_requested = None;
        let mut navigate: Option<Option<PathBuf>> = None;
        let mut history_requested = None;
        let dest = self.archive_extract_dest();
        {
            let state = self.ui.tools.archive.as_mut().unwrap();
            let title = state.path.file_name().map_or_else(
                || "Archive".to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            egui::ScrollArea::vertical()
                .id_salt("archive-sheet")
                .auto_shrink([false, false])
                .min_scrolled_height(0.0)
                .max_height(sheet_height)
                .show(ui, |ui| {
                #[cfg(feature = "visual-qa")]
                ui.ctx().data_mut(|d| { d.insert_temp(egui::Id::new("archive-sheet-clip"), ui.clip_rect()); d.remove::<(PathBuf, Vec<(String, String, bool)>)>(egui::Id::new("archive-rendered-details")); });
                ui.label(
                    egui::RichText::new(title)
                        .size(13.0)
                        .strong()
                        .color(t.text_primary),
                );
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2((ui.available_width() - 82.0).max(120.0), 22.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(state.path.display().to_string())
                                        .size(11.0)
                                        .color(t.text_muted),
                                )
                                .truncate(),
                            )
                        },
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Reveal").clicked() {
                            reveal = true;
                        }
                    });
                });
                ui.add_space(6.0);
                // Up/Down in the filter move the member cursor and Enter
                // toggles its selection, so the list works without a mouse.
                let filter_id = egui::Id::new("archive-filter");
                let cursor_move = member_cursor_input(ctx, filter_id, page_rows);
                let filter = ui.add(
                    egui::TextEdit::singleline(&mut state.filter)
                        .char_limit(256)
                        .id(filter_id)
                        .desired_width(f32::INFINITY)
                        .hint_text("Filter archive members (arrows move, Shift extends, Enter opens/selects)")
                        .margin(egui::vec2(8.0, 6.0)),
                ).on_hover_text("Up/Down move one member; PageUp/PageDown move one list viewport. Hold Shift to add a range to the selection.");
                let toggle_cursor =
                    filter.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if Self::take_tool_opened(ui.ctx(), "archive") || toggle_cursor {
                    filter.request_focus();
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label("File size");
                    let response = ui.add_enabled(state.listing.is_some() && state.run.is_none(),
                        egui::TextEdit::singleline(&mut state.member_filter.size_input)
                            .id(egui::Id::new("archive-size-filter"))
                            .char_limit(64).desired_width(180.0).hint_text("+1mi -10mi"))
                        .on_hover_text("Declared unpacked regular-file size: + minimum, - maximum, no sign exact. Two constraints form a range. b, k/m/g/t are decimal; ki/mi/gi/ti are binary. Blank disables this filter; navigation folders remain available.");
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, response.enabled(), "Archive regular-file size constraints"));
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("archive-filter-input", "Archive regular-file size constraints")), response.rect));
                    ui.label("Depth");
                    let depth = ui.add_enabled(state.listing.is_some() && state.run.is_none(),
                        egui::TextEdit::singleline(&mut state.member_filter.depth_input)
                            .id(egui::Id::new("archive-depth-filter")).char_limit(24).desired_width(110.0).hint_text("2 or 1..3"))
                        .on_hover_text("Depth from archive root: root members are 1, src/file is 2. Exact 2, minimum 2.., maximum ..3, inclusive 2..4. Applies to real member paths; navigation folders stay available. Blank disables it.");
                    depth.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, depth.enabled(), "Archive member depth constraints"));
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("archive-filter-input", "Archive member depth constraints")), depth.rect));
                });

                ui.horizontal_wrapped(|ui| {
                    let enabled = state.listing.is_some() && state.run.is_none();
                    ui.label("Modified (UTC)");
                    let dates = ui.add_enabled(enabled, egui::TextEdit::singleline(&mut state.member_filter.modified_input)
                        .id(egui::Id::new("archive-modified-filter")).char_limit(24).desired_width(240.0).hint_text("2026-01-01..2026-02-01"))
                        .on_hover_text("UTC dates only. A single date matches that day; start.. includes the start, ..end excludes the end. Unknown modification times do not match an active range. Navigation folders remain available.");
                    dates.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, dates.enabled(), "Archive modification dates in UTC"));
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("archive-filter-input", "Archive modification dates in UTC")), dates.rect));
                    ui.label("Extensions");
                    let extensions = ui.add_enabled(enabled, egui::TextEdit::singleline(&mut state.member_filter.extensions_input)
                        .id(egui::Id::new("archive-extensions-filter")).char_limit(256).desired_width(160.0).hint_text("pdf jpg tar.gz"))
                        .on_hover_text("Any listed literal extension, separated by spaces; optional leading dots. Case-insensitive, final filename only, including multi-part suffixes. Applies to all real member types; navigation folders remain available. Blank disables it. No wildcards.");
                    extensions.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, extensions.enabled(), "Archive member extensions"));
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("archive-filter-input", "Archive member extensions")), extensions.rect));
                });
                state.member_filter.exclusion.ui(ui, state.listing.is_some() && state.run.is_none());
                ui.horizontal_wrapped(|ui| {
                    let enabled = state.listing.is_some() && state.run.is_none();
                    matching_modes_ui(ui, &mut state.member_filter.options, enabled);
                    ui.add_enabled(enabled, egui::Checkbox::new(&mut state.member_filter.options.case_sensitive, "Match case"));
                    ui.add_enabled(enabled, egui::Checkbox::new(&mut state.member_filter.exclude_matches, "Exclude matches"))
                        .on_hover_text("Hide paths matched by the current path/name filter. An empty pattern matches everything, so exclusion hides all rows. Invalid patterns still show an error and no rows.");
                    ui.add_enabled(enabled, egui::Checkbox::new(&mut state.member_filter.options.smart_case, "Smart case"))
                        .on_hover_text("Match case when the query contains Unicode uppercase characters, including regex syntax. Match case overrides this; inline regex flags still apply.");
                    ui.add_enabled(enabled, egui::Checkbox::new(&mut state.member_filter.options.basename_only, "Filename only"))
                        .on_hover_text("Match the final filename or navigation-folder name instead of the full internal path. Applies to all matching modes, including exclusion.");
                    hidden_entries_ui(ui, &mut state.member_filter.kinds, enabled);
                    let reset = ui.add_enabled(enabled && state.member_filter.has_filters(&state.filter), egui::Button::new("Reset filters"))
                        .on_hover_text("Clear the path, size, depth and date queries, Regex/case/exclusion options, type filters and narrowing. Keep folder, sorting, selection, saved selection and history.");
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("archive-reset-filters"), reset.rect));
                    if reset.clicked() {
                        state.member_filter.reset_filters(&mut state.filter);
                        filter.request_focus();
                    }
                    if state.member_filter.saved_filters_ui(ui, &mut state.filter, enabled) {
                        filter.request_focus();
                    }
                });
                ui.add_space(6.0);
                if state.run.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Reading archive contents…")
                                .size(11.0)
                                .color(t.text_muted),
                        );
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    let test = ui.add_enabled(state.listing.is_some() && state.run.is_none() && state.extract.is_none() && state.check.is_none(), egui::Button::new("Test archive"))
                        .on_hover_text("Read every member without extraction, using the ZIP password below when needed. Limited to 128 MiB decoded data.");
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("archive-test"), test.rect));
                    start_check = test.clicked();
                    if state.listing.is_some() {
                        let groups=ui.add_enabled(state.run.is_none() && state.extract.is_none() && state.check.is_none(),egui::Button::new("Find matching contents"))
                            .on_hover_text("Read every member, then group safe regular files by actual size and SHA-256. ZIP requires consistent headers and unique names, with CRC/AES verification and one-use password. TAR validates headers and compressed streams but has no payload checksum; 128 MiB decoded and 8 MiB identity metadata limits. No extraction or deletion.");
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("archive-content-groups"),groups.rect));
                        if groups.clicked() {state.strict_zip_headers=state.listing.as_ref().is_some_and(|listing|listing.kind==crate::archive::ArchiveKind::Zip);start_check=true;start_content_groups=true;}
                    }

                    if state.listing.as_ref().is_some_and(|listing| listing.kind == crate::archive::ArchiveKind::Zip) {
                        let strict = ui.add_enabled(state.check.is_none() && state.extract.is_none(), egui::Checkbox::new(&mut state.strict_zip_headers, "Strict ZIP headers"))
                            .on_hover_text("Also compare local and central headers and reject duplicate names. Some permissive ZIP archives fail this stricter check.");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("archive-strict-headers"), strict.rect));
                        let _ = strict;
                    }
                    if let Some(run) = &state.check {
                        ui.spinner();
                        ui.label(format!("Checking… {} decoded bytes", run.progress.load(std::sync::atomic::Ordering::Relaxed)));
                        stop_check = ui.small_button("Stop check").clicked();
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
                    }
                });
                if let Some(result) = &state.check_result {
                    match result {
                        Ok(report) => {
                            let label = ui.label(report.summary());
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("archive-check-result"), label.rect));
                            let _ = label;
                        }
                        Err(error) => {
                            let label = ui.colored_label(ui.visuals().error_fg_color, format!("Check not completed: {error}"));
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("archive-check-result"), label.rect));
                            let _ = label;
                        }
                    }
                    if let Ok(report)=result && let Some(groups)=&report.content_groups {
                        let select=ui.add_enabled(!groups.is_empty(),egui::Button::new("Select scanned content matches"))
                            .on_hover_text("Add every scanned matching regular entry, including hidden ones, after matching original entry index, path and size to this listing. Existing selection is kept. These are SHA-256 matches from the last completed scan, not a new byte-to-byte comparison.");
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("archive-select-content-groups"),select.rect));
                        select_content_groups=select.clicked();
                        let keep_valid=state.content_keep.ui(ui);
                        let redundant=ui.add_enabled(!groups.is_empty() && keep_valid,egui::Button::new(format!("Select copies; keep {} in each group",state.content_keep.label())))
                            .on_hover_text("Select extra copies using the configured count and keep order. Kept entries are deselected even if previously selected. Unrelated selections stay unchanged. Includes hidden members and all pages; no archive files are changed.");
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new("archive-select-redundant-groups"),redundant.rect);d.insert_temp(egui::Id::new("archive-select-redundant-groups-enabled"),redundant.enabled());});
                        if redundant.clicked(){select_redundant_groups=Some(None);}

                        let details=egui::CollapsingHeader::new(format!("Matching content groups ({})",groups.len())).show(ui,|ui| {
                            let range=content_group_pager(ui,groups.len(), &mut state.content_group_page);
                            for (_rank,group) in groups.iter().enumerate().skip(range.start).take(range.len()) {
                                let _title=ui.label(format!("SHA-256 {} · {} bytes · {} members",group.sha256,group.size,group.members.len()));
                                #[cfg(feature="visual-qa")]
                                let (mut painted,mut rect,mut paths)=(ui.clip_rect().contains_rect(_title.rect),_title.rect,Vec::new());
                                let member_range=if group.members.len()>CONTENT_MEMBERS_PER_PAGE {
                                    let page=state.content_member_pages.entry(_rank).or_default();
                                    ui.push_id(("archive-group-members",_rank),|ui|content_pager(ui,group.members.len(),page,CONTENT_MEMBERS_PER_PAGE,["Previous members","Next members","Members"])).inner
                                } else {0..group.members.len()};
                                for member in &group.members[member_range] {
                                    let _label=ui.label(format!("#{} {}",member.index,member.path.display()));
                                    #[cfg(feature="visual-qa")]
                                    {painted &= ui.clip_rect().contains_rect(_label.rect);rect=rect.union(_label.rect);paths.push(member.path.clone());}
                                }
                                #[cfg(feature="visual-qa")]
                                if _rank==0 {ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("archive-rendered-content-group"),(group.sha256.clone(),paths,painted,rect)));}
                                if group.members.len()>CONTENT_MEMBERS_PER_PAGE {ui.label("Select group includes every member across all pages");}
                                ui.horizontal_wrapped(|ui| {
                                    for (select,label) in [(true,"Select group"),(false,"Deselect group")] {
                                        let button=ui.small_button(label).on_hover_text("Change all scanned members of this SHA-256 group, including hidden entries. Other groups and unrelated selections stay unchanged; identities are checked before applying.");
                                        #[cfg(feature="visual-qa")]
                                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-content-group-selection",_rank,select)),button.rect));
                                        if button.clicked(){select_content_group=Some((_rank,select));}
                                    }
                                    let redundant=ui.add_enabled(keep_valid,egui::Button::new(format!("Select copies; keep {}",state.content_keep.label())).small()).on_hover_text("Apply the configured keep count and keep order to this group. Kept entries are deselected; other groups and unrelated selections stay unchanged. Includes hidden members and every page. No files are changed.");
                                    #[cfg(feature="visual-qa")]
                                    ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new(("archive-select-redundant-group",_rank)),redundant.rect);d.insert_temp(egui::Id::new(("archive-select-redundant-group-enabled",_rank)),redundant.enabled());});
                                    if redundant.clicked(){select_redundant_groups=Some(Some(_rank));}
                                    for (format,label) in [(crate::archive_check_export::Format::Text,"Copy group report"),(crate::archive_check_export::Format::Json,"Copy group JSON")] {
                                        let button=ui.small_button(label).on_hover_text("Copy every retained member of this group, including other pages. Scan totals remain complete-scan metadata; the report identifies this group's original index.");
                                        #[cfg(feature="visual-qa")]
                                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-content-group-export",_rank,label)),button.rect));
                                        if button.clicked(){copy_check_report=Some(crate::archive_check_export::format_group(&state.path,result,state.check_recorded_at.as_ref(),_rank,format));}
                                    }
                                });
                            }
                        });
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("archive-content-groups-details"),details.header_response.rect));
                        let _=details;
                    }
                    ui.horizontal_wrapped(|ui| {
                        for format in crate::archive_check_export::Format::ALL {
                            let button = ui.small_button(format.label());
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(format.label()), button.rect));
                            if button.clicked() {
                                copy_check_report = Some(crate::archive_check_export::format(&state.path, result, state.check_strict_snapshot, state.check_recorded_at.as_ref(), format));
                            }
                        }
                    });
                }
                let status = if state.extract.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        let status = ui.label(
                            egui::RichText::new(state.status.as_deref().unwrap_or("Extracting…"))
                                .size(11.0)
                                .color(t.text_muted),
                        );
                        if ui
                            .small_button("Stop")
                            .on_hover_text("Stop extracting; members already written stay")
                            .clicked()
                        {
                            stop_extract = true;
                        }
                        Some(status)
                    })
                    .inner
                } else {
                    state.status.as_ref().map(|status| {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(t.text_secondary),
                        )
                    })
                };
                // Announce progress and the result to assistive technology.
                if let Some(status) = status {
                    ui.ctx().accesskit_node_builder(status.id, |node| {
                        node.set_role(egui::accesskit::Role::Status);
                        node.set_live(egui::accesskit::Live::Polite);
                    });
                }
                if let Some(error) = &state.error {
                    ui.add(
                        egui::Label::new(egui::RichText::new(error).size(11.0).color(t.accent_red))
                            .wrap(),
                    );
                    // Once listed, errors come from extraction: re-listing
                    // would drop the selection, and Extract retries it.
                    if state.listing.is_none() && ui.button("Retry").clicked() {
                        retry = true;
                    }
                }

                if let Some(listing) = &state.listing {
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
                                                navigate = Some(Some(folder));
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
                                                navigate = Some(Some(folder.clone()));
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
                            let enabled = if back { !state.member_filter.past.is_empty() } else { !state.member_filter.future.is_empty() };
                            let response = ui.add_enabled(enabled, egui::Button::new(label))
                                .on_hover_text(if back { "Previous view inside this archive" } else { "Next view inside this archive" });
                            if response.clicked() { history_requested = Some(back); }
                        }

                        if ui
                            .selectable_label(state.member_filter.folder.is_none(), "All members")
                            .clicked()
                        {
                            navigate = Some(None);
                        }
                        if ui
                            .add_enabled(
                                listing.tree.is_ok(),
                                egui::Button::new("Folders")
                                    .selected(state.member_filter.folder.is_some()),
                            )
                            .clicked()
                        {
                            navigate = Some(Some(PathBuf::new()));
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
                                        .add_enabled(
                                            !folder.as_os_str().is_empty(),
                                            egui::Button::new("Up"),
                                        )
                                        .clicked()
                                    {
                                        navigate = Some(Some(
                                            folder
                                                .parent()
                                                .unwrap_or(std::path::Path::new(""))
                                                .into(),
                                        ));
                                    }
                                    if ui.button("Archive root").clicked() {
                                        navigate = Some(Some(PathBuf::new()));
                                    }
                                    let mut prefix = PathBuf::new();
                                    for part in folder.components() {
                                        prefix.push(part.as_os_str());
                                        if ui.button(part.as_os_str().to_string_lossy()).clicked() {
                                            navigate = Some(Some(prefix.clone()));
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
                        ui.colored_label(ui.visuals().error_fg_color, format!("Exclude glob: {error}"));
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
                                copy_members = Some(crate::archive_export::format(&state.path, listing, &rows, state.copy_format).map(|text| (text, count)));
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
                            select_member_range(visible, anchor, *focused, listing.members.len(), &mut state.selected);
                        } else {
                            *range_anchor = *focused;
                        }
                        *scroll_to_focused = true;
                    }
                    if toggle_cursor && let Some(index) = *focused {
                        if index >= listing.members.len() {
                            if let Ok(tree) = &listing.tree {
                                navigate = Some(Some(
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
                                egui::RichText::new(format!(
                                    "{} unreadable",
                                    listing.unreadable_members
                                ))
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
                    let show_crc = listing.kind == crate::archive::ArchiveKind::Zip && ui.available_width() >= 500.0;
                    let tail_width = 230.0 + if show_crc { 86.0 } else { 0.0 } + if show_modified { 126.0 } else { 0.0 };
                    let name_width = (ui.available_width() - 8.0 - tail_width).max(40.0);
                    let mut columns = vec![
                        (ArchiveField::Name, name_width),
                        (ArchiveField::Size, 72.0),
                        (ArchiveField::Packed, 72.0),
                        (ArchiveField::Ratio, 54.0),
                    ];
                    if show_crc { columns.push((ArchiveField::Crc32, 80.0)); }
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
                                egui::Button::new(
                                    egui::RichText::new(label).size(10.0).color(t.text_muted),
                                )
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
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("archive-sort-header", field.label())), header.rect));
                            if header
                                .on_hover_text("Click to sort; click again to reverse")
                                .clicked()
                            {
                                sort_requested = Some(field);
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
                                && let Some(row) = focused
                                    .and_then(|index| visible.iter().position(|&v| v == index))
                            {
                                let pitch = 30.0 + ui.spacing().item_spacing.y;
                                let top =
                                    ui.max_rect().top() + (row as f32 - rows.start as f32) * pitch;
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
                                                    egui::RichText::new(path)
                                                        .size(11.0)
                                                        .color(t.text_secondary),
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
                                            } else if listing.kind
                                                != crate::archive::ArchiveKind::Zip
                                            {
                                                "n/a".to_string()
                                            } else {
                                                format_size(member.compressed_size)
                                            };
                                            let ratio =
                                                if member.is_dir || member.compressed_size == 0 {
                                                    String::new()
                                                } else {
                                                    format!(
                                                        "{:.1}x",
                                                        member.size as f64
                                                            / member.compressed_size as f64
                                                    )
                                                };
                                            let modified =
                                                crate::search::modified_label(member.modified);
                                            let mut values =
                                                vec![(size, 72.0), (packed, 72.0), (ratio, 54.0)];
                                            if show_crc { values.push((member.zip_info.as_ref().and_then(|info|info.crc32).map(|crc|format!("{crc:08X}")).unwrap_or_else(|| "—".into()),80.0)); }
                                            if show_modified {
                                                values.push((modified, 120.0));
                                            }
                                            for (value, width) in values {
                                                ui.add_sized(
                                                    [width, 22.0],
                                                    egui::Label::new(
                                                        egui::RichText::new(value)
                                                            .size(10.0)
                                                            .color(t.text_muted),
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
                                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("archive-member-row", member.path.clone())), response.rect));
                                }
                                response.widget_info(|| {
                                    if synthetic {
                                        return egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            true,
                                            format!(
                                                "Open archive folder {}",
                                                member.path.display()
                                            ),
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
                                        select_member_range(visible, anchor, Some(index), listing.members.len(), &mut state.selected);
                                    } else if synthetic {
                                        navigate = Some(Some(member.path.clone()));
                                    } else {
                                        *range_anchor = Some(index);
                                        if !state.selected.remove(&index) { state.selected.insert(index); }
                                    }
                                }
                                if response.double_clicked()
                                    && !ui.input(|input| input.modifiers.shift)
                                    && member.is_dir
                                    && listing.tree.is_ok()
                                {
                                    navigate = Some(Some(member.path.clone()));
                                }
                            }
                        });
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("archive-current-member-rows"), rendered_rows);
                        d.insert_temp(egui::Id::new("archive-member-clip"), member_rows.inner_rect.intersect(ui.clip_rect()));
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
                            egui::RichText::new(format!(
                                "#{}  |  {}",
                                member.index,
                                member.path.display()
                            ))
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
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("archive-details-header"), details.header_response.rect));
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
                        if selection_mask.ui(ui, listing, lower_paths.as_deref().unwrap_or(&[]), visible, &mut state.selected) {
                            *range_anchor = None;
                        }
                    });
                    if !state.selected.is_empty() {
                        ui.label(selection_stats.get(listing, &state.selected).label())
                            .on_hover_text("Declared metadata for all selected real members, including hidden ones. Byte totals cover regular files only; directories do not expand implicitly. Not verified payload sizes or required disk space.");
                    }
                    if listing.kind == crate::archive::ArchiveKind::Zip
                        && listing.members.iter().any(|member| member.zip_info.as_ref().is_some_and(|info| info.encrypted))
                    {
                        ui.add_enabled_ui(state.extract.is_none() && state.check.is_none(), |ui| {
                            let _ = archive_password_input(ui, &mut state.password);
                        });
                    }
                    let dest_hint = format!("Extract into {}", dest.display());
                    ui.horizontal_wrapped(|ui| {
                        let n = state.selected.len();
                        let hidden = if state.filter.trim().is_empty() && !folder_mode && !narrowed_view && !kind_filtered && !path_excluded {
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
                                    egui::RichText::new(extract_selected_label(n, hidden))
                                        .size(13.0),
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
                            extract_selected = true;
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
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("archive-extract-all"), all_button.rect));
                        if all_button.clicked() { extract_all = true; }
                    });
                }
            });
        }

        if let Some(back) = history_requested
            && let Some(state) = self.ui.tools.archive.as_mut()
        {
            state.member_filter.history_step(back);
            ctx.request_repaint();
        } else if let Some(folder) = navigate
            && let Some(state) = self.ui.tools.archive.as_mut()
        {
            state.member_filter.navigate(folder);
            ctx.request_repaint();
        }

        if let Some(field) = sort_requested
            && let Some(state) = self.ui.tools.archive.as_mut()
        {
            state.member_filter.order.choose(field);
            ctx.request_repaint();
        }

        if !window_open || escape_requested {
            // Dropping the run cancels it after the member in progress.
            if self
                .ui
                .tools
                .archive
                .as_ref()
                .is_some_and(|state| state.extract.is_some())
            {
                self.toast_at(
                    now,
                    crate::toasts::ToastKind::Info,
                    "Extraction cancelled; some members may already be extracted".to_string(),
                    false,
                );
            }
            self.ui.tools.archive = None;
            return;
        }
        if stop_check && let Some(state) = self.ui.tools.archive.as_mut() {
            state.check = None;
            state.check_result = Some(Err("Stopped; no complete result.".into()));
            state.check_recorded_at = Some(chrono::Utc::now());
        } else if start_check && let Some(state) = self.ui.tools.archive.as_mut() {
            let repaint = ctx.clone();
            state.check_result = None;
            state.content_group_page = 0;
            state.content_member_pages.clear();
            state.check_recorded_at = None;
            state.check_strict_snapshot = Some(state.strict_zip_headers);
            let start = if start_content_groups {
                crate::archive_check::start_content_groups
            } else {
                crate::archive_check::start
            };
            state.check = Some(start(
                state.path.clone(),
                Some(std::mem::take(&mut state.password)),
                state.strict_zip_headers,
                std::sync::Arc::new(move || repaint.request_repaint()),
            ));
        }
        if select_content_groups
            && let Some(state) = self.ui.tools.archive.as_mut()
            && let (Some(Ok(report)), Some(listing)) = (&state.check_result, &state.listing)
            && let Err(error) =
                change_content_group_selection(report, listing, None, true, &mut state.selected)
        {
            state.error = Some(error);
        }
        if let Some((group, select)) = select_content_group
            && let Some(state) = self.ui.tools.archive.as_mut()
            && let (Some(Ok(report)), Some(listing)) = (&state.check_result, &state.listing)
            && let Err(error) = change_content_group_selection(
                report,
                listing,
                Some(group),
                select,
                &mut state.selected,
            )
        {
            state.error = Some(error);
        }
        if let Some(group) = select_redundant_groups
            && let Some(state) = self.ui.tools.archive.as_mut()
            && let (Some(Ok(report)), Some(listing)) = (&state.check_result, &state.listing)
            && let Err(error) = select_redundant_content_groups(
                report,
                listing,
                group,
                &state.content_keep,
                &mut state.selected,
            )
        {
            state.error = Some(error);
        }
        if let Some(result) = copy_check_report {
            match result {
                Ok(text) => self.write_clipboard(&text, "archive check report", ctx),
                Err(error) => self.toast(ctx, crate::toasts::ToastKind::Error, error, false),
            }
        }
        if let Some(result) = copy_members {
            match result {
                Ok((text, count)) => self.write_clipboard(
                    &text,
                    &format!("listing for {count} archive members"),
                    ctx,
                ),
                Err(error) => self.toast(ctx, crate::toasts::ToastKind::Error, error, false),
            }
        }
        if stop_extract && let Some(state) = self.ui.tools.archive.as_mut() {
            // Dropping the run cancels it after the member in progress.
            state.extract = None;
            state.status =
                Some("Extraction stopped; members already written stay in place".to_string());
            self.ws.left.refresh();
            self.ws.right.refresh();
        }
        if reveal && let Some(state) = &self.ui.tools.archive {
            self.ws.reveal(&state.path);
        }
        if retry
            && let Some(path) = self
                .ui
                .tools
                .archive
                .as_ref()
                .map(|state| state.path.clone())
        {
            let side = self.ui.tools.side.unwrap_or_else(|| self.ws.active.other());
            self.open_archive_on(path, side, ctx);
            return;
        }
        if extract_selected || extract_all {
            self.start_archive_extract(ctx, extract_all);
        }
    }

    fn start_archive_extract(&mut self, ctx: &egui::Context, all: bool) {
        let Some(state) = self.ui.tools.archive.as_mut() else {
            return;
        };
        if state.check.is_some() || state.extract.is_some() {
            return;
        }
        let Some(listing) = state.listing.as_ref() else {
            return;
        };
        let indexes: Vec<usize> = if all {
            listing.members.iter().map(|m| m.index).collect()
        } else {
            state
                .selected
                .iter()
                .filter_map(|row| listing.members.get(*row).map(|m| m.index))
                .collect()
        };
        if indexes.is_empty() {
            return;
        }
        let archive = state.path.clone();
        // Empty is a valid ZIP password too; preserve it as Some rather than
        // treating an empty field as an absent password.
        let password = Some(std::mem::take(&mut state.password));
        let dest = self.archive_extract_dest();
        let repaint = ctx.clone();
        let run = crate::archive::start_extract(
            archive,
            dest,
            indexes,
            password,
            std::sync::Arc::new(move || repaint.request_repaint()),
        );
        if let Some(state) = self.ui.tools.archive.as_mut() {
            state.extract = Some(run);
            state.error = None;
            state.status = Some("Extracting…".to_string());
        }
    }
}

/// Passwords never enter persisted app/session state. Clear egui's undo history
/// each frame as `TextEdit` otherwise keeps plaintext snapshots across frames.
fn archive_password_input(
    ui: &mut egui::Ui,
    password: &mut zeroize::Zeroizing<String>,
) -> egui::Response {
    ui.horizontal_wrapped(|ui| {
        ui.label("ZIP password");
        let mut output = egui::TextEdit::singleline(&mut **password)
            .id_salt("archive-extraction-password")
            .password(true)
            .desired_width(180.0)
            .hint_text("For this operation")
            .show(ui);
        #[cfg(feature = "visual-qa")]
        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("archive-password-input"), (output.response.rect, output.response.id)));
        output.state.clear_undoer();
        output.state.store(ui.ctx(), output.response.id);
        output.response.response.on_hover_text("ZIP password (ZipCrypto or AES), used only for extraction, integrity checking or content-group scans. Cleared when either operation starts or the archive pane closes.")
    }).inner
}

/// A fixed-width table cell whose label starts at the left edge
/// (`add_sized` would center it).
fn left_cell(ui: &mut egui::Ui, width: f32, height: f32, label: egui::Label) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.add(label)
        },
    )
    .inner
}

#[cfg(test)]
mod tests {
    #[test]
    fn archive_protection_lists_union_matches_and_refuse_invalid_lines_atomically() {
        use super::{ContentKeep, select_redundant_content_groups};
        use std::collections::BTreeSet;
        let mut listing = listing(&["a.txt", "b.bak", "nested/a.txt", "other", "unrelated"]);
        for (index, member) in listing.members.iter_mut().enumerate() {
            member.index = index;
            member.size = 3;
        }
        let report = crate::archive_check::Report {
            kind: crate::archive::ArchiveKind::Tar,
            members: 5,
            decoded: 15,
            strict_headers: false,
            content_groups: Some(vec![crate::archive_check::ContentGroup {
                sha256: "hash".into(),
                size: 3,
                members: listing.members[..4]
                    .iter()
                    .map(|m| crate::archive_check::ContentMember {
                        index: m.index,
                        path: m.path.clone(),
                        size: m.size,
                    })
                    .collect(),
            }]),
        };
        for group in [None, Some(0)] {
            for (glob, basename, case_sensitive, count, wanted) in [
                ("*.txt\n*.bak", false, true, 1, vec![2, 3, 4]),
                ("*.txt\r\n\r\n*.bak\r\n", false, true, 1, vec![2, 3, 4]),
                ("*.TXT\n*.BAK", true, false, 1, vec![3, 4]),
                ("*.TXT\n*.BAK", true, true, 1, vec![1, 2, 3, 4]),
                ("*.txt\n*.txt", false, true, 2, vec![2, 3, 4]),
                ("*.txt\n*.bak", false, true, 3, vec![3, 4]),
                ("\n\r\n", false, true, 1, vec![1, 2, 3, 4]),
                (" a.txt\n b.bak", false, true, 1, vec![1, 2, 3, 4]),
            ] {
                let keep = ContentKeep {
                    protected_glob: glob.into(),
                    protect_basename: basename,
                    protect_case_sensitive: case_sensitive,
                    count,
                    ..Default::default()
                };
                let mut selected = BTreeSet::from([4]);
                select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                    .unwrap();
                assert_eq!(selected.into_iter().collect::<Vec<_>>(), wanted, "{glob:?}");
            }
            for (glob, error) in [
                ("*.txt\n[".to_owned(), "Protection line 2:"),
                (format!("*.txt\n{}", "x".repeat(257)), "Protection line 2:"),
                (
                    std::iter::repeat_n("*.txt", 17)
                        .collect::<Vec<_>>()
                        .join("\n"),
                    "at most 16",
                ),
                ("\n".repeat(8193), "8192"),
            ] {
                let keep = ContentKeep {
                    protected_glob: glob,
                    ..Default::default()
                };
                let mut selected = BTreeSet::from([2, 4]);
                let actual =
                    select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                        .unwrap_err();
                assert!(actual.contains(error), "{actual}");
                assert_eq!(selected, BTreeSet::from([2, 4]));
            }
        }
        let keep = ContentKeep {
            protected_glob: std::iter::repeat_n("x".repeat(256), 16)
                .collect::<Vec<_>>()
                .join("\n"),
            ..Default::default()
        };
        assert_eq!(keep.protection().unwrap().len(), 16);
    }

    #[test]
    fn archive_protection_can_match_names_at_any_depth_with_explicit_letter_case() {
        use super::{ContentKeep, ContentKeepOrder, select_redundant_content_groups};
        use std::collections::BTreeSet;
        let mut listing = listing(&[
            "original.txt",
            "folder/original.txt",
            "folder/Original.TXT",
            "ёж/КОПИЯ.txt",
            "other",
            "unrelated",
        ]);
        for (row, member) in listing.members.iter_mut().enumerate() {
            member.index = [9, 2, 8, 4, 1, 99][row];
            member.size = 3;
        }
        let report = crate::archive_check::Report {
            kind: crate::archive::ArchiveKind::Tar,
            members: 6,
            decoded: 18,
            strict_headers: false,
            content_groups: Some(vec![crate::archive_check::ContentGroup {
                sha256: "hash".into(),
                size: 3,
                members: listing.members[..5]
                    .iter()
                    .map(|m| crate::archive_check::ContentMember {
                        index: m.index,
                        path: m.path.clone(),
                        size: m.size,
                    })
                    .collect(),
            }]),
        };
        for (glob, basename, case_sensitive, wanted) in [
            ("", true, false, vec![0, 1, 2, 3, 5]),
            ("original.txt", false, true, vec![1, 2, 3, 4, 5]),
            ("original.txt", true, true, vec![2, 3, 4, 5]),
            ("original.txt", true, false, vec![3, 4, 5]),
            ("original.txt", false, false, vec![1, 2, 3, 4, 5]),
            ("копия.TXT", true, false, vec![0, 1, 2, 4, 5]),
            ("копия.TXT", true, true, vec![0, 1, 2, 3, 5]),
            ("*.txt", false, true, vec![1, 2, 3, 4, 5]),
            ("*.txt", true, true, vec![2, 4, 5]),
            ("*.txt", true, false, vec![4, 5]),
            ("folder/*", false, true, vec![0, 3, 4, 5]),
            ("folder/*", true, true, vec![0, 1, 2, 3, 5]),
        ] {
            for group in [None, Some(0)] {
                let keep = ContentKeep {
                    count: 1,
                    order: ContentKeepOrder::First,
                    protected_glob: glob.into(),
                    protect_basename: basename,
                    protect_case_sensitive: case_sensitive,
                };
                let mut selected = BTreeSet::from([0, 1, 2, 3, 4, 5]);
                select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                    .unwrap();
                assert_eq!(
                    selected.into_iter().collect::<Vec<_>>(),
                    wanted,
                    "{glob}, basename={basename}, case={case_sensitive}"
                );
            }
        }
    }

    #[test]
    fn protected_archive_paths_are_always_kept_and_fill_the_total_minimum() {
        use super::{ContentKeep, ContentKeepOrder, select_redundant_content_groups};
        use std::collections::BTreeSet;
        let mut listing = listing(&[
            "keep/a",
            "keep/b",
            "other/c",
            "KEEP/d",
            "nested/ёж",
            "unrelated",
        ]);
        for (row, member) in listing.members.iter_mut().enumerate() {
            member.index = [9, 2, 8, 4, 1, 99][row];
            member.size = 3;
        }
        let mut report = crate::archive_check::Report {
            kind: crate::archive::ArchiveKind::Tar,
            members: 6,
            decoded: 18,
            strict_headers: false,
            content_groups: Some(vec![crate::archive_check::ContentGroup {
                sha256: "hash".into(),
                size: 3,
                members: listing.members[..5]
                    .iter()
                    .map(|m| crate::archive_check::ContentMember {
                        index: m.index,
                        path: m.path.clone(),
                        size: m.size,
                    })
                    .collect(),
            }]),
        };
        for (glob, count, wanted) in [
            ("keep/*", 1, vec![2, 3, 4, 5]),
            ("keep/*", 3, vec![2, 3, 5]),
            ("keep/*", usize::MAX, vec![5]),
            ("KEEP/*", 1, vec![0, 1, 2, 4, 5]),
            ("**", 1, vec![5]),
            ("*", 1, vec![0, 1, 2, 3, 5]),
            ("{keep,nested}/**", 1, vec![2, 3, 5]),
            ("keep/[ab]", 1, vec![2, 3, 4, 5]),
            ("nested/ёж", 1, vec![0, 1, 2, 3, 5]),
            ("", 1, vec![0, 1, 2, 3, 5]),
        ] {
            for group in [None, Some(0)] {
                let keep = ContentKeep {
                    count,
                    order: ContentKeepOrder::First,
                    protected_glob: glob.into(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                };
                let mut selected = BTreeSet::from([0, 1, 2, 3, 4, 5]);
                select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                    .unwrap();
                assert_eq!(
                    selected.into_iter().collect::<Vec<_>>(),
                    wanted,
                    "{glob} keep {count}"
                );
            }
        }
        for glob in ["[".to_owned(), "a".repeat(257)] {
            let mut selected = BTreeSet::from([2, 5]);
            assert!(
                select_redundant_content_groups(
                    &report,
                    &listing,
                    None,
                    &ContentKeep {
                        count: 1,
                        order: ContentKeepOrder::First,
                        protected_glob: glob,
                        protect_basename: false,
                        protect_case_sensitive: true,
                    },
                    &mut selected
                )
                .is_err()
            );
            assert_eq!(selected, BTreeSet::from([2, 5]));
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let invalid = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 0xff]));
            listing.members[0].path = invalid.clone();
            report.content_groups.as_mut().unwrap()[0].members[0].path = invalid;
            let mut selected = BTreeSet::from([2, 5]);
            assert!(
                select_redundant_content_groups(
                    &report,
                    &listing,
                    None,
                    &ContentKeep {
                        count: 1,
                        order: ContentKeepOrder::First,
                        protected_glob: "**".into(),
                        protect_basename: false,
                        protect_case_sensitive: true,
                    },
                    &mut selected
                )
                .is_err()
            );
            assert_eq!(selected, BTreeSet::from([2, 5]));
        }
        report.content_groups.as_mut().unwrap()[0].members[1].path = "stale".into();
        let mut selected = BTreeSet::from([2, 5]);
        assert!(
            select_redundant_content_groups(
                &report,
                &listing,
                None,
                &ContentKeep {
                    count: 1,
                    order: ContentKeepOrder::First,
                    protected_glob: "keep/*".into(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                },
                &mut selected
            )
            .is_err()
        );
        assert_eq!(selected, BTreeSet::from([2, 5]));
    }

    #[test]
    fn archive_depth_keepers_count_folders_and_preserve_stable_identity() {
        use super::{ContentKeep, ContentKeepOrder, select_redundant_content_groups};
        use std::collections::BTreeSet;
        assert_eq!(
            super::content_path_depth(std::path::Path::new("a///./b")),
            1
        );
        assert_eq!(super::content_path_depth(std::path::Path::new("./ёж")), 0);
        let long = "ёж".repeat(100);
        let mut listing = listing(&[
            &long,
            ".hidden/same",
            ".hidden/same",
            "deep/ёж/y",
            "x",
            "unrelated",
        ]);
        for (row, member) in listing.members.iter_mut().enumerate() {
            member.index = [9, 2, 8, 4, 1, 99][row];
            member.size = 3;
        }
        let mut report = crate::archive_check::Report {
            kind: crate::archive::ArchiveKind::Tar,
            members: 6,
            decoded: 18,
            strict_headers: false,
            content_groups: Some(vec![crate::archive_check::ContentGroup {
                sha256: "hash".into(),
                size: 3,
                members: listing.members[..5]
                    .iter()
                    .map(|m| crate::archive_check::ContentMember {
                        index: m.index,
                        path: m.path.clone(),
                        size: m.size,
                    })
                    .collect(),
            }]),
        };
        for (order, count, wanted) in [
            (ContentKeepOrder::Shallowest, 1, vec![0, 1, 2, 3, 5]),
            (ContentKeepOrder::Deepest, 1, vec![0, 1, 2, 4, 5]),
            (ContentKeepOrder::Shallowest, 2, vec![1, 2, 3, 5]),
            (ContentKeepOrder::Deepest, 2, vec![0, 2, 4, 5]),
            (ContentKeepOrder::Shallowest, 4, vec![3, 5]),
            (ContentKeepOrder::Deepest, 4, vec![0, 5]),
            (ContentKeepOrder::Deepest, usize::MAX, vec![5]),
        ] {
            for group in [None, Some(0)] {
                let mut selected = BTreeSet::from([0, 1, 2, 3, 4, 5]);
                select_redundant_content_groups(
                    &report,
                    &listing,
                    group,
                    &ContentKeep {
                        count,
                        order,
                        protected_glob: String::new(),
                        protect_basename: false,
                        protect_case_sensitive: true,
                    },
                    &mut selected,
                )
                .unwrap();
                assert_eq!(
                    selected.into_iter().collect::<Vec<_>>(),
                    wanted,
                    "{order:?}, keep {count}"
                );
            }
        }
        report.content_groups.as_mut().unwrap()[0].members[1].path = "stale".into();
        let mut selected = BTreeSet::from([2, 5]);
        assert!(
            select_redundant_content_groups(
                &report,
                &listing,
                None,
                &ContentKeep {
                    count: 1,
                    order: ContentKeepOrder::Deepest,
                    protected_glob: String::new(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                },
                &mut selected
            )
            .is_err()
        );
        assert_eq!(selected, BTreeSet::from([2, 5]));
    }

    #[test]
    fn archive_time_keepers_use_declared_dates_stable_ties_and_missing_last() {
        use super::{ContentKeep, ContentKeepOrder, select_redundant_content_groups};
        use std::collections::BTreeSet;
        use std::time::{Duration, UNIX_EPOCH};
        let mut listing = listing(&["same", "same", "c", "d", "e", "unrelated"]);
        for (row, member) in listing.members.iter_mut().enumerate() {
            member.index = [9, 2, 8, 4, 1, 99][row];
            member.size = 3;
            member.modified = [Some(10), Some(30), Some(30), None, None, Some(100)][row]
                .map(|s| UNIX_EPOCH + Duration::from_secs(s));
        }
        let mut report = crate::archive_check::Report {
            kind: crate::archive::ArchiveKind::Tar,
            members: 6,
            decoded: 18,
            strict_headers: false,
            content_groups: Some(vec![crate::archive_check::ContentGroup {
                sha256: "hash".into(),
                size: 3,
                members: listing.members[..5]
                    .iter()
                    .map(|m| crate::archive_check::ContentMember {
                        index: m.index,
                        path: m.path.clone(),
                        size: m.size,
                    })
                    .collect(),
            }]),
        };
        for (order, count, wanted) in [
            (ContentKeepOrder::Newest, 1, vec![0, 2, 3, 4, 5]),
            (ContentKeepOrder::Oldest, 1, vec![1, 2, 3, 4, 5]),
            (ContentKeepOrder::Newest, 2, vec![0, 3, 4, 5]),
            (ContentKeepOrder::Oldest, 2, vec![2, 3, 4, 5]),
            (ContentKeepOrder::Newest, 4, vec![3, 5]),
            (ContentKeepOrder::Oldest, 4, vec![3, 5]),
            (ContentKeepOrder::Newest, usize::MAX, vec![5]),
        ] {
            for group in [None, Some(0)] {
                let mut selected = BTreeSet::from([0, 1, 2, 3, 4, 5]);
                select_redundant_content_groups(
                    &report,
                    &listing,
                    group,
                    &ContentKeep {
                        count,
                        order,
                        protected_glob: String::new(),
                        protect_basename: false,
                        protect_case_sensitive: true,
                    },
                    &mut selected,
                )
                .unwrap();
                assert_eq!(
                    selected.into_iter().collect::<Vec<_>>(),
                    wanted,
                    "{order:?} count={count}"
                );
            }
        }
        listing.members[2].modified =
            Some(UNIX_EPOCH + Duration::from_secs(30) + Duration::from_nanos(1));
        let mut selected = BTreeSet::from([5]);
        select_redundant_content_groups(
            &report,
            &listing,
            None,
            &ContentKeep {
                count: 1,
                order: ContentKeepOrder::Newest,
                protected_glob: String::new(),
                protect_basename: false,
                protect_case_sensitive: true,
            },
            &mut selected,
        )
        .unwrap();
        assert_eq!(selected, BTreeSet::from([0, 1, 3, 4, 5]));
        listing.members[0].modified = Some(UNIX_EPOCH - Duration::from_secs(1));
        selected = BTreeSet::from([5]);
        select_redundant_content_groups(
            &report,
            &listing,
            None,
            &ContentKeep {
                count: 1,
                order: ContentKeepOrder::Oldest,
                protected_glob: String::new(),
                protect_basename: false,
                protect_case_sensitive: true,
            },
            &mut selected,
        )
        .unwrap();
        assert_eq!(selected, BTreeSet::from([1, 2, 3, 4, 5]));
        for member in &mut listing.members {
            member.modified = None;
        }
        for order in [ContentKeepOrder::Newest, ContentKeepOrder::Oldest] {
            selected = BTreeSet::from([5]);
            select_redundant_content_groups(
                &report,
                &listing,
                None,
                &ContentKeep {
                    count: 1,
                    order,
                    protected_glob: String::new(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                },
                &mut selected,
            )
            .unwrap();
            assert_eq!(selected, BTreeSet::from([0, 1, 2, 3, 5]));
        }
        report.content_groups.as_mut().unwrap()[0].members[1].path = "stale".into();
        selected = BTreeSet::from([2, 5]);
        assert!(
            select_redundant_content_groups(
                &report,
                &listing,
                None,
                &ContentKeep {
                    count: 1,
                    order: ContentKeepOrder::Newest,
                    protected_glob: String::new(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                },
                &mut selected
            )
            .is_err()
        );
        assert_eq!(selected, BTreeSet::from([2, 5]));
    }

    #[test]
    fn archive_replica_count_and_order_keep_exact_original_indices() {
        let mut listing = listing(&["dup", "dup", "a", "z", "unrelated"]);
        for (row, m) in listing.members.iter_mut().enumerate() {
            m.index = [8, 2, 5, 1, 99][row];
            m.size = 3;
        }
        let members = [0, 2, 3, 1]
            .into_iter()
            .map(|row| {
                let m = &listing.members[row];
                crate::archive_check::ContentMember {
                    index: m.index,
                    path: m.path.clone(),
                    size: m.size,
                }
            })
            .collect();
        let report = crate::archive_check::Report {
            kind: crate::archive::ArchiveKind::Tar,
            members: 5,
            decoded: 15,
            strict_headers: false,
            content_groups: Some(vec![crate::archive_check::ContentGroup {
                sha256: "hash".into(),
                size: 3,
                members,
            }]),
        };
        for (count, last, expected) in [
            (1, false, vec![0, 1, 2, 4]),
            (2, false, vec![0, 2, 4]),
            (2, true, vec![1, 3, 4]),
            (4, false, vec![4]),
            (usize::MAX, true, vec![4]),
        ] {
            let mut selected = std::collections::BTreeSet::from([0, 1, 2, 3, 4]);
            super::select_redundant_content_groups(
                &report,
                &listing,
                None,
                &super::ContentKeep {
                    count,
                    order: if last {
                        super::ContentKeepOrder::Last
                    } else {
                        super::ContentKeepOrder::First
                    },
                    protected_glob: String::new(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                },
                &mut selected,
            )
            .unwrap();
            assert_eq!(selected.into_iter().collect::<Vec<_>>(), expected);
        }
        let mut selected = std::collections::BTreeSet::from([4]);
        assert!(
            super::select_redundant_content_groups(
                &report,
                &listing,
                None,
                &super::ContentKeep {
                    count: 0,
                    order: super::ContentKeepOrder::First,
                    protected_glob: String::new(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                },
                &mut selected
            )
            .is_err()
        );
        assert_eq!(selected, std::collections::BTreeSet::from([4]));
    }

    #[test]
    fn redundant_archive_selection_preserves_lowest_original_index_and_refuses_stale_groups() {
        let mut listing = listing(&["same", "same", "empty-a", "empty-b", "unrelated"]);
        for (row, m) in listing.members.iter_mut().enumerate() {
            m.index = [9, 2, 20, 1, 99][row];
            m.size = if row < 2 { 3 } else { 0 };
        }
        let groups = vec![vec![0, 1], vec![2, 3]]
            .into_iter()
            .map(|rows| crate::archive_check::ContentGroup {
                sha256: "hash".into(),
                size: listing.members[rows[0]].size,
                members: rows
                    .into_iter()
                    .map(|row| {
                        let m = &listing.members[row];
                        crate::archive_check::ContentMember {
                            index: m.index,
                            path: m.path.clone(),
                            size: m.size,
                        }
                    })
                    .collect(),
            })
            .collect();
        let mut report = crate::archive_check::Report {
            kind: crate::archive::ArchiveKind::Tar,
            members: 5,
            decoded: 6,
            strict_headers: false,
            content_groups: Some(groups),
        };
        let mut selected = std::collections::BTreeSet::from([1, 3, 4]);
        super::select_redundant_content_groups(
            &report,
            &listing,
            Some(0),
            &super::ContentKeep::default(),
            &mut selected,
        )
        .unwrap();
        assert_eq!(selected, std::collections::BTreeSet::from([0, 3, 4]));
        super::select_redundant_content_groups(
            &report,
            &listing,
            None,
            &super::ContentKeep::default(),
            &mut selected,
        )
        .unwrap();
        assert_eq!(selected, std::collections::BTreeSet::from([0, 2, 4]));
        let before = selected.clone();
        assert!(
            super::select_redundant_content_groups(
                &report,
                &listing,
                Some(99),
                &super::ContentKeep::default(),
                &mut selected
            )
            .is_err()
        );
        listing.members[2].size = 42;
        assert!(
            super::select_redundant_content_groups(
                &report,
                &listing,
                None,
                &super::ContentKeep::default(),
                &mut selected
            )
            .is_err()
        );
        assert_eq!(selected, before);
        listing.members[2].size = 0;
        report.content_groups.as_mut().unwrap()[1].members[0].index = 9;
        report.content_groups.as_mut().unwrap()[1].members[0].path = "same".into();
        report.content_groups.as_mut().unwrap()[1].members[0].size = 3;
        assert!(
            super::select_redundant_content_groups(
                &report,
                &listing,
                None,
                &super::ContentKeep::default(),
                &mut selected
            )
            .is_err()
        );
        assert_eq!(selected, before);
    }

    #[test]
    fn content_member_pages_are_independent_and_keep_original_entry_order() {
        let ctx = egui::Context::default();
        let mut pages = std::collections::BTreeMap::<usize, usize>::new();
        let mut frame = |events| {
            let mut ranges = Vec::new();
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    for rank in [0, 100] {
                        let range = ui
                            .push_id(("archive-group-members", rank), |ui| {
                                super::content_pager(
                                    ui,
                                    21,
                                    pages.entry(rank).or_default(),
                                    super::CONTENT_MEMBERS_PER_PAGE,
                                    ["Previous members", "Next members", "Members"],
                                )
                            })
                            .inner;
                        ranges.push(range);
                    }
                },
            ));
            ranges
        };
        assert_eq!(frame(vec![]), [0..10, 0..10]);
        // The recorded rect belongs to group 100, which renders last.
        for expected in [10..20, 20..21, 20..21] {
            let pos = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("Next members")))
                .unwrap()
                .center();
            frame(vec![egui::Event::PointerMoved(pos)]);
            frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }]);
            assert_eq!(
                frame(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default()
                }]),
                [0..10, expected]
            );
        }
        assert_eq!(pages.get(&0), Some(&0));
        assert_eq!(pages.get(&100), Some(&2));
        let entries = ["duplicate-name"; 21];
        let mut page = 2;
        let range = super::bounded_page_range(entries.len(), &mut page, 10);
        assert_eq!(
            entries
                .iter()
                .enumerate()
                .skip(range.start)
                .take(range.len())
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
            [20]
        );
        let mut stale = usize::MAX;
        assert_eq!(super::bounded_page_range(0, &mut stale, 10), 0..0);
        assert_eq!(stale, 0);
    }

    #[test]
    fn content_group_page_buttons_reach_final_group_and_return() {
        let ctx = egui::Context::default();
        let mut page = 0;
        let mut frame = |events| {
            let mut range = 0..0;
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    range = super::content_group_pager(ui, 201, &mut page);
                },
            ));
            range
        };
        assert_eq!(frame(vec![]), 0..100);
        for (label, expected) in [
            ("Next groups", 100..200),
            ("Next groups", 200..201),
            ("Next groups", 200..201),
            ("Previous groups", 100..200),
            ("Previous groups", 0..100),
            ("Previous groups", 0..100),
        ] {
            let pos = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(label)))
                .unwrap()
                .center();
            frame(vec![egui::Event::PointerMoved(pos)]);
            frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }]);
            assert_eq!(
                frame(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default()
                }]),
                expected
            );
        }
    }

    #[test]
    fn content_group_pages_cover_all_absolute_ranks_and_clamp_stale_state() {
        for count in [0usize, 1, 99, 100, 101, 200, 201, 10_001] {
            let mut visited = Vec::new();
            for requested in 0..count.div_ceil(super::CONTENT_GROUPS_PER_PAGE).max(1) {
                let mut page = requested;
                let range = super::content_group_page_range(count, &mut page);
                assert!(range.len() <= super::CONTENT_GROUPS_PER_PAGE);
                assert_eq!(page, requested);
                visited.extend(range);
            }
            assert_eq!(visited, (0..count).collect::<Vec<_>>());
            let mut page = usize::MAX;
            let range = super::content_group_page_range(count, &mut page);
            assert_eq!(range.end, count);
            assert!(range.len() <= super::CONTENT_GROUPS_PER_PAGE);
        }
        let mut page = usize::MAX;
        let range = super::content_group_page_range(usize::MAX, &mut page);
        assert_eq!(range.end, usize::MAX);
        assert!(range.len() <= super::CONTENT_GROUPS_PER_PAGE);
    }

    use super::{
        MemberFilter, extract_all_label, extract_error_summary, extract_selected_label,
        step_member_cursor,
    };

    fn listing(paths: &[&str]) -> crate::archive::ArchiveListing {
        crate::archive::ArchiveListing {
            tree: Err("Not indexed".into()),
            kind: crate::archive::ArchiveKind::Zip,
            members: paths
                .iter()
                .enumerate()
                .map(|(index, path)| crate::archive::ArchiveMember {
                    index,
                    path: std::path::PathBuf::from(path),
                    is_dir: false,
                    is_regular: true,
                    unix_mode: None,
                    owner_uid: None,
                    owner_gid: None,
                    file_type_bits: 0,
                    is_symlink: false,
                    is_executable: Some(false),
                    size: 1,
                    compressed_size: 1,
                    zip_info: None,
                    tar_name: None,
                    modified: None,
                })
                .collect(),
            declared_members: paths.len(),
            declared_uncompressed_bytes: paths.len() as u128,
            unsafe_members: 0,
            unreadable_members: 0,
            truncated: false,
        }
    }

    #[test]
    fn password_widget_masks_text_and_discards_undo_snapshots() {
        let ctx = egui::Context::default();
        let mut password = zeroize::Zeroizing::new("private-fixture-password".to_string());
        let mut id = None;
        let output =
            crate::testutil::take_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
                id = Some(super::archive_password_input(ui, &mut password).id);
            }));
        assert!(!format!("{:?}", output.shapes).contains("private-fixture-password"));
        let state = egui::text_edit::TextEditState::load(&ctx, id.unwrap()).unwrap();
        let current = (egui::text::CCursorRange::default(), String::new());
        assert!(!state.undoer().has_undo(&current));
        assert!(!state.undoer().has_redo(&current));
    }

    #[test]
    fn archive_copy_visible_omits_navigation_and_selected_keeps_hidden_sort_order() {
        let mut listing = listing(&[
            "hidden/file.txt",
            "src/file10.rs",
            "src/file2.rs",
            "src/deep/x",
        ]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let selected = [0, 1, 2, 100].into_iter().collect();
        let mut filter = MemberFilter::default();
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "");
        assert_eq!(filter.copy_rows(&listing, &selected, false), [2, 1]);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 2, 1]);
        filter.order.descending = true;
        filter.visible(&listing, "file2");
        assert_eq!(filter.copy_rows(&listing, &selected, false), [2]);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [1, 2, 0]);
        assert_eq!(selected, [0, 1, 2, 100].into_iter().collect());
    }

    #[test]
    fn selecting_folder_contents_includes_only_real_subtree_members() {
        let listing = listing(&["a/deep/file", "a/direct", "a-not/sibling", "root"]);
        let mut selected = [3].into_iter().collect();
        super::select_folder_contents(&listing, std::path::Path::new("a"), &mut selected);
        assert_eq!(selected, [0, 1, 3].into_iter().collect());
        super::select_folder_contents(&listing, std::path::Path::new(""), &mut selected);
        assert_eq!(selected, [0, 1, 2, 3].into_iter().collect());
    }

    #[test]
    fn archive_folder_navigation_preserves_member_ids_and_hidden_selection() {
        let mut listing = listing(&[
            "a/deep/child.txt",
            "a/one.txt",
            "a/one.txt",
            "root.txt",
            "b/two.txt",
        ]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let tree = listing.tree.as_ref().unwrap();
        let mut filter = MemberFilter::default();
        let selected: std::collections::BTreeSet<_> = [0, 2, 4].into_iter().collect();
        filter.navigate(Some(Default::default()));
        let root = filter.visible(&listing, "").to_vec();
        assert_eq!(root.len(), 3, "two implicit folders and one root file");
        assert!(
            root[..2]
                .iter()
                .all(|&index| index >= listing.members.len())
        );
        assert_eq!(root[2], 3);
        filter.navigate(Some("a".into()));
        let a = filter.visible(&listing, "").to_vec();
        assert_eq!(a.len(), 3);
        assert_eq!(
            tree.directories[a[0] - listing.members.len()].path,
            std::path::Path::new("a/deep")
        );
        assert_eq!(&a[1..], &[1, 2], "duplicate archive members stay distinct");
        assert_eq!(
            selected
                .iter()
                .filter(|index| !filter.visible_set.contains(index))
                .count(),
            2
        );
        filter.order.choose(crate::archive_order::Field::Size);
        filter.visible(&listing, "one");
        assert_eq!(filter.visible_set, [1, 2].into_iter().collect());
        filter.navigate(Some("a/deep".into()));
        assert_eq!(filter.visible(&listing, ""), [0]);
        filter.navigate(None);
        assert_eq!(filter.visible(&listing, "").len(), 5);
        assert_eq!(selected, [0, 2, 4].into_iter().collect());
        assert!(filter.visible(&listing, "missing").is_empty());
    }

    #[test]
    fn natural_toggle_reorders_cached_members_without_changing_focus_or_selection() {
        let listing = listing(&["file10", "file2", "file02", "file2"]);
        let mut filter = MemberFilter::default();
        let selected: std::collections::BTreeSet<_> = [0, 3].into_iter().collect();
        assert_eq!(filter.visible(&listing, ""), [1, 3, 2, 0]);
        filter.focused = Some(0);
        filter.order.natural = false;
        assert_eq!(filter.visible(&listing, ""), [2, 0, 1, 3]);
        assert_eq!(filter.focused, Some(0));
        assert!(filter.scroll_to_focused);
        assert_eq!(filter.visible_set, [0, 1, 2, 3].into_iter().collect());
        filter.visible(&listing, "2");
        filter.order.natural = true;
        assert_eq!(filter.visible(&listing, "2"), [1, 3, 2]);
        assert_eq!(
            selected
                .iter()
                .filter(|index| !filter.visible_set.contains(index))
                .count(),
            1
        );
    }

    #[test]
    fn navigation_folders_share_natural_name_order_and_reversal() {
        let mut listing = listing(&["folder10/file", "folder2/file"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.navigate(Some(Default::default()));
        assert_eq!(filter.visible(&listing, ""), [3, 2]);
        filter.order.descending = true;
        assert_eq!(filter.visible(&listing, ""), [2, 3]);
        filter.order.descending = false;
        filter.order.natural = false;
        assert_eq!(filter.visible(&listing, ""), [2, 3]);
        assert!(
            filter.visible_set.is_empty(),
            "navigation IDs are never extraction IDs"
        );
    }

    #[test]
    fn archive_member_details_distinguish_zero_false_and_unavailable_metadata() {
        let mut listing = listing(&["file"]);
        let member = &mut listing.members[0];
        member.size = 0;
        member.compressed_size = 0;
        member.unix_mode = Some(0);
        member.is_executable = Some(false);
        member.modified = Some(std::time::UNIX_EPOCH);
        let fields = super::member_details(crate::archive::ArchiveKind::Zip, member);
        assert_eq!(
            fields.map(|(_, value)| value),
            [
                "Regular file",
                "0 bytes",
                "0 bytes",
                "1970-01-01T00:00:00Z",
                "0000",
                "No",
                "Unavailable (not recorded per member)",
                "Unavailable (not recorded per member)",
                "Unavailable (not recorded per member)"
            ]
        );
        member.unix_mode = None;
        member.is_executable = None;
        member.modified = None;
        let fields = super::member_details(crate::archive::ArchiveKind::TarXz, member);
        assert_eq!(fields[2].1, "Unavailable");
        assert_eq!(fields[3].1, "Unavailable");
        assert_eq!(fields[4].1, "Unavailable");
        assert_eq!(fields[5].1, "Unavailable");
    }

    #[test]
    fn archive_details_show_zip_method_and_password_requirement() {
        let mut listing = listing(&["file"]);
        let member = &mut listing.members[0];
        member.zip_info = Some(crate::archive::ZipMemberInfo {
            compression: "Deflated".into(),
            encrypted: true,
            encryption: "ZipCrypto".into(),
            crc32: Some(0x1234abcd),
        });
        let fields = super::member_details(crate::archive::ArchiveKind::Zip, member);
        assert_eq!(fields[6], ("Compression method", "Deflated".into()));
        assert_eq!(fields[8], ("Declared CRC-32", "1234ABCD".into()));
        assert_eq!(
            fields[7],
            ("Encrypted", "Yes (ZipCrypto, password required)".into())
        );
        member.zip_info.as_mut().unwrap().encrypted = false;
        assert_eq!(
            super::member_details(crate::archive::ArchiveKind::Zip, member)[7].1,
            "No"
        );
    }

    #[test]
    fn archive_member_details_preserve_special_permission_bits_and_record_types() {
        let mut listing = listing(&["entry"]);
        let member = &mut listing.members[0];
        member.unix_mode = Some(0o107755);
        member.is_executable = Some(true);
        let fields = super::member_details(crate::archive::ArchiveKind::Zip, member);
        assert_eq!(fields[4].1, "7755");
        assert_eq!(fields[5].1, "Yes");
        member.is_regular = false;
        member.is_symlink = true;
        assert_eq!(
            super::member_details(crate::archive::ArchiveKind::Tar, member)[0].1,
            "Symbolic link"
        );
        member.is_symlink = false;
        assert_eq!(
            super::member_details(crate::archive::ArchiveKind::Tar, member)[0].1,
            "Other archive record"
        );
        member.is_dir = true;
        let fields = super::member_details(crate::archive::ArchiveKind::Zip, member);
        assert_eq!(fields[0].1, "Directory");
        assert_eq!(fields[1].1, "Not applicable (directory)");
        assert_eq!(fields[2].1, "Unavailable");
    }

    #[test]
    fn archive_reset_filters_restores_folder_rows_without_resetting_sort_or_selection_memory() {
        let mut listing = listing(&["src/a.txt", "src/b.rs", "other/c.txt"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        let selected = [0, 2].into_iter().collect();
        filter
            .selection_memory
            .save(&selected, listing.members.len());
        filter.navigate(Some("src".into()));
        filter.narrow(&selected, listing.members.len());
        filter.navigate(Some("src".into()));
        filter.order.descending = true;
        filter.options.regex = true;
        filter.options.case_sensitive = true;
        filter.options.smart_case = true;
        filter.exclude_matches = true;
        filter.kinds.files = false;
        let mut query = "[".to_owned();
        assert!(filter.visible(&listing, &query).is_empty());
        assert!(filter.has_filters(&query));
        let history_len = filter.past.len();
        filter.reset_filters(&mut query);
        assert!(!filter.has_filters(&query));
        assert_eq!(filter.visible(&listing, &query), [1, 0]);
        assert!(filter.pattern.as_ref().unwrap().is_ok());
        assert_eq!(filter.folder, Some("src".into()));
        assert!(filter.order.descending);
        assert_eq!(filter.past.len(), history_len);
        let mut restored = Default::default();
        filter
            .selection_memory
            .restore(&mut restored, listing.members.len());
        assert_eq!(restored, selected);
        filter.reset_filters(&mut query);
        assert_eq!(filter.visible(&listing, &query), [1, 0]);
    }

    #[test]
    fn archive_reset_filters_rebuilds_even_when_query_and_pattern_options_were_default() {
        let listing = listing(&["a", "b"]);
        let mut filter = MemberFilter::default();
        assert!(!filter.has_filters(""));
        assert!(filter.has_filters(" "));
        filter.visible(&listing, "");
        filter.narrow(&[0].into_iter().collect(), listing.members.len());
        assert_eq!(filter.visible(&listing, ""), [0]);
        filter.reset_filters(&mut String::new());
        assert_eq!(filter.visible(&listing, ""), [0, 1]);
        filter.exclude_matches = true;
        assert!(filter.visible(&listing, "").is_empty());
        filter.reset_filters(&mut String::new());
        assert_eq!(filter.visible(&listing, ""), [0, 1]);
    }

    #[test]
    fn archive_exclusion_inverts_valid_patterns_but_preserves_errors_and_selection() {
        let listing = listing(&["src/A.TXT", "src/b.rs", "src/b.rs", "other/c.txt"]);
        let mut filter = MemberFilter::default();
        assert_eq!(filter.visible(&listing, ".rs"), [1, 2]);
        filter.focused = Some(1);
        let selected = [1, 3].into_iter().collect();
        filter.exclude_matches = true;
        assert_eq!(filter.visible(&listing, ".rs"), [3, 0]);
        assert_eq!(filter.focused, None);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [3, 1]);
        assert!(filter.visible(&listing, "").is_empty());
        filter.options.regex = true;
        assert_eq!(filter.visible(&listing, r"\.rs$"), [3, 0]);
        assert!(filter.visible(&listing, "[").is_empty());
        assert!(filter.pattern.as_ref().unwrap().is_err());
        filter.exclude_matches = false;
        assert!(filter.visible(&listing, "[").is_empty());
        filter.exclude_matches = true;
        filter.options.smart_case = true;
        assert_eq!(filter.visible(&listing, "TXT$"), [3, 1, 2]);
        assert_eq!(selected, [1, 3].into_iter().collect());
        assert!(!MemberFilter::default().exclude_matches);
    }

    #[test]
    fn archive_exclusion_applies_to_navigation_and_intersects_type_and_snapshot() {
        let mut listing = listing(&["src/a.txt", "docs/readme.txt", "src/b.rs"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter {
            exclude_matches: true,
            ..Default::default()
        };
        filter.navigate(Some(Default::default()));
        let visible = filter.visible(&listing, "docs").to_vec();
        assert_eq!(visible.len(), 1);
        let tree = listing.tree.as_ref().unwrap();
        assert_eq!(
            tree.directories[visible[0] - listing.members.len()].path,
            std::path::Path::new("src")
        );
        assert!(filter.visible_set.is_empty());
        filter.navigate(Some("src".into()));
        assert_eq!(filter.visible(&listing, ".rs"), [0]);
        filter.narrow(&[0, 2].into_iter().collect(), listing.members.len());
        assert_eq!(filter.visible(&listing, ".rs"), [0]);
        filter.kinds.files = false;
        assert!(filter.visible(&listing, ".rs").is_empty());
    }

    #[test]
    fn archive_selection_stats_count_distinct_records_without_directory_or_link_payloads() {
        let mut listing = listing(&["same", "same", "folder", "link", "fifo"]);
        listing.members[0].size = u64::MAX;
        listing.members[1].size = u64::MAX;
        listing.members[0].compressed_size = u64::MAX;
        listing.members[1].compressed_size = 1;
        listing.members[2].is_dir = true;
        listing.members[2].is_regular = false;
        listing.members[3].is_symlink = true;
        listing.members[3].is_regular = false;
        listing.members[4].is_regular = false;
        let selected = [0, 1, 2, 3, 4, 99].into_iter().collect();
        let stats = super::SelectionStats::compute(&listing, &selected);
        assert_eq!(
            (stats.files, stats.folders, stats.links, stats.other),
            (2, 1, 1, 1)
        );
        assert_eq!(stats.unpacked, u128::from(u64::MAX) * 2);
        assert_eq!(stats.packed, Some(u128::from(u64::MAX) + 1));
        assert!(
            stats
                .label()
                .contains("36893488147419103230 unpacked bytes")
        );
        listing.kind = crate::archive::ArchiveKind::Tar;
        let tar_stats = super::SelectionStats::compute(&listing, &selected);
        assert_eq!(tar_stats.packed, None);
        assert!(tar_stats.label().contains("packed size unavailable"));
    }

    #[test]
    fn archive_selection_stats_cache_tracks_replacement_and_retains_hidden_members() {
        let listing = listing(&["src/a", "other/b"]);
        let mut filter = MemberFilter::default();
        let mut selected = [0, 1].into_iter().collect();
        assert_eq!(filter.selection_stats.get(&listing, &selected).unpacked, 2);
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "a");
        assert_eq!(filter.selection_stats.get(&listing, &selected).files, 2);
        assert_eq!(filter.selection_stats.ids, [0, 1]);
        selected = [1].into_iter().collect();
        assert_eq!(filter.selection_stats.get(&listing, &selected).files, 1);
        assert_eq!(filter.selection_stats.ids, [1]);
        selected.clear();
        assert_eq!(filter.selection_stats.get(&listing, &selected).unpacked, 0);
        assert_eq!(
            filter.selection_stats.get(&listing, &selected).packed,
            Some(0)
        );
        assert!(filter.selection_stats.ids.is_empty());
        assert!(!MemberFilter::default().selection_stats.initialized);
    }

    #[test]
    fn archive_type_filter_rebuilds_cached_rows_preserving_selection_and_exports() {
        let mut listing = listing(&["file.txt", "dir", "link", "fifo"]);
        listing.members[1].is_dir = true;
        listing.members[1].is_regular = false;
        listing.members[2].is_symlink = true;
        listing.members[2].is_regular = false;
        listing.members[3].is_regular = false;
        let mut filter = MemberFilter::default();
        assert_eq!(filter.visible(&listing, "").len(), 4);
        filter.focused = Some(0);
        let selected = [0, 2].into_iter().collect();
        filter.kinds.files = false;
        assert_eq!(filter.visible(&listing, ""), [1, 3, 2]);
        assert_eq!(filter.focused, None);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 2]);
        assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 3, 2]);
        filter.kinds.folders = false;
        filter.kinds.other = false;
        assert_eq!(filter.visible(&listing, ""), [2]);
        assert!(filter.visible(&listing, "txt").is_empty());
        filter.kinds.files = true;
        assert_eq!(filter.visible(&listing, "txt"), [0]);
        filter.kinds.links = false;
        filter.kinds.files = false;
        assert!(filter.visible(&listing, "").is_empty());
        filter.kinds = Default::default();
        assert_eq!(filter.visible(&listing, "").len(), 4);
        assert_eq!(selected, [0, 2].into_iter().collect());
    }

    #[test]
    fn archive_exclusion_glob_intersects_query_and_excludes_directory_descendants() {
        let mut listing = listing(&[
            "src/main.rs",
            "src/main.rs",
            "src/main.rs.bak",
            "build/main.rs",
            "build/deep/a.txt",
            "src/FILE.TMP",
            "other/main.rs",
            "src/a.txt",
        ]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.exclusion.settings.query = "{*.bak,*.tmp,build}".into();
        let mut actual = filter.visible(&listing, "main").to_vec();
        actual.sort_unstable();
        assert_eq!(actual, [0, 1, 6]);
        let selected = [2, 4, 5].into_iter().collect();
        assert_eq!(filter.copy_rows(&listing, &selected, true).len(), 3);
        filter.navigate(Some(std::path::PathBuf::new()));
        filter.visible(&listing, "");
        let tree = listing.tree.as_ref().unwrap();
        let build = tree
            .directories
            .iter()
            .position(|d| d.path == std::path::Path::new("build"))
            .unwrap()
            + listing.members.len();
        assert!(filter.row_position(build).is_none());
        filter.navigate(Some("build/deep".into()));
        assert!(filter.visible(&listing, "").is_empty());
        filter.navigate(None);
        filter.exclusion.settings = super::ExclusionSettings {
            query: "src/main.rs".into(),
            full_path: true,
        };
        let mut actual = filter.visible(&listing, "main").to_vec();
        actual.sort_unstable();
        assert_eq!(actual, [2, 3, 6]);
        filter.exclusion.settings.query = "src".into();
        let mut actual = filter.visible(&listing, "").to_vec();
        actual.sort_unstable();
        assert_eq!(actual, [3, 4, 6]);
        assert!(filter.save_filter(""));
        let saved = filter.saved_filters[0].clone();
        assert!(saved.label().contains("Exclude glob (Full path): src"));
        let mut query = String::new();
        filter.reset_filters(&mut query);
        assert_eq!(filter.visible(&listing, "").len(), 8);
        filter.restore_filter(saved, &mut query);
        assert_eq!(filter.visible(&listing, "").len(), 3);
        filter.exclusion.settings.query = "[".into();
        assert!(filter.visible(&listing, "").is_empty());
        assert!(!filter.save_filter(""));
        filter.exclusion.settings.query = "x".repeat(257);
        assert!(filter.visible(&listing, "").is_empty());
        filter.reset_filters(&mut query);
        assert!(!filter.has_filters(""));
    }

    #[test]
    fn archive_exclusion_glob_pointer_entry_preserves_active_query_and_selection() {
        let listing = listing(&["src/a.txt", "src/a.bak", "elsewhere/a.txt"]);
        let mut filter = MemberFilter::default();
        let selected = [1].into_iter().collect();
        let ctx = egui::Context::default();
        for frame in 0..9 {
            let mut input = egui::RawInput {
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4].contains(&frame) {
                let rect = ctx
                    .data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new("archive-exclusion-query-rect"))
                    })
                    .unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 3,
                        modifiers: Default::default(),
                    },
                ];
            } else if frame == 5 {
                input.events.push(egui::Event::Text("*.bak".into()));
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                filter.exclusion.ui(ui, true);
            }));
            filter.visible(&listing, "src/");
        }
        assert_eq!(filter.exclusion.settings.query, "*.bak");
        assert_eq!(filter.needle.as_deref(), Some("src/"));
        assert_eq!(filter.visible, [0]);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [1]);
        assert!(filter.save_filter("src/"));
        filter.exclusion.settings.full_path = true;
        assert!(filter.save_filter("src/"));
        assert_eq!(filter.saved_filters.len(), 2);
    }

    #[test]
    fn archive_dotfile_filter_hides_components_navigation_and_retains_selected_snapshots() {
        let mut listing = listing(&[
            ".env",
            ".git/config",
            "src/.cache/data",
            "src/a.txt",
            "src/a.txt",
            "src/link",
            "ordinary.name",
            "src/.秘密/file",
        ]);
        listing.members[5].is_symlink = true;
        listing.members[5].is_regular = false;
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.kinds.hide_dotfiles = true;
        assert_eq!(
            filter
                .visible(&listing, "")
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            [3, 4, 5, 6].into_iter().collect()
        );
        assert!(filter.save_filter(""));
        let saved = filter.saved_filters[0].clone();
        assert!(saved.label().contains("Hide dotfiles"));
        filter.navigate(Some("src".into()));
        assert_eq!(filter.visible(&listing, ""), [3, 4, 5]);
        let selected = [0, 2, 4].into_iter().collect();
        let copied = filter.copy_rows(&listing, &selected, true);
        assert_eq!(
            copied
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            selected
        );
        filter.kinds.links = false;
        assert_eq!(filter.visible(&listing, ""), [3, 4]);
        filter.navigate(Some(".git".into()));
        assert!(filter.visible(&listing, "").is_empty());
        let mut query = String::new();
        filter.reset_filters(&mut query);
        assert_eq!(filter.visible(&listing, ""), [1]);
        filter.restore_filter(saved, &mut query);
        assert!(filter.visible(&listing, "").is_empty());
        filter.navigate(Some("src".into()));
        assert_eq!(filter.visible(&listing, "a.txt"), [3, 4]);
        filter.kinds.hide_dotfiles = false;
        filter.visible(&listing, "");
        assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
        assert_eq!(filter.saved_filters.len(), 1);
    }

    #[test]
    fn archive_dotfile_checkbox_pointer_changes_view_without_deselecting() {
        let listing = listing(&[".env", "visible", "dir/.hidden/item"]);
        let selected: std::collections::BTreeSet<_> = [0, 2].into_iter().collect();
        let mut filter = MemberFilter::default();
        let ctx = egui::Context::default();
        for frame in 0..9 {
            let mut input = egui::RawInput {
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4, 6, 7].contains(&frame) {
                let rect = ctx
                    .data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new("archive-hide-dotfiles"))
                    })
                    .unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 3 || frame == 6,
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                super::hidden_entries_ui(ui, &mut filter.kinds, true);
            }));
            let visible = filter.visible(&listing, "");
            if (4..7).contains(&frame) {
                assert_eq!(visible, [1]);
            } else if frame >= 7 {
                assert_eq!(visible.len(), 3);
            }
            assert_eq!(filter.copy_rows(&listing, &selected, true).len(), 2);
        }
        assert!(!filter.kinds.hide_dotfiles);
        assert!(!crate::archive_filter::hidden_path(std::path::Path::new(
            "ordinary.name/./file"
        )));
        assert!(crate::archive_filter::hidden_path(std::path::Path::new(
            "dir/.hidden/file"
        )));
    }

    #[test]
    fn archive_type_filter_keeps_navigation_but_intersects_narrowing_and_query() {
        let mut listing = listing(&["src/file.txt", "src/link", "src/fifo"]);
        listing.members[1].is_symlink = true;
        listing.members[1].is_regular = false;
        listing.members[2].is_regular = false;
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter {
            kinds: crate::archive_filter::Kinds {
                hide_dotfiles: false,
                executable_only: false,
                files: false,
                folders: false,
                links: true,
                other: false,
            },
            ..Default::default()
        };
        filter.navigate(Some(Default::default()));
        assert_eq!(filter.visible(&listing, "").len(), 1);
        assert!(filter.visible_set.is_empty());
        filter.navigate(Some("src".into()));
        assert_eq!(filter.visible(&listing, ""), [1]);
        filter.narrow(&[0, 2].into_iter().collect(), listing.members.len());
        assert!(filter.visible(&listing, "").is_empty());
        filter.kinds.other = true;
        assert_eq!(filter.visible(&listing, ""), [2]);
        filter.options.regex = true;
        assert!(filter.visible(&listing, "[").is_empty());
        filter.unnarrow();
        assert_eq!(filter.visible(&listing, ""), [2, 1]);
        assert_eq!(MemberFilter::default().kinds, Default::default());
    }

    #[test]
    fn archive_narrow_snapshot_keeps_hidden_duplicates_and_survives_selection_changes() {
        let mut listing = listing(&["src/a.txt", "src/a.txt", "other/b.txt", "other/c.txt"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "a");
        let mut selected = [1, 2, 99].into_iter().collect();
        filter.narrow(&selected, listing.members.len());
        assert_eq!(filter.folder, None);
        assert_eq!(filter.visible(&listing, "a"), [1]);
        assert_eq!(filter.visible(&listing, ""), [2, 1]);
        selected.remove(&1);
        assert_eq!(filter.visible(&listing, ""), [2, 1]);
        filter.order.descending = true;
        assert_eq!(filter.visible(&listing, ""), [1, 2]);
        filter.narrow(&selected, listing.members.len());
        assert_eq!(filter.visible(&listing, ""), [2]);
        filter.unnarrow();
        assert_eq!(filter.visible(&listing, "").len(), 4);
        assert_eq!(selected, [2, 99].into_iter().collect());
    }

    #[test]
    fn archive_narrow_empty_reset_and_folder_filter_never_admit_navigation_rows() {
        let mut listing = listing(&["src/a.txt", "other/b.txt"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.narrow(&[0].into_iter().collect(), listing.members.len());
        filter.navigate(Some(Default::default()));
        assert!(filter.visible(&listing, "").is_empty());
        filter.navigate(Some("src".into()));
        assert_eq!(filter.visible(&listing, ""), [0]);
        filter.options.regex = true;
        assert!(filter.visible(&listing, "[").is_empty());
        filter.narrow(&[99].into_iter().collect(), listing.members.len());
        assert!(filter.narrowed.is_none());
        filter.navigate(None);
        assert_eq!(filter.visible(&listing, "").len(), 2);
        assert!(MemberFilter::default().narrowed.is_none());
    }

    #[test]
    fn scanned_content_groups_match_original_index_path_size_before_selection() {
        let mut listing = listing(&["same", "same", "other"]);
        for (index, member) in listing.members.iter_mut().enumerate() {
            member.index = index;
            member.size = 3;
        }
        let report = crate::archive_check::Report {
            kind: crate::archive::ArchiveKind::Zip,
            members: 3,
            decoded: 9,
            strict_headers: false,
            content_groups: Some(vec![crate::archive_check::ContentGroup {
                sha256: "abc".into(),
                size: 3,
                members: vec![
                    crate::archive_check::ContentMember {
                        index: 0,
                        path: "same".into(),
                        size: 3,
                    },
                    crate::archive_check::ContentMember {
                        index: 1,
                        path: "same".into(),
                        size: 3,
                    },
                ],
            }]),
        };
        assert_eq!(
            super::content_group_rows(&report, &listing).unwrap(),
            [0, 1]
        );
        listing.members[1].size = 4;
        assert!(super::content_group_rows(&report, &listing).is_err());
        listing.members[1].size = 3;
        listing.members[1].path = "changed".into();
        assert!(super::content_group_rows(&report, &listing).is_err());
        listing.members[1].path = "same".into();
        listing.members[1].index = 0;
        assert!(super::content_group_rows(&report, &listing).is_err());
    }

    #[test]
    fn content_group_selection_changes_only_one_group_and_refuses_atomically() {
        let mut listing = listing(&["dup", "dup", "empty-one", "empty-two", "unrelated"]);
        for (index, m) in listing.members.iter_mut().enumerate() {
            m.index = index;
            m.size = if matches!(index, 2 | 3) { 0 } else { 3 };
        }
        let mut report = crate::archive_check::Report {
            kind: crate::archive::ArchiveKind::Tar,
            members: 5,
            decoded: 9,
            strict_headers: false,
            content_groups: Some(vec![
                crate::archive_check::ContentGroup {
                    sha256: "abc".into(),
                    size: 3,
                    members: vec![
                        crate::archive_check::ContentMember {
                            index: 0,
                            path: "dup".into(),
                            size: 3,
                        },
                        crate::archive_check::ContentMember {
                            index: 1,
                            path: "dup".into(),
                            size: 3,
                        },
                    ],
                },
                crate::archive_check::ContentGroup {
                    sha256: "empty".into(),
                    size: 0,
                    members: vec![
                        crate::archive_check::ContentMember {
                            index: 2,
                            path: "empty-one".into(),
                            size: 0,
                        },
                        crate::archive_check::ContentMember {
                            index: 3,
                            path: "empty-two".into(),
                            size: 0,
                        },
                    ],
                },
            ]),
        };
        let mut selected = std::collections::BTreeSet::from([4]);
        for (group, select, expected) in [
            (Some(0), true, vec![0, 1, 4]),
            (Some(1), true, vec![0, 1, 2, 3, 4]),
            (Some(0), false, vec![2, 3, 4]),
            (Some(1), false, vec![4]),
            (None, true, vec![0, 1, 2, 3, 4]),
        ] {
            super::change_content_group_selection(&report, &listing, group, select, &mut selected)
                .unwrap();
            assert_eq!(selected.iter().copied().collect::<Vec<_>>(), expected);
        }
        let before = selected.clone();
        assert!(
            super::change_content_group_selection(
                &report,
                &listing,
                Some(99),
                false,
                &mut selected
            )
            .is_err()
        );
        assert_eq!(selected, before);
        report.content_groups.as_mut().unwrap()[1].members[1].index = 99;
        assert!(
            super::change_content_group_selection(&report, &listing, None, false, &mut selected)
                .is_err()
        );
        assert_eq!(selected, before);
        assert!(
            super::change_content_group_selection(&report, &listing, Some(1), false, &mut selected)
                .is_err()
        );
        assert_eq!(selected, before);
    }

    #[test]
    fn archive_repeated_crc_candidates_preserve_distinct_ids_zero_and_visible_scope() {
        let mut listing = listing(&[
            "dup", "dup", "zero1", "zero2", "size", "unknown", "link", "folder", "other",
        ]);
        listing.kind = crate::archive::ArchiveKind::Zip;
        for member in &mut listing.members {
            member.size = 10;
            member.zip_info = Some(crate::archive::ZipMemberInfo {
                compression: "Stored".into(),
                encrypted: false,
                encryption: "None".into(),
                crc32: Some(7),
            });
        }
        for id in [2, 3] {
            listing.members[id].size = 0;
            listing.members[id].zip_info.as_mut().unwrap().crc32 = Some(0);
        }
        listing.members[4].size = 11;
        listing.members[5].zip_info.as_mut().unwrap().crc32 = None;
        listing.members[6].is_symlink = true;
        listing.members[7].is_dir = true;
        listing.members[8].is_regular = false;
        assert_eq!(
            super::repeated_crc_rows(&listing, &[3, 2, 1, 0, 4, 5, 6, 7, 8, 99, 0]),
            [3, 2, 1, 0]
        );
        assert!(super::repeated_crc_rows(&listing, &[0, 0, 4, 5, 6, 7, 8, 99]).is_empty());
        let mut selected = std::collections::BTreeSet::from([5]);
        selected.extend(super::repeated_crc_rows(&listing, &[0, 1]));
        assert_eq!(selected, std::collections::BTreeSet::from([0, 1, 5]));
        listing.kind = crate::archive::ArchiveKind::Tar;
        assert!(super::repeated_crc_rows(&listing, &[0, 1, 2, 3]).is_empty());
    }

    #[test]
    fn archive_same_extension_matches_visible_regular_members_and_duplicate_paths() {
        let mut listing = listing(&[
            "src/a.TXT",
            "src/a.TXT",
            "src/b.txt",
            "other/c.txt",
            "src/d.rs",
            "src/folder.txt",
            "src/link.txt",
        ]);
        listing.members[5].is_dir = true;
        listing.members[5].is_regular = false;
        listing.members[6].is_regular = false;
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "");
        let rows = super::same_extension_rows(
            &listing,
            filter.lower_paths.as_deref().unwrap(),
            &filter.visible,
            Some(0),
        );
        assert_eq!(rows, [0, 1, 2]);
        let mut selected: std::collections::BTreeSet<_> = [3, 4].into_iter().collect();
        selected.extend(rows.iter().copied());
        assert_eq!(selected, [0, 1, 2, 3, 4].into_iter().collect());
        for id in rows {
            selected.remove(&id);
        }
        assert_eq!(selected, [3, 4].into_iter().collect());
        filter.visible(&listing, "b.txt");
        assert!(
            super::same_extension_rows(
                &listing,
                filter.lower_paths.as_deref().unwrap(),
                &filter.visible,
                Some(0)
            )
            .is_empty()
        );
        assert_eq!(
            super::same_extension_rows(
                &listing,
                filter.lower_paths.as_deref().unwrap(),
                &filter.visible,
                Some(2)
            ),
            [2]
        );
        assert!(super::same_extension_rows(&listing, &[], &filter.visible, Some(2)).is_empty());
    }

    #[test]
    fn archive_same_extension_handles_empty_suffixes_and_navigation_endpoints() {
        let listing = listing(&[
            ".profile",
            "noext",
            "trailing.",
            "archive.tar.gz",
            "other.gz",
            "a.ЁЖ",
            "b.ёж",
        ]);
        let mut filter = MemberFilter::default();
        filter.visible(&listing, "");
        let lower = filter.lower_paths.as_deref().unwrap();
        assert_eq!(
            super::same_extension_rows(&listing, lower, &filter.visible, Some(0)),
            [0, 1, 2]
        );
        assert_eq!(
            super::same_extension_rows(&listing, lower, &filter.visible, Some(3)),
            [3, 4]
        );
        assert_eq!(
            super::same_extension_rows(&listing, lower, &filter.visible, Some(5)),
            [5, 6]
        );
        assert!(super::same_extension_rows(&listing, lower, &[7], Some(7)).is_empty());
        assert!(super::same_extension_rows(&listing, lower, &filter.visible, None).is_empty());
    }

    #[test]
    fn archive_saved_selection_survives_filter_navigation_and_keeps_duplicate_identity() {
        let listing = listing(&["src/a", "src/a", "other/b"]);
        let mut filter = MemberFilter::default();
        let mut selected = [1, 2, 99].into_iter().collect();
        filter
            .selection_memory
            .save(&selected, listing.members.len());
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "a");
        selected = [0].into_iter().collect();
        filter.order.descending = true;
        filter
            .selection_memory
            .restore(&mut selected, listing.members.len());
        assert_eq!(selected, [1, 2].into_iter().collect());
        assert_eq!(filter.copy_rows(&listing, &selected, true), [1, 2]);
        assert!(filter.history_step(true));
        selected.clear();
        filter
            .selection_memory
            .restore(&mut selected, listing.members.len());
        assert_eq!(selected, [1, 2].into_iter().collect());
        assert!(MemberFilter::default().selection_memory.0.is_none());
    }

    #[test]
    fn archive_saved_empty_selection_differs_from_no_snapshot_and_save_replaces() {
        let mut memory = super::SelectionMemory::default();
        let mut selected = [0, 1].into_iter().collect();
        memory.restore(&mut selected, 2);
        assert_eq!(selected.len(), 2);
        memory.save(&selected, 2);
        selected.clear();
        memory.restore(&mut selected, 2);
        assert_eq!(selected.len(), 2);
        memory.save(&Default::default(), 2);
        assert_eq!(memory.0, Some(vec![]));
        memory.restore(&mut selected, 2);
        assert!(selected.is_empty());
        memory.save(&[1].into_iter().collect(), 2);
        memory.restore(&mut selected, 2);
        assert_eq!(selected, [1].into_iter().collect());
    }

    #[test]
    fn archive_range_follows_display_order_and_preserves_hidden_selection() {
        let visible = [5, 3, 1, 4, 0]; // 5 is a synthetic navigation folder.
        let mut selected = [2].into_iter().collect();
        super::select_member_range(&visible, Some(3), Some(0), 5, &mut selected);
        assert_eq!(selected, [0, 1, 2, 3, 4].into_iter().collect());
        selected = [2].into_iter().collect();
        super::select_member_range(&visible, Some(4), Some(5), 5, &mut selected);
        assert_eq!(selected, [1, 2, 3, 4].into_iter().collect());
        super::select_member_range(&visible, Some(4), Some(99), 5, &mut selected);
        assert_eq!(selected, [1, 2, 3, 4].into_iter().collect());
        selected.clear();
        super::select_member_range(&visible, Some(99), Some(0), 5, &mut selected);
        assert_eq!(selected, [0].into_iter().collect());
    }

    #[test]
    fn archive_range_anchor_resets_when_hidden_or_navigating() {
        let listing = listing(&["a.txt", "b.txt"]);
        let mut filter = MemberFilter::default();
        filter.visible(&listing, "");
        filter.range_anchor = Some(1);
        filter.visible(&listing, "b");
        assert_eq!(filter.range_anchor, Some(1));
        filter.visible(&listing, "a");
        assert_eq!(filter.range_anchor, None);
        filter.range_anchor = Some(0);
        filter.navigate(Some(Default::default()));
        assert_eq!(filter.range_anchor, None);
        filter.range_anchor = Some(0);
        assert!(filter.history_step(true));
        assert_eq!(filter.range_anchor, None);
    }

    #[test]
    fn archive_visible_selection_preserves_hidden_and_separate_duplicate_entries() {
        let mut listing = listing(&["src/a.txt", "src/a.txt", "src/b.txt", "other/c.txt"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "a.txt");
        let mut selected = [0, 3].into_iter().collect();
        super::change_visible_selection(&filter.visible_set, &mut selected, true);
        assert_eq!(selected, [1, 3].into_iter().collect());
        super::change_visible_selection(&filter.visible_set, &mut selected, true);
        assert_eq!(selected, [0, 3].into_iter().collect());
        super::change_visible_selection(&filter.visible_set, &mut selected, false);
        assert_eq!(selected, [3].into_iter().collect());
        filter.visible(&listing, "b.txt");
        super::change_visible_selection(&filter.visible_set, &mut selected, true);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [3, 2]);
    }

    #[test]
    fn archive_visible_selection_excludes_navigation_and_invalid_filter_rows() {
        let mut listing = listing(&["src/a.txt", "other/b.txt"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.navigate(Some(Default::default()));
        assert!(!filter.visible(&listing, "").is_empty());
        assert!(filter.visible_set.is_empty());
        let mut selected = [0].into_iter().collect();
        super::change_visible_selection(&filter.visible_set, &mut selected, true);
        assert_eq!(selected, [0].into_iter().collect());
        filter.navigate(None);
        filter.options.regex = true;
        filter.visible(&listing, "[");
        for invert in [true, false] {
            super::change_visible_selection(&filter.visible_set, &mut selected, invert);
            assert_eq!(selected, [0].into_iter().collect());
        }
        filter.visible(&listing, "");
        super::change_visible_selection(&filter.visible_set, &mut selected, true);
        assert_eq!(selected, [1].into_iter().collect());
        super::change_visible_selection(&filter.visible_set, &mut selected, false);
        assert!(selected.is_empty());
    }

    #[test]
    fn archive_history_walks_modes_and_folders_and_restores_visible_cursor() {
        let mut listing = listing(&["src/a.txt", "src/deep/b.txt", "other/c.txt"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        let selected = [0, 1].into_iter().collect();
        filter.visible(&listing, "");
        filter.focused = Some(1);
        filter.navigate(Some(Default::default()));
        filter.visible(&listing, "");
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "");
        filter.focused = Some(0);
        filter.navigate(Some("src/deep".into()));
        filter.visible(&listing, "");
        filter.focused = Some(1);
        assert!(filter.history_step(true));
        filter.visible(&listing, "");
        assert_eq!(filter.folder, Some("src".into()));
        assert_eq!(filter.focused, Some(0));
        assert!(filter.scroll_to_focused && filter.reset_scroll);
        assert!(filter.history_step(true));
        assert_eq!(filter.folder, Some(Default::default()));
        assert!(filter.history_step(true));
        assert_eq!(filter.folder, None);
        filter.visible(&listing, "");
        assert_eq!(filter.focused, Some(1));
        assert!(!filter.history_step(true));
        for folder in [
            Some(Default::default()),
            Some("src".into()),
            Some("src/deep".into()),
        ] {
            assert!(filter.history_step(false));
            assert_eq!(filter.folder, folder);
            filter.visible(&listing, "");
        }
        assert_eq!(filter.focused, Some(1));
        assert!(!filter.history_step(false));
        assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 1]);
        assert_eq!(selected, [0, 1].into_iter().collect());
    }

    #[test]
    fn archive_history_new_branch_clears_forward_and_same_location_does_not_duplicate() {
        let mut filter = MemberFilter::default();
        assert!(!filter.history_step(true));
        assert!(!filter.history_step(false));
        filter.navigate(None);
        assert!(filter.past.is_empty());
        filter.navigate(Some("a".into()));
        filter.navigate(Some("a".into()));
        assert_eq!(filter.past.len(), 1);
        filter.navigate(Some("b".into()));
        assert!(filter.history_step(true));
        assert_eq!(filter.folder, Some("a".into()));
        filter.navigate(Some("c".into()));
        assert!(filter.future.is_empty());
        assert!(!filter.history_step(false));
        assert!(filter.history_step(true));
        assert_eq!(filter.folder, Some("a".into()));
        assert!(filter.history_step(true));
        assert_eq!(filter.folder, None);
    }

    #[test]
    fn archive_history_keeps_current_filter_sort_and_selection_and_rejects_hidden_cursor() {
        let listing = listing(&["a/first.txt", "b/second.txt"]);
        let selected = [0].into_iter().collect();
        let mut filter = MemberFilter::default();
        filter.navigate(Some("a".into()));
        filter.visible(&listing, "");
        filter.focused = Some(0);
        filter.navigate(Some("b".into()));
        filter.options.regex = true;
        filter.options.smart_case = true;
        filter.order.descending = true;
        filter.visible(&listing, "second");
        assert!(filter.history_step(true));
        assert!(filter.visible(&listing, "second").is_empty());
        assert_eq!(filter.focused, None);
        assert!(filter.options.regex && filter.options.smart_case && filter.order.descending);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [0]);
    }

    #[test]
    fn archive_history_caps_encoded_paths_and_entries_and_default_resets_owner() {
        let mut filter = MemberFilter::default();
        for i in 0..100 {
            filter.navigate(Some(format!("folder-{i}").into()));
        }
        assert_eq!(filter.past.len(), super::MAX_ARCHIVE_HISTORY);
        let mut count = 0;
        while filter.history_step(true) {
            count += 1;
        }
        assert_eq!(count, super::MAX_ARCHIVE_HISTORY);
        assert_eq!(filter.future.len(), super::MAX_ARCHIVE_HISTORY);
        while filter.history_step(false) {}
        for i in 0..20 {
            filter.navigate(Some(format!("{}-{i}", "x".repeat(32 * 1024)).into()));
        }
        assert!(
            filter
                .past
                .iter()
                .chain(filter.future.iter())
                .map(|point| point.bytes())
                .sum::<usize>()
                <= super::MAX_ARCHIVE_HISTORY_BYTES
        );
        let reset = MemberFilter::default();
        assert!(reset.past.is_empty() && reset.future.is_empty());
        assert_eq!(reset.folder, None);
    }

    #[test]
    fn archive_regex_and_case_changes_rebuild_real_and_navigation_rows() {
        let mut listing = listing(&[
            "src/Alpha.rs",
            "src/alpha.rs",
            "src/deep/file.rs",
            "Other/x.rs",
        ]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let selected = [0, 2].into_iter().collect();
        let mut filter = MemberFilter::default();
        filter.options.regex = true;
        assert_eq!(filter.visible(&listing, r"^src/alpha\.rs$"), [0, 1]);
        filter.options.case_sensitive = true;
        assert_eq!(filter.visible(&listing, r"^src/alpha\.rs$"), [1]);
        filter.options.case_sensitive = false;
        filter.options.smart_case = true;
        assert_eq!(filter.visible(&listing, r"^src/Alpha\.rs$"), [0]);
        filter.navigate(Some("src".into()));
        filter.visible(&listing, r"(?i)^src/(?:alpha\.rs|deep)$");
        assert_eq!(filter.copy_rows(&listing, &selected, false), [0, 1]);
        let tree = listing.tree.as_ref().unwrap();
        assert!(filter.visible.iter().any(|&id| id >= listing.members.len()
            && tree.directories[id - listing.members.len()].path
                == std::path::Path::new("src/deep")));
        assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 2]);
    }

    #[test]
    fn invalid_archive_regex_hides_every_row_and_clears_invisible_cursor_not_selection() {
        let mut listing = listing(&["src/file*.rs", "src/other.rs", "src/deep/x"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let selected = [0, 2].into_iter().collect();
        let mut filter = MemberFilter::default();
        filter.navigate(Some("src".into()));
        assert_eq!(filter.visible(&listing, "*.rs"), [0]);
        filter.focused = Some(0);
        filter.options.regex = true;
        assert!(filter.visible(&listing, "*.rs").is_empty());
        assert!(matches!(filter.pattern, Some(Err(_))));
        assert_eq!(filter.focused, None);
        assert_eq!(
            step_member_cursor(&filter.visible, filter.focused, false, true, 1),
            None
        );
        assert_eq!(filter.copy_rows(&listing, &selected, true), [2, 0]);
        assert!(filter.copy_rows(&listing, &selected, false).is_empty());
        filter.options.regex = false;
        assert_eq!(filter.visible(&listing, "*.rs"), [0]);
        assert!(matches!(filter.pattern, Some(Ok(_))));
        assert_eq!(selected, [0, 2].into_iter().collect());
    }

    #[test]
    fn literal_archive_filter_preserves_visible_cursor_but_clears_a_filtered_one() {
        let listing = listing(&["first.txt", "second.txt"]);
        let mut filter = MemberFilter::default();
        filter.visible(&listing, "");
        filter.focused = Some(0);
        filter.visible(&listing, "first");
        assert_eq!(filter.focused, Some(0));
        filter.visible(&listing, "second");
        assert_eq!(filter.focused, None);
        assert!(filter.reset_scroll);
        assert!(filter.visible(&listing, &" ".repeat(257)).is_empty());
        assert!(matches!(filter.pattern, Some(Err(_))));
    }

    #[test]
    fn member_filter_matches_case_insensitively_and_tracks_the_text() {
        let listing = listing(&["src/Main.rs", "README.md", "src/lib.rs"]);
        let mut filter = MemberFilter::default();
        assert_eq!(filter.visible(&listing, ""), [1, 2, 0]);
        assert_eq!(filter.visible(&listing, "  SRC "), [2, 0]);
        assert_eq!(filter.visible(&listing, "main"), [0]);
        assert_eq!(filter.visible(&listing, "nothing"), [] as [usize; 0]);
        assert_eq!(filter.visible(&listing, ""), [1, 2, 0]);
    }

    #[test]
    fn unchanged_filter_reuses_its_matches() {
        let listing = listing(&["a.txt", "b.txt"]);
        let mut filter = MemberFilter::default();
        let first = filter.visible(&listing, "a").as_ptr();
        // Same text, even with a different listing: no rescan until reset.
        let other = self::listing(&["b.txt", "a.txt"]);
        assert_eq!(filter.visible(&other, "a").as_ptr(), first);
        assert_eq!(filter.visible(&other, "a"), [0]);
    }

    #[test]
    fn archive_sort_preserves_focus_and_selection_membership_after_filtering() {
        let mut listing = listing(&["first.txt", "middle.txt", "last.txt"]);
        listing.members[0].size = 2;
        listing.members[1].size = 100;
        listing.members[2].size = 50;
        let mut filter = MemberFilter::default();
        filter.visible(&listing, "");
        filter.focused = Some(2);
        let selected = std::collections::BTreeSet::from([1, 2]);
        filter.order.choose(crate::archive_order::Field::Size);
        assert_eq!(filter.visible(&listing, ""), [1, 2, 0]);
        assert_eq!(filter.focused, Some(2));
        assert!(filter.scroll_to_focused);
        assert_eq!(
            selected
                .iter()
                .filter(|index| !filter.visible_set.contains(*index))
                .count(),
            0
        );
        assert_eq!(filter.visible(&listing, "last"), [2]);
        assert_eq!(
            selected
                .iter()
                .filter(|index| !filter.visible_set.contains(*index))
                .count(),
            1
        );
        assert_eq!(filter.visible(&listing, ""), [1, 2, 0]);
    }

    #[test]
    fn many_extract_errors_toast_a_count_and_cap_the_inline_list() {
        let one = vec!["Could not create /ro/a: denied".to_string()];
        assert_eq!(
            extract_error_summary(&one),
            (one[0].clone(), one[0].clone())
        );

        let errors: Vec<String> = (0..8)
            .map(|i| format!("Could not create /ro/{i}"))
            .collect();
        let (toast, inline) = extract_error_summary(&errors);
        assert_eq!(toast, "8 members failed: Could not create /ro/0");
        assert_eq!(inline.lines().count(), 6);
        assert!(inline.ends_with("… and 3 more"));
    }

    #[test]
    fn extract_labels_name_hidden_selection_and_capped_listings() {
        assert_eq!(
            extract_selected_label(0, 0),
            "Extract selected to other panel"
        );
        assert_eq!(extract_selected_label(3, 0), "Extract 3 to other panel");
        assert_eq!(
            extract_selected_label(3, 2),
            "Extract 3 (2 hidden) to other panel"
        );

        let mut capped = listing(&["a", "b"]);
        assert_eq!(extract_all_label(&capped), "Extract all");
        capped.truncated = true;
        assert_eq!(extract_all_label(&capped), "Extract 2 listed");
    }

    #[test]
    fn member_cursor_steps_through_visible_rows_only() {
        let visible = [2, 5, 9];
        assert_eq!(step_member_cursor(&visible, None, false, false, 1), None);
        assert_eq!(step_member_cursor(&visible, None, false, true, 1), Some(2));
        assert_eq!(
            step_member_cursor(&visible, Some(2), false, true, 1),
            Some(5)
        );
        assert_eq!(
            step_member_cursor(&visible, Some(9), false, true, 1),
            Some(9)
        );
        assert_eq!(
            step_member_cursor(&visible, Some(5), true, false, 1),
            Some(2)
        );
        assert_eq!(
            step_member_cursor(&visible, Some(2), true, false, 1),
            Some(2)
        );
        // A cursor the filter now hides restarts at the top.
        assert_eq!(
            step_member_cursor(&visible, Some(4), true, false, 1),
            Some(2)
        );
        assert_eq!(step_member_cursor(&[], Some(4), false, true, 1), Some(4));
    }

    #[test]
    fn archive_selection_mask_keeps_view_hidden_choices_and_duplicate_ids() {
        let mut listing = listing(&[
            "src/A.rs",
            "src/b.txt",
            "hidden/keep.rs",
            "src/dup.rs",
            "src/dup.rs",
            "src/deep/c.rs",
        ]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.navigate(Some("src".into()));
        let visible = filter.visible(&listing, "").to_vec();
        let mut selected = [1, 2].into_iter().collect();
        filter
            .selection_memory
            .save(&selected, listing.members.len());
        assert!(visible.iter().any(|&id| id >= listing.members.len()));
        filter.selection_mask.query = "*.rs".into();
        let lower = filter.lower_paths.as_deref().unwrap();
        assert_eq!(
            filter
                .selection_mask
                .apply(&listing, lower, &visible, &mut selected, true),
            Ok(3)
        );
        assert_eq!(selected, [0, 1, 2, 3, 4].into_iter().collect());
        assert_eq!(
            filter
                .selection_mask
                .apply(&listing, lower, &visible, &mut selected, false),
            Ok(3)
        );
        assert_eq!(selected, [1, 2].into_iter().collect());
        filter.selection_mask.full_path = true;
        filter.selection_mask.query = "src/d*.rs".into();
        assert_eq!(
            filter
                .selection_mask
                .apply(&listing, lower, &visible, &mut selected, true),
            Ok(2)
        );
        assert_eq!(selected, [1, 2, 3, 4].into_iter().collect());
        assert_eq!(filter.visible, visible);
        assert_eq!(filter.folder, Some("src".into()));
        assert_eq!(filter.needle.as_deref(), Some(""));
        let mut saved = Default::default();
        filter
            .selection_memory
            .restore(&mut saved, listing.members.len());
        assert_eq!(saved, [1, 2].into_iter().collect());
    }

    #[test]
    fn archive_selection_mask_rejects_invalid_input_and_respects_case_and_real_records() {
        let mut listing = listing(&["Dir.RS", "link.rs", "plain.rs"]);
        listing.members[0].is_dir = true;
        listing.members[1].is_symlink = true;
        let lower: Vec<_> = listing
            .members
            .iter()
            .map(|m| m.path.to_string_lossy().to_lowercase())
            .collect();
        let mut mask = super::MaskSelection {
            query: "*.RS".into(),
            case_sensitive: true,
            ..Default::default()
        };
        let mut selected = [2].into_iter().collect();
        assert_eq!(
            mask.apply(&listing, &lower, &[0, 1, 99], &mut selected, true),
            Ok(1)
        );
        assert_eq!(selected, [0, 2].into_iter().collect());
        mask.case_sensitive = false;
        assert_eq!(
            mask.apply(&listing, &lower, &[0, 1, 99], &mut selected, true),
            Ok(2)
        );
        assert_eq!(selected, [0, 1, 2].into_iter().collect());
        let before = selected.clone();
        for query in [String::new(), "[".into(), "x".repeat(257)] {
            mask.query = query;
            assert!(
                mask.apply(&listing, &lower, &[0, 1], &mut selected, false)
                    .is_err()
            );
            assert_eq!(selected, before);
        }
        mask.query = "*".into();
        assert!(
            mask.apply(&listing, &[], &[0, 1], &mut selected, false)
                .is_err()
        );
        assert_eq!(selected, before);
    }

    #[test]
    fn archive_selection_mask_pointer_actions_change_selection_without_changing_mask() {
        let listing = listing(&["a.rs", "b.txt", "c.rs", "hidden.rs"]);
        let lower: Vec<_> = listing
            .members
            .iter()
            .map(|m| m.path.to_string_lossy().to_lowercase())
            .collect();
        let mut mask = super::MaskSelection {
            query: "*.rs".into(),
            ..Default::default()
        };
        let mut selected = [3].into_iter().collect();
        let ctx = egui::Context::default();
        let mut actions = 0;
        for frame in 0..14 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 900.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4, 10, 11].contains(&frame) {
                let rect = ctx
                    .data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new((
                            "archive-selection-mask-action",
                            frame < 10,
                        )))
                    })
                    .unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 3 || frame == 10,
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                if mask.ui(ui, &listing, &lower, &[0, 1, 2, 99], &mut selected) {
                    actions += 1;
                }
            }));
            if frame == 5 {
                assert_eq!(selected, [0, 2, 3].into_iter().collect());
            }
        }
        assert_eq!(actions, 2);
        assert_eq!(selected, [3].into_iter().collect());
        assert_eq!(mask.query, "*.rs");
    }

    #[test]
    fn archive_similar_name_uses_final_stem_casefold_and_real_regular_ids() {
        let mut listing = listing(&[
            "src/Archive.tar.gz",
            "other/archive.tar.xz",
            "src/archive.txt",
            "src/Archive.tar.gz",
            "src/archive.tar.link",
            "src/archive.tar.folder",
            ".env",
            "other/.env.txt",
        ]);
        listing.members[4].is_symlink = true;
        listing.members[5].is_dir = true;
        let lower: Vec<_> = listing
            .members
            .iter()
            .map(|m| m.path.to_string_lossy().to_lowercase())
            .collect();
        let visible = [3, 0, 4, 1, 5, 2, 6, 7, 100];
        assert_eq!(
            super::similar_member_rows(
                &listing,
                &lower,
                &visible,
                Some(0),
                super::SimilarMember::Name
            ),
            [3, 0, 1]
        );
        assert_eq!(
            super::similar_member_rows(
                &listing,
                &lower,
                &visible,
                Some(6),
                super::SimilarMember::Name
            ),
            [6, 7]
        );
        assert!(
            super::similar_member_rows(
                &listing,
                &lower,
                &visible,
                Some(100),
                super::SimilarMember::Name
            )
            .is_empty()
        );
        assert!(
            super::similar_member_rows(
                &listing,
                &[],
                &visible,
                Some(0),
                super::SimilarMember::Name
            )
            .is_empty()
        );
        assert!(
            super::similar_member_rows(&listing, &lower, &[1], Some(0), super::SimilarMember::Name)
                .is_empty()
        );
    }

    #[test]
    fn archive_similar_folder_is_case_sensitive_and_includes_real_record_types() {
        let mut listing = listing(&["src/a", "src/b", "src/deep/c", "Src/d", "root", "folder"]);
        listing.members[1].is_symlink = true;
        listing.members[5].is_dir = true;
        let visible = [1, 0, 3, 100];
        let rows = super::similar_member_rows(
            &listing,
            &[],
            &visible,
            Some(0),
            super::SimilarMember::Folder,
        );
        assert_eq!(rows, [1, 0]);
        assert_eq!(
            super::similar_member_rows(
                &listing,
                &[],
                &[4, 5],
                Some(4),
                super::SimilarMember::Folder
            ),
            [4, 5]
        );
        assert!(
            super::similar_member_rows(
                &listing,
                &[],
                &visible,
                Some(100),
                super::SimilarMember::Folder
            )
            .is_empty()
        );
    }

    #[test]
    fn archive_similar_selection_pointer_adds_and_removes_visible_matches_only() {
        for (mode, label, matches) in [
            (super::SimilarMember::Name, "Same name", vec![0, 2]),
            (super::SimilarMember::Folder, "Same folder", vec![0, 3]),
        ] {
            let listing = listing(&["src/one.txt", "hidden/keep", "other/one.md", "src/two.txt"]);
            let lower: Vec<_> = listing
                .members
                .iter()
                .map(|m| m.path.to_string_lossy().to_lowercase())
                .collect();
            let visible = [0, 2, 3, 99];
            let mut selected = [1].into_iter().collect();
            let ctx = egui::Context::default();
            let mut actions = 0;
            for frame in 0..28 {
                let mut input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 900.0),
                    )),
                    time: Some(frame as f64 / 60.0),
                    ..Default::default()
                };
                if [3, 4, 10, 11, 17, 18, 24, 25].contains(&frame) {
                    let id = if [3, 4, 17, 18].contains(&frame) {
                        egui::Id::new(("archive-similar-menu", label))
                    } else {
                        egui::Id::new(("archive-similar-action", label, frame < 14))
                    };
                    let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
                    input.events = vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed: [3, 10, 17, 24].contains(&frame),
                            modifiers: Default::default(),
                        },
                    ];
                }
                crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                    ui.horizontal(|ui| {
                        if super::similar_selection_menu(
                            ui,
                            &listing,
                            &lower,
                            &visible,
                            Some(0),
                            &mut selected,
                            mode,
                        ) {
                            actions += 1;
                        }
                    });
                }));
                if frame == 14 {
                    let expected: std::collections::BTreeSet<_> =
                        matches.iter().copied().chain([1]).collect();
                    assert_eq!(selected, expected, "{label}");
                }
            }
            assert_eq!(actions, 2, "{label}");
            assert_eq!(selected, [1].into_iter().collect(), "{label}");
        }
    }

    #[test]
    fn archive_modified_filter_preserves_navigation_selection_and_saved_ranges() {
        use std::time::{Duration, UNIX_EPOCH};
        let mut listing = listing(&["root.txt", "src/a.txt", "src/deep/b.txt", "src/unknown"]);
        listing.members[0].modified = Some(UNIX_EPOCH - Duration::from_secs(1));
        listing.members[1].modified = Some(UNIX_EPOCH);
        listing.members[2].modified = Some(UNIX_EPOCH + Duration::from_secs(86400));
        listing.members[3].modified = None;
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter {
            modified_input: "1970-01-01".into(),
            ..Default::default()
        };
        assert!(filter.save_filter(""));
        let saved = filter.saved_filters[0].clone();
        filter.modified_input = "1970-01-01..1970-01-03".into();
        assert!(filter.save_filter(""));
        assert_eq!(filter.saved_filters.len(), 2);
        filter.restore_filter(saved.clone(), &mut String::new());
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "");
        let selected = [0, 3].into_iter().collect();
        assert_eq!(filter.copy_rows(&listing, &selected, false), [1]);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 3]);
        assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
        let mut query = String::new();
        filter.reset_filters(&mut query);
        filter.visible(&listing, &query);
        assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 3]);
        filter.restore_filter(saved, &mut query);
        filter.visible(&listing, &query);
        assert_eq!(filter.copy_rows(&listing, &selected, false), [1]);
        filter.modified_input = "bad".into();
        assert!(filter.visible(&listing, &query).is_empty());
        assert!(!filter.save_filter(""));
        filter.modified_input = "\u{a0}".repeat(13);
        assert!(filter.visible(&listing, &query).is_empty());
        assert!(filter.has_filters(&query));
        filter.reset_filters(&mut query);
        assert!(!filter.visible(&listing, &query).is_empty());
    }

    #[test]
    fn archive_extensions_match_literal_suffixes_and_reject_invalid_queries() {
        let listing = listing(&[
            "report.PDF",
            "photos/a.jpg",
            "archive.tar.gz",
            "archive.gz",
            ".env",
            "dir.pdf/no_extension",
            "name.pdf.bak",
            "a.ЁЖ",
            "a.jp[g]",
        ]);
        let mut filter = MemberFilter::default();
        for (input, expected) in [
            ("pdf .JPG", vec![0, 1]),
            ("tar.gz", vec![2]),
            ("gz", vec![2, 3]),
            ("env", vec![4]),
            ("ёж", vec![7]),
            ("pdf PDF .pdf", vec![0]),
            ("", (0..9).collect()),
        ] {
            filter.extensions_input = input.into();
            let mut actual = filter.visible(&listing, "").to_vec();
            actual.sort_unstable();
            assert_eq!(actual, expected, "{input}");
        }
        for input in [
            ".", "..pdf", "pdf.", "tar..gz", "*.pdf", "jp[g]", "a/b", "a\\b", "pdf,jpg", "a\0b",
        ] {
            filter.extensions_input = input.into();
            assert!(filter.visible(&listing, "").is_empty(), "{input:?}");
            assert!(matches!(filter.extensions_pattern, Some(Err(_))));
            assert!(!filter.save_filter(""));
        }
        for input in ["a ".repeat(33), "x".repeat(257), "\u{a0}".repeat(129)] {
            filter.extensions_input = input;
            assert!(filter.visible(&listing, "").is_empty());
            assert!(filter.has_filters(""));
        }
        filter.reset_filters(&mut String::new());
        assert_eq!(filter.visible(&listing, "").len(), 9);
    }

    #[test]
    fn archive_extensions_compose_preserve_hidden_selection_and_restore_saved_settings() {
        let mut listing = listing(&[
            "root.pdf",
            "src/a.PDF",
            "src/a.PDF",
            "src/b.jpg",
            "src/folder.pdf",
            "src/link.pdf",
            "src/deep/note.txt",
        ]);
        listing.members[4].is_dir = true;
        listing.members[4].is_regular = false;
        listing.members[5].is_symlink = true;
        listing.members[5].is_regular = false;
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter {
            extensions_input: "pdf".into(),
            ..Default::default()
        };
        assert_eq!(filter.visible(&listing, ""), [0, 1, 2, 4, 5]);
        assert!(filter.save_filter(""));
        let saved = filter.saved_filters[0].clone();
        filter.extensions_input = "jpg".into();
        assert!(filter.save_filter(""));
        assert_eq!(filter.saved_filters.len(), 2);
        filter.restore_filter(saved.clone(), &mut String::new());
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "");
        let selected = [0, 3].into_iter().collect();
        assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 2, 5]);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 3]);
        assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
        filter.kinds.links = false;
        filter.visible(&listing, "a.");
        assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 2]);
        filter.exclude_matches = true;
        filter.visible(&listing, "a.");
        assert!(filter.copy_rows(&listing, &selected, false).is_empty());
        filter.extensions_input = "*.pdf".into();
        assert!(filter.visible(&listing, "").is_empty());
        let mut query = String::new();
        filter.reset_filters(&mut query);
        assert_eq!(filter.saved_filters.len(), 2);
        filter.restore_filter(saved, &mut query);
        assert_eq!(
            filter
                .visible(&listing, &query)
                .iter()
                .filter(|&&id| id < listing.members.len())
                .count(),
            3
        );
    }

    #[test]
    fn archive_depth_filter_exact_open_ranges_and_invalid_inputs() {
        let listing = listing(&["a", "dir/b", "dir/deep/c", "dir/deep/more/d"]);
        let mut filter = MemberFilter::default();
        for (text, expected) in [
            ("1", vec![0]),
            ("2..3", vec![1, 2]),
            ("..2", vec![0, 1]),
            ("3..", vec![2, 3]),
            ("0", vec![]),
            ("0..4294967295", vec![0, 1, 2, 3]),
            ("", vec![0, 1, 2, 3]),
        ] {
            filter.depth_input = text.into();
            assert_eq!(filter.visible(&listing, ""), expected, "{text}");
        }
        for text in [
            "..",
            "3..2",
            "-1",
            "+2",
            "1.2",
            "1..2..3",
            "4294967296",
            "one",
            "1 2",
        ] {
            filter.depth_input = text.into();
            assert!(filter.visible(&listing, "").is_empty(), "{text}");
            assert!(matches!(filter.depth_pattern, Some(Err(_))));
            assert!(!filter.save_filter(""));
        }
        filter.depth_input = "\u{a0}".repeat(13);
        assert!(filter.visible(&listing, "").is_empty());
        assert!(filter.has_filters(""));
        filter.reset_filters(&mut String::new());
        assert_eq!(filter.visible(&listing, "").len(), 4);
    }

    #[test]
    fn archive_depth_filter_keeps_archive_root_basis_navigation_selection_and_saved_identity() {
        let mut listing = listing(&[
            "root.txt",
            "src/a.txt",
            "src/deep/b.txt",
            "elsewhere/hidden",
        ]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter {
            depth_input: "2".into(),
            ..Default::default()
        };
        assert!(filter.save_filter(""));
        let saved = filter.saved_filters[0].clone();
        filter.depth_input = "1..2".into();
        assert!(filter.save_filter(""));
        assert_eq!(filter.saved_filters.len(), 2);
        filter.restore_filter(saved.clone(), &mut String::new());
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "");
        let selected = [0, 3].into_iter().collect();
        assert_eq!(filter.copy_rows(&listing, &selected, false), [1]);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [3, 0]);
        assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
        filter.navigate(Some("src/deep".into()));
        assert!(filter.visible(&listing, "").is_empty());
        let mut query = String::new();
        filter.reset_filters(&mut query);
        assert_eq!(filter.visible(&listing, &query), [2]);
        filter.restore_filter(saved, &mut query);
        assert!(filter.visible(&listing, &query).is_empty());
        filter.depth_input = "bad".into();
        filter.navigate(Some("src".into()));
        assert!(filter.visible(&listing, &query).is_empty());
    }

    #[test]
    fn executable_archive_filter_requires_known_regular_file_status() {
        let mut listing = listing(&["a", "b", "c", "link", "folder", "other"]);
        listing.members[0].is_executable = Some(true);
        listing.members[1].is_executable = Some(false);
        listing.members[3].is_executable = Some(true);
        listing.members[3].is_symlink = true;
        listing.members[4].is_executable = Some(true);
        listing.members[4].is_dir = true;
        listing.members[5].is_executable = Some(true);
        listing.members[5].is_regular = false;
        let mut filter = MemberFilter::default();
        filter.visible(&listing, "");
        assert_eq!(filter.visible.len(), 6);
        filter.kinds.executable_only = true;
        assert_eq!(filter.visible(&listing, ""), [0]);
        filter.kinds.files = false;
        assert!(filter.visible(&listing, "").is_empty());
        filter.kinds = Default::default();
        assert_eq!(filter.visible(&listing, "").len(), 6);
    }

    #[test]
    fn executable_archive_filter_preserves_navigation_hidden_selection_and_saved_identity() {
        let mut listing = listing(&[
            "src/run.sh",
            "src/plain.txt",
            "src/deep/tool",
            "elsewhere/hidden",
        ]);
        listing.members[0].is_executable = Some(true);
        listing.members[1].is_executable = Some(false);
        listing.members[2].is_executable = Some(true);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        assert!(filter.save_filter("src"));
        filter.kinds.executable_only = true;
        assert!(filter.save_filter("src"));
        assert_eq!(filter.saved_filters.len(), 2);
        let saved = filter.saved_filters[0].clone();
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "");
        let selected = [1, 3].into_iter().collect();
        assert_eq!(filter.copy_rows(&listing, &selected, false), [0]);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [3, 1]);
        assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
        let mut query = String::new();
        filter.reset_filters(&mut query);
        assert!(!filter.kinds.executable_only);
        filter.visible(&listing, &query);
        assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 0]);
        filter.restore_filter(saved, &mut query);
        filter.visible(&listing, &query);
        assert!(filter.kinds.executable_only);
        assert_eq!(filter.copy_rows(&listing, &selected, false), [0]);
    }

    #[test]
    fn archive_fuzzy_mode_rebuilds_duplicates_scopes_intersections_and_saved_navigation() {
        let mut listing = listing(&[
            "src/main.rs",
            "src/map.rs",
            "src/main.rs",
            "docs/Maintenance.md",
            "src/deep/lib.rs",
        ]);
        listing.members[2].size = 2;
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let selected: std::collections::BTreeSet<_> = [0, 2, 3].into_iter().collect();
        let mut filter = MemberFilter::default();
        assert!(filter.visible(&listing, "mnrs").is_empty());
        filter.options.fuzzy = true;
        assert_eq!(filter.visible(&listing, "mnrs"), [0, 2]);
        filter.focused = Some(2);
        filter.options.fuzzy = false;
        assert!(filter.visible(&listing, "mnrs").is_empty());
        assert!(filter.focused.is_none());
        filter.options.fuzzy = true;
        filter.options.basename_only = true;
        assert!(filter.visible(&listing, "smrs").is_empty());
        assert_eq!(filter.visible(&listing, "mnrs"), [0, 2]);
        filter.size_input = "1b".into();
        assert_eq!(filter.visible(&listing, "mnrs"), [0]);
        filter.size_input.clear();
        filter.kinds.files = false;
        assert!(filter.visible(&listing, "mnrs").is_empty());
        filter.kinds.files = true;
        filter.exclusion.settings.query = "*main*".into();
        assert!(filter.visible(&listing, "mnrs").is_empty());
        filter.exclusion.settings.query.clear();
        let mut query = "mnrs".to_owned();
        assert_eq!(filter.visible(&listing, &query), [0, 2]);
        assert!(filter.save_filter(&query));
        let saved = filter.saved_filters[0].clone();
        assert!(saved.label().contains("Fuzzy"));
        filter.reset_filters(&mut query);
        assert!(!filter.options.fuzzy);
        filter.restore_filter(saved, &mut query);
        assert!(filter.options.fuzzy);
        assert_eq!(filter.visible(&listing, &query), [0, 2]);
        assert_eq!(
            filter
                .copy_rows(&listing, &selected, true)
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            selected
        );
        filter.navigate(Some(Default::default()));
        filter.visible(&listing, "sc");
        let tree = listing.tree.as_ref().unwrap();
        assert_eq!(filter.visible.len(), 1);
        assert_eq!(
            tree.directories[filter.visible[0] - listing.members.len()].path,
            std::path::Path::new("src")
        );
        assert!(filter.copy_rows(&listing, &selected, false).is_empty());
        filter.exclude_matches = true;
        filter.visible(&listing, "sc");
        assert_eq!(filter.visible.len(), 1);
        assert_eq!(
            tree.directories[filter.visible[0] - listing.members.len()].path,
            std::path::Path::new("docs")
        );
        assert_eq!(
            filter
                .copy_rows(&listing, &selected, true)
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            selected
        );
        filter.options.regex = true;
        assert!(filter.visible(&listing, "").is_empty());
        assert!(!filter.save_filter(""));
    }

    #[test]
    fn archive_matching_mode_checkboxes_are_exclusive_and_preserve_scope_and_case() {
        let ctx = egui::Context::default();
        let mut options = crate::archive_filter::Options {
            regex: true,
            smart_case: true,
            basename_only: true,
            ..Default::default()
        };
        for _ in 0..2 {
            crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.horizontal_wrapped(|ui| super::matching_modes_ui(ui, &mut options, true));
            }));
        }
        for label in ["Fuzzy", "Glob", "Exact", "Regex", "Regex"] {
            for pressed in [true, false] {
                let rect = ctx
                    .data(|d| {
                        d.get_temp::<egui::Rect>(egui::Id::new(("archive-match-mode", label)))
                    })
                    .unwrap();
                let input = egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                };
                crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                    ui.horizontal_wrapped(|ui| super::matching_modes_ui(ui, &mut options, true));
                }));
            }
            assert!(
                u8::from(options.fuzzy)
                    + u8::from(options.glob)
                    + u8::from(options.exact)
                    + u8::from(options.regex)
                    <= 1
            );
            assert!(options.smart_case && options.basename_only);
            if label == "Fuzzy" {
                assert!(options.fuzzy);
            }
            if label == "Glob" {
                assert!(options.glob);
            }
            if label == "Exact" {
                assert!(options.exact);
            }
        }
        assert!(!options.regex && !options.exact && !options.glob && !options.fuzzy);
    }

    #[test]
    fn archive_glob_filter_composes_with_scope_navigation_exclusion_and_saved_modes() {
        let mut listing = listing(&["src/a.rs", "src/deep/b.rs", "src/c.txt", "other/a.rs"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.options.glob = true;
        assert!(filter.visible(&listing, "*.rs").is_empty());
        filter.options.basename_only = true;
        assert_eq!(filter.visible(&listing, "*.rs"), [3, 0, 1]);
        filter.options.basename_only = false;
        let mut query = "src/**/*.rs".to_string();
        assert_eq!(filter.visible(&listing, &query), [0, 1]);
        assert!(filter.save_filter(&query));
        let saved = filter.saved_filters[0].clone();
        filter.reset_filters(&mut query);
        assert!(!filter.options.glob);
        filter.restore_filter(saved, &mut query);
        assert_eq!(filter.visible(&listing, &query), [0, 1]);
        filter.navigate(Some(Default::default()));
        filter.visible(&listing, "src*");
        let tree = listing.tree.as_ref().unwrap();
        assert_eq!(filter.visible.len(), 1);
        assert_eq!(
            tree.directories[filter.visible[0] - listing.members.len()].path,
            std::path::Path::new("src")
        );
        filter.exclude_matches = true;
        filter.visible(&listing, "src*");
        assert_eq!(filter.visible.len(), 1);
        assert_eq!(
            tree.directories[filter.visible[0] - listing.members.len()].path,
            std::path::Path::new("other")
        );
        assert_eq!(
            filter.copy_rows(&listing, &[2].into_iter().collect(), true),
            [2]
        );
        assert!(filter.visible(&listing, "[").is_empty());
        assert!(!filter.save_filter("["));
    }

    #[test]
    fn archive_exact_filter_rebuilds_and_restores_saved_mode_with_hidden_selection() {
        let mut listing = listing(&["src/a.txt", "src/prefix-a.txt", "a.txt/child", "src/a.txt"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.options.basename_only = true;
        let mut query = "a.txt".to_string();
        assert_eq!(filter.visible(&listing, &query), [0, 3, 1]);
        assert!(filter.save_filter(&query));
        filter.options.exact = true;
        assert_eq!(filter.visible(&listing, &query), [0, 3]);
        assert!(filter.save_filter(&query));
        assert_eq!(filter.saved_filters.len(), 2);
        let saved = filter.saved_filters[0].clone();
        filter.reset_filters(&mut query);
        assert!(!filter.options.exact);
        filter.restore_filter(saved, &mut query);
        assert_eq!(filter.visible(&listing, &query), [0, 3]);
        filter.navigate(Some(Default::default()));
        filter.visible(&listing, &query);
        assert_eq!(filter.visible.len(), 1);
        assert!(filter.visible[0] >= listing.members.len());
        filter.exclude_matches = true;
        filter.visible(&listing, &query);
        let tree = listing.tree.as_ref().unwrap();
        assert_eq!(filter.visible.len(), 1);
        assert_eq!(
            tree.directories[filter.visible[0] - listing.members.len()].path,
            std::path::Path::new("src")
        );
        assert_eq!(
            filter.copy_rows(&listing, &[1].into_iter().collect(), true),
            [1]
        );
        filter.options.regex = true;
        assert!(filter.visible(&listing, &query).is_empty());
        assert!(!filter.save_filter(&query));
    }

    #[test]
    fn archive_filename_scope_rebuilds_duplicates_navigation_and_saved_filters() {
        let mut listing = listing(&[
            "src/main.rs",
            "src/other.txt",
            "main.rs/nested.txt",
            "src/main.rs",
        ]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.options.regex = true;
        let mut query = r"^main\.rs$".to_string();
        assert!(filter.visible(&listing, &query).is_empty());
        assert!(filter.save_filter(&query));
        filter.options.basename_only = true;
        assert_eq!(filter.visible(&listing, &query), [0, 3]);
        assert!(filter.save_filter(&query));
        assert_eq!(filter.saved_filters.len(), 2);
        let saved = filter.saved_filters[0].clone();
        filter.reset_filters(&mut query);
        assert!(!filter.options.basename_only);
        filter.restore_filter(saved, &mut query);
        assert_eq!(filter.visible(&listing, &query), [0, 3]);
        filter.navigate(Some(Default::default()));
        filter.visible(&listing, &query);
        assert_eq!(filter.visible.len(), 1);
        assert!(filter.visible[0] >= listing.members.len());
        let selected = [1].into_iter().collect();
        assert_eq!(filter.copy_rows(&listing, &selected, true), [1]);
        filter.exclude_matches = true;
        filter.visible(&listing, &query);
        assert_eq!(filter.visible.len(), 1);
        let tree = listing.tree.as_ref().unwrap();
        assert_eq!(
            tree.directories[filter.visible[0] - listing.members.len()].path,
            std::path::Path::new("src")
        );
        assert!(filter.visible(&listing, "[").is_empty());
    }

    #[test]
    fn archive_size_filter_units_boundaries_and_overflow_are_exact() {
        let mut listing = listing(&["a", "b", "c", "d", "e", "f"]);
        for (member, size) in
            listing
                .members
                .iter_mut()
                .zip([0, 1000, 1024, 1048576, 9007199254740993, u64::MAX])
        {
            member.size = size;
        }
        let mut filter = MemberFilter::default();
        for (input, expected) in [
            ("0b", vec![0]),
            ("+1k -1ki", vec![1, 2]),
            ("1MI", vec![3]),
            ("9007199254740993", vec![4]),
            ("18446744073709551615b", vec![5]),
            ("+18446744073709551615", vec![5]),
            ("-0", vec![0]),
            ("+2ki -1ki", vec![]),
            ("", vec![0, 1, 2, 3, 4, 5]),
        ] {
            filter.size_input = input.into();
            assert_eq!(filter.visible(&listing, ""), expected, "{input}");
        }
        for input in [
            "+",
            "--1",
            "1.5mi",
            "1kb",
            "18446744073709551616",
            "18446744073709551615ki",
            "1b 2b 3b",
            "é",
        ] {
            filter.size_input = input.into();
            assert!(filter.visible(&listing, "").is_empty(), "{input}");
            assert!(matches!(filter.size_pattern, Some(Err(_))));
            assert!(!filter.save_filter(""));
        }
        filter.size_input = "0".repeat(65);
        assert!(filter.visible(&listing, "").is_empty());
        filter.size_input = "\u{a0}".repeat(40);
        assert!(filter.visible(&listing, "").is_empty());
        assert!(filter.has_filters(""));
        filter.reset_filters(&mut String::new());
        assert_eq!(filter.visible(&listing, "").len(), 6);
    }

    #[test]
    fn archive_size_filter_retains_navigation_hidden_selection_and_saved_settings() {
        let mut listing = listing(&[
            "src/empty.txt",
            "src/data.bin",
            "src/link",
            "src/deep/child",
            "elsewhere/hidden",
        ]);
        for (member, size) in listing.members.iter_mut().zip([0, 10, 0, 20, 100]) {
            member.size = size;
        }
        listing.members[2].is_symlink = true;
        listing.members[2].is_regular = false;
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.navigate(Some("src".into()));
        filter.size_input = "0b".into();
        filter.visible(&listing, "");
        let selected = [0, 4].into_iter().collect();
        assert_eq!(filter.copy_rows(&listing, &selected, false), [0]);
        assert_eq!(filter.copy_rows(&listing, &selected, true), [4, 0]);
        assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
        assert!(filter.save_filter(""));
        let saved = filter.saved_filters[0].clone();
        filter.size_input = "+0".into();
        assert!(filter.save_filter(""));
        assert_eq!(filter.saved_filters.len(), 2);
        let mut query = String::new();
        filter.reset_filters(&mut query);
        filter.visible(&listing, &query);
        assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 0, 2]);
        filter.restore_filter(saved, &mut query);
        filter.visible(&listing, &query);
        assert_eq!(filter.size_input, "0b");
        assert_eq!(filter.copy_rows(&listing, &selected, false), [0]);
        assert!(filter.has_filters(&query));
        filter.size_input = "bad".into();
        assert!(filter.visible(&listing, &query).is_empty());
        filter.reset_filters(&mut query);
        assert!(!filter.has_filters(&query));
        assert!(!filter.visible(&listing, &query).is_empty());
    }

    #[test]
    fn saved_archive_filters_bound_text_count_and_deduplicate_complete_settings() {
        let mut filter = MemberFilter::default();
        assert!(!filter.save_filter(""));
        assert!(filter.save_filter("a"));
        filter.options.regex = true;
        assert!(!filter.save_filter("["));
        assert!(filter.save_filter("a"));
        filter.exclude_matches = true;
        assert!(filter.save_filter("a"));
        filter.kinds.links = false;
        assert!(filter.save_filter("a"));
        assert_eq!(filter.saved_filters.len(), 4);
        assert!(filter.save_filter("a"));
        assert_eq!(filter.saved_filters.len(), 4);
        for index in 0..30 {
            assert!(filter.save_filter(&index.to_string()));
        }
        assert_eq!(filter.saved_filters.len(), 20);
        assert_eq!(filter.saved_filters.back().unwrap().query, "10");
        filter.options.regex = false;
        for index in 10..30 {
            assert!(filter.save_filter(&format!("{}{index}", "😀".repeat(254))));
        }
        assert!(
            filter
                .saved_filters
                .iter()
                .map(|e| e.query.len())
                .sum::<usize>()
                <= 16 * 1024
        );
        let before = filter.saved_filters.clone();
        assert!(!filter.save_filter(&"x".repeat(257)));
        assert_eq!(filter.saved_filters, before);
    }

    #[test]
    fn removing_one_saved_archive_filter_uses_all_settings_and_preserves_active_state() {
        let listing = listing(&["a.txt", "b.rs"]);
        let mut filter = MemberFilter::default();
        assert!(filter.save_filter("a"));
        let first = filter.saved_filters[0].clone();
        filter.options.regex = true;
        assert!(filter.save_filter("a"));
        let second = filter.saved_filters[0].clone();
        filter.visible(&listing, "a");
        filter.focused = Some(0);
        filter.range_anchor = Some(0);
        filter.selection_memory.save(&[1].into_iter().collect(), 2);
        assert!(filter.remove_saved_filter(&first));
        assert_eq!(filter.saved_filters.len(), 1);
        assert_eq!(filter.saved_filters.front(), Some(&second));
        assert!(!filter.remove_saved_filter(&first));
        assert_eq!(filter.visible, [0]);
        assert_eq!(filter.needle.as_deref(), Some("a"));
        assert!(filter.options.regex);
        assert_eq!(filter.focused, Some(0));
        assert_eq!(filter.range_anchor, Some(0));
        let mut selected = Default::default();
        filter.selection_memory.restore(&mut selected, 2);
        assert_eq!(selected, [1].into_iter().collect());
        assert!(filter.remove_saved_filter(&second));
        assert!(filter.saved_filters.is_empty());
        assert_eq!(filter.visible(&listing, "a"), [0]);
    }

    #[test]
    fn saved_filter_search_pointer_input_targets_original_identity_for_recall_and_removal() {
        for remove in [false, true] {
            let listing = listing(&["current.txt", "ёж.txt"]);
            let mut filter = MemberFilter::default();
            assert!(filter.save_filter("ЁЖ"));
            assert!(filter.save_filter("other"));
            let mut query = "current".to_string();
            filter.visible(&listing, &query);
            let ctx = egui::Context::default();
            let mut recalls = 0;
            for frame in 0..17 {
                let mut input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 900.0),
                    )),
                    time: Some(frame as f64 / 60.0),
                    ..Default::default()
                };
                if [3, 4, 6, 7, 12, 13].contains(&frame) {
                    let id = if frame < 6 {
                        egui::Id::new("archive-saved-filters-menu")
                    } else if frame < 12 {
                        egui::Id::new("archive-saved-filter-search-rect")
                    } else {
                        egui::Id::new((
                            if remove {
                                "archive-remove-saved-filter"
                            } else {
                                "archive-saved-filter"
                            },
                            1usize,
                        ))
                    };
                    let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
                    input.events = vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed: [3, 6, 12].contains(&frame),
                            modifiers: Default::default(),
                        },
                    ];
                } else if frame == 8 {
                    input.events.push(egui::Event::Text("ёж".into()));
                }
                crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if filter.saved_filters_ui(ui, &mut query, true) {
                            recalls += 1;
                        }
                    });
                }));
                if (9..12).contains(&frame) {
                    assert_eq!(filter.saved_filter_search, "ёж");
                    assert_eq!(
                        ctx.data(|data| data
                            .get_temp::<usize>(egui::Id::new("archive-saved-filter-matches"))),
                        Some(1)
                    );
                    assert_eq!(query, "current");
                    assert_eq!(filter.needle.as_deref(), Some("current"));
                    assert_eq!(filter.saved_filters.len(), 2);
                }
            }
            if remove {
                assert_eq!(recalls, 0);
                assert_eq!(query, "current");
                assert_eq!(filter.saved_filters.len(), 1);
                assert_eq!(filter.saved_filters[0].query, "other");
            } else {
                assert_eq!(recalls, 1);
                assert_eq!(query, "ЁЖ");
                assert_eq!(filter.saved_filters.len(), 2);
            }
        }
    }

    #[test]
    fn saved_archive_filter_pointer_removal_does_not_recall_the_entry() {
        let listing = listing(&["a.txt", "b.rs"]);
        let mut filter = MemberFilter::default();
        assert!(filter.save_filter("a"));
        assert!(filter.save_filter("b"));
        let mut query = "current".to_string();
        filter.visible(&listing, &query);
        let ctx = egui::Context::default();
        let mut recalls = 0;
        for frame in 0..14 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 900.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4, 10, 11].contains(&frame) {
                let id = if frame < 10 {
                    egui::Id::new("archive-saved-filters-menu")
                } else {
                    egui::Id::new(("archive-remove-saved-filter", 0usize))
                };
                let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 3 || frame == 10,
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if filter.saved_filters_ui(ui, &mut query, true) {
                        recalls += 1;
                    }
                });
            }));
        }
        assert_eq!(recalls, 0);
        assert_eq!(query, "current");
        assert_eq!(filter.needle.as_deref(), Some("current"));
        assert_eq!(filter.saved_filters.len(), 1);
        assert_eq!(filter.saved_filters[0].query, "a");
    }

    #[test]
    fn saved_archive_filter_restore_rebuilds_rows_and_preserves_location_and_selection_owners() {
        let mut listing = listing(&["src/a.txt", "src/B.rs", "hidden/c.txt"]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.options.case_sensitive = true;
        filter.exclude_matches = true;
        filter.kinds.links = false;
        assert!(filter.save_filter(".rs"));
        let saved = filter.saved_filters[0].clone();
        let mut query = "other".to_string();
        filter.reset_filters(&mut query);
        assert_eq!(filter.saved_filters.len(), 1);
        filter.navigate(Some("src".into()));
        filter.order.descending = true;
        filter.narrowed = Some([0, 1].into_iter().collect());
        let selected = [0, 2].into_iter().collect();
        filter.selection_memory.save(&selected, 3);
        let history_len = filter.past.len();
        filter.restore_filter(saved.clone(), &mut query);
        assert_eq!(filter.visible(&listing, &query), [0]);
        assert_eq!(filter.options, saved.options);
        assert_eq!(filter.kinds, saved.kinds);
        assert!(filter.exclude_matches && filter.order.descending);
        assert_eq!(filter.folder, Some("src".into()));
        assert_eq!(filter.narrowed, Some([0, 1].into_iter().collect()));
        assert_eq!(filter.past.len(), history_len);
        let mut restored = Default::default();
        filter.selection_memory.restore(&mut restored, 3);
        assert_eq!(restored, selected);
        assert_eq!(filter.copy_rows(&listing, &selected, true).len(), 2);
        assert!(filter.reset_scroll);
    }

    #[test]
    fn saved_archive_filter_pointer_recall_restores_query_and_flags() {
        let listing = listing(&["a.txt", "b.rs"]);
        let mut filter = MemberFilter::default();
        filter.options.regex = true;
        filter.options.case_sensitive = true;
        filter.exclude_matches = true;
        filter.kinds.links = false;
        assert!(filter.save_filter(r"\.rs$"));
        let mut query = String::new();
        filter.reset_filters(&mut query);
        let ctx = egui::Context::default();
        let mut recalls = 0;
        for frame in 0..14 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 900.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4, 10, 11].contains(&frame) {
                let id = if frame < 10 {
                    egui::Id::new("archive-saved-filters-menu")
                } else {
                    egui::Id::new(("archive-saved-filter", 0usize))
                };
                let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 3 || frame == 10,
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if filter.saved_filters_ui(ui, &mut query, true) {
                        recalls += 1;
                    }
                });
            }));
            filter.visible(&listing, &query);
        }
        assert_eq!(recalls, 1);
        assert_eq!(query, r"\.rs$");
        assert!(filter.options.regex && filter.options.case_sensitive && filter.exclude_matches);
        assert!(!filter.kinds.links);
        assert_eq!(filter.visible(&listing, &query), [0]);
    }

    #[test]
    fn extra_copy_formats_use_sorted_real_rows_and_include_hidden_selection() {
        use crate::archive_export::{Format, format};
        let mut listing = listing(&[
            "src/one.txt",
            "src/two.md",
            "hidden/one.rs",
            "src/deep/file.txt",
        ]);
        listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
        let mut filter = MemberFilter::default();
        filter.navigate(Some("src".into()));
        filter.visible(&listing, "");
        let selected = [0, 1, 2].into_iter().collect();
        let visible = filter.copy_rows(&listing, &selected, false);
        let selected = filter.copy_rows(&listing, &selected, true);
        assert!(visible.iter().all(|&id| id < listing.members.len()));
        let archive = std::path::Path::new("/host/fixture.zip");
        assert_eq!(
            format(archive, &listing, &visible, Format::NamesWithoutExtension).unwrap(),
            "one\ntwo"
        );
        assert_eq!(
            format(archive, &listing, &selected, Format::NamesWithoutExtension).unwrap(),
            "one\none\ntwo"
        );
        assert_eq!(
            format(archive, &listing, &visible, Format::ParentFolders).unwrap(),
            "src"
        );
        assert_eq!(
            format(archive, &listing, &selected, Format::ParentFolders).unwrap(),
            "hidden\nsrc"
        );
    }

    #[test]
    fn member_page_cursor_clamps_in_display_order_and_preserves_hidden_selection() {
        let visible = [2, 9, 6, 7, 1]; // 9 is a synthetic navigation folder.
        assert_eq!(
            step_member_cursor(&visible, Some(2), false, true, 3),
            Some(7)
        );
        assert_eq!(
            step_member_cursor(&visible, Some(7), false, true, usize::MAX),
            Some(1)
        );
        assert_eq!(
            step_member_cursor(&visible, Some(7), true, false, usize::MAX),
            Some(2)
        );
        assert_eq!(step_member_cursor(&visible, None, false, true, 3), Some(2));
        assert_eq!(
            step_member_cursor(&visible, Some(0), true, false, 3),
            Some(2)
        );
        assert_eq!(step_member_cursor(&[], Some(0), false, true, 3), Some(0));
        let mut selected = [0].into_iter().collect();
        let target = step_member_cursor(&visible, Some(2), false, true, 3);
        super::select_member_range(&visible, Some(2), target, 8, &mut selected);
        assert_eq!(selected, [0, 2, 6, 7].into_iter().collect());
        let target = step_member_cursor(&visible, target, true, false, 3);
        super::select_member_range(&visible, Some(2), target, 8, &mut selected);
        assert_eq!(selected, [0, 2, 6, 7].into_iter().collect());
    }

    #[test]
    fn member_page_input_is_focus_scoped_shift_aware_and_preserves_modified_keys() {
        for (focused, modifiers, key, expected) in [
            (
                true,
                egui::Modifiers::NONE,
                egui::Key::PageDown,
                Some((true, 7, false)),
            ),
            (
                true,
                egui::Modifiers::SHIFT,
                egui::Key::PageUp,
                Some((false, 7, true)),
            ),
            (
                true,
                egui::Modifiers::SHIFT,
                egui::Key::ArrowDown,
                Some((true, 1, true)),
            ),
            (true, egui::Modifiers::CTRL, egui::Key::PageDown, None),
            (true, egui::Modifiers::ALT, egui::Key::PageUp, None),
            (false, egui::Modifiers::NONE, egui::Key::PageDown, None),
        ] {
            let ctx = egui::Context::default();
            let id = egui::Id::new("archive-filter");
            let input = egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            };
            crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
                if focused {
                    ctx.memory_mut(|memory| memory.request_focus(id));
                }
                let motion = super::member_cursor_input(&ctx, id, 7);
                assert_eq!(motion.map(|m| (m.down, m.rows, m.extend)), expected);
                assert_eq!(
                    ctx.input(|input| input.key_pressed(key)),
                    expected.is_none()
                );
            }));
        }
    }
}
