//! Duplicate-content groups: ordering, keep policy and bulk selection.

use super::*;

pub(super) fn content_group_pager(
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
pub(super) fn content_group_order(
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
pub(super) enum GroupMatch {
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
pub(super) fn content_group_copy_bounds(
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
    pub(super) sort: crate::dedup::GroupSort,
    pub(super) query: String,
    pub(super) min_copies: String,
    pub(super) max_copies: String,
    pub(super) mode: GroupMatch,
    pub(super) basename_only: bool,
    pub(super) case_sensitive: bool,
    pub(super) error: Option<String>,
    pub(super) order: Vec<usize>,
    pub(super) scanned_totals: Option<crate::archive_check::ContentGroupTotals>,
    pub(super) shown_totals: Option<crate::archive_check::ContentGroupTotals>,
    pub(super) ready: bool,
}
impl ContentGroupView {
    pub(super) fn invalidate(&mut self) {
        self.order.clear();
        self.scanned_totals = None;
        self.shown_totals = None;
        self.error = None;
        self.ready = false;
    }
    pub(super) fn indices(&mut self, groups: &[crate::archive_check::ContentGroup]) -> &[usize] {
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
    pub(super) fn ui(&mut self, ui: &mut egui::Ui, page: &mut usize) {
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

pub(super) fn content_group_rows(
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

pub(super) fn change_content_group_selection(
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

pub(super) fn change_shown_content_groups_selection(
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
    pub(super) const ALL: [Self; 6] = [
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
    pub(super) fn rememberable(&self) -> bool {
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
    pub(super) fn label(&self) -> String {
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
    pub(super) fn protection(&self) -> Result<Vec<crate::archive_filter::Pattern>, String> {
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
    pub(super) fn cutoff(&self) -> Result<Option<std::time::SystemTime>, String> {
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
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) -> bool {
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
pub(super) fn content_path_depth(path: &std::path::Path) -> usize {
    path.components()
        .filter(|part| matches!(part, std::path::Component::Normal(_)))
        .count()
        .saturating_sub(1)
}

/// Resolve the entire proposal before removing keepers from the selection.
pub(super) fn select_redundant_content_groups(
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
