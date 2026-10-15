use serde::{Deserialize, Serialize};
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

mod dir_size_cache;
mod drag;
mod facet;
mod listing;
mod listing_job;
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
use listing::ListingState;
use listing_job::{ListingJobController, PendingFocus};
// Re-export preview/visit public API under `crate::panel::...` for stable paths.
#[allow(unused_imports)]
pub use preview::{
    InfoCard, MEASURING, PreviewContent, PreviewIdentity, format_mode,
    image_preview_unavailable_message, make_info, make_preview,
};
use selection::{Focus, SelectionState};
pub use size_index::SizeSnapshot;
use size_index::{SizeIndex, SizeScanInput};
use sort::natural_cmp;
use sort::sort_entries;
pub use view::ViewConfig;
use view::{ViewSettings, ViewState};
#[allow(unused_imports)]
pub use visit::{
    RecentMatch, RecentOrder, VISITED_CAP, VisitStats, VisitUsage, push_visit, rank_visited,
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
    /// Permission denied.
    Denied,
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
    Applied,
    ReadRejected(DirStatus),
}

pub enum DirectoryRead {
    Complete(Vec<FileEntry>),
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
/// a file is no folder to show, like a gone one; anything else is treated
/// as unreadable.
fn status_for_read_error(error: &std::io::Error) -> DirStatus {
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => DirStatus::Gone,
        _ => DirStatus::Denied,
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
    pub name: String,
    pub name_lower: String,
    pub path: PathBuf,
    /// Lexical filesystem binding captured with the listing the user sees.
    /// Operations must not re-observe it synchronously on the UI thread.
    pub identity: ListingIdentity,
    pub is_dir: bool,
    pub size: u64,
    pub extension: String,
    pub modified: Option<std::time::SystemTime>,
    /// Pre-formatted date string (rendered every frame, formatted once).
    pub modified_str: String,
    /// Pre-formatted size string for files ("…" for dirs).
    pub size_str: String,
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
        Some(Self {
            name,
            name_lower,
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
        })
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
        matches!(
            self.extension.as_str(),
            "mp4" | "mov" | "avi" | "mkv" | "webm" | "m4v" | "wmv" | "flv"
        )
    }

    pub fn size_display(&self) -> &str {
        &self.size_str
    }

    pub fn modified_display(&self) -> &str {
        &self.modified_str
    }
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
/// comma-separated; a leading `!` or `-` marks subtraction. Terms are
/// lowercased for case-insensitive matching.
pub fn parse_mask(mask: &str) -> Vec<(String, bool)> {
    mask.split(',')
        .map(str::trim)
        .map(
            |t| match t.strip_prefix('!').or_else(|| t.strip_prefix('-')) {
                Some(rest) => (rest.trim().to_lowercase(), true),
                None => (t.to_lowercase(), false),
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
fn mask_verdict(terms: &[(String, bool)], entry: &FileEntry) -> Option<bool> {
    let hits = |subtracting: bool| {
        terms.iter().any(|(term, subtract)| {
            *subtract == subtracting && term_matches(term, &entry.name_lower, &entry.extension)
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SortColumn {
    Name,
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
    listing: ListingState,
    selection: SelectionState,
    pub preview: Option<PreviewContent>,
    /// Per-pane directory history (back/forward); a vim-style jump trail that
    /// truncates its forward tail on a new navigation.
    pub history: crate::jumplist::JumpList,
    view: ViewState,
    sizes: SizeIndex,
    watcher: DirectoryWatcherState,
    pub drag: DragState,
    listing_job: ListingJobController,
    workload: Option<crate::workload::WorkloadHandle>,
    /// Why the most recent listing refresh landed (path-free, research J007).
    pub change_provenance: crate::change_provenance::RowProvenance,
    /// A navigation target whose Recent entry waits for its listing: it is
    /// recorded when the read resolves, and dropped when the folder is gone.
    pending_visit: Option<PathBuf>,
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
            listing: ListingState::new(path.clone()),
            selection: SelectionState::new(path.clone()),
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
            workload: None,
            change_provenance: crate::change_provenance::RowProvenance::default(),
            pending_visit: None,
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
            .filter_map(|path| {
                // Like `read_dir`: a symlink shows its target's type (a link
                // to a folder is a folder row) but keeps the link's identity.
                let lexical = fs::symlink_metadata(path).ok()?;
                let followed = lexical
                    .file_type()
                    .is_symlink()
                    .then(|| fs::metadata(path).ok())
                    .flatten();
                let mut entry =
                    FileEntry::from_meta(path.clone(), followed.as_ref().unwrap_or(&lexical))?;
                entry.identity = ListingIdentity::Captured(
                    crate::path_identity::PathIdentity::from_metadata(path.clone(), &lexical),
                );
                Some(entry)
            })
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
        let ticket = self.watcher.snapshot_ticket_for(&path);
        let entries = match Self::read_dir(&path, candidate.show_hidden()) {
            DirectoryRead::Complete(entries) => entries,
            DirectoryRead::Incomplete(status) => {
                self.watcher.defer_snapshot(ticket.as_ref());
                return ViewApplyOutcome::ReadRejected(status);
            }
        };

        self.view.commit_config(candidate);
        self.publish_complete_listing(entries, candidate);
        self.sizes.bind(&path);
        self.watcher.ensure_binding(&path);
        self.commit_snapshot(ticket, true);
        ViewApplyOutcome::Applied
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

    pub fn refresh(&mut self) {
        self.change_provenance
            .record(crate::change_provenance::ChangeProvenance::Commander);
        self.schedule_or_reload(PendingFocus::None);
    }

    fn can_async_list(&self) -> bool {
        self.workload.is_some() && self.watcher.notify().is_some()
    }

    fn schedule_or_reload(&mut self, focus: PendingFocus) {
        if self.can_async_list() {
            self.schedule_listing(focus);
        } else {
            self.refresh_sync(focus);
        }
    }

    fn refresh_sync(&mut self, focus: PendingFocus) {
        let path = self.current_path.clone();
        self.sizes.bind(&path);
        // Subscribe before taking the snapshot. A callback racing with the
        // read stays queued with this exact binding and forces a later pass.
        self.watcher.ensure_binding(&path);
        let ticket = self.watcher.snapshot_ticket();
        if self.reload_entries() {
            self.commit_snapshot(ticket, true);
            self.apply_pending_focus(focus);
        } else {
            self.watcher.defer_snapshot(ticket.as_ref());
        }
    }

    fn schedule_listing(&mut self, focus: PendingFocus) {
        let path = self.current_path.clone();
        let show_hidden = self.view.show_hidden();
        let binding_changed = self.listing.binding() != path;
        self.sizes.bind(&path);
        self.watcher.ensure_binding(&path);
        let ticket = self.watcher.snapshot_ticket();
        if binding_changed {
            self.selection.bind(&path);
            self.drag.clear();
            self.listing.begin_loading(path.clone());
        }
        self.listing_job.request(path, show_hidden, ticket, focus);
        // Opportunistically admit/poll within this call so fast local disks
        // still settle before the next frame when the worker is free.
        let _ = self.poll_listing_results();
    }

    /// Drive in-flight listings and publish generation-checked results.
    /// Returns `true` when a listing was applied.
    pub fn poll_listing(&mut self) -> bool {
        self.poll_listing_results()
    }

    fn poll_listing_results(&mut self) -> bool {
        let Some(workload) = self.workload.clone() else {
            return false;
        };
        let Some(notify) = self.watcher.notify() else {
            return false;
        };
        self.listing_job.drive(&workload, notify, Self::read_dir);
        let Some(ready) = self.listing_job.take_ready() else {
            return false;
        };
        if ready.binding.path != self.current_path
            || ready.binding.show_hidden != self.view.show_hidden()
            || self
                .listing_job
                .desired_binding()
                .is_some_and(|desired| desired.generation > ready.binding.generation)
        {
            // Stale relative to panel intent; keep waiting for the current job.
            return false;
        }
        let applied = self.apply_directory_read(ready.read);
        if applied {
            self.commit_snapshot(ready.ticket, ready.forced_sizes);
            self.apply_pending_focus(ready.focus);
            if let Some(recovered_gap) = ready.watcher_gap {
                self.change_provenance
                    .record(watcher_provenance(recovered_gap));
                crate::watcher_health::record_listing_reconciliation(recovered_gap);
            }
        } else if let Some(ticket) = ready.ticket.as_ref() {
            self.watcher.defer_snapshot(Some(ticket));
            let _ = ready.focus;
        } else {
            let _ = ready.focus;
        }
        applied
    }

    /// A complete listing was published: acknowledge the watcher snapshot it
    /// answers (against the binding the rows now belong to) and re-plan
    /// folder sizes for it. `forced_sizes` lets expensive folders re-walk.
    fn commit_snapshot(&mut self, ticket: Option<ReconciliationTicket>, forced_sizes: bool) {
        let listing_binding = self.listing.binding().to_path_buf();
        self.watcher.acknowledge_snapshot(ticket, &listing_binding);
        self.refresh_sizes(forced_sizes);
    }

    fn apply_pending_focus(&mut self, focus: PendingFocus) {
        match focus {
            PendingFocus::None => {}
            PendingFocus::Remembered {
                cursor_path,
                scroll_anchor,
            } => {
                self.set_scroll_anchor(scroll_anchor.min(self.filtered_count().saturating_sub(1)));
                // No remembered path means the cursor was on the `..` row;
                // only a path that vanished falls back to the scroll anchor.
                let cursor = match cursor_path {
                    None => 0,
                    Some(path) => self
                        .filtered_position(|entry| entry.path == path)
                        .map_or_else(
                            || {
                                self.scroll_anchor()
                                    .saturating_add(1)
                                    .min(self.filtered_count())
                            },
                            |index| index + 1,
                        ),
                };
                self.set_cursor(cursor);
                self.set_scroll_to_cursor(self.cursor() > 0);
            }
            PendingFocus::NamedChild(name) => {
                if let Some(idx) = self.named_child_position(&name) {
                    self.reveal_cursor(idx + 1);
                }
            }
        }
    }

    /// Re-read the directory, preserving selection and cursor position
    /// by path (entries may have been added, removed or re-sorted).
    fn reload_entries(&mut self) -> bool {
        let read = Self::read_dir(&self.current_path, self.view.show_hidden());
        self.apply_directory_read(read)
    }

    fn apply_directory_read(&mut self, read: DirectoryRead) -> bool {
        let binding = self.current_path.clone();
        let binding_changed = self.listing.binding() != binding;
        if self.pending_visit.as_ref() == Some(&binding) {
            self.pending_visit = None;
            if !matches!(read, DirectoryRead::Incomplete(DirStatus::Gone)) {
                record_visit(&binding);
            }
        }
        match read {
            DirectoryRead::Complete(entries) => {
                self.publish_complete_listing(entries, self.view.config());
                true
            }
            DirectoryRead::Incomplete(status) => {
                if binding_changed {
                    self.selection.bind(&binding);
                    self.sizes.bind(&binding);
                    self.drag.clear();
                }
                let changed = self.listing.mark_incomplete(binding, status);
                debug_assert_eq!(changed, binding_changed);
                false
            }
        }
    }

    fn publish_complete_listing(&mut self, mut entries: Vec<FileEntry>, config: ViewConfig) {
        let binding = self.current_path.clone();
        let binding_changed = self.listing.binding() != binding;
        let cursor_path = (!binding_changed).then(|| self.focused_path()).flatten();
        let old_cursor = if binding_changed { 0 } else { self.cursor() };

        if binding_changed {
            self.selection.bind(&binding);
            self.sizes.bind(&binding);
            self.drag.clear();
        }

        let status = if entries.is_empty() {
            DirStatus::Empty
        } else {
            DirStatus::Listed
        };
        sort_entries(&mut entries, config);
        self.selection.publish_complete(&binding);
        self.listing.replace(binding, entries, status);

        self.selection
            .retain_present(self.listing.entries().iter().map(|entry| &entry.path));

        if !binding_changed {
            self.restore_cursor_focus(cursor_path, old_cursor);
        }
    }

    /// Check if fs watcher flagged a change; if so, refresh.
    /// Returns `true` when the directory listing was re-read.
    /// Deep events (below the watched dir) only recompute directory
    /// sizes, debounced so event floods during transfers don't thrash.
    pub fn poll_fs_changes(&mut self) -> bool {
        // Polled every frame: the filtered view can change with time alone
        // (age facets), so keep the cursor on its entry first.
        self.ensure_cursor_valid();
        let path = self.current_path.clone();
        let outcome = self.watcher.poll(&path);
        if outcome.sizes_dirty {
            self.sizes.mark_dirty();
        }
        if let Some(ticket) = outcome.ticket {
            let source = watcher_provenance(outcome.recovered_gap);
            if self.can_async_list() {
                let show_hidden = self.view.show_hidden();
                self.listing_job
                    .reconcile(path, show_hidden, ticket, outcome.recovered_gap);
                // The result usually lands on a later poll; whichever poll
                // applies it records the watcher provenance it carries.
                return self.poll_listing_results();
            }
            if self.reload_entries() {
                // Watcher-driven: expensive folders are not re-walked.
                self.commit_snapshot(Some(ticket), false);
                self.change_provenance.record(source);
                crate::watcher_health::record_listing_reconciliation(outcome.recovered_gap);
                return true;
            }
            self.watcher.defer_snapshot(Some(&ticket));
        }
        self.poll_sizes();
        false
    }

    /// Schedule background recomputation of subdirectory sizes and counts.
    /// `forced` marks user-driven refreshes: they may re-walk expensive
    /// dirs, background (watcher-noise) recomputes may not.
    /// Returns `true` when some dir was skipped because of the walk
    /// cooldown and the caller should retry later.
    fn refresh_sizes(&mut self, forced: bool) {
        let filtered = self.filtered_indices();
        let path = self.current_path.clone();
        let scroll_anchor = self.scroll_anchor();
        let page_rows = self.page_rows();
        let notify = self.watcher.notify();
        self.sizes.refresh(
            SizeScanInput {
                path: &path,
                entries: self.listing.entries(),
                filtered,
                scroll_anchor,
                page_rows,
                notify,
                show_hidden: self.view.show_hidden(),
            },
            forced,
        );
    }

    fn poll_sizes(&mut self) {
        let filtered = self.filtered_indices();
        let path = self.current_path.clone();
        let input = SizeScanInput {
            path: &path,
            entries: self.listing.entries(),
            filtered,
            scroll_anchor: self.scroll_anchor(),
            page_rows: self.page_rows(),
            notify: self.watcher.notify(),
            show_hidden: self.view.show_hidden(),
        };
        self.sizes.poll(input);
    }
    pub(super) fn read_dir(path: &Path, show_hidden: bool) -> DirectoryRead {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => return DirectoryRead::Incomplete(status_for_read_error(&error)),
        };
        Self::read_dir_entries(entries, show_hidden)
    }

    /// Build rows from a directory iterator. A child that vanished between
    /// readdir and stat is simply not listed: it is gone, not unreadable,
    /// so it must not turn the whole read into a partial one.
    fn read_dir_entries(
        entries: impl Iterator<Item = std::io::Result<fs::DirEntry>>,
        show_hidden: bool,
    ) -> DirectoryRead {
        let vanished = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
        let mut result = Vec::new();
        let mut complete = true;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            let path = entry.path();
            if !show_hidden && path.file_name().is_some_and(is_hidden_name) {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    complete &= vanished(&error);
                    continue;
                }
            };
            let lexical = match entry.metadata() {
                Ok(metadata) => Ok(metadata),
                Err(error) if vanished(&error) => continue,
                Err(error) => Err(crate::ports::NativeFailure::from_io(&error)),
            };
            let followed = file_type
                .is_symlink()
                .then(|| fs::metadata(&path).ok())
                .flatten();
            let dangling = file_type.is_symlink() && followed.is_none();
            let display_metadata = followed.as_ref().or_else(|| lexical.as_ref().ok());
            let Some(display_metadata) = display_metadata else {
                complete = false;
                continue;
            };
            match FileEntry::from_meta(path.clone(), display_metadata) {
                Some(mut file_entry) => {
                    if dangling {
                        // The lstat size is the link text's length, not
                        // content: a broken link has no size to show.
                        file_entry.size = 0;
                        file_entry.size_str = "–".to_string();
                    }
                    file_entry.identity = match lexical {
                        Ok(metadata) => ListingIdentity::Captured(
                            crate::path_identity::PathIdentity::from_metadata(path, &metadata),
                        ),
                        Err(failure) => ListingIdentity::CaptureFailed(failure),
                    };
                    result.push(file_entry);
                }
                None => complete = false,
            }
        }
        if complete {
            DirectoryRead::Complete(result)
        } else {
            DirectoryRead::Incomplete(DirStatus::Partial)
        }
    }

    fn sort_entries(&mut self) {
        self.preserving_focus(|panel| {
            let config = panel.view.config();
            panel
                .listing
                .resort(|entries| sort_entries(entries, config));
        });
    }

    /// Toggle pinning folders to the top, then re-sort in place.
    pub fn toggle_folders_first(&mut self) {
        self.view.toggle_folders_first();
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

    /// Add the well-known clutter files in the filtered view to the selection
    /// (`.DS_Store`, `.localized`, `Thumbs.db`, `desktop.ini`, the custom-icon
    /// `Icon\r`). Folders are never matched. Returns how many were added.
    pub fn select_junk(&mut self) -> usize {
        // Lowercased: case-insensitive volumes carry variants such as
        // `thumbs.db` or `Desktop.ini`. `Icon\r` stays an exact match.
        const JUNK_NAMES_LOWER: &[&str] = &[".ds_store", ".localized", "thumbs.db", "desktop.ini"];
        self.select_filtered_where(|entry| {
            !entry.is_dir
                && (entry.name == "Icon\r" || JUNK_NAMES_LOWER.contains(&entry.name_lower.as_str()))
        })
    }

    /// Select the `n` largest files in the filtered view (folders excluded).
    /// Returns how many entries were newly added to the selection.
    pub fn select_largest(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        // Top-n needs sizes before mutation; keep a bounded heap instead of a
        // full filtered `Vec<&FileEntry>` plus a second path list.
        let mut heap = std::collections::BinaryHeap::<std::cmp::Reverse<(u64, PathBuf)>>::new();
        for entry in self.filtered_iter().filter(|entry| !entry.is_dir) {
            heap.push(std::cmp::Reverse((entry.size, entry.path.clone())));
            if heap.len() > n {
                heap.pop();
            }
        }
        let mut added = 0;
        while let Some(std::cmp::Reverse((_, path))) = heap.pop() {
            added += usize::from(self.selection.insert_selected(path));
        }
        added
    }

    /// Select every filtered file sharing the cursor file's extension. Does
    /// nothing if the cursor is on `..`, a folder, or an extension-less file.
    /// Returns how many entries were added.
    pub fn select_same_extension_as_cursor(&mut self) -> usize {
        let ext = match self.cursor_entry() {
            Some(e) if !e.is_dir && !e.extension.is_empty() => e.extension.clone(),
            _ => return 0,
        };
        self.select_filtered_where(|entry| !entry.is_dir && entry.extension == ext)
    }

    /// Select the zero-byte files in the filtered view (folders excluded).
    /// Returns how many entries were added.
    pub fn select_empty_files(&mut self) -> usize {
        self.select_filtered_where(|entry| !entry.is_dir && entry.size == 0)
    }

    /// Flip the current sort order (ascending <-> descending) and re-sort.
    pub fn reverse_sort(&mut self) {
        self.view.reverse_sort();
        self.sort_entries();
    }

    /// Entries to export/copy as a listing: the current selection if anything
    /// is selected, otherwise the whole filtered view (in display order).
    pub fn listing_entries(&self) -> Vec<&FileEntry> {
        if self.selection.selected().is_empty() {
            self.filtered_entries()
        } else {
            self.visible_selected().collect()
        }
    }

    /// Apply a select-by-mask line to the selection over the filtered view:
    /// add terms select matching entries, `!`/`-` terms deselect them
    /// (subtraction wins per entry). Returns how many entries were added.
    pub fn select_by_mask(&mut self, mask: &str) -> usize {
        let terms = parse_mask(mask);
        if terms.is_empty() {
            return 0;
        }
        let mut added = 0;
        self.update_filtered_selection(|entry, selection| match mask_verdict(&terms, entry) {
            Some(false) => {
                selection.remove_selected(&entry.path);
            }
            Some(true) => added += usize::from(selection.insert_selected(entry.path.clone())),
            None => {}
        });
        added
    }

    /// How many filtered entries would change selection state if `mask` were
    /// applied. A subtraction-only mask therefore reports zero until it has a
    /// selected match, which keeps the preview aligned with the actual action.
    pub fn mask_match_count(&self, mask: &str) -> usize {
        let terms = parse_mask(mask);
        if terms.is_empty() {
            return 0;
        }
        let mut changes = 0;
        self.visit_filtered(|_, entry| {
            let selected = self.selection.selected().contains(&entry.path);
            let next = mask_verdict(&terms, entry).unwrap_or(selected);
            changes += usize::from(next != selected);
            true
        });
        changes
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
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    /// In-memory entry for pure sort/filter/selection tests.
    fn entry(name: &str, is_dir: bool, size: u64) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: PathBuf::from(format!("/test/{name}")),
            identity: ListingIdentity::Unavailable,
            is_dir,
            size,
            extension: String::new(),
            modified: None,
            modified_str: "–".to_string(),
            size_str: if is_dir {
                "…".to_string()
            } else {
                format_size(size)
            },
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn child_vanishing_between_readdir_and_stat_is_not_a_partial_read() {
        let temp = TempDir::new();
        temp.file("kept.txt", "k");
        let gone = temp.file("gone.txt", "g");
        // The first `next` buffers this small directory's names; the file
        // then disappears before its metadata is read.
        let mut entries = fs::read_dir(temp.path()).unwrap().peekable();
        let _ = entries.peek();
        fs::remove_file(gone).unwrap();

        let DirectoryRead::Complete(rows) = PanelState::read_dir_entries(entries, true) else {
            panic!("a vanished child must not make the read partial");
        };
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["kept.txt"]);
    }

    #[test]
    fn picked_entries_drop_rows_inside_a_picked_folder() {
        let tmp = TempDir::new();
        let folder = tmp.dir("folder");
        let inner = tmp.file("folder/inner.txt", "a");
        let other = tmp.file("other.txt", "b");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        p.toggle_select(folder.clone());
        p.toggle_select(other.clone());
        p.navigate_to(folder.clone());
        p.toggle_select(inner);

        let mut picked: Vec<PathBuf> = p
            .picked_entries()
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        picked.sort();
        assert_eq!(picked, vec![folder, other]);
    }

    #[cfg(unix)]
    #[test]
    fn tree_subdirs_include_folder_symlinks() {
        let temp = TempDir::new();
        let target = temp.dir("target");
        temp.file("file.txt", "f");
        let link = temp.path().join("link");
        std::os::unix::fs::symlink("target", &link).unwrap();
        std::os::unix::fs::symlink("file.txt", temp.path().join("file-link")).unwrap();

        assert_eq!(PanelState::subdirs(temp.path(), false), vec![link, target]);
    }

    #[test]
    fn tree_subdirs_use_the_file_lists_natural_order() {
        let temp = TempDir::new();
        let dir10 = temp.dir("dir10");
        let dir2 = temp.dir("dir2");
        let alpha = temp.dir("Alpha");
        temp.dir(".hidden");
        temp.file("file.txt", "");

        assert_eq!(
            PanelState::subdirs(temp.path(), false),
            vec![alpha, dir2, dir10]
        );
    }

    #[cfg(unix)]
    #[test]
    fn selection_elsewhere_reads_folder_symlinks_like_the_listing() {
        let temp = TempDir::new();
        let target = temp.dir("target");
        let link = temp.path().join("folder-link");
        std::os::unix::fs::symlink("target", &link).unwrap();

        let mut p = PanelState::new(temp.path().to_path_buf());
        p.refresh();
        p.toggle_select(link.clone());
        p.navigate_to(target);

        let picked = p.picked_entries().unwrap();
        let entry = picked.iter().find(|entry| entry.path == link).unwrap();
        assert!(entry.is_dir, "a folder link is a folder row");
        assert!(entry.is_symlink(), "identity stays the link itself");
    }

    #[cfg(unix)]
    #[test]
    fn listing_keeps_symlinks_lexically_bound_without_losing_directory_ux() {
        let temp = TempDir::new();
        temp.dir("target");
        std::os::unix::fs::symlink("target", temp.path().join("folder-link")).unwrap();
        std::os::unix::fs::symlink("missing", temp.path().join("broken-link")).unwrap();

        let DirectoryRead::Complete(entries) = PanelState::read_dir(temp.path(), true) else {
            panic!("symlink listing should be complete");
        };
        let folder = entries
            .iter()
            .find(|entry| entry.name == "folder-link")
            .expect("directory symlink");
        assert!(folder.is_dir, "working directory links remain navigable");
        assert!(matches!(
            &folder.identity,
            ListingIdentity::Captured(identity)
                if identity.kind == Some(crate::path_identity::PathKind::Symlink)
        ));

        let broken = entries
            .iter()
            .find(|entry| entry.name == "broken-link")
            .expect("broken symlink");
        assert!(!broken.is_dir);
        assert!(matches!(
            &broken.identity,
            ListingIdentity::Captured(identity)
                if identity.kind == Some(crate::path_identity::PathKind::Symlink)
        ));
        // Not sized by the length of the link text ("missing" = 7 B).
        assert_eq!(broken.size, 0);
        assert_eq!(broken.size_display(), "–");

        // Get Info says what the rows are, and where a working link points.
        let card = make_info(folder, None, None);
        assert_eq!(card.kind, "Symlink to Folder");
        assert!(
            card.path.ends_with("folder-link \u{2192} target"),
            "{}",
            card.path
        );
        assert_eq!(make_info(broken, None, None).kind, "Broken symlink");
    }

    fn panel_with(entries: Vec<FileEntry>) -> PanelState {
        let mut p = PanelState::new(PathBuf::from("/test"));
        p.replace_entries_for_test(entries);
        p
    }

    #[test]
    fn dragging_an_unselected_row_does_not_use_the_stale_selection() {
        let mut panel = panel_with(vec![
            entry("selected.txt", false, 1),
            entry("dragged.txt", false, 1),
        ]);
        panel.select_path(PathBuf::from("/test/selected.txt"));

        panel.begin_drag(PathBuf::from("/test/dragged.txt"));

        assert_eq!(panel.drag.entries(), [PathBuf::from("/test/dragged.txt")]);
    }

    #[test]
    fn dragging_a_selected_row_uses_the_visible_selection() {
        let mut panel = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 1)]);
        panel.extend_selection([PathBuf::from("/test/a.txt"), PathBuf::from("/test/b.txt")]);

        panel.begin_drag(PathBuf::from("/test/b.txt"));

        assert_eq!(
            panel.drag.entries(),
            [PathBuf::from("/test/a.txt"), PathBuf::from("/test/b.txt")]
        );
    }

    #[test]
    fn modified_dates_outside_chronos_range_show_as_unknown() {
        use std::time::Duration;
        let epoch = SystemTime::UNIX_EPOCH;
        assert!(format_modified(epoch + Duration::from_secs(86_400 * 365)).is_some());
        // Before the epoch, including a fractional second.
        assert!(format_modified(epoch - Duration::from_millis(1_500)).is_some());
        // A million years out: storable on Unix filesystems, beyond chrono.
        // (Windows' `SystemTime` cannot hold it, so there is nothing to test.)
        for far in [
            epoch.checked_add(Duration::from_secs(1 << 45)),
            epoch.checked_sub(Duration::from_secs(1 << 45)),
            epoch.checked_add(Duration::from_secs(i64::MAX as u64)),
        ]
        .into_iter()
        .flatten()
        {
            assert_eq!(format_modified(far), None);
        }
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.00 GB");
        // Just below a boundary rounds up into the next unit.
        assert_eq!(format_size(1024 * 1024 - 1), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024 - 1), "1.00 GB");
        assert_eq!(format_size(1024 * 1024 - 52), "1023.9 KB");
        // Terabytes and beyond get their own units.
        let tib = 1024_u64.pow(4);
        assert_eq!(format_size(5 * tib), "5.00 TB");
        assert_eq!(format_size(tib - 1), "1.00 TB");
        assert_eq!(format_size(3 * 1024 * tib), "3.00 PB");
        assert_eq!(format_size(u64::MAX), "16384.00 PB");
    }

    #[test]
    fn sort_puts_dirs_first_and_is_case_insensitive() {
        let mut p = panel_with(vec![
            entry("zeta.txt", false, 1),
            entry("Apple", true, 0),
            entry("beta.txt", false, 1),
            entry("zoo", true, 0),
        ]);
        p.sort_entries();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "zoo", "beta.txt", "zeta.txt"]);
    }

    #[test]
    fn sort_desc_reverses_within_groups() {
        let mut p = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 2)]);
        p.set_sort(SortColumn::Size); // asc
        p.set_sort(SortColumn::Size); // same column again -> desc
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["b.txt", "a.txt"]);
    }

    #[test]
    fn descending_sort_keeps_folders_first_and_reverses_within_each_group() {
        let mut p = panel_with(vec![
            entry("small-dir", true, 1),
            entry("large-file", false, 20),
            entry("large-dir", true, 10),
            entry("small-file", false, 2),
        ]);

        p.set_sort(SortColumn::Size);
        p.set_sort(SortColumn::Size);

        let names: Vec<&str> = p
            .entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["large-dir", "small-dir", "large-file", "small-file"]
        );
    }

    #[test]
    fn folders_first_off_sorts_dirs_inline() {
        let mut p = panel_with(vec![
            entry("zeta.txt", false, 1),
            entry("Apple", true, 0),
            entry("beta.txt", false, 1),
            entry("zoo", true, 0),
        ]);
        // Off: folders are no longer pinned, names sort as one stream.
        p.toggle_folders_first();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "beta.txt", "zeta.txt", "zoo"]);
        // Back on: folders return to the top.
        p.toggle_folders_first();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "zoo", "beta.txt", "zeta.txt"]);
    }

    #[test]
    fn natural_sort_toggle_switches_to_ascii_order() {
        let mut p = panel_with(vec![
            entry("file10.txt", false, 1),
            entry("file2.txt", false, 1),
        ]);
        // Natural (default): file2 before file10.
        p.sort_entries();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["file2.txt", "file10.txt"]);
        // ASCII: "file10" sorts before "file2" lexicographically.
        p.toggle_natural_sort();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["file10.txt", "file2.txt"]);
    }

    #[test]
    fn sort_by_extension_groups_by_ext_then_name() {
        let mut p = panel_with(vec![
            entry("b.txt", false, 1),
            entry("a.rs", false, 1),
            entry("c.txt", false, 1),
            entry("z.rs", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.set_sort(SortColumn::Extension);
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a.rs", "z.rs", "b.txt", "c.txt"]);
    }

    #[test]
    fn sort_by_kind_orders_image_before_doc_before_code() {
        let mut p = panel_with(vec![
            entry("main.rs", false, 1),
            entry("pic.jpg", false, 1),
            entry("doc.pdf", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.set_sort(SortColumn::Kind);
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        // Declaration order of Kind: Image, Document, Code.
        assert_eq!(names, vec!["pic.jpg", "doc.pdf", "main.rs"]);
    }

    #[test]
    fn select_junk_picks_known_clutter_only() {
        let mut p = panel_with(vec![
            entry(".DS_Store", false, 6),
            entry("photo.jpg", false, 100),
            entry("Thumbs.db", false, 10),
            entry("notes.txt", false, 20),
        ]);
        let n = p.select_junk();
        assert_eq!(n, 2);
        let names: std::collections::HashSet<String> = p
            .selected_paths()
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(".DS_Store"));
        assert!(names.contains("Thumbs.db"));
        assert!(!names.contains("photo.jpg"));
    }

    #[test]
    fn select_junk_matches_case_variants() {
        let mut p = panel_with(vec![
            entry("thumbs.db", false, 10),
            entry("Desktop.ini", false, 10),
            entry("notes.txt", false, 20),
        ]);
        assert_eq!(p.select_junk(), 2);
        let names = selected_names(&p);
        assert!(names.contains("thumbs.db"));
        assert!(names.contains("Desktop.ini"));
    }

    fn selected_names(p: &PanelState) -> std::collections::HashSet<String> {
        p.selected_paths()
            .iter()
            .filter_map(|x| x.file_name().map(|s| s.to_string_lossy().to_string()))
            .collect()
    }

    #[test]
    fn select_largest_picks_top_n_by_size() {
        let mut p = panel_with(vec![
            entry("a", false, 10),
            entry("b", false, 50),
            entry("c", false, 30),
            entry("dir", true, 0),
        ]);
        assert_eq!(p.select_largest(2), 2);
        let names = selected_names(&p);
        assert!(names.contains("b")); // 50
        assert!(names.contains("c")); // 30
        assert!(!names.contains("a"));
        assert!(!names.contains("dir"));
    }

    #[test]
    fn select_like_cursor_matches_extension() {
        let mut p = panel_with(vec![
            entry("a.rs", false, 1),
            entry("b.txt", false, 1),
            entry("c.rs", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.set_cursor(1); // first filtered entry: a.rs
        assert_eq!(p.select_same_extension_as_cursor(), 2);
        let names = selected_names(&p);
        assert!(names.contains("a.rs"));
        assert!(names.contains("c.rs"));
        assert!(!names.contains("b.txt"));
    }

    #[test]
    fn select_empty_files_picks_zero_byte_only() {
        let mut p = panel_with(vec![
            entry("empty", false, 0),
            entry("full", false, 100),
            entry("dir", true, 0),
        ]);
        assert_eq!(p.select_empty_files(), 1);
        let names = selected_names(&p);
        assert!(names.contains("empty"));
        assert!(!names.contains("full"));
        assert!(!names.contains("dir")); // a folder is never "empty file"
    }

    #[test]
    fn reverse_sort_flips_order() {
        let mut p = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 2)]);
        p.sort_entries(); // Name asc: a, b
        p.reverse_sort(); // -> desc: b, a
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["b.txt", "a.txt"]);
    }

    #[test]
    fn listing_entries_uses_selection_when_present() {
        let mut p = panel_with(vec![
            entry("a", false, 1),
            entry("b", false, 2),
            entry("c", false, 3),
        ]);
        // Nothing selected: the whole filtered view.
        assert_eq!(p.listing_entries().len(), 3);
        // With a selection: only the selected rows, in display order.
        p.select_path(PathBuf::from("/test/b"));
        let names: Vec<String> = p.listing_entries().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["b"]);
    }

    #[test]
    fn filter_matches_case_insensitively() {
        let mut p = panel_with(vec![
            entry("Cargo.toml", false, 1),
            entry("main.rs", false, 1),
        ]);
        p.set_search_query("CARGO");
        let names: Vec<&str> = p
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["Cargo.toml"]);
    }

    #[test]
    fn whitespace_only_filter_is_inactive_and_matches_everything() {
        let mut p = panel_with(vec![
            entry("Cargo.toml", false, 1),
            entry("main.rs", false, 1),
        ]);
        p.set_search_query("   ");
        assert!(!filter_is_active(p.search_query(), &p.facets()));
        assert_eq!(p.filtered_count(), 2);
    }

    #[test]
    fn filter_is_a_fuzzy_subsequence() {
        let mut p = panel_with(vec![
            entry("scanner.rs", false, 1),
            entry("main.rs", false, 1),
            entry("notes.txt", false, 1),
        ]);
        // "scn" is a subsequence of scanner.rs only (substring would miss it).
        p.set_search_query("scn");
        let names: Vec<&str> = p
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["scanner.rs"]);

        // Empty query shows everything.
        p.set_search_query("");
        assert_eq!(p.filtered_count(), 3);

        // A non-subsequence excludes every row.
        p.set_search_query("zzz");
        assert_eq!(p.filtered_count(), 0);
    }

    #[test]
    fn toggle_select_adds_then_removes() {
        let mut p = panel_with(vec![entry("a", false, 1)]);
        let path = p.entries()[0].path.clone();
        p.toggle_select(path.clone());
        assert!(p.is_selected(&path));
        p.toggle_select(path.clone());
        assert!(!p.is_selected(&path));
    }

    #[test]
    fn select_all_toggles_between_all_and_none() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.select_all();
        assert_eq!(p.selected_count(), 2);
        p.select_all();
        assert!(p.selection_is_empty());
    }

    #[test]
    fn select_all_preserves_selection_hidden_by_filter() {
        let mut p = panel_with(vec![
            entry("alpha", false, 1),
            entry("album", false, 1),
            entry("zebra", false, 1),
        ]);
        let alpha = p.entries()[0].path.clone();
        let album = p.entries()[1].path.clone();
        let zebra = p.entries()[2].path.clone();
        p.select_path(zebra.clone());
        p.set_search_query("al");

        p.select_all();
        assert!(p.is_selected(&alpha));
        assert!(p.is_selected(&album));
        assert!(p.is_selected(&zebra));

        p.select_all();
        assert!(!p.is_selected(&alpha));
        assert!(!p.is_selected(&album));
        assert!(p.is_selected(&zebra));
    }

    #[test]
    fn visible_selection_metrics_do_not_count_filtered_out_items() {
        let mut panel = panel_with(vec![entry("alpha", false, 10), entry("zebra", false, 90)]);
        panel.extend_selection([PathBuf::from("/test/alpha"), PathBuf::from("/test/zebra")]);
        panel.set_search_query("alpha");

        assert_eq!(panel.selected_count(), 2);
        assert_eq!(panel.visible_selected_count(), 1);
        assert_eq!(panel.visible_selected_size(), 10);
        assert_eq!(
            panel.selected_or_cursor().unwrap()[0].path,
            PathBuf::from("/test/alpha")
        );
    }

    #[test]
    fn incomplete_snapshot_preserves_selection_and_reconciliation() {
        let mut panel = panel_with(vec![entry("keep.txt", false, 10)]);
        let path = PathBuf::from("/test/keep.txt");
        panel.select_path(path.clone());
        panel.watcher.activate_test_binding(Path::new("/test"));
        let ticket = panel.watcher.snapshot_ticket();

        for status in [DirStatus::Partial, DirStatus::Denied, DirStatus::Gone] {
            assert!(!panel.apply_directory_read(DirectoryRead::Incomplete(status)));
            assert_eq!(panel.entries().len(), 1);
            assert!(panel.is_selected(&path));
            assert_eq!(panel.dir_status(), status);
        }
        panel.watcher.defer_snapshot(ticket.as_ref());
        assert!(panel.watcher.has_pending_reconciliation_for_test());
    }

    fn assert_incomplete_navigation_hides_old_binding(status: DirStatus) {
        let old_entry = entry("keep.txt", false, 10);
        let old_path = old_entry.path.clone();
        let mut panel = panel_with(vec![old_entry.clone()]);
        panel.select_path(old_path.clone());
        panel.set_cursor(1);
        panel.begin_drag(old_path.clone());
        panel.drag.set_drop_target(PathBuf::from("/test"));

        let old_listing_revision = panel.entries_gen();
        let old_size_revision = panel.size_snapshot().revision();
        let old_filter = panel.filtered_indices();
        assert_eq!(panel.folder_overview().total, Some(10));

        panel.current_path = PathBuf::from("/unavailable");
        assert!(!panel.apply_directory_read(DirectoryRead::Incomplete(status)));

        assert_eq!(panel.listing.binding(), Path::new("/unavailable"));
        assert!(panel.entries().is_empty());
        assert!(panel.filtered_indices().is_empty());
        assert!(!Arc::ptr_eq(&old_filter, &panel.filtered_indices()));
        assert!(panel.entries_gen() > old_listing_revision);
        assert!(panel.size_snapshot().revision() > old_size_revision);
        assert_eq!(
            panel.folder_overview(),
            FolderOverview {
                total: Some(0),
                largest: None,
                oldest: None,
            }
        );
        assert!(panel.selected_paths().is_empty());
        assert!(panel.selected_or_cursor().unwrap().is_empty());
        assert_eq!(panel.cursor(), 0);
        assert!(panel.cursor_entry().is_none());
        assert!(panel.drag.is_empty());
        assert!(!panel.drag.has_drop_target());

        panel.current_path = PathBuf::from("/test");
        assert!(panel.apply_directory_read(DirectoryRead::Complete(vec![old_entry])));
        assert!(panel.is_selected(&old_path));
        let actionable = panel.selected_or_cursor().unwrap();
        assert_eq!(actionable.len(), 1);
        assert_eq!(actionable[0].path, old_path);
    }

    #[test]
    fn gone_after_binding_switch_exposes_no_old_actions_and_restores_on_return() {
        assert_incomplete_navigation_hides_old_binding(DirStatus::Gone);
    }

    #[test]
    fn denied_after_binding_switch_exposes_no_old_actions_and_restores_on_return() {
        assert_incomplete_navigation_hides_old_binding(DirStatus::Denied);
    }

    #[test]
    fn partial_after_binding_switch_exposes_no_old_actions_and_restores_on_return() {
        assert_incomplete_navigation_hides_old_binding(DirStatus::Partial);
    }

    #[test]
    fn invert_selection_flips_only_filtered_rows() {
        let mut p = panel_with(vec![
            entry("alpha", false, 1),
            entry("album", false, 1),
            entry("zebra", false, 1),
        ]);
        let alpha = p.entries()[0].path.clone();
        let album = p.entries()[1].path.clone();
        let zebra = p.entries()[2].path.clone();
        // Pre-select one visible (alpha) and one that the filter will hide (zebra).
        p.extend_selection([alpha.clone(), zebra.clone()]);
        // Filter to the "al" rows; zebra is now hidden from the view.
        p.set_search_query("al");
        p.invert_selection();
        assert!(!p.is_selected(&alpha), "visible+selected -> cleared");
        assert!(p.is_selected(&album), "visible+unselected -> selected");
        assert!(p.is_selected(&zebra), "filtered-out row keeps its state");
    }

    #[test]
    fn selected_or_cursor_falls_back_to_cursor_row() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.set_cursor(0); // ".." row
        assert!(p.selected_or_cursor().unwrap().is_empty());
        p.set_cursor(2); // second file
        let picked = p.selected_or_cursor().unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "b");
    }

    #[test]
    fn sorting_reclamps_a_cursor_after_the_filter_changes() {
        let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
        p.set_cursor(2);
        p.set_search_query("alpha");

        p.sort_entries();

        assert_eq!(p.cursor(), 1);
        assert_eq!(p.filtered_get(0).unwrap().name, "alpha");
    }

    #[test]
    fn filtering_out_the_focused_row_lands_on_the_first_match() {
        let mut p = panel_with(vec![
            entry("alpha.txt", false, 1),
            entry("beta.txt", false, 1),
            entry("gamma.rs", false, 1),
            entry("delta.rs", false, 1),
            entry("omega.txt", false, 1),
        ]);
        p.sort_entries();
        // alpha, beta, delta, gamma, omega: focus omega (row 5).
        p.set_cursor(5);
        assert_eq!(p.cursor_entry().unwrap().name, "omega.txt");

        p.set_search_query(".rs");

        assert_eq!(p.cursor(), 1);
        assert_eq!(p.cursor_entry().unwrap().name, "delta.rs");
        assert_eq!(filtered_cursor(0, None, 3), 0, "the .. row stays put");
        assert_eq!(filtered_cursor(4, Some(2), 3), 2);
        assert_eq!(filtered_cursor(4, None, 0), 0);
    }

    #[test]
    fn sort_and_filter_keep_focus_on_the_same_path() {
        let mut p = panel_with(vec![
            entry("beta.txt", false, 20),
            entry("alpha.txt", false, 10),
            entry("notes.md", false, 30),
        ]);
        p.set_cursor(1);
        let focused = p.focused_path().unwrap();

        p.set_search_query("t");
        assert_eq!(p.focused_path(), Some(focused.clone()));
        p.set_sort(SortColumn::Size);
        assert_eq!(p.focused_path(), Some(focused));
    }

    #[test]
    fn equal_sort_keys_use_path_as_a_stable_tie_breaker() {
        let mut z = entry("same.txt", false, 10);
        z.path = PathBuf::from("/z/same.txt");
        let mut a = entry("same.txt", false, 10);
        a.path = PathBuf::from("/a/same.txt");
        let mut p = panel_with(vec![z, a]);

        p.set_sort(SortColumn::Size);

        assert_eq!(p.entries()[0].path, PathBuf::from("/a/same.txt"));
        assert_eq!(p.entries()[1].path, PathBuf::from("/z/same.txt"));

        p.set_sort(SortColumn::Size);
        assert_eq!(p.sort_order(), SortOrder::Desc);
        assert_eq!(p.entries()[0].path, PathBuf::from("/a/same.txt"));
        assert_eq!(p.entries()[1].path, PathBuf::from("/z/same.txt"));
    }

    #[test]
    fn stale_filter_indices_are_bounded_and_cursor_miss_is_explicit() {
        let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
        p.set_cursor(2);
        assert_eq!(p.filtered_count(), 2); // warm the index cache
        p.clear_entries_without_revision_for_test();

        assert!(p.filtered_entries().is_empty());
        assert!(p.filtered_indices().is_empty());
        assert_eq!(
            p.selected_or_cursor().unwrap_err(),
            StaleCursor {
                cursor: 2,
                visible_entries: 0,
            }
        );
    }

    #[test]
    fn reload_preserves_cursor_by_path_and_prunes_selection() {
        let tmp = TempDir::new();
        tmp.file("a.txt", "1");
        tmp.file("b.txt", "2");
        let doomed = tmp.file("c.txt", "3");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        assert_eq!(p.entries().len(), 3);

        // Cursor on "b.txt" (row 2), select "c.txt".
        p.set_cursor(2);
        p.toggle_select(doomed.clone());

        // A new file shifts sort order; a selected file disappears.
        tmp.file("0-first.txt", "0");
        std::fs::remove_file(&doomed).unwrap();
        p.refresh();

        let under_cursor = p.filtered_entries()[p.cursor() - 1].path.clone();
        assert!(under_cursor.ends_with("b.txt"), "cursor follows the path");
        assert!(p.selection_is_empty(), "selection drops deleted paths");
    }

    #[test]
    fn seeded_view_config_drives_the_first_complete_listing() {
        let root = TempDir::new();
        root.file("small.txt", "1");
        root.file("large.txt", "12345");
        root.file(".hidden.txt", "123");
        let config = ViewConfig::default()
            .with_sort(SortColumn::Size, SortOrder::Desc)
            .with_show_hidden(true)
            .with_density(crate::density::Density::Compact);
        let mut panel = PanelState::new_with_view(root.path().to_path_buf(), config);

        panel.refresh();

        assert_eq!(panel.view_config(), config);
        assert_eq!(
            panel
                .entries()
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["large.txt", ".hidden.txt", "small.txt"]
        );
    }

    #[test]
    fn hidden_toggle_commits_config_and_listing_together() {
        let root = TempDir::new();
        root.file("visible.txt", "v");
        root.file(".hidden.txt", "h");
        let mut panel = PanelState::new(root.path().to_path_buf());
        panel.refresh();
        let before_revision = panel.entries_gen();
        assert!(!panel.show_hidden());
        assert_eq!(panel.entries().len(), 1);
        panel.set_cursor(1);
        let focused = panel.cursor_entry().unwrap().path.clone();

        assert_eq!(panel.toggle_hidden(), ViewApplyOutcome::Applied);

        assert!(panel.show_hidden());
        assert_eq!(panel.entries().len(), 2);
        assert_eq!(panel.entries_gen(), before_revision + 1);
        assert_eq!(panel.cursor_entry().unwrap().path, focused);
    }

    #[test]
    fn rejected_hidden_toggle_preserves_complete_view_and_rows() {
        let root = TempDir::new();
        let folder = root.dir("folder");
        root.file("folder/visible.txt", "v");
        root.file("folder/second.txt", "s");
        let mut panel = PanelState::new(folder.clone());
        panel.refresh();
        let selected = panel.entries()[0].path.clone();
        panel.select_path(selected.clone());
        panel.set_cursor(1);
        panel.set_scroll_anchor(1);
        panel.set_search_query("txt");
        panel.watcher.activate_test_binding(&folder);

        let config = panel.view_config();
        let paths = panel
            .entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let revision = panel.entries_gen();
        let status = panel.dir_status();
        let cursor = panel.cursor();
        let scroll_anchor = panel.scroll_anchor();
        let selected = panel.selected_paths().clone();
        let filter = panel.filtered_indices();
        let size_revision = panel.size_snapshot().revision();
        let size_binding = panel.sizes.binding_for_test().to_path_buf();
        let watcher_state = panel.watcher.reconciliation_state_for_test();
        std::fs::remove_dir_all(&folder).unwrap();

        assert_eq!(
            panel.toggle_hidden(),
            ViewApplyOutcome::ReadRejected(DirStatus::Gone)
        );

        assert_eq!(panel.view_config(), config);
        assert_eq!(panel.entries_gen(), revision);
        assert_eq!(panel.dir_status(), status);
        assert_eq!(panel.cursor(), cursor);
        assert_eq!(panel.scroll_anchor(), scroll_anchor);
        assert_eq!(panel.selected_paths(), &selected);
        assert!(Arc::ptr_eq(&filter, &panel.filtered_indices()));
        assert_eq!(panel.size_snapshot().revision(), size_revision);
        assert_eq!(panel.sizes.binding_for_test(), size_binding);
        assert_eq!(panel.watcher.reconciliation_state_for_test(), watcher_state);
        assert_eq!(
            panel
                .entries()
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            paths
        );
    }

    #[test]
    fn watcher_event_racing_hidden_snapshot_remains_pending_for_reconciliation() {
        let root = TempDir::new();
        root.file("before.txt", "before");
        let mut panel = PanelState::new(root.path().to_path_buf());
        panel.refresh();
        panel.watcher.activate_test_binding(root.path());

        let raced = root.file("raced.txt", "raced");
        panel.watcher.inject_current_change_for_test(raced.clone());

        assert_eq!(panel.toggle_hidden(), ViewApplyOutcome::Applied);
        assert!(panel.poll_fs_changes());
        assert!(panel.entries().iter().any(|entry| entry.path == raced));
        assert!(!panel.watcher.has_pending_reconciliation_for_test());
    }

    #[test]
    fn history_navigation_walks_back_and_forward() {
        let tmp = TempDir::new();
        let sub = tmp.dir("sub");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        p.navigate_to(sub.clone());
        assert_eq!(p.current_path, sub);
        assert!(p.can_go_back());

        p.go_back();
        assert_eq!(p.current_path, tmp.path());
        assert!(p.can_go_forward());

        p.go_forward();
        assert_eq!(p.current_path, sub);
    }

    #[test]
    fn history_navigation_prunes_directories_that_disappeared() {
        let temp = TempDir::new();
        let first = temp.dir("first");
        let removed = temp.dir("removed");
        let last = temp.dir("last");

        let mut panel = PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        panel.navigate_to(first.clone());
        panel.navigate_to(removed.clone());
        panel.navigate_to(last.clone());
        std::fs::remove_dir(&removed).unwrap();

        panel.go_back();
        assert_eq!(panel.current_path, first);
        panel.go_forward();
        assert_eq!(panel.current_path, last);
    }

    #[test]
    fn history_back_skips_a_repeat_of_the_current_directory() {
        let temp = TempDir::new();
        let a = temp.dir("a");
        let removed = temp.dir("removed");

        let mut panel = PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        panel.navigate_to(a.clone()); // [root, a]
        panel.navigate_to(removed.clone()); // [root, a, removed]
        panel.navigate_to(a.clone()); // [root, a, removed, a]
        std::fs::remove_dir(&removed).unwrap();

        // Pruning `removed` leaves `a` next to itself; Back moves on to root
        // instead of "reloading" the directory already shown.
        panel.go_back();
        assert_eq!(panel.current_path, temp.path());
        panel.go_forward();
        assert_eq!(panel.current_path, a);
        assert!(!panel.can_go_forward());
    }

    #[test]
    fn navigating_to_a_missing_directory_is_not_a_recent_visit() {
        let temp = TempDir::new();
        let missing = temp.path().join("never-created");
        let mut panel = PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        panel.navigate_to(missing.clone());
        assert!(!visited_paths().contains(&missing));

        let file = temp.file("plain.txt", "x");
        panel.navigate_to(file.clone());
        assert!(!visited_paths().contains(&file));
    }

    #[test]
    fn async_navigation_records_the_visit_when_the_listing_lands() {
        let temp = TempDir::new();
        let target = temp.dir("visit-after-listing");
        let mut panel = PanelState::new(temp.path().to_path_buf());
        panel.refresh();

        // A background listing that never gets a worker: navigation does not
        // stat the folder on the UI thread, so nothing is recorded yet.
        let mut limits = crate::workload::SchedulerLimits::default();
        limits.per_kind_running[0] = 0;
        let runtime = crate::workload::WorkloadRuntime::new(limits);
        panel.set_workload(crate::workload::WorkloadHandle::from_runtime(runtime));
        panel.set_notify(Arc::new(|| {}));
        panel.navigate_to(target.clone());
        assert_eq!(panel.listing.status(), DirStatus::Loading);
        assert!(!visited_paths().contains(&target));

        // Once the read resolves, the visit lands in Recent.
        panel.workload = None;
        panel.refresh();
        assert!(visited_paths().contains(&target));
        assert_eq!(panel.pending_visit, None);
    }

    #[test]
    fn history_jump_back_twice_then_forward_and_truncate() {
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        let b = tmp.dir("b");
        let c = tmp.dir("c");
        let d = tmp.dir("d");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        p.navigate_to(a.clone()); // trail: [root, a]
        p.navigate_to(b.clone()); // [root, a, b]
        p.navigate_to(c.clone()); // [root, a, b, c]

        // Back twice lands on `a`; forward returns to `b`.
        p.go_back();
        p.go_back();
        assert_eq!(p.current_path, a);
        p.go_forward();
        assert_eq!(p.current_path, b);

        // A fresh navigation from `b` truncates the forward tail (drops c).
        p.navigate_to(d.clone());
        assert_eq!(p.current_path, d);
        assert!(!p.can_go_forward(), "forward tail truncated by a new jump");
        // Back now steps to `b`, not the discarded `c`.
        p.go_back();
        assert_eq!(p.current_path, b);
    }

    #[test]
    fn go_up_lands_cursor_on_the_left_directory() {
        let tmp = TempDir::new();
        tmp.dir("aaa");
        let mid = tmp.dir("mmm");
        tmp.dir("zzz");

        let mut p = PanelState::new(mid.clone());
        p.refresh();
        p.go_up();

        assert_eq!(p.current_path, tmp.path());
        // Cursor should sit on "mmm" (the dir we came from), not row 0.
        let under = p.filtered_get(p.cursor() - 1).unwrap();
        assert_eq!(under.name, "mmm");
    }

    #[test]
    fn focus_child_falls_back_to_a_case_insensitive_name() {
        let mut p = panel_with(vec![
            entry("Desktop", true, 0),
            entry("Downloads", true, 0),
            entry("downloads.txt", false, 1),
        ]);
        p.sort_entries();
        // A path typed in another case on a case-insensitive volume.
        p.focus_child("downloads".to_string());
        assert_eq!(p.cursor_entry().unwrap().name, "Downloads");
        // An exact match still wins over a case variant.
        p.focus_child("downloads.txt".to_string());
        assert_eq!(p.cursor_entry().unwrap().name, "downloads.txt");
    }

    #[test]
    fn navigate_to_restores_the_view_remembered_for_that_directory() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        let docs = tmp.dir("docs");

        let mut p = PanelState::new(downloads.clone());
        p.refresh();
        p.set_sort(SortColumn::Size);
        p.reverse_sort();
        assert_eq!(p.toggle_hidden(), ViewApplyOutcome::Applied);
        p.set_density(crate::density::Density::Compact);
        p.set_search_query("download-filter");

        // "docs" has never been visited: like before per-folder memory
        // existed, its view carries over from wherever we came from.
        p.navigate_to(docs.clone());
        assert_eq!(p.sort_column(), SortColumn::Size);
        assert_eq!(p.sort_order(), SortOrder::Desc);
        assert!(p.show_hidden());
        assert_eq!(p.density(), crate::density::Density::Compact);
        assert_eq!(p.search_query(), "");

        // Now give "docs" its own, different view.
        p.set_sort(SortColumn::Extension);
        assert_eq!(p.toggle_hidden(), ViewApplyOutcome::Applied);
        p.set_density(crate::density::Density::Spacious);
        p.set_search_query("docs-filter");

        // Back to "downloads": its own remembered view returns, not "docs"'s.
        p.navigate_to(downloads.clone());
        assert_eq!(p.sort_column(), SortColumn::Size);
        assert_eq!(p.sort_order(), SortOrder::Desc);
        assert!(p.show_hidden());
        assert_eq!(p.density(), crate::density::Density::Compact);
        assert_eq!(p.search_query(), "download-filter");

        // And "docs" kept its own distinct view too.
        p.navigate_to(docs);
        assert_eq!(p.sort_column(), SortColumn::Extension);
        assert_eq!(p.sort_order(), SortOrder::Asc);
        assert!(!p.show_hidden());
        assert_eq!(p.density(), crate::density::Density::Spacious);
        assert_eq!(p.search_query(), "docs-filter");
    }

    #[test]
    fn navigate_to_restores_cursor_path_and_scroll_anchor() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        let docs = tmp.dir("docs");
        tmp.file("downloads/a.txt", "a");
        let focused = tmp.file("downloads/b.txt", "b");
        tmp.file("downloads/c.txt", "c");

        let mut p = PanelState::new(downloads.clone());
        p.refresh();
        let cursor = p
            .filtered_entries()
            .iter()
            .position(|entry| entry.path == focused)
            .unwrap()
            + 1;
        p.set_cursor(cursor);
        p.set_scroll_anchor(1);

        p.navigate_to(docs);
        p.navigate_to(downloads);

        assert_eq!(p.filtered_get(p.cursor() - 1).unwrap().path, focused);
        assert_eq!(p.scroll_anchor(), 1);
        assert!(p.scroll_to_cursor());
    }

    #[test]
    fn cursor_follows_its_entry_when_an_age_facet_expires_rows_above_it() {
        let now = SystemTime::now();
        let mut p = panel_with(vec![
            entry("a.txt", false, 1),
            entry("b.txt", false, 1),
            entry("c.txt", false, 1),
        ]);
        let window = std::time::Duration::from_secs(1);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.modified = Some(if e.name == "a.txt" {
                    // Turns one day old (and leaves the facet) in `window`.
                    now - std::time::Duration::from_secs(24 * 60 * 60) + window
                } else {
                    now
                });
            }
        });
        p.set_facets(FacetSet {
            max_age_days: Some(1),
            ..FacetSet::default()
        });
        assert_eq!(p.filtered_count(), 3, "a.txt is still young enough");
        p.set_cursor(2);
        assert_eq!(p.cursor_entry().unwrap().name, "b.txt");

        std::thread::sleep(window + std::time::Duration::from_millis(100));
        assert_eq!(p.filtered_count(), 2);
        // The per-frame hook re-resolves the cursor with no fs event at all.
        let _ = p.poll_fs_changes();
        assert_eq!(p.cursor(), 1);
        assert_eq!(p.cursor_entry().unwrap().name, "b.txt");
    }

    #[test]
    fn remembered_parent_row_focus_is_restored_on_the_parent_row() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        let docs = tmp.dir("docs");
        tmp.file("downloads/a.txt", "a");
        tmp.file("downloads/b.txt", "b");

        let mut p = PanelState::new(downloads.clone());
        p.refresh();
        p.set_cursor(0);
        p.navigate_to(docs);
        p.navigate_to(downloads);

        assert_eq!(p.cursor(), 0, "the `..` row stays focused");
    }

    #[test]
    fn leaving_a_folder_before_its_listing_lands_keeps_its_remembered_cursor() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        tmp.file("downloads/a.txt", "a");
        let focused = tmp.file("downloads/b.txt", "b");
        tmp.file("downloads/c.txt", "c");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        p.navigate_to(downloads.clone());
        let cursor = p.filtered_position(|entry| entry.path == focused).unwrap() + 1;
        p.set_cursor(cursor);
        p.navigate_to(tmp.path().to_path_buf());

        // Background listings that never get a worker: "downloads" stays
        // loading while we leave it again.
        let mut limits = crate::workload::SchedulerLimits::default();
        limits.per_kind_running[0] = 0;
        let runtime = crate::workload::WorkloadRuntime::new(limits);
        p.set_workload(crate::workload::WorkloadHandle::from_runtime(runtime));
        p.set_notify(Arc::new(|| {}));
        p.navigate_to(downloads.clone());
        assert_eq!(p.listing.status(), DirStatus::Loading);
        p.navigate_to(tmp.path().to_path_buf());

        // Back on synchronous listing, the original focus is still there.
        p.workload = None;
        p.navigate_to(downloads);
        assert_eq!(
            p.cursor_entry().map(|entry| entry.path.clone()),
            Some(focused)
        );
    }

    #[test]
    fn missing_remembered_cursor_falls_back_to_scroll_anchor() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        let docs = tmp.dir("docs");
        tmp.file("downloads/a.txt", "a");
        let focused = tmp.file("downloads/b.txt", "b");
        tmp.file("downloads/c.txt", "c");

        let mut p = PanelState::new(downloads.clone());
        p.refresh();
        p.set_cursor(2);
        p.set_scroll_anchor(1);
        p.navigate_to(docs);
        std::fs::remove_file(focused).unwrap();
        p.navigate_to(downloads);

        assert_eq!(p.cursor(), 2.min(p.filtered_count()));
        assert!(p.cursor() <= p.filtered_count());
    }

    #[test]
    fn natural_cmp_orders_numbers_by_value() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        // Equal numeric value: the shorter run (fewer leading zeros) sorts first.
        assert_eq!(natural_cmp("img9", "img09"), Ordering::Less);
        assert_eq!(natural_cmp("v1.2", "v1.10"), Ordering::Less);
    }

    #[test]
    fn natural_sort_applies_to_listing() {
        let mut p = panel_with(vec![
            entry("file10.txt", false, 1),
            entry("file2.txt", false, 1),
            entry("file1.txt", false, 1),
        ]);
        p.sort_entries();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["file1.txt", "file2.txt", "file10.txt"]);
    }

    #[test]
    fn read_errors_map_to_gone_or_denied() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            status_for_read_error(&Error::from(ErrorKind::NotFound)),
            DirStatus::Gone
        );
        assert_eq!(
            status_for_read_error(&Error::from(ErrorKind::NotADirectory)),
            DirStatus::Gone
        );
        assert_eq!(
            status_for_read_error(&Error::from(ErrorKind::PermissionDenied)),
            DirStatus::Denied
        );
        // Anything else (an I/O error on a dying disk) is unreadable too.
        assert_eq!(
            status_for_read_error(&Error::other("io")),
            DirStatus::Denied
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_dir_distinguishes_empty_denied_gone() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new();
        let status = |path: &Path| {
            let mut panel = PanelState::new(path.to_path_buf());
            panel.refresh();
            panel.dir_status()
        };

        assert_eq!(status(&tmp.dir("empty")), DirStatus::Empty);

        let full = tmp.dir("full");
        tmp.file("full/x.txt", "x");
        assert_eq!(status(&full), DirStatus::Listed);

        assert_eq!(status(&tmp.path().join("nope")), DirStatus::Gone);
        assert_eq!(status(&tmp.file("file.txt", "f")), DirStatus::Gone);

        let denied = tmp.dir("denied");
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&denied).is_ok(); // root ignores modes
        let result = status(&denied);
        let _ = std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o755));
        if !readable {
            assert_eq!(result, DirStatus::Denied);
        }
    }

    #[test]
    fn push_visit_dedupes_caps_and_orders_recent_first() {
        let mut v: Vec<PathBuf> = Vec::new();
        push_visit(&mut v, Path::new("/a"), 3);
        push_visit(&mut v, Path::new("/b"), 3);
        push_visit(&mut v, Path::new("/a"), 3); // revisit /a -> front, no dup
        assert_eq!(v, vec![PathBuf::from("/a"), PathBuf::from("/b")]);

        push_visit(&mut v, Path::new("/c"), 3);
        push_visit(&mut v, Path::new("/d"), 3); // cap 3 drops oldest
        assert_eq!(
            v,
            vec![
                PathBuf::from("/d"),
                PathBuf::from("/c"),
                PathBuf::from("/a"),
            ]
        );
    }

    #[test]
    fn recording_visits_prunes_stats_for_paths_off_the_list() {
        let mut v: Vec<PathBuf> = Vec::new();
        let mut stats = VisitStats::default();
        for name in ["/a", "/b", "/c", "/d"] {
            visit::record_visit_into(&mut v, &mut stats, Path::new(name), 3);
        }
        assert_eq!(v.len(), 3);
        assert_eq!(stats.uses.len(), 3);
        assert!(!stats.uses.contains_key(Path::new("/a")));
        assert_eq!(stats.uses[Path::new("/d")].count, 1);
    }

    #[test]
    fn recent_visit_policy_keeps_two_hundred_paths() {
        let mut v: Vec<PathBuf> = Vec::new();
        for i in 0..205 {
            push_visit(&mut v, Path::new(&format!("/recent/{i}")), VISITED_CAP);
        }

        assert_eq!(v.len(), 200);
        assert_eq!(v.first(), Some(&PathBuf::from("/recent/204")));
        assert_eq!(v.last(), Some(&PathBuf::from("/recent/5")));
    }

    #[test]
    fn recent_destinations_can_switch_between_frecency_and_chronology() {
        let a = PathBuf::from("/work/frequent");
        let b = PathBuf::from("/work/middle");
        let c = PathBuf::from("/work/latest");
        let paths = vec![c.clone(), b.clone(), a.clone()];
        let mut stats = VisitStats::default();
        stats.record(&a);
        stats.record(&a);
        stats.record(&a);
        stats.record(&b);
        stats.record(&c);

        let chronological = rank_visited(&paths, "", RecentOrder::Chronological, &stats);
        assert_eq!(chronological[0].path, c);

        let frecency = rank_visited(&paths, "", RecentOrder::Frecency, &stats);
        assert_eq!(frecency[0].path, a);
        assert_eq!(frecency[0].count, 3);
    }

    #[test]
    fn recent_destination_query_uses_fuzzy_path_matching() {
        let paths = vec![
            PathBuf::from("/Users/me/Documents"),
            PathBuf::from("/Users/me/Downloads"),
        ];
        let matches = rank_visited(
            &paths,
            "dwn",
            RecentOrder::Chronological,
            &VisitStats::default(),
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, PathBuf::from("/Users/me/Downloads"));
    }

    #[test]
    fn format_mode_renders_rwx() {
        assert_eq!(format_mode(0o755), "rwxr-xr-x");
        assert_eq!(format_mode(0o644), "rw-r--r--");
        assert_eq!(format_mode(0o600), "rw-------");
        assert_eq!(format_mode(0o000), "---------");
        assert_eq!(format_mode(0o777), "rwxrwxrwx");
        // Special bits, as `ls -l` shows them.
        assert_eq!(format_mode(0o1777), "rwxrwxrwt");
        assert_eq!(format_mode(0o4755), "rwsr-xr-x");
        assert_eq!(format_mode(0o2755), "rwxr-sr-x");
        assert_eq!(format_mode(0o7644), "rwSr-Sr-T");
    }

    #[test]
    fn make_info_describes_file_and_folder() {
        let tmp = TempDir::new();
        let f = tmp.file("notes.txt", "hello");
        let meta = std::fs::metadata(&f).unwrap();
        let fe = FileEntry::from_meta(f, &meta).unwrap();
        let card = make_info(&fe, None, None);
        assert_eq!(card.kind, "TXT file");
        assert_eq!(card.size, "5 B");
        assert!(card.children.is_none());
        assert_eq!(card.permissions.len(), 9);

        let d = tmp.dir("box");
        let meta = std::fs::metadata(&d).unwrap();
        let de = FileEntry::from_meta(d, &meta).unwrap();
        let card = make_info(&de, Some(4096), Some(3));
        assert_eq!(card.kind, "Folder");
        assert_eq!(card.size, "4.0 KB");
        assert_eq!(card.children, Some(3));
    }

    #[test]
    fn make_info_says_when_permissions_cannot_be_read() {
        let tmp = TempDir::new();
        let f = tmp.file("gone.txt", "hello");
        let meta = std::fs::metadata(&f).unwrap();
        let fe = FileEntry::from_meta(f.clone(), &meta).unwrap();
        std::fs::remove_file(&f).unwrap();
        let card = make_info(&fe, None, None);
        // Not a fake mode string indistinguishable from a real `000` file.
        assert!(
            card.permissions.starts_with("unavailable ("),
            "{}",
            card.permissions
        );
    }

    #[test]
    fn declined_image_preview_message_names_the_file_and_the_switch() {
        let mut img = entry("photo.jpg", false, 5_000);
        img.extension = "jpg".into();
        let message = image_preview_unavailable_message(&img);
        assert!(message.contains("photo.jpg"), "{message}");
        assert!(message.contains("Runtime controls"), "{message}");
    }

    #[test]
    fn facet_matches_kind_size_age() {
        use std::time::{Duration, UNIX_EPOCH};
        let now = UNIX_EPOCH + Duration::from_secs(100 * 24 * 60 * 60); // day 100
        let mut img = entry("photo.jpg", false, 5_000_000);
        img.extension = "jpg".into();
        img.modified = Some(UNIX_EPOCH + Duration::from_secs(99 * 24 * 60 * 60)); // 1 day old
        let mut code = entry("main.rs", false, 100);
        code.extension = "rs".into();
        code.modified = Some(UNIX_EPOCH + Duration::from_secs(50 * 24 * 60 * 60)); // 50 days old
        let dir = entry("box", true, 0);

        let images = FacetSet {
            kind: Some(KindFacet::Images),
            ..Default::default()
        };
        assert!(facet_matches(&img, &images, now));
        assert!(!facet_matches(&code, &images, now));

        let folders = FacetSet {
            kind: Some(KindFacet::Folders),
            ..Default::default()
        };
        assert!(facet_matches(&dir, &folders, now));
        assert!(!facet_matches(&img, &folders, now));

        let big = FacetSet {
            min_size: Some(1_000_000),
            ..Default::default()
        };
        assert!(facet_matches(&img, &big, now)); // 5MB
        assert!(!facet_matches(&code, &big, now)); // 100B
        assert!(facet_matches(&dir, &big, now)); // folders ignore size

        let recent = FacetSet {
            max_age_days: Some(7),
            ..Default::default()
        };
        assert!(facet_matches(&img, &recent, now)); // 1 day
        assert!(!facet_matches(&code, &recent, now)); // 50 days

        let stale = FacetSet {
            min_age_days: Some(30),
            ..Default::default()
        };
        assert!(!facet_matches(&img, &stale, now)); // 1 day: too fresh
        assert!(facet_matches(&code, &stale, now)); // 50 days: old enough
        assert!(!facet_matches(&dir, &stale, now)); // unknown mtime: fails

        // Empty facets pass everything.
        assert!(facet_matches(&code, &FacetSet::default(), now));
    }

    #[test]
    fn folder_overview_reports_largest_and_oldest() {
        use std::time::{Duration, UNIX_EPOCH};
        let mut a = entry("a.txt", false, 100);
        a.modified = Some(UNIX_EPOCH + Duration::from_secs(300));
        let mut big = entry("big.bin", false, 900);
        big.modified = Some(UNIX_EPOCH + Duration::from_secs(200));
        let mut old = entry("old.log", false, 50);
        old.modified = Some(UNIX_EPOCH + Duration::from_secs(100));
        let p = panel_with(vec![a, big, old]);
        let o = p.folder_overview();
        assert_eq!(o.total, Some(1050));
        assert_eq!(o.largest, Some(("big.bin".to_string(), 900)));
        assert_eq!(
            o.oldest,
            Some(("old.log".to_string(), UNIX_EPOCH + Duration::from_secs(100)))
        );
    }

    #[test]
    fn facets_filter_the_listing() {
        let mut p = panel_with(vec![
            entry("a.jpg", false, 1),
            entry("b.rs", false, 1),
            entry("c.jpg", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.set_facets(FacetSet {
            kind: Some(KindFacet::Images),
            ..Default::default()
        });
        let names: Vec<&str> = p
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["a.jpg", "c.jpg"]);
    }

    #[test]
    fn glob_match_handles_star_and_question() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("img_*.jpg", "img_2024.jpg"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(!glob_match("*.rs", "main.txt"));
        assert!(glob_match("*", "anything"));
        // Multi-byte names: `?` is one character, and a failed match after a
        // star re-anchors on a character boundary.
        assert!(glob_match("caf?.txt", "café.txt"));
        assert!(glob_match("*é*", "résumé.pdf"));
        assert!(glob_match("*ü.txt", "üüü.txt"));
        assert!(!glob_match("?", "éé"));
        assert!(glob_match("**a", "a"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "a"));
    }

    #[test]
    fn parse_mask_splits_and_marks_subtraction() {
        let terms = parse_mask("*.JPG, !*raw*, -tmp");
        assert_eq!(
            terms,
            vec![
                ("*.jpg".to_string(), false),
                ("*raw*".to_string(), true),
                ("tmp".to_string(), true),
            ]
        );
        // A bare subtraction marker is not an (empty) term.
        assert!(parse_mask("-").is_empty());
        assert_eq!(parse_mask("*.jpg, !"), vec![("*.jpg".to_string(), false)]);
    }

    #[test]
    fn dotted_mask_terms_match_name_suffixes() {
        // A leading dot or a multi-part extension still selects by suffix.
        assert!(term_matches(".jpg", "photo.jpg", "jpg"));
        assert!(term_matches("tar.gz", "backup.tar.gz", "gz"));
        assert!(term_matches(".tar.gz", "backup.tar.gz", "gz"));
        assert!(!term_matches("tar.gz", "backup.gz", "gz"));
        assert!(!term_matches("tar.gz", "backuptar.gz", "gz"));
        // Whole names still match exactly, and wildcards stay pure globs.
        assert!(term_matches(".gitignore", ".gitignore", ""));
        assert!(term_matches("readme.md", "readme.md", "md"));
        assert!(!term_matches("*.tar", "backup.tar.gz", "gz"));
        assert!(!term_matches(".", "a.", ""));
    }

    #[test]
    fn select_by_mask_adds_then_subtracts() {
        let mut p = panel_with(vec![
            entry("a.jpg", false, 1),
            entry("b.jpg", false, 1),
            entry("c.png", false, 1),
            entry("raw.jpg", false, 1),
        ]);
        // bare ext for files needs the extension field populated:
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.sort_entries();

        // Add all jpgs, then subtract anything containing "raw".
        // raw.jpg matches the add but the subtract wins, so it is not added.
        let added = p.select_by_mask("*.jpg, !*raw*");
        assert_eq!(added, 2);
        let names: Vec<String> = p
            .selected_paths()
            .iter()
            .filter_map(|pth| pth.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(&"a.jpg".to_string()));
        assert!(names.contains(&"b.jpg".to_string()));
        assert!(!names.contains(&"raw.jpg".to_string()));
        assert!(!names.contains(&"c.png".to_string()));
        assert_eq!(p.selected_count(), 2);
    }

    #[test]
    fn mask_match_count_reports_selection_changes() {
        let mut p = panel_with(vec![
            entry("a.jpg", false, 1),
            entry("raw.jpg", false, 1),
            entry("note.txt", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        let raw = p
            .entries()
            .iter()
            .find(|entry| entry.name == "raw.jpg")
            .unwrap()
            .path
            .clone();

        assert_eq!(p.mask_match_count("!*raw*"), 0);
        p.select_path(raw);
        assert_eq!(p.mask_match_count("!*raw*"), 1);
        assert_eq!(p.mask_match_count("*.jpg, !*raw*"), 2);
    }

    #[test]
    fn bare_extension_term_matches_by_extension() {
        let mut p = panel_with(vec![
            entry("doc.pdf", false, 1),
            entry("note.txt", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        assert_eq!(p.mask_match_count("pdf"), 1);
        p.select_by_mask("pdf");
        assert_eq!(p.selected_count(), 1);
    }

    #[test]
    fn type_ahead_jumps_by_prefix_then_substring() {
        let mut p = panel_with(vec![
            entry("apple.txt", false, 1),
            entry("banana.txt", false, 1),
            entry("cherry-banana.txt", false, 1),
        ]);
        p.sort_entries();

        assert!(p.type_ahead("ban"));
        assert_eq!(p.filtered_get(p.cursor() - 1).unwrap().name, "banana.txt");

        // No prefix hit -> substring fallback finds "cherry-banana".
        assert!(p.type_ahead("cherry"));
        assert_eq!(
            p.filtered_get(p.cursor() - 1).unwrap().name,
            "cherry-banana.txt"
        );

        assert!(!p.type_ahead("zzz"));
    }

    #[test]
    fn select_cursor_adds_current_row() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.set_cursor(2); // second file
        p.select_cursor();
        assert!(p.is_selected(&p.entries()[1].path));
        assert_eq!(p.selected_count(), 1);
    }

    #[test]
    fn filter_cache_tracks_query_and_entry_changes() {
        let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
        assert_eq!(p.filtered_count(), 2);

        // Query change invalidates the cache.
        p.set_search_query("al");
        assert_eq!(p.filtered_count(), 1);
        assert_eq!(p.filtered_get(0).unwrap().name, "alpha");

        // Entry change (generation bump via sort) invalidates it too.
        p.push_entry_for_test(entry("alps", false, 1));
        p.sort_entries();
        assert_eq!(p.filtered_count(), 2);
        let names: Vec<&str> = p
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "alps"]);
    }

    #[test]
    fn filtered_indices_point_into_entries() {
        let mut p = panel_with(vec![
            entry("keep.txt", false, 1),
            entry("skip.rs", false, 1),
            entry("keeper.txt", false, 1),
        ]);
        p.set_search_query("keep");
        let idx = p.filtered_indices();
        assert_eq!(idx.len(), 2);
        for &i in idx.iter() {
            assert!(p.entries()[i].name.contains("keep"));
        }
    }

    #[test]
    fn size_cache_invalidation_cancels_epoch_and_removes_ancestors_only() {
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        let other = tmp.dir("other");
        let a_key = VolumePathKey::observe(&a);
        let other_key = VolumePathKey::observe(&other);
        let active_epoch = capture_path_epoch(&a_key);
        let now = SystemTime::now();
        {
            let _boundary = lock_recover(scan_commit_boundary());
            let mut cache = lock_recover(dir_size_cache());
            cache.insert(a_key.clone(), (now, 100));
            cache.insert(other_key.clone(), (now, 5));
        }

        invalidate_size_cache(&a.join("b/c/file.txt"));

        assert!(active_epoch.is_cancelled());
        let cache = lock_recover(dir_size_cache());
        assert!(!cache.contains_key(&a_key));
        assert_eq!(cache[&other_key].0, now, "unrelated dirs stay valid");
    }

    #[test]
    fn batched_invalidation_matches_per_path_invalidation() {
        let tmp = TempDir::new();
        let keys: Vec<VolumePathKey> = ["x", "x/deep", "x/deep/below", "y", "y/sub", "z"]
            .into_iter()
            .map(|name| VolumePathKey::observe(&tmp.dir(name)))
            .collect();
        let now = SystemTime::now();
        {
            let _boundary = lock_recover(scan_commit_boundary());
            let mut cache = lock_recover(dir_size_cache());
            for key in &keys {
                cache.insert(key.clone(), (now, 1));
            }
        }

        let changed_x = tmp.path().join("x/deep/file.txt");
        let changed_y = tmp.path().join("y");
        invalidate_size_cache_many([changed_x.as_path(), changed_y.as_path()]);

        let cache = lock_recover(dir_size_cache());
        let kept: Vec<bool> = keys.iter().map(|key| cache.contains_key(key)).collect();
        // Ancestors of each change go; a folder below a change and an
        // unrelated sibling stay.
        assert_eq!(kept, [false, false, true, false, true, true]);
    }

    #[test]
    fn directory_cache_key_rejects_an_old_mount_generation() {
        let path = PathBuf::from("/fixture/folder");
        let first = VolumePathKey {
            path: path.clone(),
            volume_id: 7,
            generation: 1,
        };
        let remounted = VolumePathKey {
            path,
            volume_id: 7,
            generation: 2,
        };
        assert_ne!(first, remounted);
    }

    #[test]
    fn partial_sizes_are_reused_in_memory_but_never_cached_as_exact() {
        let tmp = TempDir::new();
        let directory = tmp.dir("partial");
        let cache_key = VolumePathKey::observe(&directory);
        let path_epoch = capture_path_epoch(&cache_key);
        let current = Arc::new(Mutex::new(Arc::new(ScanEpoch::new())));
        let sizes = Arc::new(Mutex::new(HashMap::new()));
        let counts = Arc::new(Mutex::new(HashMap::new()));
        let revision = Arc::new(AtomicU64::new(0));
        let scan_epoch = begin_panel_scan(
            &current,
            &sizes,
            &counts,
            &HashSet::from([directory.clone()]),
            &revision,
        );
        let mtime = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        let measurement = DirMeasurement {
            path: directory.clone(),
            modified: Some(mtime),
            complete: false,
            cache_key: cache_key.clone(),
            path_epoch,
            size: 7,
            completed_at: std::time::Instant::now(),
            elapsed: std::time::Duration::from_millis(1),
        };
        let outcome = publish_dir_measurements_if_current(
            &current,
            &scan_epoch,
            &sizes,
            &[measurement],
            &revision,
        );
        assert_eq!(outcome.published, 1);
        assert!(!outcome.cache_changed, "a partial size is not persisted");
        assert!(!lock_recover(dir_size_cache()).contains_key(&cache_key));

        // A later recompute with the same folder mtime reuses it: no re-walk.
        lock_recover(&sizes).clear();
        assert!(publish_cached_size_if_current(
            &current,
            &scan_epoch,
            &sizes,
            &directory,
            &cache_key,
            Some(mtime),
            &revision,
        ));
        assert_eq!(lock_recover(&sizes).get(&directory), Some(&7));

        // A change inside the folder drops it like the exact cache.
        invalidate_size_cache(&directory.join("file.bin"));
        assert!(!publish_cached_size_if_current(
            &current,
            &scan_epoch,
            &sizes,
            &directory,
            &cache_key,
            Some(mtime),
            &revision,
        ));
    }

    #[test]
    fn watcher_invalidation_wins_against_worker_paused_before_commit() {
        let tmp = TempDir::new();
        let directory = tmp.dir("subject");
        let changed = directory.join("nested/file.bin");
        let cache_key = VolumePathKey::observe(&directory);
        let path_epoch = capture_path_epoch(&cache_key);
        let current = Arc::new(Mutex::new(Arc::new(ScanEpoch::new())));
        let sizes = Arc::new(Mutex::new(HashMap::new()));
        let counts = Arc::new(Mutex::new(HashMap::new()));
        let revision = Arc::new(AtomicU64::new(0));
        let scan_epoch = begin_panel_scan(
            &current,
            &sizes,
            &counts,
            &HashSet::from([directory.clone()]),
            &revision,
        );

        {
            let _boundary = lock_recover(scan_commit_boundary());
            lock_recover(dir_size_cache()).remove(&cache_key);
            lock_recover(walk_log()).remove(&cache_key);
        }

        let barrier = Arc::new(std::sync::Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);
        let worker_current = Arc::clone(&current);
        let worker_sizes = Arc::clone(&sizes);
        let worker_revision = Arc::clone(&revision);
        let worker_scan_epoch = Arc::clone(&scan_epoch);
        let worker_path_epoch = Arc::clone(&path_epoch);
        let worker_key = cache_key.clone();
        let worker_path = directory.clone();
        let worker = std::thread::spawn(move || {
            let measurement = DirMeasurement {
                path: worker_path,
                modified: Some(SystemTime::now()),
                complete: true,
                cache_key: worker_key,
                path_epoch: worker_path_epoch,
                size: 42,
                completed_at: std::time::Instant::now(),
                elapsed: std::time::Duration::from_millis(1),
            };
            worker_barrier.wait();
            worker_barrier.wait();
            publish_dir_measurements_if_current(
                &worker_current,
                &worker_scan_epoch,
                &worker_sizes,
                &[measurement],
                &worker_revision,
            )
        });

        barrier.wait();
        invalidate_size_cache(&changed);
        assert!(path_epoch.is_cancelled());
        assert!(scan_is_current(&current, &scan_epoch));
        barrier.wait();

        assert_eq!(worker.join().unwrap(), PublishOutcome::default());
        assert!(!lock_recover(&sizes).contains_key(&directory));
        let _boundary = lock_recover(scan_commit_boundary());
        assert!(!lock_recover(dir_size_cache()).contains_key(&cache_key));
        assert!(!lock_recover(walk_log()).contains_key(&cache_key));
    }

    #[test]
    fn recursive_size_walk_stops_between_entries_when_cancelled() {
        let tmp = TempDir::new();
        tmp.file("a.bin", "123");
        tmp.file("b.bin", "456");
        tmp.file("c.bin", "789");
        let checks = std::sync::atomic::AtomicUsize::new(0);

        let result = dir_size_recursive_until(tmp.path(), false, || {
            checks.fetch_add(1, AtomicOrdering::SeqCst) >= 2
        });

        assert_eq!(result, None);
        assert_eq!(checks.load(AtomicOrdering::SeqCst), 3);
    }

    #[cfg(unix)]
    #[test]
    fn recursive_size_counts_file_symlinks_not_their_targets() {
        let outside = TempDir::new();
        let target = outside.file("big.bin", &"x".repeat(4000));
        let tmp = TempDir::new();
        tmp.file("small.bin", "12345");
        std::os::unix::fs::symlink(&target, tmp.path().join("link.bin")).unwrap();
        let link_len = std::fs::symlink_metadata(tmp.path().join("link.bin"))
            .unwrap()
            .len();

        let expected = Some(5 + link_len);
        assert_eq!(
            dir_size_recursive_until(tmp.path(), false, || false),
            expected
        );
        // A background walk measures the same tree.
        assert_eq!(
            dir_size_recursive_until(tmp.path(), true, || false),
            expected
        );
    }

    #[cfg(unix)]
    #[test]
    fn recursive_size_counts_hard_linked_files_once() {
        let tmp = TempDir::new();
        let data = tmp.file("data.bin", &"x".repeat(1000));
        tmp.dir("sub");
        std::fs::hard_link(&data, tmp.path().join("link.bin")).unwrap();
        std::fs::hard_link(&data, tmp.path().join("sub/link.bin")).unwrap();
        tmp.file("other.bin", "12345");

        assert_eq!(
            dir_size_recursive_until(tmp.path(), false, || false),
            Some(1005)
        );
    }

    #[test]
    fn size_walk_reports_whether_every_part_was_read() {
        let tmp = TempDir::new();
        tmp.dir("sub");
        tmp.file("sub/a.bin", "1234");
        assert_eq!(
            dir_size_walk_until(tmp.path(), false, || false),
            Some(DirWalk {
                size: 4,
                complete: true
            })
        );
        // A vanished child is gone, an unreadable one is a gap in the total.
        let gone = std::io::Error::from(std::io::ErrorKind::NotFound);
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(!walk_error_leaves_gap(&gone));
        assert!(walk_error_leaves_gap(&denied));
    }

    #[cfg(unix)]
    #[test]
    fn size_walk_with_unreadable_subfolder_is_incomplete() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new();
        tmp.file("a.bin", "12");
        let locked = tmp.dir("locked");
        tmp.file("locked/b.bin", "345");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&locked).is_ok(); // root ignores modes
        let walk = dir_size_walk_until(tmp.path(), false, || false);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            return;
        }
        assert_eq!(
            walk,
            Some(DirWalk {
                size: 2,
                complete: false
            })
        );
    }

    #[test]
    fn cache_persistence_failures_warn_once() {
        let warned = AtomicBool::new(false);
        assert!(first_cache_warning(&warned));
        assert!(!first_cache_warning(&warned));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recursive_size_does_not_walk_into_another_filesystem() {
        use std::os::unix::fs::MetadataExt;
        let proc_dev = std::fs::metadata("/proc").map(|m| m.dev());
        let root_dev = std::fs::metadata("/").map(|m| m.dev());
        if !matches!((proc_dev, root_dev), (Ok(p), Ok(r)) if p != r) {
            return; // no separate procfs mount to test against
        }
        // `/proc` listed in `/` is a mount point: sizing it must not walk
        // procfs (whose `kcore` alone reports ~128 TB).
        assert_eq!(
            dir_size_recursive_until(Path::new("/proc"), false, || false),
            Some(0)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recursive_size_measures_a_local_mount_point_root() {
        use std::os::unix::fs::MetadataExt;
        let shm = Path::new("/dev/shm");
        let (Ok(shm_meta), Ok(dev_meta)) = (std::fs::metadata(shm), std::fs::metadata("/dev"))
        else {
            return; // no /dev/shm here
        };
        if shm_meta.dev() == dev_meta.dev() {
            return; // /dev/shm is not its own mount
        }
        let file = shm.join(format!("commander-size-test-{}", std::process::id()));
        if std::fs::write(&file, vec![0u8; 4096]).is_err() {
            return; // not writable in this sandbox
        }
        // `/dev/shm` is a tmpfs mount on another device than `/dev`: a local
        // mount-point row is measured in full, not reported as 0 B.
        let size = dir_size_recursive_until(shm, false, || false);
        let _ = std::fs::remove_file(&file);
        assert!(size.is_some_and(|size| size >= 4096), "got {size:?}");
    }

    #[test]
    fn shuffled_duplicate_batch_deduplicates_before_top_k() {
        let key = |name: &str| VolumePathKey {
            path: PathBuf::from(format!("/batch/{name}")),
            volume_id: 1,
            generation: 1,
        };
        let a = key("a");
        let b = key("b");
        let started = std::time::Instant::now();
        let completed = |seconds| started + std::time::Duration::from_secs(seconds);
        let modified = |seconds| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        let entries = [
            (
                a.clone(),
                RankedDirSize {
                    modified: modified(1),
                    completed_at: Some(completed(100)),
                    size: 100,
                },
            ),
            (
                a.clone(),
                RankedDirSize {
                    modified: modified(200),
                    completed_at: Some(completed(99)),
                    size: 99,
                },
            ),
            (
                b.clone(),
                RankedDirSize {
                    modified: modified(98),
                    completed_at: Some(completed(98)),
                    size: 98,
                },
            ),
        ];
        let permutations = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];

        for order in permutations {
            let mut ordered = order.into_iter().map(|index| entries[index].clone());
            let mut cache = HashMap::new();
            let (first_key, first_value) = ordered.next().unwrap();
            cache.insert(first_key, first_value);

            extend_bounded_by(&mut cache, ordered, 2, 2, compare_ranked_dir_sizes);

            assert_eq!(cache.len(), 2, "order {order:?}");
            assert!(cache.capacity() <= hash_map_capacity_upper_bound(2));
            assert_eq!(
                cache[&a].completed_at,
                Some(completed(100)),
                "order {order:?}"
            );
            assert_eq!(cache[&a].size, 100, "order {order:?}");
            assert_eq!(
                cache[&b].completed_at,
                Some(completed(98)),
                "order {order:?}"
            );
        }
    }

    #[test]
    fn advisory_lock_state_is_recovered_after_poisoning() {
        let state = Mutex::new(1usize);
        let panic = std::panic::catch_unwind(|| {
            let mut state = state.lock().unwrap();
            *state = 2;
            panic!("poison advisory state");
        });

        assert!(panic.is_err());
        assert!(state.is_poisoned());
        assert_eq!(*lock_recover(&state), 2);
    }

    #[test]
    fn dir_size_cache_pruning_is_bounded_and_deterministic() {
        let key = |name: &str| VolumePathKey {
            path: PathBuf::from(format!("/cache/{name}")),
            volume_id: 1,
            generation: 1,
        };
        let oldest = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let mut cache = HashMap::from([
            (key("a"), (oldest, 1)),
            (key("b"), (oldest, 2)),
            (key("c"), (oldest + std::time::Duration::from_secs(1), 3)),
            (key("d"), (oldest + std::time::Duration::from_secs(2), 4)),
            (key("e"), (oldest + std::time::Duration::from_secs(3), 5)),
        ]);

        prune_dir_size_cache_to(&mut cache, 4, 3);

        assert_eq!(cache.len(), 3);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(4));
        assert_eq!(
            cache
                .keys()
                .map(|key| key.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([key("c").path, key("d").path, key("e").path])
        );
    }

    #[test]
    fn walk_log_pruning_is_bounded_and_deterministic() {
        let key = |name: &str| VolumePathKey {
            path: PathBuf::from(format!("/walk/{name}")),
            volume_id: 1,
            generation: 1,
        };
        let oldest = std::time::Instant::now();
        let cost = std::time::Duration::from_millis(1);
        let mut log = HashMap::from([
            (key("a"), (oldest, cost)),
            (key("b"), (oldest, cost)),
            (key("c"), (oldest + std::time::Duration::from_secs(1), cost)),
            (key("d"), (oldest + std::time::Duration::from_secs(2), cost)),
            (key("e"), (oldest + std::time::Duration::from_secs(3), cost)),
        ]);

        prune_walk_log_to(&mut log, 4, 3);

        assert_eq!(log.len(), 3);
        assert!(log.capacity() <= hash_map_capacity_upper_bound(4));
        assert_eq!(
            log.keys()
                .map(|key| key.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([key("c").path, key("d").path, key("e").path])
        );
    }

    #[test]
    fn oversized_batch_is_selected_before_extend_and_shrinks_capacity() {
        let key = |prefix: &str, index: usize| VolumePathKey {
            path: PathBuf::from(format!("/{prefix}/{index:03}")),
            volume_id: 1,
            generation: 1,
        };
        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let mut cache = HashMap::with_capacity(1_024);
        cache.extend((0..4).map(|index| (key("old", index), (base, index as u64))));
        let incoming: Vec<_> = (0..100)
            .map(|index| {
                (
                    key("incoming", index),
                    (
                        base + std::time::Duration::from_secs(index as u64 + 1),
                        index as u64,
                    ),
                )
            })
            .collect();

        extend_bounded_by(&mut cache, incoming, 8, 6, compare_cache_entries);

        assert_eq!(cache.len(), 6);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(8));
        assert_eq!(
            cache
                .keys()
                .map(|key| key.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            (94..100).map(|index| key("incoming", index).path).collect()
        );
    }

    #[test]
    fn corrupt_persisted_mtime_is_dropped_without_panicking() {
        let entry = |path: &str, mtime_secs: u64, mtime_nanos: u32| PersistedCacheEntry {
            key: VolumePathKey {
                path: PathBuf::from(path),
                volume_id: 1,
                generation: 1,
            },
            mtime_secs,
            mtime_nanos,
            size: 7,
        };
        let persisted = PersistedCache {
            schema: 1,
            entries: vec![
                // Carrying the nanos into the seconds would overflow them.
                entry("/corrupt/carry", u64::MAX, 1_000_000_000),
                entry("/corrupt/nanos", 5, u32::MAX),
                entry("/valid", 5, 999_999_999),
            ],
        };
        let json = serde_json::to_vec(&persisted).unwrap();

        let cache = load_cache_from_reader_with_bounds(json.as_slice(), 8, 6).unwrap();

        assert_eq!(
            cache.keys().map(|key| key.path.clone()).collect::<Vec<_>>(),
            vec![PathBuf::from("/valid")]
        );
    }

    #[test]
    fn oversized_persisted_load_is_streamed_and_capacity_bounded() {
        let persisted = PersistedCache {
            schema: 1,
            entries: (0..50)
                .map(|index| PersistedCacheEntry {
                    key: VolumePathKey {
                        path: PathBuf::from(format!("/load/{index:03}")),
                        volume_id: 1,
                        generation: 1,
                    },
                    mtime_secs: index as u64 + 1,
                    mtime_nanos: 0,
                    size: index as u64,
                })
                .collect(),
        };
        let json = serde_json::to_vec(&persisted).unwrap();

        let cache = load_cache_from_reader_with_bounds(json.as_slice(), 8, 6).unwrap();

        assert_eq!(cache.len(), 6);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(8));
        assert_eq!(
            cache
                .keys()
                .map(|key| key.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            (44..50)
                .map(|index| PathBuf::from(format!("/load/{index:03}")))
                .collect()
        );
    }

    #[test]
    fn shuffled_persisted_duplicates_are_deduplicated_before_top_k() {
        let key = |name: &str| VolumePathKey {
            path: PathBuf::from(format!("/load/{name}")),
            volume_id: 1,
            generation: 1,
        };
        let a = key("a");
        let b = key("b");
        let entries = [
            PersistedCacheEntry {
                key: a.clone(),
                mtime_secs: 100,
                mtime_nanos: 0,
                size: 100,
            },
            PersistedCacheEntry {
                key: a.clone(),
                mtime_secs: 99,
                mtime_nanos: 0,
                size: 99,
            },
            PersistedCacheEntry {
                key: b.clone(),
                mtime_secs: 98,
                mtime_nanos: 0,
                size: 98,
            },
        ];
        let permutations = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];

        for order in permutations {
            let persisted = PersistedCache {
                schema: 1,
                entries: order
                    .into_iter()
                    .map(|index| PersistedCacheEntry {
                        key: entries[index].key.clone(),
                        mtime_secs: entries[index].mtime_secs,
                        mtime_nanos: entries[index].mtime_nanos,
                        size: entries[index].size,
                    })
                    .collect(),
            };
            let json = serde_json::to_vec(&persisted).unwrap();

            let cache = load_cache_from_reader_with_bounds(json.as_slice(), 2, 2).unwrap();

            assert_eq!(cache.len(), 2, "order {order:?}");
            assert!(cache.capacity() <= hash_map_capacity_upper_bound(2));
            assert_eq!(
                cache[&a],
                (
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(100),
                    100,
                ),
                "order {order:?}"
            );
            assert_eq!(
                cache[&b],
                (
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(98),
                    98,
                ),
                "order {order:?}"
            );
        }
    }

    #[test]
    fn batch_between_retain_and_limit_is_not_pruned() {
        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let incoming = (0..9_500).map(|index| {
            (
                VolumePathKey {
                    path: PathBuf::from(format!("/batch-threshold/{index:05}")),
                    volume_id: 1,
                    generation: 1,
                },
                (
                    base + std::time::Duration::from_secs(index as u64),
                    index as u64,
                ),
            )
        });
        let mut cache = HashMap::new();

        extend_bounded_by(
            &mut cache,
            incoming,
            DIR_SIZE_CACHE_LIMIT,
            DIR_SIZE_CACHE_RETAIN,
            compare_cache_entries,
        );

        assert_eq!(cache.len(), 9_500);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(DIR_SIZE_CACHE_LIMIT));
    }

    #[test]
    fn persisted_load_between_retain_and_limit_is_not_pruned() {
        let persisted = PersistedCache {
            schema: 1,
            entries: (0..9_500)
                .map(|index| PersistedCacheEntry {
                    key: VolumePathKey {
                        path: PathBuf::from(format!("/load-threshold/{index:05}")),
                        volume_id: 1,
                        generation: 1,
                    },
                    mtime_secs: index as u64 + 1,
                    mtime_nanos: 0,
                    size: index as u64,
                })
                .collect(),
        };
        let json = serde_json::to_vec(&persisted).unwrap();

        let cache = load_cache_from_reader_with_bounds(
            json.as_slice(),
            DIR_SIZE_CACHE_LIMIT,
            DIR_SIZE_CACHE_RETAIN,
        )
        .unwrap();

        assert_eq!(cache.len(), 9_500);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(DIR_SIZE_CACHE_LIMIT));
    }

    #[test]
    fn deep_change_recomputes_dir_size_without_reloading_listing() {
        fn wait_for_size(p: &PanelState, dir: &Path, expected: u64) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if p.size_snapshot().size_of(dir) == Some(expected) {
                    return;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "size {expected} for {dir:?} not observed"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        let tmp = TempDir::new();
        tmp.dir("sub/deep");
        tmp.file("sub/deep/a.bin", "12345"); // 5 bytes
        let sub = tmp.path().join("sub");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        wait_for_size(&p, &sub, 5);

        // A deep change: sub's own mtime does not move, so the mtime
        // cache alone would keep serving the stale 5 bytes.
        let new_file = tmp.file("sub/deep/b.bin", "1234567"); // +7 bytes

        // What the recursive watcher does on such an event:
        invalidate_size_cache(&new_file);
        p.sizes.mark_dirty_immediately_for_test();
        reset_walk_log(); // bypass the walk cooldown in the test

        let reloaded = p.poll_fs_changes();
        assert!(!reloaded, "sizes-only events must not reload the listing");
        wait_for_size(&p, &sub, 12);
    }

    #[test]
    fn mutation_during_subscription_snapshot_handoff_forces_reconciliation() {
        let tmp = TempDir::new();
        tmp.file("before.txt", "before");
        let mut panel = PanelState::new(tmp.path().to_path_buf());
        panel.refresh();

        panel.watcher.activate_test_binding(tmp.path());
        let subscription_ticket = panel.watcher.snapshot_ticket();

        // Deterministic handoff barrier: the subscription is active, but its
        // first complete listing has not yet been acknowledged.
        let created = tmp.file("during.txt", "during");
        panel
            .watcher
            .inject_current_change_for_test(created.clone());
        assert!(panel.reload_entries());
        let listing_binding = panel.listing.binding().to_path_buf();
        panel
            .watcher
            .acknowledge_snapshot(subscription_ticket.clone(), &listing_binding);

        assert!(panel.poll_fs_changes());
        assert!(panel.entries().iter().any(|entry| entry.path == created));
        assert!(!panel.watcher.has_pending_reconciliation_for_test());
    }

    #[test]
    fn watcher_gap_replaces_the_listing_and_applies_its_generation() {
        let health_before = crate::watcher_health::snapshot();
        let tmp = TempDir::new();
        tmp.file("before.txt", "before");
        let mut panel = PanelState::new(tmp.path().to_path_buf());
        panel.refresh();
        assert!(
            panel
                .entries()
                .iter()
                .any(|entry| entry.name == "before.txt")
        );

        std::fs::remove_file(tmp.path().join("before.txt")).unwrap();
        tmp.file("after.txt", "after");
        panel.watcher.activate_test_binding(tmp.path());
        panel.watcher.inject_gap_for_test();

        assert!(panel.poll_fs_changes());
        assert!(
            panel
                .entries()
                .iter()
                .any(|entry| entry.name == "after.txt")
        );
        assert!(
            !panel
                .entries()
                .iter()
                .any(|entry| entry.name == "before.txt")
        );
        assert!(!panel.watcher.has_pending_reconciliation_for_test());
        let health_after = crate::watcher_health::snapshot();
        assert!(health_after.listing_reconciliations > health_before.listing_reconciliations);
        assert!(health_after.gap_reconciliations > health_before.gap_reconciliations);
    }

    /// Profiling harness, not a test: drives a real watcher + poll loop
    /// at ~60 "fps" against `COMMANDER_PROFILE_ROOT` for
    /// `COMMANDER_PROFILE_SECS` seconds while an external driver generates
    /// fs activity and samples this process's CPU.
    /// Run: cargo test --release `watcher_profile_harness` -- --ignored --nocapture
    #[test]
    #[ignore = "profiling harness, run manually"]
    fn watcher_profile_harness() {
        let root = std::env::var("COMMANDER_PROFILE_ROOT").expect("set COMMANDER_PROFILE_ROOT");
        let secs: u64 = std::env::var("COMMANDER_PROFILE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);

        let mut p = PanelState::new(PathBuf::from(root));
        p.refresh();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        let mut reloads = 0u32;
        while std::time::Instant::now() < deadline {
            if p.poll_fs_changes() {
                reloads += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        println!("harness done: {} listing reloads", reloads);
    }

    #[test]
    fn make_preview_marks_text_pending_without_reading_and_skips_dirs() {
        let tmp = TempDir::new();
        let file = tmp.file("note.txt", "hello");
        let meta = std::fs::metadata(&file).unwrap();
        let fe = FileEntry::from_meta(file.clone(), &meta).unwrap();
        std::fs::remove_file(file).unwrap();
        match make_preview(&fe) {
            Some(PreviewContent::Pending(identity)) => assert_eq!(identity.path, fe.path),
            other => panic!("expected pending preview, got {other:?}"),
        }

        let dir = tmp.dir("d");
        let meta = std::fs::metadata(&dir).unwrap();
        let de = FileEntry::from_meta(dir, &meta).unwrap();
        assert!(make_preview(&de).is_none());
    }

    #[test]
    fn visible_window_clamps_anchor_and_has_a_cold_start_default() {
        assert_eq!(visible_window(100, 40, 10), 40..72);
        assert_eq!(visible_window(8, 99, 20), 0..8);
        assert_eq!(visible_window(20, 0, 0), 0..20);
    }

    /// `entry` rows whose extension is filled in from the name, as a real
    /// listing does.
    fn with_extensions(mut entries: Vec<FileEntry>) -> Vec<FileEntry> {
        for e in &mut entries {
            e.extension = Path::new(&e.name)
                .extension()
                .map(|ext| ext.to_string_lossy().to_lowercase())
                .unwrap_or_default();
        }
        entries
    }

    #[test]
    fn subtracting_mask_deselects_an_already_selected_entry() {
        let mut p = panel_with(with_extensions(vec![
            entry("a.jpg", false, 1),
            entry("raw.jpg", false, 1),
        ]));
        p.select_path(PathBuf::from("/test/raw.jpg"));
        p.select_path(PathBuf::from("/test/a.jpg"));

        // Nothing was added, but the selected match is removed.
        assert_eq!(p.select_by_mask("!*raw*"), 0);
        assert!(!p.is_selected(Path::new("/test/raw.jpg")));
        assert!(p.is_selected(Path::new("/test/a.jpg")));
    }

    #[test]
    fn parse_mask_trims_space_after_the_subtract_marker() {
        assert_eq!(
            parse_mask("! *.TMP, -  Raw"),
            vec![("*.tmp".to_string(), true), ("raw".to_string(), true)]
        );
    }

    #[test]
    fn select_largest_takes_every_file_when_n_exceeds_the_count() {
        let mut p = panel_with(vec![
            entry("a", false, 10),
            entry("b", false, 20),
            entry("c", false, 30),
            entry("dir", true, 0),
        ]);
        assert_eq!(p.select_largest(10), 3);
        assert_eq!(selected_names(&p), ["a", "b", "c"].map(String::from).into());
    }

    #[test]
    fn select_largest_breaks_size_ties_by_path_whatever_the_order() {
        for names in [
            ["x", "y", "small"],
            ["y", "x", "small"],
            ["small", "y", "x"],
        ] {
            let size = |name: &str| if name == "small" { 1 } else { 50 };
            let mut p = panel_with(names.map(|n| entry(n, false, size(n))).into());
            assert_eq!(p.select_largest(1), 1);
            // The bounded heap evicts the smaller (size, path) first.
            assert_eq!(selected_names(&p), ["y".to_string()].into(), "{names:?}");
        }
    }

    #[test]
    fn select_largest_ignores_files_hidden_by_the_filter() {
        let mut p = panel_with(vec![
            entry("huge.iso", false, 9_000),
            entry("notes-big.txt", false, 500),
            entry("notes-small.txt", false, 5),
        ]);
        p.set_search_query("notes");
        assert_eq!(p.select_largest(1), 1);
        assert_eq!(selected_names(&p), ["notes-big.txt".to_string()].into());
    }

    #[test]
    fn select_same_extension_does_nothing_off_a_file_with_an_extension() {
        let mut p = panel_with(with_extensions(vec![
            entry("box", true, 0),
            entry("Makefile", false, 1),
            entry("a.rs", false, 1),
            entry("b.rs", false, 1),
        ]));
        p.sort_entries(); // box, a.rs, b.rs, Makefile
        for (row, what) in [(0, ".."), (1, "a folder"), (4, "an extension-less file")] {
            p.set_cursor(row);
            assert_eq!(p.select_same_extension_as_cursor(), 0, "cursor on {what}");
            assert!(p.selection_is_empty(), "cursor on {what}");
        }
    }

    #[test]
    fn type_ahead_matches_typed_capitals_case_insensitively() {
        let mut p = panel_with(vec![
            entry("apple.txt", false, 1),
            entry("Banana.txt", false, 1),
        ]);
        p.sort_entries();
        assert!(p.type_ahead("BAN"));
        assert_eq!(p.cursor_entry().unwrap().name, "Banana.txt");
    }

    #[test]
    fn cursor_entry_is_none_when_another_entry_moved_under_the_cursor() {
        let mut p = panel_with(vec![
            entry("a", false, 1),
            entry("b", false, 1),
            entry("c", false, 1),
        ]);
        p.set_cursor(1);
        assert_eq!(p.cursor_entry().unwrap().name, "a");

        // Row 1 now holds "c", which the cursor never focused.
        p.mutate_entries_for_test(|entries| entries.reverse());
        assert!(p.cursor_entry().is_none());
        assert_eq!(
            p.selected_or_cursor().unwrap_err(),
            StaleCursor {
                cursor: 1,
                visible_entries: 3,
            }
        );
    }

    #[test]
    fn ensure_cursor_valid_clamps_a_cursor_past_the_end_and_is_otherwise_a_no_op() {
        let mut p = panel_with(vec![
            entry("a", false, 1),
            entry("b", false, 1),
            entry("c", false, 1),
        ]);
        p.set_cursor(1);
        p.set_scroll_to_cursor(false);
        p.ensure_cursor_valid();
        assert_eq!(p.cursor(), 1);
        assert!(!p.scroll_to_cursor(), "a valid cursor is left alone");

        p.set_cursor(3);
        p.set_scroll_to_cursor(false);
        p.mutate_entries_for_test(|entries| {
            entries.pop();
        });
        p.ensure_cursor_valid();
        assert_eq!(p.cursor(), 2);
        assert_eq!(p.cursor_entry().unwrap().name, "b");
        assert!(p.scroll_to_cursor());
    }

    #[test]
    fn parent_row_cursor_stays_put_across_filter_and_sort_changes() {
        let mut p = panel_with(vec![entry("a", false, 2), entry("b", false, 1)]);
        p.set_cursor(0);
        p.set_scroll_to_cursor(false);

        p.set_search_query("a");
        assert_eq!(p.cursor(), 0);
        p.set_sort(SortColumn::Size);
        assert_eq!(p.cursor(), 0);
        p.clear_filters();
        assert_eq!(p.cursor(), 0);
        assert!(p.cursor_entry().is_none());
        assert!(!p.scroll_to_cursor(), "the .. row is not scrolled to");
    }

    #[test]
    fn clear_filters_drops_query_and_facets_and_keeps_focus() {
        let mut p = panel_with(with_extensions(vec![
            entry("alpha.jpg", false, 1),
            entry("beta.jpg", false, 1),
            entry("notes.txt", false, 1),
        ]));
        p.sort_entries();
        p.set_facets(FacetSet {
            kind: Some(KindFacet::Images),
            ..FacetSet::default()
        });
        p.set_search_query("beta");
        assert_eq!(p.filtered_count(), 1);
        p.set_cursor(1);
        let focused = p.cursor_entry().unwrap().path.clone();

        p.clear_filters();

        assert_eq!(p.search_query(), "");
        assert_eq!(p.facets(), FacetSet::default());
        assert_eq!(p.filtered_count(), 3);
        assert_eq!(p.cursor_entry().map(|e| e.path.clone()), Some(focused));
        assert_eq!(p.cursor(), 2);
    }

    #[test]
    fn clear_selection_keeps_picks_remembered_in_other_folders() {
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        let b = tmp.dir("b");
        let in_a = tmp.file("a/x.txt", "x");
        let in_b = tmp.file("b/y.txt", "y");

        let mut p = PanelState::new(a);
        p.refresh();
        p.toggle_select(in_a);
        p.navigate_to(b);
        p.toggle_select(in_b);

        p.clear_selection();
        assert!(p.selection_is_empty());
        assert_eq!(p.selected_elsewhere_count(), 1, "only this folder clears");

        p.clear_selection_everywhere();
        assert_eq!(p.selected_elsewhere_count(), 0);
    }

    #[test]
    fn picks_only_in_other_folders_win_over_the_cursor_here() {
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        let b = tmp.dir("b");
        let in_a = tmp.file("a/x.txt", "x");
        tmp.file("b/y.txt", "y");

        let mut p = PanelState::new(a);
        p.refresh();
        p.toggle_select(in_a.clone());
        p.navigate_to(b);
        p.set_cursor(1);
        assert_eq!(p.cursor_entry().unwrap().name, "y.txt");

        let picked: Vec<PathBuf> = p
            .picked_entries()
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(picked, vec![in_a]);
    }

    #[test]
    fn remembered_scroll_anchor_is_clamped_when_the_folder_shrank() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        let docs = tmp.dir("docs");
        let files: Vec<PathBuf> = (0..12)
            .map(|i| tmp.file(&format!("downloads/f{i:02}.txt"), "x"))
            .collect();

        let mut p = PanelState::new(downloads.clone());
        p.refresh();
        p.set_cursor(11); // f10
        p.set_scroll_anchor(10);
        p.navigate_to(docs);
        for file in &files[1..11] {
            std::fs::remove_file(file).unwrap();
        }
        p.navigate_to(downloads);

        assert_eq!(p.filtered_count(), 2);
        assert!(
            p.scroll_anchor() < p.filtered_count(),
            "{}",
            p.scroll_anchor()
        );
        assert_eq!(p.cursor(), 2);
        assert_eq!(p.cursor_entry().unwrap().path, files[11]);
    }

    #[test]
    fn navigating_away_clears_an_in_progress_drag() {
        let tmp = TempDir::new();
        let file = tmp.file("x.txt", "x");
        let sub = tmp.dir("sub");
        let other = tmp.dir("other");
        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();

        p.begin_drag(file.clone());
        p.drag.set_drop_target(tmp.path().to_path_buf());
        p.navigate_to(sub.clone());
        assert!(p.drag.is_empty(), "synchronous listing");
        assert!(!p.drag.has_drop_target());

        // A background listing that never gets a worker: the drag is
        // dropped as soon as the breadcrumb changes, not when rows land.
        let mut limits = crate::workload::SchedulerLimits::default();
        limits.per_kind_running[0] = 0;
        let runtime = crate::workload::WorkloadRuntime::new(limits);
        p.set_workload(crate::workload::WorkloadHandle::from_runtime(runtime));
        p.set_notify(Arc::new(|| {}));
        p.begin_drag(sub.join("y"));
        p.drag.set_drop_target(sub);
        p.navigate_to(other);
        assert_eq!(p.listing.status(), DirStatus::Loading);
        assert!(p.drag.is_empty(), "asynchronous listing");
        assert!(!p.drag.has_drop_target());
    }

    #[test]
    fn from_meta_derives_a_lowercase_last_extension() {
        let tmp = TempDir::new();
        let extension = |name: &str| {
            let path = tmp.file(name, "x");
            let meta = std::fs::metadata(&path).unwrap();
            FileEntry::from_meta(path, &meta).unwrap().extension
        };
        assert_eq!(extension("PHOTO.JPG"), "jpg");
        assert_eq!(extension(".bashrc"), "");
        assert_eq!(extension("a.tar.gz"), "gz");
        assert_eq!(extension("Makefile"), "");
        let dir = tmp.dir("Folder.D");
        let meta = std::fs::metadata(&dir).unwrap();
        let folder = FileEntry::from_meta(dir, &meta).unwrap();
        assert_eq!(folder.extension, "d");
        assert!(folder.is_dir);
        assert_eq!(folder.size, 0);
    }

    #[test]
    fn videos_count_as_images_but_not_as_static_images() {
        let [video, still, text] = with_extensions(vec![
            entry("clip.MP4", false, 1),
            entry("pic.png", false, 1),
            entry("a.txt", false, 1),
        ])
        .try_into()
        .unwrap();
        assert!(video.is_video() && video.is_image() && !video.is_static_image());
        assert!(!still.is_video() && still.is_image() && still.is_static_image());
        assert!(!text.is_video() && !text.is_image());
    }

    /// Hand-driven background listings for `p`: the worker runs only when
    /// the test says so.
    fn deterministic_listings(p: &mut PanelState) -> crate::workload::DeterministicWorkload {
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        p.set_workload(workload.handle());
        p.set_notify(Arc::new(|| {}));
        workload
    }

    #[test]
    fn async_go_up_lands_on_the_child_once_the_listing_publishes() {
        let tmp = TempDir::new();
        tmp.dir("aaa");
        let mid = tmp.dir("mmm");
        tmp.dir("zzz");
        let mut p = PanelState::new(mid);
        p.refresh();
        let workload = deterministic_listings(&mut p);

        p.go_up();
        assert_eq!(p.listing.status(), DirStatus::Loading);
        assert_eq!(p.cursor(), 0, "nothing to land on yet");

        assert!(workload.run_next());
        assert!(p.poll_listing());
        assert_eq!(p.cursor_entry().unwrap().name, "mmm");
        assert!(p.scroll_to_cursor());
    }

    #[test]
    fn listing_read_under_the_old_hidden_setting_is_not_applied() {
        let tmp = TempDir::new();
        tmp.file("visible.txt", "v");
        let hidden = tmp.file(".hidden.txt", "h");
        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        let workload = deterministic_listings(&mut p);

        // A background re-read starts with hidden files off, then the user
        // shows them before it lands.
        p.refresh();
        assert_eq!(p.toggle_hidden(), ViewApplyOutcome::Applied);
        assert!(p.entries().iter().any(|entry| entry.path == hidden));
        let revision = p.entries_gen();

        assert!(workload.run_next());
        assert!(!p.poll_listing(), "the stale read is dropped");
        assert!(p.entries().iter().any(|entry| entry.path == hidden));
        assert_eq!(p.entries_gen(), revision);
    }

    #[test]
    fn preview_identity_goes_stale_when_size_or_date_changes() {
        let mut file = entry("notes.txt", false, 5);
        file.modified = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100));
        let identity = PreviewIdentity::from_entry(&file);
        assert!(identity.matches_entry(&file));

        let mut grown = file.clone();
        grown.size = 6;
        assert!(!identity.matches_entry(&grown));

        let mut touched = file.clone();
        touched.modified = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(101));
        assert!(!identity.matches_entry(&touched));
        touched.modified = None;
        assert!(!identity.matches_entry(&touched));

        let mut moved = file;
        moved.path = PathBuf::from("/test/other.txt");
        assert!(!identity.matches_entry(&moved));
    }

    #[cfg(unix)]
    #[test]
    fn recursive_size_does_not_follow_folder_symlinks_or_loop() {
        let outside = TempDir::new();
        outside.file("big.bin", &"x".repeat(4000));
        let tmp = TempDir::new();
        tmp.file("small.bin", "12345");
        tmp.dir("sub");
        // A link back to the walked folder and one to a folder elsewhere.
        std::os::unix::fs::symlink(tmp.path(), tmp.path().join("sub/loop")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("away")).unwrap();
        let link_len = |name: &str| {
            std::fs::symlink_metadata(tmp.path().join(name))
                .unwrap()
                .len()
        };

        assert_eq!(
            dir_size_walk_until(tmp.path(), false, || false),
            Some(DirWalk {
                size: 5 + link_len("sub/loop") + link_len("away"),
                complete: true,
            })
        );
    }

    #[test]
    fn cache_files_from_another_schema_load_empty() {
        let entry = r#"{"key":{"path":"/s","volume_id":1,"generation":1},"mtime_secs":5,"mtime_nanos":0,"size":7}"#;
        for json in [
            format!(r#"{{"schema":2,"entries":[{entry}]}}"#),
            format!(r#"{{"entries":[{entry}]}}"#),
            format!(r#"{{"schema":0,"entries":[{entry}]}}"#),
        ] {
            let cache = load_cache_from_reader_with_bounds(json.as_bytes(), 8, 6).unwrap();
            assert!(cache.is_empty(), "{json}");
        }
        let current = format!(r#"{{"schema":1,"entries":[{entry}]}}"#);
        let cache = load_cache_from_reader_with_bounds(current.as_bytes(), 8, 6).unwrap();
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn persisted_snapshot_round_trips_and_drops_pre_epoch_dates() {
        let key = |name: &str| VolumePathKey {
            path: PathBuf::from(format!("/persist/{name}")),
            volume_id: 3,
            generation: 4,
        };
        let at =
            |secs: u64, nanos: u32| SystemTime::UNIX_EPOCH + std::time::Duration::new(secs, nanos);
        let cache = HashMap::from([
            (key("b"), (at(200, 5), 20)),
            (key("a"), (at(100, 999_999_999), 10)),
            (
                key("ancient"),
                (
                    SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1),
                    30,
                ),
            ),
        ]);

        let persisted = persisted_snapshot(&cache);
        assert_eq!(persisted.schema, 1);
        let order: Vec<_> = persisted.entries.iter().map(|e| e.key.clone()).collect();
        assert_eq!(order, [key("a"), key("b")], "sorted, pre-1970 dropped");

        let json = serde_json::to_vec(&persisted).unwrap();
        let loaded = load_cache_from_reader_with_bounds(json.as_slice(), 8, 6).unwrap();
        let mut expected = cache;
        expected.remove(&key("ancient"));
        assert_eq!(loaded, expected);
    }

    #[test]
    fn path_epoch_is_shared_while_live_and_replaced_once_cancelled() {
        let tmp = TempDir::new();
        let key = VolumePathKey::observe(&tmp.dir("epoch"));
        let first = capture_path_epoch(&key);
        let second = capture_path_epoch(&key);
        assert!(Arc::ptr_eq(&first, &second), "concurrent walkers share it");

        first.cancel();
        let fresh = capture_path_epoch(&key);
        assert!(!Arc::ptr_eq(&first, &fresh));
        assert!(!fresh.is_cancelled());
        assert!(Arc::ptr_eq(&fresh, &capture_path_epoch(&key)));
    }

    #[test]
    fn invalidation_keeps_a_sibling_that_shares_a_name_prefix() {
        let tmp = TempDir::new();
        let b = VolumePathKey::observe(&tmp.dir("b"));
        let bc = VolumePathKey::observe(&tmp.dir("bc"));
        let now = SystemTime::now();
        {
            let _boundary = lock_recover(scan_commit_boundary());
            let mut cache = lock_recover(dir_size_cache());
            cache.insert(b.clone(), (now, 1));
            cache.insert(bc.clone(), (now, 2));
        }

        invalidate_size_cache(&tmp.path().join("bc/file.txt"));

        let cache = lock_recover(dir_size_cache());
        assert!(!cache.contains_key(&bc));
        assert_eq!(cache.get(&b), Some(&(now, 1)), "b is not an ancestor of bc");
    }

    /// A fresh panel scan over one folder, with its own size map.
    struct TestScan {
        current: Mutex<Arc<ScanEpoch>>,
        epoch: Arc<ScanEpoch>,
        sizes: Mutex<HashMap<PathBuf, u64>>,
        revision: AtomicU64,
    }

    fn scan_for(directory: &Path) -> TestScan {
        let current = Mutex::new(Arc::new(ScanEpoch::new()));
        let sizes = Mutex::new(HashMap::new());
        let revision = AtomicU64::new(0);
        let epoch = begin_panel_scan(
            &current,
            &sizes,
            &Mutex::new(HashMap::new()),
            &HashSet::from([directory.to_path_buf()]),
            &revision,
        );
        TestScan {
            current,
            epoch,
            sizes,
            revision,
        }
    }

    #[test]
    fn cached_size_is_reused_only_for_the_same_folder_mtime() {
        let tmp = TempDir::new();
        let directory = tmp.dir("cached");
        let key = VolumePathKey::observe(&directory);
        let mtime = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(5_000);
        {
            let _boundary = lock_recover(scan_commit_boundary());
            lock_recover(dir_size_cache()).insert(key.clone(), (mtime, 77));
        }
        let scan = scan_for(&directory);
        let sizes = &scan.sizes;
        let publish = |expected: Option<SystemTime>| {
            publish_cached_size_if_current(
                &scan.current,
                &scan.epoch,
                sizes,
                &directory,
                &key,
                expected,
                &scan.revision,
            )
        };

        let later = mtime + std::time::Duration::from_secs(1);
        assert!(!publish(Some(later)), "the folder changed since");
        assert_eq!(lock_recover(sizes).get(&directory), None);

        assert!(publish(Some(mtime)));
        assert_eq!(lock_recover(sizes).get(&directory), Some(&77));
        // No expected mtime (a walk held back by its cooldown) takes it as is.
        lock_recover(sizes).clear();
        assert!(publish(None));
        assert_eq!(lock_recover(sizes).get(&directory), Some(&77));
    }

    #[test]
    fn measurement_without_an_mtime_is_shown_but_not_cached() {
        let tmp = TempDir::new();
        let directory = tmp.dir("undated");
        let cache_key = VolumePathKey::observe(&directory);
        let path_epoch = capture_path_epoch(&cache_key);
        let scan = scan_for(&directory);
        let measurement = DirMeasurement {
            path: directory.clone(),
            modified: None,
            complete: true,
            cache_key: cache_key.clone(),
            path_epoch,
            size: 9,
            completed_at: std::time::Instant::now(),
            elapsed: std::time::Duration::from_millis(1),
        };

        let outcome = publish_dir_measurements_if_current(
            &scan.current,
            &scan.epoch,
            &scan.sizes,
            &[measurement],
            &scan.revision,
        );

        assert_eq!(outcome.published, 1);
        assert!(!outcome.cache_changed);
        assert_eq!(lock_recover(&scan.sizes).get(&directory), Some(&9));
        let _boundary = lock_recover(scan_commit_boundary());
        assert!(!lock_recover(dir_size_cache()).contains_key(&cache_key));
        assert!(!lock_recover(partial_size_cache()).contains_key(&cache_key));
    }

    #[test]
    fn merging_a_measurement_replaces_the_folders_old_mount_generation() {
        let key = |name: &str, generation: u64| VolumePathKey {
            path: PathBuf::from(format!("/merge/{name}")),
            volume_id: 1,
            generation,
        };
        let at = |secs: u64| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let measured = DirMeasurement {
            path: PathBuf::from("/merge/a"),
            modified: Some(at(50)),
            complete: true,
            cache_key: key("a", 2),
            path_epoch: Arc::new(ScanEpoch::new()),
            size: 42,
            completed_at: std::time::Instant::now(),
            elapsed: std::time::Duration::from_millis(1),
        };
        let previous = HashMap::from([(key("a", 1), (at(40), 1)), (key("other", 1), (at(30), 3))]);

        let merged = merge_measurements_into_cache(previous, &[&measured], 8, 6);

        assert_eq!(
            merged,
            HashMap::from([(key("a", 2), (at(50), 42)), (key("other", 1), (at(30), 3))])
        );
    }

    #[test]
    fn merging_past_the_limit_keeps_the_newest_entries() {
        let key = |index: u64| VolumePathKey {
            path: PathBuf::from(format!("/merge-bound/{index}")),
            volume_id: 1,
            generation: 1,
        };
        let at = |secs: u64| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let previous: HashMap<_, _> = (0..4).map(|i| (key(i), (at(10 + i), i))).collect();
        let measured = DirMeasurement {
            path: key(9).path,
            modified: Some(at(5)),
            complete: true,
            cache_key: key(9),
            path_epoch: Arc::new(ScanEpoch::new()),
            size: 9,
            completed_at: std::time::Instant::now(),
            elapsed: std::time::Duration::from_millis(1),
        };

        let merged = merge_measurements_into_cache(previous, &[&measured], 4, 3);

        // Five entries pass the limit of four; the three newest mtimes stay,
        // which drops the fresh measurement of an old folder too.
        let mut kept: Vec<_> = merged.keys().map(|key| key.path.clone()).collect();
        kept.sort();
        assert_eq!(kept, [key(1).path, key(2).path, key(3).path]);
        assert!(merged.capacity() <= hash_map_capacity_upper_bound(4));
    }
}
