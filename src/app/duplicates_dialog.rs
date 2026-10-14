//! Duplicate-finder sheet: groups byte-identical files in the active folder,
//! lets the user pick which copy to keep per group, and trashes the rest.
//! Grouping and keep-policy live in `crate::dedup`.

use super::*;
use crate::dedup::{KeepPolicy, bytes_reclaimed, default_keep, delete_count};
use crate::panel::format_size;

/// Toast for a Trash request the workspace refused: the block reason when
/// there is one, else a generic busy message with the next step.
fn trash_refused_message(reason: Option<String>) -> String {
    reason.unwrap_or_else(|| {
        "Could not move duplicates to Trash while another operation is running. \
         Wait for it to finish, then try again."
            .to_string()
    })
}

/// Radio label for one copy: its name and, when known, how long ago it was
/// modified, so the user can judge which copy to keep.
fn copy_label(file: &crate::dedup::FileKey, now: std::time::SystemTime) -> String {
    let name = file
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    match file.modified {
        Some(modified) => format!(
            "{name}  \u{00b7}  {}",
            crate::reldate::relative_date(modified, now)
        ),
        None => name,
    }
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

/// The folder the groups were found in (all members share one listing).
fn scanned_folder(groups: &[crate::dedup::DupGroup]) -> Option<&std::path::Path> {
    groups.first()?.files.first()?.path.parent()
}

impl App {
    pub(crate) fn open_duplicates(&mut self) {
        let policy = KeepPolicy::KeepShortestPath;
        let groups = self.ws.find_duplicates();
        let keep = groups
            .iter()
            .map(|group| default_keep(group, policy))
            .collect();
        self.ui.tools.replace_on(self.ws.active.other());
        self.ui.tools.duplicates = Some(DupState {
            groups,
            keep,
            policy,
        });
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
                        .map(|f| (copy_label(f, now), f.path.display().to_string()))
                        .collect();
                    (g.size, labels)
                })
                .collect();
            (
                view,
                delete_count(&s.groups),
                bytes_reclaimed(&s.groups),
                scanned_folder(&s.groups).map(|p| p.display().to_string()),
            )
        };

        let mut new_policy: Option<KeepPolicy> = None;
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

                if view.is_empty() {
                    ui.label(
                        egui::RichText::new("No duplicate files in this folder.")
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
                            .on_hover_text("Scan the active folder again")
                            .clicked()
                        {
                            rescan = true;
                        }
                    });
                    return;
                }

                // Keep policy selector.
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Keep").size(11.0).color(t.text_muted));
                    for (p, text) in [
                        (KeepPolicy::KeepShortestPath, "shortest path"),
                        (KeepPolicy::KeepOldest, "oldest"),
                    ] {
                        if ui
                            .selectable_label(s.policy == p, egui::RichText::new(text).size(12.0))
                            .clicked()
                            && s.policy != p
                        {
                            new_policy = Some(p);
                        }
                    }
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let groups = view.len();
                        for (group, ((size, labels), keep)) in
                            view.iter().zip(s.keep.iter_mut()).enumerate()
                        {
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
                                let radio = ui.radio_value(keep, fi, label).on_hover_text(path);
                                let checked = *keep == fi;
                                radio.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::RadioButton,
                                        true,
                                        checked,
                                        keep_radio_name(label, group, groups, labels.len()),
                                    )
                                });
                            }
                            ui.add_space(6.0);
                        }
                    });

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
                            .on_hover_text("Scan the active folder again")
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
            self.open_duplicates();
            return;
        }
        if let Some(p) = new_policy
            && let Some(s) = self.ui.tools.duplicates.as_mut()
        {
            s.policy = p;
            s.keep = s.groups.iter().map(|g| default_keep(g, p)).collect();
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
            // Every non-kept file across all groups goes to the Trash.
            let to_trash: Vec<crate::ports::TrashBatchItem> = {
                let Some(s) = self.ui.tools.duplicates.as_ref() else {
                    return;
                };
                s.groups
                    .iter()
                    .zip(&s.keep)
                    .flat_map(|(g, &keep)| {
                        g.files
                            .iter()
                            .enumerate()
                            .filter(move |(fi, _)| *fi != keep)
                            .map(|(_, file)| {
                                crate::workspace::trash_batch_item_from_listing(
                                    file.path.clone(),
                                    &file.identity,
                                )
                            })
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
    use super::{copy_label, keep_radio_name, scanned_folder, trash_refused_message};
    use std::path::{Path, PathBuf};
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
        let dated = copy_label(&file("/d/a.txt", Some(0)), now);
        assert!(dated.starts_with("a.txt  \u{00b7}  "), "{dated}");
        assert_ne!(dated, "a.txt  \u{00b7}  ");
        assert_eq!(copy_label(&file("/d/b.txt", None), now), "b.txt");
        assert_eq!(
            keep_radio_name("a.txt", 0, 2, 3),
            "Keep a.txt (group 1 of 2, 3 copies)"
        );
    }

    #[test]
    fn scanned_folder_comes_from_the_groups() {
        assert_eq!(scanned_folder(&[]), None);
        let group = crate::dedup::DupGroup {
            size: 1,
            files: vec![file("/photos/a.jpg", None), file("/photos/b.jpg", None)],
        };
        assert_eq!(scanned_folder(&[group]), Some(Path::new("/photos")));
    }
}
