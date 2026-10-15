//! Duplicate-finder sheet: groups byte-identical files in the active folder,
//! lets the user pick which copy to keep per group, and trashes the rest.
//! Grouping and keep-policy live in `crate::dedup`.

use super::*;
use crate::dedup::{KeepPolicy, bytes_reclaimed, default_keep, delete_count, trashed_members};
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

/// Progress line shown while the duplicate scan hashes files.
fn scan_progress_text(hashed: usize, to_hash: usize) -> String {
    format!(
        "Comparing file contents\u{2026} {} of {to_hash} files",
        hashed.min(to_hash)
    )
}

/// The folder the groups were found in (all members share one listing).
fn scanned_folder(groups: &[crate::dedup::DupGroup]) -> Option<&std::path::Path> {
    groups.first()?.files.first()?.path.parent()
}

impl App {
    /// Opens the tool and starts hashing the active folder's candidates in
    /// the background; the groups appear when the scan finishes.
    pub(crate) fn open_duplicates(&mut self, ctx: &egui::Context) {
        let repaint = ctx.clone();
        let scan = crate::dedup_scan::spawn(self.ws.duplicate_candidates(), move || {
            repaint.request_repaint();
        });
        self.ui.tools.replace_on(self.ws.active.other());
        self.ui.tools.duplicates = Some(DupState {
            groups: Vec::new(),
            keep: Vec::new(),
            policy: KeepPolicy::KeepShortestPath,
            scan: Some(scan),
        });
    }

    /// Takes the finished scan's groups into the tool. Returns the progress
    /// line while the scan still runs.
    fn poll_duplicate_scan(&mut self, ctx: &egui::Context) -> Option<String> {
        let state = self.ui.tools.duplicates.as_mut()?;
        let scan = state.scan.as_ref()?;
        let Some(groups) = scan.poll() else {
            let progress = scan_progress_text(scan.hashed(), scan.to_hash);
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
            return Some(progress);
        };
        state.keep = groups
            .iter()
            .map(|group| Some(default_keep(group, state.policy)))
            .collect();
        state.groups = groups;
        state.scan = None;
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
                delete_count(&s.groups, &s.keep),
                bytes_reclaimed(&s.groups, &s.keep),
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
                                let radio =
                                    ui.radio_value(keep, Some(fi), label).on_hover_text(path);
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
            self.open_duplicates(ctx);
            return;
        }
        if let Some(p) = new_policy
            && let Some(s) = self.ui.tools.duplicates.as_mut()
        {
            s.policy = p;
            s.keep = s.groups.iter().map(|g| Some(default_keep(g, p))).collect();
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
    use super::{
        copy_label, keep_radio_name, scan_progress_text, scanned_folder, trash_refused_message,
    };
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
