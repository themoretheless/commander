use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

mod text_preview;
pub use text_preview::*;
mod native_loaders;
use native_loaders::*;

pub const MAX_CACHE_BYTES: usize = 1024 * 1024 * 1024; // 1 GB
const MAX_DECODED_IMAGE_BYTES: usize = 256 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 32_768;
const MAX_PRELOAD_WORKERS: usize = 4;
const MAX_PREVIEW_TARGET_DIMENSION: u32 = 4_096;
const DECODE_TIMEOUT: Duration = Duration::from_secs(12);
/// How long a capacity refusal (busy decoders, power pressure) is shown
/// before the next preload pass tries the image again.
const TRANSIENT_FAILURE_RETRY: Duration = Duration::from_secs(2);
const MAX_TEXT_PREVIEW_CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TEXT_PREVIEW_CACHE_ENTRIES: usize = 16;
const TEXT_PREVIEW_DEBOUNCE: Duration = Duration::from_millis(75);
const TEXT_PREVIEW_POLL_INTERVAL: Duration = Duration::from_millis(100);
const TEXT_PREVIEW_TIMEOUT: Duration = Duration::from_secs(8);
const TEXT_PREVIEW_CHUNK_BYTES: usize = 32 * 1024;
const MAX_TEXT_PREVIEW_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewTarget {
    width: u32,
    height: u32,
}

impl PreviewTarget {
    const DEFAULT: Self = Self {
        width: 1_600,
        height: 1_200,
    };

    fn from_points(width: f32, height: f32, pixels_per_point: f32) -> Self {
        let scaled = |points: f32| {
            let pixels = if points.is_finite() && pixels_per_point.is_finite() {
                points.max(1.0) * pixels_per_point.max(1.0)
            } else {
                1.0
            };
            (pixels.ceil() as u32).clamp(1, MAX_PREVIEW_TARGET_DIMENSION)
        };
        Self {
            width: scaled(width),
            height: scaled(height),
        }
    }

    const fn covers(self, other: Self) -> bool {
        self.width >= other.width && self.height >= other.height
    }

    #[cfg(target_os = "macos")]
    const fn max_dimension(self) -> u32 {
        if self.width > self.height {
            self.width
        } else {
            self.height
        }
    }
}

#[derive(Debug)]
struct DecodedPreview {
    image: ColorImage,
    byte_size: usize,
    decoded_for: PreviewTarget,
}

struct CacheEntry {
    texture: TextureHandle,
    byte_size: usize,
    last_used: u64, // frame counter
    decoded_for: PreviewTarget,
}

/// A background-load slot: the cache generation that scheduled it, plus the
/// decoded image once the worker finishes (`None` while still loading).
#[derive(Debug)]
struct PendingLoad {
    generation: u64,
    decoded: Option<DecodedPreview>,
}

/// Background-load slots shared with the worker threads, keyed by path.
type PendingMap = Arc<Mutex<HashMap<PathBuf, PendingLoad>>>;

/// Remove `path`'s slot only if `generation` still owns it. A stale worker
/// (folder A -> B -> A) must not release the newer generation's slot for the
/// same path. Returns whether the slot was released.
fn release_pending(pending: &PendingMap, path: &Path, generation: u64) -> bool {
    let mut pending = crate::lock_util::recover(pending);
    if pending
        .get(path)
        .is_some_and(|slot| slot.generation == generation)
    {
        pending.remove(path);
        true
    } else {
        false
    }
}

/// Store a finished decode only into the slot `generation` scheduled.
fn publish_pending(
    pending: &PendingMap,
    path: &Path,
    generation: u64,
    decoded: DecodedPreview,
) -> bool {
    let mut pending = crate::lock_util::recover(pending);
    match pending.get_mut(path) {
        Some(slot) if slot.generation == generation => {
            slot.decoded = Some(decoded);
            true
        }
        _ => false,
    }
}

/// Worker slots left for new decodes. A decoder that outlived its timeout no
/// longer has a pending slot but still holds a decoder permit, so the busier
/// of the two counts bounds new work.
fn free_preload_slots(pending: usize, active_decoders: usize) -> usize {
    MAX_PRELOAD_WORKERS.saturating_sub(pending.max(active_decoders))
}
type FailedMap = Arc<Mutex<HashMap<PathBuf, FailedLoad>>>;

/// A negative-cache entry. Capacity refusals expire on their own; real
/// decode failures stay until the user retries or the folder changes.
#[derive(Clone, Copy, Debug)]
struct FailedLoad {
    failure: PreviewFailure,
    retry_at: Option<Instant>,
}

impl FailedLoad {
    fn new(failure: PreviewFailure, now: Instant) -> Self {
        Self {
            failure,
            retry_at: (failure == PreviewFailure::Busy).then(|| now + TRANSIENT_FAILURE_RETRY),
        }
    }

    fn expired(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|retry_at| now >= retry_at)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewError {
    TooLarge,
    NotRegular,
    Unreadable,
    Binary,
    InvalidUnicode,
    InvalidUtf8,
    InvalidDocument,
    Changed,
    Cancelled,
    ProviderUnavailable,
    ProviderPanicked,
    SchedulerUnavailable,
    WorkerBusy,
    TimedOut,
}

impl PreviewError {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "This file exceeds the text preview limit.",
            Self::NotRegular => "Only regular files can be previewed as text.",
            Self::Unreadable => "Commander cannot read this file.",
            Self::Binary => "This file appears to contain binary data.",
            Self::InvalidUnicode => "This file contains malformed UTF-16 or UTF-32 text.",
            Self::InvalidUtf8 => "This file is not valid UTF-8 text.",
            Self::InvalidDocument => "This document is malformed, encrypted or unsupported.",
            // Retry reuses the listing's size and mtime, so it keeps failing
            // until the folder is refreshed (watched folders do this alone).
            Self::Changed => {
                "The file changed while it was being read. Refresh the folder, then retry the preview."
            }
            Self::Cancelled => "The preview request was cancelled.",
            Self::ProviderUnavailable => "No text preview provider is available for this file.",
            Self::ProviderPanicked => "The text preview provider stopped unexpectedly.",
            Self::SchedulerUnavailable => "The preview queue is busy. Retry in a moment.",
            // Shown once a hung earlier read (dead mount) has kept the single
            // preview worker for a full timeout.
            Self::WorkerBusy => {
                "An earlier preview read is not responding. Retry in a moment, or open the file directly."
            }
            Self::TimedOut => {
                "The text preview took too long to load. Retry, or open the file directly."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewFailure {
    TooLarge,
    Unreadable,
    Unsupported,
    TimedOut,
    Busy,
    /// The decoder breaker paused this format after repeated failures.
    Quarantined,
    Decode,
}

impl PreviewFailure {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "This image exceeds the preview memory limit.",
            Self::Unreadable => "Commander cannot read this file.",
            Self::Unsupported => "This image format is not supported.",
            Self::TimedOut => {
                "The preview took too long to load. Retry, or open the file directly."
            }
            Self::Busy => "All preview decoders are busy. Retry in a moment.",
            Self::Quarantined => {
                "Previews of this format are paused after repeated decode failures. Retry to test the decoder again."
            }
            Self::Decode => "The image is incomplete or damaged.",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderTally {
    pub attempts: u64,
    pub successes: u64,
    pub failures: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreviewProviderStats {
    pub image_io: ProviderTally,
    pub standard: ProviderTally,
    pub video: ProviderTally,
    pub fallbacks: u64,
    pub timeouts: u64,
    pub saturated: u64,
    pub active_decoders: usize,
}

#[derive(Default)]
struct ProviderCounters {
    image_io: ProviderAtomicTally,
    standard: ProviderAtomicTally,
    video: ProviderAtomicTally,
    fallbacks: AtomicU64,
    timeouts: AtomicU64,
    saturated: AtomicU64,
}

#[derive(Default)]
struct ProviderAtomicTally {
    attempts: AtomicU64,
    successes: AtomicU64,
    failures: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecoderProvider {
    #[cfg(target_os = "macos")]
    ImageIo,
    Standard,
    #[cfg(target_os = "macos")]
    Video,
}

fn provider_counters() -> &'static ProviderCounters {
    static COUNTERS: OnceLock<ProviderCounters> = OnceLock::new();
    COUNTERS.get_or_init(ProviderCounters::default)
}

fn provider_tally(provider: DecoderProvider) -> &'static ProviderAtomicTally {
    let counters = provider_counters();
    match provider {
        #[cfg(target_os = "macos")]
        DecoderProvider::ImageIo => &counters.image_io,
        DecoderProvider::Standard => &counters.standard,
        #[cfg(target_os = "macos")]
        DecoderProvider::Video => &counters.video,
    }
}

fn record_provider_result<T>(
    provider: DecoderProvider,
    result: Result<T, String>,
) -> Result<T, String> {
    let tally = provider_tally(provider);
    tally.attempts.fetch_add(1, Ordering::Relaxed);
    if result.is_ok() {
        tally.successes.fetch_add(1, Ordering::Relaxed);
    } else {
        tally.failures.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn preview_provider_stats() -> PreviewProviderStats {
    let counters = provider_counters();
    let tally = |value: &ProviderAtomicTally| ProviderTally {
        attempts: value.attempts.load(Ordering::Relaxed),
        successes: value.successes.load(Ordering::Relaxed),
        failures: value.failures.load(Ordering::Relaxed),
    };
    PreviewProviderStats {
        image_io: tally(&counters.image_io),
        standard: tally(&counters.standard),
        video: tally(&counters.video),
        fallbacks: counters.fallbacks.load(Ordering::Relaxed),
        timeouts: counters.timeouts.load(Ordering::Relaxed),
        saturated: counters.saturated.load(Ordering::Relaxed),
        active_decoders: ACTIVE_DECODERS.load(Ordering::Acquire),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewLoadState {
    Disabled,
    Loading,
    Failed(PreviewFailure),
    Idle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageCacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub pending: usize,
    pub failed: usize,
    pub providers: PreviewProviderStats,
}

/// A listing's (size, mtime) for a file, used to spot in-place edits.
pub type FileStamp = (u64, Option<std::time::SystemTime>);

pub struct ImageCache {
    entries: HashMap<PathBuf, CacheEntry>,
    /// The listing stamp each preloaded path was last seen with.
    stamps: HashMap<PathBuf, FileStamp>,
    total_bytes: usize,
    frame: u64,
    /// Images currently being loaded in background
    pending: PendingMap,
    failed: FailedMap,
    generation: Arc<AtomicU64>,
    current_dir: Option<PathBuf>,
    preview_target: PreviewTarget,
    text_previews: TextPreviewPipeline<IsolatedTextPreviewExecutor>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            stamps: HashMap::new(),
            total_bytes: 0,
            frame: 0,
            pending: Arc::new(Mutex::new(HashMap::new())),
            failed: Arc::new(Mutex::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            current_dir: None,
            preview_target: PreviewTarget::DEFAULT,
            text_previews: TextPreviewPipeline::production(),
        }
    }

    pub fn set_preview_extent(&mut self, width: f32, height: f32, pixels_per_point: f32) {
        self.preview_target = PreviewTarget::from_points(width, height, pixels_per_point);
    }

    pub(crate) fn request_text_preview(
        &mut self,
        ctx: &Context,
        identity: &crate::panel::PreviewIdentity,
    ) -> TextPreviewPoll {
        self.text_previews.request(ctx, identity)
    }

    pub(crate) fn text_preview_load_state(&self, path: &Path) -> TextPreviewLoadState {
        self.text_previews.load_state(path)
    }

    pub(crate) fn cancel_text_preview(&mut self) {
        self.text_previews.cancel();
    }

    pub(crate) fn cancel_text_preview_if_active(
        &mut self,
        identity: &crate::panel::PreviewIdentity,
    ) {
        self.text_previews.cancel_if_active(identity);
    }

    pub(crate) fn retry_text_preview(&mut self, path: &Path) {
        self.text_previews.retry(path);
    }

    /// Get cached texture for a path, or None if not loaded yet.
    pub fn get(&mut self, path: &Path) -> Option<&TextureHandle> {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ImagePreview) {
            return None;
        }
        if let Some(entry) = self.entries.get_mut(path) {
            entry.last_used = self.frame;
            Some(&entry.texture)
        } else {
            None
        }
    }

    /// Forget anything cached for a path whose listing (size, mtime) changed
    /// since it was last seen, so a file edited in place is decoded again
    /// instead of showing its old texture (or old failure) until the folder
    /// changes. An in-flight decode of the old content is dropped too.
    /// Stamps come from the listing, so no file is stat-ed on the UI thread.
    /// Stamps are dropped with the textures when `preload` flushes a folder.
    pub fn invalidate_changed(&mut self, stamped: &[(PathBuf, FileStamp)]) {
        for (path, stamp) in stamped {
            let previous = self.stamps.insert(path.clone(), *stamp);
            if previous.is_some_and(|previous| previous != *stamp) {
                self.remove_entry(path);
                crate::lock_util::recover(&self.failed).remove(path);
                crate::lock_util::recover(&self.pending).remove(path);
                // A new generation, so the old decode cannot fill the slot
                // the next `preload` opens for the same path.
                self.generation.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    /// Request preloading in priority order, with fixed worker concurrency.
    pub fn preload(&mut self, ctx: &Context, paths: &[PathBuf], dir: &Path) {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ImagePreview) {
            let mut pending = crate::lock_util::recover(&self.pending);
            if !pending.is_empty() {
                pending.clear();
                self.generation.fetch_add(1, Ordering::AcqRel);
            }
            drop(pending);
            self.clear_entries();
            self.current_dir = None;
            crate::lock_util::recover(&self.failed).clear();
            return;
        }
        self.frame += 1;

        // Track directory change: flush the cache so a stale preview from the
        // old folder can never be served (the get() path only checks by path,
        // and two folders can hold same-named-but-different images).
        if self.current_dir.as_deref() != Some(dir) {
            self.current_dir = Some(dir.to_path_buf());
            self.clear_entries();
            self.generation.fetch_add(1, Ordering::AcqRel);
            crate::lock_util::recover(&self.pending).clear();
            crate::lock_util::recover(&self.failed).clear();
        }

        // Publish finished decodes before calculating free worker slots.
        let completed: Vec<(PathBuf, DecodedPreview)> = {
            let mut pending = crate::lock_util::recover(&self.pending);
            let paths: Vec<_> = pending
                .iter()
                .filter(|(_, slot)| slot.decoded.is_some())
                .map(|(path, _)| path.clone())
                .collect();
            paths
                .into_iter()
                .filter_map(|path| match pending.remove(&path) {
                    Some(PendingLoad {
                        decoded: Some(decoded),
                        ..
                    }) => Some((path, decoded)),
                    _ => None,
                })
                .collect()
        };
        for (path, decoded) in completed {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let texture = ctx.load_texture(&name, decoded.image, TextureOptions::LINEAR);
            // A sharper re-decode replaces the smaller texture that stayed
            // on screen while it loaded.
            self.remove_entry(&path);
            self.total_bytes = self.total_bytes.saturating_add(decoded.byte_size);
            self.entries.insert(
                path,
                CacheEntry {
                    texture,
                    byte_size: decoded.byte_size,
                    last_used: self.frame,
                    decoded_for: decoded.decoded_for,
                },
            );
        }

        let target = self.preview_target;
        self.purge_expired_failures(Instant::now());
        let failed = crate::lock_util::recover(&self.failed).clone();
        let mut available_slots = free_preload_slots(
            crate::lock_util::recover(&self.pending).len(),
            ACTIVE_DECODERS.load(Ordering::Acquire),
        );
        for path in paths {
            if self
                .entries
                .get(path)
                .is_some_and(|entry| entry.decoded_for.covers(target))
            {
                if let Some(entry) = self.entries.get_mut(path) {
                    entry.last_used = self.frame;
                }
                continue;
            }
            // A cached texture smaller than the pane (it just grew) stays
            // until the sharper decode publishes, so the preview never blanks
            // while it loads or while every worker slot is busy.
            if available_slots == 0 || failed.contains_key(path) {
                continue;
            }
            let active_generation = Arc::clone(&self.generation);
            let generation = active_generation.load(Ordering::Acquire);
            {
                let mut pending = crate::lock_util::recover(&self.pending);
                if pending.contains_key(path) {
                    continue;
                }
                pending.insert(
                    path.clone(),
                    PendingLoad {
                        generation,
                        decoded: None,
                    },
                );
            }
            let path_clone = path.clone();
            let pending_clone = Arc::clone(&self.pending);
            let failed_clone = Arc::clone(&self.failed);
            let ctx_clone = ctx.clone();
            let abandoned_path = path.clone();
            let abandoned_pending = Arc::clone(&self.pending);
            let task = crate::workload::submit_with_abandonment(
                crate::workload::TaskSpec::new(
                    crate::workload::TaskKind::Preview,
                    dir.to_path_buf(),
                    generation,
                )
                .priority(crate::workload::Priority::Interactive)
                .estimated_bytes(64 * 1024 * 1024)
                .replace_older_generation(),
                move |scheduler_cancel| {
                    let cancelled = || {
                        scheduler_cancel.is_cancelled()
                            || active_generation.load(Ordering::Acquire) != generation
                    };
                    if cancelled() || !crate::io_budget::background_checkpoint(cancelled) {
                        release_pending(&pending_clone, &path_clone, generation);
                        return;
                    }
                    let result = load_image_with_timeout(&path_clone, target);
                    if cancelled() {
                        release_pending(&pending_clone, &path_clone, generation);
                        return;
                    }
                    match result {
                        Ok(decoded) => {
                            if publish_pending(&pending_clone, &path_clone, generation, decoded) {
                                ctx_clone.request_repaint();
                            }
                        }
                        Err(error) => {
                            if release_pending(&pending_clone, &path_clone, generation) {
                                let failure =
                                    FailedLoad::new(classify_failure(&error), Instant::now());
                                crate::lock_util::recover(&failed_clone)
                                    .insert(path_clone, failure);
                                if failure.retry_at.is_some() {
                                    ctx_clone.request_repaint_after(TRANSIENT_FAILURE_RETRY);
                                }
                                ctx_clone.request_repaint();
                            }
                        }
                    }
                },
                // A job dropped before it ran (spawn failure, superseded,
                // backend gone) must not leave its slot stuck in Loading.
                move |_reason| {
                    release_pending(&abandoned_pending, &abandoned_path, generation);
                },
            );
            if task.is_ok() {
                available_slots -= 1;
            } else {
                release_pending(&self.pending, path, generation);
            }
        }

        // Evict entries not in the keep set and over budget
        let keep_set: std::collections::HashSet<&PathBuf> = paths.iter().collect();
        self.evict(&keep_set);
    }

    fn evict(&mut self, keep: &std::collections::HashSet<&PathBuf>) {
        let victims = eviction_order(
            self.entries
                .iter()
                .map(|(path, entry)| (path, entry.byte_size, entry.last_used)),
            keep,
            self.total_bytes,
            MAX_CACHE_BYTES,
        );
        for path in victims {
            self.remove_entry(&path);
        }
    }

    /// Drop one texture and release its bytes from the running total.
    fn remove_entry(&mut self, path: &Path) -> Option<CacheEntry> {
        let entry = self.entries.remove(path)?;
        self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
        Some(entry)
    }

    fn clear_entries(&mut self) {
        self.entries.clear();
        self.stamps.clear();
        self.total_bytes = 0;
    }

    pub fn load_state(&self, path: &Path) -> PreviewLoadState {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ImagePreview) {
            return PreviewLoadState::Disabled;
        }
        if let Some(failed) = crate::lock_util::recover(&self.failed).get(path).copied() {
            return PreviewLoadState::Failed(failed.failure);
        }
        if crate::lock_util::recover(&self.pending).contains_key(path) {
            PreviewLoadState::Loading
        } else {
            PreviewLoadState::Idle
        }
    }

    /// Drop capacity refusals whose retry delay has passed.
    fn purge_expired_failures(&self, now: Instant) {
        crate::lock_util::recover(&self.failed).retain(|_, failed| !failed.expired(now));
    }

    /// Clear a negative cache entry so the next preload pass can try again.
    /// Arms a one-shot decoder probe retry when the format is quarantined (J010).
    pub fn retry(&mut self, path: &Path) {
        crate::decoder_breaker::arm_probe_retry(path);
        crate::lock_util::recover(&self.failed).remove(path);
        crate::lock_util::recover(&self.pending).remove(path);
        self.remove_entry(path);
    }

    pub fn stats(&self) -> ImageCacheStats {
        ImageCacheStats {
            entries: self.entries.len(),
            bytes: self.total_bytes,
            pending: crate::lock_util::recover(&self.pending).len(),
            failed: crate::lock_util::recover(&self.failed).len(),
            providers: preview_provider_stats(),
        }
    }
}

/// Which cached textures to drop, in order, to get `total_bytes` within
/// `budget`. Under budget nothing goes. Over it, every entry outside `keep`
/// (another folder's or no longer visible) goes first, then the least
/// recently used of the kept ones until the total fits.
fn eviction_order<'a>(
    entries: impl Iterator<Item = (&'a PathBuf, usize, u64)>,
    keep: &std::collections::HashSet<&PathBuf>,
    mut total_bytes: usize,
    budget: usize,
) -> Vec<PathBuf> {
    if total_bytes <= budget {
        return Vec::new();
    }
    let mut victims = Vec::new();
    let mut kept = Vec::new();
    for (path, byte_size, last_used) in entries {
        if keep.contains(path) {
            kept.push((path, byte_size, last_used));
        } else {
            total_bytes = total_bytes.saturating_sub(byte_size);
            victims.push(path.clone());
        }
    }
    kept.sort_by_key(|(_, _, last_used)| *last_used);
    for (path, byte_size, _) in kept {
        if total_bytes <= budget {
            break;
        }
        total_bytes = total_bytes.saturating_sub(byte_size);
        victims.push(path.clone());
    }
    victims
}

fn classify_failure(error: &str) -> PreviewFailure {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("quarantined") {
        PreviewFailure::Quarantined
    } else if normalized.contains("unsupported video format") {
        // Checked before the keywords below: AVFoundation's own description
        // (e.g. "Cannot Open") follows and must not change the verdict.
        PreviewFailure::Unsupported
    } else if normalized.contains("video frame could not be read") {
        PreviewFailure::Decode
    } else if normalized.contains("still busy")
        || normalized.contains("decoder capacity")
        || normalized.contains("could not start")
        || normalized.contains("deferred")
    {
        PreviewFailure::Busy
    } else if normalized.contains("timed out") || normalized.contains("time budget") {
        PreviewFailure::TimedOut
    } else if normalized.contains("zero dimensions") {
        // An empty image is damaged, not over the dimension limit.
        PreviewFailure::Decode
    } else if normalized.contains("too large")
        || normalized.contains("allocation")
        || normalized.contains("memory")
        || normalized.contains("limit")
        || normalized.contains("dimension")
    {
        PreviewFailure::TooLarge
    } else if normalized.contains("permission")
        || normalized.contains("denied")
        || normalized.contains("not found")
        || normalized.contains("no such file")
        || normalized.contains("cannot open")
    {
        PreviewFailure::Unreadable
    } else if normalized.contains("unsupported")
        || normalized.contains("could not be determined")
        || normalized.contains("image source")
    {
        PreviewFailure::Unsupported
    } else {
        PreviewFailure::Decode
    }
}

/// Lowercase video extensions, shared with `panel::FileEntry::is_video`.
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "avi", "mkv", "webm", "m4v", "wmv", "flv"];

/// Case-insensitive extension check without allocating during scans.
fn has_extension_in(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            extensions
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

#[cfg(target_os = "macos")]
fn is_video_ext(path: &Path) -> bool {
    has_extension_in(path, VIDEO_EXTENSIONS)
}

/// Extensions the standard `image` crate path cannot decode. Early-reject so
/// callers classify as Unsupported even when platform error strings differ.
fn is_undecodable_standard_ext(path: &Path) -> bool {
    has_extension_in(path, &["svg"]) || has_extension_in(path, VIDEO_EXTENSIONS)
}

/// Allocate a zeroed RGBA buffer without integer wrap or an aborting reserve.
/// CoreGraphics dimensions are trusted only after both multiplications and the
/// allocation request have succeeded.
#[cfg(any(target_os = "macos", test))]
fn allocate_rgba_pixels(width: usize, height: usize) -> Result<(usize, Vec<u8>), String> {
    if width > MAX_IMAGE_DIMENSION as usize || height > MAX_IMAGE_DIMENSION as usize {
        return Err("image dimensions exceed the preview limit".to_string());
    }
    let bytes_per_row = width
        .checked_mul(4)
        .ok_or_else(|| "image row is too wide".to_string())?;
    let byte_len = height
        .checked_mul(bytes_per_row)
        .ok_or_else(|| "image pixel buffer is too large".to_string())?;
    if byte_len > MAX_DECODED_IMAGE_BYTES {
        return Err("image pixel buffer exceeds the preview memory limit".to_string());
    }
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(byte_len)
        .map_err(|_| "image pixel buffer allocation failed".to_string())?;
    pixels.resize(byte_len, 0);
    Ok((bytes_per_row, pixels))
}

/// Convert a checked RGBA allocation into egui pixels without letting the
/// second allocation panic on an oversized or malformed buffer.
fn color_image_from_rgba(size: [usize; 2], rgba: Vec<u8>) -> Result<(ColorImage, usize), String> {
    color_image_from_rgba_managed(size, rgba, crate::color_manage::ColorSpaceHint::Srgb, false)
}

fn color_image_from_rgba_managed(
    size: [usize; 2],
    mut rgba: Vec<u8>,
    space: crate::color_manage::ColorSpaceHint,
    hdr: bool,
) -> Result<(ColorImage, usize), String> {
    let pixel_count = size[0]
        .checked_mul(size[1])
        .ok_or_else(|| "image dimensions are too large".to_string())?;
    let byte_size = pixel_count
        .checked_mul(4)
        .ok_or_else(|| "image pixel buffer is too large".to_string())?;
    if byte_size > MAX_DECODED_IMAGE_BYTES {
        return Err("image pixel buffer exceeds the preview memory limit".to_string());
    }
    if rgba.len() != byte_size {
        return Err("decoder returned an incomplete RGBA buffer".to_string());
    }

    let frame = crate::color_manage::ColorManagedFrame::display_referred(
        size[0] as u32,
        size[1] as u32,
        space,
        hdr,
    );
    frame.apply_rgba8(&mut rgba);

    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(pixel_count)
        .map_err(|_| "image color buffer allocation failed".to_string())?;
    pixels.extend(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])),
    );
    Ok((ColorImage::new(size, pixels), byte_size))
}

static ACTIVE_DECODERS: AtomicUsize = AtomicUsize::new(0);

/// Serializes tests that drive `run_decode_with_timeout`. The decoder permits
/// in `ACTIVE_DECODERS` are process-global, so a test that deliberately holds
/// all of them would make a concurrent decode in another test see "busy" (or
/// see its own decode refused). Any test reaching the decode path, directly or
/// through `preload_images`, must hold this lock (widen its visibility when a
/// test outside this module needs it).
#[cfg(test)]
static DECODER_PERMITS_TEST_LOCK: Mutex<()> = Mutex::new(());

struct DecoderPermit;

impl DecoderPermit {
    fn acquire() -> Option<Self> {
        ACTIVE_DECODERS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_PRELOAD_WORKERS).then_some(active + 1)
            })
            .ok()
            .map(|_| Self)
    }
}

impl Drop for DecoderPermit {
    fn drop(&mut self) {
        ACTIVE_DECODERS.fetch_sub(1, Ordering::AcqRel);
    }
}

fn run_decode_with_timeout(
    timeout: Duration,
    decode: impl FnOnce() -> Result<DecodedPreview, String> + Send + 'static,
) -> Result<DecodedPreview, String> {
    let permit = match DecoderPermit::acquire() {
        Some(permit) => permit,
        None => {
            provider_counters()
                .saturated
                .fetch_add(1, Ordering::Relaxed);
            return Err("preview providers are still busy after an earlier timeout".to_string());
        }
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("commander-preview-decoder".to_string())
        .spawn(move || {
            let _permit = permit;
            let _ = sender.send(decode());
        })
        .map_err(|error| format!("preview provider thread could not start: {error}"))?;

    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            provider_counters().timeouts.fetch_add(1, Ordering::Relaxed);
            Err("preview provider timed out".to_string())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("preview provider stopped unexpectedly".to_string())
        }
    }
}

fn load_image_with_timeout(path: &Path, target: PreviewTarget) -> Result<DecodedPreview, String> {
    if !crate::machine_pressure::snapshot().allows_background_preview() {
        return Err("preview deferred under battery/thermal pressure".to_string());
    }
    match crate::decoder_breaker::admit(path) {
        crate::decoder_breaker::AdmitDecision::Quarantined => {
            return Err(
                "preview format is temporarily quarantined after repeated decode failures"
                    .to_string(),
            );
        }
        crate::decoder_breaker::AdmitDecision::ProbeRetry
        | crate::decoder_breaker::AdmitDecision::Allow => {}
    }
    let path_buf = path.to_path_buf();
    let result = run_decode_with_timeout(DECODE_TIMEOUT, {
        let path_buf = path_buf.clone();
        move || load_image_from_disk(&path_buf, target)
    });
    match &result {
        Ok(_) => crate::decoder_breaker::record_success(&path_buf),
        Err(error) if !counts_against_decoder(error) => {}
        Err(_) => crate::decoder_breaker::record_failure(&path_buf),
    }
    result
}

/// Capacity refusals say nothing about the format, so they must not trip the
/// per-format breaker. Timeouts still count: a decoder that hangs keeps its
/// permit, and the breaker is what stops one format from pinning them all.
fn counts_against_decoder(error: &str) -> bool {
    classify_failure(error) != PreviewFailure::Busy
}

fn load_image_from_disk(path: &Path, target: PreviewTarget) -> Result<DecodedPreview, String> {
    // Video uses AVFoundation's orientation-aware thumbnail provider.
    #[cfg(target_os = "macos")]
    if is_video_ext(path) {
        return record_provider_result(DecoderProvider::Video, load_video_thumbnail(path, target));
    }

    // ImageIO handles macOS-native camera/HEIF formats. A typed standard
    // provider remains available as a bounded fallback for common formats.
    #[cfg(target_os = "macos")]
    {
        match record_provider_result(DecoderProvider::ImageIo, load_via_imageio(path, target)) {
            Ok(decoded) => Ok(decoded),
            Err(image_io_error) => {
                provider_counters()
                    .fallbacks
                    .fetch_add(1, Ordering::Relaxed);
                record_provider_result(
                    DecoderProvider::Standard,
                    load_via_image_crate(path, target),
                )
                .map_err(|standard_error| {
                    format!("ImageIO: {image_io_error}; standard decoder: {standard_error}")
                })
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    record_provider_result(
        DecoderProvider::Standard,
        load_via_image_crate(path, target),
    )
}

/// Stream a standard image decoder from the file instead of retaining a second
/// full compressed copy beside the decoded pixels.
fn load_via_image_crate(path: &Path, target: PreviewTarget) -> Result<DecodedPreview, String> {
    use image::ImageDecoder as _;

    // Formats the standard crate path must never claim to decode: reject up
    // front so negative-cache classification stays Unsupported across platforms
    // (macOS ImageIO/AVFoundation error strings often omit "unsupported").
    if is_undecodable_standard_ext(path) {
        return Err("unsupported image format".into());
    }

    let mut reader = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES as u64);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let img = image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    let rgba = fit_oriented(img, orientation, target).into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (space, hdr) = crate::color_manage::hint_from_extension(&extension);
    let (image, byte_size) = if matches!(space, crate::color_manage::ColorSpaceHint::Srgb) && !hdr {
        color_image_from_rgba(size, pixels)?
    } else {
        color_image_from_rgba_managed(size, pixels, space, hdr)?
    };
    Ok(DecodedPreview {
        image,
        byte_size,
        decoded_for: target,
    })
}

/// Downscale `img` to fit `target` once `orientation` is applied. The
/// thumbnail is taken first (into a swapped box for quarter turns) so a
/// rotation copies the small image, not a second full-resolution buffer.
fn fit_oriented(
    img: image::DynamicImage,
    orientation: image::metadata::Orientation,
    target: PreviewTarget,
) -> image::DynamicImage {
    use image::metadata::Orientation;
    let quarter_turn = matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    let (box_width, box_height) = if quarter_turn {
        (target.height, target.width)
    } else {
        (target.width, target.height)
    };
    let mut img = if img.width() > box_width || img.height() > box_height {
        img.thumbnail(box_width, box_height)
    } else {
        img
    };
    img.apply_orientation(orientation);
    img
}

#[cfg(test)]
mod tests;
