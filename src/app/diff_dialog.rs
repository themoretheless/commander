//! Read-only unified diff sheet. The diff is computed by `crate::textdiff`;
//! this file only reads the files and renders the result.

use super::*;
use crate::textdiff::{DiffKind, change_counts, diff_lines};
use std::path::Path;

/// Largest file we will read into memory for a diff.
const DIFF_SIZE_CAP: u64 = 2 * 1024 * 1024;

/// Read a regular file as UTF-8, never more than `DIFF_SIZE_CAP` bytes.
/// FIFOs and devices are rejected (they would block or stream forever on the
/// UI thread), and the read itself is bounded in case the file grows.
fn read_text(path: &Path) -> Result<String, ()> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|_| ())?;
    if !meta.is_file() || meta.len() > DIFF_SIZE_CAP {
        return Err(());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| ())?
        .take(DIFF_SIZE_CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > DIFF_SIZE_CAP {
        return Err(());
    }
    String::from_utf8(bytes).map_err(|_| ())
}

impl App {
    pub(crate) fn open_diff(&mut self, ctx: &egui::Context) {
        match self.ws.diff_targets() {
            None => {
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Info,
                    "Select a file pair to diff",
                    false,
                );
            }
            Some((a, b)) => {
                let name = |path: &Path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default()
                };
                let (lines, message) = match (read_text(&a), read_text(&b)) {
                    (Ok(ta), Ok(tb)) => match diff_lines(&ta, &tb) {
                        Ok(lines) => (lines, None),
                        Err(_) => (
                            Vec::new(),
                            Some(
                                "These files contain too many line combinations to compare safely."
                                    .to_string(),
                            ),
                        ),
                    },
                    _ => (
                        Vec::new(),
                        Some("One or both files are binary, too large, or unreadable.".to_string()),
                    ),
                };
                self.ui.tools.replace_on(self.ws.active.other());
                self.ui.tools.diff = Some(DiffState {
                    name_a: name(&a),
                    name_b: name(&b),
                    lines,
                    message,
                });
            }
        }
    }

    pub(crate) fn show_diff_tool(&mut self, ui: &mut egui::Ui) {
        let escape_requested =
            self.take_escape_request(crate::accessibility::EscapeRoute::PaneTool);
        let Some(state) = &self.ui.tools.diff else {
            return;
        };
        let t = self.colors;
        let mut close = false;

        let (ins, del) = change_counts(&state.lines);

        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}  vs  {}", state.name_a, state.name_b))
                        .size(13.0)
                        .strong()
                        .color(t.text_primary),
                );
                if state.message.is_none() {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!("+{ins}"))
                            .size(11.0)
                            .color(t.accent),
                    );
                    ui.label(
                        egui::RichText::new(format!("-{del}"))
                            .size(11.0)
                            .color(t.accent_red),
                    );
                }
            });
            ui.add_space(8.0);

            if let Some(msg) = &state.message {
                ui.label(egui::RichText::new(msg).size(12.0).color(t.text_muted));
            } else if state.lines.is_empty() {
                ui.label(
                    egui::RichText::new("The files are identical.")
                        .size(12.0)
                        .color(t.text_muted),
                );
            } else {
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for line in &state.lines {
                            let (mark, color) = match line.kind {
                                DiffKind::Equal => (" ", t.text_muted),
                                DiffKind::Insert => ("+", t.accent),
                                DiffKind::Delete => ("-", t.accent_red),
                            };
                            ui.label(
                                egui::RichText::new(format!("{mark} {}", line.text))
                                    .monospace()
                                    .size(12.0)
                                    .color(color),
                            );
                        }
                    });
            }

            ui.add_space(10.0);
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
                    close = true;
                }
            });
        });

        if close {
            self.ui.tools.diff = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::read_text;

    #[test]
    fn reads_regular_text_files() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.file("a.txt", "one\ntwo\n");
        assert_eq!(read_text(&path), Ok("one\ntwo\n".to_string()));
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert_eq!(read_text(&path), Err(()));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_devices_instead_of_streaming_them() {
        // A device reports length 0, so only the file-type check stops it.
        assert_eq!(read_text(std::path::Path::new("/dev/null")), Err(()));
    }
}
