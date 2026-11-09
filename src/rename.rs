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
    /// Optional ordered complete-name seeds, one per LF/CRLF record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_list: Option<String>,
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
    /// In regex mode, insert replacement text literally instead of expanding captures.
    #[serde(skip_serializing_if = "is_false")]
    pub literal_replacement: bool,
    /// Trim Unicode whitespace from the final complete name.
    #[serde(skip_serializing_if = "is_false")]
    pub trim_spaces: bool,
    /// Replace slash/NUL in the final target; empty text removes them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_char_replacement: Option<String>,
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

pub const MAX_NAME_LIST_BYTES: usize = 256 * 1024;
pub fn parse_name_list(input: &str) -> Result<Vec<&str>, String> {
    if input.len() > MAX_NAME_LIST_BYTES {
        return Err("Name lists are limited to 256 KiB".into());
    }
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut rows = Vec::new();
    for line in input.split_inclusive('\n') {
        let name = if let Some(line) = line.strip_suffix('\n') {
            line.strip_suffix('\r').unwrap_or(line)
        } else {
            line
        };
        if name.is_empty() {
            return Err(format!("Name list row {} is empty", rows.len() + 1));
        }
        if rows.len() == 10000 {
            return Err("Name lists are limited to 10000 rows".into());
        }
        rows.push(name);
    }
    if rows.is_empty() {
        return Err("Enter one complete filename per row".into());
    }
    Ok(rows)
}
/// Build a list only when every name survives the plain-record format exactly.
pub fn name_list_text(names: &[String]) -> Result<String, String> {
    if names.is_empty() || names.len() > 10000 {
        return Err("Name lists need 1–10000 rows".into());
    }
    let mut bytes = names.len() - 1;
    for name in names {
        if name.contains(['\r', '\n']) {
            return Err("Names with line breaks cannot be represented in a plain name list".into());
        }
        bytes = bytes
            .checked_add(name.len())
            .filter(|&n| n <= MAX_NAME_LIST_BYTES)
            .ok_or("Name lists are limited to 256 KiB")?;
    }
    let text = names.join("\n");
    let parsed = parse_name_list(&text)?;
    if !parsed.into_iter().eq(names.iter().map(String::as_str)) {
        return Err("Names cannot be represented losslessly in a plain name list".into());
    }
    Ok(text)
}

pub fn name_list_error(rule: &RenameRule, count: usize) -> Option<String> {
    let input = rule.name_list.as_deref()?;
    match parse_name_list(input) {
        Err(error) => Some(error),
        Ok(rows) if rows.len() != count => Some(format!(
            "Name list has {} rows; this batch needs {count}",
            rows.len()
        )),
        _ => None,
    }
}

fn invalid_replacement_error(rule: &RenameRule) -> Option<&'static str> {
    rule.invalid_char_replacement.as_ref().and_then(|text| {
        (text.len() > 32 || text.contains(['/', '\0']))
            .then_some("Invalid-character replacement needs at most 32 bytes without slash or NUL")
    })
}
fn compile_find(rule: &RenameRule) -> Result<Vec<Option<regex::Regex>>, String> {
    if let Some(input) = rule.name_list.as_deref() {
        parse_name_list(input)?;
    }
    if let Some(error) = invalid_replacement_error(rule) {
        return Err(error.into());
    }
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
    let list_error = name_list_error(rule, names.len()).is_some();
    let listed = rule
        .name_list
        .as_deref()
        .and_then(|text| parse_name_list(text).ok());
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
            let seed = listed
                .as_ref()
                .and_then(|rows| rows.get(index))
                .copied()
                .unwrap_or(name);
            match rename_one_checked(
                seed,
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
            let status = if list_error || invalid_replacement_error(rule).is_some() {
                PlanStatus::Invalid
            } else if folder_unavailable {
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
mod tests;
