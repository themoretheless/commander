//! Bounded JSON whitespace formatting, preserving every non-whitespace token.
use serde::Deserialize;
use std::{path::Path, sync::Arc};

pub const MAX_INPUT: usize = 256 * 1024;
const MAX_OUTPUT: usize = 1024 * 1024;

pub fn is_json(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn pretty(input: &str) -> Result<String, String> {
    if input.len() > MAX_INPUT {
        return Err("JSON formatting is limited to 256 KiB of loaded text".into());
    }
    let mut deserializer = serde_json::Deserializer::from_str(input);
    serde::de::IgnoredAny::deserialize(&mut deserializer)
        .map_err(|error| format!("Could not format loaded JSON: {error}"))?;
    deserializer
        .end()
        .map_err(|error| format!("Could not format loaded JSON: {error}"))?;

    let mut compact = Vec::with_capacity(input.len());
    let (mut quoted, mut escaped) = (false, false);
    for byte in input.bytes() {
        if quoted {
            compact.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
            compact.push(byte);
        } else if !byte.is_ascii_whitespace() {
            compact.push(byte);
        }
    }
    let mut output = Vec::new();
    let mut depth = 0usize;
    quoted = false;
    escaped = false;
    let newline = |output: &mut Vec<u8>, depth: usize| {
        output.push(b'\n');
        output.resize(output.len() + depth * 2, b' ');
    };
    for (index, &byte) in compact.iter().enumerate() {
        if quoted {
            output.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => {
                    quoted = true;
                    output.push(byte);
                }
                b'{' | b'[' => {
                    output.push(byte);
                    depth += 1;
                    if depth > 128 {
                        return Err("JSON formatting is limited to 128 nested containers".into());
                    }
                    if compact.get(index + 1) != Some(&(if byte == b'{' { b'}' } else { b']' })) {
                        newline(&mut output, depth);
                    }
                }
                b'}' | b']' => {
                    depth -= 1;
                    if index > 0 && !matches!(compact[index - 1], b'{' | b'[') {
                        newline(&mut output, depth);
                    }
                    output.push(byte);
                }
                b',' => {
                    output.push(byte);
                    newline(&mut output, depth);
                }
                b':' => {
                    output.extend_from_slice(b": ");
                }
                _ => output.push(byte),
            }
        }
        if output.len() > MAX_OUTPUT {
            return Err("Formatted JSON exceeds the 1 MiB preview limit".into());
        }
    }
    String::from_utf8(output).map_err(|error| error.to_string())
}

#[derive(Clone)]
struct Cached {
    source: Arc<str>,
    text: Arc<str>,
    error: Option<String>,
}

pub fn formatted(ui: &mut egui::Ui, source: &Arc<str>, side: &str) -> Arc<str> {
    let id = egui::Id::new(("json_preview", side));
    let cached = ui.ctx().data(|data| data.get_temp::<Cached>(id));
    let cached = match cached {
        Some(cached) if Arc::ptr_eq(&cached.source, source) => cached,
        _ => {
            let (text, error) = match pretty(source) {
                Ok(text) => (Arc::from(text), None),
                Err(error) => (source.clone(), Some(error)),
            };
            let cached = Cached {
                source: source.clone(),
                text,
                error,
            };
            ui.ctx()
                .data_mut(|data| data.insert_temp(id, cached.clone()));
            cached
        }
    };
    if let Some(error) = &cached.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
        ui.small("Showing the loaded source text instead.");
    }
    cached.text
}

#[cfg(feature = "visual-qa")]
pub fn shown_format(ctx: &egui::Context, side: &str) -> Option<Arc<str>> {
    ctx.data(|data| data.get_temp::<Cached>(egui::Id::new(("json_preview", side))))
        .filter(|cached| cached.error.is_none())
        .map(|cached| cached.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_whitespace_formatting_keeps_order_duplicates_numbers_and_strings() {
        let input = r#" { "z":9007199254740993123456789,"a":-0.00e+007,"z":1e9999,"s":"ёж \" , : [] {} \\ \n \u0061","empty": [ ],"object":{} } "#;
        let output = pretty(input).unwrap();
        assert!(output.starts_with(
            "{\n  \"z\": 9007199254740993123456789,\n  \"a\": -0.00e+007,\n  \"z\": 1e9999,"
        ));
        assert!(output.contains(r#""s": "ёж \" , : [] {} \\ \n \u0061""#));
        assert!(output.contains("\"empty\": []"));
        assert!(output.ends_with("\"object\": {}\n}"));
        for scalar in ["true", "false", "null", "123", r#""hello""#] {
            assert_eq!(pretty(scalar).unwrap(), scalar);
        }
        assert!(is_json(Path::new("DATA.JSON")));
        assert!(!is_json(Path::new("data.jsonl")));
    }
    #[test]
    fn rejects_invalid_incomplete_and_bounded_expansion() {
        for input in [
            "",
            "[",
            "[1,]",
            "{}{}",
            "NaN",
            "{\"a\":01}",
            "{\"a\":1e+}",
            r#""\x""#,
            "{]",
            "//comment\n{}",
        ] {
            assert!(pretty(input).is_err(), "{input}");
        }
        assert!(
            pretty(&" ".repeat(MAX_INPUT + 1))
                .unwrap_err()
                .contains("256 KiB")
        );
        assert!(pretty(&("[".repeat(128) + "0" + &"]".repeat(128))).is_ok());
        assert!(
            pretty(&("[".repeat(129) + "0" + &"]".repeat(129)))
                .unwrap_err()
                .contains("128")
        );
        let expansion = "[".repeat(100) + &"0,".repeat(10_000) + "0" + &"]".repeat(100);
        assert!(pretty(&expansion).unwrap_err().contains("1 MiB"));
    }
    #[test]
    fn cache_reuses_layout_input_and_errors_keep_original_source() {
        let ctx = egui::Context::default();
        let input: Arc<str> = r#"{"a":1}"#.into();
        let mut views = Vec::new();
        for source in [input.clone(), input.clone(), Arc::from("{bad")] {
            crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
                views.push(formatted(ui, &source, "right"));
            }));
        }
        assert!(Arc::ptr_eq(&views[0], &views[1]));
        assert_eq!(views[0].as_ref(), "{\n  \"a\": 1\n}");
        assert_eq!(views[2].as_ref(), "{bad");
    }
}
