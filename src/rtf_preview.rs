//! Bounded static RTF text. No document engine, attachments, URLs or commands.
use crate::image_cache::PreviewError;
use encoding_rs::{Encoding, WINDOWS_1252};
use std::{collections::HashMap, path::Path};

pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_TOKENS: usize = 200_000;

pub fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("rtf"))
}
fn invalid() -> PreviewError {
    PreviewError::InvalidDocument
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Destination {
    Body,
    Skip,
    Fonts,
}
#[derive(Clone)]
struct State {
    dest: Destination,
    hidden: bool,
    deleted: bool,
    font: i32,
    codepage: Option<&'static Encoding>,
    uc: usize,
    start: bool,
    starred: bool,
    upr_children: Option<usize>,
    unicode_branch: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            dest: Destination::Body,
            hidden: false,
            deleted: false,
            font: 0,
            codepage: Some(WINDOWS_1252),
            uc: 1,
            start: true,
            starred: false,
            upr_children: None,
            unicode_branch: false,
        }
    }
}
fn codepage(value: i32) -> Option<&'static Encoding> {
    let label = match value {
        874 => "windows-874",
        932 => "shift_jis",
        936 => "gbk",
        949 => "euc-kr",
        950 => "big5",
        1250 => "windows-1250",
        1251 => "windows-1251",
        1252 => "windows-1252",
        1253 => "windows-1253",
        1254 => "windows-1254",
        1255 => "windows-1255",
        1256 => "windows-1256",
        1257 => "windows-1257",
        1258 => "windows-1258",
        10000 => "macintosh",
        65001 => "utf-8",
        _ => return None,
    };
    Encoding::for_label(label.as_bytes())
}
fn charset(value: i32) -> Option<&'static Encoding> {
    match value {
        128 => codepage(932),
        129 => codepage(949),
        134 => codepage(936),
        136 => codepage(950),
        161 => codepage(1253),
        162 => codepage(1254),
        163 => codepage(1258),
        177 => codepage(1255),
        178 => codepage(1256),
        186 => codepage(1257),
        204 => codepage(1251),
        222 => codepage(874),
        238 => codepage(1250),
        77 => codepage(10000),
        _ => None,
    }
}
struct Text {
    out: String,
    high: Option<u16>,
    bytes: Vec<u8>,
}
#[derive(Default)]
struct FontEncoding {
    charset: Option<Option<&'static Encoding>>,
    codepage: Option<Option<&'static Encoding>>,
}
impl Text {
    fn append(&mut self, text: &str) -> Result<(), PreviewError> {
        if self.high.is_some() {
            return Err(invalid());
        }
        if self.out.len().saturating_add(text.len()) > MAX_TEXT_BYTES {
            return Err(PreviewError::TooLarge);
        }
        self.out.push_str(text);
        Ok(())
    }
    fn unit(&mut self, value: u16) -> Result<(), PreviewError> {
        if (0xd800..=0xdbff).contains(&value) {
            if self.high.replace(value).is_some() {
                return Err(invalid());
            }
            return Ok(());
        }
        let scalar = if let Some(high) = self.high.take() {
            if !(0xdc00..=0xdfff).contains(&value) {
                return Err(invalid());
            }
            0x10000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(value) - 0xdc00)
        } else {
            u32::from(value)
        };
        let c = char::from_u32(scalar).ok_or_else(invalid)?;
        self.append(c.encode_utf8(&mut [0; 4]))
    }
    fn flush(
        &mut self,
        state: &State,
        fonts: &HashMap<i32, FontEncoding>,
    ) -> Result<(), PreviewError> {
        if self.bytes.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.bytes);
        let encoding = fonts
            .get(&state.font)
            .and_then(|font| font.codepage.or(font.charset))
            .unwrap_or(state.codepage)
            .ok_or_else(invalid)?;
        let text = encoding
            .decode_without_bom_handling_and_without_replacement(&bytes)
            .ok_or_else(invalid)?;
        self.append(&text)
    }
}
fn visible(state: &State) -> bool {
    state.dest == Destination::Body && !state.hidden && !state.deleted
}
fn skipped_destination(word: &str) -> bool {
    matches!(
        word,
        "colortbl"
            | "stylesheet"
            | "info"
            | "filetbl"
            | "revtbl"
            | "rsidtbl"
            | "listtable"
            | "listoverridetable"
            | "generator"
            | "pict"
            | "object"
            | "objdata"
            | "nonshppict"
            | "shppict"
            | "shp"
            | "shpinst"
            | "header"
            | "headerl"
            | "headerr"
            | "headerf"
            | "footer"
            | "footerl"
            | "footerr"
            | "footerf"
            | "footnote"
            | "annotation"
            | "atnauthor"
            | "atnid"
            | "fldinst"
            | "datafield"
            | "bkmkstart"
            | "bkmkend"
            | "xe"
            | "tc"
            | "xmlnstbl"
            | "themedata"
            | "colorschememapping"
            | "datastore"
            | "latentstyles"
            | "docvar"
            | "htmltag"
            | "mhtmltag"
    )
}

pub fn extract(input: &[u8], cancelled: &dyn Fn() -> bool) -> Result<String, PreviewError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(PreviewError::TooLarge);
    }
    if !input.starts_with(b"{\\rtf1") || input.get(6).is_none_or(u8::is_ascii_digit) {
        return Err(invalid());
    }
    let mut state = State::default();
    let mut stack = Vec::new();
    let mut fonts: HashMap<i32, FontEncoding> = HashMap::new();
    let mut default_font = 0;
    let mut text = Text {
        out: String::new(),
        high: None,
        bytes: Vec::new(),
    };
    let mut fallback = 0_usize;
    let mut i = 0;
    let mut tokens = 0;
    let mut closed = false;
    while i < input.len() {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        tokens += 1;
        if tokens > MAX_TOKENS {
            return Err(PreviewError::TooLarge);
        }
        let byte = input[i];
        i += 1;
        if closed {
            if !byte.is_ascii_whitespace() {
                return Err(invalid());
            }
            continue;
        }
        match byte {
            b'{' => {
                text.flush(&state, &fonts)?;
                fallback = 0;
                if stack.len() >= 128 {
                    return Err(PreviewError::TooLarge);
                }
                let branch = state.upr_children.as_mut().map(|count| {
                    *count += 1;
                    *count
                });
                stack.push(state.clone());
                state.start = true;
                state.starred = false;
                state.upr_children = None;
                state.unicode_branch = branch == Some(2);
                if branch.is_some_and(|n| n != 2) {
                    state.dest = Destination::Skip;
                }
            }
            b'}' => {
                text.flush(&state, &fonts)?;
                fallback = 0;
                state = stack.pop().ok_or_else(invalid)?;
                closed = stack.is_empty();
            }
            b'\r' | b'\n' => {}
            b'\\' => {
                let symbol = *input.get(i).ok_or_else(invalid)?;
                i += 1;
                if symbol == b'\'' {
                    let digits = input.get(i..i + 2).ok_or_else(invalid)?;
                    i += 2;
                    let hex = |b: u8| {
                        (b as char)
                            .to_digit(16)
                            .map(|n| n as u8)
                            .ok_or_else(invalid)
                    };
                    let value = hex(digits[0])? * 16 + hex(digits[1])?;
                    if fallback > 0 {
                        fallback -= 1;
                    } else if visible(&state) {
                        text.bytes.push(value);
                    }
                    state.start = false;
                    continue;
                }
                if !symbol.is_ascii_alphabetic() {
                    if symbol == b'*' && state.start {
                        state.starred = true;
                        continue;
                    }
                    if fallback > 0 {
                        fallback -= 1;
                        continue;
                    }
                    text.flush(&state, &fonts)?;
                    if visible(&state) {
                        match symbol {
                            b'\\' => text.append("\\")?,
                            b'{' => text.append("{")?,
                            b'}' => text.append("}")?,
                            b'~' => text.append("\u{a0}")?,
                            b'_' => text.append("\u{2011}")?,
                            b'-' => text.append("\u{ad}")?,
                            b'\r' | b'\n' => {
                                if symbol == b'\r' && input.get(i) == Some(&b'\n') {
                                    i += 1;
                                }
                                text.append("\n")?;
                            }
                            _ => {}
                        }
                    }
                    state.start = false;
                    continue;
                }
                let start = i - 1;
                while input.get(i).is_some_and(u8::is_ascii_alphabetic) {
                    i += 1;
                }
                if i - start > 32 {
                    return Err(invalid());
                }
                let word = std::str::from_utf8(&input[start..i]).map_err(|_| invalid())?;
                let number_start = i;
                if input.get(i) == Some(&b'-') {
                    i += 1;
                }
                let digits = i;
                while input.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                let number = if i > digits {
                    Some(
                        std::str::from_utf8(&input[number_start..i])
                            .map_err(|_| invalid())?
                            .parse::<i32>()
                            .map_err(|_| invalid())?,
                    )
                } else {
                    i = number_start;
                    None
                };
                if input.get(i) == Some(&b' ') {
                    i += 1;
                }
                if word == "bin" {
                    if state.start && state.starred {
                        state.dest = Destination::Skip;
                    }
                    let size =
                        usize::try_from(number.ok_or_else(invalid)?).map_err(|_| invalid())?;
                    i = i
                        .checked_add(size)
                        .filter(|end| *end <= input.len())
                        .ok_or_else(invalid)?;
                    fallback = fallback.saturating_sub(1);
                    state.start = false;
                    continue;
                }
                if fallback > 0 {
                    fallback -= 1;
                    continue;
                }
                text.flush(&state, &fonts)?;
                if state.start {
                    if state.starred && !(word == "ud" && state.unicode_branch) {
                        state.dest = Destination::Skip;
                    }
                    if state.dest != Destination::Skip {
                        if word == "fonttbl" {
                            state.dest = Destination::Fonts;
                        } else if skipped_destination(word) {
                            state.dest = Destination::Skip;
                        }
                    }
                }
                state.start = false;
                if state.dest == Destination::Skip {
                    continue;
                }
                if state.dest == Destination::Fonts {
                    match word {
                        "f" => state.font = number.ok_or_else(invalid)?,
                        "fcharset" => {
                            let n = number.ok_or_else(invalid)?;
                            fonts.entry(state.font).or_default().charset = if n == 0 || n == 1 {
                                None
                            } else {
                                Some(charset(n))
                            };
                        }
                        "cpg" => {
                            fonts.entry(state.font).or_default().codepage =
                                Some(codepage(number.ok_or_else(invalid)?));
                        }
                        _ => {}
                    }
                    continue;
                }
                match word {
                    "rtf" if number != Some(1) => return Err(invalid()),
                    "ansi" => state.codepage = Some(WINDOWS_1252),
                    "mac" => state.codepage = codepage(10000),
                    "pc" | "pca" => state.codepage = None,
                    "ansicpg" => state.codepage = codepage(number.ok_or_else(invalid)?),
                    "deff" => {
                        default_font = number.ok_or_else(invalid)?;
                        state.font = default_font;
                    }
                    "f" => state.font = number.ok_or_else(invalid)?,
                    "plain" => {
                        state.font = default_font;
                        state.hidden = false;
                        state.deleted = false;
                    }
                    "v" => state.hidden = number != Some(0),
                    "deleted" => state.deleted = number != Some(0),
                    "uc" => {
                        state.uc =
                            usize::try_from(number.ok_or_else(invalid)?).map_err(|_| invalid())?;
                        if state.uc > 256 {
                            return Err(PreviewError::TooLarge);
                        }
                    }
                    "u" => {
                        let value = number
                            .filter(|n| (-32768..=65535).contains(n))
                            .ok_or_else(invalid)?;
                        if visible(&state) {
                            text.unit(value as u16)?;
                        }
                        fallback = state.uc;
                    }
                    "upr" => state.upr_children = Some(0),
                    "fromhtml" => return Err(invalid()),
                    word if visible(&state) => match word {
                        "par" | "line" | "page" | "sect" | "row" | "nestrow" => {
                            text.append("\n")?
                        }
                        "tab" | "cell" | "nestcell" => text.append("\t")?,
                        "emdash" => text.append("—")?,
                        "endash" => text.append("–")?,
                        "bullet" => text.append("•")?,
                        "lquote" => text.append("‘")?,
                        "rquote" => text.append("’")?,
                        "ldblquote" => text.append("“")?,
                        "rdblquote" => text.append("”")?,
                        "emspace" => text.append("\u{2003}")?,
                        "enspace" => text.append("\u{2002}")?,
                        "qmspace" => text.append("\u{2005}")?,
                        _ => {}
                    },
                    _ => {}
                }
            }
            byte => {
                if stack.is_empty() || (state.start && state.starred) {
                    return Err(invalid());
                }
                state.start = false;
                if fallback > 0 {
                    fallback -= 1;
                } else if visible(&state) {
                    if byte < 0x20 && byte != b'\t' {
                        return Err(invalid());
                    }
                    text.bytes.push(byte);
                }
            }
        }
    }
    if !closed || !stack.is_empty() || text.high.is_some() {
        return Err(invalid());
    }
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    Ok(text.out)
}

#[cfg(any(test, feature = "visual-qa"))]
pub const FIXTURE_TEXT: &str = "RTF preview\nmessage: café / ёж 🌍\nA\tB\t\n";
#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture() -> Vec<u8> {
    br"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fcharset0 Helvetica;}{\f1\fcharset204 Cyrillic;}}{\info{\author Hidden author}}RTF preview\par message: caf\'e9 / {\f1\'b8\'e6} \u-10180?\u-8435?\par A\cell B\cell\row{\*\unknown secret}{\pict 00ff}}".to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_text_font_codepages_unicode_and_table_cells_are_preserved() {
        assert_eq!(extract(&fixture(), &|| false).unwrap(), FIXTURE_TEXT);
        assert!(supported(Path::new("book.RTF")));
        assert!(!supported(Path::new("book.rtf.zip")));
    }
    #[test]
    fn group_scoping_fallback_tokens_and_unicode_alternatives_do_not_duplicate_text() {
        for (input, expected) in [
            (r"{\rtf1\ansi A{\uc2\u233??}B\u937?C}", "AéBΩC"),
            (r"{\rtf1\uc1\u233\'e9\u937\~\u1046\tab X}", "éΩЖX"),
            (r"{\rtf1\uc0\u-10180\u-8435}", "🌍"),
            (
                r"{\rtf1{\upr{ANSI fallback}{\*\ud Unicode \u233?}}}",
                "Unicode é",
            ),
            (r"{\rtf1\u233\bin3 {}\X}", "éX"),
            (r"{\rtf1\uc2\u233?{next}end}", "énextend"),
        ] {
            assert_eq!(
                extract(input.as_bytes(), &|| false).unwrap(),
                expected,
                "{input}"
            );
        }
    }
    #[test]
    fn symbols_paragraphs_and_format_groups_preserve_only_visible_text() {
        assert_eq!(extract(br"{\rtf1 A{\b bold} \{x\} \\ \~\_\-\tab\emdash\endash\bullet\lquote\rquote\ldblquote\rdblquote\line next\par}", &|| false).unwrap(), "Abold {x} \\ \u{a0}\u{2011}\u{ad}\t—–•‘’“”\nnext\n");
        assert_eq!(
            extract(b"{\\rtf1 a\r\nb\\\r\nc}", &|| false).unwrap(),
            "ab\nc"
        );
    }
    #[test]
    fn destinations_binary_hidden_text_and_field_instructions_are_inert() {
        assert_eq!(
            extract(br"{\rtf1{\*\bin3 abcsecret}end}", &|| false).unwrap(),
            "end"
        );
        assert_eq!(
            extract(br"{\rtf1{\v hidden\deleted0 still hidden}end}", &|| false).unwrap(),
            "end"
        );
        let bytes = br#"{\rtf1{\fonttbl{\f0 Arial;}}{\stylesheet leak}{\info leak}{\header leak}{\footer leak}{\footnote leak}{\annotation leak}{\*\future leak}{\pict\bin5 {}\{} }{\object{\objdata leak}}visible{\v hidden}{\deleted deleted}{\field{\*\fldinst HYPERLINK "https://example.com"}{\fldrslt label}}}"#;
        assert_eq!(extract(bytes, &|| false).unwrap(), "visiblelabel");
        assert_eq!(
            extract(
                br"{\rtf1{\*\unknown{\upr{a}{\*\ud should stay hidden}}}end}",
                &|| false
            )
            .unwrap(),
            "end"
        );
    }
    #[test]
    fn multibyte_font_encodings_and_document_charset_switches_are_strict() {
        for (input, expected) in [
            (r"{\rtf1\ansicpg1251\'b8\'e6}", "ёж"),
            (r"{\rtf1\mac\'8e}", "é"),
            (
                r"{\rtf1{\fonttbl{\f0\fcharset0 Arial;}{\f1\fcharset128 MS;}}\f1\'82\'a0\f0\'e9}",
                "あé",
            ),
            (r"{\rtf1{\fonttbl{\f0\cpg1251 Arial;}}\'b8\'e6}", "ёж"),
            (
                r"{\rtf1{\fonttbl{\f0\cpg1251\fcharset128 Arial;}}\'b8\'e6}",
                "ёж",
            ),
            (
                r"{\rtf1{\fonttbl{\f0\fcharset128\cpg1251 Arial;}}\'b8\'e6}",
                "ёж",
            ),
            (r"{\rtf1{\fonttbl{\f1\fcharset2 Symbol;}}text}", "text"),
        ] {
            assert_eq!(
                extract(input.as_bytes(), &|| false).unwrap(),
                expected,
                "{input}"
            );
        }
        for input in [
            r"{\rtf1\ansicpg1 text}",
            r"{\rtf1{\fonttbl{\f1\fcharset2 Symbol;}}\f1 text}",
            r"{\rtf1\ansicpg932\'82}",
        ] {
            assert_eq!(
                extract(input.as_bytes(), &|| false),
                Err(invalid()),
                "{input}"
            );
        }
    }
    #[test]
    fn malformed_or_truncated_documents_do_not_return_partial_text() {
        for input in [
            "plain",
            "{\\rtf10 text}",
            "{\\rtf1 text",
            "{\\rtf1 text}}",
            "{\\rtf1}tail",
            r"{\rtf1\'xx}",
            r"{\rtf1\bin-1 abc}",
            r"{\rtf1\bin30 abc}",
            r"{\rtf1\uc-1 x}",
            r"{\rtf1\u-10180?}",
            r"{\rtf1\u-8435?}",
            r"{\rtf1\u-10180?x}",
            r"{\rtf1\u999999?}",
            r"{\rtf1\fromhtml1 html}",
            r"{\rtf1\fs99999999999999 text}",
        ] {
            assert_eq!(
                extract(input.as_bytes(), &|| false),
                Err(invalid()),
                "{input}"
            );
        }
        let bytes = fixture();
        for end in 0..bytes.len() {
            assert!(extract(&bytes[..end], &|| false).is_err(), "prefix {end}");
        }
    }
    #[test]
    fn bounded_output_depth_input_tokens_and_cancellation_are_enforced() {
        assert_eq!(
            extract(&vec![b'x'; MAX_INPUT_BYTES + 1], &|| false),
            Err(PreviewError::TooLarge)
        );
        assert_eq!(
            extract(
                format!("{{\\rtf1 {}x{}}}", "{".repeat(128), "}".repeat(128)).as_bytes(),
                &|| false
            ),
            Err(PreviewError::TooLarge)
        );
        assert_eq!(
            extract(
                format!("{{\\rtf1 {}}}", r"\u30000?".repeat(90_000)).as_bytes(),
                &|| false
            ),
            Err(PreviewError::TooLarge)
        );
        assert_eq!(
            extract(
                format!("{{\\rtf1 {}}}", r"\b ".repeat(MAX_TOKENS)).as_bytes(),
                &|| false
            ),
            Err(PreviewError::TooLarge)
        );
        let calls = std::cell::Cell::new(0);
        assert_eq!(
            extract(&fixture(), &|| {
                calls.set(calls.get() + 1);
                calls.get() > 15
            }),
            Err(PreviewError::Cancelled)
        );
    }
}
