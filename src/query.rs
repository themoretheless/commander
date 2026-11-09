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
mod tests;
