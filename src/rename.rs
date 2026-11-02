//! Pure batch-rename planning: turn a list of file names plus a rule into a
//! list of proposed new names, flagging anything invalid or colliding. There
//! is no I/O and no UI here, so the whole transform is unit-tested directly on
//! fabricated name lists.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A lexical ancestor of the folder captured when the rename dialog opens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParentAffix {
    pub depth: u8,
    pub separator: String,
    pub suffix: bool,
}
impl Default for ParentAffix {
    fn default() -> Self {
        Self {
            depth: 1,
            separator: "_".into(),
            suffix: false,
        }
    }
}
impl ParentAffix {
    pub fn error(&self) -> Option<&'static str> {
        if !(1..=32).contains(&self.depth) {
            return Some("Folder level must be between 1 and 32");
        }
        if self.separator.len() > 32
            || self.separator.chars().any(char::is_control)
            || !is_valid_name(&format!("x{}x", self.separator))
        {
            return Some(
                "Folder separator must be at most 32 bytes without control or invalid filename characters",
            );
        }
        None
    }
    fn folder_name<'a>(&self, folder: &'a std::path::Path) -> Option<&'a str> {
        if self.error().is_some() {
            return None;
        }
        folder
            .ancestors()
            .nth(usize::from(self.depth) - 1)?
            .file_name()?
            .to_str()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatePosition {
    #[default]
    Prefix,
    Suffix,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DateStamp {
    pub format: String,
    pub position: DatePosition,
}
impl Default for DateStamp {
    fn default() -> Self {
        Self {
            format: "%Y-%m-%d_".into(),
            position: DatePosition::Prefix,
        }
    }
}
impl DateStamp {
    pub fn format_error(&self) -> Option<&'static str> {
        if self.format.is_empty() || self.format.len() > 64 {
            return Some("Date format needs 1–64 bytes");
        }
        if chrono::format::StrftimeItems::new(&self.format)
            .any(|item| matches!(item, chrono::format::Item::Error))
        {
            return Some("Invalid strftime date format");
        }
        use std::fmt::Write;
        let mut probe = String::new();
        let epoch =
            chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("epoch is representable");
        if write!(&mut probe, "{}", epoch.format(&self.format)).is_err() || probe.len() > 512 {
            return Some("Date format cannot be rendered within 512 bytes");
        }
        None
    }
    fn render(&self, time: std::time::SystemTime) -> Result<String, ()> {
        use std::fmt::Write;
        if self.format_error().is_some() {
            return Err(());
        }
        let (seconds, nanos) = match time.duration_since(std::time::UNIX_EPOCH) {
            Ok(delta) => (
                i64::try_from(delta.as_secs()).map_err(|_| ())?,
                delta.subsec_nanos(),
            ),
            Err(error) => {
                let delta = error.duration();
                let seconds = i64::try_from(delta.as_secs()).map_err(|_| ())?;
                if delta.subsec_nanos() == 0 {
                    (-seconds, 0)
                } else {
                    (
                        -seconds.checked_add(1).ok_or(())?,
                        1_000_000_000 - delta.subsec_nanos(),
                    )
                }
            }
        };
        let date = chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos).ok_or(())?;
        let mut out = String::new();
        write!(&mut out, "{}", date.format(&self.format)).map_err(|_| ())?;
        if out.len() > 512 {
            return Err(());
        }
        Ok(out)
    }
}

/// Case transform applied to the stem; `ExtensionCase` controls the suffix independently.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum CaseMode {
    #[default]
    Keep,
    Lower,
    Upper,
    /// Uppercase the first Unicode scalar and lowercase the remainder.
    Capitalize,
    /// Capitalize Unicode word segments while preserving their separators.
    TitleWords,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ExtensionCase {
    #[default]
    Keep,
    Lower,
    Upper,
}
impl ExtensionCase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Keep => "Keep",
            Self::Lower => "lower",
            Self::Upper => "UPPER",
        }
    }
    fn is_keep(&self) -> bool {
        *self == Self::Keep
    }
}

/// Unicode normalization applied to the complete final name.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum NameNormalization {
    #[default]
    Keep,
    Nfc,
    Nfd,
    Nfkc,
    Nfkd,
}
impl NameNormalization {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Keep => "Keep",
            Self::Nfc => "NFC",
            Self::Nfd => "NFD",
            Self::Nfkc => "NFKC",
            Self::Nfkd => "NFKD",
        }
    }
    fn is_keep(&self) -> bool {
        *self == Self::Keep
    }
    fn apply(self, name: String) -> String {
        use unicode_normalization::UnicodeNormalization;
        match self {
            Self::Keep => name,
            Self::Nfc => name.nfc().collect(),
            Self::Nfd => name.nfd().collect(),
            Self::Nfkc => name.nfkc().collect(),
            Self::Nfkd => name.nfkd().collect(),
        }
    }
}

/// Which original name component receives find/replace. Structural edits
/// still apply to the resulting stem, independently of this setting.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum ReplaceTarget {
    #[default]
    WholeName,
    Stem,
    Extension,
}
impl ReplaceTarget {
    pub const fn label(self) -> &'static str {
        match self {
            Self::WholeName => "Whole name",
            Self::Stem => "Name stem",
            Self::Extension => "Extension",
        }
    }
    fn is_whole_name(&self) -> bool {
        *self == Self::WholeName
    }
}

/// Scope of positional removal/insertion, independent of find/replace scope.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum CharacterTarget {
    #[default]
    Stem,
    Extension,
    WholeName,
}
impl CharacterTarget {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stem => "Name stem",
            Self::Extension => "Extension",
            Self::WholeName => "Whole name",
        }
    }
    fn is_stem(&self) -> bool {
        *self == Self::Stem
    }
}

/// Place the counter around the decorated stem, or replace that stem entirely.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum NumberPosition {
    BeforeName,
    CounterOnly,
    #[default]
    AfterName,
}
impl NumberPosition {
    pub const fn label(self) -> &'static str {
        match self {
            Self::BeforeName => "Before name",
            Self::CounterOnly => "Counter only",
            Self::AfterName => "After name",
        }
    }
    fn is_after(&self) -> bool {
        *self == Self::AfterName
    }
}

/// Rendering of the signed ordinal; letter modes use a bijective Latin alphabet.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum NumberFormat {
    #[default]
    Decimal,
    LowerLetters,
    UpperLetters,
}
impl NumberFormat {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Decimal => "Decimal",
            Self::LowerLetters => "a, b, …",
            Self::UpperLetters => "A, B, …",
        }
    }
    fn is_decimal(&self) -> bool {
        *self == Self::Decimal
    }
    fn render(self, value: i128, pad: usize) -> String {
        if self == Self::Decimal {
            return format!("{value:0width$}", width = pad);
        }
        if value == 0 {
            return "0".into();
        }
        let mut magnitude = value.unsigned_abs();
        let base = if self == Self::UpperLetters {
            b'A'
        } else {
            b'a'
        };
        let mut letters = Vec::new();
        while magnitude != 0 {
            magnitude -= 1;
            letters.push(base + (magnitude % 26) as u8);
            magnitude /= 26;
        }
        if value < 0 {
            letters.push(b'-');
        }
        letters.reverse();
        // Every byte is an ASCII Latin letter or the sign.
        letters.into_iter().map(char::from).collect()
    }
}

/// Optional signed counter on each decorated stem, numbered by input order.
/// Decimal padding includes a possible minus sign; zero step repeats the same value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Numbering {
    pub start: i64,
    pub step: i64,
    pub pad: usize,
}

pub const MAX_EXTRA_REPLACEMENTS: usize = 16;
const MAX_EXTRA_REPLACEMENT_BYTES: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ReplacementPair {
    pub find: String,
    pub replace: String,
}

/// Either remove a positional range or retain only that range.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum RangeAction {
    #[default]
    Remove,
    Keep,
}
impl RangeAction {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Remove => "Remove",
            Self::Keep => "Keep",
        }
    }
    fn is_remove(&self) -> bool {
        *self == Self::Remove
    }
}

/// Positional range of whole grapheme clusters in a transformed component.
/// Offset counts clusters to skip from the chosen edge; action is stored in the rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RemoveRange {
    pub offset: usize,
    pub count: usize,
    pub from_end: bool,
}

impl RemoveRange {
    fn apply(self, stem: &str, action: RangeAction) -> String {
        use unicode_segmentation::UnicodeSegmentation;
        let len = stem.graphemes(true).count();
        let (start, end) = if self.from_end {
            let end = len.saturating_sub(self.offset);
            (end.saturating_sub(self.count), end)
        } else {
            let start = self.offset.min(len);
            (start, start.saturating_add(self.count).min(len))
        };
        stem.graphemes(true)
            .enumerate()
            .filter_map(|(index, part)| {
                let selected = (start..end).contains(&index);
                (selected == (action == RangeAction::Keep)).then_some(part)
            })
            .collect()
    }
}

/// Insert at a grapheme boundary, optionally replacing as many following
/// graphemes as the inserted text contains. From-end changes the anchor only.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct InsertText {
    pub text: String,
    pub offset: usize,
    pub from_end: bool,
    pub overwrite: bool,
}

impl InsertText {
    fn apply(&self, stem: &str) -> String {
        use unicode_segmentation::UnicodeSegmentation;
        let len = stem.graphemes(true).count();
        let start = if self.from_end {
            len.saturating_sub(self.offset)
        } else {
            self.offset.min(len)
        };
        let end = if self.overwrite {
            start
                .saturating_add(self.text.graphemes(true).count())
                .min(len)
        } else {
            start
        };
        let byte_at = |index| {
            stem.grapheme_indices(true)
                .nth(index)
                .map_or(stem.len(), |(byte, _)| byte)
        };
        format!(
            "{}{}{}",
            &stem[..byte_at(start)],
            self.text,
            &stem[byte_at(end)..]
        )
    }
}

/// A composable rename rule. Operations apply in a fixed, documented order:
/// find/replace over the chosen name component, then split off the extension, then
/// removal and insertion on the stem, independent case, prefix/suffix and counter.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RenameRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_stamp: Option<DateStamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_affix: Option<ParentAffix>,
    #[serde(skip_serializing_if = "CharacterTarget::is_stem")]
    pub character_target: CharacterTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert_text: Option<InsertText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_range: Option<RemoveRange>,
    #[serde(skip_serializing_if = "RangeAction::is_remove")]
    pub range_action: RangeAction,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remove_prefix: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remove_suffix: String,
    pub find: String,
    /// In regex mode, `$1`/`${name}`-style capture-group references are
    /// expanded (see [`regex::Regex::replace_all`]).
    pub replace: String,
    /// Additional sequential pairs, sharing the primary replacement options/target.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_replacements: Vec<ReplacementPair>,
    /// Treat `find` as a regular expression instead of a literal substring.
    pub regex: bool,
    /// Case-insensitive matching uses the regex engine's Unicode simple folding.
    #[serde(skip_serializing_if = "is_false")]
    pub ignore_case: bool,
    /// Replace only the leftmost occurrence in the selected name component.
    #[serde(skip_serializing_if = "is_false")]
    pub first_only: bool,
    /// Trim Unicode whitespace from the final complete name.
    #[serde(skip_serializing_if = "is_false")]
    pub trim_spaces: bool,
    /// Collapse identical consecutive non-alphanumeric graphemes in the final name.
    #[serde(skip_serializing_if = "is_false")]
    pub collapse_repeats: bool,
    #[serde(skip_serializing_if = "NameNormalization::is_keep")]
    pub normalization: NameNormalization,
    /// Limit final stem graphemes, keeping the final extension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stem_limit: Option<usize>,
    #[serde(skip_serializing_if = "ReplaceTarget::is_whole_name")]
    pub replace_target: ReplaceTarget,
    pub prefix: String,
    pub suffix: String,
    pub case: CaseMode,
    /// Independent transform of the resulting last extension; directories have none.
    #[serde(skip_serializing_if = "ExtensionCase::is_keep")]
    pub extension_case: ExtensionCase,
    pub numbering: Option<Numbering>,
    #[serde(skip_serializing_if = "NumberFormat::is_decimal")]
    pub number_format: NumberFormat,
    #[serde(skip_serializing_if = "NumberPosition::is_after")]
    pub number_position: NumberPosition,
}

fn is_false(value: &bool) -> bool {
    !value
}

fn compile_pattern(value: &str, rule: &RenameRule) -> Result<Option<regex::Regex>, String> {
    if value.is_empty() || !(rule.regex || rule.ignore_case) {
        return Ok(None);
    }
    let pattern = if rule.regex {
        std::borrow::Cow::Borrowed(value)
    } else {
        std::borrow::Cow::Owned(regex::escape(value))
    };
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(rule.ignore_case)
        .build()
        .map(Some)
        .map_err(|error| error.to_string())
}

fn compile_find(rule: &RenameRule) -> Result<Vec<Option<regex::Regex>>, String> {
    if rule.extra_replacements.len() > MAX_EXTRA_REPLACEMENTS {
        return Err("At most 16 additional replacements are allowed".into());
    }
    let bytes = rule.extra_replacements.iter().fold(0usize, |bytes, pair| {
        bytes
            .saturating_add(pair.find.len())
            .saturating_add(pair.replace.len())
    });
    if bytes > MAX_EXTRA_REPLACEMENT_BYTES {
        return Err("Additional replacement text exceeds 64 KiB".into());
    }
    let mut compiled = vec![compile_pattern(&rule.find, rule)?];
    for (index, pair) in rule.extra_replacements.iter().enumerate() {
        if pair.find.is_empty() {
            return Err(format!(
                "Additional replacement {} needs a Find value",
                index + 1
            ));
        }
        compiled.push(
            compile_pattern(&pair.find, rule)
                .map_err(|error| format!("Additional replacement {}: {error}", index + 1))?,
        );
    }
    Ok(compiled)
}

/// Compiler errors for regex or escaped case-insensitive literal matching.
/// Invalid patterns remain no-ops in the pure planner; callers gate application
/// on this error channel before any filesystem mutation.
pub fn regex_error(rule: &RenameRule) -> Option<String> {
    compile_find(rule).err()
}

/// Outcome of planning one entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlanStatus {
    /// A chained replacement exceeded the bounded intermediate output size.
    ExpansionLimit,
    /// Valid and different from the source name.
    Ok,
    /// The result equals the source name (nothing to do).
    Unchanged,
    /// Empty, contains '/', or is "." / "..".
    Invalid,
    /// Captured modification time or its requested format is unavailable.
    DateUnavailable,
    FolderUnavailable,
    /// Two entries map to the same target, or the target collides with a
    /// sibling that is not itself being renamed away.
    Collision,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenamePlan {
    pub from: String,
    pub to: String,
    pub status: PlanStatus,
}

/// Split a file name into (stem, extension-with-dot). A leading dot (dotfiles
/// like `.gitignore`) is part of the stem, not an extension; the split uses
/// the last interior dot, so `archive.tar.gz` -> (`archive.tar`, `.gz`).
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn apply_case(stem: &str, mode: CaseMode) -> String {
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

const MAX_REPLACEMENT_OUTPUT_BYTES: usize = 64 * 1024;

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
            if rule.regex {
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
fn rename_one(
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
fn rename_one_checked(
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
                        if rule.regex {
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
    Ok(if let Some(limit) = rule.stem_limit {
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
    })
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

fn is_valid_name(name: &str) -> bool {
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

/// Plan a batch rename. `names` are the entries to rename (their order drives
/// the counter); `existing` is every name currently in the directory
/// (including the ones being renamed) so collisions with untouched siblings
/// are caught. `dirs` names the targets that are folders, whose whole name
/// is the stem (no extension split). This compatibility entry point uses the
/// conservative case-folded policy; production callers use
/// [`plan_batch_rename_with`] with the directory's captured policy.
#[cfg(test)]
pub fn plan_batch_rename(
    names: &[String],
    existing: &HashSet<String>,
    rule: &RenameRule,
    dirs: &HashSet<String>,
) -> Vec<RenamePlan> {
    plan_batch_rename_with(
        names,
        existing,
        rule,
        dirs,
        crate::rename_order::NameFolding::CaseFolded,
    )
}

#[cfg(test)]
pub fn plan_batch_rename_with(
    names: &[String],
    existing: &HashSet<String>,
    rule: &RenameRule,
    dirs: &HashSet<String>,
    folding: crate::rename_order::NameFolding,
) -> Vec<RenamePlan> {
    plan_batch_rename_with_dates(names, existing, rule, dirs, folding, &HashMap::new())
}
#[cfg(test)]
pub fn plan_batch_rename_with_dates(
    names: &[String],
    existing: &HashSet<String>,
    rule: &RenameRule,
    dirs: &HashSet<String>,
    folding: crate::rename_order::NameFolding,
    modified: &HashMap<String, std::time::SystemTime>,
) -> Vec<RenamePlan> {
    plan_batch_rename_in_folder(names, existing, rule, dirs, folding, modified, None)
}
pub fn plan_batch_rename_in_folder(
    names: &[String],
    existing: &HashSet<String>,
    rule: &RenameRule,
    dirs: &HashSet<String>,
    folding: crate::rename_order::NameFolding,
    modified: &HashMap<String, std::time::SystemTime>,
    folder: Option<&std::path::Path>,
) -> Vec<RenamePlan> {
    let mut contextual = rule.clone();
    let mut folder_unavailable = false;
    if let Some(affix) = &rule.parent_affix {
        if let Some(name) = folder.and_then(|folder| affix.folder_name(folder)) {
            if affix.suffix {
                contextual.suffix = format!("{}{}{name}", rule.suffix, affix.separator);
            } else {
                contextual.prefix = format!("{name}{}{}", affix.separator, rule.prefix);
            }
        } else {
            folder_unavailable = true;
        }
    }
    let rule = &contextual;
    let batch_lower: HashSet<String> = names.iter().map(|name| folding.key(name)).collect();
    let existing_lower: HashSet<String> = existing.iter().map(|name| folding.key(name)).collect();

    // Compiled once for the whole batch, not per name.
    let compiled = compile_find(rule).ok();
    let mut exceeded = Vec::with_capacity(names.len());
    let mut date_errors = Vec::with_capacity(names.len());
    let targets: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let mut stamped;
            let effective = if let Some(stamp) = &rule.date_stamp {
                let date = modified.get(name).and_then(|time| stamp.render(*time).ok());
                let Some(date) = date else {
                    date_errors.push(true);
                    exceeded.push(false);
                    return name.clone();
                };
                stamped = rule.clone();
                match stamp.position {
                    DatePosition::Prefix => stamped.prefix = format!("{date}{}", rule.prefix),
                    DatePosition::Suffix => stamped.suffix = format!("{}{date}", rule.suffix),
                }
                &stamped
            } else {
                rule
            };
            date_errors.push(false);
            match rename_one_checked(
                name,
                dirs.contains(name),
                index,
                effective,
                compiled.as_deref(),
            ) {
                Ok(target) => {
                    exceeded.push(false);
                    target
                }
                Err(()) => {
                    exceeded.push(true);
                    name.clone()
                }
            }
        })
        .collect();

    // How many targets share each lowercased name (intra-batch clashes).
    let mut counts: HashMap<String, usize> = HashMap::new();
    for t in &targets {
        *counts.entry(folding.key(t)).or_insert(0) += 1;
    }

    names
        .iter()
        .zip(targets.iter())
        .zip(exceeded.iter())
        .zip(date_errors.iter())
        .map(|(((from, to), exceeded), date_error)| {
            let to_lower = folding.key(to);
            let status = if folder_unavailable {
                PlanStatus::FolderUnavailable
            } else if *date_error {
                PlanStatus::DateUnavailable
            } else if *exceeded {
                PlanStatus::ExpansionLimit
            } else if !is_valid_name(to) {
                PlanStatus::Invalid
            } else if to == from {
                PlanStatus::Unchanged
            } else if counts.get(&to_lower).copied().unwrap_or(0) > 1 {
                PlanStatus::Collision
            } else if existing_lower.contains(&to_lower) && !batch_lower.contains(&to_lower) {
                // Clashes with a sibling that is not being renamed away.
                PlanStatus::Collision
            } else {
                PlanStatus::Ok
            };
            RenamePlan {
                from: from.clone(),
                to: to.clone(),
                status,
            }
        })
        .collect()
}

/// Whether a plan set is safe to apply: at least one effective change and no
/// invalid or colliding rows.
pub fn plan_is_applicable(plans: &[RenamePlan]) -> bool {
    let any_change = plans.iter().any(|p| p.status == PlanStatus::Ok);
    let any_bad = plans.iter().any(|p| {
        matches!(
            p.status,
            PlanStatus::ExpansionLimit
                | PlanStatus::Invalid
                | PlanStatus::DateUnavailable
                | PlanStatus::FolderUnavailable
                | PlanStatus::Collision
        )
    });
    any_change && !any_bad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn existing(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Plan a batch whose targets are all regular files.
    fn plan_files(
        names: &[String],
        existing: &HashSet<String>,
        rule: &RenameRule,
    ) -> Vec<RenamePlan> {
        plan_batch_rename(names, existing, rule, &HashSet::new())
    }

    #[test]
    fn parent_folder_affixes_compose_with_dates_and_preserve_directory_extensions() {
        let names: Vec<String> = vec!["Photo.JPG".into(), "folder.old".into()];
        let dates = names
            .iter()
            .map(|name| (name.clone(), std::time::UNIX_EPOCH))
            .collect();
        let dirs = HashSet::from(["folder.old".into()]);
        let mut rule = RenameRule {
            parent_affix: Some(ParentAffix::default()),
            prefix: "pre_".into(),
            suffix: "_post".into(),
            extension_case: ExtensionCase::Lower,
            date_stamp: Some(DateStamp {
                format: "%Y_".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let plan = |rule: &RenameRule| {
            plan_batch_rename_in_folder(
                &names,
                &HashSet::new(),
                rule,
                &dirs,
                crate::rename_order::NameFolding::Exact,
                &dates,
                Some(std::path::Path::new("/Library/旅行/Album")),
            )
        };
        let first = plan(&rule);
        assert_eq!(first[0].to, "1970_Album_pre_Photo_post.jpg");
        assert_eq!(first[1].to, "1970_Album_pre_folder.old_post");
        rule.parent_affix = Some(ParentAffix {
            depth: 2,
            separator: "-".into(),
            suffix: true,
        });
        assert_eq!(plan(&rule)[0].to, "1970_pre_Photo_post-旅行.jpg");
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert!(legacy.parent_affix.is_none());
    }

    #[test]
    fn parent_folder_affixes_refuse_missing_levels_and_keep_collision_checks() {
        let names: Vec<String> = vec!["one.txt".into()];
        let mut rule = RenameRule {
            parent_affix: Some(ParentAffix::default()),
            ..Default::default()
        };
        for folder in [None, Some(std::path::Path::new("/"))] {
            let plans = plan_batch_rename_in_folder(
                &names,
                &HashSet::new(),
                &rule,
                &HashSet::new(),
                crate::rename_order::NameFolding::Exact,
                &HashMap::new(),
                folder,
            );
            assert_eq!(plans[0].status, PlanStatus::FolderUnavailable);
            assert!(!plan_is_applicable(&plans));
        }
        for (depth, separator) in [(0, "_"), (33, "_"), (1, "/"), (1, "\n")] {
            rule.parent_affix = Some(ParentAffix {
                depth,
                separator: separator.into(),
                suffix: false,
            });
            assert!(rule.parent_affix.as_ref().unwrap().error().is_some());
        }
        rule.parent_affix = Some(ParentAffix::default());
        let plans = plan_batch_rename_in_folder(
            &names,
            &HashSet::from(["Album_one.txt".into()]),
            &rule,
            &HashSet::new(),
            crate::rename_order::NameFolding::Exact,
            &HashMap::new(),
            Some(std::path::Path::new("/Album")),
        );
        assert_eq!(plans[0].status, PlanStatus::Collision);
        rule.parent_affix.as_mut().unwrap().depth = 3;
        let plans = plan_batch_rename_in_folder(
            &names,
            &HashSet::new(),
            &rule,
            &HashSet::new(),
            crate::rename_order::NameFolding::Exact,
            &HashMap::new(),
            Some(std::path::Path::new("/Album")),
        );
        assert_eq!(plans[0].status, PlanStatus::FolderUnavailable);
    }

    #[test]
    fn captured_date_affixes_follow_original_names_when_reordered() {
        use std::time::{Duration, UNIX_EPOCH};
        let names = vec!["a.JPG".into(), "b.JPG".into(), "folder.old".into()];
        let existing = names.iter().cloned().collect();
        let dirs = HashSet::from(["folder.old".into()]);
        let modified = HashMap::from([
            ("a.JPG".into(), UNIX_EPOCH),
            ("b.JPG".into(), UNIX_EPOCH + Duration::from_secs(86400)),
            (
                "folder.old".into(),
                UNIX_EPOCH + Duration::from_secs(172800),
            ),
        ]);
        let mut rule = RenameRule {
            date_stamp: Some(DateStamp::default()),
            prefix: "pre-".into(),
            suffix: "-post".into(),
            case: CaseMode::Upper,
            extension_case: ExtensionCase::Lower,
            ..Default::default()
        };
        let plan = plan_batch_rename_with_dates(
            &names,
            &existing,
            &rule,
            &dirs,
            crate::rename_order::NameFolding::Exact,
            &modified,
        );
        assert_eq!(
            plan.iter().map(|p| p.to.as_str()).collect::<Vec<_>>(),
            [
                "1970-01-01_pre-A-post.jpg",
                "1970-01-02_pre-B-post.jpg",
                "1970-01-03_pre-FOLDER.OLD-post"
            ]
        );
        assert!(plan_is_applicable(&plan));
        rule.date_stamp = Some(DateStamp {
            format: "_%Y%m%d".into(),
            position: DatePosition::Suffix,
        });
        let reordered = vec![names[1].clone(), names[0].clone()];
        let plan = plan_batch_rename_with_dates(
            &reordered,
            &existing,
            &rule,
            &dirs,
            crate::rename_order::NameFolding::Exact,
            &modified,
        );
        assert_eq!(plan[0].to, "pre-B-post_19700102.jpg");
        assert_eq!(plan[1].to, "pre-A-post_19700101.jpg");
    }

    #[test]
    fn date_rules_refuse_missing_bad_and_unsafe_formats_and_keep_pre_epoch_precision() {
        use std::time::{Duration, UNIX_EPOCH};
        let names = vec!["ёж.txt".into()];
        let existing = names.iter().cloned().collect();
        let dirs = HashSet::new();
        let modified = HashMap::from([("ёж.txt".into(), UNIX_EPOCH - Duration::from_millis(1))]);
        let mut rule = RenameRule {
            date_stamp: Some(DateStamp {
                format: "%s_%f_".into(),
                position: DatePosition::Prefix,
            }),
            ..Default::default()
        };
        let plan = plan_batch_rename_with_dates(
            &names,
            &existing,
            &rule,
            &dirs,
            crate::rename_order::NameFolding::Exact,
            &modified,
        );
        assert_eq!(plan[0].to, "-1_999000000_ёж.txt");
        let missing = plan_batch_rename_with_dates(
            &names,
            &existing,
            &rule,
            &dirs,
            crate::rename_order::NameFolding::Exact,
            &HashMap::new(),
        );
        assert_eq!(missing[0].status, PlanStatus::DateUnavailable);
        assert!(!plan_is_applicable(&missing));
        for format in ["".to_owned(), "%Q".into(), "%#z".into(), "x".repeat(65)] {
            rule.date_stamp.as_mut().unwrap().format = format;
            let plan = plan_batch_rename_with_dates(
                &names,
                &existing,
                &rule,
                &dirs,
                crate::rename_order::NameFolding::Exact,
                &modified,
            );
            assert_eq!(plan[0].status, PlanStatus::DateUnavailable);
            assert!(!plan_is_applicable(&plan));
        }
        rule.date_stamp.as_mut().unwrap().format = "%Y/%m/%d_".into();
        let plan = plan_batch_rename_with_dates(
            &names,
            &existing,
            &rule,
            &dirs,
            crate::rename_order::NameFolding::Exact,
            &modified,
        );
        assert_eq!(plan[0].status, PlanStatus::Invalid);
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert!(legacy.date_stamp.is_none());
    }

    #[test]
    fn dated_targets_still_use_shared_collision_detection() {
        let names: Vec<String> = vec!["A.txt".into(), "a.txt".into()];
        let existing = names.iter().cloned().collect();
        let modified = names
            .iter()
            .map(|name| (name.clone(), std::time::UNIX_EPOCH))
            .collect();
        let rule = RenameRule {
            date_stamp: Some(DateStamp::default()),
            case: CaseMode::Lower,
            ..Default::default()
        };
        let plan = plan_batch_rename_with_dates(
            &names,
            &existing,
            &rule,
            &HashSet::new(),
            crate::rename_order::NameFolding::Exact,
            &modified,
        );
        assert!(plan.iter().all(|row| row.status == PlanStatus::Collision));
        assert!(!plan_is_applicable(&plan));
    }

    #[test]
    fn replacement_chain_is_ordered_and_composes_with_existing_transforms() {
        let mut rule = RenameRule {
            find: "old".into(),
            replace: "tmp".into(),
            extra_replacements: vec![
                ReplacementPair {
                    find: "tmp".into(),
                    replace: "final".into(),
                },
                ReplacementPair {
                    find: "-".into(),
                    replace: "_".into(),
                },
            ],
            replace_target: ReplaceTarget::Stem,
            case: CaseMode::Upper,
            extension_case: ExtensionCase::Lower,
            prefix: "P-".into(),
            suffix: "-S".into(),
            numbering: Some(Numbering {
                start: 1,
                step: 1,
                pad: 2,
            }),
            ..Default::default()
        };
        for (first_only, expected) in [
            (false, "P-FINAL_FINAL-S01.txt"),
            (true, "P-FINAL_OLD-S01.txt"),
        ] {
            rule.first_only = first_only;
            let compiled = compile_find(&rule).unwrap();
            assert_eq!(
                rename_one("old-old.TXT", false, 0, &rule, Some(&compiled)),
                expected
            );
        }
        rule.first_only = false;
        let compiled = compile_find(&rule).unwrap();
        assert_eq!(
            rename_one("old-old.TXT", true, 0, &rule, Some(&compiled)),
            "P-FINAL_FINAL.TXT-S01"
        );
        assert_eq!(
            serde_json::from_str::<RenameRule>(&serde_json::to_string(&rule).unwrap()).unwrap(),
            rule
        );
        assert!(
            !serde_json::to_value(RenameRule::default())
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("extra_replacements")
        );
        assert_eq!(
            serde_json::from_str::<RenameRule>("{}")
                .unwrap()
                .extra_replacements,
            vec![]
        );
    }

    #[test]
    fn replacement_chain_preserves_regex_capture_and_literal_semantics() {
        for (find, replacement, text) in [
            (r"(?P<word>[a-z]+)", "${word}-$0-$$-${missing}", "ab.txt"),
            (r"^|$", "x", "ab"),
            (r"(a)?b", "$1:${1}x:$missing:$$", "b ab"),
        ] {
            let rule = RenameRule {
                regex: true,
                extra_replacements: vec![ReplacementPair {
                    find: find.into(),
                    replace: replacement.into(),
                }],
                ..Default::default()
            };
            let compiled = compile_find(&rule).unwrap();
            let expected = regex::Regex::new(find)
                .unwrap()
                .replace_all(text, replacement)
                .into_owned();
            assert_eq!(rename_one(text, true, 0, &rule, Some(&compiled)), expected);
        }
        let rule = RenameRule {
            ignore_case: true,
            replace_target: ReplaceTarget::Stem,
            extra_replacements: vec![
                ReplacementPair {
                    find: "a".into(),
                    replace: "$1".into(),
                },
                ReplacementPair {
                    find: "1".into(),
                    replace: "Z".into(),
                },
            ],
            ..Default::default()
        };
        let compiled = compile_find(&rule).unwrap();
        assert_eq!(
            rename_one("A-a.txt", false, 0, &rule, Some(&compiled)),
            "$Z-$Z.txt"
        );
        for (target, expected) in [
            (ReplaceTarget::WholeName, "y.y"),
            (ReplaceTarget::Stem, "y.foo"),
            (ReplaceTarget::Extension, "foo.y"),
        ] {
            let rule = RenameRule {
                find: "foo".into(),
                replace: "x".into(),
                extra_replacements: vec![ReplacementPair {
                    find: "x".into(),
                    replace: "y".into(),
                }],
                replace_target: target,
                ..Default::default()
            };
            let compiled = compile_find(&rule).unwrap();
            assert_eq!(
                rename_one("foo.foo", false, 0, &rule, Some(&compiled)),
                expected
            );
            if target == ReplaceTarget::Extension {
                assert_eq!(
                    rename_one("foo.foo", true, 0, &rule, Some(&compiled)),
                    "foo.foo"
                );
            }
        }
    }

    #[test]
    fn replacement_chain_limits_block_expansion_without_rejecting_bounded_shrinking() {
        let mut rule = RenameRule {
            regex: true,
            replace_target: ReplaceTarget::Stem,
            extra_replacements: vec![
                ReplacementPair {
                    find: ".+".into(),
                    replace: "x".repeat(32768),
                },
                ReplacementPair {
                    find: "(x+)".into(),
                    replace: "$1$1".into(),
                },
                ReplacementPair {
                    find: "x+".into(),
                    replace: "ok".into(),
                },
            ],
            ..Default::default()
        };
        let names = vec!["a.txt".into()];
        let existing = names.iter().cloned().collect();
        assert!(regex_error(&rule).is_none());
        let plans = plan_batch_rename(&names, &existing, &rule, &HashSet::new());
        assert_eq!(plans[0].to, "ok.txt");
        assert!(plan_is_applicable(&plans));
        rule.extra_replacements[1].replace = "${1}${1}x".into();
        let plans = plan_batch_rename(&names, &existing, &rule, &HashSet::new());
        assert_eq!(plans[0].status, PlanStatus::ExpansionLimit);
        assert!(!plan_is_applicable(&plans));
        let mut rule = RenameRule {
            extra_replacements: vec![ReplacementPair::default()],
            ..Default::default()
        };
        assert!(
            regex_error(&rule)
                .unwrap()
                .contains("Additional replacement 1")
        );
        rule.extra_replacements = vec![
            ReplacementPair {
                find: "a".into(),
                replace: "b".into()
            };
            17
        ];
        assert!(regex_error(&rule).is_some());
        rule.extra_replacements = vec![ReplacementPair {
            find: "a".into(),
            replace: "b".repeat(65536),
        }];
        assert!(regex_error(&rule).is_some());
        rule.extra_replacements = vec![ReplacementPair {
            find: "[".into(),
            replace: "b".into(),
        }];
        rule.regex = true;
        assert!(
            regex_error(&rule)
                .unwrap()
                .contains("Additional replacement 1")
        );
    }

    #[test]
    fn number_position_composes_with_decorations_extensions_and_legacy_rules() {
        let mut rule = RenameRule {
            prefix: "_".into(),
            suffix: "_end".into(),
            case: CaseMode::Upper,
            extension_case: ExtensionCase::Lower,
            numbering: Some(Numbering {
                start: 3,
                step: -2,
                pad: 3,
            }),
            number_position: NumberPosition::BeforeName,
            ..Default::default()
        };
        assert_eq!(
            rename_one("Photo.JPG", false, 0, &rule, None),
            "003_PHOTO_end.jpg"
        );
        assert_eq!(
            rename_one("Photo.JPG", false, 2, &rule, None),
            "-01_PHOTO_end.jpg"
        );
        assert_eq!(rename_one("a.b", true, 0, &rule, None), "003_A.B_end");
        rule.number_position = NumberPosition::AfterName;
        assert_eq!(
            rename_one("Photo.JPG", false, 0, &rule, None),
            "_PHOTO_end003.jpg"
        );
        assert!(
            !serde_json::to_string(&rule)
                .unwrap()
                .contains("number_position")
        );
        let restored: RenameRule =
            serde_json::from_str(r#"{"numbering":{"start":3,"step":1,"pad":2}}"#).unwrap();
        assert_eq!(restored.number_position, NumberPosition::AfterName);
        assert_eq!(rename_one("x.txt", false, 0, &restored, None), "x03.txt");
        rule.numbering = None;
        rule.number_position = NumberPosition::BeforeName;
        assert_eq!(
            rename_one("Photo.JPG", false, 0, &rule, None),
            "_PHOTO_end.jpg"
        );
    }

    #[test]
    fn keep_range_extracts_graphemes_from_either_edge_and_clamps() {
        for (offset, count, from_end, input, expected) in [
            (4, 8, false, "IMG_20261030_raw.jpg", "20261030.jpg"),
            (4, 8, true, "IMG_20261030_raw.jpg", "20261030.jpg"),
            (1, 2, false, "Ae\u{301}👩‍💻Z.txt", "e\u{301}👩‍💻.txt"),
            (1, 2, true, "Ae\u{301}👩‍💻Z.txt", "e\u{301}👩‍💻.txt"),
            (0, usize::MAX, false, "short.txt", "short.txt"),
            (0, usize::MAX, true, "short.txt", "short.txt"),
            (usize::MAX, 2, false, "short.txt", ".txt"),
            (usize::MAX, 2, true, "short.txt", ".txt"),
        ] {
            let rule = RenameRule {
                range_action: RangeAction::Keep,
                remove_range: Some(RemoveRange {
                    offset,
                    count,
                    from_end,
                }),
                ..Default::default()
            };
            assert_eq!(rename_one(input, false, 0, &rule, None), expected);
        }
        let rule = RenameRule {
            range_action: RangeAction::Keep,
            remove_range: Some(RemoveRange {
                offset: 7,
                count: 4,
                from_end: false,
            }),
            ..Default::default()
        };
        assert_eq!(rename_one("folder.name", true, 0, &rule, None), "name");
    }

    #[test]
    fn keep_range_composes_with_scopes_replacement_insertion_and_case() {
        let mut rule = RenameRule {
            range_action: RangeAction::Keep,
            character_target: CharacterTarget::Extension,
            replace_target: ReplaceTarget::Extension,
            find: "old".into(),
            replace: "abcde".into(),
            remove_range: Some(RemoveRange {
                offset: 1,
                count: 2,
                from_end: false,
            }),
            insert_text: Some(InsertText {
                text: "z".into(),
                offset: 2,
                from_end: false,
                overwrite: false,
            }),
            extension_case: ExtensionCase::Upper,
            ..Default::default()
        };
        assert_eq!(
            rename_one("report.old", false, 0, &rule, None),
            "report.BCZ"
        );
        assert_eq!(rename_one("folder.old", true, 0, &rule, None), "folder.old");
        rule.character_target = CharacterTarget::WholeName;
        rule.find.clear();
        rule.insert_text = None;
        rule.extension_case = ExtensionCase::Keep;
        assert_eq!(rename_one("report.txt", false, 0, &rule, None), "ep");
    }

    #[test]
    fn keep_range_preserves_legacy_remove_and_validates_empty_or_duplicate_names() {
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.range_action, RangeAction::Remove);
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("range_action")
        );
        let mut rule = RenameRule {
            range_action: RangeAction::Keep,
            remove_range: Some(RemoveRange {
                offset: 1,
                count: 3,
                from_end: false,
            }),
            ..Default::default()
        };
        let plans = plan_files(
            &["xfoo.txt".into(), "yfoo.txt".into()],
            &HashSet::new(),
            &rule,
        );
        assert!(
            plans
                .iter()
                .all(|plan| plan.to == "foo.txt" && plan.status == PlanStatus::Collision)
        );
        rule.character_target = CharacterTarget::WholeName;
        rule.remove_range.as_mut().unwrap().offset = 255;
        assert_eq!(
            plan_files(&["foo.txt".into()], &HashSet::new(), &rule)[0].status,
            PlanStatus::Invalid
        );
    }

    #[test]
    fn letter_counter_boundaries_signs_and_padding_are_explicit() {
        for (value, expected) in [
            (0, "0"),
            (1, "a"),
            (26, "z"),
            (27, "aa"),
            (52, "az"),
            (53, "ba"),
            (702, "zz"),
            (703, "aaa"),
            (-1, "-a"),
            (-27, "-aa"),
        ] {
            assert_eq!(NumberFormat::LowerLetters.render(value, 6), expected);
            assert_eq!(
                NumberFormat::UpperLetters.render(value, 0),
                expected.to_uppercase()
            );
        }
        assert_eq!(NumberFormat::Decimal.render(-7, 4), "-007");
        for value in [i128::MIN, i128::MAX] {
            let result = NumberFormat::LowerLetters.render(value, usize::MAX);
            assert!(result.len() <= 29);
            assert!(
                result
                    .trim_start_matches('-')
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase())
            );
            assert_eq!(result.starts_with('-'), value < 0);
        }
    }

    #[test]
    fn letter_numbering_composes_with_position_case_and_signed_steps() {
        let mut rule = RenameRule {
            number_format: NumberFormat::LowerLetters,
            numbering: Some(Numbering {
                start: 25,
                step: 1,
                pad: 6,
            }),
            number_position: NumberPosition::CounterOnly,
            ..Default::default()
        };
        for (index, expected) in [(0, "y.txt"), (1, "z.txt"), (2, "aa.txt")] {
            assert_eq!(rename_one("photo.txt", false, index, &rule, None), expected);
        }
        rule.numbering = Some(Numbering {
            start: 1,
            step: -1,
            pad: 2,
        });
        for (index, expected) in [(0, "a.txt"), (1, "0.txt"), (2, "-a.txt")] {
            assert_eq!(rename_one("photo.txt", false, index, &rule, None), expected);
        }
        rule.number_position = NumberPosition::BeforeName;
        rule.prefix = "P-".into();
        rule.suffix = "-S".into();
        rule.case = CaseMode::Upper;
        assert_eq!(
            rename_one("photo.txt", false, 0, &rule, None),
            "aP-PHOTO-S.txt"
        );
        rule.number_format = NumberFormat::UpperLetters;
        rule.number_position = NumberPosition::AfterName;
        assert_eq!(
            rename_one("photo.txt", false, 0, &rule, None),
            "P-PHOTO-SA.txt"
        );
    }

    #[test]
    fn letter_counters_keep_legacy_defaults_and_collision_guards() {
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.number_format, NumberFormat::Decimal);
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("number_format")
        );
        let rule = RenameRule {
            number_format: NumberFormat::LowerLetters,
            numbering: Some(Numbering {
                start: 1,
                step: 0,
                pad: 2,
            }),
            number_position: NumberPosition::CounterOnly,
            ..Default::default()
        };
        let plans = plan_files(
            &["one.txt".into(), "two.txt".into()],
            &HashSet::new(),
            &rule,
        );
        assert!(
            plans
                .iter()
                .all(|plan| plan.to == "a.txt" && plan.status == PlanStatus::Collision)
        );
        assert!(!plan_is_applicable(&plans));
        let wide = RenameRule {
            numbering: Some(Numbering {
                start: i64::MIN,
                step: i64::MIN,
                pad: 0,
            }),
            ..rule
        };
        let output = rename_one("photo.txt", false, usize::MAX, &wide, None);
        assert!(output.starts_with('-') && output.ends_with(".txt") && output.len() < 40);
    }

    #[test]
    fn character_edit_scopes_handle_extensions_folders_and_dotfiles() {
        for (target, input, is_dir, expected) in [
            (CharacterTarget::Stem, "report.txt", false, "_report.txt"),
            (
                CharacterTarget::Extension,
                "report.txt",
                false,
                "report._txt",
            ),
            (
                CharacterTarget::WholeName,
                "report.txt",
                false,
                "_report.txt",
            ),
            (CharacterTarget::Extension, "report", false, "report._"),
            (CharacterTarget::Extension, ".hidden", false, ".hidden._"),
            (
                CharacterTarget::Extension,
                "folder.name",
                true,
                "folder.name",
            ),
            (
                CharacterTarget::WholeName,
                "folder.name",
                true,
                "_folder.name",
            ),
            (
                CharacterTarget::Extension,
                "archive.tar.gz",
                false,
                "archive.tar._gz",
            ),
        ] {
            let rule = RenameRule {
                character_target: target,
                insert_text: Some(InsertText {
                    text: "_".into(),
                    offset: 0,
                    from_end: false,
                    overwrite: false,
                }),
                ..Default::default()
            };
            assert_eq!(rename_one(input, is_dir, 0, &rule, None), expected);
        }
        let no_op = RenameRule {
            character_target: CharacterTarget::Extension,
            insert_text: Some(InsertText {
                text: String::new(),
                offset: 0,
                from_end: false,
                overwrite: true,
            }),
            ..Default::default()
        };
        assert_eq!(rename_one("report.", false, 0, &no_op, None), "report.");
    }

    #[test]
    fn character_edits_compose_with_replacement_and_extension_case() {
        let rule = RenameRule {
            character_target: CharacterTarget::Extension,
            replace_target: ReplaceTarget::Extension,
            find: "old".into(),
            replace: "txt".into(),
            remove_range: Some(RemoveRange {
                offset: 0,
                count: 1,
                from_end: false,
            }),
            insert_text: Some(InsertText {
                text: "p".into(),
                offset: 0,
                from_end: false,
                overwrite: false,
            }),
            extension_case: ExtensionCase::Upper,
            case: CaseMode::Lower,
            prefix: "P-".into(),
            ..Default::default()
        };
        assert_eq!(
            rename_one("Report.old", false, 0, &rule, None),
            "P-report.PXT"
        );
        let whole = RenameRule {
            character_target: CharacterTarget::WholeName,
            remove_range: Some(RemoveRange {
                offset: 6,
                count: 1,
                from_end: false,
            }),
            ..Default::default()
        };
        assert_eq!(
            rename_one("report.txt", false, 0, &whole, None),
            "reporttxt"
        );
        let unicode = RenameRule {
            character_target: CharacterTarget::Extension,
            remove_range: Some(RemoveRange {
                offset: 0,
                count: 1,
                from_end: false,
            }),
            ..Default::default()
        };
        assert_eq!(
            rename_one("name.e\u{301}👩‍💻", false, 0, &unicode, None),
            "name.👩‍💻"
        );
    }

    #[test]
    fn character_scope_defaults_and_validation_preserve_existing_guards() {
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.character_target, CharacterTarget::Stem);
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("character_target")
        );
        let mut rule = RenameRule {
            character_target: CharacterTarget::Extension,
            remove_range: Some(RemoveRange {
                offset: 0,
                count: 255,
                from_end: false,
            }),
            ..Default::default()
        };
        let plans = plan_files(&["a.txt".into(), "a.md".into()], &HashSet::new(), &rule);
        assert!(
            plans
                .iter()
                .all(|plan| plan.to == "a" && plan.status == PlanStatus::Collision)
        );
        rule.character_target = CharacterTarget::WholeName;
        assert_eq!(
            plan_files(&["foo.txt".into()], &HashSet::new(), &rule)[0].status,
            PlanStatus::Invalid
        );
    }

    #[test]
    fn insertion_and_overwrite_use_whole_graphemes_and_clamp_anchors() {
        for (offset, from_end, overwrite, text, input, expected) in [
            (2, false, false, "X", "abcdef.txt", "abXcdef.txt"),
            (2, true, false, "X", "abcdef.txt", "abcdXef.txt"),
            (1, false, true, "文", "Ae\u{301}👩‍💻Z.jpg", "A文👩‍💻Z.jpg"),
            (2, true, true, "👨‍👩‍👧", "Ae\u{301}👩‍💻Z.jpg", "Ae\u{301}👨‍👩‍👧Z.jpg"),
            (usize::MAX, false, false, "X", "a.txt", "aX.txt"),
            (usize::MAX, true, true, "YZ", "a.txt", "YZ.txt"),
            (1, false, true, "LONG", "ab.txt", "aLONG.txt"),
            (0, false, true, "", "a.txt", "a.txt"),
            (0, false, false, "$1", ".hidden", "$1.hidden"),
            (0, true, false, "_", "archive.tar.gz", "archive.tar_.gz"),
        ] {
            let rule = RenameRule {
                insert_text: Some(InsertText {
                    offset,
                    from_end,
                    overwrite,
                    text: text.into(),
                }),
                ..Default::default()
            };
            assert_eq!(rename_one(input, false, 0, &rule, None), expected);
        }
        let rule = RenameRule {
            insert_text: Some(InsertText {
                offset: 0,
                from_end: true,
                overwrite: false,
                text: "_".into(),
            }),
            ..Default::default()
        };
        assert_eq!(
            rename_one("folder.name", true, 0, &rule, None),
            "folder.name_"
        );
    }

    #[test]
    fn insertion_composes_after_removal_before_case_and_decoration() {
        let rule = RenameRule {
            remove_range: Some(RemoveRange {
                offset: 1,
                count: 2,
                from_end: false,
            }),
            insert_text: Some(InsertText {
                text: "z".into(),
                offset: 1,
                from_end: false,
                overwrite: false,
            }),
            find: "old".into(),
            replace: "abcde".into(),
            case: CaseMode::Upper,
            prefix: "P-".into(),
            suffix: "-S".into(),
            numbering: Some(Numbering {
                start: 7,
                step: 1,
                pad: 2,
            }),
            ..Default::default()
        };
        assert_eq!(
            rename_one("old.txt", false, 0, &rule, None),
            "P-AZDE-S07.txt"
        );
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert!(legacy.insert_text.is_none());
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("insert_text")
        );
    }

    #[test]
    fn inserted_invalid_names_and_overwrite_collisions_are_blocked() {
        let mut rule = RenameRule {
            insert_text: Some(InsertText {
                text: "z".into(),
                offset: 0,
                from_end: false,
                overwrite: true,
            }),
            ..Default::default()
        };
        let inputs = ["xfoo.txt".into(), "yfoo.txt".into()];
        let plans = plan_files(&inputs, &HashSet::new(), &rule);
        assert!(
            plans
                .iter()
                .all(|plan| plan.status == PlanStatus::Collision)
        );
        assert!(!plan_is_applicable(&plans));
        rule.insert_text.as_mut().unwrap().text = "/".into();
        assert!(
            plan_files(&inputs, &HashSet::new(), &rule)
                .iter()
                .all(|plan| plan.status == PlanStatus::Invalid)
        );
        rule.insert_text.as_mut().unwrap().text = "a".repeat(256);
        assert!(
            plan_files(&inputs, &HashSet::new(), &rule)
                .iter()
                .all(|plan| plan.status == PlanStatus::Invalid)
        );
    }

    #[test]
    fn remove_ranges_keep_graphemes_extensions_and_clamp_from_both_edges() {
        for (offset, count, from_end, input, expected) in [
            (1, 2, false, "abcdef.txt", "adef.txt"),
            (1, 2, true, "abcdef.txt", "abcf.txt"),
            (1, 1, false, "Ae\u{301}👩‍💻Z.jpg", "A👩‍💻Z.jpg"),
            (1, 1, true, "Ae\u{301}👩‍💻Z.jpg", "Ae\u{301}Z.jpg"),
            (99, usize::MAX, false, "short.txt", "short.txt"),
            (99, usize::MAX, true, "short.txt", "short.txt"),
            (1, usize::MAX, false, "abc.txt", "a.txt"),
            (1, usize::MAX, true, "abc.txt", "c.txt"),
            (0, 1, false, ".hidden", "hidden"),
            (0, 2, true, "archive.tar.gz", "archive.t.gz"),
        ] {
            let rule = RenameRule {
                remove_range: Some(RemoveRange {
                    offset,
                    count,
                    from_end,
                }),
                ..Default::default()
            };
            assert_eq!(rename_one(input, false, 0, &rule, None), expected);
        }
        let rule = RenameRule {
            remove_range: Some(RemoveRange {
                offset: 1,
                count: 2,
                from_end: true,
            }),
            ..Default::default()
        };
        assert_eq!(rename_one("folder.name", true, 0, &rule, None), "folder.ne");
    }

    #[test]
    fn remove_range_composes_after_replacements_before_decoration_and_numbering() {
        let rule = RenameRule {
            remove_range: Some(RemoveRange {
                offset: 1,
                count: 2,
                from_end: false,
            }),
            find: "old".into(),
            replace: "abcde".into(),
            case: CaseMode::Upper,
            prefix: "P-".into(),
            suffix: "-S".into(),
            numbering: Some(Numbering {
                start: 7,
                step: 1,
                pad: 2,
            }),
            ..Default::default()
        };
        assert_eq!(
            rename_one("old.txt", false, 0, &rule, None),
            "P-ADE-S07.txt"
        );
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert!(legacy.remove_range.is_none());
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("remove_range")
        );
    }

    #[test]
    fn remove_range_reports_empty_and_duplicate_results() {
        let rule = RenameRule {
            remove_range: Some(RemoveRange {
                offset: 0,
                count: 1,
                from_end: false,
            }),
            ..Default::default()
        };
        let plans = plan_files(
            &["a".into(), "xfoo.txt".into(), "yfoo.txt".into()],
            &HashSet::new(),
            &rule,
        );
        assert_eq!(plans[0].status, PlanStatus::Invalid);
        assert_eq!(plans[1].status, PlanStatus::Collision);
        assert_eq!(plans[2].status, PlanStatus::Collision);
        assert!(!plan_is_applicable(&plans));
    }

    #[test]
    fn stem_limit_preserves_extension_and_whole_graphemes_and_checks_collisions() {
        let rule = RenameRule {
            stem_limit: Some(2),
            ..Default::default()
        };
        for (input, expected) in [
            ("abcdef.txt", "ab.txt"),
            ("e\u{301}👩‍💻rest.jpg", "e\u{301}👩‍💻.jpg"),
            ("a.txt", "a.txt"),
            (".hidden", ".h"),
            ("archive.tar.gz", "ar.gz"),
        ] {
            assert_eq!(rename_one(input, false, 0, &rule, None), expected);
        }
        assert_eq!(rename_one("folder.name", true, 0, &rule, None), "fo");
        let decorated = RenameRule {
            prefix: "XY".into(),
            ..rule.clone()
        };
        assert_eq!(
            rename_one("abcdef.txt", false, 0, &decorated, None),
            "XY.txt"
        );
        let plans = plan_files(
            &["abc.txt".into(), "abd.txt".into()],
            &HashSet::new(),
            &rule,
        );
        assert!(
            plans
                .iter()
                .all(|plan| plan.status == PlanStatus::Collision)
        );
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.stem_limit, None);
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("stem_limit")
        );
        assert_eq!(
            rename_one("abcdef.txt", false, 0, &legacy, None),
            "abcdef.txt"
        );
    }

    #[test]
    fn remove_affixes_is_exact_once_before_extension_and_other_transforms() {
        let rule = RenameRule {
            remove_prefix: "backup_".into(),
            remove_suffix: ".old".into(),
            extension_case: ExtensionCase::Upper,
            ..Default::default()
        };
        assert_eq!(
            rename_one("backup_photo.jpg.old", false, 0, &rule, None),
            "photo.JPG"
        );
        assert_eq!(
            rename_one("backup_backup_photo.jpg.old.old", false, 0, &rule, None),
            "backup_photo.jpg.OLD"
        );
        assert_eq!(
            rename_one("BACKUP_photo.jpg.OLD", false, 0, &rule, None),
            "BACKUP_photo.jpg.OLD"
        );
        assert_eq!(rename_one("backup_目录.old", true, 0, &rule, None), "目录");
        let decorated = RenameRule {
            prefix: "new_".into(),
            suffix: "_done".into(),
            ..rule.clone()
        };
        assert_eq!(
            rename_one("backup_photo.jpg.old", false, 0, &decorated, None),
            "new_photo_done.JPG"
        );
        let names = vec![
            "backup_photo.jpg.old".into(),
            "photo.JPG".into(),
            "backup_.old".into(),
        ];
        let plans = plan_files(&names, &HashSet::new(), &rule);
        assert_eq!(plans[0].status, PlanStatus::Collision);
        assert_eq!(plans[2].status, PlanStatus::Invalid);
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert!(legacy.remove_prefix.is_empty() && legacy.remove_suffix.is_empty());
        assert!(!serde_json::to_string(&legacy).unwrap().contains("remove_"));
    }

    #[test]
    fn normalization_forms_apply_after_decoration_and_reject_invalid_results() {
        for (normalization, input, expected) in [
            (NameNormalization::Nfc, "cafe\u{301}.txt", "café.txt"),
            (NameNormalization::Nfd, "café.txt", "cafe\u{301}.txt"),
            (NameNormalization::Nfkc, "Ｆｉｌｅ①.txt", "File1.txt"),
            (NameNormalization::Nfkd, "é①.txt", "e\u{301}1.txt"),
        ] {
            let rule = RenameRule {
                normalization,
                ..Default::default()
            };
            assert_eq!(rename_one(input, false, 0, &rule, None), expected);
            assert_eq!(rename_one(input, true, 0, &rule, None), expected);
            assert_eq!(rename_one(expected, false, 0, &rule, None), expected);
        }
        let rule = RenameRule {
            suffix: "\u{301}".into(),
            normalization: NameNormalization::Nfc,
            ..Default::default()
        };
        assert_eq!(rename_one("cafe", false, 0, &rule, None), "café");
        let rule = RenameRule {
            normalization: NameNormalization::Nfkc,
            ..Default::default()
        };
        let names = vec![
            "a／b.txt".into(),
            "．．".into(),
            "Ｆｉｌｅ.txt".into(),
            "File.txt".into(),
        ];
        let plans = plan_files(&names, &HashSet::new(), &rule);
        assert_eq!(plans[0].status, PlanStatus::Invalid);
        assert_eq!(plans[1].status, PlanStatus::Invalid);
        assert_eq!(plans[2].status, PlanStatus::Collision);
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.normalization, NameNormalization::Keep);
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("normalization")
        );
        assert_eq!(
            rename_one("cafe\u{301}.txt", false, 0, &legacy, None),
            "cafe\u{301}.txt"
        );
    }

    #[test]
    fn collapse_repeats_preserves_letters_and_unicode_clusters_and_checks_collisions() {
        let rule = RenameRule {
            collapse_repeats: true,
            ..Default::default()
        };
        for (name, expected) in [
            ("book___final--v22..txt", "book_final-v22.txt"),
            ("a  b__--c", "a b_-c"),
            ("éé_e\u{301}e\u{301}__文件11", "éé_e\u{301}e\u{301}_文件11"),
            ("a👩‍💻👩‍💻b", "a👩‍💻b"),
        ] {
            assert_eq!(rename_one(name, false, 0, &rule, None), expected);
            assert_eq!(rename_one(name, true, 0, &rule, None), expected);
        }
        let decorated = RenameRule {
            prefix: "__".into(),
            suffix: "__".into(),
            trim_spaces: true,
            ..rule.clone()
        };
        assert_eq!(
            rename_one("_folder_ ", true, 0, &decorated, None),
            "_folder_ _"
        );
        let names = vec!["a__b.txt".into(), "a_b.txt".into(), "...".into()];
        let plans = plan_files(&names, &HashSet::new(), &rule);
        assert_eq!(plans[0].status, PlanStatus::Collision);
        assert_eq!(plans[2].status, PlanStatus::Invalid);
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert!(!legacy.collapse_repeats);
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("collapse_repeats")
        );
        assert_eq!(rename_one("a__b", false, 0, &legacy, None), "a__b");
    }

    #[test]
    fn trim_spaces_cleans_final_names_and_reports_invalid_or_colliding_results() {
        let rule = RenameRule {
            trim_spaces: true,
            ..Default::default()
        };
        for (name, expected) in [
            (" Photo.JPG ", "Photo.JPG"),
            ("\u{2003}a b.txt\t", "a b.txt"),
            ("folder name ", "folder name"),
        ] {
            assert_eq!(rename_one(name, false, 0, &rule, None), expected);
            assert_eq!(rename_one(name, true, 0, &rule, None), expected);
        }
        let decorated = RenameRule {
            prefix: "  ".into(),
            suffix: "  ".into(),
            ..rule.clone()
        };
        assert_eq!(rename_one("folder", true, 0, &decorated, None), "folder");
        let names = vec![" a.txt ".into(), "a.txt".into(), "   ".into()];
        let plans = plan_files(&names, &HashSet::new(), &rule);
        assert_eq!(plans[0].status, PlanStatus::Collision);
        assert_eq!(plans[2].status, PlanStatus::Invalid);
        let legacy: RenameRule = serde_json::from_str("{}").unwrap();
        assert!(!legacy.trim_spaces);
        assert_eq!(rename_one(" a.txt ", false, 0, &legacy, None), " a.txt ");
    }

    #[test]
    fn counter_only_replaces_decorated_stem_and_retains_extension() {
        let mut rule = RenameRule {
            prefix: "ignored_".into(),
            suffix: "_ignored".into(),
            number_position: NumberPosition::CounterOnly,
            extension_case: ExtensionCase::Lower,
            numbering: Some(Numbering {
                start: 1,
                step: -1,
                pad: 3,
            }),
            ..Default::default()
        };
        assert_eq!(rename_one("Photo.JPG", false, 0, &rule, None), "001.jpg");
        assert_eq!(rename_one("Photo.JPG", false, 2, &rule, None), "-01.jpg");
        assert_eq!(rename_one("folder.with.dots", true, 0, &rule, None), "001");
        assert_eq!(rename_one("LICENSE", false, 0, &rule, None), "001");
        let names = vec!["a.txt".to_string(), "b.txt".to_string()];
        rule.numbering.as_mut().unwrap().step = 0;
        let plans = plan_files(&names, &HashSet::new(), &rule);
        assert!(
            plans
                .iter()
                .all(|plan| plan.status == PlanStatus::Collision)
        );
        rule.numbering = None;
        assert_eq!(
            rename_one("Photo.JPG", false, 0, &rule, None),
            "ignored_Photo_ignored.jpg"
        );
    }

    #[test]
    fn signed_numbering_descends_crosses_zero_and_pads_the_sign() {
        let rule = RenameRule {
            numbering: Some(Numbering {
                start: 3,
                step: -2,
                pad: 3,
            }),
            ..Default::default()
        };
        for (index, expected) in [
            "Photo003.JPG",
            "Photo001.JPG",
            "Photo-01.JPG",
            "Photo-03.JPG",
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(
                rename_one("Photo.JPG", false, index, &rule, None),
                *expected
            );
        }
        let mut negative = rule.clone();
        negative.numbering.as_mut().unwrap().start = -12;
        assert_eq!(rename_one("folder", true, 1, &negative, None), "folder-14");
        negative.numbering.as_mut().unwrap().step = 0;
        assert_eq!(rename_one("a", false, 99, &negative, None), "a-12");
    }

    #[test]
    fn signed_numbering_widens_arithmetic_and_accepts_legacy_unsigned_values() {
        let legacy: Numbering =
            serde_json::from_str(r#"{"start":4294967295,"step":4294967295,"pad":0}"#).unwrap();
        let rule = RenameRule {
            numbering: Some(legacy),
            ..Default::default()
        };
        assert_eq!(rename_one("x", false, 1, &rule, None), "x8589934590");
        for (start, step) in [(i64::MAX, i64::MAX), (i64::MIN, i64::MIN)] {
            let rule = RenameRule {
                numbering: Some(Numbering {
                    start,
                    step,
                    pad: 0,
                }),
                ..Default::default()
            };
            let expected = i128::from(start) + usize::MAX as i128 * i128::from(step);
            assert_eq!(
                rename_one("x", false, usize::MAX, &rule, None),
                format!("x{expected}")
            );
        }
    }

    #[test]
    fn extension_case_changes_only_the_last_extension_after_find_replace() {
        for (source, lower, upper) in [
            ("Photo.JPG", "Photo.jpg", "Photo.JPG"),
            ("archive.TAR.GZ", "archive.TAR.gz", "archive.TAR.GZ"),
            (".ENV", ".ENV", ".ENV"),
            ("config.É", "config.é", "config.É"),
            ("file.ß", "file.ß", "file.SS"),
            ("plain", "plain", "plain"),
            ("trailing.", "trailing.", "trailing."),
        ] {
            for (mode, expected) in [(ExtensionCase::Lower, lower), (ExtensionCase::Upper, upper)] {
                let rule = RenameRule {
                    extension_case: mode,
                    ..Default::default()
                };
                assert_eq!(rename_one(source, false, 0, &rule, None), expected);
                assert_eq!(
                    rename_one(source, true, 0, &rule, None),
                    source,
                    "directories have no extension"
                );
            }
        }
        let rule = RenameRule {
            find: "JPG".into(),
            replace: "PNG".into(),
            extension_case: ExtensionCase::Lower,
            case: CaseMode::Upper,
            prefix: "p_".into(),
            numbering: Some(Numbering {
                start: 2,
                step: 1,
                pad: 1,
            }),
            ..Default::default()
        };
        assert_eq!(
            rename_one("Photo.JPG", false, 0, &rule, None),
            "p_PHOTO2.png"
        );
        let rule = RenameRule {
            replace_target: ReplaceTarget::Extension,
            ..rule
        };
        assert_eq!(
            rename_one("Photo.JPG", false, 0, &rule, None),
            "p_PHOTO2.png"
        );
    }

    #[test]
    fn extension_case_defaults_preserve_legacy_json_and_collisions_are_blocked() {
        let legacy: RenameRule = serde_json::from_str(r#"{"case":"Lower"}"#).unwrap();
        assert_eq!(legacy.extension_case, ExtensionCase::Keep);
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("extension_case")
        );
        let rule = RenameRule {
            extension_case: ExtensionCase::Lower,
            ..Default::default()
        };
        let plan = plan_files(
            &names(&["a.TXT", "a.txt"]),
            &existing(&["a.TXT", "a.txt"]),
            &rule,
        );
        assert_eq!(plan[0].status, PlanStatus::Collision);
        assert_eq!(plan[1].status, PlanStatus::Unchanged);
        assert!(!plan_is_applicable(&plan));
        assert!(serde_json::from_str::<RenameRule>(r#"{"extension_case":"TitleWords"}"#).is_err());
    }

    #[test]
    fn capitalization_modes_use_unicode_and_preserve_word_separators() {
        for (input, capital, title) in [
            ("hELLO wORLD", "Hello world", "Hello World"),
            ("пРИВЕТ-мИР_тЕСТ", "Привет-мир_тест", "Привет-Мир_тест"),
            (
                "e\u{301}COLE café",
                "E\u{301}cole café",
                "E\u{301}cole Café",
            ),
            ("ßETA TEST", "SSeta test", "SSeta Test"),
            (
                "hello  WORLD\tAGAIN",
                "Hello  world\tagain",
                "Hello  World\tAgain",
            ),
            ("", "", ""),
        ] {
            assert_eq!(apply_case(input, CaseMode::Capitalize), capital, "{input}");
            assert_eq!(apply_case(input, CaseMode::TitleWords), title, "{input}");
        }
    }

    #[test]
    fn capitalization_keeps_extension_and_existing_transform_order() {
        for (mode, expected) in [
            (CaseMode::Capitalize, "PRE_Hello world_END07.TXT"),
            (CaseMode::TitleWords, "PRE_Hello World_END07.TXT"),
        ] {
            let rule = RenameRule {
                find: "OLD".into(),
                replace: "hELLO".into(),
                case: mode,
                prefix: "PRE_".into(),
                suffix: "_END".into(),
                numbering: Some(Numbering {
                    start: 7,
                    step: 1,
                    pad: 2,
                }),
                ..Default::default()
            };
            let plans = plan_files(
                &names(&["OLD wORLD.TXT"]),
                &existing(&["OLD wORLD.TXT"]),
                &rule,
            );
            assert_eq!(plans[0].to, expected);
            assert_eq!(plans[0].status, PlanStatus::Ok);
            assert_eq!(
                rename_one(
                    "hELLO.wORLD",
                    true,
                    0,
                    &RenameRule {
                        case: mode,
                        ..Default::default()
                    },
                    None
                ),
                "Hello.world"
            );
        }
    }
    #[test]
    fn first_only_and_ignore_case_compose_without_expanding_literal_replacements() {
        let input = names(&["Aa-aA-aa.txt"]);
        let existing = existing(&["Aa-aA-aa.txt"]);
        let mut rule = RenameRule {
            find: "aa".into(),
            replace: "x".into(),
            ..Default::default()
        };
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "Aa-aA-x.txt");
        rule.ignore_case = true;
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "x-x-x.txt");
        rule.first_only = true;
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "x-aA-aa.txt");
        rule.ignore_case = false;
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "Aa-aA-x.txt");
        rule.ignore_case = true;
        rule.replace = "$1${name}".into();
        assert_eq!(
            plan_files(&input, &existing, &rule)[0].to,
            "$1${name}-aA-aa.txt"
        );
        rule.first_only = false;
        assert_eq!(
            plan_files(&input, &existing, &rule)[0].to,
            "$1${name}-$1${name}-$1${name}.txt"
        );
        rule.regex = true;
        rule.first_only = true;
        rule.find = "(aa)".into();
        rule.replace = "$1_$1".into();
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "Aa-aA-aa.txt");
        // Explicit braces disambiguate a capture reference from a longer name.
        rule.replace = "${1}_${1}".into();
        assert_eq!(
            plan_files(&input, &existing, &rule)[0].to,
            "Aa_Aa-aA-aa.txt"
        );
    }

    #[test]
    fn matching_options_use_unicode_simple_folding_and_respect_component_scope() {
        let input = names(&["Ёж-ёж.ЁЖ"]);
        let existing = existing(&["Ёж-ёж.ЁЖ"]);
        let mut rule = RenameRule {
            find: "ёж".into(),
            replace: "ёжик".into(),
            ignore_case: true,
            first_only: true,
            ..Default::default()
        };
        rule.replace_target = ReplaceTarget::Extension;
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "Ёж-ёж.ёжик");
        rule.replace_target = ReplaceTarget::Stem;
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "ёжик-ёж.ЁЖ");
        rule.replace_target = ReplaceTarget::WholeName;
        rule.first_only = false;
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "ёжик-ёжик.ёжик");
        let input = names(&["Straße.txt"]);
        rule.find = "STRASSE".into();
        rule.replace = "Road".into();
        assert_eq!(
            plan_files(&input, &HashSet::new(), &rule)[0].to,
            "Straße.txt"
        );
    }

    #[test]
    fn first_only_handles_zero_width_regex_but_empty_find_keeps_legacy_noop() {
        let input = names(&["ёж.txt"]);
        let existing = existing(&["ёж.txt"]);
        let mut rule = RenameRule {
            find: "(?:)".into(),
            replace: "-".into(),
            regex: true,
            replace_target: ReplaceTarget::Stem,
            first_only: true,
            ..Default::default()
        };
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "-ёж.txt");
        rule.first_only = false;
        assert_eq!(plan_files(&input, &existing, &rule)[0].to, "-ё-ж-.txt");
        rule.find.clear();
        rule.first_only = true;
        rule.ignore_case = true;
        assert_eq!(
            plan_files(&input, &existing, &rule)[0].status,
            PlanStatus::Unchanged
        );
        let old: RenameRule = serde_json::from_str(r#"{"find":"a","replace":"b"}"#).unwrap();
        assert!(!old.ignore_case && !old.first_only);
        let encoded = serde_json::to_value(old).unwrap();
        assert!(encoded.get("ignore_case").is_none() && encoded.get("first_only").is_none());
    }

    #[test]
    fn replacement_target_distinguishes_whole_stem_and_extension() {
        let input = names(&["txt-report.txt"]);
        let existing = existing(&["txt-report.txt"]);
        let mut rule = RenameRule {
            find: "txt".into(),
            replace: "log".into(),
            ..Default::default()
        };
        for (target, expected) in [
            (ReplaceTarget::WholeName, "log-report.log"),
            (ReplaceTarget::Stem, "log-report.txt"),
            (ReplaceTarget::Extension, "txt-report.log"),
        ] {
            rule.replace_target = target;
            let plan = plan_files(&input, &existing, &rule);
            assert_eq!(plan[0].to, expected);
            assert_eq!(plan[0].status, PlanStatus::Ok);
        }
        rule.replace_target = ReplaceTarget::Stem;
        rule.replace = "log.md".into();
        assert_eq!(
            plan_files(&input, &existing, &rule)[0].to,
            "log.md-report.txt"
        );
    }

    #[test]
    fn extension_target_supports_removal_creation_and_identity_without_affecting_folders() {
        let input = names(&[
            "report.txt",
            ".gitignore",
            "archive.tar.gz",
            "trailing.",
            "plain",
            "folder.v1",
        ]);
        let existing: HashSet<_> = input.iter().cloned().collect();
        let dirs = names(&["folder.v1"]).into_iter().collect();
        let mut rule = RenameRule {
            replace_target: ReplaceTarget::Extension,
            ..Default::default()
        };
        let plans = plan_batch_rename(&input, &existing, &rule, &dirs);
        assert!(
            plans
                .iter()
                .all(|plan| plan.status == PlanStatus::Unchanged)
        );
        rule.find = "txt".into();
        rule.replace = String::new();
        assert_eq!(
            plan_batch_rename(&input, &existing, &rule, &dirs)[0].to,
            "report"
        );
        rule.find = "^$".into();
        rule.replace = "txt".into();
        rule.regex = true;
        let plans = plan_batch_rename(&input, &existing, &rule, &dirs);
        assert_eq!(plans[1].to, ".gitignore.txt");
        assert_eq!(plans[3].to, "trailing.txt");
        assert_eq!(plans[4].to, "plain.txt");
        assert_eq!(plans[5].to, "folder.v1");
        rule.find = "gz".into();
        rule.replace = "zip".into();
        rule.prefix = "x_".into();
        let plans = plan_batch_rename(&input, &existing, &rule, &dirs);
        assert_eq!(plans[2].to, "x_archive.tar.zip");
        assert_eq!(plans[5].to, "x_folder.v1");
    }

    #[test]
    fn targeted_regex_groups_structural_edits_and_collisions_use_the_same_planner() {
        let input = names(&["item12.JPG"]);
        let mut rule = RenameRule {
            find: r"item(\d+)".into(),
            replace: "photo-$1".into(),
            regex: true,
            replace_target: ReplaceTarget::Stem,
            suffix: "_end".into(),
            case: CaseMode::Upper,
            numbering: Some(Numbering {
                start: 1,
                step: 1,
                pad: 2,
            }),
            ..Default::default()
        };
        let plans = plan_files(&input, &existing(&["item12.JPG"]), &rule);
        assert_eq!(plans[0].to, "PHOTO-12_end01.JPG");
        rule = RenameRule {
            find: "JPG".into(),
            replace: "png".into(),
            replace_target: ReplaceTarget::Extension,
            ..Default::default()
        };
        assert_eq!(
            plan_files(&input, &existing(&["item12.JPG", "item12.png"]), &rule)[0].status,
            PlanStatus::Collision
        );
        rule.replace = "../png".into();
        assert_eq!(
            plan_files(&input, &existing(&["item12.JPG"]), &rule)[0].status,
            PlanStatus::Invalid
        );
    }

    #[test]
    fn missing_target_deserializes_to_legacy_whole_name_and_unknown_targets_fail() {
        let rule: RenameRule = serde_json::from_str(r#"{"find":"txt","replace":"log"}"#).unwrap();
        assert_eq!(rule.replace_target, ReplaceTarget::WholeName);
        assert!(
            serde_json::to_value(&rule)
                .unwrap()
                .get("replace_target")
                .is_none()
        );
        assert!(serde_json::from_str::<RenameRule>(r#"{"replace_target":"Unexpected"}"#).is_err());
    }

    #[test]
    fn folder_names_have_no_extension_to_preserve() {
        let rule = RenameRule {
            suffix: "_old".into(),
            case: CaseMode::Upper,
            ..Default::default()
        };
        let n = names(&["v1.2", "notes.txt"]);
        let ex = existing(&["v1.2", "notes.txt"]);
        let plans = plan_batch_rename(&n, &ex, &rule, &existing(&["v1.2"]));
        assert_eq!(plans[0].to, "V1.2_old");
        assert_eq!(plans[1].to, "NOTES_old.txt");
    }

    #[test]
    fn collision_policy_distinguishes_case_sensitive_directories() {
        let selected = names(&["bar"]);
        let siblings = existing(&["Foo", "bar"]);
        let rule = RenameRule {
            find: "bar".into(),
            replace: "foo".into(),
            ..Default::default()
        };

        let folded = plan_batch_rename_with(
            &selected,
            &siblings,
            &rule,
            &HashSet::new(),
            crate::rename_order::NameFolding::CaseFolded,
        );
        let exact = plan_batch_rename_with(
            &selected,
            &siblings,
            &rule,
            &HashSet::new(),
            crate::rename_order::NameFolding::Exact,
        );

        assert_eq!(folded[0].status, PlanStatus::Collision);
        assert_eq!(exact[0].status, PlanStatus::Ok);
    }

    #[test]
    fn find_replace_touches_whole_name_then_splits_extension() {
        let rule = RenameRule {
            find: "IMG".into(),
            replace: "Photo".into(),
            ..Default::default()
        };
        let n = names(&["IMG_01.jpg", "IMG_02.jpg"]);
        let ex = existing(&["IMG_01.jpg", "IMG_02.jpg"]);
        let plans = plan_files(&n, &ex, &rule);
        assert_eq!(plans[0].to, "Photo_01.jpg");
        assert_eq!(plans[1].to, "Photo_02.jpg");
        assert_eq!(plans[0].status, PlanStatus::Ok);
    }

    #[test]
    fn regex_find_replace_expands_capture_groups() {
        let rule = RenameRule {
            find: r"IMG_(\d+)".into(),
            replace: "Photo-$1".into(),
            regex: true,
            ..Default::default()
        };
        let n = names(&["IMG_042.jpg", "IMG_007.jpg"]);
        let ex = existing(&["IMG_042.jpg", "IMG_007.jpg"]);
        let plans = plan_files(&n, &ex, &rule);
        assert_eq!(plans[0].to, "Photo-042.jpg");
        assert_eq!(plans[1].to, "Photo-007.jpg");
    }

    #[test]
    fn regex_error_reports_an_invalid_pattern() {
        let rule = RenameRule {
            find: "(unclosed".into(),
            regex: true,
            ..Default::default()
        };
        assert!(regex_error(&rule).is_some());
    }

    #[test]
    fn regex_error_is_none_outside_regex_mode_or_with_an_empty_pattern() {
        assert_eq!(regex_error(&RenameRule::default()), None);
        let literal = RenameRule {
            find: "(unclosed".into(), // invalid as regex, fine as a literal
            ..Default::default()
        };
        assert_eq!(regex_error(&literal), None);
        let empty_pattern = RenameRule {
            regex: true,
            ..Default::default()
        };
        assert_eq!(regex_error(&empty_pattern), None);
    }

    #[test]
    fn invalid_regex_pattern_leaves_names_unchanged_instead_of_panicking() {
        let rule = RenameRule {
            find: "(unclosed".into(),
            replace: "x".into(),
            regex: true,
            ..Default::default()
        };
        let plans = plan_files(&names(&["a.txt"]), &existing(&["a.txt"]), &rule);
        assert_eq!(plans[0].to, "a.txt");
        assert_eq!(plans[0].status, PlanStatus::Unchanged);
    }

    #[test]
    fn prefix_suffix_and_case_apply_to_the_stem_only() {
        let rule = RenameRule {
            prefix: "v_".into(),
            suffix: "_final".into(),
            case: CaseMode::Upper,
            ..Default::default()
        };
        let n = names(&["report.txt"]);
        let plans = plan_files(&n, &existing(&["report.txt"]), &rule);
        // stem REPORT -> v_REPORT_final, extension untouched.
        assert_eq!(plans[0].to, "v_REPORT_final.txt");
    }

    #[test]
    fn numbering_uses_input_order_with_padding() {
        let rule = RenameRule {
            prefix: "shot_".into(),
            numbering: Some(Numbering {
                start: 5,
                step: 5,
                pad: 3,
            }),
            ..Default::default()
        };
        // Counter follows input order; it is appended to the stem, ahead of
        // the extension. The third name has no extension.
        let n = names(&["a.png", "b.png", "c"]);
        let plans = plan_files(&n, &existing(&[]), &rule);
        assert_eq!(plans[0].to, "shot_a005.png");
        assert_eq!(plans[1].to, "shot_b010.png");
        assert_eq!(plans[2].to, "shot_c015");
    }

    #[test]
    fn dotfiles_keep_their_leading_dot_as_stem() {
        let rule = RenameRule {
            prefix: "x".into(),
            ..Default::default()
        };
        let plans = plan_files(&names(&[".gitignore"]), &existing(&[".gitignore"]), &rule);
        assert_eq!(plans[0].to, "x.gitignore");
    }

    #[test]
    fn unchanged_when_rule_is_a_noop_for_the_name() {
        let rule = RenameRule {
            find: "zzz".into(),
            replace: "q".into(),
            ..Default::default()
        };
        let plans = plan_files(&names(&["a.txt"]), &existing(&["a.txt"]), &rule);
        assert_eq!(plans[0].status, PlanStatus::Unchanged);
        assert_eq!(plans[0].to, "a.txt");
    }

    #[test]
    fn two_entries_to_same_target_are_collisions() {
        // Both lose their unique digits, colliding on "img.jpg".
        let rule = RenameRule {
            find: "1".into(),
            replace: "".into(),
            ..Default::default()
        };
        let n = names(&["img1.jpg", "img11.jpg"]);
        let plans = plan_files(
            &n,
            &existing(&n.iter().map(String::as_str).collect::<Vec<_>>()),
            &rule,
        );
        assert_eq!(plans[0].to, "img.jpg");
        assert_eq!(plans[1].to, "img.jpg");
        assert_eq!(plans[0].status, PlanStatus::Collision);
        assert_eq!(plans[1].status, PlanStatus::Collision);
        assert!(!plan_is_applicable(&plans));
    }

    #[test]
    fn collision_with_an_untouched_sibling_is_flagged() {
        let rule = RenameRule {
            find: "draft".into(),
            replace: "final".into(),
            ..Default::default()
        };
        // "final.txt" already exists and is NOT in the rename batch.
        let n = names(&["draft.txt"]);
        let ex = existing(&["draft.txt", "final.txt"]);
        let plans = plan_files(&n, &ex, &rule);
        assert_eq!(plans[0].status, PlanStatus::Collision);
    }

    #[test]
    fn vacate_guard_suppresses_sibling_check_not_duplicate_check() {
        // "a.txt" -> "b.txt"; the existing "b.txt" is a collision only when it
        // is NOT part of the batch. When "b.txt" is also selected (and so will
        // be vacated), the guard lets the rename through.
        let rule = RenameRule {
            find: "a".into(),
            replace: "b".into(),
            ..Default::default()
        };
        let ex = existing(&["a.txt", "b.txt"]);
        // b.txt not in batch -> collision with an untouched sibling.
        assert_eq!(
            plan_files(&names(&["a.txt"]), &ex, &rule)[0].status,
            PlanStatus::Collision
        );
        // b.txt also in the batch -> a.txt's target is no longer a sibling
        // clash. (b.txt has no 'a', so it stays put and is Unchanged.)
        let plans = plan_files(&names(&["a.txt", "b.txt"]), &ex, &rule);
        assert_eq!(plans[0].to, "b.txt");
        assert_eq!(plans[1].status, PlanStatus::Unchanged);
        // With b.txt staying, both want "b.txt" -> the duplicate-target rule
        // still flags a.txt. The vacate guard only suppresses the *sibling*
        // check, not genuine duplicates.
        assert_eq!(plans[0].status, PlanStatus::Collision);
    }

    #[test]
    fn invalid_target_names_are_flagged() {
        let rule = RenameRule {
            find: "a".into(),
            replace: "/".into(),
            ..Default::default()
        };
        let plans = plan_files(&names(&["a"]), &existing(&["a"]), &rule);
        assert_eq!(plans[0].status, PlanStatus::Invalid);
        assert!(!plan_is_applicable(&plans));
    }

    #[test]
    fn nul_and_overlong_target_names_are_flagged() {
        let nul = RenameRule {
            find: "a".into(),
            replace: "\0".into(),
            ..Default::default()
        };
        let plans = plan_files(&names(&["a"]), &existing(&["a"]), &nul);
        assert_eq!(plans[0].status, PlanStatus::Invalid);

        let overlong = RenameRule {
            prefix: "x".repeat(255),
            ..Default::default()
        };
        let plans = plan_files(&names(&["a"]), &existing(&["a"]), &overlong);
        assert_eq!(plans[0].status, PlanStatus::Invalid);
        assert!(!plan_is_applicable(&plans));

        let at_limit = RenameRule {
            prefix: "x".repeat(254),
            ..Default::default()
        };
        let plans = plan_files(&names(&["a"]), &existing(&["a"]), &at_limit);
        assert_eq!(plans[0].status, PlanStatus::Ok);
    }

    #[test]
    fn normalization_equivalent_sibling_is_a_collision() {
        // A decomposed "café.txt" (e + U+0301) already exists untouched; a
        // rename producing the precomposed form would land on it on APFS.
        let decomposed = "cafe\u{301}.txt";
        let rule = RenameRule {
            find: "Q".into(),
            replace: "\u{e9}".into(),
            ..Default::default()
        };
        let plans = plan_files(
            &names(&["cafQ.txt"]),
            &existing(&["cafQ.txt", decomposed]),
            &rule,
        );
        assert_eq!(plans[0].to, "caf\u{e9}.txt");
        assert_eq!(plans[0].status, PlanStatus::Collision);
    }

    #[test]
    fn case_only_rename_of_the_entry_itself_is_ok() {
        // `foo.txt` matches `Foo.txt` under the case-insensitive key, but the
        // clash is the entry being renamed away, not an untouched sibling.
        let rule = RenameRule {
            case: CaseMode::Lower,
            ..Default::default()
        };
        let plans = plan_files(&names(&["Foo.txt"]), &existing(&["Foo.txt"]), &rule);
        assert_eq!(plans[0].to, "foo.txt");
        assert_eq!(plans[0].status, PlanStatus::Ok);
        assert!(plan_is_applicable(&plans));
    }

    #[test]
    fn applicable_requires_a_change_and_no_problems() {
        // All unchanged -> not applicable (nothing to do).
        let noop = plan_files(
            &names(&["a.txt"]),
            &existing(&["a.txt"]),
            &RenameRule::default(),
        );
        assert!(!plan_is_applicable(&noop));
        // One clean change -> applicable.
        let rule = RenameRule {
            prefix: "p_".into(),
            ..Default::default()
        };
        let ok = plan_files(&names(&["a.txt"]), &existing(&["a.txt"]), &rule);
        assert!(plan_is_applicable(&ok));
        assert_eq!(ok.iter().filter(|p| p.status == PlanStatus::Ok).count(), 1);
    }
}
