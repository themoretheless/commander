//! Static document formatting. No URLs are fetched and no embedded code runs.
use egui::{Color32, FontId, Stroke, TextFormat, text::LayoutJob};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::path::Path;

const EVENT_LIMIT: usize = 20_000;
const INPUT_LIMIT: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Markdown,
    Html,
}

pub fn kind(path: &Path) -> Option<DocumentKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "md" | "markdown" | "mdown" | "mkd" => Some(DocumentKind::Markdown),
        "html" | "htm" | "xhtml" => Some(DocumentKind::Html),
        _ => None,
    }
}

#[derive(Clone, Copy, Default)]
struct Style {
    heading: u8,
    bold: usize,
    italic: usize,
    code: usize,
    strike: usize,
    link: usize,
}

fn append(job: &mut LayoutJob, text: &str, style: Style, color: Color32) {
    let size = match style.heading {
        1 => 26.0,
        2 => 22.0,
        3 => 19.0,
        4.. => 16.0,
        _ => 13.0,
    };
    let mut format = TextFormat {
        font_id: if style.code > 0 {
            FontId::monospace(12.0)
        } else {
            FontId::proportional(size + if style.bold > 0 { 0.5 } else { 0.0 })
        },
        color,
        italics: style.italic > 0,
        strikethrough: if style.strike > 0 {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        underline: if style.link > 0 {
            Stroke::new(0.5, color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    };
    if style.code > 0 {
        format.background = color.linear_multiply(0.08);
    }
    job.append(text, 0.0, format);
}

fn newline(job: &mut LayoutJob, color: Color32) {
    if !job.text.is_empty() && !job.text.ends_with('\n') {
        append(job, "\n", Style::default(), color);
    }
}

pub fn layout(input: &str, kind: DocumentKind, color: Color32, width: f32) -> LayoutJob {
    let end = input.floor_char_boundary(input.len().min(INPUT_LIMIT));
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let limited = match kind {
        DocumentKind::Markdown => markdown(&input[..end], &mut job, color),
        DocumentKind::Html => html(&input[..end], &mut job, color),
    };
    if limited || end < input.len() {
        append(
            &mut job,
            "\n… Document preview limit reached. Use Source to see the loaded text.",
            Style::default(),
            color,
        );
    }
    job
}

fn markdown(input: &str, job: &mut LayoutJob, color: Color32) -> bool {
    let mut style = Style::default();
    let mut lists: Vec<Option<u64>> = Vec::new();
    for (index, event) in Parser::new_ext(
        input,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    )
    .enumerate()
    {
        if index >= EVENT_LIMIT {
            return true;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    newline(job, color);
                    style.heading = level as u8;
                }
                Tag::Strong => style.bold += 1,
                Tag::Emphasis => style.italic += 1,
                Tag::Strikethrough => style.strike += 1,
                Tag::CodeBlock(_) => {
                    newline(job, color);
                    style.code += 1;
                }
                Tag::Link { .. } => style.link += 1,
                Tag::Image { .. } => append(job, "[Image: ", style, color),
                Tag::List(start) => lists.push(start),
                Tag::Item => {
                    newline(job, color);
                    let marker = match lists.last_mut() {
                        Some(Some(n)) => {
                            let text = format!("{n}. ");
                            *n = n.saturating_add(1);
                            text
                        }
                        _ => "• ".into(),
                    };
                    append(
                        job,
                        &format!(
                            "{}{marker}",
                            "  ".repeat(lists.len().saturating_sub(1).min(32))
                        ),
                        style,
                        color,
                    );
                }
                Tag::BlockQuote(_) => {
                    newline(job, color);
                    append(job, "│ ", style, color);
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    newline(job, color);
                    style.heading = 0;
                }
                TagEnd::Strong => style.bold = style.bold.saturating_sub(1),
                TagEnd::Emphasis => style.italic = style.italic.saturating_sub(1),
                TagEnd::Strikethrough => style.strike = style.strike.saturating_sub(1),
                TagEnd::Link => style.link = style.link.saturating_sub(1),
                TagEnd::Image => append(job, "]", style, color),
                TagEnd::CodeBlock => {
                    newline(job, color);
                    style.code = style.code.saturating_sub(1);
                }
                TagEnd::List(_) => {
                    lists.pop();
                    newline(job, color);
                }
                TagEnd::Paragraph
                | TagEnd::Item
                | TagEnd::TableRow
                | TagEnd::TableHead
                | TagEnd::BlockQuote(_) => newline(job, color),
                TagEnd::TableCell => append(job, "  │  ", style, color),
                _ => {}
            },
            Event::Text(text) => append(job, &text, style, color),
            Event::Code(text) => append(job, &text, Style { code: 1, ..style }, color),
            Event::SoftBreak => append(job, " ", style, color),
            Event::HardBreak => append(job, "\n", style, color),
            Event::Rule => {
                newline(job, color);
                append(job, "────────────\n", style, color);
            }
            Event::TaskListMarker(checked) => {
                append(job, if checked { "☑ " } else { "☐ " }, style, color)
            }
            // Raw HTML stays literal inside Markdown; it never becomes active content.
            Event::Html(text) | Event::InlineHtml(text) => {
                append(job, &text, Style { code: 1, ..style }, color)
            }
            _ => {}
        }
    }
    false
}

fn html(input: &str, job: &mut LayoutJob, color: Color32) -> bool {
    use html5gum::{DefaultEmitter, Token, Tokenizer};
    let mut emitter = DefaultEmitter::default();
    emitter.naively_switch_states(true);
    let mut style = Style::default();
    let mut hidden: Vec<String> = Vec::new();
    for (index, token) in Tokenizer::new_with_emitter(input, emitter).enumerate() {
        if index >= EVENT_LIMIT {
            return true;
        }
        match token {
            Ok(Token::StartTag(tag)) => {
                let name = String::from_utf8_lossy(&tag.name).to_ascii_lowercase();
                if matches!(
                    name.as_str(),
                    "script" | "style" | "head" | "template" | "iframe" | "object"
                ) {
                    hidden.push(name);
                    continue;
                }
                if !hidden.is_empty() {
                    continue;
                }
                match name.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        newline(job, color);
                        style.heading = name.as_bytes()[1] - b'0';
                    }
                    "b" | "strong" => style.bold += 1,
                    "i" | "em" => style.italic += 1,
                    "s" | "del" => style.strike += 1,
                    "pre" | "code" => style.code += 1,
                    "a" => style.link += 1,
                    "br" => append(job, "\n", style, color),
                    "p" | "div" | "tr" | "blockquote" => newline(job, color),
                    "li" => {
                        newline(job, color);
                        append(job, "• ", style, color);
                    }
                    "img" => append(job, "[Image]", style, color),
                    _ => {}
                }
            }
            Ok(Token::EndTag(tag)) => {
                let name = String::from_utf8_lossy(&tag.name).to_ascii_lowercase();
                if !hidden.is_empty() {
                    if hidden.last() == Some(&name) {
                        hidden.pop();
                    }
                    continue;
                }
                match name.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        newline(job, color);
                        style.heading = 0;
                    }
                    "b" | "strong" => style.bold = style.bold.saturating_sub(1),
                    "i" | "em" => style.italic = style.italic.saturating_sub(1),
                    "s" | "del" => style.strike = style.strike.saturating_sub(1),
                    "pre" | "code" => style.code = style.code.saturating_sub(1),
                    "a" => style.link = style.link.saturating_sub(1),
                    "p" | "div" | "tr" | "li" | "blockquote" => newline(job, color),
                    "td" | "th" => append(job, "  │  ", style, color),
                    _ => {}
                }
            }
            Ok(Token::String(text)) if hidden.is_empty() => {
                let text = String::from_utf8_lossy(&text.value);
                if style.code > 0 {
                    append(job, &text, style, color);
                } else {
                    let mut collapsed = String::new();
                    let mut space = job.text.ends_with(char::is_whitespace);
                    for ch in text.chars() {
                        if ch.is_whitespace() {
                            if !space {
                                collapsed.push(' ');
                            }
                            space = true;
                        } else {
                            collapsed.push(ch);
                            space = false;
                        }
                    }
                    append(job, &collapsed, style, color);
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_renders_structure_and_inline_styles() {
        let job = layout(
            "# Title\n\nA **bold** and *italic* with `code`.\n\n3. three\n4. four",
            DocumentKind::Markdown,
            Color32::WHITE,
            300.0,
        );
        assert!(job.text.contains("Title\nA bold and italic with code."));
        assert!(job.text.contains("3. three\n4. four"));
        assert!(job.sections.iter().any(|s| s.format.font_id.size == 26.0));
        assert!(job.sections.iter().any(|s| s.format.italics));
    }
    #[test]
    fn html_is_static_decodes_entities_and_hides_executable_content() {
        let job = layout(
            "<head><title>hidden</title></head><h1>Hello &amp; world</h1><script>alert('bad')</script><p>Visible<img src='https://example.com/tracker'></p>",
            DocumentKind::Html,
            Color32::WHITE,
            400.0,
        );
        assert!(job.text.contains("Hello & world"));
        assert!(job.text.contains("Visible[Image]"));
        assert!(!job.text.contains("hidden"));
        assert!(!job.text.contains("alert"));
        assert!(!job.text.contains("tracker"));
    }
    #[test]
    fn html_keeps_word_boundaries_across_inline_tags() {
        let job = layout(
            "<p>Hello <b>bold</b> and <i>italic</i> world</p><pre> a\n  b</pre>",
            DocumentKind::Html,
            Color32::WHITE,
            400.0,
        );
        assert_eq!(job.text, "Hello bold and italic world\n a\n  b");
    }
    #[test]
    fn previews_bound_work_and_accept_unicode_at_input_limit() {
        let input = "я".repeat(INPUT_LIMIT);
        let job = layout(&input, DocumentKind::Markdown, Color32::WHITE, 200.0);
        assert!(job.text.contains("preview limit"));
        let input = "**a** ".repeat(EVENT_LIMIT);
        assert!(
            layout(&input, DocumentKind::Markdown, Color32::WHITE, 200.0)
                .text
                .contains("preview limit")
        );
    }
}
