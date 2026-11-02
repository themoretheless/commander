//! Canonical search query model and grammar.
//!
//! Parsing, validation and metadata matching are UI-independent. Recursive I/O
//! and content inspection live in `crate::search`, while panel filters and saved
//! searches share this representation.

use crate::panel::FileEntry;
use crate::selection_summary::{Kind, kind_of};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::time::SystemTime;

const SECONDS_PER_DAY: u64 = 86_400;
const MAX_ALTERNATIVE_TERMS: usize = 64;
const MAX_ALTERNATIVE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum MatchMode {
    #[default]
    Exact,
    Fuzzy,
    Regex,
}

impl MatchMode {
    pub const ALL: [Self; 3] = [Self::Exact, Self::Fuzzy, Self::Regex];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Exact => "Exact",
            Self::Fuzzy => "Fuzzy",
            Self::Regex => "Regex",
        }
    }
}

/// Exact elapsed-time comparison for the `age:` field.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum AgeComparison {
    Less,
    AtMost,
    Equal,
    AtLeast,
    Greater,
}

impl AgeComparison {
    fn operator(self) -> &'static str {
        match self {
            Self::Less => "<",
            Self::AtMost => "<=",
            Self::Equal => "=",
            Self::AtLeast => ">=",
            Self::Greater => ">",
        }
    }
    fn matches(self, age: std::time::Duration, bound: std::time::Duration) -> bool {
        match self {
            Self::Less => age < bound,
            Self::AtMost => age <= bound,
            Self::Equal => age == bound,
            Self::AtLeast => age >= bound,
            Self::Greater => age > bound,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum HiddenMode {
    #[default]
    Include,
    Exclude,
    Only,
}
impl HiddenMode {
    fn label(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
            Self::Only => "only",
        }
    }
}

/// Lexical Unix kinds, independent of target-following or content reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecialFileType {
    Fifo,
    Socket,
    BlockDevice,
    CharDevice,
}

impl SpecialFileType {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fifo => "fifo",
            Self::Socket => "socket",
            Self::BlockDevice => "block-device",
            Self::CharDevice => "char-device",
        }
    }
    pub const fn bits(self) -> u32 {
        match self {
            Self::Fifo => 0o010000,
            Self::Socket => 0o140000,
            Self::BlockDevice => 0o060000,
            Self::CharDevice => 0o020000,
        }
    }
}

/// Native lexical ownership, without a user/group database lookup or further I/O.
pub fn native_uid(metadata: &std::fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(u64::from(metadata.uid()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}
pub fn native_gid(metadata: &std::fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(u64::from(metadata.gid()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

fn parse_numeric_id(value: &str, field: &str) -> Result<(u64, bool), QueryError> {
    let (exclude, digits) = value
        .strip_prefix('!')
        .map_or((false, value), |digits| (true, digits));
    if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(QueryError::new(
            value,
            format!("{field} expects a decimal numeric ID, optionally prefixed by !"),
        ));
    }
    let id = digits.parse::<u64>().map_err(|_| {
        QueryError::new(
            value,
            format!("{field} ID exceeds the unsigned 64-bit range"),
        )
    })?;
    Ok((id, exclude))
}

pub fn native_file_type_bits(metadata: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.mode() & 0o170000)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// A non-recursive type condition for an explicit OR list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeCondition {
    File,
    Empty,
    Regular,
    Symlink,
    BrokenSymlink,
    Executable,
    Special(SpecialFileType),
    Kind(Kind),
}

impl TypeCondition {
    fn parse(value: &str) -> Option<Self> {
        Some(match value.to_ascii_lowercase().as_str() {
            "file" | "files" => Self::File,
            "empty" => Self::Empty,
            "regular" | "regular-file" => Self::Regular,
            "symlink" | "link" => Self::Symlink,
            "broken-link" | "broken-symlink" => Self::BrokenSymlink,
            "executable" | "exec" => Self::Executable,
            "fifo" | "pipe" => Self::Special(SpecialFileType::Fifo),
            "socket" => Self::Special(SpecialFileType::Socket),
            "block" | "block-device" => Self::Special(SpecialFileType::BlockDevice),
            "char" | "char-device" => Self::Special(SpecialFileType::CharDevice),
            _ => Self::Kind(parse_kind(value)?),
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Empty => "empty",
            Self::Regular => "regular",
            Self::Symlink => "symlink",
            Self::BrokenSymlink => "broken-link",
            Self::Executable => "executable",
            Self::Special(kind) => kind.label(),
            Self::Kind(kind) => kind.label(),
        }
    }
    fn predicate(self) -> Predicate {
        match self {
            Self::File => Predicate::FilesOnly,
            Self::Empty => Predicate::Empty,
            Self::Regular => Predicate::RegularFiles,
            Self::Symlink => Predicate::SymlinksOnly,
            Self::BrokenSymlink => Predicate::BrokenSymlinks,
            Self::Executable => Predicate::ExecutableFiles,
            Self::Special(kind) => Predicate::SpecialFileType(kind),
            Self::Kind(kind) => Predicate::Kind(kind),
        }
    }
}

/// One condition a found entry must satisfy. Ordinary text predicates use the
/// query's [`MatchMode`]; `NameFilter` retains file-pane matching semantics.
/// Content is deliberately deferred to the search provider.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Predicate {
    NameContains(String),
    NameExact(String),
    NameNotExact(String),
    NameAny(Vec<String>),
    NameExcludes(String),
    NameGlob(String),
    ExcludeGlob(String),
    ExcludeFileGlob(String),
    ExcludeDirGlob(String),
    /// Preserve file-pane fuzzy/literal/anchor semantics independently of `MatchMode`.
    NameFilter(String),
    Extension(String),
    ExtensionsAny(Vec<String>),
    PathContains(String),
    PathExcludes(String),
    /// Exact lexical immediate parent; independent of text matching mode.
    Parent(String),
    ContentContains(String),
    ContentAny(Vec<String>),
    ContentExcludes(String),
    ContentWord(String),
    ContentWordExcludes(String),
    ContentLine(String),
    ContentLineExcludes(String),
    SearchArchives(bool),
    Hidden(HiddenMode),
    CaseSensitive(bool),
    SmartCase,
    TypesAny(Vec<TypeCondition>),
    Empty,
    FilesOnly,
    RegularFiles,
    SpecialFileType(SpecialFileType),
    SymlinksOnly,
    BrokenSymlinks,
    ExecutableFiles,
    UnixPermissions(String),
    OwnerId {
        id: u64,
        exclude: bool,
    },
    GroupId {
        id: u64,
        exclude: bool,
    },
    DeviceId {
        id: u64,
        exclude: bool,
    },
    Inode {
        id: u64,
        exclude: bool,
    },
    Kind(Kind),
    MinDepth(usize),
    MaxDepth(usize),
    MinSize(u64),
    MinAllocated(u64),
    MinLinks(u64),
    MaxLinks(u64),
    MaxAllocated(u64),
    MaxSize(u64),
    ModifiedAfter(String),
    ModifiedBefore(String),
    AccessedAfter(String),
    AccessedBefore(String),
    CreatedAfter(String),
    CreatedBefore(String),
    ChangedAfter(String),
    ChangedBefore(String),
    AgeSeconds {
        comparison: AgeComparison,
        seconds: u64,
    },
    MaxAgeDays(u64),
    MinAgeDays(u64),
}

impl Predicate {
    /// Whether `e` satisfies this predicate's metadata condition (type, size,
    /// age). Text predicates (name, path, content) are not metadata: they
    /// are evaluated by the search provider's compiled matchers, so they pass here.
    pub(crate) fn matches_metadata(&self, e: &FileEntry, now: SystemTime) -> bool {
        match self {
            Self::NameExact(_)
            | Self::NameNotExact(_)
            | Self::NameAny(_)
            | Self::NameContains(_)
            | Self::NameExcludes(_)
            | Self::PathExcludes(_)
            | Self::NameGlob(_)
            | Self::ExcludeGlob(_)
            | Self::ExcludeFileGlob(_)
            | Self::ExcludeDirGlob(_)
            | Self::NameFilter(_)
            | Self::PathContains(_)
            | Self::ContentContains(_)
            | Self::ContentAny(_)
            | Self::ContentExcludes(_)
            | Self::ContentWord(_)
            | Self::ContentWordExcludes(_)
            | Self::ContentLine(_)
            | Self::ContentLineExcludes(_)
            | Self::SearchArchives(_)
            | Self::Hidden(_)
            | Self::CaseSensitive(_)
            | Self::SmartCase
            | Self::UnixPermissions(_)
            | Self::OwnerId { .. }
            | Self::GroupId { .. } => true,
            Self::Extension(value) => !e.is_dir && extension_matches(&e.name_lower, value),
            Self::ExtensionsAny(values) => {
                !e.is_dir
                    && values
                        .iter()
                        .any(|value| extension_matches(&e.name_lower, value))
            }
            Self::Parent(value) => e.path.parent() == Some(Path::new(value)),
            Self::DeviceId { id, exclude } => e
                .filesystem_id()
                .is_some_and(|(device, _)| (device == *id) != *exclude),
            Self::Inode { id, exclude } => e
                .filesystem_id()
                .is_some_and(|(_, inode)| (inode == *id) != *exclude),
            Self::MinDepth(_) | Self::MaxDepth(_) => true, // Requires the provider root.
            Self::TypesAny(_) => true, // Requires provider observations for each alternative.
            Self::Empty => true, // Requires a provider observation, including directory entries.
            Self::FilesOnly => !e.is_dir,
            Self::SpecialFileType(_) => true, // Requires provider lexical file-type metadata.
            Self::RegularFiles => true,       // Requires provider lexical file-type metadata.
            Self::SymlinksOnly => e.is_symlink(),
            Self::BrokenSymlinks => true, // Requires an observed target state.
            Self::ExecutableFiles => true, // Requires provider lexical mode/type metadata.
            Self::Kind(kind) => kind_of(e) == *kind,
            Self::MinSize(min) => e.size >= *min,
            Self::MinLinks(min) => e.hard_link_count().is_some_and(|n| n >= *min),
            Self::MaxLinks(max) => e.hard_link_count().is_some_and(|n| n <= *max),
            Self::MinAllocated(min) => e.allocated_bytes().is_some_and(|bytes| bytes >= *min),
            Self::MaxAllocated(max) => e.allocated_bytes().is_some_and(|bytes| bytes <= *max),
            Self::MaxSize(max) => e.size <= *max,
            Self::AccessedAfter(value) => e
                .accessed
                .zip(parse_absolute_time(value).ok())
                .is_some_and(|(time, boundary)| time > boundary),
            Self::AccessedBefore(value) => e
                .accessed
                .zip(parse_absolute_time(value).ok())
                .is_some_and(|(time, boundary)| time < boundary),
            Self::CreatedAfter(value) => e
                .created
                .zip(parse_absolute_time(value).ok())
                .is_some_and(|(time, boundary)| time > boundary),
            Self::CreatedBefore(value) => e
                .created
                .zip(parse_absolute_time(value).ok())
                .is_some_and(|(time, boundary)| time < boundary),
            Self::ChangedAfter(value) => e
                .changed
                .zip(parse_absolute_time(value).ok())
                .is_some_and(|(time, boundary)| time > boundary),
            Self::ChangedBefore(value) => e
                .changed
                .zip(parse_absolute_time(value).ok())
                .is_some_and(|(time, boundary)| time < boundary),
            Self::ModifiedAfter(value) => e
                .modified
                .zip(parse_absolute_time(value).ok())
                .is_some_and(|(modified, boundary)| modified > boundary),
            Self::ModifiedBefore(value) => e
                .modified
                .zip(parse_absolute_time(value).ok())
                .is_some_and(|(modified, boundary)| modified < boundary),
            Self::AgeSeconds {
                comparison,
                seconds,
            } => e.modified.is_some_and(|modified| {
                comparison.matches(
                    now.duration_since(modified).unwrap_or_default(),
                    std::time::Duration::from_secs(*seconds),
                )
            }),
            Self::MaxAgeDays(days) => age_days(e.modified, now).is_some_and(|d| d <= *days),
            Self::MinAgeDays(days) => age_days(e.modified, now).is_some_and(|d| d >= *days),
        }
    }

    pub fn chip(&self) -> String {
        match self {
            Self::NameExcludes(value) => format!("notname:{value}"),
            Self::PathExcludes(value) => format!("notpath:{value}"),
            Self::NameContains(value) => format!("name:{value}"),
            Self::NameExact(value) => format!("nameexact:{}", quote_if_needed(value)),
            Self::NameNotExact(value) => format!("notnameexact:{}", quote_if_needed(value)),
            Self::ExcludeGlob(value) => format!("exclude:{value}"),
            Self::ExcludeFileGlob(value) => format!("excludefile:{value}"),
            Self::ExcludeDirGlob(value) => format!("excludedir:{value}"),
            Self::NameGlob(value) => format!("glob:{value}"),
            Self::NameFilter(value) => format!("filter:{value}"),
            Self::Extension(value) => format!("ext:{value}"),
            Self::ExtensionsAny(values) => format!("exts:{}", values.join(",")),
            Self::PathContains(value) => format!("path:{value}"),
            Self::Parent(value) => format!("parent:{value}"),
            Self::ContentExcludes(value) => format!("notcontent:{value}"),
            Self::ContentContains(value) => format!("content:{value}"),
            Self::NameAny(values) => format!(
                "anyname:({})",
                values
                    .iter()
                    .map(|value| format!("{value:?}"))
                    .collect::<Vec<_>>()
                    .join(" OR ")
            ),
            Self::ContentAny(values) => format!(
                "anycontent:({})",
                values
                    .iter()
                    .map(|value| format!("{value:?}"))
                    .collect::<Vec<_>>()
                    .join(" OR ")
            ),
            Self::ContentLine(value) => format!("contentline:{value}"),
            Self::ContentLineExcludes(value) => format!("notcontentline:{value}"),
            Self::ContentWord(value) => format!("contentword:{value}"),
            Self::ContentWordExcludes(value) => format!("notcontentword:{value}"),
            Self::SearchArchives(enabled) => {
                format!("archives:{}", if *enabled { "on" } else { "off" })
            }
            Self::Hidden(mode) => format!("hidden:{}", mode.label()),
            Self::SmartCase => "case:smart".to_string(),
            Self::CaseSensitive(enabled) => format!(
                "case:{}",
                if *enabled { "sensitive" } else { "insensitive" }
            ),
            Self::TypesAny(types) => format!(
                "types:{}",
                types
                    .iter()
                    .map(|kind| kind.label())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Self::Empty => "type:empty".to_string(),
            Self::FilesOnly => "type:file".to_string(),
            Self::UnixPermissions(value) => format!("perm:{value}"),
            Self::DeviceId { id, exclude } => {
                format!("device:{}{id}", if *exclude { "!" } else { "" })
            }
            Self::Inode { id, exclude } => format!("inode:{}{id}", if *exclude { "!" } else { "" }),
            Self::OwnerId { id, exclude } => format!("uid:{}{id}", if *exclude { "!" } else { "" }),
            Self::GroupId { id, exclude } => format!("gid:{}{id}", if *exclude { "!" } else { "" }),
            Self::RegularFiles => "type:regular".to_string(),
            Self::SpecialFileType(kind) => format!("type:{}", kind.label()),
            Self::SymlinksOnly => "type:symlink".to_string(),
            Self::BrokenSymlinks => "type:broken-link".to_string(),
            Self::ExecutableFiles => "type:executable".to_string(),
            Self::Kind(kind) => format!("type:{}", kind.label()),
            Self::MinDepth(depth) => format!("depth:>={depth}"),
            Self::MaxDepth(depth) => format!("depth:<={depth}"),
            Self::MinSize(bytes) => format!("size:>={}", format_bytes(*bytes)),
            Self::MinLinks(n) => format!("links:>={n}"),
            Self::MaxLinks(n) => format!("links:<={n}"),
            Self::MinAllocated(bytes) => format!("allocated:>={}", format_bytes(*bytes)),
            Self::MaxAllocated(bytes) => format!("allocated:<={}", format_bytes(*bytes)),
            Self::MaxSize(bytes) => format!("size:<={}", format_bytes(*bytes)),
            Self::ModifiedAfter(value) => format!("after:{}", quote_if_needed(value)),
            Self::ModifiedBefore(value) => format!("before:{}", quote_if_needed(value)),
            Self::AccessedAfter(value) => format!("accessed-after:{}", quote_if_needed(value)),
            Self::AccessedBefore(value) => format!("accessed-before:{}", quote_if_needed(value)),
            Self::CreatedAfter(value) => format!("created-after:{}", quote_if_needed(value)),
            Self::CreatedBefore(value) => format!("created-before:{}", quote_if_needed(value)),
            Self::ChangedAfter(value) => format!("changed-after:{}", quote_if_needed(value)),
            Self::ChangedBefore(value) => format!("changed-before:{}", quote_if_needed(value)),
            Self::AgeSeconds {
                comparison,
                seconds,
            } => format!("age:{}{seconds}s", comparison.operator()),
            Self::MaxAgeDays(days) => format!("date:<={days}d"),
            Self::MinAgeDays(days) => format!("date:>={days}d"),
        }
    }

    fn expression(&self) -> String {
        match self {
            // A bare `a:b` would reparse as field `a`, so spell the field out.
            Self::NameContains(value) if value.contains(':') => {
                format!("name:{}", quote_if_needed(value))
            }
            Self::NameContains(value) => quote_if_needed(value),
            Self::NameExact(value) => format!("nameexact:{}", quote_if_needed(value)),
            Self::NameNotExact(value) => format!("notnameexact:{}", quote_if_needed(value)),
            Self::ExcludeGlob(value) => format!("exclude:{}", quote_if_needed(value)),
            Self::ExcludeFileGlob(value) => format!("excludefile:{}", quote_if_needed(value)),
            Self::ExcludeDirGlob(value) => format!("excludedir:{}", quote_if_needed(value)),
            Self::NameGlob(value) => format!("glob:{}", quote_if_needed(value)),
            Self::NameFilter(value) => format!("filter:{}", quote_if_needed(value)),
            Self::Extension(value) => format!("ext:{}", quote_if_needed(value)),
            Self::ExtensionsAny(values) => format!("exts:{}", quote_if_needed(&values.join(","))),
            Self::NameExcludes(value) => format!("notname:{}", quote_if_needed(value)),
            Self::PathExcludes(value) => format!("notpath:{}", quote_if_needed(value)),
            Self::PathContains(value) => format!("path:{}", quote_if_needed(value)),
            Self::Parent(value) => format!("parent:{}", quote_if_needed(value)),
            Self::ContentExcludes(value) => format!("notcontent:{}", quote_if_needed(value)),
            Self::ContentContains(value) => format!("content:{}", quote_if_needed(value)),
            Self::NameAny(values) => values
                .iter()
                .map(|value| format!("anyname:{}", quote_if_needed(value)))
                .collect::<Vec<_>>()
                .join(" "),
            Self::ContentAny(values) => values
                .iter()
                .map(|value| format!("anycontent:{}", quote_if_needed(value)))
                .collect::<Vec<_>>()
                .join(" "),
            Self::ContentLine(value) => format!("contentline:{}", quote_if_needed(value)),
            Self::ContentLineExcludes(value) => {
                format!("notcontentline:{}", quote_if_needed(value))
            }
            Self::ContentWord(value) => format!("contentword:{}", quote_if_needed(value)),
            Self::ContentWordExcludes(value) => {
                format!("notcontentword:{}", quote_if_needed(value))
            }
            Self::SearchArchives(enabled) => {
                format!("archives:{}", if *enabled { "on" } else { "off" })
            }
            Self::Hidden(mode) => format!("hidden:{}", mode.label()),
            Self::SmartCase => "case:smart".to_string(),
            Self::CaseSensitive(enabled) => format!(
                "case:{}",
                if *enabled { "sensitive" } else { "insensitive" }
            ),
            Self::TypesAny(types) => format!(
                "types:{}",
                types
                    .iter()
                    .map(|kind| kind.label())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Self::Empty => "type:empty".to_string(),
            Self::FilesOnly => "type:file".to_string(),
            Self::UnixPermissions(value) => format!("perm:{value}"),
            Self::DeviceId { id, exclude } => {
                format!("device:{}{id}", if *exclude { "!" } else { "" })
            }
            Self::Inode { id, exclude } => format!("inode:{}{id}", if *exclude { "!" } else { "" }),
            Self::OwnerId { id, exclude } => format!("uid:{}{id}", if *exclude { "!" } else { "" }),
            Self::GroupId { id, exclude } => format!("gid:{}{id}", if *exclude { "!" } else { "" }),
            Self::RegularFiles => "type:regular".to_string(),
            Self::SpecialFileType(kind) => format!("type:{}", kind.label()),
            Self::SymlinksOnly => "type:symlink".to_string(),
            Self::BrokenSymlinks => "type:broken-link".to_string(),
            Self::ExecutableFiles => "type:executable".to_string(),
            Self::Kind(kind) => format!("type:{}", kind.label()),
            Self::MinDepth(depth) => format!("depth:>={depth}"),
            Self::MaxDepth(depth) => format!("depth:<={depth}"),
            Self::MinSize(bytes) => format!("size:>={}", format_bytes(*bytes)),
            Self::MinLinks(n) => format!("links:>={n}"),
            Self::MaxLinks(n) => format!("links:<={n}"),
            Self::MinAllocated(bytes) => format!("allocated:>={}", format_bytes(*bytes)),
            Self::MaxAllocated(bytes) => format!("allocated:<={}", format_bytes(*bytes)),
            Self::MaxSize(bytes) => format!("size:<={}", format_bytes(*bytes)),
            Self::ModifiedAfter(value) => format!("after:{}", quote_if_needed(value)),
            Self::ModifiedBefore(value) => format!("before:{}", quote_if_needed(value)),
            Self::AccessedAfter(value) => format!("accessed-after:{}", quote_if_needed(value)),
            Self::AccessedBefore(value) => format!("accessed-before:{}", quote_if_needed(value)),
            Self::CreatedAfter(value) => format!("created-after:{}", quote_if_needed(value)),
            Self::CreatedBefore(value) => format!("created-before:{}", quote_if_needed(value)),
            Self::ChangedAfter(value) => format!("changed-after:{}", quote_if_needed(value)),
            Self::ChangedBefore(value) => format!("changed-before:{}", quote_if_needed(value)),
            Self::AgeSeconds {
                comparison,
                seconds,
            } => format!("age:{}{seconds}s", comparison.operator()),
            Self::MaxAgeDays(days) => format!("date:<={days}d"),
            Self::MinAgeDays(days) => format!("date:>={days}d"),
        }
    }
}

/// Parse a strict calendar date at midnight UTC or an explicit RFC 3339 instant.
/// Leap seconds are rejected because `SystemTime` cannot represent them distinctly.
pub fn parse_absolute_time(value: &str) -> Result<SystemTime, QueryError> {
    let invalid = || {
        QueryError::new(
            value.to_string(),
            "expected YYYY-MM-DD (UTC) or RFC 3339 time with offset",
        )
    };
    let timestamp =
        if value.len() == 10 && value.as_bytes()[4] == b'-' && value.as_bytes()[7] == b'-' {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| invalid())?
                .and_hms_opt(0, 0, 0)
                .ok_or_else(invalid)?
                .and_utc()
        } else {
            chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|_| invalid())?
                .to_utc()
        };
    let seconds = timestamp.timestamp();
    let nanos = timestamp.timestamp_subsec_nanos();
    if nanos >= 1_000_000_000 {
        return Err(invalid());
    }
    let whole = if seconds >= 0 {
        std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds.unsigned_abs()))
    } else {
        std::time::UNIX_EPOCH.checked_sub(std::time::Duration::from_secs(seconds.unsigned_abs()))
    };
    whole
        .and_then(|time| time.checked_add(std::time::Duration::from_nanos(u64::from(nanos))))
        .ok_or_else(invalid)
}

/// Whether lexical metadata describes a regular Unix file with any execute bit.
/// This is a mode filter, not a promise of permission for the current user.
pub fn executable_file(metadata: &std::fs::Metadata) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// Observed Unix permission bits, never access checks or inferred file kinds.
pub fn unix_mode(permissions: &std::fs::Permissions) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(permissions.mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = permissions;
        None
    }
}

const PERMISSION_BITS: [(&str, u32); 12] = [
    ("ur", 0o400),
    ("uw", 0o200),
    ("ux", 0o100),
    ("gr", 0o040),
    ("gw", 0o020),
    ("gx", 0o010),
    ("or", 0o004),
    ("ow", 0o002),
    ("ox", 0o001),
    ("us", 0o4000),
    ("gs", 0o2000),
    ("sb", 0o1000),
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct PermissionGroup {
    required: u32,
    forbidden: u32,
    inverted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionPattern {
    groups: Vec<PermissionGroup>,
}

impl PermissionPattern {
    pub fn matches(&self, mode: u32) -> bool {
        self.groups.iter().any(|group| {
            let matches = mode & group.required == group.required && mode & group.forbidden == 0;
            matches != group.inverted
        })
    }
}

/// AND within groups, OR between groups; ! negates one whole group.
/// Octal masks require all + bits or forbid all - bits, like symbolic terms.
pub fn parse_permissions(value: &str) -> Result<PermissionPattern, QueryError> {
    let invalid = || {
        QueryError::new(
            value.to_string(),
            "perm expects signed symbolic/octal bits, optionally grouped with | and !",
        )
    };
    if value.is_empty() || value.len() > 4096 {
        return Err(invalid());
    }
    let mut groups = Vec::new();
    for group in value.split('|') {
        if groups.len() >= 64 {
            return Err(invalid());
        }
        let (inverted, terms) = group
            .strip_prefix('!')
            .map_or((false, group), |terms| (true, terms));
        if terms.is_empty() {
            return Err(invalid());
        }
        let bytes = terms.as_bytes();
        let (mut required, mut forbidden, mut offset) = (0, 0, 0);
        while offset < bytes.len() {
            let bit = if bytes[offset].is_ascii_digit() {
                let start = offset;
                while offset < bytes.len() && bytes[offset].is_ascii_digit() {
                    offset += 1;
                }
                let digits = &terms[start..offset];
                if !(1..=4).contains(&digits.len()) {
                    return Err(invalid());
                }
                u32::from_str_radix(digits, 8).map_err(|_| invalid())?
            } else {
                let label_bytes = bytes.get(offset..offset + 2).ok_or_else(invalid)?;
                let label = std::str::from_utf8(label_bytes).map_err(|_| invalid())?;
                offset += 2;
                PERMISSION_BITS
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(label))
                    .map(|(_, bit)| *bit)
                    .ok_or_else(invalid)?
            };
            match bytes.get(offset) {
                Some(b'+') => required |= bit,
                Some(b'-') => forbidden |= bit,
                _ => return Err(invalid()),
            }
            offset += 1;
        }
        if required & forbidden != 0 {
            return Err(QueryError::new(
                value.to_string(),
                "conflicting permission requirements",
            ));
        }
        groups.push(PermissionGroup {
            required,
            forbidden,
            inverted,
        });
    }
    Ok(PermissionPattern { groups })
}

#[derive(Default)]
struct CaseEvidence {
    literal: bool,
    uppercase: bool,
}
impl CaseEvidence {
    fn character(&mut self, character: char) {
        self.literal = true;
        self.uppercase |= character.is_uppercase();
    }
    fn literal(&mut self, value: &str) {
        for character in value.chars() {
            self.character(character);
        }
    }
    fn merge(&mut self, other: Self) {
        self.literal |= other.literal;
        self.uppercase |= other.uppercase;
    }
}

fn regex_case_evidence(value: &str) -> CaseEvidence {
    use regex_syntax::ast::{self, Ast, Visitor};
    struct LiteralVisitor(CaseEvidence);
    impl Visitor for LiteralVisitor {
        type Output = CaseEvidence;
        type Err = std::convert::Infallible;
        fn finish(self) -> Result<Self::Output, Self::Err> {
            Ok(self.0)
        }
        fn visit_pre(&mut self, node: &Ast) -> Result<(), Self::Err> {
            if let Ast::Literal(literal) = node {
                self.0.character(literal.c);
            }
            Ok(())
        }
        fn visit_class_set_item_pre(&mut self, item: &ast::ClassSetItem) -> Result<(), Self::Err> {
            match item {
                ast::ClassSetItem::Literal(literal) => self.0.character(literal.c),
                ast::ClassSetItem::Range(range) => {
                    self.0.character(range.start.c);
                    self.0.character(range.end.c);
                }
                _ => {}
            }
            Ok(())
        }
    }
    let Ok(tree) = ast::parse::Parser::new().parse(value) else {
        return CaseEvidence::default();
    };
    match ast::visit(&tree, LiteralVisitor(CaseEvidence::default())) {
        Ok(evidence) => evidence,
        Err(never) => match never {},
    }
}

/// One entire literal basename, independent of the query's text mode.
pub fn compile_exact_name(value: &str, case_sensitive: bool) -> Result<regex::Regex, QueryError> {
    if value.is_empty() {
        return Err(QueryError::new(value.to_string(), "exact name is empty"));
    }
    regex::RegexBuilder::new(&format!(r"\A(?:{})\z", regex::escape(value)))
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}

/// One literal line, preserving spaces and accepting LF/CR/CRLF terminators.
pub fn compile_content_line(value: &str, case_sensitive: bool) -> Result<regex::Regex, QueryError> {
    if value.contains(['\n', '\r']) {
        return Err(QueryError::new(
            value.to_string(),
            "contentline expects a single line without line terminators",
        ));
    }
    let pattern = format!(r"(?mR:^(?:{})$)", regex::escape(value));
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}

/// Literal terms with ripgrep-style Unicode half-word boundaries on both edges.
pub fn compile_content_word(value: &str, case_sensitive: bool) -> Result<regex::Regex, QueryError> {
    if value.is_empty() {
        return Err(QueryError::new(
            value.to_string(),
            "word condition is empty",
        ));
    }
    let pattern = format!(
        r"\b{{start-half}}(?:{})\b{{end-half}}",
        regex::escape(value)
    );
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}

pub fn extension_matches(name: &str, extension: &str) -> bool {
    name.strip_suffix(extension)
        .is_some_and(|prefix| prefix.ends_with('.'))
}

fn valid_extension(value: &str) -> bool {
    !value.is_empty()
        && !value.split('.').any(str::is_empty)
        && !value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '*' | '?'))
}

/// Compile Commander's whole-name `*`/`?` mask without changing Unicode character boundaries.
pub fn compile_name_glob(value: &str, case_sensitive: bool) -> Result<regex::Regex, QueryError> {
    let mut pattern = String::from("\\A");
    let mut literal_start = 0;
    for (index, character) in value.char_indices() {
        if matches!(character, '*' | '?') {
            pattern.push_str(&regex::escape(&value[literal_start..index]));
            pattern.push_str(if character == '*' { ".*" } else { "." });
            literal_start = index + 1;
        }
    }
    pattern.push_str(&regex::escape(&value[literal_start..]));
    pattern.push_str("\\z");
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .dot_matches_new_line(true)
        .build()
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}

/// Basenames match at any depth; relative paths use provider roots and absolute
/// Unix paths use the lexical filesystem root.
pub enum DirectoryMask {
    Name(regex::Regex),
    Relative(regex::Regex),
    Absolute(regex::Regex),
}

impl DirectoryMask {
    pub fn matches_name(&self, name: &str) -> bool {
        matches!(self, Self::Name(regex) if regex.is_match(name))
    }

    pub fn matches_directory(&self, path: &Path, root: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        if relative.as_os_str().is_empty() {
            return false;
        }
        match self {
            Self::Name(regex) => path
                .file_name()
                .is_some_and(|name| regex.is_match(&name.to_string_lossy())),
            Self::Absolute(regex) => {
                if !path.is_absolute() || !path.starts_with(Path::new("/")) {
                    return false;
                }
                let normalized = path
                    .components()
                    .filter_map(|component| match component {
                        std::path::Component::Normal(name) => Some(name.to_string_lossy()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                regex.is_match(&normalized)
            }
            Self::Relative(regex) => {
                let normalized = relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                regex.is_match(&normalized)
            }
        }
    }
}

pub fn compile_directory_mask(
    value: &str,
    case_sensitive: bool,
) -> Result<DirectoryMask, QueryError> {
    let invalid = || {
        QueryError::new(
            value.to_string(),
            "excludedir expects a name, relative or absolute directory mask without empty, . or .. components",
        )
    };
    if value.is_empty() {
        return Err(invalid());
    }
    if !value.contains('/') {
        return compile_name_glob(value, case_sensitive).map(DirectoryMask::Name);
    }
    let absolute = value.starts_with('/');
    let relative = if absolute {
        value.strip_prefix('/').unwrap()
    } else {
        value.strip_prefix("./").unwrap_or(value)
    };
    let relative = relative.strip_suffix('/').unwrap_or(relative);
    let mut parts = Vec::new();
    for part in relative.split('/') {
        if part.is_empty() || matches!(part, "." | "..") || (part.contains("**") && part != "**") {
            return Err(invalid());
        }
        if part == "**" && parts.last() == Some(&"**") {
            continue;
        }
        parts.push(part);
    }
    let mut pattern = String::from("\\A");
    for (index, part) in parts.iter().enumerate() {
        if *part == "**" {
            if index + 1 == parts.len() {
                pattern.push_str(if index == 0 {
                    "(?:[^/]+(?:/[^/]+)*)?"
                } else {
                    "(?:/[^/]+)*"
                });
            } else {
                if index > 0 {
                    pattern.push('/');
                }
                pattern.push_str("(?:[^/]+/)*");
            }
        } else {
            if index > 0 && parts[index - 1] != "**" {
                pattern.push('/');
            }
            let mut literal_start = 0;
            for (offset, character) in part.char_indices() {
                if matches!(character, '*' | '?') {
                    pattern.push_str(&regex::escape(&part[literal_start..offset]));
                    pattern.push_str(if character == '*' { "[^/]*" } else { "[^/]" });
                    literal_start = offset + 1;
                }
            }
            pattern.push_str(&regex::escape(&part[literal_start..]));
        }
    }
    pattern.push_str("\\z");
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map(|regex| {
            if absolute {
                DirectoryMask::Absolute(regex)
            } else {
                DirectoryMask::Relative(regex)
            }
        })
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}

/// Whole days since `modified`. A future mtime (clock skew, a copy from a
/// machine ahead of us) counts as modified today, as search's time buckets do.
fn age_days(modified: Option<SystemTime>, now: SystemTime) -> Option<u64> {
    Some(
        now.duration_since(modified?)
            .map_or(0, |d| d.as_secs() / SECONDS_PER_DAY),
    )
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
        for (token, quoted) in tokenize(expression)? {
            // A token that opens with a quote is literal name text, so
            // `"note: draft"` searches for the colon instead of a field.
            let split = if quoted { None } else { token.split_once(':') };
            let Some((field, value)) = split else {
                predicates.push(Predicate::NameContains(token));
                continue;
            };
            if value.is_empty()
                && !field.eq_ignore_ascii_case("contentline")
                && !field.eq_ignore_ascii_case("notcontentline")
            {
                return Err(QueryError::new(token, "field value is empty"));
            }
            match field.to_ascii_lowercase().as_str() {
                "name" => predicates.push(Predicate::NameContains(value.to_string())),
                "nameexact" => predicates.push(Predicate::NameExact(value.to_string())),
                "notnameexact" => predicates.push(Predicate::NameNotExact(value.to_string())),
                "notname" => predicates.push(Predicate::NameExcludes(value.to_string())),
                "notpath" => predicates.push(Predicate::PathExcludes(value.to_string())),
                "glob" => predicates.push(Predicate::NameGlob(value.to_string())),
                "exclude" => predicates.push(Predicate::ExcludeGlob(value.to_string())),
                "excludefile" => predicates.push(Predicate::ExcludeFileGlob(value.to_string())),
                "excludedir" => predicates.push(Predicate::ExcludeDirGlob(value.to_string())),
                "filter" => predicates.push(Predicate::NameFilter(value.to_string())),
                "ext" => predicates.push(Predicate::Extension(
                    value.strip_prefix('.').unwrap_or(value).to_string(),
                )),
                "exts" => predicates.push(Predicate::ExtensionsAny(
                    value
                        .split(',')
                        .map(|value| {
                            value
                                .trim()
                                .strip_prefix('.')
                                .unwrap_or(value.trim())
                                .to_string()
                        })
                        .collect(),
                )),
                "perm" => predicates.push(Predicate::UnixPermissions(value.to_string())),
                "device" | "inode" => {
                    let (id, exclude) = parse_numeric_id(value, field)?;
                    predicates.push(if field.eq_ignore_ascii_case("device") {
                        Predicate::DeviceId { id, exclude }
                    } else {
                        Predicate::Inode { id, exclude }
                    });
                }
                "uid" | "gid" => {
                    let (id, exclude) = parse_numeric_id(value, field)?;
                    predicates.push(if field.eq_ignore_ascii_case("uid") {
                        Predicate::OwnerId { id, exclude }
                    } else {
                        Predicate::GroupId { id, exclude }
                    });
                }
                "path" => predicates.push(Predicate::PathContains(value.to_string())),
                "parent" => predicates.push(Predicate::Parent(value.to_string())),
                "anyname" => {
                    if value.len() > MAX_ALTERNATIVE_BYTES {
                        return Err(QueryError::new(
                            "anyname",
                            "alternative name patterns exceed 64 KiB",
                        ));
                    }
                    if let Some(Predicate::NameAny(values)) = predicates
                        .iter_mut()
                        .find(|predicate| matches!(predicate, Predicate::NameAny(_)))
                    {
                        let bytes = values.iter().fold(value.len(), |total, value| {
                            total.saturating_add(value.len())
                        });
                        if values.len() >= MAX_ALTERNATIVE_TERMS || bytes > MAX_ALTERNATIVE_BYTES {
                            return Err(QueryError::new(
                                "anyname",
                                "anyname allows at most 64 patterns and 64 KiB",
                            ));
                        }
                        values.push(value.to_string());
                    } else {
                        predicates.push(Predicate::NameAny(vec![value.to_string()]));
                    }
                }
                "anycontent" => {
                    if value.len() > MAX_ALTERNATIVE_BYTES {
                        return Err(QueryError::new(
                            "anycontent",
                            "alternative content patterns exceed 64 KiB",
                        ));
                    }
                    if let Some(Predicate::ContentAny(values)) = predicates
                        .iter_mut()
                        .find(|predicate| matches!(predicate, Predicate::ContentAny(_)))
                    {
                        let bytes = values.iter().fold(value.len(), |total, value| {
                            total.saturating_add(value.len())
                        });
                        if values.len() >= MAX_ALTERNATIVE_TERMS || bytes > MAX_ALTERNATIVE_BYTES {
                            return Err(QueryError::new(
                                "anycontent",
                                "anycontent allows at most 64 patterns and 64 KiB",
                            ));
                        }
                        values.push(value.to_string());
                    } else {
                        predicates.push(Predicate::ContentAny(vec![value.to_string()]));
                    }
                }
                "content" => predicates.push(Predicate::ContentContains(value.to_string())),
                "notcontent" => predicates.push(Predicate::ContentExcludes(value.to_string())),
                "contentline" => predicates.push(Predicate::ContentLine(value.to_string())),
                "notcontentline" => {
                    predicates.push(Predicate::ContentLineExcludes(value.to_string()))
                }
                "contentword" => predicates.push(Predicate::ContentWord(value.to_string())),
                "notcontentword" => {
                    predicates.push(Predicate::ContentWordExcludes(value.to_string()))
                }
                "case" if value.eq_ignore_ascii_case("smart") => {
                    predicates.push(Predicate::SmartCase)
                }
                "case" => predicates.push(Predicate::CaseSensitive(
                    match value.to_ascii_lowercase().as_str() {
                        "sensitive" => true,
                        "insensitive" => false,
                        _ => {
                            return Err(QueryError::new(
                                token.clone(),
                                "case expects sensitive, insensitive or smart",
                            ));
                        }
                    },
                )),
                "hidden" => predicates.push(Predicate::Hidden(
                    match value.to_ascii_lowercase().as_str() {
                        "include" | "on" | "true" => HiddenMode::Include,
                        "exclude" | "off" | "false" => HiddenMode::Exclude,
                        "only" => HiddenMode::Only,
                        _ => {
                            return Err(QueryError::new(
                                token.clone(),
                                "hidden expects include, exclude or only",
                            ));
                        }
                    },
                )),
                "archives" => predicates.push(Predicate::SearchArchives(
                    match value.to_ascii_lowercase().as_str() {
                        "on" | "true" => true,
                        "off" | "false" => false,
                        _ => {
                            return Err(QueryError::new(
                                token.clone(),
                                "archives expects on or off",
                            ));
                        }
                    },
                )),
                "type" => predicates.push(
                    TypeCondition::parse(value)
                        .ok_or_else(|| QueryError::new(token.clone(), "unknown type"))?
                        .predicate(),
                ),
                "types" => {
                    let mut types = Vec::new();
                    for value in value.split(',') {
                        if types.len() == MAX_ALTERNATIVE_TERMS {
                            return Err(QueryError::new(
                                token.clone(),
                                "types allows at most 64 alternatives",
                            ));
                        }
                        types.push(TypeCondition::parse(value.trim()).ok_or_else(|| {
                            QueryError::new(
                                token.clone(),
                                "types expects a comma-separated list of known types",
                            )
                        })?);
                    }
                    predicates.push(Predicate::TypesAny(types));
                }
                "depth" => predicates.extend(
                    parse_depth(value)
                        .map_err(|message| QueryError::new(token.clone(), message))?,
                ),
                "links" => predicates.extend(
                    parse_links(value)
                        .map_err(|message| QueryError::new(token.clone(), message))?,
                ),
                "allocated" => predicates.extend(
                    parse_size(value)
                        .map_err(|message| QueryError::new(token.clone(), message))?
                        .into_iter()
                        .map(|p| match p {
                            Predicate::MinSize(n) => Predicate::MinAllocated(n),
                            Predicate::MaxSize(n) => Predicate::MaxAllocated(n),
                            _ => unreachable!(),
                        }),
                ),
                "size" => predicates.extend(
                    parse_size(value).map_err(|message| QueryError::new(token.clone(), message))?,
                ),
                "accessed-after" | "accessed-before" => {
                    parse_absolute_time(value)?;
                    predicates.push(if field.eq_ignore_ascii_case("accessed-after") {
                        Predicate::AccessedAfter(value.to_string())
                    } else {
                        Predicate::AccessedBefore(value.to_string())
                    });
                }
                "created-after" | "created-before" => {
                    parse_absolute_time(value)?;
                    predicates.push(if field.eq_ignore_ascii_case("created-after") {
                        Predicate::CreatedAfter(value.to_string())
                    } else {
                        Predicate::CreatedBefore(value.to_string())
                    });
                }
                "changed-after" | "changed-before" => {
                    parse_absolute_time(value)?;
                    predicates.push(if field.eq_ignore_ascii_case("changed-after") {
                        Predicate::ChangedAfter(value.to_string())
                    } else {
                        Predicate::ChangedBefore(value.to_string())
                    });
                }
                "after" | "before" => {
                    parse_absolute_time(value)?;
                    predicates.push(if field.eq_ignore_ascii_case("after") {
                        Predicate::ModifiedAfter(value.to_string())
                    } else {
                        Predicate::ModifiedBefore(value.to_string())
                    });
                }
                "age" => predicates.push(
                    parse_age(value).map_err(|message| QueryError::new(token.clone(), message))?,
                ),
                "date" => predicates.push(
                    parse_date(value).map_err(|message| QueryError::new(token.clone(), message))?,
                ),
                _ => return Err(QueryError::new(token, "unknown query field")),
            }
        }
        let query = Self { predicates, mode };
        query.validate()?;
        Ok(query)
    }

    pub fn case_sensitive(&self) -> bool {
        if self.predicates.contains(&Predicate::CaseSensitive(true)) {
            return true;
        }
        if !self.predicates.contains(&Predicate::SmartCase) {
            return false;
        }
        let mut evidence = CaseEvidence::default();
        for predicate in &self.predicates {
            match predicate {
                Predicate::NameAny(values) | Predicate::ContentAny(values) => {
                    for value in values {
                        if self.mode == MatchMode::Regex {
                            evidence.merge(regex_case_evidence(value));
                        } else {
                            evidence.literal(value);
                        }
                    }
                }
                Predicate::NameContains(value)
                | Predicate::NameExcludes(value)
                | Predicate::PathContains(value)
                | Predicate::PathExcludes(value)
                | Predicate::ContentContains(value)
                | Predicate::ContentExcludes(value) => {
                    if self.mode == MatchMode::Regex {
                        evidence.merge(regex_case_evidence(value));
                    } else {
                        evidence.literal(value);
                    }
                }
                Predicate::NameGlob(value)
                | Predicate::ExcludeGlob(value)
                | Predicate::ExcludeFileGlob(value)
                | Predicate::ExcludeDirGlob(value) => {
                    for character in value.chars().filter(|ch| !matches!(ch, '*' | '?')) {
                        evidence.character(character);
                    }
                }
                Predicate::NameExact(value)
                | Predicate::NameNotExact(value)
                | Predicate::ContentWord(value)
                | Predicate::ContentWordExcludes(value)
                | Predicate::ContentLine(value)
                | Predicate::ContentLineExcludes(value)
                | Predicate::NameFilter(value)
                | Predicate::Extension(value) => evidence.literal(value),
                Predicate::ExtensionsAny(values) => {
                    for value in values {
                        evidence.literal(value);
                    }
                }
                _ => {}
            }
            if evidence.uppercase {
                return true;
            }
        }
        // Regex classes without literals must retain their intrinsic case meaning.
        !evidence.literal
    }

    pub fn validate(&self) -> Result<(), QueryError> {
        for predicate in &self.predicates {
            if let Predicate::ModifiedAfter(value)
            | Predicate::ModifiedBefore(value)
            | Predicate::AccessedAfter(value)
            | Predicate::AccessedBefore(value)
            | Predicate::CreatedAfter(value)
            | Predicate::CreatedBefore(value)
            | Predicate::ChangedAfter(value)
            | Predicate::ChangedBefore(value) = predicate
            {
                parse_absolute_time(value)?;
            }
        }
        let mut case = None;
        for predicate in &self.predicates {
            let current = match predicate {
                Predicate::CaseSensitive(false) => Some(0),
                Predicate::CaseSensitive(true) => Some(1),
                Predicate::SmartCase => Some(2),
                _ => None,
            };
            if let Some(current) = current {
                if case.is_some_and(|previous| previous != current) {
                    return Err(QueryError::new(
                        "case".to_string(),
                        "conflicting case options",
                    ));
                }
                case = Some(current);
            }
        }
        let mut alternative_groups = [false; 2];
        for predicate in &self.predicates {
            let (values, index, field) = match predicate {
                Predicate::NameAny(values) => (values, 0, "anyname"),
                Predicate::ContentAny(values) => (values, 1, "anycontent"),
                _ => continue,
            };
            if alternative_groups[index]
                || values.is_empty()
                || values.len() > MAX_ALTERNATIVE_TERMS
                || values.iter().any(String::is_empty)
                || values
                    .iter()
                    .fold(0usize, |total, value| total.saturating_add(value.len()))
                    > MAX_ALTERNATIVE_BYTES
            {
                return Err(QueryError::new(
                    field,
                    "expected one group of 1..64 nonempty patterns, at most 64 KiB",
                ));
            }
            alternative_groups[index] = true;
        }
        for predicate in &self.predicates {
            if let Predicate::Parent(value) = predicate {
                let path = Path::new(value);
                if !path.is_absolute()
                    || value.contains('\0')
                    || path
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    return Err(QueryError::new(
                        value.clone(),
                        "parent expects an absolute lexical folder path without .. or NUL",
                    ));
                }
            }
        }
        let case_sensitive = self.case_sensitive();
        let mut hidden = None;
        for predicate in &self.predicates {
            if let Predicate::Hidden(mode) = predicate {
                if hidden.is_some_and(|previous| previous != *mode) {
                    return Err(QueryError::new(
                        "hidden".to_string(),
                        "conflicting hidden-file options",
                    ));
                }
                hidden = Some(*mode);
            }
        }
        let mut archives = None;
        for predicate in &self.predicates {
            if let Predicate::SearchArchives(enabled) = predicate {
                if archives.is_some_and(|previous| previous != *enabled) {
                    return Err(QueryError::new(
                        "archives".to_string(),
                        "conflicting archive search options",
                    ));
                }
                archives = Some(*enabled);
            }
        }
        for predicate in &self.predicates {
            if let Predicate::TypesAny(types) = predicate
                && (types.is_empty() || types.len() > MAX_ALTERNATIVE_TERMS)
            {
                return Err(QueryError::new("types", "expected 1..64 type alternatives"));
            }
            if let Predicate::NameExact(value) | Predicate::NameNotExact(value) = predicate {
                compile_exact_name(value, case_sensitive)?;
            }
            if let Predicate::UnixPermissions(value) = predicate {
                parse_permissions(value)?;
            }
            if let Predicate::ExcludeGlob(value) | Predicate::ExcludeFileGlob(value) = predicate
                && (value.is_empty() || value.contains('/'))
            {
                return Err(QueryError::new(
                    value.clone(),
                    "exclusion expects a non-empty name mask, not a path",
                ));
            }
            if let Predicate::NameGlob(value)
            | Predicate::ExcludeGlob(value)
            | Predicate::ExcludeFileGlob(value) = predicate
            {
                compile_name_glob(value, case_sensitive)?;
            }
            if let Predicate::ExcludeDirGlob(value) = predicate {
                compile_directory_mask(value, case_sensitive)?;
            }
            if let Predicate::ContentLine(value) | Predicate::ContentLineExcludes(value) = predicate
            {
                compile_content_line(value, case_sensitive)?;
            }
            if let Predicate::ContentWord(value) | Predicate::ContentWordExcludes(value) = predicate
            {
                compile_content_word(value, case_sensitive)?;
            }
            if let Predicate::Extension(value) = predicate
                && !valid_extension(value)
            {
                return Err(QueryError::new(
                    value.clone(),
                    "expected a literal extension such as rs or tar.gz",
                ));
            }
            if let Predicate::ExtensionsAny(values) = predicate
                && (values.is_empty()
                    || values
                        .iter()
                        .any(|value| !valid_extension(value) || value.contains(',')))
            {
                return Err(QueryError::new(
                    values.join(","),
                    "exts expects a non-empty comma-separated list of literal extensions",
                ));
            }
        }

        if self.mode == MatchMode::Regex {
            for predicate in &self.predicates {
                let values: &[String] = match predicate {
                    Predicate::NameContains(value)
                    | Predicate::NameExcludes(value)
                    | Predicate::PathExcludes(value)
                    | Predicate::PathContains(value)
                    | Predicate::ContentContains(value)
                    | Predicate::ContentExcludes(value) => std::slice::from_ref(value),
                    Predicate::NameAny(values) | Predicate::ContentAny(values) => values,
                    _ => continue,
                };
                for value in values {
                    regex::RegexBuilder::new(value)
                        .case_insensitive(!case_sensitive)
                        .build()
                        .map_err(|error| QueryError::new(value.clone(), error.to_string()))?;
                }
            }
        }
        Ok(())
    }

    /// Metadata-only matching, through the same compiled matchers Find uses
    /// (name and path text under the query's mode; `path:` sees the full
    /// path). Content predicates are deferred and therefore pass this stage;
    /// the search provider evaluates them before emitting a hit.
    #[cfg(test)]
    pub fn matches_metadata(&self, e: &FileEntry, now: SystemTime) -> bool {
        crate::search::query_matches_metadata(self, e, now)
    }

    /// Match only queries that require no file reads. Kept for lightweight
    /// panel filters and direct unit tests.
    #[cfg(test)]
    pub fn matches(&self, e: &FileEntry, now: SystemTime) -> bool {
        !self.requires_content() && self.matches_metadata(e, now)
    }

    pub fn requires_content(&self) -> bool {
        self.predicates.iter().any(|predicate| {
            matches!(
                predicate,
                Predicate::ContentContains(_)
                    | Predicate::ContentAny(_)
                    | Predicate::ContentExcludes(_)
                    | Predicate::ContentWord(_)
                    | Predicate::ContentWordExcludes(_)
                    | Predicate::ContentLine(_)
                    | Predicate::ContentLineExcludes(_)
            )
        })
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

/// Split an expression into tokens, each flagged with whether it opened with
/// a quote.
fn tokenize(expression: &str) -> Result<Vec<(String, bool)>, QueryError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut quoted_start = false;
    let mut chars = expression.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            // Outside quotes `\"` and `\'` are literal quote characters.
            let escapable = chars.peek().is_some_and(|next| {
                next.is_whitespace()
                    || *next == '\\'
                    || Some(*next) == quote
                    || (quote.is_none() && matches!(*next, '"' | '\''))
            });
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
            if current.is_empty() {
                quoted_start = true;
            }
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push((std::mem::take(&mut current), quoted_start));
            }
            quoted_start = false;
        } else {
            current.push(ch);
        }
    }
    if let Some(delimiter) = quote {
        return Err(QueryError::new(delimiter.to_string(), "unterminated quote"));
    }
    if !current.is_empty() {
        tokens.push((current, quoted_start));
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

fn parse_depth(value: &str) -> Result<Vec<Predicate>, &'static str> {
    let number = |value: &str| {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("depth must be a non-negative integer");
        }
        value.parse::<usize>().map_err(|_| "depth is too large")
    };
    if let Some((min, max)) = value.split_once("..") {
        let (min, max) = (number(min)?, number(max)?);
        if min > max {
            return Err("depth range is reversed");
        }
        return Ok(vec![Predicate::MinDepth(min), Predicate::MaxDepth(max)]);
    }
    if let Some(value) = value.strip_prefix("<=") {
        return Ok(vec![Predicate::MaxDepth(number(value)?)]);
    }
    if let Some(value) = value.strip_prefix(">=") {
        return Ok(vec![Predicate::MinDepth(number(value)?)]);
    }
    let depth = number(value)?;
    Ok(vec![Predicate::MinDepth(depth), Predicate::MaxDepth(depth)])
}

fn parse_links(value: &str) -> Result<Vec<Predicate>, &'static str> {
    let number = |raw: &str| {
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err("links expects an unsigned decimal integer");
        }
        raw.parse::<u64>().map_err(|_| "link count is too large")
    };
    if let Some((lo, hi)) = value.split_once("..") {
        let (lo, hi) = (number(lo)?, number(hi)?);
        if lo > hi {
            return Err("link count range is reversed");
        }
        return Ok(vec![Predicate::MinLinks(lo), Predicate::MaxLinks(hi)]);
    }
    if let Some(raw) = value.strip_prefix('=') {
        let n = number(raw)?;
        return Ok(vec![Predicate::MinLinks(n), Predicate::MaxLinks(n)]);
    }
    let (op, raw) = split_operator(value);
    let n = number(raw)?;
    match op {
        "" | "=" => Ok(vec![Predicate::MinLinks(n), Predicate::MaxLinks(n)]),
        ">=" => Ok(vec![Predicate::MinLinks(n)]),
        "<=" => Ok(vec![Predicate::MaxLinks(n)]),
        ">" => n
            .checked_add(1)
            .map(|v| vec![Predicate::MinLinks(v)])
            .ok_or("no link count exceeds u64::MAX"),
        "<" => n
            .checked_sub(1)
            .map(|v| vec![Predicate::MaxLinks(v)])
            .ok_or("no link count is below zero"),
        _ => Err("invalid link count operator"),
    }
}

fn parse_size(value: &str) -> Result<Vec<Predicate>, &'static str> {
    if let Some(exact) = value.strip_prefix('=') {
        let bytes = parse_bytes(exact)?;
        return Ok(vec![Predicate::MinSize(bytes), Predicate::MaxSize(bytes)]);
    }
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
    // Size predicates are inclusive, so strict operators shift by one byte.
    match operator {
        "<" => bytes
            .checked_sub(1)
            .map(|max| vec![Predicate::MaxSize(max)])
            .ok_or("no size is below zero"),
        "<=" => Ok(vec![Predicate::MaxSize(bytes)]),
        ">" => bytes
            .checked_add(1)
            .map(|min| vec![Predicate::MinSize(min)])
            .ok_or("no size exceeds the maximum byte count"),
        ">=" | "" => Ok(vec![Predicate::MinSize(bytes)]),
        _ => Err("invalid size operator"),
    }
}

fn parse_bytes(value: &str) -> Result<u64, &'static str> {
    let upper = value.trim().to_ascii_uppercase();
    let split = upper
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .unwrap_or(upper.len());
    let numeric = &upper[..split];
    let (whole, fraction) = numeric.split_once('.').unwrap_or((numeric, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("invalid size");
    }
    let multiplier = match upper[split..].trim() {
        "" | "B" => 1,
        "K" | "KB" | "KIB" => 1_u64 << 10,
        "M" | "MB" | "MIB" => 1_u64 << 20,
        "G" | "GB" | "GIB" => 1_u64 << 30,
        "T" | "TB" | "TIB" => 1_u64 << 40,
        _ => return Err("unknown size unit"),
    };
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u64>().map_err(|_| "size is too large")?
    };
    // Multiply decimal digits right-to-left. Carry stays below the unit,
    // regardless of fractional length; its last discarded digit decides rounding.
    let mut fractional_bytes = 0;
    let mut round_up = false;
    for digit in fraction.bytes().rev() {
        let product = u64::from(digit - b'0') * multiplier + fractional_bytes;
        fractional_bytes = product / 10;
        round_up = product % 10 >= 5;
    }
    whole
        .checked_mul(multiplier)
        .and_then(|bytes| bytes.checked_add(fractional_bytes))
        .and_then(|bytes| bytes.checked_add(u64::from(round_up)))
        .ok_or("size is too large")
}

fn parse_age(value: &str) -> Result<Predicate, &'static str> {
    let value = value.trim();
    let (comparison, amount) = [
        ("<=", AgeComparison::AtMost),
        (">=", AgeComparison::AtLeast),
        ("<", AgeComparison::Less),
        (">", AgeComparison::Greater),
        ("=", AgeComparison::Equal),
    ]
    .into_iter()
    .find_map(|(operator, comparison)| {
        value
            .strip_prefix(operator)
            .map(|amount| (comparison, amount))
    })
    .unwrap_or((AgeComparison::AtMost, value));
    let amount = amount.trim().to_ascii_lowercase();
    let split = amount
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(amount.len());
    let number = amount[..split]
        .parse::<u64>()
        .map_err(|_| "age expects an unsigned whole duration")?;
    let multiplier = match amount[split..].trim() {
        "s" | "sec" | "second" | "seconds" => 1,
        "m" | "min" | "minute" | "minutes" => 60,
        "h" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        "w" | "week" | "weeks" => 604800,
        _ => return Err("age expects seconds, minutes, hours, days or weeks"),
    };
    let seconds = number
        .checked_mul(multiplier)
        .ok_or("age duration is too large")?;
    Ok(Predicate::AgeSeconds {
        comparison,
        seconds,
    })
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
    if value
        .chars()
        .all(|ch| !ch.is_whitespace() && !matches!(ch, '\\' | '"' | '\''))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Convert the active panel's lightweight filter controls into a reusable
/// smart-folder query.
pub fn from_panel_filter(search: &str, facets: &crate::panel::FacetSet) -> Query {
    let mut predicates = Vec::new();
    let text = crate::name_filter::normalized_query(search);
    if !text.is_empty() {
        predicates.push(Predicate::NameFilter(text.to_string()));
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

const fn kind_from_facet(facet: crate::panel::KindFacet) -> Kind {
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
            symlink: None,
            permissions: None,
            allocated_size: None,
            hard_links: None,
            owner_uid: None,
            owner_gid: None,
            accessed: None,
            created: None,
            accessed_str: "Unavailable".into(),
            created_str: "Unavailable".into(),
            changed: None,
            changed_str: "Unavailable".into(),
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            sort_key: crate::panel::diacritic_sort_key(&name.to_lowercase()),
            path: PathBuf::from(format!("/x/{name}")),
            identity: crate::panel::ListingIdentity::Unavailable,
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
    fn hard_link_count_queries_preserve_integer_boundaries_modes_and_saved_definitions() {
        let now = SystemTime::now();
        for mode in MatchMode::ALL {
            for expression in [
                "links:2",
                "links:=0",
                "links:>1",
                "links:<2",
                "links:2..4",
                "links:>=18446744073709551615",
                "links:<=18446744073709551615",
            ] {
                let query = Query::parse(expression, mode).unwrap();
                let rendered = Query::parse(&query.to_expression(), mode).unwrap();
                assert_eq!(query.predicates, rendered.predicates);
                let saved = crate::smart_folder::Definition {
                    name: "Hard-linked files".into(),
                    root: "/records".into(),
                    query: query.clone(),
                };
                assert_eq!(
                    serde_json::from_str::<crate::smart_folder::Definition>(
                        &serde_json::to_string(&saved).unwrap()
                    )
                    .unwrap(),
                    saved
                );
                let mut e = entry("sample", false, 0, None);
                for n in [0, 1, 2, 3, 4, u64::MAX] {
                    e.hard_links = Some(n);
                    let expected = match expression {
                        "links:2" => n == 2,
                        "links:=0" => n == 0,
                        "links:>1" => n > 1,
                        "links:<2" => n < 2,
                        "links:2..4" => (2..=4).contains(&n),
                        "links:>=18446744073709551615" => n == u64::MAX,
                        _ => true,
                    };
                    assert_eq!(
                        query.matches_metadata(&e, now),
                        expected,
                        "{mode:?}:{expression}:{n}"
                    );
                }
                e.hard_links = None;
                assert!(!query.matches_metadata(&e, now));
                e.hard_links = Some(2);
                e.is_dir = true;
                assert!(!query.matches_metadata(&e, now));
            }
            for bad in [
                "",
                "-1",
                "+2",
                "1.0",
                "1KB",
                "１",
                "1..",
                "..2",
                "3..2",
                "1..2..3",
                "18446744073709551616",
                ">18446744073709551615",
                "<0",
                "!2",
                "==2",
            ] {
                assert!(
                    Query::parse(&format!("links:{bad}"), mode).is_err(),
                    "{bad}"
                );
            }
        }
    }

    #[test]
    fn filesystem_identity_query_ids_roundtrip_and_reject_ambiguous_numbers() {
        for mode in MatchMode::ALL {
            let query =
                Query::parse("DEVICE:00042 inode:!18446744073709551615 device:0", mode).unwrap();
            assert_eq!(
                query.predicates,
                vec![
                    Predicate::DeviceId {
                        id: 42,
                        exclude: false
                    },
                    Predicate::Inode {
                        id: u64::MAX,
                        exclude: true
                    },
                    Predicate::DeviceId {
                        id: 0,
                        exclude: false
                    }
                ]
            );
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            let restored: Query =
                serde_json::from_str(&serde_json::to_string(&query).unwrap()).unwrap();
            restored.validate().unwrap();
            assert_eq!(restored, query);
            for field in ["device", "inode"] {
                for value in [
                    "",
                    "!",
                    "-1",
                    "+1",
                    "1.0",
                    "1KB",
                    "0x10",
                    "18446744073709551616",
                ] {
                    assert!(
                        Query::parse(&format!("{field}:{value}"), mode).is_err(),
                        "{field}:{value}"
                    );
                }
            }
        }
    }

    #[test]
    fn parent_filter_is_literal_immediate_and_roundtrips() {
        let now = SystemTime::now();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut file = entry("child.txt", false, 1, None);
            file.path = PathBuf::from(std::ffi::OsString::from_vec(b"/x/\xff/child.txt".to_vec()));
            let query = Query::parse("parent:/x/�", MatchMode::Exact).unwrap();
            assert!(
                !query.matches_metadata(&file, now),
                "lossy labels cannot alias a byte path"
            );
        }
        for mode in MatchMode::ALL {
            let expression = if cfg!(windows) {
                r#"PARENT:"C:/x/a [literal]" case:insensitive"#
            } else {
                r#"PARENT:"/x/a [literal]" case:insensitive"#
            };
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            let restored: Query =
                serde_json::from_str(&serde_json::to_string(&query).unwrap()).unwrap();
            restored.validate().unwrap();
            assert_eq!(restored, query);
            for (path, matches) in [
                ("/x/a [literal]/child.txt", true),
                ("/x/a [literal]/folder", true),
                ("/x/a [literal]/nested/child.txt", false),
                ("/x/a [literal]-other/child.txt", false),
                ("/x/A [literal]/child.txt", false),
                ("/x/a [literal]", false),
            ] {
                let mut file = entry("child.txt", false, 1, None);
                file.path = PathBuf::from(if cfg!(windows) {
                    format!("C:{path}")
                } else {
                    path.into()
                });
                assert_eq!(
                    query.matches_metadata(&file, now),
                    matches,
                    "{mode:?}: {path}"
                );
            }
            for value in ["", "relative", "/x/../y", "/x/\0y"] {
                let query = Query {
                    predicates: vec![Predicate::Parent(value.into())],
                    mode,
                };
                assert!(query.validate().is_err(), "{value:?}");
                assert!(Query::parse(&query.to_expression(), mode).is_err());
            }
            let mut file = entry("child.txt", false, 0, None);
            file.path = PathBuf::from(if cfg!(windows) {
                "C:/child.txt"
            } else {
                "/child.txt"
            });
            assert!(
                Query::parse(
                    if cfg!(windows) {
                        "parent:C:/"
                    } else {
                        "parent:/"
                    },
                    mode
                )
                .unwrap()
                .matches_metadata(&file, now)
            );
            assert!(
                !Query::parse(
                    if cfg!(windows) {
                        "parent:C:/ parent:C:/other"
                    } else {
                        "parent:/ parent:/other"
                    },
                    mode
                )
                .unwrap()
                .matches_metadata(&file, now)
            );
        }
    }

    #[test]
    fn allocated_query_grammar_roundtrips_bounds_and_rejects_invalid_sizes() {
        for mode in MatchMode::ALL {
            for expression in [
                "allocated:>1KB size:<=4MB",
                "allocated:=0",
                "allocated:1KB..2KB",
                "allocated:<=18446744073709551615B",
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
            assert_eq!(
                Query::parse("allocated:>1KB", mode).unwrap().predicates,
                vec![Predicate::MinAllocated(1025)]
            );
            for value in [
                "",
                "<0",
                ">18446744073709551615B",
                "-1",
                "4KB..1KB",
                "1ZZ",
                "18446744073709551616B",
            ] {
                assert!(
                    Query::parse(&format!("allocated:{value}"), mode).is_err(),
                    "{value}"
                );
            }
        }
    }

    #[test]
    fn owner_id_queries_roundtrip_and_reject_ambiguous_ids() {
        for mode in MatchMode::ALL {
            let query = Query::parse("UID:000501 gid:!18446744073709551615 uid:0", mode).unwrap();
            assert_eq!(
                query.predicates,
                vec![
                    Predicate::OwnerId {
                        id: 501,
                        exclude: false
                    },
                    Predicate::GroupId {
                        id: u64::MAX,
                        exclude: true
                    },
                    Predicate::OwnerId {
                        id: 0,
                        exclude: false
                    }
                ]
            );
            assert_eq!(
                query.chips(),
                vec!["uid:501", "gid:!18446744073709551615", "uid:0"]
            );
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            for value in [
                "",
                "!",
                "!!1",
                "-1",
                "+1",
                "1.5",
                "1e3",
                "0x10",
                "alice",
                "1:2",
                "１２",
                "18446744073709551616",
                "000000000000000000000",
            ] {
                for field in ["uid", "gid"] {
                    assert!(
                        Query::parse(&format!("{field}:{value}"), mode).is_err(),
                        "{field}:{value}"
                    );
                }
            }
        }
    }

    #[test]
    fn permission_query_validates_all_bits_and_saved_semantics() {
        for (label, bit) in PERMISSION_BITS {
            assert_eq!(
                parse_permissions(&format!("{label}+")).unwrap().groups,
                vec![PermissionGroup {
                    required: bit,
                    forbidden: 0,
                    inverted: false
                }]
            );
            assert_eq!(
                parse_permissions(&format!("{label}-")).unwrap().groups,
                vec![PermissionGroup {
                    required: 0,
                    forbidden: bit,
                    inverted: false
                }]
            );
        }
        assert_eq!(
            parse_permissions("UR+uw-ux+gs+sb-").unwrap().groups,
            vec![PermissionGroup {
                required: 0o2500,
                forbidden: 0o1200,
                inverted: false
            }]
        );
        for mode in MatchMode::ALL {
            let query = Query::parse("perm:ur+uw-ux+gs+sb- type:regular", mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            assert!(!query.requires_content());
        }
        for value in [
            "", "ur", "ur=", "zz+", "r+", "755", "ur+ur-", "ur+||uw+", "!!ur+", "é+", "ur+ ",
        ] {
            assert!(parse_permissions(value).is_err(), "{value}");
            assert!(
                q(vec![Predicate::UnixPermissions(value.into())])
                    .validate()
                    .is_err(),
                "{value}"
            );
        }
        assert!(Query::parse("perm:", MatchMode::Exact).is_err());
        assert_eq!(
            parse_permissions("ux+ux+").unwrap(),
            parse_permissions("ux+").unwrap()
        );
    }

    #[test]
    fn advanced_permission_query_masks_groups_precedence_and_limits() {
        for mode in MatchMode::ALL {
            for expression in [
                "perm:uw+|gw+|ow+",
                "perm:!700+",
                "perm:us+|gs+ perm:uw-",
                "perm:0750+002-",
                "perm:!500+|us+",
                "perm:0+",
                "perm:!0+",
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
        }
        for (octal, symbolic) in [
            ("0750+", "ur+uw+ux+gr+gx+"),
            ("222-", "uw-gw-ow-"),
            ("700+|2000+", "ur+uw+ux+|gs+"),
            ("!500+002-|us+", "!ur+ux+ow-|us+"),
            ("7777+", "ur+uw+ux+gr+gw+gx+or+ow+ox+us+gs+sb+"),
        ] {
            let octal = parse_permissions(octal).unwrap();
            let symbolic = parse_permissions(symbolic).unwrap();
            for mode in 0..=0o7777 {
                assert_eq!(octal.matches(mode), symbolic.matches(mode), "{mode:o}");
            }
        }
        let any_write = parse_permissions("uw+|gw+|ow+").unwrap();
        let not_all = parse_permissions("!700+").unwrap();
        let inverted_group = parse_permissions("!500+002-|us+").unwrap();
        for mode in 0..=0o7777 {
            assert_eq!(any_write.matches(mode), mode & 0o222 != 0);
            assert_eq!(not_all.matches(mode), mode & 0o700 != 0o700);
            assert_eq!(
                inverted_group.matches(mode),
                !(mode & 0o500 == 0o500 && mode & 0o002 == 0) || mode & 0o4000 != 0
            );
            assert!(parse_permissions("0+").unwrap().matches(mode));
            assert!(!parse_permissions("!0+").unwrap().matches(mode));
        }
        for invalid in [
            "|ur+",
            "ur+|",
            "ur+||uw+",
            "!",
            "!!ur+",
            "ur+!uw-",
            "(ur+)",
            "888+",
            "10000+",
            "-700+",
            "0o755+",
            "7777+us-",
            "00000+",
            "750",
            "ur+||!",
            "ur+|uw+uw-",
        ] {
            assert!(parse_permissions(invalid).is_err(), "{invalid}");
        }
        assert!(parse_permissions(&vec!["ur+"; 64].join("|")).is_ok());
        assert!(parse_permissions(&vec!["ur+"; 65].join("|")).is_err());
        assert!(parse_permissions(&"ur+".repeat(1365)).is_ok());
        assert!(parse_permissions(&"ur+".repeat(1366)).is_err());
        assert!(parse_permissions(&"0+".repeat(2048)).is_ok());
        assert!(parse_permissions(&format!("{}|", "0+".repeat(2048))).is_err());
    }

    #[test]
    fn regular_file_query_preserves_saved_semantics_and_unknown_kind() {
        for mode in MatchMode::ALL {
            for expression in ["type:regular", "TYPE:REGULAR-FILE", "type:regular ext:txt"] {
                let query = Query::parse(expression, mode).unwrap();
                assert!(query.predicates.contains(&Predicate::RegularFiles));
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
                assert!(!query.matches(
                    &entry("ordinary-looking.txt", false, 1, None),
                    SystemTime::now()
                ));
            }
        }
        assert_eq!(parsed("type:file"), vec![Predicate::FilesOnly]);
    }

    #[test]
    fn precise_age_query_preserves_strict_subsecond_boundaries_and_saved_semantics() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(10000);
        for mode in MatchMode::ALL {
            for (operator, expected) in [
                ("<", [true, false, false]),
                ("<=", [true, true, false]),
                ("=", [false, true, false]),
                (">=", [false, true, true]),
                (">", [false, false, true]),
            ] {
                let query = Query::parse(&format!("age:{operator}30min"), mode).unwrap();
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
                for (age, expected) in [
                    Duration::from_secs(1800) - Duration::from_nanos(1),
                    Duration::from_secs(1800),
                    Duration::from_secs(1800) + Duration::from_nanos(1),
                ]
                .into_iter()
                .zip(expected)
                {
                    assert_eq!(
                        query.matches(&entry("a", false, 1, Some(now - age)), now),
                        expected,
                        "{operator}: {age:?}"
                    );
                }
                assert!(!query.matches(&entry("a", false, 1, None), now));
            }
            let zero = Query::parse("age:=0s", mode).unwrap();
            assert!(zero.matches(
                &entry("future", false, 1, Some(now + Duration::from_secs(3600))),
                now
            ));
            assert_eq!(
                Query::parse("age:30m", mode).unwrap().to_expression(),
                "age:<=1800s"
            );
        }
        assert_eq!(parsed("date:<2m"), vec![Predicate::MaxAgeDays(60)]);
        assert_eq!(
            parsed("age:2m"),
            vec![Predicate::AgeSeconds {
                comparison: AgeComparison::AtMost,
                seconds: 120
            }]
        );
        for value in ["2h", "3d", "4w", "5SECONDS", "30min"] {
            assert!(Query::parse(&format!("age:{value}"), MatchMode::Exact).is_ok());
        }
        assert!(Query::parse(&format!("age:{}s", u64::MAX), MatchMode::Exact).is_ok());
        for value in [
            "30",
            "-1h",
            "1.5h",
            "3months",
            "!=3h",
            "18446744073709551615w",
            "18446744073709551616s",
        ] {
            assert!(
                Query::parse(&format!("age:{value}"), MatchMode::Exact).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn executable_query_roundtrips_across_modes() {
        for mode in MatchMode::ALL {
            for expression in ["type:executable", "TYPE:EXEC", "type:executable ext:sh"] {
                let query = Query::parse(expression, mode).unwrap();
                assert!(query.predicates.contains(&Predicate::ExecutableFiles));
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
                assert!(!query.requires_content());
                assert!(!query.matches(&entry("a.sh", false, 0, None), SystemTime::now()));
            }
        }
    }

    #[test]
    fn broken_symlink_query_aliases_and_type_alternatives_roundtrip() {
        for mode in MatchMode::ALL {
            for expression in [
                "type:broken-link",
                "TYPE:BROKEN-SYMLINK",
                "type:broken-link depth:<=2",
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert!(query.predicates.contains(&Predicate::BrokenSymlinks));
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert!(!query.requires_content());
                let saved = serde_json::to_string(&query).unwrap();
                assert_eq!(serde_json::from_str::<Query>(&saved).unwrap(), query);
            }
            let query = Query::parse("types:broken-symlink,regular", mode).unwrap();
            assert!(query.predicates.contains(&Predicate::TypesAny(vec![
                TypeCondition::BrokenSymlink,
                TypeCondition::Regular
            ])));
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        }
        assert!(Query::parse("type:broken", MatchMode::Exact).is_err());
    }

    #[test]
    fn symlink_query_roundtrips_across_modes() {
        for mode in MatchMode::ALL {
            for expression in ["type:symlink", "TYPE:LINK", "type:symlink ext:txt"] {
                let query = Query::parse(expression, mode).unwrap();
                assert!(query.predicates.contains(&Predicate::SymlinksOnly));
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
                assert!(!query.requires_content());
                assert!(!query.matches(&entry("link.txt", false, 0, None), SystemTime::now()));
            }
        }
    }

    #[test]
    fn accessed_boundaries_are_strict_precise_and_saved_query_safe() {
        let time = parse_absolute_time("2026-01-01T00:00:00.5Z").unwrap();
        let mut e = entry("file.txt", false, 1, Some(time));
        for mode in MatchMode::ALL {
            let q=Query::parse("Accessed-after:2026-01-01T03:00:00.25+03:00 accessed-before:2026-01-01T00:00:00.75Z",mode).unwrap();
            assert!(!q.matches(&e, SystemTime::now()));
            e.accessed = Some(time);
            assert!(q.matches(&e, SystemTime::now()));
            assert_eq!(Query::parse(&q.to_expression(), mode).unwrap(), q);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&q).unwrap()).unwrap(),
                q
            );
            for field in ["accessed-after", "accessed-before"] {
                assert!(
                    !Query::parse(&format!("{field}:2026-01-01T00:00:00.5Z"), mode)
                        .unwrap()
                        .matches(&e, SystemTime::now())
                );
            }
            e.accessed = None;
            for value in [
                "bad",
                "2026-13-01",
                "2026-01-01T00:00:00",
                "2026-01-01T25:00:00Z",
            ] {
                assert!(Query::parse(&format!("accessed-after:{value}"), mode).is_err());
                assert!(
                    Query {
                        mode,
                        predicates: vec![Predicate::AccessedBefore(value.into())]
                    }
                    .validate()
                    .is_err()
                );
            }
        }
        e.accessed = Some(parse_absolute_time("1969-12-31T23:59:59.5Z").unwrap());
        assert!(
            Query::parse("accessed-before:1970-01-01", MatchMode::Exact)
                .unwrap()
                .matches(&e, SystemTime::now())
        );
    }

    #[test]
    fn created_boundaries_are_strict_precise_and_saved_query_safe() {
        let time = parse_absolute_time("2026-01-01T00:00:00.5Z").unwrap();
        let mut e = entry("file.txt", false, 1, Some(time));
        for mode in MatchMode::ALL {
            let q = Query::parse(
                "Created-after:2026-01-01T03:00:00.25+03:00 created-before:2026-01-01T00:00:00.75Z",
                mode,
            )
            .unwrap();
            assert!(!q.matches(&e, SystemTime::now()));
            e.created = Some(time);
            assert!(q.matches(&e, SystemTime::now()));
            assert_eq!(Query::parse(&q.to_expression(), mode).unwrap(), q);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&q).unwrap()).unwrap(),
                q
            );
            for field in ["created-after", "created-before"] {
                assert!(
                    !Query::parse(&format!("{field}:2026-01-01T00:00:00.5Z"), mode)
                        .unwrap()
                        .matches(&e, SystemTime::now())
                );
            }
            e.created = None;
            for value in [
                "bad",
                "2026-13-01",
                "2026-01-01T00:00:00",
                "2026-01-01T25:00:00Z",
            ] {
                assert!(Query::parse(&format!("created-after:{value}"), mode).is_err());
                assert!(
                    Query {
                        mode,
                        predicates: vec![Predicate::CreatedBefore(value.into())]
                    }
                    .validate()
                    .is_err()
                );
            }
        }
        e.created = Some(parse_absolute_time("1969-12-31T23:59:59.5Z").unwrap());
        assert!(
            Query::parse("created-before:1970-01-01", MatchMode::Exact)
                .unwrap()
                .matches(&e, SystemTime::now())
        );
    }

    #[test]
    fn changed_boundaries_are_strict_precise_and_saved_query_safe() {
        let time = parse_absolute_time("2026-01-01T00:00:00.5Z").unwrap();
        let mut e = entry("file.txt", false, 1, Some(time));
        for mode in MatchMode::ALL {
            let q = Query::parse(
                "Changed-after:2026-01-01T03:00:00.25+03:00 changed-before:2026-01-01T00:00:00.75Z",
                mode,
            )
            .unwrap();
            assert!(!q.matches(&e, SystemTime::now()));
            e.changed = Some(time);
            assert!(q.matches(&e, SystemTime::now()));
            assert_eq!(Query::parse(&q.to_expression(), mode).unwrap(), q);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&q).unwrap()).unwrap(),
                q
            );
            for field in ["changed-after", "changed-before"] {
                assert!(
                    !Query::parse(&format!("{field}:2026-01-01T00:00:00.5Z"), mode)
                        .unwrap()
                        .matches(&e, SystemTime::now())
                );
            }
            e.changed = None;
            for value in [
                "bad",
                "2026-13-01",
                "2026-01-01T00:00:00",
                "2026-01-01T25:00:00Z",
            ] {
                assert!(Query::parse(&format!("changed-after:{value}"), mode).is_err());
                assert!(
                    Query {
                        mode,
                        predicates: vec![Predicate::ChangedBefore(value.into())]
                    }
                    .validate()
                    .is_err()
                );
            }
        }
        e.changed = Some(parse_absolute_time("1969-12-31T23:59:59.5Z").unwrap());
        assert!(
            Query::parse("changed-before:1970-01-01", MatchMode::Exact)
                .unwrap()
                .matches(&e, SystemTime::now())
        );
    }

    #[test]
    fn absolute_modified_dates_roundtrip_and_obey_strict_instant_boundaries() {
        let boundary = parse_absolute_time("2026-10-28").unwrap();
        assert_eq!(
            parse_absolute_time("2026-10-28T03:00:00+03:00").unwrap(),
            boundary
        );
        assert_eq!(
            parse_absolute_time("1969-12-31T23:59:59.5Z").unwrap(),
            std::time::UNIX_EPOCH - Duration::from_millis(500)
        );
        for mode in MatchMode::ALL {
            let query = Query::parse("After:2026-10-28 before:2026-10-29", mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            for (modified, expected) in [
                (None, false),
                (Some(boundary), false),
                (boundary.checked_sub(Duration::from_nanos(1)), false),
                (boundary.checked_add(Duration::from_nanos(1)), true),
                (boundary.checked_add(Duration::from_secs(86400)), false),
            ] {
                assert_eq!(
                    query.matches(&entry("a.rs", false, 1, modified), boundary),
                    expected
                );
            }
            let fractional =
                Query::parse("after:1969-12-31T23:59:59.5Z before:1970-01-01", mode).unwrap();
            assert!(fractional.matches(
                &entry(
                    "old",
                    false,
                    1,
                    Some(std::time::UNIX_EPOCH - Duration::from_millis(250))
                ),
                boundary
            ));
        }
        for value in [
            "2026-02-30",
            "2026-1-1",
            "today",
            "2026-10-28T12:00:00",
            "2016-12-31T23:59:60Z",
        ] {
            assert!(
                Query::parse(&format!("after:{value}"), MatchMode::Exact).is_err(),
                "{value}"
            );
            assert!(
                q(vec![Predicate::ModifiedBefore(value.to_string())])
                    .validate()
                    .is_err()
            );
        }
    }

    #[test]
    fn extension_alternatives_preserve_literal_boundaries_case_and_legacy_queries() {
        for mode in MatchMode::ALL {
            let query = Query::parse(r#"exts:".RS, toml, tar.gz""#, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            for (name, directory, expected) in [
                ("main.rs", false, true),
                ("Cargo.toml", false, true),
                ("backup.tar.gz", false, true),
                ("backup.xtar.gz", false, false),
                ("folder.rs", true, false),
                (".rs", false, true),
                ("rs", false, false),
                ("main.rs.old", false, false),
            ] {
                assert_eq!(
                    query.matches(&entry(name, directory, 1, None), SystemTime::UNIX_EPOCH),
                    expected,
                    "{name}"
                );
            }
            let query = Query::parse("case:sensitive exts:RS,TOML", mode).unwrap();
            assert!(query.matches(&entry("main.RS", false, 1, None), SystemTime::UNIX_EPOCH));
            assert!(!query.matches(&entry("main.rs", false, 1, None), SystemTime::UNIX_EPOCH));
        }
        let legacy = Query::parse("ext:rs,toml", MatchMode::Exact).unwrap();
        assert_eq!(
            Query::parse(&legacy.to_expression(), legacy.mode).unwrap(),
            legacy
        );
        assert!(legacy.matches(
            &entry("literal.rs,toml", false, 1, None),
            SystemTime::UNIX_EPOCH
        ));
        assert!(!legacy.matches(&entry("main.rs", false, 1, None), SystemTime::UNIX_EPOCH));
        for expression in [
            "exts:",
            "exts:rs,",
            "exts:,rs",
            "exts:rs,,toml",
            "exts:..rs",
            "exts:rs,*",
            "exts:rs,a/b",
        ] {
            assert!(
                Query::parse(expression, MatchMode::Exact).is_err(),
                "{expression}"
            );
        }
        for values in [
            vec![],
            vec!["rs,toml".into()],
            vec!["rs".into(), String::new()],
        ] {
            assert!(
                q(vec![Predicate::ExtensionsAny(values)])
                    .validate()
                    .is_err()
            );
        }
    }

    #[test]
    fn negative_metadata_text_queries_preserve_modes_case_and_serialization() {
        for mode in MatchMode::ALL {
            let query =
                Query::parse(r#"notname:"test draft" notpath:"vendor files""#, mode).unwrap();
            assert!(!query.requires_content());
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            let sensitive = Query::parse("case:sensitive notname:Draft", mode).unwrap();
            assert!(sensitive.matches(&entry("draft.txt", false, 1, None), SystemTime::UNIX_EPOCH));
            assert!(
                !sensitive.matches(&entry("Draft.txt", false, 1, None), SystemTime::UNIX_EPOCH)
            );
        }
        for (mode, pattern, excluded, retained) in [
            (MatchMode::Exact, "draft", "old-draft.txt", "dr-a-ft.txt"),
            (MatchMode::Fuzzy, "drft", "dr-a-ft.txt", "done.txt"),
            (MatchMode::Regex, "^draft", "draft.txt", "old-draft.txt"),
        ] {
            let query = Query::parse(&format!("notname:{pattern}"), mode).unwrap();
            assert!(!query.matches(&entry(excluded, false, 1, None), SystemTime::UNIX_EPOCH));
            assert!(query.matches(&entry(retained, false, 1, None), SystemTime::UNIX_EPOCH));
        }
        for expression in ["notname:", "notpath:", "notname:[", "notpath:["] {
            assert!(Query::parse(expression, MatchMode::Regex).is_err());
        }
    }

    #[test]
    fn case_sensitive_query_controls_text_masks_extensions_and_saved_filters() {
        for mode in MatchMode::ALL {
            for (expression, name, expected) in [
                ("case:sensitive name:Doc", "Doc.rs", true),
                ("case:sensitive name:Doc", "doc.rs", false),
                ("case:insensitive name:Doc", "doc.rs", true),
                ("case:sensitive ext:RS", "main.RS", true),
                ("case:sensitive ext:RS", "main.rs", false),
                ("case:sensitive glob:*.RS", "main.rs", false),
                ("case:sensitive exclude:*.RS", "main.rs", true),
                ("case:sensitive exclude:*.RS", "main.RS", false),
                (
                    r#"case:sensitive filter:"^Read && .TXT$ | 'Other""#,
                    "Readme.TXT",
                    true,
                ),
                (
                    r#"case:sensitive filter:"^Read && .TXT$ | 'Other""#,
                    "readme.TXT",
                    false,
                ),
                (
                    r#"case:sensitive filter:"^Read && .TXT$ | 'Other""#,
                    "Other",
                    true,
                ),
                (
                    r#"case:sensitive filter:"^Read && .TXT$ | 'Other""#,
                    "other",
                    false,
                ),
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
                assert_eq!(
                    query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                    expected,
                    "{mode:?}: {expression}: {name}"
                );
            }
        }
        for expression in ["case:", "case:automatic", "case:sensitive case:insensitive"] {
            assert!(Query::parse(expression, MatchMode::Exact).is_err());
        }
        assert!(
            q(vec![
                Predicate::CaseSensitive(true),
                Predicate::CaseSensitive(false)
            ])
            .validate()
            .is_err()
        );
    }

    #[test]
    fn empty_query_type_parses_saves_and_activates_observation() {
        for mode in MatchMode::ALL {
            let query =
                Query::parse("case:smart name:lower type:EMPTY types:empty,folders", mode).unwrap();
            assert!(!query.requires_content());
            assert!(!query.case_sensitive());
            assert_eq!(query.predicates[2], Predicate::Empty);
            assert_eq!(query.chips()[2], "type:empty");
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
    }

    #[test]
    fn type_alternatives_parse_validate_and_preserve_conjunctions() {
        for mode in MatchMode::ALL {
            let query = Query::parse(r#"case:smart name:lower types:"REGULAR-FILE, link, PIPE, char, block, dir, doc, exec""#, mode).unwrap();
            assert!(!query.requires_content());
            assert!(!query.case_sensitive());
            assert_eq!(
                query.chips()[2],
                "types:regular,symlink,fifo,char-device,block-device,folders,docs,executable"
            );
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            let query =
                Query::parse("types:fifo,socket types:regular,symlink type:file", mode).unwrap();
            assert_eq!(query.predicates.len(), 3);
            assert!(matches!(&query.predicates[0], Predicate::TypesAny(types) if types.len() == 2));
            assert!(matches!(&query.predicates[1], Predicate::TypesAny(types) if types.len() == 2));
            assert_eq!(
                Query::parse("type:fifo type:socket", mode)
                    .unwrap()
                    .predicates
                    .len(),
                2
            );
            for expression in [
                "types:",
                "types:,file",
                "types:file,",
                "types:file,,socket",
                "types:file,unknown",
            ] {
                assert!(Query::parse(expression, mode).is_err(), "{expression}");
            }
            assert!(Query::parse(&format!("types:{}", vec!["fifo"; 64].join(",")), mode).is_ok());
            assert!(Query::parse(&format!("types:{}", vec!["fifo"; 65].join(",")), mode).is_err());
            assert!(
                Query {
                    predicates: vec![Predicate::TypesAny(vec![])],
                    mode
                }
                .validate()
                .is_err()
            );
            assert!(
                Query {
                    predicates: vec![Predicate::TypesAny(vec![TypeCondition::File; 65])],
                    mode
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn special_file_query_aliases_round_trip_without_content_or_case_effects() {
        for mode in MatchMode::ALL {
            for (alias, kind) in [
                ("fifo", SpecialFileType::Fifo),
                ("pipe", SpecialFileType::Fifo),
                ("socket", SpecialFileType::Socket),
                ("block", SpecialFileType::BlockDevice),
                ("block-device", SpecialFileType::BlockDevice),
                ("char", SpecialFileType::CharDevice),
                ("char-device", SpecialFileType::CharDevice),
            ] {
                let query = Query::parse(
                    &format!("case:smart name:lower type:{}", alias.to_uppercase()),
                    mode,
                )
                .unwrap();
                assert!(!query.requires_content());
                assert!(!query.case_sensitive());
                assert_eq!(query.predicates[2], Predicate::SpecialFileType(kind));
                assert_eq!(query.chips()[2], format!("type:{}", kind.label()));
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
        }
    }

    #[test]
    fn exact_name_query_is_literal_and_mode_independent() {
        for mode in MatchMode::ALL {
            for name in [
                "report[1]*?.txt",
                "two words.txt",
                "line\nname.txt",
                "ёж.文",
                " a ",
            ] {
                let query = Query {
                    predicates: vec![Predicate::NameExact(name.into())],
                    mode,
                };
                query.validate().unwrap();
                assert!(!query.requires_content());
                assert!(query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH));
                assert!(query.matches(&entry(name, true, 0, None), SystemTime::UNIX_EPOCH));
                for other in [
                    format!("old-{name}"),
                    format!("{name}.bak"),
                    format!("{name}\n"),
                ] {
                    assert!(!query.matches(&entry(&other, false, 1, None), SystemTime::UNIX_EPOCH));
                }
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
                let negative = Query {
                    predicates: vec![Predicate::NameNotExact(name.into())],
                    mode,
                };
                negative.validate().unwrap();
                assert!(!negative.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH));
                assert!(negative.matches(
                    &entry(&format!("old-{name}"), false, 1, None),
                    SystemTime::UNIX_EPOCH
                ));
                assert_eq!(
                    Query::parse(&negative.to_expression(), mode).unwrap(),
                    negative
                );
            }
            assert!(Query::parse("nameexact:[", mode).is_ok());
            assert!(Query::parse("nameexact:", mode).is_err());
            assert!(Query::parse("notnameexact:", mode).is_err());
            assert!(
                Query {
                    predicates: vec![Predicate::NameExact(String::new())],
                    mode
                }
                .validate()
                .is_err()
            );
            assert!(
                Query {
                    predicates: vec![Predicate::NameNotExact(String::new())],
                    mode
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn exact_name_query_case_policy_treats_regex_syntax_as_literal() {
        for mode in MatchMode::ALL {
            for (expression, candidate, expected) in [
                ("nameexact:Résumé.txt", "résumé.txt", true),
                ("case:sensitive nameexact:Résumé.txt", "résumé.txt", false),
                ("case:smart nameexact:Résumé.txt", "résumé.txt", false),
                ("case:smart nameexact:résumé.txt", "Résumé.txt", true),
                (r"case:smart nameexact:\S", r"\s", false),
                ("case:smart notnameexact:Report.txt", "report.txt", true),
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert_eq!(
                    query.matches(&entry(candidate, false, 1, None), SystemTime::UNIX_EPOCH),
                    expected,
                    "{mode:?}: {expression}"
                );
            }
        }
    }

    #[test]
    fn any_name_query_groups_repeated_terms_without_changing_name_and() {
        for mode in MatchMode::ALL {
            let query = Query::parse(
                r#"anyname:error ext:txt anyname:"warning issued" notname:old"#,
                mode,
            )
            .unwrap();
            assert!(!query.requires_content());
            assert_eq!(
                query.predicates[0],
                Predicate::NameAny(vec!["error".into(), "warning issued".into()])
            );
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            let and = Query::parse("name:error name:warn", mode).unwrap();
            assert_eq!(and.predicates.len(), 2);
            assert!(
                !and.predicates
                    .iter()
                    .any(|p| matches!(p, Predicate::NameAny(_)))
            );
        }
        assert!(Query::parse("anyname:", MatchMode::Exact).is_err());
        assert!(Query::parse("anyname:valid anyname:[", MatchMode::Regex).is_err());
        assert!(q(vec![Predicate::NameAny(vec![])]).validate().is_err());
        assert!(
            q(vec![Predicate::NameAny(vec![String::new()])])
                .validate()
                .is_err()
        );
        assert!(
            q(vec![
                Predicate::NameAny(vec!["a".into()]),
                Predicate::NameAny(vec!["b".into()])
            ])
            .validate()
            .is_err()
        );
        assert!(Query::parse(&vec!["anyname:a"; 64].join(" "), MatchMode::Exact).is_ok());
        assert!(Query::parse(&vec!["anyname:a"; 65].join(" "), MatchMode::Exact).is_err());
        assert!(
            q(vec![Predicate::NameAny(vec!["a".repeat(64 * 1024)])])
                .validate()
                .is_ok()
        );
        assert!(
            q(vec![Predicate::NameAny(vec!["a".repeat(64 * 1024 + 1)])])
                .validate()
                .is_err()
        );
        assert!(
            q(vec![Predicate::NameAny(vec![
                "a".repeat(32 * 1024),
                "b".repeat(32 * 1024 + 1)
            ])])
            .validate()
            .is_err()
        );
    }

    #[test]
    fn any_name_query_respects_modes_literals_and_independent_groups() {
        for (mode, expression, name, expected) in [
            (
                MatchMode::Exact,
                "anyname:invoice anyname:receipt",
                "invoice-2026.pdf",
                true,
            ),
            (
                MatchMode::Exact,
                "anyname:invoice anyname:receipt",
                "inv-oice.pdf",
                false,
            ),
            (
                MatchMode::Fuzzy,
                "anyname:invoice anyname:receipt",
                "inv-oice.pdf",
                true,
            ),
            (
                MatchMode::Regex,
                "anyname:^invoice anyname:^receipt",
                "old-invoice.pdf",
                false,
            ),
            (
                MatchMode::Regex,
                "anyname:^invoice anyname:^receipt",
                "receipt-2026.pdf",
                true,
            ),
            (
                MatchMode::Exact,
                r#"anyname:"a;b|c" anyname:"d,e""#,
                "b.txt",
                false,
            ),
            (
                MatchMode::Exact,
                r#"anyname:"a;b|c" anyname:"d,e""#,
                "a;b|c.txt",
                true,
            ),
            (
                MatchMode::Exact,
                "case:smart anyname:invoice anyname:RECEIPT",
                "Invoice.pdf",
                false,
            ),
            (
                MatchMode::Exact,
                "case:insensitive anyname:invoice anyname:RECEIPT",
                "Receipt.pdf",
                true,
            ),
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(
                query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                expected,
                "{mode:?}: {expression}: {name}"
            );
        }
        let query = Query::parse(
            "anyname:invoice content:ready anyname:receipt anycontent:error anycontent:warn",
            MatchMode::Exact,
        )
        .unwrap();
        assert!(matches!(&query.predicates[0], Predicate::NameAny(values) if values.len() == 2));
        assert!(matches!(&query.predicates[2], Predicate::ContentAny(values) if values.len() == 2));
        assert_eq!(query.chips()[0], "anyname:(\"invoice\" OR \"receipt\")");
        assert_eq!(
            Query::parse(&query.to_expression(), query.mode).unwrap(),
            query
        );
    }

    #[test]
    fn any_name_query_smart_case_reads_all_member_literals() {
        for (values, sensitive) in [
            (vec![r"error\S*", "warn"], false),
            (vec![r"\p{Lu}"], true),
            (vec!["error", "WARN"], true),
            (vec![r"error\S*", r"warn\x{41}"], true),
        ] {
            let query = Query {
                predicates: vec![
                    Predicate::SmartCase,
                    Predicate::NameAny(values.into_iter().map(str::to_string).collect()),
                ],
                mode: MatchMode::Regex,
            };
            query.validate().unwrap();
            assert_eq!(query.case_sensitive(), sensitive);
            assert_eq!(
                Query::parse(&query.to_expression(), query.mode).unwrap(),
                query
            );
        }
    }

    #[test]
    fn any_content_query_groups_repeated_terms_without_changing_content_and() {
        for mode in MatchMode::ALL {
            let query = Query::parse(
                r#"anycontent:error ext:txt anycontent:"warning issued" content:ready"#,
                mode,
            )
            .unwrap();
            assert!(query.requires_content());
            assert_eq!(
                query.predicates[0],
                Predicate::ContentAny(vec!["error".into(), "warning issued".into()])
            );
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            let and = Query::parse("content:error content:warn", mode).unwrap();
            assert_eq!(and.predicates.len(), 2);
            assert!(
                !and.predicates
                    .iter()
                    .any(|p| matches!(p, Predicate::ContentAny(_)))
            );
        }
        assert!(Query::parse("anycontent:", MatchMode::Exact).is_err());
        assert!(Query::parse("anycontent:valid anycontent:[", MatchMode::Regex).is_err());
        assert!(q(vec![Predicate::ContentAny(vec![])]).validate().is_err());
        assert!(
            q(vec![Predicate::ContentAny(vec![String::new()])])
                .validate()
                .is_err()
        );
        assert!(
            q(vec![
                Predicate::ContentAny(vec!["a".into()]),
                Predicate::ContentAny(vec!["b".into()])
            ])
            .validate()
            .is_err()
        );
        assert!(Query::parse(&vec!["anycontent:a"; 64].join(" "), MatchMode::Exact).is_ok());
        assert!(Query::parse(&vec!["anycontent:a"; 65].join(" "), MatchMode::Exact).is_err());
        assert!(
            q(vec![Predicate::ContentAny(vec!["a".repeat(64 * 1024)])])
                .validate()
                .is_ok()
        );
        assert!(
            q(vec![Predicate::ContentAny(vec!["a".repeat(64 * 1024 + 1)])])
                .validate()
                .is_err()
        );
        assert!(
            q(vec![Predicate::ContentAny(vec![
                "a".repeat(32 * 1024),
                "b".repeat(32 * 1024 + 1)
            ])])
            .validate()
            .is_err()
        );
    }

    #[test]
    fn any_content_query_smart_case_reads_all_member_literals() {
        for (values, sensitive) in [
            (vec![r"error\S*", "warn"], false),
            (vec![r"\p{Lu}"], true),
            (vec!["error", "WARN"], true),
            (vec![r"error\S*", r"warn\x{41}"], true),
        ] {
            let query = Query {
                predicates: vec![
                    Predicate::SmartCase,
                    Predicate::ContentAny(values.into_iter().map(str::to_string).collect()),
                ],
                mode: MatchMode::Regex,
            };
            query.validate().unwrap();
            assert_eq!(query.case_sensitive(), sensitive);
            assert_eq!(
                Query::parse(&query.to_expression(), query.mode).unwrap(),
                query
            );
        }
    }

    #[test]
    fn hidden_search_query_preserves_modes_aliases_conflicts_and_saved_conditions() {
        for mode in MatchMode::ALL {
            for expression in [
                "hidden:include",
                "hidden:exclude",
                "hidden:only",
                "hidden:ON",
                "hidden:off",
                "hidden:true hidden:include",
                "hidden:false hidden:exclude",
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
            for expression in [
                "hidden:",
                "hidden:maybe",
                "hidden:include hidden:only",
                "hidden:on hidden:off",
            ] {
                assert!(Query::parse(expression, mode).is_err());
            }
            for (condition, hidden, visible) in [
                ("hidden:include", true, true),
                ("hidden:exclude", false, true),
                ("hidden:only", true, false),
            ] {
                let query = Query::parse(condition, mode).unwrap();
                assert_eq!(
                    query.matches(&entry(".env", false, 1, None), SystemTime::UNIX_EPOCH),
                    hidden
                );
                assert_eq!(
                    query.matches(&entry("plain.txt", false, 1, None), SystemTime::UNIX_EPOCH),
                    visible
                );
            }
        }
        assert!(
            !Query::parse("case:smart hidden:ONLY content:ready", MatchMode::Exact)
                .unwrap()
                .case_sensitive()
        );
    }

    #[test]
    fn smart_case_query_preserves_case_modes_structural_fields_and_saved_queries() {
        for mode in MatchMode::ALL {
            for (expression, sensitive) in [
                ("case:smart content:ready", false),
                ("case:SMART content:READY", true),
                ("case:smart contentword:Äpfel", true),
                ("case:smart contentline:готово", false),
                ("case:smart exts:rs,TXT", true),
                ("case:smart glob:*.txt", false),
                ("case:smart excludedir:Vendor/**", true),
                (
                    "case:smart type:IMAGE perm:UR+ after:2026-01-01T00:00:00Z content:ready",
                    false,
                ),
                ("case:smart case:smart content:ready", false),
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert_eq!(query.case_sensitive(), sensitive, "{mode:?}: {expression}");
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
            for expression in [
                "case:smart case:sensitive",
                "case:smart case:insensitive",
                "case:sensitive case:smart",
            ] {
                assert!(Query::parse(expression, mode).is_err(), "{expression}");
            }
        }
        assert!(
            !Query::parse("content:READY", MatchMode::Exact)
                .unwrap()
                .case_sensitive()
        );
        assert!(
            !Query::parse("case:insensitive content:READY", MatchMode::Exact)
                .unwrap()
                .case_sensitive()
        );
    }

    #[test]
    fn smart_case_query_regex_analysis_uses_literals_not_syntax_names() {
        for (pattern, sensitive) in [
            (r"ready\S*", false),
            (r"ready\p{Lu}", false),
            (r"\p{Lu}", true),
            (r"[a-z]+", false),
            (r"[A-Z]+", true),
            (r"[a-z&&[^B]]+", true),
            (r"[[:upper:]]", true),
            (r"(?P<UPPER>ready)", false),
            (r"\x{41}bc", true),
            (r"\x{61}bc", false),
            (r"(?i:READY)", true),
            ("(?x)# UPPER\nready", false),
            (r"ready\\S", true),
        ] {
            let query = Query {
                predicates: vec![
                    Predicate::SmartCase,
                    Predicate::ContentContains(pattern.into()),
                ],
                mode: MatchMode::Regex,
            };
            query.validate().unwrap();
            assert_eq!(query.case_sensitive(), sensitive, "{pattern}");
            assert_eq!(
                Query::parse(&query.to_expression(), MatchMode::Regex).unwrap(),
                query
            );
        }
        let literal = Query {
            predicates: vec![Predicate::SmartCase, Predicate::ContentWord(r"\S".into())],
            mode: MatchMode::Regex,
        };
        assert!(literal.case_sensitive());
        assert!(Query::parse("case:smart content:(", MatchMode::Regex).is_err());
    }

    #[test]
    fn content_line_query_preserves_empty_literals_spaces_and_saved_conditions() {
        for mode in MatchMode::ALL {
            for expression in [
                "contentline:",
                "notcontentline:",
                "CONTENTLINE:",
                r#"contentline:" cat " notcontentline:["#,
                r#"contentline:"""#,
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert!(query.requires_content());
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
        }
        for value in ["one\ntwo", "one\rtwo", &"a".repeat(1 << 18)] {
            assert!(
                q(vec![Predicate::ContentLine(value.into())])
                    .validate()
                    .is_err()
            );
            assert!(
                q(vec![Predicate::ContentLineExcludes(value.into())])
                    .validate()
                    .is_err()
            );
        }
        for expression in ["content:", "contentword:", "name:", "notcontent:"] {
            assert!(Query::parse(expression, MatchMode::Exact).is_err());
        }
    }

    #[test]
    fn content_word_query_preserves_literals_modes_and_saved_predicates() {
        for mode in MatchMode::ALL {
            for expression in [
                "contentword:cat",
                "notcontentword:cat",
                r#"contentword:"cat dog" notcontentword:["#,
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert!(query.requires_content());
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
        }
        for predicate in [
            Predicate::ContentWord(String::new()),
            Predicate::ContentWordExcludes(String::new()),
            Predicate::ContentWord("a".repeat(1 << 18)),
        ] {
            assert!(q(vec![predicate]).validate().is_err());
        }
        assert!(Query::parse("contentword:", MatchMode::Exact).is_err());
    }

    #[test]
    fn negative_content_query_preserves_syntax_and_requires_inspection() {
        for mode in MatchMode::ALL {
            let query = Query::parse(r#"content:allowed notcontent:"not expected""#, mode).unwrap();
            assert!(query.requires_content());
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            assert!(!query.matches(&entry("file.txt", false, 1, None), SystemTime::UNIX_EPOCH));
        }
        assert!(Query::parse("notcontent:[", MatchMode::Regex).is_err());
        assert!(Query::parse("notcontent:", MatchMode::Exact).is_err());
    }

    #[test]
    fn archive_scope_query_round_trips_and_rejects_conflicting_controls() {
        for mode in MatchMode::ALL {
            for expression in [
                "archives:off",
                "archives:on",
                "archives:FALSE",
                "archives:TRUE",
                "archives:off archives:off",
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
        }
        for expression in ["archives:", "archives:maybe", "archives:on archives:off"] {
            assert!(Query::parse(expression, MatchMode::Exact).is_err());
        }
        assert!(
            q(vec![
                Predicate::SearchArchives(false),
                Predicate::SearchArchives(true)
            ])
            .validate()
            .is_err()
        );
    }

    #[test]
    fn exact_size_file_query_excludes_folders_and_preserves_legacy_lower_bound() {
        for mode in MatchMode::ALL {
            for expression in ["type:file size:=0", "type:FILES size:=1KB"] {
                let query = Query::parse(expression, mode).unwrap();
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
                let expected_size = if expression.ends_with('0') { 0 } else { 1024 };
                for (name, directory, size, expected) in [
                    ("empty", false, expected_size, true),
                    ("folder", true, expected_size, false),
                    ("photo.jpg", false, expected_size, true),
                    ("file", false, expected_size + 1, false),
                ] {
                    assert_eq!(
                        query.matches(&entry(name, directory, size, None), SystemTime::UNIX_EPOCH),
                        expected,
                        "{expression}: {name}"
                    );
                }
            }
        }
        assert_eq!(
            Query::parse("size:1KB", MatchMode::Exact)
                .unwrap()
                .predicates,
            [Predicate::MinSize(1024)]
        );
        for expression in ["size:=", "size:==1", "size:=-1", "size:=1unknown"] {
            assert!(
                Query::parse(expression, MatchMode::Exact).is_err(),
                "{expression}"
            );
        }
        for size in [9_007_199_254_740_993, u64::MAX] {
            let query = Query::parse(&format!("size:={size}B"), MatchMode::Exact).unwrap();
            assert_eq!(
                query.predicates,
                [Predicate::MinSize(size), Predicate::MaxSize(size)]
            );
            assert_eq!(
                Query::parse(&query.to_expression(), query.mode).unwrap(),
                query
            );
            assert!(query.matches(&entry("exact", false, size, None), SystemTime::UNIX_EPOCH));
            assert!(!query.matches(
                &entry("below", false, size - 1, None),
                SystemTime::UNIX_EPOCH
            ));
        }
        assert!(Query::parse("size:=18446744073709551616B", MatchMode::Exact).is_err());
        assert!(Query::parse("size:=18446744073709551615KB", MatchMode::Exact).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn absolute_directory_query_masks_keep_lexical_root_and_member_boundaries() {
        let root = Path::new("/scope");
        for (pattern, path, expected) in [
            ("/scope/vendor/cache", "/scope/vendor/cache", true),
            ("/scope/vendor/cache", "/scope/other/vendor/cache", false),
            ("/scope/vendor/*/cache", "/scope/vendor/a/cache", true),
            ("/scope/vendor/*/cache", "/scope/vendor/a/b/cache", false),
            ("/scope/vendor/**/cache", "/scope/vendor/cache", true),
            ("/scope/vendor/**/cache", "/scope/vendor/a/b/cache", true),
            ("/scope/vendor/**", "/scope/vendor", true),
            ("/scope/vendor/cache/", "/scope/vendor/cache", true),
            ("/scope/**", "/scope", false),
            ("/outside/cache", "/outside/cache", false),
        ] {
            assert_eq!(
                compile_directory_mask(pattern, false)
                    .unwrap()
                    .matches_directory(Path::new(path), root),
                expected,
                "{pattern}: {path}"
            );
        }
        assert!(
            !compile_directory_mask("/scope/**/cache", false)
                .unwrap()
                .matches_directory(Path::new("scope/vendor/cache"), Path::new(""))
        );
        assert!(
            !compile_directory_mask("/scope/CACHE", true)
                .unwrap()
                .matches_directory(Path::new("/scope/cache"), root)
        );
        for mode in MatchMode::ALL {
            let query = Query::parse(r#"excludedir:"/scope/build cache/**""#, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
        for value in [
            "/",
            "//scope/cache",
            "/scope/../cache",
            "/scope/./cache",
            "/scope//cache",
            "/scope/**cache",
        ] {
            assert!(compile_directory_mask(value, false).is_err(), "{value}");
        }
    }

    #[test]
    fn relative_directory_query_masks_are_component_aware_and_rooted() {
        let root = Path::new("/scope");
        for (pattern, path, expected) in [
            ("vendor/cache", "/scope/vendor/cache", true),
            ("vendor/cache", "/scope/other/vendor/cache", false),
            ("vendor/*/cache", "/scope/vendor/a/cache", true),
            ("vendor/*/cache", "/scope/vendor/a/b/cache", false),
            ("vendor/**/cache", "/scope/vendor/cache", true),
            ("vendor/**/cache", "/scope/vendor/a/b/cache", true),
            ("**/cache", "/scope/cache", true),
            ("**/cache", "/scope/a/b/cache", true),
            ("vendor/**", "/scope/vendor", true),
            ("vendor/**", "/scope/vendor/a/b", true),
            ("vendor/**/**/cache", "/scope/vendor/cache", true),
            ("./cache", "/scope/cache", true),
            ("./cache", "/scope/other/cache", false),
            ("vendor/cache/", "/scope/vendor/cache", true),
            ("vendor/cach?", "/scope/vendor/caché", true),
            ("vendor/[cache]", "/scope/vendor/cache", false),
            ("vendor/[cache]", "/scope/vendor/[cache]", true),
            ("vendor/*", "/scope/vendor/line\ncache", true),
            ("**/cache", "/outside/cache", false),
            ("./**", "/scope", false),
        ] {
            let mask = compile_directory_mask(pattern, false).unwrap();
            assert_eq!(
                mask.matches_directory(Path::new(path), root),
                expected,
                "{pattern}: {path}"
            );
        }
        let mask = compile_directory_mask("vendor/CACHE", true).unwrap();
        assert!(!mask.matches_directory(Path::new("/scope/vendor/cache"), root));
        assert!(
            compile_directory_mask("vendor/CACHE", false)
                .unwrap()
                .matches_directory(Path::new("/scope/vendor/cache"), root)
        );
        for mode in MatchMode::ALL {
            for expression in [
                "excludedir:vendor/cache",
                "excludedir:vendor/**/cache",
                "excludedir:./cache",
                r#"excludedir:"build cache/**/out""#,
            ] {
                let query = Query::parse(expression, mode).unwrap();
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
        }
        for value in [
            "//cache",
            "./",
            "../cache",
            "vendor/../cache",
            "vendor/./cache",
            "vendor//cache",
            "vendor/cache//",
            "vendor/**cache",
            "vendor/c**ache",
        ] {
            assert!(compile_directory_mask(value, false).is_err(), "{value}");
        }
    }

    #[test]
    fn directory_exclusion_query_preserves_files_case_and_saved_masks() {
        for mode in MatchMode::ALL {
            let query = Query::parse(r#"excludedir:*.tmp excludedir:"build cache""#, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            for (name, directory, expected) in [
                ("cache.tmp", true, false),
                ("cache.TMP", true, false),
                ("cache.tmp", false, true),
                ("build cache", true, false),
                ("build cache", false, true),
                ("cache.tmp.txt", true, true),
            ] {
                assert_eq!(
                    query.matches(&entry(name, directory, 1, None), SystemTime::UNIX_EPOCH),
                    expected,
                    "{mode:?}: {name}"
                );
            }
            let sensitive = Query::parse("case:sensitive excludedir:*.TMP", mode).unwrap();
            assert!(sensitive.matches(&entry("cache.tmp", true, 1, None), SystemTime::UNIX_EPOCH));
            assert!(!sensitive.matches(&entry("cache.TMP", true, 1, None), SystemTime::UNIX_EPOCH));
        }
        for value in ["", "target/../cache", &"ab".repeat(1 << 17)] {
            assert!(
                q(vec![Predicate::ExcludeDirGlob(value.into())])
                    .validate()
                    .is_err()
            );
        }
        assert!(Query::parse("excludedir:", MatchMode::Exact).is_err());
    }

    #[test]
    fn exclude_file_query_preserves_masks_case_and_saved_semantics() {
        for mode in MatchMode::ALL {
            let query =
                Query::parse(r#"excludefile:*.tmp excludefile:"draft ?.txt""#, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            for (name, directory, expected) in [
                ("data.tmp", false, false),
                ("data.TMP", false, false),
                ("data.tmp", true, true),
                ("data.tmp.txt", false, true),
                ("draft é.txt", false, false),
                ("draft ab.txt", false, true),
                ("line\n.tmp", false, false),
                ("keep.txt", false, true),
            ] {
                let hit = query.matches(&entry(name, directory, 1, None), SystemTime::UNIX_EPOCH);
                assert_eq!(hit, expected, "{mode:?}: {name}");
            }
            let query = Query::parse("case:sensitive excludefile:*.TMP", mode).unwrap();
            assert!(query.matches(&entry("data.tmp", false, 1, None), SystemTime::UNIX_EPOCH));
            assert!(!query.matches(&entry("data.TMP", false, 1, None), SystemTime::UNIX_EPOCH));
        }
        for value in ["", "dir/*.tmp", &"ab".repeat(1 << 17)] {
            assert!(
                q(vec![Predicate::ExcludeFileGlob(value.into())])
                    .validate()
                    .is_err()
            );
        }
        assert!(Query::parse("excludefile:", MatchMode::Exact).is_err());
        let literal = Query::parse("excludefile:[ab].tmp", MatchMode::Regex).unwrap();
        assert!(literal.matches(&entry("a.tmp", false, 1, None), SystemTime::UNIX_EPOCH));
        assert!(!literal.matches(&entry("[ab].tmp", false, 1, None), SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn exclusion_query_round_trips_and_rejects_invalid_masks() {
        for mode in MatchMode::ALL {
            let query = Query::parse(r#"exclude:*.bak exclude:"build cache""#, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
        for expression in ["exclude:", "exclude:target/*"] {
            assert!(Query::parse(expression, MatchMode::Exact).is_err());
        }
        assert!(
            q(vec![Predicate::ExcludeGlob(String::new())])
                .validate()
                .is_err()
        );
        assert!(
            q(vec![Predicate::ExcludeGlob("ab".repeat(1 << 17))])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn depth_query_parses_ranges_and_preserves_saved_conditions() {
        for expression in ["depth:<=1", "depth:>=2", "depth:0", "depth:1..3"] {
            let query = Query::parse(expression, MatchMode::Exact).unwrap();
            assert_eq!(
                Query::parse(&query.to_expression(), query.mode).unwrap(),
                query
            );
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
        assert_eq!(
            Query::parse("depth:2", MatchMode::Exact)
                .unwrap()
                .predicates,
            [Predicate::MinDepth(2), Predicate::MaxDepth(2)]
        );
        for expression in [
            "depth:",
            "depth:-1",
            "depth:+1",
            "depth:1.5",
            "depth:3..1",
            "depth:1..",
            "depth:<=",
            "depth:9999999999999999999999999",
        ] {
            assert!(
                Query::parse(expression, MatchMode::Exact).is_err(),
                "{expression}"
            );
        }
    }

    #[test]
    fn glob_query_round_trips_and_uses_whole_unicode_names() {
        for mode in MatchMode::ALL {
            for (pattern, name, expected) in [
                ("*.RS", "main.rs", true),
                ("*.rs", "main.rs.bak", false),
                ("report", "myreport", false),
                ("report", "REPORT", true),
                ("caf?.txt", "café.txt", true),
                ("caf?.txt", "caf.txt", false),
                ("[x].txt", "[x].txt", true),
                ("[x].txt", "x.txt", false),
                ("report *.pdf", "report annual.pdf", true),
                ("*", "資料", true),
                ("?.txt", "İ.txt", true),
                ("*.txt", "line\nbreak.txt", true),
            ] {
                let query = q(vec![Predicate::NameGlob(pattern.into())]);
                let query = Query { mode, ..query };
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                let restored: Query =
                    serde_json::from_str(&serde_json::to_string(&query).unwrap()).unwrap();
                assert_eq!(
                    restored.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                    expected,
                    "{pattern}: {name}"
                );
            }
        }
        assert!(
            q(vec![Predicate::NameGlob("ab".repeat(1 << 17))])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn extension_query_matches_literal_file_suffixes_in_every_mode() {
        for mode in [MatchMode::Exact, MatchMode::Fuzzy, MatchMode::Regex] {
            let query = Query::parse("ext:.TAR.GZ", mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            let restored: Query =
                serde_json::from_str(&serde_json::to_string(&query).unwrap()).unwrap();
            for (name, dir, expected) in [
                ("backup.TAR.GZ", false, true),
                ("backup.tar.gz", true, false),
                ("backup.tar.gz.old", false, false),
                ("backup.xtar.gz", false, false),
                ("tar.gz", false, false),
                (".tar.gz", false, true),
            ] {
                assert_eq!(
                    restored.matches(&entry(name, dir, 1, None), SystemTime::UNIX_EPOCH),
                    expected,
                    "{name}"
                );
            }
        }
        let query = q(vec![Predicate::Extension("RS".into())]);
        assert!(query.matches(&entry("main.rs", false, 1, None), SystemTime::UNIX_EPOCH));
        for expression in [
            "ext:",
            "ext:.",
            "ext:tar..gz",
            "ext:*.rs",
            "ext:a/b",
            "ext:tar.",
        ] {
            assert!(
                Query::parse(expression, MatchMode::Exact).is_err(),
                "{expression}"
            );
        }
        assert!(
            q(vec![Predicate::Extension(String::new())])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn conjunctive_filter_export_preserves_expression_and_search_results() {
        for pattern in [
            ".rs$ && !^test | .toml$",
            r#"^x && "a | b""#,
            r#""ab" && "bc""#,
            ".rs$ &&",
        ] {
            let query = from_panel_filter(pattern, &crate::panel::FacetSet::default());
            assert_eq!(
                Query::parse(&query.to_expression(), query.mode).unwrap(),
                query
            );
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            for name in ["main.rs", "test.rs", "test.toml", "x a | b", "abc"] {
                assert_eq!(
                    query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                    crate::name_filter::is_match(pattern, name),
                    "{pattern:?}, {name:?}"
                );
            }
        }
    }

    #[test]
    fn quoted_alternative_export_survives_expression_json_and_search_matching() {
        for pattern in [
            r#""a | b" | .rs$"#,
            r#".rs$ | "^a$""#,
            r#"" a ""#,
            r#".rs$ | "unfinished"#,
        ] {
            let query = from_panel_filter(pattern, &crate::panel::FacetSet::default());
            let parsed = Query::parse(&query.to_expression(), query.mode).unwrap();
            assert_eq!(parsed, query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            for name in ["a | b", "main.rs", "a", "^a$", "x a y"] {
                assert_eq!(
                    parsed.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                    crate::name_filter::is_match(pattern, name)
                );
            }
        }
    }

    #[test]
    fn alternative_pane_filters_roundtrip_and_use_the_shared_search_matcher() {
        for pattern in [
            ".rs$ | .toml$",
            "ab | cd",
            "^é | !backup",
            "'literal | pipe",
        ] {
            let query = from_panel_filter(pattern, &crate::panel::FacetSet::default());
            assert_eq!(
                Query::parse(&query.to_expression(), query.mode).unwrap(),
                query
            );
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            for name in [
                "main.rs",
                "Cargo.toml",
                "a-b",
                "Éclair",
                "backup",
                "literal | pipe",
            ] {
                assert_eq!(
                    query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                    crate::name_filter::is_match(pattern, name),
                    "{pattern:?}, {name:?}"
                );
            }
        }
    }

    #[test]
    fn pane_filter_predicate_roundtrips_all_modes_independently_of_query_mode() {
        for pattern in ["ab", "'a b ", "^i.txt$", "σ$", "''", "^$", "a:b", r"^a\b$"] {
            let query = from_panel_filter(pattern, &crate::panel::FacetSet::default());
            assert_eq!(
                Query::parse(&query.to_expression(), query.mode).unwrap(),
                query
            );
            let json = serde_json::to_string(&query).unwrap();
            assert_eq!(serde_json::from_str::<Query>(&json).unwrap(), query);
        }
        let query = Query::parse(r#"filter:"^a.b$""#, MatchMode::Regex).unwrap();
        assert!(query.matches(&entry("a.b", false, 1, None), SystemTime::UNIX_EPOCH));
        assert!(!query.matches(&entry("axb", false, 1, None), SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn literal_panel_filter_exports_its_original_matching_semantics() {
        let query = from_panel_filter("'a b ", &crate::panel::FacetSet::default());
        assert_eq!(query.mode, MatchMode::Exact);
        assert_eq!(query.predicates, [Predicate::NameFilter("'a b ".into())]);
        assert_eq!(
            from_panel_filter("'", &crate::panel::FacetSet::default()).predicates,
            [Predicate::NameFilter("'".into())]
        );
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

    fn round_trip(query: &Query) -> Query {
        Query::parse(&query.to_expression(), query.mode).unwrap()
    }

    #[test]
    fn name_with_colon_survives_round_trip() {
        let query = from_panel_filter("12:30", &crate::panel::FacetSet::default());
        assert_eq!(round_trip(&query), query);
        let query = Query::parse("name:a:b", MatchMode::Exact).unwrap();
        assert_eq!(
            query.predicates,
            vec![Predicate::NameContains("a:b".into())]
        );
        assert_eq!(round_trip(&query), query);
        let spaced = q(vec![Predicate::NameContains("note: draft".into())]);
        assert_eq!(round_trip(&spaced), spaced);
    }

    #[test]
    fn quoted_token_with_colon_is_name_text() {
        let query = Query::parse("\"note: draft\" 'a:b'", MatchMode::Exact).unwrap();
        assert_eq!(
            query.predicates,
            vec![
                Predicate::NameContains("note: draft".into()),
                Predicate::NameContains("a:b".into()),
            ]
        );
    }

    #[test]
    fn apostrophe_value_survives_round_trip() {
        let query = q(vec![
            Predicate::NameContains("it's".into()),
            Predicate::PathContains("bob's files".into()),
        ]);
        assert_eq!(round_trip(&query), query);
    }

    #[test]
    fn backslash_escapes_quotes_outside_quotes() {
        let query = Query::parse(r#"foo\"bar it\'s"#, MatchMode::Exact).unwrap();
        assert_eq!(
            query.predicates,
            vec![
                Predicate::NameContains("foo\"bar".into()),
                Predicate::NameContains("it's".into()),
            ]
        );
    }

    #[test]
    fn doubled_regex_backslash_survives_expression_round_trip() {
        let query = Query {
            mode: MatchMode::Regex,
            predicates: vec![Predicate::NameContains(r"a\\d".into())],
        };

        assert_eq!(round_trip(&query), query);
        assert_eq!(query.to_expression(), r#""a\\\\d""#);
    }

    #[test]
    fn future_mtime_counts_as_today() {
        let now = SystemTime::now();
        let future = now + Duration::from_secs(3 * SECONDS_PER_DAY);
        let file = entry("a", false, 1, Some(future));
        assert!(q(vec![Predicate::MaxAgeDays(1)]).matches(&file, now));
        assert!(!q(vec![Predicate::MinAgeDays(1)]).matches(&file, now));
    }

    #[test]
    fn strict_size_decimal_bounds_round_exactly_without_float_saturation() {
        for mode in MatchMode::ALL {
            for (value, bytes) in [
                ("9007199254740993.0B", 9_007_199_254_740_993),
                ("18446744073709551614.4B", u64::MAX - 1),
                ("18446744073709551614.5B", u64::MAX),
                ("18446744073709551615.4B", u64::MAX),
                ("0.4999999999999999999999999999999999999B", 0),
                ("0.5B", 1),
                (".5KB", 512),
                ("1.B", 1),
                ("0.0004882812499999999999999999999999999KB", 0),
                ("0.00048828125KB", 1),
                ("0.0000000000000000000000000000000000001TB", 0),
            ] {
                let query = Query::parse(&format!("size:={value}"), mode).unwrap();
                assert_eq!(
                    query.predicates,
                    vec![Predicate::MinSize(bytes), Predicate::MaxSize(bytes)],
                    "{value}"
                );
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
            }
            for value in [
                "18446744073709551615.5B",
                "18446744073709551616.0B",
                "18446744073709551615.0KB",
            ] {
                assert!(
                    Query::parse(&format!("size:={value}"), mode).is_err(),
                    "{value}"
                );
            }
        }
    }

    #[test]
    fn strict_size_upper_bound_never_saturates_to_an_inclusive_match() {
        for mode in MatchMode::ALL {
            assert!(Query::parse(&format!("size:>{}B", u64::MAX), mode).is_err());
            for expression in [
                format!("size:>={}B", u64::MAX),
                format!("size:>{}B", u64::MAX - 1),
                format!("size:<{}B", u64::MAX),
            ] {
                let query = Query::parse(&expression, mode).unwrap();
                assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
                assert_eq!(
                    serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                    query
                );
                let is_upper = !expression.starts_with("size:<");
                assert_eq!(
                    query.matches(&entry("max", false, u64::MAX, None), SystemTime::UNIX_EPOCH),
                    is_upper
                );
                assert_eq!(
                    query.matches(
                        &entry("below", false, u64::MAX - 1, None),
                        SystemTime::UNIX_EPOCH
                    ),
                    !is_upper
                );
            }
        }
    }

    #[test]
    fn strict_size_operators_exclude_the_bound() {
        let now = SystemTime::now();
        let positive = Query::parse("size:>0", MatchMode::Exact).unwrap();
        assert!(!positive.matches(&entry("empty", false, 0, None), now));
        assert!(positive.matches(&entry("one", false, 1, None), now));
        let small = Query::parse("size:<1KB", MatchMode::Exact).unwrap();
        assert!(!small.matches(&entry("k", false, 1024, None), now));
        assert!(small.matches(&entry("k", false, 1023, None), now));
        let inclusive = Query::parse("size:<=1KB", MatchMode::Exact).unwrap();
        assert!(inclusive.matches(&entry("k", false, 1024, None), now));
        assert!(Query::parse("size:<0", MatchMode::Exact).is_err());
    }

    fn parse_error(expression: &str) -> String {
        Query::parse(expression, MatchMode::Exact)
            .unwrap_err()
            .message
    }

    fn parsed(expression: &str) -> Vec<Predicate> {
        Query::parse(expression, MatchMode::Exact)
            .unwrap()
            .predicates
    }

    #[test]
    fn field_names_are_case_insensitive_and_need_a_value() {
        assert_eq!(parsed("NAME:x"), vec![Predicate::NameContains("x".into())]);
        assert_eq!(parsed("Type:docs"), vec![Predicate::Kind(Kind::Document)]);
        assert_eq!(parse_error(":foo"), "unknown query field");
        assert_eq!(parse_error("name:"), "field value is empty");
    }

    #[test]
    fn size_units_accept_fractions_and_binary_aliases() {
        assert_eq!(parsed("size:1.5k"), vec![Predicate::MinSize(1536)]);
        assert_eq!(parsed("size:2kib"), vec![Predicate::MinSize(2048)]);
        assert_eq!(parse_error("size:MB"), "invalid size");
        assert_eq!(parse_error("size:1.2.3K"), "invalid size");
    }

    #[test]
    fn size_ranges_must_be_ordered_and_closed() {
        assert_eq!(parse_error("size:5MB..1MB"), "size range is reversed");
        // Open-ended ranges are not supported; the empty side is no number.
        assert_eq!(parse_error("size:1MB.."), "invalid size");
        assert_eq!(parse_error("size:..5MB"), "invalid size");
    }

    #[test]
    fn date_units_reject_overflow_and_treat_m_as_months() {
        assert_eq!(
            parse_error("date:99999999999999999y"),
            "date age is too large"
        );
        assert_eq!(parse_error("date:3h"), "unknown date unit");
        // `m` means months (30 days), not minutes.
        assert_eq!(parsed("date:<2m"), vec![Predicate::MaxAgeDays(60)]);
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
                Predicate::NameFilter("raw".into()),
                Predicate::Kind(Kind::Image),
                Predicate::MinSize(1 << 20),
                Predicate::MaxAgeDays(7),
                Predicate::MinAgeDays(30),
            ]
        );
    }
}
