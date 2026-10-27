//! Bounded syntax formatting using the egui-extras fallback lexer already shipped.
use std::path::Path;

pub const SYNTAX_BYTE_LIMIT: usize = 64 * 1024;

pub fn language(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "rs" => Some("rust"),
        "py" | "pyw" => Some("python"),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hxx" => Some("cpp"),
        "toml" => Some("toml"),
        _ => None,
    }
}

pub fn highlight(
    ui: &egui::Ui,
    text: &str,
    language: &'static str,
    width: f32,
    dark: bool,
) -> egui::text::LayoutJob {
    if text.len() > SYNTAX_BYTE_LIMIT {
        return egui::text::LayoutJob::simple(
            text.to_string(),
            egui::FontId::monospace(12.0),
            ui.visuals().text_color(),
            width,
        );
    }
    let theme = if dark {
        egui_extras::syntax_highlighting::CodeTheme::dark(12.0)
    } else {
        egui_extras::syntax_highlighting::CodeTheme::light(12.0)
    };
    let mut job =
        egui_extras::syntax_highlighting::highlight(ui.ctx(), ui.style(), &theme, text, language);
    job.wrap.max_width = width;
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rust_preview_colors_tokens_without_changing_source_or_wrap_budget() {
        let ctx = egui::Context::default();
        let source = "fn main() { let text = \"日本語\"; } // comment\n";
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = highlight(
                ui,
                source,
                language(Path::new("test.RS")).unwrap(),
                240.0,
                true,
            );
            assert_eq!(job.text, source);
            assert_eq!(job.wrap.max_width, 240.0);
            assert!(
                job.sections
                    .iter()
                    .map(|s| s.format.color)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    > 1
            );
        }));
        assert_eq!(language(Path::new("data.json")), None);
    }
    #[test]
    fn oversized_unicode_source_keeps_all_text_without_running_syntax_lexer() {
        let ctx = egui::Context::default();
        let source = "é".repeat(SYNTAX_BYTE_LIMIT / 2 + 1);
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = highlight(ui, &source, "rust", f32::INFINITY, true);
            assert_eq!(job.text, source);
            assert_eq!(job.sections.len(), 1);
        }));
    }
}
