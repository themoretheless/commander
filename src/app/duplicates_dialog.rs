//! Duplicate-finder sheet: groups byte-identical files in the active folder,
//! lets the user pick which copy to keep per group, and trashes the rest.
//! Grouping and keep-policy live in `crate::dedup`.

use super::*;
use crate::dedup::{KeepPolicy, bytes_reclaimed, delete_count, keep_plan, trashed_members};
use crate::panel::format_size;

/// Toast for a Trash request the workspace refused: the block reason when
/// there is one, else a neutral message with the next step (the request can
/// also fail because the Trash worker could not start, not only when busy).
fn trash_refused_message(reason: Option<String>) -> String {
    reason.unwrap_or_else(|| {
        "Could not start moving duplicates to Trash. \
         Wait for any running operation to finish, then try again."
            .to_string()
    })
}

/// Radio label for one copy: its name and, when known, how long ago it was
/// modified, so the user can judge which copy to keep.
fn copy_label(
    file: &crate::dedup::FileKey,
    now: std::time::SystemTime,
    root: Option<&std::path::Path>,
) -> String {
    let name = file
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = root
        .and_then(|root| file.path.strip_prefix(root).ok())
        .map_or(name, |relative| relative.display().to_string());
    match file.modified {
        Some(modified) => format!(
            "{name}  \u{00b7}  {}",
            crate::reldate::relative_date(modified, now)
        ),
        None => name,
    }
}

fn scoped_copy_label(
    file: &crate::dedup::FileKey,
    now: std::time::SystemTime,
    root: &std::path::Path,
    other: Option<&std::path::Path>,
    recursive: bool,
) -> String {
    if let Some(other) = other {
        let roots = [root, other];
        let owner = crate::dedup_scan::root_owner(&file.path, roots);
        return owner.map_or_else(
            || file.path.display().to_string(),
            |i| {
                format!(
                    "Folder {} / {}",
                    i + 1,
                    copy_label(file, now, Some(roots[i]))
                )
            },
        );
    }
    copy_label(file, now, recursive.then_some(root))
}

/// Accessible name for a keep radio: the copy plus its group, since the
/// radios of every group are siblings and the group header is a separate
/// label.
fn keep_radio_name(label: &str, group: usize, groups: usize, copies: usize) -> String {
    format!(
        "Keep {label} (group {} of {groups}, {copies} copies)",
        group + 1
    )
}

/// Progress line shown while the duplicate scan hashes files.
fn scan_progress_text(hashed: usize, to_hash: usize) -> String {
    format!(
        "Comparing file contents\u{2026} {} of {to_hash} files",
        hashed.min(to_hash)
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeepAction {
    Apply(KeepPolicy),
    KeepAll,
    Undo,
    Redo,
    PreferRoot(usize),
    CopyCsv,
    CopyJson,
    CopyPaths(crate::dedup::PathList),
    Sort(crate::dedup::GroupSort),
}

/// Pick one existing member from the preferred captured owner, retaining
/// unmatched groups and original member indices (independent of view sorting).
fn prefer_root_keepers(
    groups: &[crate::dedup::DupGroup],
    keep: &mut [Option<usize>],
    roots: &[std::path::PathBuf],
    preferred: usize,
) -> usize {
    if preferred >= roots.len() {
        return 0;
    }
    let mut changed = 0;
    for (group, choice) in groups.iter().zip(keep) {
        if let Some(index) = group.files.iter().position(|file| {
            crate::dedup_scan::root_owner(&file.path, roots.iter().map(std::path::PathBuf::as_path))
                == Some(preferred)
        }) && *choice != Some(index)
        {
            *choice = Some(index);
            changed += 1;
        }
    }
    changed
}

fn keep_policy_controls(
    ui: &mut egui::Ui,
    policy: KeepPolicy,
    sort: crate::dedup::GroupSort,
    muted: egui::Color32,
) -> Option<KeepAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(egui::RichText::new("Keep").size(11.0).color(muted));
        for value in KeepPolicy::ALL {
            let label=value.label();
            let response = ui.selectable_label(policy == value, egui::RichText::new(label).size(12.0))
                .on_hover_text("Apply to every group, replacing manual choices. Names use literal native ordering and encoded byte length; path depth counts named parent components in the full path. Unknown values sort last; ties keep the first listed copy.");
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("duplicate-policy", label)), response.rect));
            if response.clicked() && policy != value { action = Some(KeepAction::Apply(value)); }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for (label, value, help) in [
            ("Apply policy", KeepAction::Apply(policy), "Apply the selected rule again to all groups, replacing manual choices."),
            ("Keep all groups", KeepAction::KeepAll, "Skip every group; clear the proposed Trash list while retaining the chosen policy."),
            ("Copy CSV plan", KeepAction::CopyCsv, "Copy every result with its proposed keep/trash/skip action, full path and size. Does not change the plan or files."),
            ("Copy JSON report", KeepAction::CopyJson, "Copy current results and actions with applied scan settings and partial-scan status; no new file reads or removal."),
            ("Copy keeper paths", KeepAction::CopyPaths(crate::dedup::PathList::Keepers), "Copy chosen keeper paths, one absolute UTF-8 path per line. Skipped groups are omitted. Plain text, not shell commands; rejects line breaks and output above 1 MiB."),
            ("Copy proposed Trash paths", KeepAction::CopyPaths(crate::dedup::PathList::ProposedTrash), "Copy current proposed Trash candidates, one absolute UTF-8 path per line. Does not delete or reread files; skipped groups are omitted. Plain text, not shell commands."),
        ] {
            let response = ui.small_button(label).on_hover_text(help);
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("duplicate-policy", label)), response.rect));
            if response.clicked() { action = Some(value); }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.small("Sort groups");
        for value in crate::dedup::GroupSort::ALL {
            let response = ui.selectable_label(sort == value, value.label())
                .on_hover_text(if value == crate::dedup::GroupSort::Scan { "Original scan order; keeper choices stay unchanged." } else { "Largest first; ties use scan order. Potential savings assumes one retained copy in every group, including skipped groups. Keeper choices stay unchanged." });
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("duplicate-sort", value.label())), response.rect));
            if response.clicked() && sort != value { action = Some(KeepAction::Sort(value)); }
        }
    });
    action
}

/// Capture depth only on Apply; toggling the scope retains its bounded policy.
fn scope_controls(ui: &mut egui::Ui, state: &mut DupState) -> bool {
    let mut rescan = false;
    ui.horizontal_wrapped(|ui| {
        let scope=ui.checkbox(&mut state.recursive,"Include subfolders")
            .on_hover_text("Restart with fresh enumeration. Hidden folders are included; symlinks and repeated hard links are excluded. Manual choices are rebuilt.");
        #[cfg(any(test,feature="visual-qa"))]
        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("duplicate-recursive"),scope.rect));
        rescan|=scope.changed();
        ui.label("Subfolder levels");
        let response=ui.add_enabled(state.recursive,egui::TextEdit::singleline(&mut state.depth_draft).id(egui::Id::new("duplicate-depth-input")).desired_width(65.0).char_limit(32).hint_text("Unlimited"))
            .on_hover_text("0 = root files only; 1 = root plus immediate subfolders; empty = unlimited");
        response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::TextEdit,response.enabled(),"Subfolder levels"));
        #[cfg(any(test,feature="visual-qa"))]
        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-depth","input")),response.rect));
        let parsed=crate::dedup_scan::parse_depth(&state.depth_draft);
        let apply=ui.add_enabled(state.recursive && parsed.as_ref().is_ok_and(|depth|*depth!=state.max_depth),egui::Button::new("Apply depth"));
        #[cfg(any(test,feature="visual-qa"))]
        ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new(("duplicate-depth","apply")),apply.rect);d.insert_temp(egui::Id::new("duplicate-depth-apply-enabled"),apply.enabled());});
        if apply.clicked() && let Ok(depth)=parsed {state.max_depth=depth;rescan=true;}
        let reset=ui.add_enabled(state.max_depth.is_some() || !state.depth_draft.is_empty(),egui::Button::new("Reset depth"));
        #[cfg(any(test,feature="visual-qa"))]
        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-depth","reset")),reset.rect));
        if reset.clicked() {rescan|=state.recursive && state.max_depth.is_some();state.max_depth=None;state.depth_draft.clear();}
    });
    if state.recursive {
        ui.small(state.max_depth.map_or_else(
            || "Applied subfolder levels: unlimited".to_string(),
            |depth| format!("Applied subfolder levels: {depth} (0 = root only)"),
        ));
        if let Err(error) = crate::dedup_scan::parse_depth(&state.depth_draft) {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
    rescan
}

/// Draft edits do not change the proposal until an explicit Apply or Reset.
fn size_controls(ui: &mut egui::Ui, state: &mut DupState) -> bool {
    let mut rescan = false;
    ui.horizontal_wrapped(|ui| {
        for (label, key, value) in [("Min bytes", "min", &mut state.min_size), ("Max bytes", "max", &mut state.max_size)] {
            ui.label(label);
            let response = ui.add(egui::TextEdit::singleline(value).id(egui::Id::new(("duplicate-size-input", key))).char_limit(64).desired_width(120.0).hint_text("No limit"));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, response.enabled(), label));
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("duplicate-size", key)), response.rect));
            #[cfg(not(any(test, feature = "visual-qa")))]
            let _ = response;
        }
        let parsed = crate::dedup_scan::SizeRange::parse(&state.min_size, &state.max_size);
        let changed = parsed.as_ref().is_ok_and(|sizes| *sizes != state.sizes);
        let apply = ui.add_enabled(changed, egui::Button::new("Apply size limits"))
            .on_hover_text("Restart this scope with inclusive byte limits, retaining the rule and sorting and rebuilding manual choices. Draft edits alone do not change current results.");
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| { data.insert_temp(egui::Id::new(("duplicate-size", "apply")), apply.rect); data.insert_temp(egui::Id::new("duplicate-size-apply-enabled"), apply.enabled()); });
        if apply.clicked() && let Ok(sizes) = parsed { state.sizes = sizes; rescan = true; }
        let reset = ui.add_enabled(state.sizes != crate::dedup_scan::SizeRange::default() || !state.min_size.is_empty() || !state.max_size.is_empty(), egui::Button::new("Reset size limits"));
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("duplicate-size", "reset")), reset.rect));
        if reset.clicked() {
            rescan |= state.sizes != crate::dedup_scan::SizeRange::default();
            state.sizes = crate::dedup_scan::SizeRange::default();
            state.min_size.clear(); state.max_size.clear();
        }
    });
    ui.small(state.sizes.label());
    if let Err(error) = crate::dedup_scan::SizeRange::parse(&state.min_size, &state.max_size) {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    rescan
}

/// Drafts never retarget existing results or removal choices until Apply.
fn name_controls(ui: &mut egui::Ui, state: &mut DupState) -> bool {
    let mut rescan = false;
    let header=ui.collapsing("Name filters",|ui| {
        ui.small("One basename glob per line · includes are ORed · exclusions win");
        ui.horizontal_wrapped(|ui| {
            for (label,key,value) in [("Include names","include",&mut state.name_draft.include),("Exclude names","exclude",&mut state.name_draft.exclude)] {
                ui.label(label);
                let response=ui.add(egui::TextEdit::multiline(value).id(egui::Id::new(("duplicate-name-input",key))).char_limit(4096).desired_rows(2).desired_width(160.0).hint_text("One glob per line"));
                response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::TextEdit,response.enabled(),label));
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-name",key)),response.rect));
            }
            let response=ui.checkbox(&mut state.name_draft.case,"Match name case");
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-name","case")),response.rect));
            #[cfg(not(any(test,feature="visual-qa")))]
            let _=response;
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Group by");
            for component in crate::duplicate_name_group::Component::ALL {
                let response=ui.selectable_value(&mut state.name_draft.grouping,component,component.label());
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-grouping",component as u8)),response.rect));
                #[cfg(not(any(test,feature="visual-qa")))]
                let _=response;
            }
            let response=ui.add_enabled(state.name_draft.grouping!=crate::duplicate_name_group::Component::Any,egui::Checkbox::new(&mut state.name_draft.grouping_ignore_ascii_case,"Ignore ASCII grouping case")).on_hover_text("Compare the selected name component using ASCII case folding only. Apply name filters restarts this scope; content verification is still required. Parent means the immediate folder name. Relative path includes every component below the most specific captured root; root matching remains literal.");
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("duplicate-grouping-case"),response.rect));
            #[cfg(not(any(test,feature="visual-qa")))]
            let _=response;
        });
        ui.horizontal_wrapped(|ui| {
            let mut enabled=state.name_draft.extension!=crate::duplicate_extension::Mode::Any;
            let response=ui.checkbox(&mut enabled,"Match extensions").on_hover_text("Partition content-verified groups by final extension. Enabling starts with case-sensitive matching. Apply name filters restarts the scan and rebuilds manual choices.");
            if response.changed(){state.name_draft.extension=if enabled{crate::duplicate_extension::Mode::CaseSensitive}else{crate::duplicate_extension::Mode::Any};}
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-name","extension")),response.rect));
            let mut ignore=state.name_draft.extension==crate::duplicate_extension::Mode::IgnoreAsciiCase;
            let response=ui.add_enabled(enabled,egui::Checkbox::new(&mut ignore,"Ignore ASCII extension case")).on_hover_text("For extensions only: .PNG and .png can match. Other Unicode characters and native bytes remain distinct.");
            if response.changed(){state.name_draft.extension=if ignore{crate::duplicate_extension::Mode::IgnoreAsciiCase}else{crate::duplicate_extension::Mode::CaseSensitive};}
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-name","extension-case")),response.rect));
        });
        let parsed=state.name_validation.get(&state.name_draft);
        ui.horizontal_wrapped(|ui| {
            let apply=ui.add_enabled(parsed.is_ok() && state.name_draft!=state.names.rules,egui::Button::new("Apply name filters"))
                .on_hover_text("Restart this scope. Name includes are ORed; exclusions win. Name-component and extension grouping follow content verification. Manual choices are rebuilt.");
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d| {d.insert_temp(egui::Id::new(("duplicate-name","apply")),apply.rect);d.insert_temp(egui::Id::new("duplicate-name-apply-enabled"),apply.enabled());});
            if apply.clicked() && let Ok(names)=&parsed {state.names=names.clone();rescan=true;}
            let reset=ui.add_enabled(state.name_draft!=Default::default() || state.names.rules!=Default::default(),egui::Button::new("Reset name filters"));
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-name","reset")),reset.rect));
            if reset.clicked() {rescan|=state.names.rules!=Default::default();state.name_draft=Default::default();state.names=Default::default();}
        });
        if let Err(error)=parsed {ui.colored_label(ui.visuals().error_fg_color,error);}
    });
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new("duplicate-name-toggle"),
            header.header_response.rect,
        )
    });
    #[cfg(not(any(test, feature = "visual-qa")))]
    let _ = header;
    let names_label = if state.names.rules == Default::default() {
        "Applied names: all basenames".to_string()
    } else {
        format!(
            "Applied names: include {} · exclude {} · case {}",
            if state.names.rules.include.trim().is_empty() {
                "all"
            } else {
                state.names.rules.include.trim()
            },
            if state.names.rules.exclude.trim().is_empty() {
                "none"
            } else {
                state.names.rules.exclude.trim()
            },
            if state.names.rules.case {
                "sensitive"
            } else {
                "insensitive"
            }
        )
    };
    let extension_label = match state.names.rules.extension {
        crate::duplicate_extension::Mode::Any => "any",
        crate::duplicate_extension::Mode::CaseSensitive => "exact",
        crate::duplicate_extension::Mode::IgnoreAsciiCase => "ASCII-insensitive",
    };
    ui.small(format!(
        "{names_label} · extension grouping: {extension_label} · {} ({})",
        state.names.rules.grouping.label(),
        if state.names.rules.grouping_ignore_ascii_case {
            "ASCII-insensitive"
        } else {
            "exact"
        }
    ));
    rescan
}

fn copies_controls(ui: &mut egui::Ui, state: &mut DupState) -> bool {
    let mut rescan = false;
    let header=egui::CollapsingHeader::new(format!("Groups with at least {} copies",state.min_copies.get())).id_salt("duplicate-copy-threshold").show(ui,|ui| {
        ui.small("Only show groups with this many distinct identical files; the keeper rule stays the same");
        ui.horizontal_wrapped(|ui| {
            ui.label("Minimum copies");
            let response=ui.add(egui::TextEdit::singleline(&mut state.copies_draft).id(egui::Id::new("duplicate-copies-input")).char_limit(32).desired_width(80.0).hint_text("2"));
            response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::TextEdit,response.enabled(),"Minimum copies"));
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-copies","input")),response.rect));
            let parsed=crate::dedup_scan::MinCopies::parse(&state.copies_draft);
            let apply=ui.add_enabled(parsed.as_ref().is_ok_and(|n|*n!=state.min_copies),egui::Button::new("Apply minimum"));
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d| {d.insert_temp(egui::Id::new(("duplicate-copies","apply")),apply.rect);d.insert_temp(egui::Id::new("duplicate-copies-apply-enabled"),apply.enabled());});
            if apply.clicked() && let Ok(n)=parsed {state.min_copies=n;rescan=true;}
            let reset=ui.add_enabled(state.min_copies!=Default::default() || !state.copies_draft.is_empty(),egui::Button::new("Reset minimum"));
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("duplicate-copies","reset")),reset.rect));
            if reset.clicked() {rescan|=state.min_copies!=Default::default();state.min_copies=Default::default();state.copies_draft.clear();}
        });
        if let Err(error)=crate::dedup_scan::MinCopies::parse(&state.copies_draft) {ui.colored_label(ui.visuals().error_fg_color,error);}
    });
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new("duplicate-copies-toggle"),
            header.header_response.rect,
        )
    });
    #[cfg(not(any(test, feature = "visual-qa")))]
    let _ = header;
    rescan
}

/// A new scan keeps preferences, but never reuses group-indexed manual choices.
fn duplicate_scan_state(
    scan: crate::dedup_scan::DupScan,
    previous: Option<&DupState>,
    root: PathBuf,
    other_root: PathBuf,
    fresh_enumeration: bool,
) -> DupState {
    DupState {
        other_root: previous.map_or(other_root, |s| s.other_root.clone()),
        include_other: previous.is_some_and(|s| s.include_other),
        cross_only: previous.is_some_and(|s| s.cross_only),
        fresh_enumeration,
        min_copies: previous.map_or_else(Default::default, |s| s.min_copies),
        copies_draft: previous.map_or_else(String::new, |s| s.copies_draft.clone()),
        name_validation: Default::default(),
        name_draft: previous.map_or_else(Default::default, |s| s.name_draft.clone()),
        names: previous.map_or_else(Default::default, |s| s.names.clone()),
        groups: Vec::new(),
        keep: Vec::new(),
        history: Default::default(),
        policy: previous.map_or(KeepPolicy::ShortestPath, |state| state.policy),
        sort: previous.map_or(crate::dedup::GroupSort::default(), |state| state.sort),
        group_order: Vec::new(),
        root,
        recursive: previous.is_some_and(|state| state.recursive),
        max_depth: previous.and_then(|s| s.max_depth),
        depth_draft: previous.map_or_else(String::new, |s| s.depth_draft.clone()),
        sizes: previous.map_or(crate::dedup_scan::SizeRange::default(), |state| state.sizes),
        min_size: previous.map_or_else(String::new, |state| state.min_size.clone()),
        max_size: previous.map_or_else(String::new, |state| state.max_size.clone()),
        scan_issues: 0,
        scan: Some(scan),
    }
}

fn publish_duplicate_scan(state: &mut DupState, groups: Vec<crate::dedup::DupGroup>) {
    state.history.clear();
    state.keep = keep_plan(&groups, state.policy);
    state.group_order = crate::dedup::group_order(&groups, state.sort);
    state.groups = groups;
    state.scan_issues = state.scan.as_ref().map_or(0, |scan| scan.issues());
    state.scan = None;
}

impl App {
    /// Opens the tool and starts hashing the active folder's candidates in
    /// the background; the groups appear when the scan finishes.
    pub(crate) fn open_duplicates(&mut self, ctx: &egui::Context) {
        self.start_duplicate_scan(ctx, false);
    }

    fn start_duplicate_scan(&mut self, ctx: &egui::Context, retain_preferences: bool) {
        let previous = retain_preferences
            .then_some(self.ui.tools.duplicates.as_ref())
            .flatten();
        let root = previous.map_or_else(
            || self.ws.active_panel_ref().current_path.clone(),
            |state| state.root.clone(),
        );
        let other_root = previous.map_or_else(
            || self.ws.inactive_panel().current_path.clone(),
            |s| s.other_root.clone(),
        );
        let include_other = previous.is_some_and(|s| s.include_other);
        let recursive = previous.is_some_and(|s| s.recursive);
        let fresh_enumeration =
            include_other || recursive || self.ws.active_panel_ref().current_path != root;
        let sizes = previous.map_or(crate::dedup_scan::SizeRange::default(), |state| state.sizes);
        let names = previous.map_or_else(Default::default, |s| s.names.clone());
        let min_copies = previous.map_or_else(Default::default, |s| s.min_copies);
        let repaint = ctx.clone();
        let scan = if include_other || (!recursive && fresh_enumeration) {
            let mut roots = vec![root.clone()];
            if include_other {
                roots.push(other_root.clone());
            }
            crate::dedup_scan::spawn_roots_selected(
                roots,
                sizes,
                names,
                if recursive {
                    previous.and_then(|s| s.max_depth)
                } else {
                    Some(0)
                },
                min_copies,
                include_other && previous.is_some_and(|s| s.cross_only),
                move || repaint.request_repaint(),
            )
        } else if recursive {
            crate::dedup_scan::spawn_tree_filtered(
                root.clone(),
                sizes,
                names,
                previous.and_then(|s| s.max_depth),
                min_copies,
                move || repaint.request_repaint(),
            )
        } else {
            crate::dedup_scan::spawn_filtered_at(
                root.clone(),
                self.ws.active_panel_ref().entries().to_vec(),
                sizes,
                names,
                min_copies,
                move || repaint.request_repaint(),
            )
        };
        let state = duplicate_scan_state(scan, previous, root, other_root, fresh_enumeration);
        self.ui.tools.replace_on(self.ws.active.other());
        self.ui.tools.duplicates = Some(state);
    }

    /// Takes the finished scan's groups into the tool. Returns the progress
    /// line while the scan still runs.
    fn poll_duplicate_scan(&mut self, ctx: &egui::Context) -> Option<String> {
        let state = self.ui.tools.duplicates.as_mut()?;
        let scan = state.scan.as_ref()?;
        let Some(groups) = scan.poll() else {
            let progress = if scan.enumerating() {
                if state.include_other {
                    "Finding files in both captured folders…".to_owned()
                } else if state.recursive {
                    "Finding files in subfolders…".to_owned()
                } else {
                    "Finding files in the captured folder…".to_owned()
                }
            } else {
                scan_progress_text(scan.hashed(), scan.total())
            };
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
            return Some(progress);
        };
        publish_duplicate_scan(state, groups);
        None
    }

    pub(crate) fn show_duplicates_tool(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let escape_requested =
            self.take_escape_request(crate::accessibility::EscapeRoute::PaneTool);
        if self.ui.tools.duplicates.is_none() {
            return;
        }
        let t = self.colors;
        if let Some(progress) = self.poll_duplicate_scan(ctx) {
            let mut cancel = escape_requested;
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Duplicate files")
                        .size(13.0)
                        .strong()
                        .color(t.text_primary),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(progress).size(12.0).color(t.text_muted));
                });
                ui.add_space(12.0);
                if ui
                    .add(super::dialog_button(
                        t,
                        super::ButtonRole::Secondary,
                        egui::RichText::new("Cancel").size(13.0),
                    ))
                    .clicked()
                {
                    cancel = true;
                }
            });
            if cancel {
                // Dropping the scan stops its worker.
                self.ui.tools.duplicates = None;
            }
            return;
        }

        let previous = self
            .ui
            .tools
            .duplicates
            .as_ref()
            .filter(|s| s.keep.len() <= crate::duplicate_history::MAX_CHOICES)
            .map(|s| crate::duplicate_history::Snapshot {
                keep: s.keep.clone(),
                policy: s.policy,
            });
        // Snapshot display data so the window body only mutates `keep`.
        let now = std::time::SystemTime::now();
        let (view, delete_total, reclaimed, folder) = {
            let Some(s) = self.ui.tools.duplicates.as_ref() else {
                return;
            };
            let view: Vec<(u64, Vec<(String, String)>)> = s
                .groups
                .iter()
                .map(|g| {
                    let labels = g
                        .files
                        .iter()
                        .map(|f| {
                            (
                                scoped_copy_label(
                                    f,
                                    now,
                                    &s.root,
                                    s.include_other.then_some(s.other_root.as_path()),
                                    s.recursive,
                                ),
                                f.path.display().to_string(),
                            )
                        })
                        .collect();
                    (g.size, labels)
                })
                .collect();
            (
                view,
                delete_count(&s.groups, &s.keep),
                bytes_reclaimed(&s.groups, &s.keep),
                Some(s.root.display().to_string()),
            )
        };

        let mut keep_action = None;
        let mut rescan = false;
        let mut commit = false;
        let mut cancel = false;

        {
            let Some(s) = self.ui.tools.duplicates.as_mut() else {
                return;
            };
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Duplicate files")
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    if let Some(folder) = &folder {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("in {folder}"))
                                    .size(11.0)
                                    .color(t.text_muted),
                            )
                            .truncate(),
                        )
                        .on_hover_text(folder);
                    }
                });
                ui.add_space(8.0);

                ui.horizontal_wrapped(|ui| {
                    let other=ui.add_enabled(s.root!=s.other_root,egui::Checkbox::new(&mut s.include_other,"Include other panel folder"))
                        .on_hover_text(format!("Folder 1: {}\nFolder 2 (captured on open): {}\nRestart using these captured folders; overlapping paths and hard links count once.",s.root.display(),s.other_root.display()));
                    #[cfg(any(test,feature="visual-qa"))]
                    ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("duplicate-other-root"),other.rect));
                    rescan|=other.changed();
                    if s.include_other {
                        let cross=ui.checkbox(&mut s.cross_only,"Only groups spanning folders")
                            .on_hover_text("Require retained distinct file copies in different captured folders. Overlapping paths belong to the most specific root; hard-link aliases remain counted once.");
                        #[cfg(any(test,feature="visual-qa"))]
                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("duplicate-cross-only"),cross.rect));
                        rescan|=cross.changed();
                    }
                });
                if s.include_other {ui.add(egui::Label::new(format!("Folder 2: {}",s.other_root.display())).truncate()).on_hover_text(s.other_root.display().to_string());}
                rescan |= scope_controls(ui,s);
                rescan |= size_controls(ui, s);
                rescan |= name_controls(ui, s);
                rescan |= copies_controls(ui,s);
                if s.scan_issues > 0 {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("Partial scan: {} paths could not be read or changed while scanning.", s.scan_issues));
                }

                if view.is_empty() {
                    ui.label(
                        egui::RichText::new(if s.include_other && s.cross_only {format!("No groups with at least {} copies span both captured folders.",s.min_copies.get())}else if s.min_copies.get()>2 {format!("No groups with at least {} identical copies in this scan.",s.min_copies.get())}else if s.include_other {"No duplicate files in these folders.".to_string()}else{"No duplicate files in this folder.".to_string()})
                            .size(12.0)
                            .color(t.text_muted),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(super::dialog_button(
                                t,
                                super::ButtonRole::Secondary,
                                egui::RichText::new("Close").size(13.0),
                            ))
                            .clicked()
                            || escape_requested
                        {
                            cancel = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add(super::dialog_button(
                                t,
                                super::ButtonRole::Secondary,
                                egui::RichText::new("Rescan").size(13.0),
                            ))
                            .on_hover_text("Repeat this scope; keep the rule and sorting, reset manual choices")
                            .clicked()
                        {
                            rescan = true;
                        }
                    });
                    return;
                }

                keep_action = keep_policy_controls(ui, s.policy, s.sort, t.text_muted);
                ui.horizontal_wrapped(|ui| {
                    for (label,enabled,action) in [("Undo plan edit",s.history.can_undo(),KeepAction::Undo),("Redo plan edit",s.history.can_redo(),KeepAction::Redo)] {
                        let response=ui.add_enabled(enabled,egui::Button::new(label)).on_hover_text("Restore keeper choices and selected rule in this scan only. No filesystem undo. Up to 32 edits and 65,536 stored choices; a rescan clears history.");
                        #[cfg(any(test,feature="visual-qa"))]
                        ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new(("duplicate-history",label)),response.rect));
                        if response.clicked(){keep_action=Some(action);}
                    }
                });

                if s.include_other {
                    ui.horizontal_wrapped(|ui| {
                        for (label,root) in [("Prefer primary folder",0),("Prefer other folder",1)] {
                            let response=ui.small_button(label).on_hover_text("Choose the first scanned copy owned by this captured folder as the one keeper. Other copies, including additional copies in that folder, enter the proposed Trash plan. Groups with no matching copy keep their current choice. Overlapping folders use the most specific owner. Does not move or delete files.");
                            #[cfg(any(test,feature="visual-qa"))]
                            ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new(("duplicate-prefer-root",root)),response.rect));
                            if response.clicked(){keep_action=Some(KeepAction::PreferRoot(root));}
                        }
                    });
                    ui.small("Folder preference picks one keeper per matching group; review all other proposed Trash copies.");
                }


                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);

                // Expanded filters must not force the result area beyond the remaining footer budget.
                let footer_height = ui.spacing().interact_size.y.max(ui.text_style_height(&egui::TextStyle::Body) + 2.0 * ui.spacing().button_padding.y);
                let footer_budget = footer_height + 10.0 + ui.spacing().item_spacing.y;
                let group_height=(ui.available_height()-footer_budget).clamp(0.0,320.0);
                let results_scroll=egui::ScrollArea::vertical()
                    .max_height(group_height)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let groups = view.len();
                        for (group, &source_group) in s.group_order.iter().enumerate() {
                            let Some(((size, labels), keep)) =
                                view.get(source_group).zip(s.keep.get_mut(source_group))
                            else {
                                continue;
                            };
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} copies \u{00b7} {} each",
                                    labels.len(),
                                    format_size(*size)
                                ))
                                .size(11.0)
                                .color(t.accent),
                            );
                            for (fi, (label, path)) in labels.iter().enumerate() {
                                let radio =
                                    ui.radio_value(keep, Some(fi), label).on_hover_text(path);
                                #[cfg(any(test, feature = "visual-qa"))]
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(
                                        egui::Id::new(("duplicate-keep", path.as_str())),
                                        radio.rect,
                                    )
                                });
                                let checked = *keep == Some(fi);
                                radio.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::RadioButton,
                                        true,
                                        checked,
                                        keep_radio_name(label, group, groups, labels.len()),
                                    )
                                });
                            }
                            let label = "Keep all (skip this group)";
                            let radio = ui
                                .radio_value(keep, None, label)
                                .on_hover_text("Move none of these copies to the Trash");
                            #[cfg(any(test, feature = "visual-qa"))]
                            if let Some((_, path)) = labels.first() {
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(
                                        egui::Id::new(("duplicate-skip", path.as_str())),
                                        radio.rect,
                                    )
                                });
                            }
                            let checked = keep.is_none();
                            radio.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::RadioButton,
                                    true,
                                    checked,
                                    format!("Keep all copies (group {} of {groups})", group + 1),
                                )
                            });
                            ui.add_space(6.0);
                        }
                    });
                #[cfg(any(test, feature="visual-qa"))]
                ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("duplicate-results-viewport"),results_scroll.inner_rect));
                #[cfg(not(any(test, feature="visual-qa")))]
                let _=results_scroll;

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{delete_total} file(s) ({}) will move to Trash",
                            format_size(reclaimed)
                        ))
                        .size(11.0)
                        .color(t.text_muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(super::dialog_button(
                                t,
                                super::ButtonRole::Secondary,
                                egui::RichText::new("Cancel").size(13.0),
                            ))
                            .clicked()
                        {
                            cancel = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add(super::dialog_button(
                                t,
                                super::ButtonRole::Secondary,
                                egui::RichText::new("Rescan").size(13.0),
                            ))
                            .on_hover_text("Repeat this scope; keep the rule and sorting, reset manual choices")
                            .clicked()
                        {
                            rescan = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add_enabled(
                                delete_total > 0,
                                super::dialog_button(
                                    t,
                                    super::ButtonRole::Danger,
                                    egui::RichText::new("Move others to Trash").size(13.0),
                                ),
                            )
                            .clicked()
                        {
                            commit = true;
                        }
                    });
                });

                if escape_requested {
                    cancel = true;
                }
            });
        }

        if cancel {
            self.ui.tools.duplicates = None;
            return;
        }
        if rescan {
            // The active pane is the scanned one (the tool covers the other).
            self.start_duplicate_scan(ctx, true);
            return;
        }
        let mut copy_plan = None;
        if let Some(action) = keep_action
            && let Some(s) = self.ui.tools.duplicates.as_mut()
        {
            match action {
                KeepAction::Apply(policy) => {
                    s.policy = policy;
                    s.keep = keep_plan(&s.groups, policy);
                }
                KeepAction::KeepAll => s.keep.fill(None),
                KeepAction::Undo | KeepAction::Redo => {
                    let current = crate::duplicate_history::Snapshot {
                        keep: s.keep.clone(),
                        policy: s.policy,
                    };
                    let restored = if action == KeepAction::Undo {
                        s.history.undo(current)
                    } else {
                        s.history.redo(current)
                    };
                    if let Some(restored) = restored {
                        s.keep = restored.keep;
                        s.policy = restored.policy;
                    }
                }
                KeepAction::PreferRoot(preferred) => {
                    if s.include_other {
                        prefer_root_keepers(
                            &s.groups,
                            &mut s.keep,
                            &[s.root.clone(), s.other_root.clone()],
                            preferred,
                        );
                    }
                }
                KeepAction::CopyCsv => {
                    copy_plan = Some((
                        "duplicate CSV plan",
                        if s.sort == crate::dedup::GroupSort::Scan {
                            crate::dedup::plan_csv(&s.groups, &s.keep)
                        } else {
                            crate::dedup::plan_csv_in_order(&s.groups, &s.keep, &s.group_order)
                        },
                    ));
                }
                KeepAction::CopyJson => {
                    copy_plan = Some(("duplicate JSON report", crate::duplicate_report::json(s)));
                }
                KeepAction::CopyPaths(list) => {
                    let order = if s.sort == crate::dedup::GroupSort::Scan {
                        (0..s.groups.len()).collect()
                    } else {
                        s.group_order.clone()
                    };
                    copy_plan = Some((
                        "duplicate path list",
                        crate::dedup::plan_paths(&s.groups, &s.keep, &order, list),
                    ));
                }
                KeepAction::Sort(sort) => {
                    s.sort = sort;
                    s.group_order = crate::dedup::group_order(&s.groups, sort);
                }
            }
        }
        if !matches!(keep_action, Some(KeepAction::Undo | KeepAction::Redo))
            && let Some(state) = self.ui.tools.duplicates.as_mut()
        {
            if let Some(previous) = previous {
                state.history.record(previous, &state.keep, state.policy);
            } else {
                state.history.clear();
            }
        }
        if let Some((label, result)) = copy_plan {
            match result {
                Ok(text) => self.write_clipboard(&text, label, ctx),
                Err(error) => self.toast(ctx, crate::toasts::ToastKind::Error, error, false),
            }
        }
        if commit {
            if self.ws.mutations_blocked() {
                let reason = self.ws.mutation_block_reason("moving duplicates to Trash");
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Info,
                    trash_refused_message(reason),
                    false,
                );
                return;
            }
            // Every non-kept file goes to the Trash; "Keep all" groups are skipped.
            let to_trash: Vec<crate::ports::TrashBatchItem> = {
                let Some(s) = self.ui.tools.duplicates.as_ref() else {
                    return;
                };
                trashed_members(&s.groups, &s.keep)
                    .map(|(_, file)| {
                        crate::workspace::trash_batch_item_from_listing(
                            file.path.clone(),
                            &file.identity,
                        )
                    })
                    .collect()
            };
            let requested = to_trash.len();
            if requested == 0 {
                return;
            }
            let repaint = ctx.clone();
            if self
                .ws
                .trash_entries(to_trash, move || repaint.request_repaint())
            {
                self.ui.tools.duplicates = None;
            } else {
                let reason = self.ws.mutation_block_reason("moving duplicates to Trash");
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Info,
                    trash_refused_message(reason),
                    false,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{copy_label, keep_radio_name, scan_progress_text, trash_refused_message};
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn file(path: &str, modified: Option<u64>) -> crate::dedup::FileKey {
        crate::dedup::FileKey {
            path: PathBuf::from(path),
            identity: crate::panel::ListingIdentity::Unavailable,
            size: 1,
            hash: 1,
            modified: modified.map(|secs| UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }

    #[test]
    fn preferred_folder_keeps_original_member_ids_and_preserves_unmatched_choices() {
        let roots = vec![PathBuf::from("/data"), PathBuf::from("/data/backup")];
        let group = |paths: &[&str]| crate::dedup::DupGroup {
            size: 1,
            files: paths.iter().map(|path| file(path, None)).collect(),
        };
        let groups = vec![
            group(&["/data/backup/a", "/data/a", "/data/b"]),
            group(&["/data/local-a", "/data/local-b"]),
            group(&["/elsewhere/a", "/elsewhere/b"]),
        ];
        let before = groups.clone();
        let mut keep = vec![Some(0), None, Some(1)];
        assert_eq!(super::prefer_root_keepers(&groups, &mut keep, &roots, 0), 2);
        assert_eq!(keep, vec![Some(1), Some(0), Some(1)]);
        assert_eq!(super::prefer_root_keepers(&groups, &mut keep, &roots, 1), 1);
        assert_eq!(keep, vec![Some(0), Some(0), Some(1)]);
        assert_eq!(super::prefer_root_keepers(&groups, &mut keep, &roots, 1), 0);
        assert_eq!(super::prefer_root_keepers(&groups, &mut keep, &roots, 2), 0);
        assert_eq!(groups, before);
        keep[1] = None;
        super::prefer_root_keepers(&groups, &mut keep, &roots, 1);
        assert!(keep[1].is_none());
        let trash = crate::dedup::trashed_members(&groups, &keep)
            .map(|(_, file)| file.path.clone())
            .collect::<Vec<_>>();
        assert!(
            trash.contains(&PathBuf::from("/data/a")) && trash.contains(&PathBuf::from("/data/b"))
        );
        assert!(
            !trash.contains(&PathBuf::from("/data/backup/a"))
                && !trash.contains(&PathBuf::from("/data/local-a"))
        );
    }
    #[test]
    fn pointer_policy_controls_emit_newest_clear_and_explicit_reapply() {
        use super::{KeepAction, keep_policy_controls};
        use crate::dedup::KeepPolicy;
        let ctx = egui::Context::default();
        let actions = std::cell::RefCell::new(Vec::new());
        let frame = |policy, events| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if let Some(action) = keep_policy_controls(
                        ui,
                        policy,
                        crate::dedup::GroupSort::default(),
                        egui::Color32::GRAY,
                    ) {
                        actions.borrow_mut().push(action);
                    }
                },
            ));
        };
        let mut policy = KeepPolicy::ShortestPath;
        frame(policy, vec![]);
        for (label, expected) in [
            ("newest", KeepAction::Apply(KeepPolicy::Newest)),
            ("Keep all groups", KeepAction::KeepAll),
            ("Apply policy", KeepAction::Apply(KeepPolicy::Newest)),
            ("oldest", KeepAction::Apply(KeepPolicy::Oldest)),
            ("name A–Z", KeepAction::Apply(KeepPolicy::FirstName)),
            ("name Z–A", KeepAction::Apply(KeepPolicy::LastName)),
            ("shortest name", KeepAction::Apply(KeepPolicy::ShortestName)),
            ("longest name", KeepAction::Apply(KeepPolicy::LongestName)),
            (
                "shallowest path",
                KeepAction::Apply(KeepPolicy::ShallowestPath),
            ),
            ("deepest path", KeepAction::Apply(KeepPolicy::DeepestPath)),
            ("Copy CSV plan", KeepAction::CopyCsv),
            ("Copy JSON report", KeepAction::CopyJson),
        ] {
            let pos = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new(("duplicate-policy", label)))
                })
                .unwrap()
                .center();
            let before = actions.borrow().len();
            for phase in 0..3 {
                let mut events = vec![egui::Event::PointerMoved(pos)];
                if phase > 0 {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase == 1,
                        modifiers: Default::default(),
                    });
                }
                frame(policy, events);
            }
            assert_eq!(actions.borrow().len(), before + 1);
            assert_eq!(actions.borrow().last(), Some(&expected));
            if let KeepAction::Apply(value) = expected {
                policy = value;
            }
        }
    }

    #[test]
    fn rescan_retains_preferences_rehashes_and_rebuilds_group_choices() {
        use super::{duplicate_scan_state, publish_duplicate_scan};
        use crate::dedup::{GroupSort, KeepPolicy};
        use crate::panel::FileEntry;

        let dir = crate::testutil::TempDir::new();
        let paths = [
            dir.file("large", &"L".repeat(64)),
            dir.file("large-copy", &"L".repeat(64)),
            dir.file("small-old", "ssssssssss"),
            dir.file("small-middle", "ssssssssss"),
            dir.file("small-new", "ssssssssss"),
        ];
        let candidates: Vec<_> = paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                std::fs::File::options()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_modified(UNIX_EPOCH + Duration::from_secs(100 + index as u64))
                    .unwrap();
                FileEntry::from_meta(path.clone(), &std::fs::metadata(path).unwrap()).unwrap()
            })
            .collect();
        let start = |previous: Option<&super::DupState>| {
            let sizes =
                previous.map_or(crate::dedup_scan::SizeRange::default(), |state| state.sizes);
            duplicate_scan_state(
                crate::dedup_scan::spawn_filtered(
                    candidates.clone(),
                    sizes,
                    previous.map_or_else(Default::default, |s| s.names.clone()),
                    previous.map_or_else(Default::default, |s| s.min_copies),
                    || {},
                ),
                previous,
                dir.path().to_path_buf(),
                dir.path().join("other"),
                false,
            )
        };
        let finish = |state: &mut super::DupState| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let groups = loop {
                if let Some(groups) = state.scan.as_ref().unwrap().poll() {
                    break groups;
                }
                assert!(std::time::Instant::now() < deadline, "scan timed out");
                std::thread::yield_now();
            };
            publish_duplicate_scan(state, groups);
        };
        let mut state = start(None);
        assert_eq!(state.policy, KeepPolicy::ShortestPath);
        assert_eq!(state.sort, GroupSort::Scan);
        finish(&mut state);
        assert_eq!(state.groups.len(), 2);
        state.policy = KeepPolicy::Newest;
        state.sort = GroupSort::Savings;
        state.sizes = crate::dedup_scan::SizeRange::parse("10", "").unwrap();
        state.min_size = "10".into();
        state.min_copies = crate::dedup_scan::MinCopies::parse("3").unwrap();
        state.copies_draft = "invalid".into();
        state.max_depth = Some(1);
        state.depth_draft = "invalid".into();
        state.names = crate::duplicate_name_filter::Rules {
            grouping: crate::duplicate_name_group::Component::ParentName,
            grouping_ignore_ascii_case: true,
            extension: crate::duplicate_extension::Mode::IgnoreAsciiCase,
            include: "*".into(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        state.name_draft = crate::duplicate_name_filter::Rules {
            grouping: crate::duplicate_name_group::Component::Stem,
            extension: crate::duplicate_extension::Mode::CaseSensitive,
            include: "[".into(),
            ..Default::default()
        };
        state.group_order = crate::dedup::group_order(&state.groups, state.sort);
        // Clear the small group's proposal and manually keep the older large file.
        for (group, keep) in state.groups.iter().zip(&mut state.keep) {
            *keep = if group.size == 64 { Some(0) } else { None };
        }
        // Same captured listing and sizes; only a new hash can detect this change.
        std::fs::write(&paths[1], "X".repeat(64)).unwrap();
        let mut rescanned = start(Some(&state));
        assert_eq!(rescanned.policy, KeepPolicy::Newest);
        assert_eq!(rescanned.sort, GroupSort::Savings);
        assert_eq!(rescanned.sizes, state.sizes);
        assert_eq!(rescanned.min_size, "10");
        assert_eq!(rescanned.min_copies, state.min_copies);
        assert_eq!(rescanned.copies_draft, "invalid");
        assert_eq!(rescanned.max_depth, Some(1));
        assert_eq!(rescanned.depth_draft, "invalid");
        assert_eq!(rescanned.names.rules, state.names.rules);
        assert_eq!(rescanned.name_draft, state.name_draft);
        assert!(rescanned.name_draft.compile().is_err());
        assert!(rescanned.groups.is_empty());
        assert!(rescanned.keep.is_empty());
        assert!(rescanned.group_order.is_empty());
        assert!(rescanned.scan.is_some());
        finish(&mut rescanned);
        assert_eq!(rescanned.groups.len(), 1);
        assert_eq!(rescanned.groups[0].size, 10);
        assert_eq!(rescanned.group_order, vec![0]);
        assert_eq!(rescanned.keep.len(), 1);
        let keeper = rescanned.keep[0].expect("reapply newest, not the old skip");
        assert_eq!(rescanned.groups[0].files[keeper].path, paths[4]);
        assert_eq!(
            crate::dedup::delete_count(&rescanned.groups, &rescanned.keep),
            2
        );
        assert_eq!(
            crate::dedup::bytes_reclaimed(&rescanned.groups, &rescanned.keep),
            20
        );
        assert!(rescanned.scan.is_none());

        for (index, path) in paths[2..].iter().enumerate() {
            std::fs::write(path, format!("{index:010}")).unwrap();
        }
        let mut empty = start(Some(&rescanned));
        finish(&mut empty);
        assert!(empty.groups.is_empty());
        assert!(empty.keep.is_empty());
        assert!(empty.group_order.is_empty());
        assert!(empty.scan.is_none());
        assert_eq!(empty.policy, KeepPolicy::Newest);
        assert_eq!(empty.sort, GroupSort::Savings);
        assert_eq!(empty.sizes, state.sizes);
        assert_eq!(empty.min_size, "10");
    }

    #[test]
    fn duplicate_scope_retains_captured_other_root_across_reconstruction() {
        let scan = || {
            crate::dedup_scan::spawn_filtered(
                vec![],
                Default::default(),
                Default::default(),
                Default::default(),
                || {},
            )
        };
        let mut first = super::duplicate_scan_state(
            scan(),
            None,
            "/primary".into(),
            "/captured-other".into(),
            false,
        );
        first.include_other = true;
        first.cross_only = true;
        let next = super::duplicate_scan_state(
            scan(),
            Some(&first),
            first.root.clone(),
            "/changed-panel".into(),
            true,
        );
        assert_eq!(next.other_root, std::path::Path::new("/captured-other"));
        assert!(next.include_other && next.fresh_enumeration && next.cross_only);
        assert_eq!(next.root, first.root);
        let now = UNIX_EPOCH;
        assert_eq!(
            super::scoped_copy_label(
                &file("/primary/copy.bin", None),
                now,
                std::path::Path::new("/primary"),
                Some(std::path::Path::new("/other")),
                false
            ),
            "Folder 1 / copy.bin"
        );
        assert_eq!(
            super::scoped_copy_label(
                &file("/other/n/copy.bin", None),
                now,
                std::path::Path::new("/primary"),
                Some(std::path::Path::new("/other")),
                true
            ),
            "Folder 2 / n/copy.bin"
        );
        assert_eq!(
            super::scoped_copy_label(
                &file("/primary/n/copy.bin", None),
                now,
                std::path::Path::new("/primary"),
                Some(std::path::Path::new("/primary/n")),
                true
            ),
            "Folder 2 / copy.bin"
        );
    }

    #[test]
    fn scan_progress_counts_hashed_files() {
        assert_eq!(
            scan_progress_text(2, 5),
            "Comparing file contents\u{2026} 2 of 5 files"
        );
        assert!(scan_progress_text(9, 5).ends_with("5 of 5 files"));
    }

    #[test]
    fn refused_trash_says_why_or_what_to_do() {
        assert_eq!(
            trash_refused_message(Some("Wait for the current Trash operation".to_string())),
            "Wait for the current Trash operation"
        );
        assert!(trash_refused_message(None).contains("try again"));
    }

    #[test]
    fn copies_show_their_age_and_group() {
        let now = UNIX_EPOCH + Duration::from_secs(10 * 86_400);
        let dated = copy_label(&file("/d/a.txt", Some(0)), now, None);
        assert!(dated.starts_with("a.txt  \u{00b7}  "), "{dated}");
        assert_ne!(dated, "a.txt  \u{00b7}  ");
        let root = std::path::Path::new("/d");
        assert_eq!(
            copy_label(&file("/d/nested/copy.bin", None), now, Some(root)),
            "nested/copy.bin"
        );
        assert_eq!(
            copy_label(&file("/d/.hidden/deep/copy.bin", None), now, Some(root)),
            ".hidden/deep/copy.bin"
        );
        assert_eq!(copy_label(&file("/d/b.txt", None), now, None), "b.txt");
        assert_eq!(
            keep_radio_name("a.txt", 0, 2, 3),
            "Keep a.txt (group 1 of 2, 3 copies)"
        );
    }
}
