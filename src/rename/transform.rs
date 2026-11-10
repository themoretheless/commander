//! Per-name transformation: case, replacement, counters and validity checks.

use super::*;

/// Split a file name into (stem, extension-with-dot). A leading dot (dotfiles
/// like `.gitignore`) is part of the stem, not an extension; the split uses
/// the last interior dot, so `archive.tar.gz` -> (`archive.tar`, `.gz`).
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

pub(super) fn apply_case(stem: &str, mode: CaseMode) -> String {
    match mode {
        CaseMode::Keep => stem.to_string(),
        CaseMode::Lower => stem.to_lowercase(),
        CaseMode::Upper => stem.to_uppercase(),
        CaseMode::Capitalize => capitalize(stem),
        CaseMode::TitleWords => {
            use unicode_segmentation::UnicodeSegmentation;
            stem.split_word_bounds().map(capitalize).collect()
        }
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

pub(super) const MAX_REPLACEMENT_OUTPUT_BYTES: usize = 64 * 1024;

fn bounded_replace(
    part: &str,
    find: &str,
    replacement: &str,
    matcher: Option<&regex::Regex>,
    rule: &RenameRule,
) -> Result<String, ()> {
    let mut output = String::new();
    let append = |output: &mut String, value: &str| -> Result<(), ()> {
        if value.len() > MAX_REPLACEMENT_OUTPUT_BYTES.saturating_sub(output.len()) {
            return Err(());
        }
        output.push_str(value);
        Ok(())
    };
    if find.is_empty() {
        append(&mut output, part)?;
        return Ok(output);
    }
    let mut last = 0;
    if rule.regex || rule.ignore_case {
        let Some(regex) = matcher else {
            append(&mut output, part)?;
            return Ok(output);
        };
        for captures in regex.captures_iter(part) {
            let found = captures.get(0).ok_or(())?;
            append(&mut output, &part[last..found.start()])?;
            if rule.regex && !rule.literal_replacement {
                // Reuse the regex engine's exact $name/${name}/$$ grammar.
                // Literal replacement text is bounded at compile time; capture
                // appends are checked before allocation, avoiding expanding bombs.
                if replacement.len() > MAX_REPLACEMENT_OUTPUT_BYTES {
                    return Err(());
                }
                let mut expanded = String::new();
                let mut overflow = false;
                regex_automata::util::interpolate::string(
                    replacement,
                    |index, dst| {
                        if let Some(capture) = captures.get(index) {
                            if capture.len()
                                > MAX_REPLACEMENT_OUTPUT_BYTES.saturating_sub(dst.len())
                            {
                                overflow = true;
                            } else {
                                dst.push_str(capture.as_str());
                            }
                        }
                    },
                    |name| {
                        regex
                            .capture_names()
                            .position(|candidate| candidate == Some(name))
                    },
                    &mut expanded,
                );
                if overflow {
                    return Err(());
                }
                append(&mut output, &expanded)?;
            } else {
                append(&mut output, replacement)?;
            }
            last = found.end();
            if rule.first_only {
                break;
            }
        }
    } else {
        for (start, _) in part.match_indices(find) {
            append(&mut output, &part[last..start])?;
            append(&mut output, replacement)?;
            last = start + find.len();
            if rule.first_only {
                break;
            }
        }
    }
    append(&mut output, &part[last..])?;
    Ok(output)
}

#[cfg(test)]
pub(super) fn rename_one(
    name: &str,
    is_dir: bool,
    index: usize,
    rule: &RenameRule,
    compiled: Option<&[Option<regex::Regex>]>,
) -> String {
    rename_one_checked(name, is_dir, index, rule, compiled).unwrap_or_default()
}

/// Compute the proposed new name for a single entry at position `index`.
/// `compiled` is the pre-compiled pattern for regex mode (`None` when not in
/// regex/ignore-case mode, or when compilation failed — see [`regex_error`]).
pub(super) fn rename_one_checked(
    name: &str,
    is_dir: bool,
    index: usize,
    rule: &RenameRule,
    compiled: Option<&[Option<regex::Regex>]>,
) -> Result<String, ()> {
    let name = name.strip_prefix(&rule.remove_prefix).unwrap_or(name);
    let name = name.strip_suffix(&rule.remove_suffix).unwrap_or(name);
    let replace_part =
        |part: &str, find: &str, replacement: &str, matcher: Option<&regex::Regex>| {
            if find.is_empty() {
                part.to_string()
            } else if rule.regex || rule.ignore_case {
                matcher.map_or_else(
                    || part.to_string(),
                    |regex| {
                        if rule.regex && !rule.literal_replacement {
                            if rule.first_only {
                                regex.replace(part, replacement).into_owned()
                            } else {
                                regex.replace_all(part, replacement).into_owned()
                            }
                        } else {
                            // Literal replacement never expands $1/${name}, even when
                            // the escaped matcher uses Unicode case-insensitive search.
                            if rule.first_only {
                                regex
                                    .replace(part, regex::NoExpand(replacement))
                                    .into_owned()
                            } else {
                                regex
                                    .replace_all(part, regex::NoExpand(replacement))
                                    .into_owned()
                            }
                        }
                    },
                )
            } else if rule.first_only {
                part.replacen(find, replacement, 1)
            } else {
                part.replace(find, replacement)
            }
        };
    let replace = |part: &str| -> Result<String, ()> {
        if !rule.extra_replacements.is_empty() {
            let mut result = bounded_replace(
                part,
                &rule.find,
                &rule.replace,
                compiled
                    .and_then(|patterns| patterns.first())
                    .and_then(Option::as_ref),
                rule,
            )?;
            for (index, pair) in rule
                .extra_replacements
                .iter()
                .take(MAX_EXTRA_REPLACEMENTS)
                .enumerate()
            {
                result = bounded_replace(
                    &result,
                    &pair.find,
                    &pair.replace,
                    compiled
                        .and_then(|patterns| patterns.get(index + 1))
                        .and_then(Option::as_ref),
                    rule,
                )?;
            }
            return Ok(result);
        }
        let result = replace_part(
            part,
            &rule.find,
            &rule.replace,
            compiled
                .and_then(|patterns| patterns.first())
                .and_then(Option::as_ref),
        );
        Ok(result)
    };
    let original_parts = || if is_dir { (name, "") } else { split_ext(name) };
    let (stem, ext) = match rule.replace_target {
        ReplaceTarget::WholeName => {
            let replaced = replace(name)?;
            let (stem, ext) = if is_dir {
                (replaced.as_str(), "")
            } else {
                split_ext(&replaced)
            };
            (stem.to_string(), ext.to_string())
        }
        ReplaceTarget::Stem => {
            let (stem, ext) = original_parts();
            (replace(stem)?, ext.to_string())
        }
        ReplaceTarget::Extension => {
            let (stem, ext) = original_parts();
            if is_dir {
                (stem.to_string(), ext.to_string())
            } else {
                let raw = ext.strip_prefix('.').unwrap_or(ext);
                let replaced = replace(raw)?;
                let extension = if replaced == raw {
                    ext.to_string()
                } else if replaced.is_empty() {
                    String::new()
                } else {
                    format!(".{replaced}")
                };
                (stem.to_string(), extension)
            }
        }
    };
    let edit = |value: String| {
        let value = if let Some(range) = rule.remove_range {
            range.apply(&value, rule.range_action)
        } else {
            value
        };
        if let Some(insert) = &rule.insert_text {
            insert.apply(&value)
        } else {
            value
        }
    };
    let (stem, ext) = if rule.remove_range.is_none() && rule.insert_text.is_none() {
        (stem, ext)
    } else {
        match rule.character_target {
            CharacterTarget::Stem => (edit(stem), ext),
            CharacterTarget::Extension if is_dir => (stem, ext),
            CharacterTarget::Extension => {
                let raw = ext.strip_prefix('.').unwrap_or(&ext);
                let edited = edit(raw.to_owned());
                let ext = if edited == raw {
                    ext
                } else if edited.is_empty() {
                    String::new()
                } else {
                    format!(".{edited}")
                };
                (stem, ext)
            }
            CharacterTarget::WholeName => {
                let edited = edit(format!("{stem}{ext}"));
                let (stem, ext) = if is_dir {
                    (edited.as_str(), "")
                } else {
                    split_ext(&edited)
                };
                (stem.to_owned(), ext.to_owned())
            }
        }
    };
    let ext = match rule.extension_case {
        ExtensionCase::Keep => ext,
        ExtensionCase::Lower => ext.to_lowercase(),
        ExtensionCase::Upper => ext.to_uppercase(),
    };
    let mut stem = apply_case(&stem, rule.case);
    stem = format!("{}{}{}", rule.prefix, stem, rule.suffix);
    if let Some(n) = rule.numbering {
        // Widen before arithmetic: all i64 values and usize indices fit this sum.
        let value = i128::from(n.start) + index as i128 * i128::from(n.step);
        let counter = rule.number_format.render(value, n.pad);
        stem = match rule.number_position {
            NumberPosition::CounterOnly => counter,
            NumberPosition::BeforeName => format!("{counter}{stem}"),
            NumberPosition::AfterName => format!("{stem}{counter}"),
        };
    }
    let name = format!("{stem}{ext}");
    let name = if rule.collapse_repeats {
        collapse_repeats(&name)
    } else {
        name
    };
    let name = if rule.trim_spaces {
        name.trim().to_owned()
    } else {
        name
    };
    let name = rule.normalization.apply(name);
    let name = if let Some(limit) = rule.stem_limit {
        use unicode_segmentation::UnicodeSegmentation;
        let (stem, ext) = if is_dir {
            (name.as_str(), "")
        } else {
            split_ext(&name)
        };
        let shortened: String = stem.graphemes(true).take(limit).collect();
        format!("{shortened}{ext}")
    } else {
        name
    };
    if let Some(replacement) = &rule.invalid_char_replacement {
        let count = name.chars().filter(|&ch| matches!(ch, '/' | '\0')).count();
        if count > 0 && invalid_replacement_error(rule).is_none() {
            let length = name
                .len()
                .checked_sub(count)
                .and_then(|n| {
                    count
                        .checked_mul(replacement.len())
                        .and_then(|extra| n.checked_add(extra))
                })
                .ok_or(())?;
            if length > MAX_REPLACEMENT_OUTPUT_BYTES {
                return Err(());
            }
            return Ok(name.replace(['/', '\0'], replacement));
        }
    }
    Ok(name)
}

fn collapse_repeats(name: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let mut previous = None;
    let mut result = String::with_capacity(name.len());
    for grapheme in name.graphemes(true) {
        if previous != Some(grapheme) || grapheme.chars().any(char::is_alphanumeric) {
            result.push_str(grapheme);
        }
        previous = Some(grapheme);
    }
    result
}

pub(super) fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\0')
        && name != "."
        && name != ".."
        && !exceeds_component_limit(name)
}

/// The OS rejects a longer path component (`ENAMETOOLONG`): 255 bytes on
/// Linux/BSD filesystems, 255 UTF-16 units on APFS/HFS+ and NTFS.
fn exceeds_component_limit(name: &str) -> bool {
    if cfg!(any(target_os = "macos", windows)) {
        name.encode_utf16().count() > 255
    } else {
        name.len() > 255
    }
}
