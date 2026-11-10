//! Integrity-check result: summary, content-group actions and report exports.

use super::*;

/// What the user asked for while the check result was drawn; applied after the frame.
#[derive(Default)]
pub(super) struct CheckResultActions {
    pub(super) select_content_groups: bool,
    pub(super) select_content_group: Option<(usize, bool)>,
    pub(super) select_shown_groups: Option<(Vec<usize>, bool)>,
    pub(super) select_redundant_groups: Option<Option<usize>>,
    pub(super) copy_check_report: Option<Result<String, String>>,
}

pub(super) fn check_result_ui(ui: &mut egui::Ui, state: &mut ArchiveState) -> CheckResultActions {
    let mut actions = CheckResultActions::default();
    let Some(result) = &state.check_result else {
        return actions;
    };
    match result {
        Ok(report) => {
            let label = ui.label(report.summary());
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("archive-check-result"), label.rect));
            let _ = label;
        }
        Err(error) => {
            let label = ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Check not completed: {error}"),
            );
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("archive-check-result"), label.rect));
            let _ = label;
        }
    }
    if let Ok(report) = result
        && let Some(groups) = &report.content_groups
    {
        let select=ui.add_enabled(!groups.is_empty(),egui::Button::new("Select scanned content matches"))
            .on_hover_text("Add every scanned matching regular entry, including hidden ones, after matching original entry index, path and size to this listing. Existing selection is kept. These are SHA-256 matches from the last completed scan, not a new byte-to-byte comparison.");
        #[cfg(feature = "visual-qa")]
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("archive-select-content-groups"), select.rect)
        });
        actions.select_content_groups = select.clicked();
        let keep_valid = state.content_keep.ui(ui);
        let redundant=ui.add_enabled(!groups.is_empty() && keep_valid,egui::Button::new(format!("Select copies; keep {} in each group",state.content_keep.label())))
            .on_hover_text("Select extra copies using the configured count and keep order. Kept entries are deselected even if previously selected. Unrelated selections stay unchanged. Includes hidden members and all pages; no archive files are changed.");
        #[cfg(feature = "visual-qa")]
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("archive-select-redundant-groups"),
                redundant.rect,
            );
            d.insert_temp(
                egui::Id::new("archive-select-redundant-groups-enabled"),
                redundant.enabled(),
            );
        });
        if redundant.clicked() {
            actions.select_redundant_groups = Some(None);
        }

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
                    if button.clicked(){actions.select_shown_groups=Some((order.clone(),select));}
                }
            });
            ui.horizontal_wrapped(|ui| {
                for (format,label) in [(crate::archive_check_export::Format::Text,"Copy shown groups report"),(crate::archive_check_export::Format::Json,"Copy shown groups JSON"),(crate::archive_check_export::Format::Csv,"Copy shown groups CSV")] {
                    let button=ui.add_enabled(!order.is_empty() && state.content_group_view.error.is_none(),egui::Button::new(label).small())
                        .on_hover_text("Copy complete matching groups across all pages, in current group order, with original group indices. Scan totals describe the complete scan; selection and archive files stay unchanged.");
                    #[cfg(feature="visual-qa")]
                    ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new(label),button.rect);d.insert_temp(egui::Id::new((label,"enabled")),button.enabled());});
                    if button.clicked(){actions.copy_check_report=Some(crate::archive_check_export::format_shown_groups(&state.path,result,state.check_recorded_at.as_ref(),order,format));}
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
                        if button.clicked(){actions.select_content_group=Some((_rank,select));}
                    }
                    let redundant=ui.add_enabled(keep_valid,egui::Button::new(format!("Select copies; keep {}",state.content_keep.label())).small()).on_hover_text("Apply the configured keep count and keep order to this group. Kept entries are deselected; other groups and unrelated selections stay unchanged. Includes hidden members and every page. No files are changed.");
                    #[cfg(feature="visual-qa")]
                    ui.ctx().data_mut(|d|{d.insert_temp(egui::Id::new(("archive-select-redundant-group",_rank)),redundant.rect);d.insert_temp(egui::Id::new(("archive-select-redundant-group-enabled",_rank)),redundant.enabled());});
                    if redundant.clicked(){actions.select_redundant_groups=Some(Some(_rank));}
                    for (format,label) in [(crate::archive_check_export::Format::Text,"Copy group report"),(crate::archive_check_export::Format::Json,"Copy group JSON"),(crate::archive_check_export::Format::Csv,"Copy group CSV")] {
                        let button=ui.small_button(label).on_hover_text("Copy every retained member of this group, including other pages. Scan totals remain complete-scan metadata; the report identifies this group's original index.");
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("archive-content-group-export",_rank,label)),button.rect));
                        if button.clicked(){actions.copy_check_report=Some(crate::archive_check_export::format_group(&state.path,result,state.check_recorded_at.as_ref(),_rank,format));}
                    }
                });
            }
        });
        #[cfg(feature = "visual-qa")]
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("archive-content-groups-details"),
                details.header_response.rect,
            );
            d.insert_temp(
                egui::Id::new("archive-content-groups-open"),
                details.body_response.is_some(),
            );
        });
        let _ = details;
    }
    ui.horizontal_wrapped(|ui| {
        for format in crate::archive_check_export::Format::ALL {
            let button = ui.small_button(format.label());
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new(format.label()), button.rect));
            if button.clicked() {
                actions.copy_check_report = Some(crate::archive_check_export::format(
                    &state.path,
                    result,
                    state.check_strict_snapshot,
                    state.check_recorded_at.as_ref(),
                    format,
                ));
            }
        }
    });
    actions
}
