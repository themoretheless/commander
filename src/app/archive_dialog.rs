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

fn content_group_sort_label(sort: crate::dedup::GroupSort) -> &'static str {
    use crate::dedup::GroupSort;
    match sort {
        GroupSort::Scan => "Scan order",
        GroupSort::Savings => "Extra payload bytes",
        GroupSort::Size => "Member size",
        GroupSort::Copies => "Copies",
    }
}
fn content_group_order(
    groups: &[crate::archive_check::ContentGroup],
    sort: crate::dedup::GroupSort,
) -> Vec<usize> {
    use crate::dedup::GroupSort;
    let mut order = (0..groups.len()).collect::<Vec<_>>();
    if sort != GroupSort::Scan {
        order.sort_unstable_by(|&a, &b| {
            let extra = |group: &crate::archive_check::ContentGroup| {
                u128::from(group.size) * group.members.len().saturating_sub(1) as u128
            };
            let comparison = match sort {
                GroupSort::Savings => extra(&groups[b]).cmp(&extra(&groups[a])),
                GroupSort::Size => groups[b].size.cmp(&groups[a].size),
                GroupSort::Copies => groups[b].members.len().cmp(&groups[a].members.len()),
                GroupSort::Scan => std::cmp::Ordering::Equal,
            };
            comparison.then_with(|| a.cmp(&b))
        });
    }
    order
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GroupMatch {
    #[default]
    Literal,
    Glob,
    Regex,
    Exact,
}
impl GroupMatch {
    fn label(self) -> &'static str {
        match self {
            Self::Literal => "Literal",
            Self::Glob => "Glob",
            Self::Regex => "Regex",
            Self::Exact => "Exact",
        }
    }
}
/// Inclusive bounds on total retained members, not the number of extra copies.
fn content_group_copy_bounds(
    min: &str,
    max: &str,
) -> Result<(Option<usize>, Option<usize>), String> {
    let parse = |text: &str| -> Result<Option<usize>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        if text.len() > 20 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Group copy limits need whole nonnegative counts (up to 20 digits); no groups are shown.".into());
        }
        text.parse::<usize>().map(Some).map_err(|_| {
            "Group copy limit is too large for this platform; no groups are shown.".into()
        })
    };
    let (min, max) = (parse(min)?, parse(max)?);
    if min.zip(max).is_some_and(|(min, max)| min > max) {
        return Err("Minimum group copies exceeds maximum; no groups are shown.".into());
    }
    Ok((min, max))
}
/// Cached display permutation only; every action keeps its original scan rank.
#[derive(Default)]
pub struct ContentGroupView {
    sort: crate::dedup::GroupSort,
    query: String,
    min_copies: String,
    max_copies: String,
    mode: GroupMatch,
    basename_only: bool,
    case_sensitive: bool,
    error: Option<String>,
    order: Vec<usize>,
    scanned_totals: Option<crate::archive_check::ContentGroupTotals>,
    shown_totals: Option<crate::archive_check::ContentGroupTotals>,
    ready: bool,
}
impl ContentGroupView {
    fn invalidate(&mut self) {
        self.order.clear();
        self.scanned_totals = None;
        self.shown_totals = None;
        self.error = None;
        self.ready = false;
    }
    fn indices(&mut self, groups: &[crate::archive_check::ContentGroup]) -> &[usize] {
        if !self.ready {
            self.scanned_totals = Some(crate::archive_check::ContentGroupTotals::from_groups(
                groups,
            ));
            self.shown_totals = None;
            let pattern = crate::archive_filter::Pattern::compile(
                &self.query,
                crate::archive_filter::Options {
                    glob: self.mode == GroupMatch::Glob,
                    regex: self.mode == GroupMatch::Regex,
                    exact: self.mode == GroupMatch::Exact,
                    basename_only: self.basename_only,
                    case_sensitive: self.case_sensitive,
                    ..Default::default()
                },
            );
            let filtered = content_group_copy_bounds(&self.min_copies, &self.max_copies).and_then(
                |(min, max)| {
                    pattern.and_then(|pattern| {
                        let mut order = content_group_order(groups, self.sort);
                        order.retain(|&rank| {
                            min.is_none_or(|min| groups[rank].members.len() >= min)
                                && max.is_none_or(|max| groups[rank].members.len() <= max)
                        });
                        if !matches!(pattern, crate::archive_filter::Pattern::All) {
                            let mut matches = vec![false; groups.len()];
                            for (rank, group) in groups.iter().enumerate() {
                                for member in &group.members {
                                    let path = member.path.to_str().ok_or(
                                "Group search needs UTF-8 member paths; no groups are shown.",
                            )?;
                                    matches[rank] |= pattern.matches(path, &path.to_lowercase());
                                }
                            }
                            order.retain(|&rank| matches[rank]);
                        }
                        Ok(order)
                    })
                },
            );
            match filtered {
                Ok(order) => {
                    self.shown_totals =
                        Some(crate::archive_check::ContentGroupTotals::from_groups(
                            order.iter().map(|&rank| &groups[rank]),
                        ));
                    self.order = order;
                }
                Err(error) => {
                    self.order.clear();
                    self.error = Some(error);
                }
            }
            self.ready = true;
        }
        &self.order
    }
    fn ui(&mut self, ui: &mut egui::Ui, page: &mut usize) {
        let previous = (
            self.sort,
            self.query.clone(),
            self.min_copies.clone(),
            self.max_copies.clone(),
            self.mode,
            self.basename_only,
            self.case_sensitive,
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Group order:");
            for sort in crate::dedup::GroupSort::ALL {
                let label=content_group_sort_label(sort);
                let button=ui.selectable_value(&mut self.sort,sort,label).on_hover_text("Display only; actions and exports keep original scan identities. Numeric modes show largest first, with stable scan-order ties. Extra payload bytes = member size × (copies − 1), assuming one keeper. This is uncompressed payload, not compressed disk savings; protection and replica rules may retain more copies.");
                #[cfg(feature="visual-qa")]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-group-sort",label)),button.rect));
                let _=button;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Search groups:");
            let input=ui.add(egui::TextEdit::singleline(&mut self.query).id(egui::Id::new("archive-group-query-input")).desired_width(260.0).char_limit(1024).hint_text("member name or path"))
                .on_hover_text("Show a complete scanned group if any member matches. Up to 256 characters, case-insensitive by default. This does not rescan or change selection. Global actions and reports still include all scanned groups.");
            #[cfg(feature="visual-qa")]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("archive-group-query"),input.rect));
            let _=input;
            let reset=ui.add_enabled(!self.query.is_empty() || !self.min_copies.is_empty() || !self.max_copies.is_empty() || self.mode!=GroupMatch::Literal || self.basename_only || self.case_sensitive,egui::Button::new("Reset group search"));
            #[cfg(feature="visual-qa")]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("archive-group-query-reset"),reset.rect));
            if reset.clicked(){self.query.clear();self.min_copies.clear();self.max_copies.clear();self.mode=GroupMatch::Literal;self.basename_only=false;self.case_sensitive=false;}
        });
        ui.horizontal_wrapped(|ui| {
            for mode in [
                GroupMatch::Literal,
                GroupMatch::Glob,
                GroupMatch::Regex,
                GroupMatch::Exact,
            ] {
                let button = ui.selectable_value(&mut self.mode, mode, mode.label());
                #[cfg(feature = "visual-qa")]
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new(("archive-group-match", mode.label())),
                        button.rect,
                    )
                });
                let _ = button;
            }
            for (value, label, id) in [
                (
                    &mut self.basename_only,
                    "Filename only",
                    "archive-group-query-basename",
                ),
                (
                    &mut self.case_sensitive,
                    "Match case",
                    "archive-group-query-case",
                ),
            ] {
                let button = ui.checkbox(value, label);
                #[cfg(feature = "visual-qa")]
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new(id), button.rect));
                let _ = (button, id);
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Group copies:");
            for (value,label,id) in [(&mut self.min_copies,"Min","archive-group-min-copies"),(&mut self.max_copies,"Max","archive-group-max-copies")] {
                ui.label(label);
                let input=ui.add(egui::TextEdit::singleline(value).id(egui::Id::new((id,"input"))).desired_width(75.0).char_limit(32).hint_text("any"))
                    .on_hover_text("Inclusive bounds on total members in a scanned group, including the keeper. Blank means no bound. Combines with group search; changes shown groups and shown-group actions/reports only. Does not rescan, apply keeper rules or change selection.");
                #[cfg(feature="visual-qa")]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(id),input.rect));
                let _=(input,id);
            }
        });
        if (
            self.sort,
            self.query.clone(),
            self.min_copies.clone(),
            self.max_copies.clone(),
            self.mode,
            self.basename_only,
            self.case_sensitive,
        ) != previous
        {
            self.invalidate();
            *page = 0;
        }
    }
    #[cfg(feature = "visual-qa")]
    pub(crate) fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"sort":content_group_sort_label(self.sort),"order":self.order})
    }
    #[cfg(feature = "visual-qa")]
    pub(crate) fn totals_snapshot(&self) -> serde_json::Value {
        serde_json::json!({"scanned":self.scanned_totals,"shown":self.shown_totals})
    }
    #[cfg(feature = "visual-qa")]
    pub(crate) fn copy_bounds_snapshot(&self) -> serde_json::Value {
        serde_json::json!({"min":self.min_copies,"max":self.max_copies})
    }
    #[cfg(feature = "visual-qa")]
    pub(crate) fn filter_snapshot(&self) -> serde_json::Value {
        serde_json::json!({"query":self.query,"mode":self.mode.label(),"basename_only":self.basename_only,"case_sensitive":self.case_sensitive,"error":self.error})
    }
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

/// Selection edits only, bound to one immutable listing; no filesystem undo.
#[derive(Default)]
pub struct SelectionHistory {
    current: Option<Vec<usize>>,
    members: usize,
    undo: std::collections::VecDeque<Vec<usize>>,
    redo: Vec<Vec<usize>>,
}
impl SelectionHistory {
    const MAX_IDS: usize = 65_536;
    const MAX_STEPS: usize = 32;
    fn reset(&mut self, selected: &std::collections::BTreeSet<usize>, members: usize) {
        *self = Self {
            members,
            ..Default::default()
        };
        if selected.len() <= Self::MAX_IDS && selected.iter().all(|&id| id < members) {
            self.current = Some(selected.iter().copied().collect());
        }
    }
    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    fn observe(&mut self, selected: &std::collections::BTreeSet<usize>, members: usize) {
        if self.members != members
            || self.current.is_none()
            || selected.len() > Self::MAX_IDS
            || selected.iter().any(|&id| id >= members)
        {
            self.reset(selected, members);
            return;
        }
        let previous = self.current.as_ref().unwrap();
        if previous.iter().copied().eq(selected.iter().copied()) {
            return;
        }
        self.redo.clear();
        self.undo.push_back(self.current.take().unwrap());
        self.current = Some(selected.iter().copied().collect());
        while self.undo.len() > Self::MAX_STEPS
            || self.undo.iter().map(Vec::len).sum::<usize>() + selected.len() > Self::MAX_IDS
        {
            self.undo.pop_front();
        }
    }
    fn restore(
        &mut self,
        selected: &mut std::collections::BTreeSet<usize>,
        members: usize,
        redo: bool,
    ) {
        self.observe(selected, members);
        let next = if redo {
            self.redo.pop()
        } else {
            self.undo.pop_back()
        };
        let Some(next) = next else {
            return;
        };
        let current = self.current.replace(next.clone()).unwrap();
        if redo {
            self.undo.push_back(current);
        } else {
            self.redo.push(current);
        }
        *selected = next.into_iter().collect();
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

fn change_shown_content_groups_selection(
    report: &crate::archive_check::Report,
    listing: &crate::archive::ArchiveListing,
    ranks: &[usize],
    select: bool,
    selected: &mut std::collections::BTreeSet<usize>,
) -> Result<(), String> {
    let groups = report
        .content_groups
        .as_ref()
        .ok_or("No completed content-group scan is available.")?;
    let mut rows = std::collections::BTreeSet::new();
    for &rank in ranks {
        let group = groups
            .get(rank)
            .ok_or("A shown group is no longer available; run the scan again.")?;
        rows.extend(content_groups_rows(std::slice::from_ref(group), listing)?);
    }
    if select {
        selected.extend(rows);
    } else {
        selected.retain(|row| !rows.contains(row));
    }
    Ok(())
}

/// Runtime-only replica policy shared by the per-group and global actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
    const ALL: [Self; 6] = [
        Self::First,
        Self::Last,
        Self::Newest,
        Self::Oldest,
        Self::Shallowest,
        Self::Deepest,
    ];
    fn compare(
        self,
        a: &crate::archive::ArchiveMember,
        b: &crate::archive::ArchiveMember,
    ) -> std::cmp::Ordering {
        match self {
            Self::First => a.index.cmp(&b.index),
            Self::Last => b.index.cmp(&a.index),
            Self::Shallowest => content_path_depth(&a.path).cmp(&content_path_depth(&b.path)),
            Self::Deepest => content_path_depth(&b.path).cmp(&content_path_depth(&a.path)),
            Self::Newest | Self::Oldest => match (a.modified, b.modified) {
                (Some(a), Some(b)) => {
                    if self == Self::Newest {
                        b.cmp(&a)
                    } else {
                        a.cmp(&b)
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
        }
    }
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentKeep {
    pub count: usize,
    pub order: ContentKeepOrder,
    #[serde(default)]
    pub tie_breakers: Vec<ContentKeepOrder>,
    pub protected_glob: String,
    pub protect_basename: bool,
    pub protect_case_sensitive: bool,
    pub modified_before: String,
    pub candidate_globs: String,
    #[serde(default)]
    pub preferred_globs: String,
}
impl Default for ContentKeep {
    fn default() -> Self {
        Self {
            count: 1,
            order: ContentKeepOrder::First,
            tie_breakers: Vec::new(),
            protected_glob: String::new(),
            protect_basename: false,
            protect_case_sensitive: true,
            modified_before: String::new(),
            candidate_globs: String::new(),
            preferred_globs: String::new(),
        }
    }
}
impl ContentKeep {
    fn validate_priorities(&self) -> Result<(), String> {
        let mut seen = vec![self.order];
        if self.tie_breakers.len() > 5 {
            return Err("Use at most five distinct tie breakers.".into());
        }
        for &order in &self.tie_breakers {
            if seen.contains(&order) {
                return Err("Priority rules must be distinct, including the main rule.".into());
            }
            seen.push(order);
        }
        Ok(())
    }
    fn tie_breakers_ui(&mut self, ui: &mut egui::Ui) {
        let summary = if self.tie_breakers.is_empty() {
            "entry index".into()
        } else {
            self.tie_breakers
                .iter()
                .map(|order| order.short_label())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let header=egui::CollapsingHeader::new(format!("Break ties: {summary}"))
            .id_salt("archive-copy-tie-breakers").show(ui,|ui| {
                ui.label("Rules run in order only for ties; lowest entry number is the final fallback. Edits apply on Select copies.");
                let mut action=None;
                for rank in 0..self.tie_breakers.len() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("{}. {}",rank+1,self.tie_breakers[rank].label()));
                        for (kind,label,enabled) in [(0,"Up",rank>0),(1,"Down",rank+1<self.tie_breakers.len()),(2,"Remove",true)] {
                            let button=ui.add_enabled(enabled,egui::Button::new(label));
                            #[cfg(feature="visual-qa")]
                            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-tie-edit",rank,kind)),button.rect));
                            if button.clicked(){action=Some((rank,kind));}
                        }
                    });
                }
                if let Some((rank,kind))=action {match kind {0=>self.tie_breakers.swap(rank,rank-1),1=>self.tie_breakers.swap(rank,rank+1),_=>{self.tie_breakers.remove(rank);}}}
                ui.horizontal_wrapped(|ui| {
                    ui.label("Add rule:");
                    for order in ContentKeepOrder::ALL {
                        if order==self.order || self.tie_breakers.contains(&order){continue;}
                        let button=ui.small_button(format!("+ {}",order.short_label()));
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-tie-add",order.label())),button.rect));
                        if button.clicked(){self.tie_breakers.push(order);}
                    }
                });
            });
        #[cfg(feature = "visual-qa")]
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("archive-tie-breakers"),
                header.header_response.rect,
            )
        });
        let _ = header;
    }
    fn rememberable(&self) -> bool {
        self.validate_priorities().is_ok()
            && (1..=100_000).contains(&self.count)
            && self.protection().is_ok()
            && self.candidates().is_ok()
            && self.preferences().is_ok()
            && self.cutoff().is_ok()
    }
    pub(crate) fn saved(&self) -> serde_json::Value {
        if !self.rememberable() {
            return serde_json::Value::Null;
        }
        serde_json::json!({"version":1,"policy":self})
    }
    pub(crate) fn from_saved(value: &serde_json::Value) -> Self {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Saved {
            version: u8,
            policy: ContentKeep,
        }
        serde_json::from_value::<Saved>(value.clone())
            .ok()
            .filter(|s| s.version == 1 && s.policy.rememberable())
            .map(|s| s.policy)
            .unwrap_or_default()
    }
    fn label(&self) -> String {
        format!(
            "{}{} {}",
            if self.protected_glob.lines().all(str::is_empty)
                && self.candidate_globs.lines().all(str::is_empty)
            {
                ""
            } else {
                "at least "
            },
            self.count,
            if self.preferred_globs.lines().all(str::is_empty) {
                self.order.short_label().to_owned()
            } else {
                format!("preferred paths, then {}", self.order.short_label())
            }
        )
    }
    fn protection(&self) -> Result<Vec<crate::archive_filter::Pattern>, String> {
        self.pattern_list(&self.protected_glob, "Protection", "protection")
    }
    fn candidates(&self) -> Result<Vec<crate::archive_filter::Pattern>, String> {
        self.pattern_list(&self.candidate_globs, "Candidate", "candidate")
    }
    fn preferences(&self) -> Result<Vec<crate::archive_filter::Pattern>, String> {
        self.pattern_list(
            &self.preferred_globs,
            "Preferred keeper",
            "preferred keeper",
        )
    }
    fn pattern_list(
        &self,
        input: &str,
        label: &str,
        kind: &str,
    ) -> Result<Vec<crate::archive_filter::Pattern>, String> {
        if input.chars().count() > 8192 {
            return Err(format!("{label} list exceeds 8192 characters."));
        }
        let mut patterns = Vec::new();
        for (line, glob) in input.lines().enumerate() {
            if glob.is_empty() {
                continue;
            }
            if patterns.len() == 16 {
                return Err(format!("Use at most 16 nonempty {kind} patterns."));
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
                .map_err(|error| format!("{label} line {}: {error}", line + 1))?,
            );
        }
        Ok(patterns)
    }
    fn cutoff(&self) -> Result<Option<std::time::SystemTime>, String> {
        let error = "Use a UTC date (2026-01-01) or RFC 3339 timestamp (2026-01-01T12:30:00Z), up to 64 bytes; the boundary is excluded.";
        if self.modified_before.len() > 64 {
            return Err(error.into());
        }
        let text = self.modified_before.trim();
        if text.is_empty() {
            return Ok(None);
        }
        let time = if text.len() == 10 {
            chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map_err(|_| error.to_string())?
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
        } else {
            chrono::DateTime::parse_from_rfc3339(text)
                .map_err(|_| error.to_string())?
                .to_utc()
        };
        let seconds = time.timestamp();
        let nanos = time.timestamp_subsec_nanos();
        if nanos >= 1_000_000_000 {
            return Err(error.into());
        }
        let duration = std::time::Duration::from_secs(seconds.unsigned_abs());
        let base = if seconds >= 0 {
            std::time::UNIX_EPOCH.checked_add(duration)
        } else {
            std::time::UNIX_EPOCH.checked_sub(duration)
        };
        base.and_then(|time| time.checked_add(std::time::Duration::from_nanos(nanos.into())))
            .map(Some)
            .ok_or_else(|| error.into())
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        ui.label(
            egui::RichText::new(
                "Valid copy rules are remembered for the next archive and session.",
            )
            .small(),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Keep copies per group:");
            ui.add(egui::DragValue::new(&mut self.count).range(1..=100_000).speed(1));
            for order in ContentKeepOrder::ALL {
                let label=order.label();
                let button=ui.selectable_value(&mut self.order,order,label).on_hover_text("First/last use original entry numbers. Newest/oldest use member timestamps declared in the archive: known dates first, missing dates last. Shallowest/deepest count parent folders inside the archive. Ties use the ordered tie breakers, then lowest original entry number. First/last already resolve every entry, so later rules have no effect. Groups with fewer members keep them all. Apply with Select copies; changing this policy leaves selection unchanged.");
                if button.clicked(){self.tie_breakers.retain(|&rule|rule!=order);}
                #[cfg(feature="visual-qa")]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-keep-order",label)),button.rect));
                let _=button;
            }
        });
        self.tie_breakers_ui(ui);
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
            let basename=ui.checkbox(&mut self.protect_basename,"Filename only").on_hover_text("For protection, candidates and preferred keepers: match the final filename at any folder depth instead of the complete archive-relative path. Editing this option leaves selection unchanged until Select copies.");
            let case=ui.checkbox(&mut self.protect_case_sensitive,"Match case").on_hover_text("For protection, candidates and preferred keepers: when off, use the archive filter's lowercase-based matching for patterns and filenames. Editing this option leaves selection unchanged until Select copies.");
            #[cfg(feature="visual-qa")]
            ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new("archive-protect-basename"),basename.rect);d.insert_temp(egui::Id::new("archive-protect-case"),case.rect);});
            let _=(basename,case);
        });
        ui.add(egui::Label::new(egui::RichText::new("One glob per line; all matches are kept; copy count is a minimum.").small()).wrap()).on_hover_text("One archive-relative glob per line; up to 16 nonempty patterns, 256 characters each. Any matching pattern protects the member. Empty lines are ignored; spaces are literal. Filename only matches the final name at any depth; Match case controls letter case. In full-path mode, * stays within a folder and ** crosses folders. Empty disables protection. Editing the pattern leaves selection unchanged until Select copies.");
        ui.horizontal_wrapped(|ui| {
            ui.label("Copy candidates (globs):");
            let input=ui.add(egui::TextEdit::multiline(&mut self.candidate_globs)
                .id(egui::Id::new("archive-candidate-globs-input")).desired_width(260.0).desired_rows(2).char_limit(8192)
                .hint_text("backups/**\n*.bak"))
                .on_hover_text("Only members matching any candidate pattern can be selected as extra copies. Empty allows every member. Nonmatching and protected members are kept and count toward the minimum. One glob per line; the same limits, Filename only and Match case options apply to all three lists. Edits apply on Select copies.");
            #[cfg(feature="visual-qa")]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("archive-candidate-globs"),input.rect));
            let _=input;
        });
        ui.label(
            egui::RichText::new("Filename only and Match case apply to all three pattern lists.")
                .small(),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Prefer keepers (globs):");
            let input=ui.add(egui::TextEdit::multiline(&mut self.preferred_globs)
                .id(egui::Id::new("archive-preferred-globs-input")).desired_width(260.0).desired_rows(2).char_limit(8192)
                .hint_text("originals/**\nmaster.*"))
                .on_hover_text("Choose matching eligible members first when filling keeper slots. Multiple matches still compete using the main priority, tie breakers and original entry number; this does not protect every match or increase the keeper count. Protected/noncandidate members remain kept and consume slots first. No match falls back to existing priorities. One glob per line with the same limits and Filename only/Match case options. Apply with Select copies; editing leaves selection unchanged.");
            #[cfg(feature="visual-qa")]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("archive-preferred-globs"),input.rect));
            let _=input;
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Only groups modified before:");
            let input=ui.add(egui::TextEdit::singleline(&mut self.modified_before)
                .id(egui::Id::new("archive-group-before-input")).desired_width(260.0).char_limit(64)
                .hint_text("2026-01-01 or RFC 3339"))
                .on_hover_text("Apply only if every member has a known declared archive modification time strictly before this instant. Dates mean midnight UTC; RFC 3339 offsets are accepted. Newer, equal or unknown dates skip the entire group, preserving its selection. Empty disables the limit. Edits apply on Select copies.");
            #[cfg(feature="visual-qa")]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("archive-group-before"),input.rect));
            let _=input;
        });
        let error = self
            .protection()
            .err()
            .or_else(|| self.candidates().err())
            .or_else(|| self.preferences().err())
            .or_else(|| self.cutoff().err())
            .or_else(|| self.validate_priorities().err());
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
    keep.validate_priorities()?;
    let protection = keep.protection()?;
    let candidates = keep.candidates()?;
    let preferences = keep.preferences()?;
    let cutoff = keep.cutoff()?;
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
    let mut applicable_rows = Vec::new();
    let mut offset = 0;
    for group in groups {
        if group.members.len() < 2 {
            return Err("Content group has fewer than two members; run the scan again.".into());
        }
        let group_rows = &rows[offset..offset + group.members.len()];
        if cutoff.is_some_and(|before| {
            group_rows.iter().any(|&id| {
                listing.members[id]
                    .modified
                    .is_none_or(|time| time >= before)
            })
        }) {
            offset += group.members.len();
            continue;
        }
        applicable_rows.extend_from_slice(group_rows);
        let mut protected = vec![false; group.members.len()];
        let mut preferred = vec![false; group.members.len()];
        if !protection.is_empty() || !candidates.is_empty() || !preferences.is_empty() {
            for (rank, member) in group.members.iter().enumerate() {
                let path = member.path.to_str().ok_or(
                    "Path policy matching requires UTF-8 member paths; selection was not changed.",
                )?;
                let lowercase = if keep.protect_case_sensitive {
                    std::borrow::Cow::Borrowed(path)
                } else {
                    std::borrow::Cow::Owned(path.to_lowercase())
                };
                preferred[rank] = preferences
                    .iter()
                    .any(|pattern| pattern.matches(path, &lowercase));
                if (!candidates.is_empty()
                    && !candidates
                        .iter()
                        .any(|pattern| pattern.matches(path, &lowercase)))
                    || protection
                        .iter()
                        .any(|pattern| pattern.matches(path, &lowercase))
                {
                    protected[rank] = true;
                    keepers.push(rows[offset + rank]);
                }
            }
        }
        let mut ranked = group.members.iter().enumerate().collect::<Vec<_>>();
        ranked.sort_unstable_by(|(left, a), (right, b)| {
            preferred[*right].cmp(&preferred[*left]).then_with(|| {
                std::iter::once(keep.order)
                    .chain(keep.tie_breakers.iter().copied())
                    .map(|order| {
                        order.compare(
                            &listing.members[rows[offset + left]],
                            &listing.members[rows[offset + right]],
                        )
                    })
                    .find(|order| !order.is_eq())
                    .unwrap_or_else(|| a.index.cmp(&b.index))
            })
        });
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
    selected.extend(applicable_rows);
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
    pub(crate) fn archive_preferences(&self) -> &ContentKeep {
        self.ui
            .tools
            .archive
            .as_ref()
            .map(|s| &s.content_keep)
            .filter(|keep| keep.rememberable())
            .unwrap_or(&self.archive_keep_preferences)
    }
    pub(crate) fn restore_archive_preferences(&mut self, value: &serde_json::Value) {
        self.archive_keep_preferences = ContentKeep::from_saved(value);
    }
    fn remember_archive_preferences(&mut self) {
        if let Some(state) = &self.ui.tools.archive
            && state.content_keep != self.archive_keep_preferences
            && state.content_keep.rememberable()
        {
            self.archive_keep_preferences = state.content_keep.clone();
        }
    }
    #[cfg(feature = "visual-qa")]
    pub(crate) fn archive_reopen_preferences_for_qa(
        &mut self,
        ctx: &egui::Context,
    ) -> Result<serde_json::Value, String> {
        let state = self.ui.tools.archive.as_ref().ok_or("Missing archive")?;
        if state.check.is_some() || state.extract.is_some() || state.run.is_some() {
            return Err("Archive is busy".into());
        }
        let path = state.path.clone();
        let side = self.ui.tools.side.unwrap_or_else(|| self.ws.active.other());
        self.remember_archive_preferences();
        let session = self.to_session();
        let valid = self.ui.tools.archive.as_ref().unwrap().content_keep.clone();
        self.ui
            .tools
            .archive
            .as_mut()
            .unwrap()
            .content_keep
            .candidate_globs = "[".into();
        let invalid_pending_fallback = self.to_session().archive_copy_preferences;
        self.ui.tools.archive.as_mut().unwrap().content_keep = valid;
        if invalid_pending_fallback != session.archive_copy_preferences {
            return Err("Invalid pending policy replaced the last valid preferences".into());
        }
        crate::session::save_with(self.persistence.as_ref(), &session, &mut self.session_gate)
            .map_err(|e| format!("{e:?}"))?;
        let loaded = crate::session::load_with(self.persistence.as_ref())
            .value
            .ok_or("Missing saved session")?;
        self.ui.tools.archive = None;
        self.restore_archive_preferences(&serde_json::Value::Null);
        self.restore_archive_preferences(&loaded.archive_copy_preferences);
        let restored = self.archive_preferences().saved();
        if restored != session.archive_copy_preferences {
            return Err("Archive copy preferences changed on reopen".into());
        }
        self.open_archive_on(path, side, ctx);
        Ok(
            serde_json::json!({"saved":session.archive_copy_preferences,"restored":restored,"invalid_pending_fallback":invalid_pending_fallback}),
        )
    }
    pub(crate) fn open_archive(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.open_archive_on(path, self.ws.active.other(), ctx);
    }

    /// Open the archive tool over pane `side`. Retry passes the tool's own
    /// side: clicking in the tool makes its pane active, so recomputing it
    /// from focus would flip the tool to the other pane.
    fn open_archive_on(&mut self, path: PathBuf, side: ActivePanel, ctx: &egui::Context) {
        self.remember_archive_preferences();
        let repaint = ctx.clone();
        let run = crate::archive::start_listing(
            path.clone(),
            std::sync::Arc::new(move || repaint.request_repaint()),
        );
        self.ui.tools.replace_on(side);
        self.ui.tools.archive = Some(ArchiveState {
            content_group_view: ContentGroupView::default(),
            selection_history: SelectionHistory::default(),
            content_group_page: 0,
            content_keep: self.archive_keep_preferences.clone(),
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
                    state
                        .selection_history
                        .reset(&state.selected, listing.members.len());
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
                state.content_group_view.invalidate();
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
        if let Some(state) = self.ui.tools.archive.as_mut()
            && let Some(listing) = &state.listing
        {
            state
                .selection_history
                .observe(&state.selected, listing.members.len());
        }

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
        let mut select_shown_groups = None;
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
                if let Some(listing)=&state.listing {
                    ui.horizontal_wrapped(|ui| {
                        for (redo,label,enabled) in [(false,"Undo selection",state.selection_history.can_undo()),(true,"Redo selection",state.selection_history.can_redo())] {
                            let button=ui.add_enabled(enabled,egui::Button::new(label))
                                .on_hover_text("Restore archive member selection in this listing only, including hidden members. Rules, filters, navigation and saved selection are unchanged. Up to 32 edits and 65,536 stored member IDs; reopening resets history. No filesystem undo.");
                            #[cfg(feature="visual-qa")]
                            ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new(label),button.rect);d.insert_temp(egui::Id::new((label,"enabled")),button.enabled());});
                            if button.clicked() {state.selection_history.restore(&mut state.selected,listing.members.len(),redo);}
                        }
                    });
                }
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
                            state.content_group_view.ui(ui,&mut state.content_group_page);
                            state.content_group_view.indices(groups);
                            if let Some(error)=&state.content_group_view.error {ui.colored_label(ui.visuals().error_fg_color,error);}
                            let order=&state.content_group_view.order;
                            ui.label(format!("Showing {} of {} scanned groups; all members of matching groups remain visible.",order.len(),groups.len()));
                            let totals_hint="Logical uncompressed payload, not archive size or disk savings. Extra copies/bytes assume one keeper per group, independent of selection, keep count, protections and candidate/date rules. Scanned totals cover only retained matching-content groups, not every archive entry.";
                            for (total,scope) in [(state.content_group_view.scanned_totals,"Scanned groups"),(state.content_group_view.shown_totals,"Shown groups")] {
                                let text=total.map(|total|total.label(scope)).unwrap_or_default();
                                #[cfg(feature="visual-qa")]
                                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-group-totals",scope)),text.clone()));
                                if total.is_some(){ui.label(text).on_hover_text(totals_hint);}
                            }
                            ui.small("Extra payload assumes one keeper per group; not compressed disk savings.");
                            ui.horizontal_wrapped(|ui| {
                                for (select,label) in [(true,"Select shown groups"),(false,"Deselect shown groups")] {
                                    let button=ui.add_enabled(!order.is_empty() && state.content_group_view.error.is_none(),egui::Button::new(label))
                                        .on_hover_text("Change every member of the currently shown groups, including members that do not match the search and other group pages. Hidden groups and unrelated selections remain unchanged. Does not apply keeper rules or change files.");
                                    #[cfg(feature="visual-qa")]
                                    ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new(label),button.rect);d.insert_temp(egui::Id::new((label,"enabled")),button.enabled());});
                                    if button.clicked(){select_shown_groups=Some((order.clone(),select));}
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                for (format,label) in [(crate::archive_check_export::Format::Text,"Copy shown groups report"),(crate::archive_check_export::Format::Json,"Copy shown groups JSON"),(crate::archive_check_export::Format::Csv,"Copy shown groups CSV")] {
                                    let button=ui.add_enabled(!order.is_empty() && state.content_group_view.error.is_none(),egui::Button::new(label).small())
                                        .on_hover_text("Copy complete matching groups across all pages, in current group order, with original group indices. Scan totals describe the complete scan; selection and archive files stay unchanged.");
                                    #[cfg(feature="visual-qa")]
                                    ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new(label),button.rect);d.insert_temp(egui::Id::new((label,"enabled")),button.enabled());});
                                    if button.clicked(){copy_check_report=Some(crate::archive_check_export::format_shown_groups(&state.path,result,state.check_recorded_at.as_ref(),order,format));}
                                }
                            });
                            let range=content_group_pager(ui,order.len(), &mut state.content_group_page);
                            for &_rank in &order[range] {
                                let group=&groups[_rank];
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
                                    for (format,label) in [(crate::archive_check_export::Format::Text,"Copy group report"),(crate::archive_check_export::Format::Json,"Copy group JSON"),(crate::archive_check_export::Format::Csv,"Copy group CSV")] {
                                        let button=ui.small_button(label).on_hover_text("Copy every retained member of this group, including other pages. Scan totals remain complete-scan metadata; the report identifies this group's original index.");
                                        #[cfg(feature="visual-qa")]
                                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-content-group-export",_rank,label)),button.rect));
                                        if button.clicked(){copy_check_report=Some(crate::archive_check_export::format_group(&state.path,result,state.check_recorded_at.as_ref(),_rank,format));}
                                    }
                                });
                            }
                        });
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new("archive-content-groups-details"),details.header_response.rect);d.insert_temp(egui::Id::new("archive-content-groups-open"),details.body_response.is_some());});
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

        self.remember_archive_preferences();
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
            state.content_group_view.invalidate();
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
        if let Some((ranks, select)) = select_shown_groups
            && let Some(state) = self.ui.tools.archive.as_mut()
            && let (Some(Ok(report)), Some(listing)) = (&state.check_result, &state.listing)
            && let Err(error) = change_shown_content_groups_selection(
                report,
                listing,
                &ranks,
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
        if let Some(state) = self.ui.tools.archive.as_mut()
            && let Some(listing) = &state.listing
        {
            state
                .selection_history
                .observe(&state.selected, listing.members.len());
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
mod tests;
