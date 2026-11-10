//! Content-index details: freshness, coverage, rebuild and exclusions.

use super::*;

#[derive(Default)]
pub(super) struct IndexPanelActions {
    pub(super) rebuild: bool,
    pub(super) apply_exclusions: bool,
}

pub(super) fn index_details_ui(
    ui: &mut egui::Ui,
    state: &mut FindState,
    t: crate::theme::ThemeColors,
    index_status: &crate::content_index::IndexStatus,
) -> IndexPanelActions {
    let mut actions = IndexPanelActions::default();
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let phase_color = match index_status.phase {
            crate::content_index::IndexPhase::Error => t.accent_red,
            crate::content_index::IndexPhase::Building
            | crate::content_index::IndexPhase::WaitingForIdle => t.accent_warning,
            crate::content_index::IndexPhase::Ready => t.accent,
            _ => t.text_muted,
        };
        ui.label(
            egui::RichText::new("Content index")
                .size(11.0)
                .strong()
                .color(t.text_secondary),
        );
        ui.label(
            egui::RichText::new(index_phase_label(index_status.phase))
                .size(11.0)
                .color(phase_color),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(index_status.enabled, egui::Button::new("Rebuild"))
                .clicked()
            {
                actions.rebuild = true;
            }
        });
    });

    if matches!(
        index_status.phase,
        crate::content_index::IndexPhase::Building
            | crate::content_index::IndexPhase::WaitingForIdle
    ) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                egui::RichText::new(format!(
                    "{} scanned  |  {} indexed",
                    index_status.progress.scanned, index_status.progress.indexed
                ))
                .size(11.0)
                .color(t.text_muted),
            );
        });
    }

    let coverage = index_status.coverage_percent();
    ui.add(
        egui::ProgressBar::new((coverage / 100.0).clamp(0.0, 1.0)).text(format!(
            "{coverage:.0}% text coverage  |  {} files  |  {}",
            index_status.progress.files_seen,
            format_size(index_status.progress.content_bytes as u64)
        )),
    );
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "Freshness: {}",
                index_freshness(index_status.built_at_secs)
            ))
            .size(10.0)
            .color(t.text_muted),
        );
        if let Some(skipped) = index_skipped_summary(
            index_status.skipped_binary,
            index_status.skipped_large,
            index_status.skipped_unreadable,
            index_status.skipped_quota,
        ) {
            ui.label(egui::RichText::new(skipped).size(10.0).color(t.text_muted))
                .on_hover_text(format!(
                    "Text is indexed for files up to {} and up to {} in total",
                    format_size(crate::content_index::MAX_FILE_BYTES),
                    format_size(crate::content_index::MAX_CONTENT_BYTES as u64)
                ));
        }
        if index_status.truncated {
            ui.label(
                egui::RichText::new("Document cap reached")
                    .size(10.0)
                    .color(t.accent_warning),
            );
        }
    });
    if let Some(error) = &index_status.last_error {
        ui.label(egui::RichText::new(error).size(10.0).color(t.accent_red));
    }
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "Excluded paths ({})",
                index_status.excluded_roots.len()
            ))
            .size(10.0)
            .color(t.text_muted),
        );
        if ui.button("Apply").clicked() {
            actions.apply_exclusions = true;
        }
    });
    ui.add(
        egui::TextEdit::multiline(&mut state.index_exclusions)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("cache\nbuild"),
    );
    ui.add_space(4.0);
    ui.separator();
    actions
}
