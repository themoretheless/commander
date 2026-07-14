//! Canonical search query model and grammar.
//!
//! Parsing, validation and metadata matching are UI-independent. Recursive I/O
//! and content inspection live in `crate::search`, while panel filters and saved
//! searches share this representation.

use crate::panel::FileEntry;
use crate::selection_summary::{Kind, kind_of};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

const SECONDS_PER_DAY: u64 = 86_400;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum MatchMode {
    #[default]
    Exact,
    Fuzzy,
    Regex,
}

impl MatchMode {
    pub const ALL: [Self; 3] = [Self::Exact, Self::Fuzzy, Self::Regex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Exact => "Exact",
            Self::Fuzzy => "Fuzzy",
            Self::Regex => "Regex",
        }
    }
}

/// One condition a found entry must satisfy. Text predicates use the query's
/// [`MatchMode`]. Content is deliberately deferred to the search provider.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Predicate {
    NameContains(String),
    PathContains(String),
    ContentContains(String),
    Kind(Kind),
    MinSize(u64),
    MaxSize(u64),
    MaxAgeDays(u64),
    MinAgeDays(u64),
}

impl Predicate {
    fn matches_metadata(&self, e: &FileEntry, now: SystemTime, mode: MatchMode) -> bool {
        match self {
            Predicate::NameContains(value) => text_matches(mode, value, &e.name),
            Predicate::PathContains(value) => text_matches(mode, value, &e.path.to_string_lossy()),
            Predicate::ContentContains(_) => true,
            Predicate::Kind(kind) => kind_of(e) == *kind,
            Predicate::MinSize(min) => e.size >= *min,
            Predicate::MaxSize(max) => e.size <= *max,
            Predicate::MaxAgeDays(days) => age_days(e.modified, now).is_some_and(|d| d <= *days),
            Predicate::MinAgeDays(days) => age_days(e.modified, now).is_some_and(|d| d >= *days),
        }
    }

    pub fn chip(&self) -> String {
        match self {
            Predicate::NameContains(value) => format!("name:{value}"),
            Predicate::PathContains(value) => format!("path:{value}"),
            Predicate::ContentContains(value) => format!("content:{value}"),
            Predicate::Kind(kind) => format!("type:{}", kind.label()),
            Predicate::MinSize(bytes) => format!("size:>={}", format_bytes(*bytes)),
            Predicate::MaxSize(bytes) => format!("size:<={}", format_bytes(*bytes)),
            Predicate::MaxAgeDays(days) => format!("date:<={days}d"),
            Predicate::MinAgeDays(days) => format!("date:>={days}d"),
        }
    }

    fn expression(&self) -> String {
        match self {
            Predicate::NameContains(value) => quote_if_needed(value),
            Predicate::PathContains(value) => format!("path:{}", quote_if_needed(value)),
            Predicate::ContentContains(value) => format!("content:{}", quote_if_needed(value)),
            Predicate::Kind(kind) => format!("type:{}", kind.label()),
            Predicate::MinSize(bytes) => format!("size:>={}", format_bytes(*bytes)),
            Predicate::MaxSize(bytes) => format!("size:<={}", format_bytes(*bytes)),
            Predicate::MaxAgeDays(days) => format!("date:<={days}d"),
            Predicate::MinAgeDays(days) => format!("date:>={days}d"),
        }
    }
}

fn age_days(modified: Option<SystemTime>, now: SystemTime) -> Option<u64> {
    now.duration_since(modified?)
        .ok()
        .map(|d| d.as_secs() / SECONDS_PER_DAY)
}

fn text_matches(mode: MatchMode, needle: &str, candidate: &str) -> bool {
    match mode {
        MatchMode::Exact => candidate.to_lowercase().contains(&needle.to_lowercase()),
        MatchMode::Fuzzy => crate::fuzzy::is_match(needle, candidate),
        MatchMode::Regex => regex::RegexBuilder::new(needle)
            .case_insensitive(true)
            .build()
            .is_ok_and(|regex| regex.is_match(candidate)),
    }
}

/// A conjunction (AND) of predicates. An empty query matches everything.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Query {
    pub predicates: Vec<Predicate>,
    #[serde(default)]
    pub mode: MatchMode,
}

impl Query {
    pub fn parse(expression: &str, mode: MatchMode) -> Result<Self, QueryError> {
        let mut predicates = Vec::new();
        for token in tokenize(expression)? {
            let Some((field, value)) = token.split_once(':') else {
                predicates.push(Predicate::NameContains(token));
                continue;
            };
            if value.is_empty() {
                return Err(QueryError::new(token, "field value is empty"));
            }
            match field.to_ascii_lowercase().as_str() {
                "name" => predicates.push(Predicate::NameContains(value.to_string())),
                "path" => predicates.push(Predicate::PathContains(value.to_string())),
                "content" => predicates.push(Predicate::ContentContains(value.to_string())),
                "type" => predicates
                    .push(Predicate::Kind(parse_kind(value).ok_or_else(|| {
                        QueryError::new(token.clone(), "unknown type")
                    })?)),
                "size" => predicates.extend(
                    parse_size(value).map_err(|message| QueryError::new(token.clone(), message))?,
                ),
                "date" => predicates.push(
                    parse_date(value).map_err(|message| QueryError::new(token.clone(), message))?,
                ),
                _ => return Err(QueryError::new(token, "unknown query field")),
            }
        }
        let query = Query { predicates, mode };
        query.validate()?;
        Ok(query)
    }

    pub fn validate(&self) -> Result<(), QueryError> {
        if self.mode == MatchMode::Regex {
            for predicate in &self.predicates {
                let value = match predicate {
                    Predicate::NameContains(value)
                    | Predicate::PathContains(value)
                    | Predicate::ContentContains(value) => value,
                    _ => continue,
                };
                regex::RegexBuilder::new(value)
                    .case_insensitive(true)
                    .build()
                    .map_err(|error| QueryError::new(value.clone(), error.to_string()))?;
            }
        }
        Ok(())
    }

    /// Metadata-only matching. Content predicates are deferred and therefore
    /// pass this stage; the search provider evaluates them before emitting a
    /// hit.
    pub fn matches_metadata(&self, e: &FileEntry, now: SystemTime) -> bool {
        self.predicates
            .iter()
            .all(|predicate| predicate.matches_metadata(e, now, self.mode))
    }

    /// Match only queries that require no file reads. Kept for lightweight
    /// panel filters and direct unit tests.
    #[cfg(test)]
    pub fn matches(&self, e: &FileEntry, now: SystemTime) -> bool {
        !self.requires_content() && self.matches_metadata(e, now)
    }

    pub fn requires_content(&self) -> bool {
        self.predicates
            .iter()
            .any(|predicate| matches!(predicate, Predicate::ContentContains(_)))
    }

    pub fn chips(&self) -> Vec<String> {
        self.predicates.iter().map(Predicate::chip).collect()
    }

    pub fn to_expression(&self) -> String {
        self.predicates
            .iter()
            .map(Predicate::expression)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QueryError {
    pub token: String,
    pub message: String,
}

impl QueryError {
    fn new(token: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.token, self.message)
    }
}

fn tokenize(expression: &str) -> Result<Vec<String>, QueryError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = expression.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let escapable = chars
                .peek()
                .is_some_and(|next| next.is_whitespace() || *next == '\\' || Some(*next) == quote);
            if escapable {
                current.push(chars.next().unwrap());
            } else {
                // Preserve regex and path escapes such as `\.` and `\d`.
                current.push(ch);
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if let Some(delimiter) = quote {
        return Err(QueryError::new(delimiter.to_string(), "unterminated quote"));
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

fn parse_kind(value: &str) -> Option<Kind> {
    match value.to_ascii_lowercase().as_str() {
        "folder" | "folders" | "dir" | "directory" => Some(Kind::Folder),
        "image" | "images" | "photo" | "photos" => Some(Kind::Image),
        "video" | "videos" => Some(Kind::Video),
        "audio" | "music" => Some(Kind::Audio),
        "document" | "documents" | "doc" | "docs" | "pdf" => Some(Kind::Document),
        "code" | "source" => Some(Kind::Code),
        "archive" | "archives" => Some(Kind::Archive),
        "other" => Some(Kind::Other),
        _ => None,
    }
}

fn parse_size(value: &str) -> Result<Vec<Predicate>, &'static str> {
    if let Some((min, max)) = value.split_once("..") {
        let min = parse_bytes(min)?;
        let max = parse_bytes(max)?;
        if min > max {
            return Err("size range is reversed");
        }
        return Ok(vec![Predicate::MinSize(min), Predicate::MaxSize(max)]);
    }
    let (operator, number) = split_operator(value);
    let bytes = parse_bytes(number)?;
    match operator {
        "<" | "<=" => Ok(vec![Predicate::MaxSize(bytes)]),
        ">" | ">=" | "" => Ok(vec![Predicate::MinSize(bytes)]),
        _ => Err("invalid size operator"),
    }
}

fn parse_bytes(value: &str) -> Result<u64, &'static str> {
    let upper = value.trim().to_ascii_uppercase();
    let split = upper
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .unwrap_or(upper.len());
    let number = upper[..split].parse::<f64>().map_err(|_| "invalid size")?;
    if !number.is_finite() || number < 0.0 {
        return Err("invalid size");
    }
    let multiplier = match upper[split..].trim() {
        "" | "B" => 1.0,
        "K" | "KB" | "KIB" => 1024.0,
        "M" | "MB" | "MIB" => 1024.0 * 1024.0,
        "G" | "GB" | "GIB" => 1024.0 * 1024.0 * 1024.0,
        "T" | "TB" | "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err("unknown size unit"),
    };
    let bytes = number * multiplier;
    if bytes > u64::MAX as f64 {
        return Err("size is too large");
    }
    Ok(bytes.round() as u64)
}

fn parse_date(value: &str) -> Result<Predicate, &'static str> {
    let (operator, amount) = split_operator(value);
    let lower = amount.trim().to_ascii_lowercase();
    let split = lower
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(lower.len());
    let number = lower[..split]
        .parse::<u64>()
        .map_err(|_| "invalid date age")?;
    let multiplier = match lower[split..].trim() {
        "" | "d" | "day" | "days" => 1,
        "w" | "week" | "weeks" => 7,
        "m" | "month" | "months" => 30,
        "y" | "year" | "years" => 365,
        _ => return Err("unknown date unit"),
    };
    let days = number
        .checked_mul(multiplier)
        .ok_or("date age is too large")?;
    match operator {
        "<" | "<=" | "" => Ok(Predicate::MaxAgeDays(days)),
        ">" | ">=" => Ok(Predicate::MinAgeDays(days)),
        _ => Err("invalid date operator"),
    }
}

fn split_operator(value: &str) -> (&str, &str) {
    for operator in [">=", "<=", ">", "<"] {
        if let Some(rest) = value.strip_prefix(operator) {
            return (operator, rest);
        }
    }
    ("", value)
}

fn format_bytes(bytes: u64) -> String {
    for (unit, divisor) in [
        ("TB", 1_u64 << 40),
        ("GB", 1_u64 << 30),
        ("MB", 1_u64 << 20),
        ("KB", 1_u64 << 10),
    ] {
        if bytes >= divisor && bytes.is_multiple_of(divisor) {
            return format!("{}{unit}", bytes / divisor);
        }
    }
    format!("{bytes}B")
}

fn quote_if_needed(value: &str) -> String {
    if value.chars().all(|ch| !ch.is_whitespace() && ch != '"') {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Convert the active panel's lightweight filter controls into a reusable
/// smart-folder query.
pub fn from_panel_filter(search: &str, facets: &crate::panel::FacetSet) -> Query {
    let mut predicates = Vec::new();
    let trimmed = search.trim();
    if !trimmed.is_empty() {
        predicates.push(Predicate::NameContains(trimmed.to_string()));
    }
    if let Some(facet) = facets.kind {
        predicates.push(Predicate::Kind(kind_from_facet(facet)));
    }
    if let Some(min) = facets.min_size {
        predicates.push(Predicate::MinSize(min));
    }
    if let Some(days) = facets.max_age_days {
        predicates.push(Predicate::MaxAgeDays(days));
    }
    if let Some(days) = facets.min_age_days {
        predicates.push(Predicate::MinAgeDays(days));
    }
    Query {
        predicates,
        mode: MatchMode::Exact,
    }
}

fn kind_from_facet(facet: crate::panel::KindFacet) -> Kind {
    match facet {
        crate::panel::KindFacet::Folders => Kind::Folder,
        crate::panel::KindFacet::Images => Kind::Image,
        crate::panel::KindFacet::Docs => Kind::Document,
        crate::panel::KindFacet::Archives => Kind::Archive,
        crate::panel::KindFacet::Code => Kind::Code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn entry(name: &str, is_dir: bool, size: u64, modified: Option<SystemTime>) -> FileEntry {
        let ext = PathBuf::from(name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: PathBuf::from(format!("/x/{name}")),
            is_dir,
            size,
            extension: ext,
            modified,
            modified_str: "-".to_string(),
            size_str: crate::panel::format_size(size),
        }
    }

    fn q(predicates: Vec<Predicate>) -> Query {
        Query {
            predicates,
            mode: MatchMode::Exact,
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        let now = SystemTime::now();
        assert!(q(vec![]).matches(&entry("a.txt", false, 1, None), now));
    }

    #[test]
    fn metadata_predicates_are_anded() {
        let now = SystemTime::now();
        let query = q(vec![
            Predicate::NameContains("log".into()),
            Predicate::MinSize(500),
            Predicate::MaxSize(1000),
        ]);
        assert!(query.matches(&entry("server.log", false, 800, None), now));
        assert!(!query.matches(&entry("server.log", false, 100, None), now));
        assert!(!query.matches(&entry("readme.md", false, 800, None), now));
    }

    #[test]
    fn kind_and_age_predicates_use_entry_metadata() {
        let now = SystemTime::now();
        let recent = now - Duration::from_secs(2 * SECONDS_PER_DAY);
        let old = now - Duration::from_secs(40 * SECONDS_PER_DAY);
        assert!(
            q(vec![Predicate::Kind(Kind::Image)]).matches(&entry("a.png", false, 1, None), now)
        );
        assert!(
            q(vec![Predicate::MaxAgeDays(7)]).matches(&entry("a", false, 1, Some(recent)), now)
        );
        assert!(q(vec![Predicate::MinAgeDays(30)]).matches(&entry("a", false, 1, Some(old)), now));
        assert!(!q(vec![Predicate::MaxAgeDays(7)]).matches(&entry("a", false, 1, None), now));
    }

    #[test]
    fn grammar_parses_fields_quotes_ranges_and_units() {
        let query = Query::parse(
            "name:\"quarterly report\" path:Finance type:docs size:1MB..2GB date:<2w content:revenue",
            MatchMode::Exact,
        )
        .unwrap();
        assert_eq!(
            query.predicates,
            vec![
                Predicate::NameContains("quarterly report".into()),
                Predicate::PathContains("Finance".into()),
                Predicate::Kind(Kind::Document),
                Predicate::MinSize(1 << 20),
                Predicate::MaxSize(2_u64 << 30),
                Predicate::MaxAgeDays(14),
                Predicate::ContentContains("revenue".into()),
            ]
        );
        assert!(query.requires_content());
    }

    #[test]
    fn grammar_rejects_unknown_fields_and_invalid_regex() {
        assert!(Query::parse("owner:me", MatchMode::Exact).is_err());
        assert!(Query::parse("name:[", MatchMode::Regex).is_err());
        assert!(Query::parse("path:\"unfinished", MatchMode::Exact).is_err());
    }

    #[test]
    fn expression_round_trip_is_canonical() {
        let first = Query::parse(
            "\"annual report\" type:pdf size:>=4MB date:>30d",
            MatchMode::Fuzzy,
        )
        .unwrap();
        let expression = first.to_expression();
        let second = Query::parse(&expression, MatchMode::Fuzzy).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.chips().len(), 4);
    }

    #[test]
    fn content_queries_never_claim_a_metadata_only_match() {
        let now = SystemTime::now();
        let query = q(vec![Predicate::ContentContains("needle".into())]);
        let file = entry("a.txt", false, 1, None);
        assert!(query.matches_metadata(&file, now));
        assert!(!query.matches(&file, now));
    }

    #[test]
    fn fuzzy_and_regex_modes_apply_to_text_predicates() {
        let now = SystemTime::now();
        let file = entry("quarterly-report.txt", false, 1, None);
        let fuzzy = Query::parse("qrpt", MatchMode::Fuzzy).unwrap();
        let regex = Query::parse("^quarterly.*\\.txt$", MatchMode::Regex).unwrap();
        assert!(fuzzy.matches(&file, now));
        assert!(regex.matches(&file, now));
    }

    #[test]
    fn tokenizer_preserves_regex_escapes() {
        let query = Query::parse(r"^quarterly.*\.txt$", MatchMode::Regex).unwrap();
        assert_eq!(
            query.predicates,
            vec![Predicate::NameContains(r"^quarterly.*\.txt$".into())]
        );
        assert!(query.matches(
            &entry("quarterly-report.txt", false, 1, None),
            SystemTime::now()
        ));
        assert!(!query.matches(
            &entry("quarterly-reportXtxt", false, 1, None),
            SystemTime::now()
        ));
    }

    #[test]
    fn panel_filter_converts_to_smart_folder_query() {
        let facets = crate::panel::FacetSet {
            kind: Some(crate::panel::KindFacet::Images),
            min_size: Some(1 << 20),
            max_age_days: Some(7),
            min_age_days: Some(30),
        };
        let query = from_panel_filter(" raw ", &facets);
        assert_eq!(query.mode, MatchMode::Exact);
        assert_eq!(
            query.predicates,
            vec![
                Predicate::NameContains("raw".into()),
                Predicate::Kind(Kind::Image),
                Predicate::MinSize(1 << 20),
                Predicate::MaxAgeDays(7),
                Predicate::MinAgeDays(30),
            ]
        );
    }
}
