use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::Hash;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::SystemTime;

mod loading;
mod masks;
mod smart_select;

#[derive(Clone, Copy, Debug, Default)]
pub struct SelectionMaskOptions {
    pub target: SelectionTarget,
    pub case_sensitive: bool,
    pub ignore_accents: bool,
    pub extensionless_star: bool,
}

/// Restrict group selection without changing the panel's visible-row filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionTarget {
    #[default]
    Both,
    Files,
    Folders,
}
impl SelectionTarget {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Both => "Files and folders",
            Self::Files => "Files",
            Self::Folders => "Folders",
        }
    }
    const fn accepts(self, is_dir: bool) -> bool {
        match self {
            Self::Both => true,
            Self::Files => !is_dir,
            Self::Folders => is_dir,
        }
    }
}

mod dir_size_cache;
mod drag;
mod facet;
mod link_info;
mod listing;
mod listing_job;
mod miller;
pub use miller::Widths as MillerWidths;
mod nav;
mod preview;
mod selection;
mod size_index;
mod sort;
mod view;
mod visit;
mod watcher;

#[cfg(test)]
pub use dir_size_cache::reset_walk_log;
use dir_size_cache::*;
pub use dir_size_cache::{flush_cache, invalidate_size_cache};
pub use drag::DragState;
pub use facet::{FacetSet, KindFacet, facet_matches, filter_is_active};
pub use link_info::{LinkInfo, LinkTargetState};
use listing::ListingState;
use listing_job::{ListingJobController, PendingFocus};
// Re-export preview/visit public API under `crate::panel::...` for stable paths.
use crate::natural_sort::compare as natural_cmp;
#[allow(unused_imports)]
pub use preview::{
    InfoCard, MEASURING, PreviewContent, PreviewIdentity, format_mode,
    image_preview_unavailable_message, make_info, make_preview,
};
use selection::{Focus, SelectionState};
pub use size_index::SizeSnapshot;
use size_index::{SizeIndex, SizeScanInput};
use sort::sort_entries;
use sort::sort_entries_with_sizes;
pub use view::{BrowserMode, ViewConfig};
use view::{ViewSettings, ViewState};
#[allow(unused_imports)]
pub use visit::{
    RecentMatch, RecentMatchMode, RecentOrder, VISITED_CAP, VisitStats, VisitUsage, forget_visit,
    push_visit, rank_visited, rank_visited_with_mode, recent_match_error, recent_paths_within,
    record_visit, restore_visit_snapshot, visit_snapshot, visited_paths,
};
pub use watcher::WATCHER_RETRY_BACKOFF;
use watcher::{DirectoryWatcherState, ReconciliationTicket};

const DEFAULT_VISIBLE_ROWS: usize = 32;

fn visible_window(total: usize, anchor: usize, page_rows: usize) -> std::ops::Range<usize> {
    let rows = page_rows.max(DEFAULT_VISIBLE_ROWS).min(total);
    let start = anchor.min(total.saturating_sub(rows));
    let end = start.saturating_add(rows).min(total);
    start..end
}

/// Why a directory listing is the way it is, so an empty list can be told
/// apart from an unreadable or vanished directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirStatus {
    /// Read succeeded and there are entries.
    Listed,
    /// Read succeeded but the directory is empty.
    Empty,
    /// Read succeeded with hidden files off, and every entry was hidden:
    /// the folder holds this many hidden items and nothing else.
    HiddenOnly(usize),
    /// Permission denied.
    Denied,
    /// The directory could not be read for another reason (an I/O error,
    /// too many open files, a stalled mount, or the listing worker could not
    /// run it). Retrying may succeed.
    Unavailable,
    /// The directory no longer exists.
    Gone,
    /// The directory opened, but at least one child could not be observed.
    /// The previous complete snapshot remains authoritative.
    Partial,
    /// An asynchronous listing for this binding is in flight; rows are empty.
    Loading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum ViewApplyOutcome {
    Pending,
    Applied,
    ReadRejected(DirStatus),
}

pub enum DirectoryRead {
    Complete(Vec<FileEntry>),
    /// A complete read with hidden files off that found only hidden
    /// entries (this many); no rows are shown.
    OnlyHidden(usize),
    Incomplete(DirStatus),
}

/// Provenance of a watcher-driven re-list: a gap recovery reconciles.
const fn watcher_provenance(recovered_gap: bool) -> crate::change_provenance::ChangeProvenance {
    if recovered_gap {
        crate::change_provenance::ChangeProvenance::Reconciliation
    } else {
        crate::change_provenance::ChangeProvenance::ExternalWatcher
    }
}

/// Why a directory could not be opened, for UI messaging. A path that names
/// a file is no folder to show, like a gone one; only a permission error is
/// "denied", and any other failure (EIO, EMFILE, a mount timeout) is shown
/// as unavailable rather than blamed on permissions.
fn status_for_read_error(error: &std::io::Error) -> DirStatus {
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => DirStatus::Gone,
        std::io::ErrorKind::PermissionDenied => DirStatus::Denied,
        _ => DirStatus::Unavailable,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingIdentity {
    Captured(crate::path_identity::PathIdentity),
    CaptureFailed(crate::ports::NativeFailure),
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Observed with lexical/followed metadata; shared cheaply by row clones.
    pub symlink: Option<Arc<LinkInfo>>,
    pub name: String,
    pub name_lower: String,
    /// Lowercase NFD name without combining marks, computed once at listing.
    pub sort_key: String,
    pub path: PathBuf,
    /// Lexical filesystem binding captured with the listing the user sees.
    /// Operations must not re-observe it synchronously on the UI thread.
    pub identity: ListingIdentity,
    pub is_dir: bool,
    pub size: u64,
    /// Filesystem-reported allocation for a regular file, captured with metadata.
    pub allocated_size: Option<u64>,
    /// Observed regular-file link count; use `hard_link_count()` for lexical rows.
    pub hard_links: Option<u64>,
    /// Lexical Unix ownership captured by the listing worker; no account lookup.
    pub owner_uid: Option<u64>,
    pub owner_gid: Option<u64>,
    /// Lexical object dates, captured with existing metadata (not target dates).
    pub accessed: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed_str: String,
    pub created_str: String,
    /// Unix metadata/status change time, distinct from creation and modification.
    pub changed: Option<SystemTime>,
    pub changed_str: String,
    pub extension: String,
    pub modified: Option<std::time::SystemTime>,
    /// Pre-formatted date string (rendered every frame, formatted once).
    pub modified_str: String,
    /// Pre-formatted size string for files ("…" for dirs).
    pub size_str: String,
    /// Permissions observed with the listing (a link's target's), so Get
    /// Info never stats on the UI thread. `None` when not observed: a
    /// dangling link, or a row that was never stat-ed.
    pub permissions: Option<fs::Permissions>,
}

/// Unix block counts use 512-byte units, independently of filesystem block size.
pub fn allocated_size(metadata: &fs::Metadata) -> Option<u64> {
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.blocks().checked_mul(512)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

pub fn hard_link_count(metadata: &fs::Metadata) -> Option<u64> {
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.nlink())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Interpret POSIX signed seconds plus a nonnegative nanosecond fraction.
#[cfg(any(unix, test))]
fn unix_time_parts(seconds: i64, nanos: i64) -> Option<SystemTime> {
    if !(0..1_000_000_000).contains(&nanos) {
        return None;
    }
    let whole = std::time::Duration::from_secs(seconds.unsigned_abs());
    let base = if seconds < 0 {
        SystemTime::UNIX_EPOCH.checked_sub(whole)?
    } else {
        SystemTime::UNIX_EPOCH.checked_add(whole)?
    };
    base.checked_add(std::time::Duration::from_nanos(nanos as u64))
}

pub fn metadata_change_time(metadata: &fs::Metadata) -> Option<SystemTime> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        unix_time_parts(metadata.ctime(), metadata.ctime_nsec())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

impl FileEntry {
    /// Build an entry from a path and its (already fetched) metadata.
    pub fn from_meta(path: PathBuf, meta: &fs::Metadata) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_string();
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let modified = meta.modified().ok();
        let modified_str = modified
            .and_then(format_modified)
            .unwrap_or_else(|| "–".to_string());
        let size_str = if is_dir {
            "…".to_string()
        } else {
            format_size(size)
        };
        let name_lower = name.to_lowercase();
        let sort_key = diacritic_sort_key(&name_lower);
        Some(Self {
            symlink: None,
            name,
            name_lower,
            sort_key,
            identity: ListingIdentity::Captured(crate::path_identity::PathIdentity::from_metadata(
                path.clone(),
                meta,
            )),
            path,
            is_dir,
            size,
            extension,
            modified,
            modified_str,
            size_str,
            permissions: Some(meta.permissions()),
            allocated_size: allocated_size(meta),
            hard_links: hard_link_count(meta),
            owner_uid: crate::query::native_uid(meta),
            owner_gid: crate::query::native_gid(meta),
            accessed: meta.accessed().ok(),
            created: meta.created().ok(),
            accessed_str: listing_date(meta.accessed().ok()),
            created_str: listing_date(meta.created().ok()),
            changed: metadata_change_time(meta),
            changed_str: listing_date(metadata_change_time(meta)),
        })
    }

    /// Observe `path` the way a listing row is built: a symlink shows its
    /// target's type (a link to a folder is a folder row) but keeps the
    /// link's own identity, and a broken link is still an entry.
    pub(crate) fn observe_lexical(path: PathBuf) -> Option<Self> {
        let lexical = fs::symlink_metadata(&path).ok()?;
        let followed = lexical
            .file_type()
            .is_symlink()
            .then(|| fs::metadata(&path));
        let link_info = followed
            .as_ref()
            .map(|result| Arc::new(LinkInfo::observe(&path, result)));
        let dangling = followed.as_ref().is_some_and(Result::is_err);
        let identity = crate::path_identity::PathIdentity::from_metadata(path.clone(), &lexical);
        let mut entry = Self::from_meta(
            path,
            followed
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .unwrap_or(&lexical),
        )?;
        entry.symlink = link_info;
        entry.identity = ListingIdentity::Captured(identity);
        entry.hard_links = hard_link_count(&lexical);
        entry.owner_uid = crate::query::native_uid(&lexical);
        entry.owner_gid = crate::query::native_gid(&lexical);
        entry.set_lexical_dates(Some(&lexical));
        if dangling {
            // The link's own mode says nothing about a target that is gone.
            entry.permissions = None;
            entry.allocated_size = None;
        }
        Some(entry)
    }

    fn set_lexical_dates(&mut self, metadata: Option<&fs::Metadata>) {
        self.accessed = metadata.and_then(|m| m.accessed().ok());
        self.created = metadata.and_then(|m| m.created().ok());
        self.accessed_str = listing_date(self.accessed);
        self.created_str = listing_date(self.created);
        self.changed = metadata.and_then(metadata_change_time);
        self.changed_str = listing_date(self.changed);
    }
    pub fn listing_time(&self, column: SortColumn) -> (Option<SystemTime>, &str) {
        match column {
            SortColumn::Accessed => (self.accessed, &self.accessed_str),
            SortColumn::Created => (self.created, &self.created_str),
            SortColumn::Changed => (self.changed, &self.changed_str),
            _ => (self.modified, &self.modified_str),
        }
    }

    pub fn filesystem_id(&self) -> Option<(u64, u64)> {
        if let ListingIdentity::Captured(identity) = &self.identity {
            Some((identity.volume?, identity.file_id?))
        } else {
            None
        }
    }
    /// Inode first, device as a secondary key; never mixes unknown identities with zero.
    pub fn inode_sort_key(&self) -> Option<(u64, u64)> {
        self.filesystem_id().map(|(device, inode)| (inode, device))
    }

    /// Retained lexical identity only for a positively observed regular file.
    pub fn regular_file_id(&self) -> Option<(u64, u64)> {
        if self.is_dir || self.is_symlink() {
            return None;
        }
        match &self.identity {
            ListingIdentity::Captured(identity)
                if identity.exists
                    && identity.kind == Some(crate::path_identity::PathKind::File) =>
            {
                self.filesystem_id()
            }
            _ => None,
        }
    }

    pub fn hard_link_count(&self) -> Option<u64> {
        if self.is_dir || self.is_symlink() {
            None
        } else {
            self.hard_links
        }
    }

    /// Only regular non-link rows expose allocation as a sortable file metric.
    pub fn allocated_bytes(&self) -> Option<u64> {
        if self.is_dir || self.is_symlink() {
            None
        } else {
            self.allocated_size
        }
    }

    /// Whether the listing captured this row as a symlink (its display
    /// fields describe the link's target).
    pub fn is_symlink(&self) -> bool {
        matches!(
            &self.identity,
            ListingIdentity::Captured(identity)
                if identity.kind == Some(crate::path_identity::PathKind::Symlink)
        )
    }

    pub fn is_image(&self) -> bool {
        self.is_static_image() || self.is_video()
    }

    pub fn is_static_image(&self) -> bool {
        matches!(
            self.extension.as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "bmp"
                | "webp"
                | "svg"
                | "ico"
                | "heic"
                | "heif"
                | "tiff"
                | "tif"
                | "dng"
                | "cr2"
                | "cr3"
                | "nef"
                | "arw"
                | "orf"
                | "raf"
                | "rw2"
                | "pef"
                | "srw"
        )
    }

    pub fn is_video(&self) -> bool {
        // `extension` is already lowercased; the list is shared with the
        // image cache so preview and listing agree on what a video is.
        crate::image_cache::VIDEO_EXTENSIONS.contains(&self.extension.as_str())
    }

    pub fn size_display(&self) -> &str {
        &self.size_str
    }

    pub fn modified_display(&self) -> &str {
        &self.modified_str
    }
}

fn listing_date(time: Option<SystemTime>) -> String {
    time.and_then(format_modified)
        .unwrap_or_else(|| "Unavailable".into())
}

pub fn diacritic_sort_key(name_lower: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    name_lower
        .nfd()
        .filter(|character| !unicode_normalization::char::is_combining_mark(*character))
        .collect()
}

/// Listing date for a modification time, or `None` when chrono cannot
/// represent it. `From<SystemTime>` panics past chrono's ~262,000-year range,
/// which tmpfs/btrfs/NFS can store (e.g. from a crafted archive); such a row
/// shows an unknown date instead of aborting the listing.
fn format_modified(time: SystemTime) -> Option<String> {
    let utc = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => chrono::DateTime::<chrono::Utc>::from_timestamp(
            i64::try_from(after.as_secs()).ok()?,
            after.subsec_nanos(),
        )?,
        Err(before) => {
            let before = before.duration();
            let secs = i64::try_from(before.as_secs()).ok()?.checked_neg()?;
            match before.subsec_nanos() {
                0 => chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)?,
                nanos => chrono::DateTime::<chrono::Utc>::from_timestamp(
                    secs.checked_sub(1)?,
                    1_000_000_000 - nanos,
                )?,
            }
        }
    };
    // Converting to local time adds the zone offset, which panics at the
    // very edge of the range: keep a day of headroom on both sides.
    let day = chrono::TimeDelta::days(1);
    utc.checked_add_signed(day)?;
    utc.checked_sub_signed(day)?;
    Some(
        utc.with_timezone(&chrono::Local)
            .format("%d %b %y  %H:%M")
            .to_string(),
    )
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{} B", bytes);
    }
    // The unit is chosen after rounding to the printed precision, so a value
    // just under a boundary reads "1.0 MB" rather than "1024.0 KB".
    let kb = bytes as f64 / 1024.0;
    if (kb * 10.0).round() < 10240.0 {
        return format!("{:.1} KB", kb);
    }
    let mb = kb / 1024.0;
    if (mb * 10.0).round() < 10240.0 {
        return format!("{:.1} MB", mb);
    }
    // Larger units print two decimals; a multi-TiB folder or volume reads
    // "5.00 TB" rather than "5120.00 GB".
    let mut value = mb / 1024.0;
    for unit in ["GB", "TB"] {
        if (value * 100.0).round() < 102_400.0 {
            return format!("{value:.2} {unit}");
        }
        value /= 1024.0;
    }
    format!("{value:.2} PB")
}

/// Whether a directory child is a dotfile, which listings, the tree and
/// folder item counts leave out while hidden files are off.
fn is_hidden_name(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// Cursor row after a filter change. The `..` row stays put; a focused
/// entry that still matches keeps the cursor (`found`); otherwise the first
/// match (or `..` when nothing matches) takes it.
const fn filtered_cursor(old_cursor: usize, found: Option<usize>, count: usize) -> usize {
    if old_cursor == 0 {
        return 0;
    }
    match found {
        Some(row) => row,
        None if count > 0 => 1,
        None => 0,
    }
}

/// Glob match supporting `*` (any run) and `?` (one char). Inputs are
/// expected lowercased; matching is greedy with backtracking. Positions are
/// byte offsets into the inputs, so the mask preview (re-run per entry every
/// frame) does not allocate.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let next = |s: &str, at: usize| s[at..].chars().next();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while let Some(tc) = next(text, ti) {
        match next(pattern, pi) {
            Some(pc) if pc == '?' || pc == tc => {
                pi += pc.len_utf8();
                ti += tc.len_utf8();
            }
            Some('*') => {
                star = Some(pi);
                mark = ti;
                pi += 1;
            }
            _ => {
                let Some(s) = star else {
                    return false;
                };
                pi = s + 1;
                // Re-anchor one character further along the text.
                mark += next(text, mark).map_or(1, char::len_utf8);
                ti = mark;
            }
        }
    }
    while next(pattern, pi) == Some('*') {
        pi += 1;
    }
    pi == pattern.len()
}

/// Parse a select-by-mask line into `(term, is_subtract)` pairs. Terms are
/// comma- or semicolon-separated; a leading `!` or `-` marks subtraction. Terms are
/// lowercased for case-insensitive matching.
pub fn parse_mask(mask: &str) -> Vec<(String, bool)> {
    parse_mask_case(mask, false)
}

fn fold_mask_accents(text: &str) -> String {
    use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
    let mut result = String::with_capacity(text.len());
    for ch in text.nfd().filter(|ch| !is_combining_mark(*ch)) {
        match ch {
            'œ' => result.push_str("oe"),
            'Œ' => result.push_str("OE"),
            'æ' => result.push_str("ae"),
            'Æ' => result.push_str("AE"),
            'ß' => result.push_str("ss"),
            'ẞ' => result.push_str("SS"),
            'ﬀ' => result.push_str("ff"),
            'ﬁ' => result.push_str("fi"),
            'ﬂ' => result.push_str("fl"),
            'ﬃ' => result.push_str("ffi"),
            'ﬄ' => result.push_str("ffl"),
            _ => result.push(ch),
        }
    }
    result
}

fn parse_mask_case(mask: &str, case_sensitive: bool) -> Vec<(String, bool)> {
    parse_mask_folding(mask, case_sensitive, false)
}

fn parse_mask_folding(
    mask: &str,
    case_sensitive: bool,
    ignore_accents: bool,
) -> Vec<(String, bool)> {
    let case = |term: &str| {
        let folded = ignore_accents.then(|| fold_mask_accents(term));
        let term = folded.as_deref().unwrap_or(term);
        if case_sensitive {
            term.to_string()
        } else {
            term.to_lowercase()
        }
    };
    mask.split([',', ';'])
        .map(str::trim)
        .map(
            |t| match t.strip_prefix('!').or_else(|| t.strip_prefix('-')) {
                Some(rest) => (case(rest.trim()), true),
                None => (case(t), false),
            },
        )
        // A bare `!`/`-` leaves an empty term, which would match every
        // extension-less entry; drop it like an empty positive term.
        .filter(|(t, _)| !t.is_empty())
        .collect()
}

/// Whether a single lowercased `term` matches an entry. A term with no
/// wildcard and no `.` matches by extension ("jpg" selects all *.jpg);
/// otherwise it is globbed against the full name. A dotted term with no
/// wildcard also matches as a name suffix, so ".jpg" and "tar.gz" select
/// what they look like they should.
fn term_matches(term: &str, name_lower: &str, ext: &str) -> bool {
    let wild = term.contains('*') || term.contains('?');
    if !wild && !term.contains('.') {
        return ext == term;
    }
    if glob_match(term, name_lower) {
        return true;
    }
    if wild {
        return false;
    }
    let suffix = term.trim_start_matches('.');
    !suffix.is_empty()
        && name_lower
            .strip_suffix(suffix)
            .is_some_and(|stem| stem.ends_with('.'))
}

/// What a parsed mask does to `entry`: `Some(false)` deselects (subtraction
/// wins), `Some(true)` selects, `None` leaves it as is.
fn mask_verdict(
    terms: &[(String, bool)],
    entry: &FileEntry,
    case_sensitive: bool,
    ignore_accents: bool,
    extensionless_star: bool,
) -> Option<bool> {
    let name = if case_sensitive {
        &entry.name
    } else {
        &entry.name_lower
    };
    let ext = if case_sensitive {
        if entry.is_dir {
            ""
        } else {
            Path::new(&entry.name)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
        }
    } else {
        &entry.extension
    };
    let folded_name = ignore_accents.then(|| fold_mask_accents(name));
    let folded_ext = ignore_accents.then(|| fold_mask_accents(ext));
    let name = folded_name.as_deref().unwrap_or(name);
    let ext = folded_ext.as_deref().unwrap_or(ext);
    let hits = |subtracting: bool| {
        terms.iter().any(|(term, subtract)| {
            *subtract == subtracting
                && ((extensionless_star && term == "*.*") || term_matches(term, name, ext))
        })
    };
    if hits(true) {
        Some(false)
    } else if hits(false) {
        Some(true)
    } else {
        None
    }
}

/// Drop entries that sit inside another picked folder: picking `X` in its
/// parent and `X/a` inside it must act on `X` once, not on both (a copy
/// would duplicate `a` or conflict with itself). Folder symlinks are not
/// containers here: acting on the link does not act on its target's rows.
fn drop_nested_picks(entries: Vec<FileEntry>) -> Vec<FileEntry> {
    let folders: HashSet<PathBuf> = entries
        .iter()
        .filter(|entry| entry.is_dir && !entry.is_symlink())
        .map(|entry| entry.path.clone())
        .collect();
    if folders.is_empty() {
        return entries;
    }
    entries
        .into_iter()
        .filter(|entry| !entry.path.ancestors().skip(1).any(|a| folders.contains(a)))
        .collect()
}

/// Wake-up callback into the UI (e.g. a repaint request). Panels never
/// talk to the UI toolkit directly.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// See [`PanelState::selection_view_key`].
#[derive(Clone, Debug)]
pub struct SelectionViewKey {
    listing: u64,
    selection: u64,
    filtered: Arc<[usize]>,
}

impl PartialEq for SelectionViewKey {
    fn eq(&self, other: &Self) -> bool {
        self.listing == other.listing
            && self.selection == other.selection
            && Arc::ptr_eq(&self.filtered, &other.filtered)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SortColumn {
    HardLinks,
    Changed,
    Inode,
    Allocated,
    Accessed,
    Created,
    Name,
    Stem,
    Size,
    Modified,
    Extension,
    Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// One-pass folder aggregates for the status bar: total bytes (`None` until at
/// least one subdirectory has been sized), the largest entry by size, and the
/// oldest entry by mtime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderOverview {
    pub total: Option<u64>,
    /// Some subdirectory is not measured (yet, or skipped as too costly):
    /// `total` is a lower bound, not the folder's size.
    pub partial: bool,
    pub largest: Option<(String, u64)>,
    pub oldest: Option<(String, SystemTime)>,
}

/// A non-parent cursor row no longer exists in the filtered view. This should
/// be prevented by [`PanelState::ensure_cursor_valid`], but remains explicit at
/// file-operation call sites so a future invariant regression cannot silently
/// turn a command into an empty selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleCursor {
    pub cursor: usize,
    pub visible_entries: usize,
}

pub struct PanelState {
    pub current_path: PathBuf,
    pub(crate) tab_title: Option<String>,
    pub(crate) tab_close_protected: bool,
    pub(crate) tab_full_path_label: bool,
    pub(crate) tab_pinned_path: Option<PathBuf>,
    listing: ListingState,
    selection: SelectionState,
    saved_selection: Option<(PathBuf, HashSet<PathBuf>)>,
    pub(crate) tab_selection: crate::selection_snapshot::Snapshot,
    pub preview: Option<PreviewContent>,
    /// Per-pane directory history (back/forward); a vim-style jump trail that
    /// truncates its forward tail on a new navigation.
    pub history: crate::jumplist::JumpList,
    view: ViewState,
    sizes: SizeIndex,
    watcher: DirectoryWatcherState,
    pub drag: DragState,
    listing_job: ListingJobController,
    pub(crate) miller: miller::MillerState,
    miller_context_only: bool,
    miller_widths: miller::Widths,
    initial_focus_child: Option<(PathBuf, String)>,
    pending_view_config: Option<ViewConfig>,
    view_apply_outcome: Option<ViewApplyOutcome>,
    workload: Option<crate::workload::WorkloadHandle>,
    /// Why the most recent listing refresh landed (path-free, research J007).
    pub change_provenance: crate::change_provenance::RowProvenance,
    /// A navigation target whose Recent entry waits for its listing: it is
    /// recorded when the read resolves, and dropped when the folder is gone.
    pending_visit: Option<PathBuf>,
    last_size_sort_revision: u64,
    selection_summary_cache: RefCell<Option<SelectionSummaryCache>>,
}

/// The footer's visible-selection summary and what it was computed from.
struct SelectionSummaryCache {
    key: SelectionViewKey,
    sizes: Arc<SizeSnapshot>,
    summary: (usize, u64, bool),
}

impl PanelState {
    /// Create a panel pointed at `path`. The directory is NOT read yet:
    /// call [`refresh`](Self::refresh) (the UI does this when wiring the
    /// notify callback on the first frame).
    #[cfg(test)]
    pub fn new(path: PathBuf) -> Self {
        Self::new_with_view(path, ViewConfig::default())
    }

    /// Seed persisted view configuration before the first directory listing.
    /// This prevents sort/hidden indicators from temporarily disagreeing with
    /// rows loaded under default settings.
    pub(crate) fn new_with_view(path: PathBuf, config: ViewConfig) -> Self {
        Self {
            current_path: path.clone(),
            tab_title: None,
            tab_close_protected: false,
            tab_full_path_label: false,
            tab_pinned_path: None,
            listing: ListingState::new(path.clone()),
            selection: SelectionState::new(path.clone()),
            saved_selection: None,
            tab_selection: Default::default(),
            preview: None,
            history: {
                let mut h = crate::jumplist::JumpList::new();
                h.push(path.clone());
                h
            },
            view: ViewState::with_config(config),
            sizes: SizeIndex::new(path.clone()),
            watcher: DirectoryWatcherState::default(),
            drag: DragState::new(),
            listing_job: ListingJobController::default(),
            miller: miller::MillerState::default(),
            miller_context_only: false,
            miller_widths: miller::Widths::default(),
            initial_focus_child: None,
            pending_view_config: None,
            view_apply_outcome: None,
            workload: None,
            change_provenance: crate::change_provenance::RowProvenance::default(),
            pending_visit: None,
            last_size_sort_revision: 0,
            selection_summary_cache: RefCell::new(None),
        }
    }

    /// Attach the shared workload handle used for off-thread listings.
    pub fn set_workload(&mut self, workload: crate::workload::WorkloadHandle) {
        self.workload = Some(workload);
    }

    /// Wire the UI wake-up callback and restart the watcher so its
    /// notifications reach the UI.
    pub fn set_notify(&mut self, notify: Notify) {
        self.watcher.set_notify(notify);
        self.watcher.ensure_binding(&self.current_path);
    }

    pub(crate) fn prepare_tab_activation(&mut self) {
        // Retiring rows makes loading-frame cursor validation clamp to `..`.
        // Keep the path-owned landing intent in the listing controller so
        // the first refresh (and any repeated refresh) restores it on publish.
        let focus = if let Some(path) = self
            .tab_pinned_path
            .clone()
            .filter(|p| *p != self.current_path)
        {
            self.stash_view_settings();
            self.history.push(path.clone());
            self.pending_visit = Some(path.clone());
            self.current_path = path;
            let remembered = self.restore_view_settings();
            if remembered.is_none() {
                self.view.clear_filters();
            }
            let (cursor_path, scroll_anchor) = remembered.unwrap_or((None, 0));
            self.selection.bind(&self.current_path);
            self.selection.set_scroll_anchor(scroll_anchor);
            self.miller = Default::default();
            self.preview = None;
            self.initial_focus_child = None;
            PendingFocus::Remembered {
                cursor_path,
                scroll_anchor,
            }
        } else {
            PendingFocus::Remembered {
                cursor_path: self.focused_path(),
                scroll_anchor: self.scroll_anchor(),
            }
        };
        self.listing_job.request(
            self.current_path.clone(),
            self.view.show_hidden(),
            None,
            focus,
        );
        self.listing.begin_loading(self.current_path.clone());
        self.selection.set_scroll_to_cursor(true);
    }

    pub(crate) fn suspend_for_tab(&mut self) {
        self.miller = Default::default();
        self.watcher = DirectoryWatcherState::default();
        self.listing_job = ListingJobController::default();
        self.pending_view_config = None;
        self.view_apply_outcome = None;
        self.sizes.suspend();
        self.drag.clear();
    }

    pub fn has_notify(&self) -> bool {
        self.watcher.has_notify()
    }

    pub(crate) fn notify_callback(&self) -> Option<Notify> {
        self.watcher.notify()
    }

    pub fn entries(&self) -> &[FileEntry] {
        self.listing.entries()
    }

    #[cfg(test)]
    fn replace_entries_for_test(&mut self, entries: Vec<FileEntry>) {
        self.selection.publish_complete(&self.current_path);
        self.listing.replace_for_test(entries);
    }

    #[cfg(test)]
    fn mutate_entries_for_test(&mut self, mutate: impl FnOnce(&mut Vec<FileEntry>)) {
        self.listing.mutate_for_test(mutate);
    }

    #[cfg(test)]
    fn push_entry_for_test(&mut self, entry: FileEntry) {
        self.listing.push_for_test(entry);
    }

    #[cfg(test)]
    fn clear_entries_without_revision_for_test(&mut self) {
        self.listing
            .mutate_entries_without_revision_for_test(Vec::clear);
    }

    pub const fn dir_status(&self) -> DirStatus {
        self.listing.status()
    }

    #[cfg(test)]
    pub const fn selected_paths(&self) -> &HashSet<PathBuf> {
        self.selection.selected()
    }

    /// What the selection HUD summarizes: equal keys mean the visible
    /// selection, the counts elsewhere and the entries they describe are
    /// unchanged. Holding the filtered view's `Arc` keeps its address from
    /// being reused, so pointer equality is a sound identity check.
    pub fn selection_view_key(&self) -> SelectionViewKey {
        SelectionViewKey {
            listing: self.entries_gen(),
            selection: self.selection.revision(),
            filtered: self.filtered_snapshot(),
        }
    }

    pub fn selection_is_empty(&self) -> bool {
        self.selection.selected().is_empty()
    }

    pub fn selected_count(&self) -> usize {
        self.selection.selected().len()
    }

    pub fn is_selected(&self, path: &Path) -> bool {
        self.selection.selected().contains(path)
    }

    #[cfg(test)]
    pub(crate) fn select_path(&mut self, path: PathBuf) -> bool {
        self.selection.insert_selected(path)
    }

    /// Save/restore only against a complete listing for the current binding.
    pub fn can_save_selection(&self) -> bool {
        self.listing.binding() == self.current_path
            && matches!(
                self.listing.status(),
                DirStatus::Listed | DirStatus::Empty | DirStatus::HiddenOnly(_)
            )
    }

    pub fn can_restore_selection(&self) -> bool {
        self.can_save_selection()
            && self
                .saved_selection
                .as_ref()
                .is_some_and(|(path, _)| path == &self.current_path)
    }

    pub fn save_selection(&mut self) -> bool {
        if !self.can_save_selection() {
            return false;
        }
        self.saved_selection = Some((self.current_path.clone(), self.selection.selected().clone()));
        true
    }

    /// Replace this folder's picks, retaining selections in other folders.
    /// Deleted/unlisted paths cannot be resurrected by the memory buffer.
    pub fn restore_saved_selection(&mut self) -> bool {
        if !self.can_restore_selection() {
            return false;
        }
        let (_, saved) = self
            .saved_selection
            .as_ref()
            .expect("checked saved selection");
        let restored = self
            .listing
            .entries()
            .iter()
            .filter(|entry| saved.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect();
        self.selection.replace_selected(restored);
        true
    }

    pub fn save_tab_selection(&mut self) -> Result<(), String> {
        if !self.can_save_selection() {
            return Err("Wait for a complete folder listing".into());
        }
        let snapshot = crate::selection_snapshot::Snapshot::capture(
            &self.current_path,
            self.selection.selected(),
        )?;
        self.tab_selection = snapshot;
        Ok(())
    }
    pub fn can_restore_tab_selection(&self) -> bool {
        self.can_save_selection() && self.tab_selection.belongs_to(&self.current_path)
    }
    pub fn restore_tab_selection(&mut self) -> bool {
        if !self.can_restore_tab_selection() {
            return false;
        }
        let paths = self
            .listing
            .entries()
            .iter()
            .filter(|e| self.tab_selection.contains(&e.path))
            .map(|e| e.path.clone())
            .collect();
        self.selection.replace_selected(paths);
        true
    }
    pub fn clear_selection(&mut self) {
        self.selection.clear_selected();
    }

    /// Clear the selection in this folder and every other folder visited.
    pub fn clear_selection_everywhere(&mut self) {
        self.selection.clear_selected_everywhere();
    }

    /// How many items are selected in folders other than this one.
    pub fn selected_elsewhere_count(&self) -> usize {
        self.selection
            .selected_elsewhere()
            .map(|(_, paths)| paths.len())
            .sum()
    }

    /// How many folders hold part of the selection, this one included.
    pub fn selection_folder_count(&self) -> usize {
        self.selection.selected_elsewhere().count() + usize::from(!self.selection_is_empty())
    }

    /// Items selected in other folders that still exist, folder by folder in
    /// path order. Vanished paths are dropped rather than acted on.
    fn selected_elsewhere_entries(&self) -> Vec<FileEntry> {
        let mut folders: Vec<_> = self.selection.selected_elsewhere().collect();
        folders.sort_by(|a, b| a.0.cmp(b.0));
        folders
            .into_iter()
            .flat_map(|(_, paths)| {
                let mut paths: Vec<&PathBuf> = paths.iter().collect();
                paths.sort();
                paths
            })
            .filter_map(|path| FileEntry::observe_lexical(path.clone()))
            .collect()
    }

    /// What an operation acts on: the selection across every folder this
    /// panel visited, or the highlighted entry when nothing is selected.
    pub fn picked_entries(&self) -> Result<Vec<FileEntry>, StaleCursor> {
        if self.selected_elsewhere_count() == 0 {
            return self.selected_or_cursor();
        }
        let mut entries = self.selected_entries();
        entries.extend(self.selected_elsewhere_entries());
        Ok(drop_nested_picks(entries))
    }

    /// Non-recursive metadata operations keep every explicitly selected path.
    /// Do not use `picked_entries`: it drops nested picks and probes selections
    /// from other folders synchronously. Foreign observations belong to the
    /// metadata worker and are reviewed before any commit.
    pub(crate) fn permission_targets(
        &self,
    ) -> Result<Vec<crate::permissions::Target>, StaleCursor> {
        if self.selection_is_empty() && self.selected_elsewhere_count() == 0 {
            return self.selected_or_cursor().map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| crate::permissions::Target {
                        path: entry.path,
                        identity: entry.identity,
                        capture_on_worker: false,
                    })
                    .collect()
            });
        }
        let current: std::collections::HashMap<_, _> = self
            .entries()
            .iter()
            .map(|entry| (&entry.path, &entry.identity))
            .collect();
        let mut targets: Vec<_> = self
            .selection
            .selected()
            .iter()
            .map(|path| crate::permissions::Target {
                path: path.clone(),
                identity: current
                    .get(path)
                    .map_or(ListingIdentity::Unavailable, |identity| (*identity).clone()),
                capture_on_worker: false,
            })
            .collect();
        for (_, paths) in self.selection.selected_elsewhere() {
            targets.extend(paths.iter().map(|path| crate::permissions::Target {
                path: path.clone(),
                identity: ListingIdentity::Unavailable,
                capture_on_worker: true,
            }));
        }
        targets.sort_by(|a, b| a.path.cmp(&b.path));
        targets.dedup_by(|a, b| a.path == b.path);
        Ok(targets)
    }

    /// The same operation inputs as [`Self::picked_entries`], excluding a
    /// destination row that must not become its own transfer source.
    pub fn picked_entries_excluding(&self, excluded: &Path) -> Result<Vec<FileEntry>, StaleCursor> {
        let mut entries = self.picked_entries()?;
        entries.retain(|entry| entry.path != excluded);
        Ok(entries)
    }

    pub fn replace_selection(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.selection
            .replace_selected(paths.into_iter().collect::<HashSet<_>>());
    }

    pub const fn cursor(&self) -> usize {
        self.selection.cursor()
    }

    pub fn set_cursor(&mut self, row: usize) {
        let row = row.min(self.filtered_count());
        let path = row
            .checked_sub(1)
            .and_then(|index| self.filtered_get(index))
            .map(|entry| entry.path.clone());
        self.selection.set_cursor(row, path);
    }

    pub fn cursor_entry(&self) -> Option<&FileEntry> {
        let Focus::Entry(expected) = self.selection.focus() else {
            return None;
        };
        self.cursor()
            .checked_sub(1)
            .and_then(|index| self.filtered_get(index))
            .filter(|entry| &entry.path == expected)
    }

    pub const fn scroll_to_cursor(&self) -> bool {
        self.selection.scroll_to_cursor()
    }

    pub const fn set_scroll_to_cursor(&mut self, value: bool) {
        self.selection.set_scroll_to_cursor(value);
    }

    pub const fn scroll_anchor(&self) -> usize {
        self.selection.scroll_anchor()
    }

    pub const fn set_scroll_anchor(&mut self, anchor: usize) {
        self.selection.set_scroll_anchor(anchor);
    }

    pub const fn page_rows(&self) -> usize {
        self.selection.page_rows()
    }

    pub const fn set_page_rows(&mut self, rows: usize) {
        self.selection.set_page_rows(rows);
    }

    pub const fn view_config(&self) -> ViewConfig {
        self.view.config()
    }

    pub fn search_query(&self) -> &str {
        self.view.search_query()
    }

    pub fn set_search_query(&mut self, query: impl Into<String>) {
        self.preserving_focus_while_filtering(|panel| panel.view.set_search_query(query));
    }

    pub const fn facets(&self) -> FacetSet {
        self.view.facets()
    }

    pub fn set_facets(&mut self, facets: FacetSet) {
        self.preserving_focus_while_filtering(|panel| *panel.view.facets_mut() = facets);
    }

    #[cfg(test)]
    pub(crate) const fn sort_column(&self) -> SortColumn {
        self.view.sort_col()
    }

    #[cfg(test)]
    pub(crate) const fn sort_order(&self) -> SortOrder {
        self.view.sort_order()
    }

    pub const fn show_hidden(&self) -> bool {
        self.view.show_hidden()
    }

    /// Toggle hidden entries as one view/listing transition.
    ///
    /// A rejected read leaves both the prior configuration and its complete
    /// rows authoritative; callers can present the typed failure without a
    /// split-brain hidden indicator.
    pub fn toggle_hidden(&mut self) -> ViewApplyOutcome {
        let candidate = self
            .view
            .config()
            .with_show_hidden(!self.view.show_hidden());
        let path = self.current_path.clone();
        if self.can_async_list() {
            self.sizes.bind(&path);
            self.watcher.ensure_binding(&path);
            let ticket = self.watcher.snapshot_ticket();
            self.pending_view_config = Some(candidate);
            self.view_apply_outcome = None;
            self.listing_job
                .request(path, candidate.show_hidden(), ticket, PendingFocus::None);
            let _ = self.poll_listing_results();
            return self
                .view_apply_outcome
                .take()
                .unwrap_or(ViewApplyOutcome::Pending);
        }
        let ticket = self.watcher.snapshot_ticket_for(&path);
        let (entries, hidden) = match Self::read_dir(&path, candidate.show_hidden()) {
            DirectoryRead::Complete(entries) => (entries, 0),
            DirectoryRead::OnlyHidden(hidden) => (Vec::new(), hidden),
            DirectoryRead::Incomplete(status) => {
                self.watcher.defer_snapshot(ticket.as_ref());
                return ViewApplyOutcome::ReadRejected(status);
            }
        };

        self.view.commit_config(candidate);
        self.publish_complete_listing(entries, hidden, candidate);
        self.sizes.bind(&path);
        self.watcher.ensure_binding(&path);
        self.commit_snapshot(ticket, true);
        ViewApplyOutcome::Applied
    }

    pub const fn browser_mode(&self) -> BrowserMode {
        self.view.config().mode()
    }

    pub fn set_browser_mode(&mut self, mode: BrowserMode) {
        if mode != BrowserMode::Miller {
            self.miller = Default::default();
        }
        self.view.commit_config(self.view.config().with_mode(mode));
        self.selection.set_scroll_to_cursor(true);
    }

    pub const fn density(&self) -> crate::density::Density {
        self.view.density()
    }

    pub const fn set_density(&mut self, density: crate::density::Density) {
        self.view.set_density(density);
    }

    pub const fn watcher_active(&self) -> bool {
        self.watcher.is_active()
    }

    fn sort_entries(&mut self) {
        let sizes = self.sizes.snapshot();
        self.preserving_focus(|panel| {
            let config = panel.view.config();
            panel
                .listing
                .resort(|entries| sort_entries_with_sizes(entries, config, Some(sizes.as_ref())));
        });
    }

    fn resort_when_directory_sizes_settle(&mut self) {
        if self.view.sort_col() != SortColumn::Size {
            return;
        }
        let sizes = self.sizes.snapshot();
        if sizes.revision() == self.last_size_sort_revision
            || self
                .listing
                .entries()
                .iter()
                .filter(|entry| entry.is_dir)
                .any(|entry| sizes.size_of(&entry.path).is_none())
        {
            return;
        }
        self.last_size_sort_revision = sizes.revision();
        self.preserving_focus(|panel| {
            let config = panel.view.config();
            panel
                .listing
                .resort(|entries| sort_entries_with_sizes(entries, config, Some(sizes.as_ref())));
        });
    }

    /// Toggle pinning folders to the top, then re-sort in place.
    pub fn toggle_folders_first(&mut self) {
        self.view.toggle_folders_first();
        self.sort_entries();
    }

    /// Keep a name-ordered folder group ahead of independently sorted files.
    pub fn toggle_folders_by_name(&mut self) {
        self.view.toggle_folders_by_name();
        self.sort_entries();
    }

    /// Use original Unicode names rather than folded keys, then re-sort.
    pub fn toggle_case_sensitive_name_sort(&mut self) {
        self.view.toggle_case_sensitive_name_sort();
        self.sort_entries();
    }

    /// Toggle natural vs plain A-Z name ordering, then re-sort in place.
    pub fn toggle_natural_sort(&mut self) {
        self.view.toggle_natural_sort();
        self.sort_entries();
    }

    fn snapshot_view_settings(&self) -> ViewSettings {
        ViewSettings {
            config: self.view.config(),
            search_query: self.view.search_query().to_string(),
            facets: self.view.facets(),
            cursor_path: self.focused_path(),
            scroll_anchor: self.scroll_anchor(),
        }
    }

    /// Remember the current directory's view settings under its own path.
    fn stash_view_settings(&mut self) {
        let mut settings = self.snapshot_view_settings();
        if self.listing.status() == DirStatus::Loading
            && self.listing.binding() == self.current_path
        {
            // Leaving before the listing landed: keep the focus it was about
            // to restore, not the empty placeholder's `..` row.
            match self.listing_job.pending_focus() {
                PendingFocus::None => {}
                PendingFocus::Remembered {
                    cursor_path,
                    scroll_anchor,
                } => {
                    settings.cursor_path.clone_from(cursor_path);
                    settings.scroll_anchor = *scroll_anchor;
                }
                PendingFocus::NamedChild(name) => {
                    settings.cursor_path = Some(self.current_path.join(name));
                }
            }
        }
        self.view.remember(&self.current_path, settings);
    }

    /// Apply the current directory's remembered view settings, if any.
    /// Leaves everything unchanged (carrying over whatever was already
    /// active) when this directory has never been visited this session.
    fn restore_view_settings(&mut self) -> Option<(Option<PathBuf>, usize)> {
        let s = self.view.restore(&self.current_path)?;
        Some((s.cursor_path, s.scroll_anchor))
    }

    fn load_remembered_path(&mut self, path: PathBuf) {
        self.current_path = path;
        let remembered = self.restore_view_settings();
        if self.browser_mode() != BrowserMode::Miller {
            self.miller = Default::default();
        }
        if remembered.is_none() {
            self.view.clear_filters();
        }
        let focus = match remembered {
            Some((cursor_path, scroll_anchor)) => {
                self.set_scroll_anchor(scroll_anchor);
                PendingFocus::Remembered {
                    cursor_path,
                    scroll_anchor,
                }
            }
            None => {
                self.set_scroll_anchor(0);
                PendingFocus::None
            }
        };
        self.schedule_or_reload(focus);
    }

    /// Move the cursor to the first filtered entry whose name matches `buffer`
    /// (prefix first, then substring), both lowercased. Returns whether a
    /// match was found. Drives type-to-jump navigation.
    pub fn type_ahead(&mut self, buffer: &str) -> bool {
        if buffer.is_empty() {
            return false;
        }
        let q = buffer.to_lowercase();
        let pos = self
            .filtered_position(|e| e.name_lower.starts_with(&q))
            .or_else(|| self.filtered_position(|e| e.name_lower.contains(&q)));
        if let Some(idx) = pos {
            self.reveal_cursor(idx + 1);
            true
        } else {
            false
        }
    }

    /// Flip the current sort order (ascending <-> descending) and re-sort.
    pub fn reverse_sort(&mut self) {
        self.view.reverse_sort();
        self.sort_entries();
    }

    /// Add the file under the cursor to the selection (range-select step).
    pub fn select_cursor(&mut self) {
        if let Some(path) = self.cursor_entry().map(|entry| entry.path.clone()) {
            self.selection.insert_selected(path);
        }
    }

    fn filtered_snapshot(&self) -> Arc<[usize]> {
        self.listing
            .filtered_snapshot(self.view.search_query(), self.view.facets())
    }

    fn focused_path(&self) -> Option<PathBuf> {
        self.selection.focused_path().map(Path::to_path_buf)
    }

    /// Move the cursor and scroll it into view.
    fn reveal_cursor(&mut self, cursor: usize) {
        self.set_cursor(cursor);
        self.set_scroll_to_cursor(true);
    }

    /// Apply a filter, facet or ordering change, then put the cursor back on
    /// the entry it focused (or the nearest surviving row).
    fn preserving_focus(&mut self, change: impl FnOnce(&mut Self)) {
        let focus = self.focused_path();
        let old_cursor = self.cursor();
        change(self);
        self.restore_cursor_focus(focus, old_cursor);
    }

    /// Like [`Self::preserving_focus`] for a query or facet change: when the
    /// focused entry no longer matches, the cursor lands on the first match
    /// rather than on whatever row now sits at the old (unfiltered) index.
    fn preserving_focus_while_filtering(&mut self, change: impl FnOnce(&mut Self)) {
        let focus = self.focused_path();
        let old_cursor = self.cursor();
        change(self);
        if old_cursor == 0 {
            self.set_cursor(0);
            return;
        }
        let found = focus
            .and_then(|path| self.filtered_position(|entry| entry.path == path))
            .map(|index| index + 1);
        self.reveal_cursor(filtered_cursor(old_cursor, found, self.filtered_count()));
    }

    fn restore_cursor_focus(&mut self, focused: Option<PathBuf>, old_cursor: usize) {
        if old_cursor == 0 {
            self.set_cursor(0);
            return;
        }
        let cursor = focused
            .and_then(|path| self.filtered_position(|entry| entry.path == path))
            .map(|index| index + 1)
            .unwrap_or_else(|| old_cursor.min(self.filtered_count()));
        self.reveal_cursor(cursor);
    }

    /// Number of entries matching the current filter (no allocation).
    pub fn filtered_count(&self) -> usize {
        self.filtered_snapshot().len()
    }

    /// Clamp the cursor after any filter, facet, or ordering change. Cursor 0
    /// is the synthetic parent row; real rows occupy `1..=filtered_count()`.
    ///
    /// A cursor row that no longer shows its focused entry (rows above it
    /// left the view, e.g. when an age facet expires them) follows that
    /// entry by path, so operations do not stop on a stale cursor.
    pub fn ensure_cursor_valid(&mut self) {
        let cursor = self.cursor();
        if cursor > 0 && self.cursor_entry().is_none() {
            let focused = self.focused_path();
            self.restore_cursor_focus(focused, cursor);
            return;
        }
        let clamped = self.cursor().min(self.filtered_count());
        if self.cursor() != clamped {
            self.reveal_cursor(clamped);
        }
    }

    /// Clear both text and facet filters as one invariant-preserving action.
    pub fn clear_filters(&mut self) {
        self.preserving_focus_while_filtering(|panel| panel.view.clear_filters());
    }

    /// The i-th entry of the filtered view (no allocation).
    pub fn filtered_get(&self, i: usize) -> Option<&FileEntry> {
        let snapshot = self.filtered_snapshot();
        let idx = *snapshot.get(i)?;
        self.listing.entries().get(idx)
    }

    /// Generation-bound shared snapshot of indices into [`Self::entries`].
    /// Cloning this is O(1), so a warm renderer frame does not allocate O(N).
    pub fn filtered_indices(&self) -> Arc<[usize]> {
        self.filtered_snapshot()
    }

    pub fn filtered_entries(&self) -> Vec<&FileEntry> {
        self.filtered_iter().collect()
    }

    /// The filtered view in display order, with each entry's listing index.
    fn filtered_iter_indexed(&self) -> impl Iterator<Item = (usize, &FileEntry)> {
        let snapshot = self.filtered_snapshot();
        let entries = self.listing.entries();
        (0..snapshot.len()).filter_map(move |i| {
            let index = snapshot[i];
            entries.get(index).map(|entry| (index, entry))
        })
    }

    /// The filtered view in display order.
    fn filtered_iter(&self) -> impl Iterator<Item = &FileEntry> {
        self.filtered_iter_indexed().map(|(_, entry)| entry)
    }

    /// Selected entries the current filter shows, in display order.
    fn visible_selected(&self) -> impl Iterator<Item = &FileEntry> {
        self.filtered_iter()
            .filter(|entry| self.selection.selected().contains(&entry.path))
    }

    /// Visit the filtered view with mutable access to the selection.
    fn update_filtered_selection(
        &mut self,
        mut update: impl FnMut(&FileEntry, &mut SelectionState),
    ) {
        let snapshot = self.filtered_snapshot();
        let entries = self.listing.entries();
        for entry in snapshot.iter().filter_map(|&index| entries.get(index)) {
            update(entry, &mut self.selection);
        }
    }

    /// Add every filtered entry matching `wanted`; returns how many were new.
    fn select_filtered_where(&mut self, mut wanted: impl FnMut(&FileEntry) -> bool) -> usize {
        let mut added = 0;
        self.update_filtered_selection(|entry, selection| {
            if wanted(entry) {
                added += usize::from(selection.insert_selected(entry.path.clone()));
            }
        });
        added
    }

    fn filtered_position(&self, predicate: impl FnMut(&FileEntry) -> bool) -> Option<usize> {
        self.filtered_iter().position(predicate)
    }

    fn filtered_available_count(&self) -> usize {
        self.filtered_iter().count()
    }

    /// Visit the filtered view without allocating a temporary `Vec`. Returning
    /// `false` stops the walk, which keeps action-bar capability checks cheap
    /// when the first actionable selection is near the front of the listing.
    pub(crate) fn visit_filtered(&self, mut visitor: impl FnMut(usize, &FileEntry) -> bool) {
        for (index, entry) in self.filtered_iter_indexed() {
            if !visitor(index, entry) {
                break;
            }
        }
    }

    pub fn toggle_select(&mut self, path: PathBuf) {
        self.selection.toggle_selected(path);
    }

    /// Start a row drag. An unselected anchor always drags only itself; a
    /// selected anchor drags the visible selected set in listing order.
    pub fn begin_drag(&mut self, anchor: PathBuf) {
        let entries = if self.selection.selected().contains(&anchor) {
            self.visible_selected()
                .map(|entry| entry.path.clone())
                .collect()
        } else {
            vec![anchor]
        };
        self.drag.set(entries);
    }

    pub fn select_all(&mut self) {
        let all_selected = self.visible_selected().count() == self.filtered_available_count();
        self.update_filtered_selection(|entry, selection| {
            if all_selected {
                selection.remove_selected(&entry.path);
            } else {
                selection.insert_selected(entry.path.clone());
            }
        });
    }

    /// Flip selection membership across the filtered view: selected entries
    /// become unselected and vice versa. Entries hidden by the current filter
    /// keep their state, so an invert respects what the user can actually see.
    pub fn invert_selection(&mut self) {
        self.update_filtered_selection(|entry, selection| {
            selection.toggle_selected(entry.path.clone());
        });
    }

    /// Invert one visible kind, retaining the other kind and filtered-out picks.
    pub fn invert_visible_kind(&mut self, folders: bool) -> usize {
        let mut changed = 0;
        self.update_filtered_selection(|entry, selection| {
            if entry.is_dir == folders {
                selection.toggle_selected(entry.path.clone());
                changed += 1;
            }
        });
        changed
    }

    /// Add `paths` to the current selection, keeping any existing picks.
    /// Used by relationship-based selectors (e.g. "select files also in the
    /// other panel") so selections compose instead of replacing each other.
    pub fn extend_selection(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.selection.extend_selected(paths);
    }

    pub fn selected_entries(&self) -> Vec<FileEntry> {
        self.visible_selected().cloned().collect()
    }

    pub fn selected_or_cursor(&self) -> Result<Vec<FileEntry>, StaleCursor> {
        if self.selection.selected().is_empty() {
            match self.selection.focus() {
                Focus::Parent => Ok(vec![]),
                Focus::Entry(_) => match self.cursor_entry() {
                    Some(entry) => Ok(vec![entry.clone()]),
                    None => Err(StaleCursor {
                        cursor: self.cursor(),
                        visible_entries: self.filtered_available_count(),
                    }),
                },
            }
        } else {
            Ok(self.selected_entries())
        }
    }

    /// Selected rows in the filtered view and their total display size, plus
    /// whether any selected folder is still being measured (it counts as 0 B
    /// until then). Recomputed only when the listing, filter, selection or
    /// measured sizes change, so the footer can ask for it every frame.
    pub fn visible_selection_summary(&self) -> (usize, u64, bool) {
        if self.selected_count() == 0 {
            return (0, 0, false);
        }
        let key = self.selection_view_key();
        let sizes = self.size_snapshot();
        if let Some(cache) = &*self.selection_summary_cache.borrow()
            && cache.key == key
            && Arc::ptr_eq(&cache.sizes, &sizes)
        {
            return cache.summary;
        }
        let summary = self.visible_selection_summary_with(&sizes);
        *self.selection_summary_cache.borrow_mut() = Some(SelectionSummaryCache {
            key,
            sizes,
            summary,
        });
        summary
    }

    pub(crate) fn visible_selection_summary_with(
        &self,
        sizes: &SizeSnapshot,
    ) -> (usize, u64, bool) {
        self.visible_selected()
            .fold((0, 0, false), |(count, size, pending), entry| {
                (
                    count + 1,
                    size.saturating_add(sizes.display_size(entry)),
                    // Folder symlinks are never walked (as in folder_overview),
                    // so they count as 0 B rather than as still measuring.
                    pending
                        || (entry.is_dir
                            && !entry.is_symlink()
                            && sizes.size_of(&entry.path).is_none()),
                )
            })
    }

    pub fn visible_selected_count(&self) -> usize {
        self.visible_selected().count()
    }

    #[cfg(test)]
    pub fn visible_selected_size(&self) -> u64 {
        let sizes = self.sizes.snapshot();
        self.visible_selected()
            .map(|entry| sizes.display_size(entry))
            .sum()
    }

    pub fn folder_overview(&self) -> FolderOverview {
        self.sizes
            .folder_overview(self.entries_gen(), self.listing.entries())
    }

    pub fn size_snapshot(&self) -> Arc<SizeSnapshot> {
        self.sizes.snapshot()
    }

    pub fn max_display_size(&self, filtered: &Arc<[usize]>) -> u64 {
        self.sizes
            .max_display_size(self.entries_gen(), self.listing.entries(), filtered)
    }

    /// Monotonic generation of this panel's entry list, bumped on every content
    /// or order change. Lets the app cache per-panel derived data (e.g. the
    /// cross-panel compare map) and rebuild only when the entries change.
    pub const fn entries_gen(&self) -> u64 {
        self.listing.revision().value()
    }

    pub fn set_sort(&mut self, col: SortColumn) {
        self.view.toggle_sort(col);
        self.sort_entries();
    }

    /// List subdirectories of `path` (for tree view).
    pub fn subdirs(path: &Path, show_hidden: bool) -> Vec<PathBuf> {
        let Ok(rd) = fs::read_dir(path) else {
            return Vec::new();
        };
        let dirs: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            // Folder symlinks are folders here, as in the file list; the
            // tree expands lazily, so following them cannot loop.
            .filter(|e| match e.file_type() {
                Ok(t) if t.is_symlink() => e.path().is_dir(),
                Ok(t) => t.is_dir(),
                Err(_) => false,
            })
            .map(|e| e.path())
            .filter(|p| show_hidden || !p.file_name().is_some_and(is_hidden_name))
            .collect();
        // Natural order, like the file list's default, so "dir2" sits above
        // "dir10" in both views.
        let mut keyed: Vec<(String, PathBuf)> = dirs
            .into_iter()
            .map(|path| {
                let key = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                (key, path)
            })
            .collect();
        keyed.sort_by(|(a, _), (b, _)| natural_cmp(a, b));
        keyed.into_iter().map(|(_, path)| path).collect()
    }

    pub fn sort_order_for(&self, col: SortColumn) -> Option<SortOrder> {
        (self.view.sort_col() == col).then(|| self.view.sort_order())
    }
}

#[cfg(test)]
mod tests;
