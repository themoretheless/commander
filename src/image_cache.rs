use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

pub(crate) const MAX_CACHE_BYTES: usize = 1024 * 1024 * 1024; // 1 GB
const MAX_DECODED_IMAGE_BYTES: usize = 256 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 32_768;
const MAX_PRELOAD_WORKERS: usize = 4;
const MAX_PREVIEW_TARGET_DIMENSION: u32 = 4_096;
const DECODE_TIMEOUT: Duration = Duration::from_secs(12);
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

/// A decoded image plus its GPU byte size, or `None` while still loading.
type PendingLoad = Option<DecodedPreview>;
/// Background-load slots shared with the worker threads, keyed by path.
type PendingMap = Arc<Mutex<HashMap<PathBuf, PendingLoad>>>;
type FailedMap = Arc<Mutex<HashMap<PathBuf, PreviewFailure>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewError {
    TooLarge,
    NotRegular,
    Unreadable,
    Binary,
    UnsupportedEncoding,
    InvalidUtf8,
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
            Self::UnsupportedEncoding => "UTF-16 text preview is not supported.",
            Self::InvalidUtf8 => "This file is not valid UTF-8 text.",
            Self::Changed => "The file changed while it was being read. Retry the preview.",
            Self::Cancelled => "The preview request was cancelled.",
            Self::ProviderUnavailable => "No text preview provider is available for this file.",
            Self::ProviderPanicked => "The text preview provider stopped unexpectedly.",
            Self::SchedulerUnavailable => "The preview queue is busy. Retry in a moment.",
            Self::WorkerBusy => "Waiting for the previous preview read to finish.",
            Self::TimedOut => "The text preview provider exceeded its time budget.",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TextPreviewPoll {
    Current,
    Loading,
    Ready(Arc<str>),
    Failed(PreviewError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextPreviewLoadState {
    Idle,
    Debouncing,
    Waiting,
    Loading,
    Ready,
    Failed(PreviewError),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TextPreviewFingerprint {
    path: PathBuf,
    volume: Option<u64>,
    file_id: Option<u64>,
    size: u64,
    modified: Option<SystemTime>,
    change_seconds: Option<i64>,
    change_nanoseconds: Option<i64>,
}

impl TextPreviewFingerprint {
    fn from_metadata(path: &Path, metadata: &std::fs::Metadata) -> Result<Self, PreviewError> {
        if !metadata.file_type().is_file() {
            return Err(PreviewError::NotRegular);
        }
        #[cfg(unix)]
        let (volume, file_id, change_seconds, change_nanoseconds) = {
            use std::os::unix::fs::MetadataExt;
            (
                Some(metadata.dev()),
                Some(metadata.ino()),
                Some(metadata.ctime()),
                Some(metadata.ctime_nsec()),
            )
        };
        #[cfg(not(unix))]
        let (volume, file_id, change_seconds, change_nanoseconds) = (None, None, None, None);
        Ok(Self {
            path: path.to_path_buf(),
            volume,
            file_id,
            size: metadata.len(),
            modified: metadata.modified().ok(),
            change_seconds,
            change_nanoseconds,
        })
    }

    const fn has_stable_change_token(&self) -> bool {
        self.change_seconds.is_some() && self.change_nanoseconds.is_some()
    }

    fn matches_request(&self, identity: &crate::panel::PreviewIdentity) -> bool {
        self.path == identity.path
            && self.size == identity.size
            && identity
                .modified
                .is_none_or(|modified| self.modified == Some(modified))
    }
}

struct TextPreviewEntry {
    content: Arc<str>,
    byte_size: usize,
    last_used: u64,
}

struct TextPreviewStore {
    entries: HashMap<TextPreviewFingerprint, TextPreviewEntry>,
    total_bytes: usize,
    max_bytes: usize,
    max_entries: usize,
    tick: u64,
}

impl TextPreviewStore {
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            max_bytes,
            max_entries,
            tick: 0,
        }
    }

    fn get(&mut self, key: &TextPreviewFingerprint) -> Option<Arc<str>> {
        self.tick = self.tick.saturating_add(1);
        let entry = self.entries.get_mut(key)?;
        entry.last_used = self.tick;
        Some(Arc::clone(&entry.content))
    }

    fn insert(&mut self, key: TextPreviewFingerprint, content: Arc<str>) {
        self.tick = self.tick.saturating_add(1);
        let old_versions: Vec<_> = self
            .entries
            .keys()
            .filter(|candidate| candidate.path == key.path && *candidate != &key)
            .cloned()
            .collect();
        for old in old_versions {
            self.remove(&old);
        }
        self.remove(&key);

        let byte_size = content.len();
        self.total_bytes = self.total_bytes.saturating_add(byte_size);
        self.entries.insert(
            key,
            TextPreviewEntry {
                content,
                byte_size,
                last_used: self.tick,
            },
        );
        self.evict_to_limits();
    }

    fn remove_path(&mut self, path: &Path) {
        let keys: Vec<_> = self
            .entries
            .keys()
            .filter(|key| key.path == path)
            .cloned()
            .collect();
        for key in keys {
            self.remove(&key);
        }
    }

    fn remove(&mut self, key: &TextPreviewFingerprint) {
        if let Some(entry) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
        }
    }

    fn evict_to_limits(&mut self) {
        while self.entries.len() > self.max_entries || self.total_bytes > self.max_bytes {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                break;
            };
            self.remove(&oldest);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextPreviewOwnership {
    identity: crate::panel::PreviewIdentity,
}

struct TextPreviewSuccess {
    fingerprint: TextPreviewFingerprint,
    content: Arc<str>,
}

struct TextPreviewCompletion {
    generation: u64,
    result: Result<TextPreviewSuccess, PreviewError>,
}

enum TextPreviewPhase {
    Idle,
    Debouncing { ready_at: Instant },
    WaitingForWorker,
    Running { deadline: Instant },
    Ready(Arc<str>),
    Failed(PreviewError),
}

trait TextPreviewClock: Send + Sync {
    fn now(&self) -> Instant;
}

struct SystemTextPreviewClock;

impl TextPreviewClock for SystemTextPreviewClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

trait TextPreviewProvider: Send + Sync {
    fn supports(&self, _identity: &crate::panel::PreviewIdentity) -> bool {
        true
    }

    fn fingerprint(
        &self,
        identity: &crate::panel::PreviewIdentity,
    ) -> Result<TextPreviewFingerprint, PreviewError>;

    fn read(
        &self,
        identity: &crate::panel::PreviewIdentity,
        expected: &TextPreviewFingerprint,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Arc<str>, PreviewError>;
}

#[derive(Default)]
struct FsTextPreviewProvider {
    chunk_observer: Option<Arc<dyn Fn(usize) + Send + Sync>>,
}

fn read_preview_bytes(
    reader: &mut impl Read,
    reserve_bytes: usize,
    cancelled: &dyn Fn() -> bool,
    chunk_observer: Option<&(dyn Fn(usize) + Send + Sync)>,
) -> Result<Vec<u8>, PreviewError> {
    let max_bytes = MAX_TEXT_PREVIEW_BYTES as usize;
    let mut bytes = Vec::new();
    bytes
        .try_reserve(reserve_bytes.min(max_bytes))
        .map_err(|_| PreviewError::TooLarge)?;
    let mut chunk = [0u8; TEXT_PREVIEW_CHUNK_BYTES];
    let mut chunks_read = 0usize;
    loop {
        if cancelled() || !crate::io_budget::background_checkpoint(cancelled) {
            return Err(PreviewError::Cancelled);
        }
        let count = reader
            .read(&mut chunk)
            .map_err(|_| PreviewError::Unreadable)?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) > max_bytes {
            return Err(PreviewError::TooLarge);
        }
        bytes.extend_from_slice(&chunk[..count]);
        chunks_read = chunks_read.saturating_add(1);
        if let Some(observer) = chunk_observer {
            observer(chunks_read);
        }
    }
    Ok(bytes)
}

impl TextPreviewProvider for FsTextPreviewProvider {
    fn supports(&self, identity: &crate::panel::PreviewIdentity) -> bool {
        let extension = identity
            .path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        crate::provider_runtime::activate_builtin(
            "native-preview",
            &crate::provider_runtime::ActivationRequest {
                capability: crate::provider_runtime::ProviderCapability::PreviewText,
                root: identity.path.parent().unwrap_or(Path::new("/")),
                extension: extension.as_deref(),
                bytes: Some(identity.size),
            },
        )
    }

    fn fingerprint(
        &self,
        identity: &crate::panel::PreviewIdentity,
    ) -> Result<TextPreviewFingerprint, PreviewError> {
        let metadata =
            std::fs::symlink_metadata(&identity.path).map_err(|_| PreviewError::Unreadable)?;
        TextPreviewFingerprint::from_metadata(&identity.path, &metadata)
    }

    fn read(
        &self,
        identity: &crate::panel::PreviewIdentity,
        expected: &TextPreviewFingerprint,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Arc<str>, PreviewError> {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let before = self.fingerprint(identity)?;
        if &before != expected {
            return Err(PreviewError::Changed);
        }

        let mut file = std::fs::File::open(&identity.path).map_err(|_| PreviewError::Unreadable)?;
        let opened = file
            .metadata()
            .map_err(|_| PreviewError::Unreadable)
            .and_then(|metadata| {
                TextPreviewFingerprint::from_metadata(&identity.path, &metadata)
            })?;
        if &opened != expected {
            return Err(PreviewError::Changed);
        }

        let bytes = read_preview_bytes(
            &mut file,
            expected.size as usize,
            cancelled,
            self.chunk_observer.as_deref(),
        )?;

        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let after_handle = file
            .metadata()
            .map_err(|_| PreviewError::Unreadable)
            .and_then(|metadata| {
                TextPreviewFingerprint::from_metadata(&identity.path, &metadata)
            })?;
        let after_path = self.fingerprint(identity)?;
        if &after_handle != expected || &after_path != expected {
            return Err(PreviewError::Changed);
        }

        // Platforms without a metadata change token do not use the text cache
        // and verify a second complete snapshot before publishing the first.
        #[cfg(not(unix))]
        {
            let mut verification_file =
                std::fs::File::open(&identity.path).map_err(|_| PreviewError::Unreadable)?;
            let verification_opened = verification_file
                .metadata()
                .map_err(|_| PreviewError::Unreadable)
                .and_then(|metadata| {
                    TextPreviewFingerprint::from_metadata(&identity.path, &metadata)
                })?;
            if &verification_opened != expected {
                return Err(PreviewError::Changed);
            }
            let verification = read_preview_bytes(
                &mut verification_file,
                expected.size as usize,
                cancelled,
                None,
            )?;
            let verification_after = self.fingerprint(identity)?;
            if &verification_after != expected || verification != bytes {
                return Err(PreviewError::Changed);
            }
        }
        decode_text_bytes(bytes)
    }
}

fn decode_text_bytes(mut bytes: Vec<u8>) -> Result<Arc<str>, PreviewError> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return Err(PreviewError::UnsupportedEncoding);
    }
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        bytes.drain(..3);
    }
    if bytes.contains(&0) {
        return Err(PreviewError::Binary);
    }
    let controls = bytes
        .iter()
        .filter(|byte| {
            let byte = **byte;
            (byte < 0x20 && !matches!(byte, b'\n' | b'\r' | b'\t' | 0x08 | 0x0c)) || byte == 0x7f
        })
        .count();
    if !bytes.is_empty() && controls.saturating_mul(20) > bytes.len() {
        return Err(PreviewError::Binary);
    }
    String::from_utf8(bytes)
        .map(Arc::from)
        .map_err(|_| PreviewError::InvalidUtf8)
}

trait TextPreviewTaskHandle {
    fn cancel(&self) -> bool;

    fn is_finished(&self) -> bool;
}

type TextPreviewJob = Box<dyn FnOnce(Arc<dyn Fn() -> bool + Send + Sync>) + Send + 'static>;

struct TextPreviewSubmission {
    generation: u64,
}

trait TextPreviewExecutor: Send + Sync {
    type Handle: TextPreviewTaskHandle;

    fn submit(
        &self,
        submission: TextPreviewSubmission,
        job: TextPreviewJob,
    ) -> Result<Self::Handle, PreviewError>;
}

#[derive(Default)]
struct IsolatedTextPreviewExecutor {
    active: Arc<AtomicBool>,
}

struct IsolatedTextPreviewTaskHandle {
    cancelled: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

impl TextPreviewTaskHandle for IsolatedTextPreviewTaskHandle {
    fn cancel(&self) -> bool {
        !self.cancelled.swap(true, Ordering::AcqRel)
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

impl TextPreviewExecutor for IsolatedTextPreviewExecutor {
    type Handle = IsolatedTextPreviewTaskHandle;

    fn submit(
        &self,
        submission: TextPreviewSubmission,
        job: TextPreviewJob,
    ) -> Result<Self::Handle, PreviewError> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| PreviewError::WorkerBusy)?;

        let cancelled = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_finished = Arc::clone(&finished);
        let active = Arc::clone(&self.active);
        let spawn = std::thread::Builder::new()
            .name(format!("text-preview-{}", submission.generation))
            .spawn(move || {
                let probe: Arc<dyn Fn() -> bool + Send + Sync> =
                    Arc::new(move || worker_cancelled.load(Ordering::Acquire));
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(probe)));
                active.store(false, Ordering::Release);
                worker_finished.store(true, Ordering::Release);
            });
        if spawn.is_err() {
            self.active.store(false, Ordering::Release);
            return Err(PreviewError::SchedulerUnavailable);
        }
        Ok(IsolatedTextPreviewTaskHandle {
            cancelled,
            finished,
        })
    }
}

struct RunningTextPreviewTask<H> {
    generation: u64,
    handle: H,
}

struct TextPreviewPipeline<E: TextPreviewExecutor> {
    executor: E,
    provider: Arc<dyn TextPreviewProvider>,
    clock: Arc<dyn TextPreviewClock>,
    store: Arc<Mutex<TextPreviewStore>>,
    generation: Arc<AtomicU64>,
    active: Option<TextPreviewOwnership>,
    phase: TextPreviewPhase,
    task: Option<RunningTextPreviewTask<E::Handle>>,
    delivered: bool,
    sender: Sender<TextPreviewCompletion>,
    receiver: Receiver<TextPreviewCompletion>,
}

impl TextPreviewPipeline<IsolatedTextPreviewExecutor> {
    fn production() -> Self {
        Self::with_parts(
            IsolatedTextPreviewExecutor::default(),
            Arc::new(FsTextPreviewProvider::default()),
            Arc::new(SystemTextPreviewClock),
        )
    }
}

impl<E: TextPreviewExecutor> TextPreviewPipeline<E> {
    fn with_parts(
        executor: E,
        provider: Arc<dyn TextPreviewProvider>,
        clock: Arc<dyn TextPreviewClock>,
    ) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            executor,
            provider,
            clock,
            store: Arc::new(Mutex::new(TextPreviewStore::new(
                MAX_TEXT_PREVIEW_CACHE_ENTRIES,
                MAX_TEXT_PREVIEW_CACHE_BYTES,
            ))),
            generation: Arc::new(AtomicU64::new(0)),
            active: None,
            phase: TextPreviewPhase::Idle,
            task: None,
            delivered: false,
            sender,
            receiver,
        }
    }

    fn advance_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn cancel_task(&mut self) {
        if let Some(task) = &self.task {
            task.handle.cancel();
        }
    }

    fn begin_request(&mut self, ownership: TextPreviewOwnership, now: Instant) {
        self.cancel_task();
        self.advance_generation();
        self.active = Some(ownership.clone());
        self.delivered = false;
        self.phase = if ownership.identity.size > MAX_TEXT_PREVIEW_BYTES {
            TextPreviewPhase::Failed(PreviewError::TooLarge)
        } else {
            TextPreviewPhase::Debouncing {
                ready_at: now + TEXT_PREVIEW_DEBOUNCE,
            }
        };
    }

    fn request(
        &mut self,
        ctx: &Context,
        identity: &crate::panel::PreviewIdentity,
    ) -> TextPreviewPoll {
        let now = self.clock.now();
        self.drain_completions();
        self.reap_finished_without_completion();
        let ownership = TextPreviewOwnership {
            identity: identity.clone(),
        };
        if self.active.as_ref() != Some(&ownership) {
            self.begin_request(ownership, now);
        }

        self.expire_timeout(now);
        let ready_to_submit = matches!(
            self.phase,
            TextPreviewPhase::Debouncing { ready_at } if now >= ready_at
        ) || matches!(self.phase, TextPreviewPhase::WaitingForWorker)
            && self.task.is_none();
        if ready_to_submit {
            self.submit(ctx, now);
        }
        self.schedule_repaint(ctx, now);

        if self.delivered {
            return TextPreviewPoll::Current;
        }
        match &self.phase {
            TextPreviewPhase::Idle
            | TextPreviewPhase::Debouncing { .. }
            | TextPreviewPhase::WaitingForWorker
            | TextPreviewPhase::Running { .. } => TextPreviewPoll::Loading,
            TextPreviewPhase::Ready(content) => {
                self.delivered = true;
                TextPreviewPoll::Ready(Arc::clone(content))
            }
            TextPreviewPhase::Failed(error) => {
                self.delivered = true;
                TextPreviewPoll::Failed(*error)
            }
        }
    }

    fn submit(&mut self, ctx: &Context, now: Instant) {
        if self.task.is_some() {
            self.phase = TextPreviewPhase::WaitingForWorker;
            self.delivered = false;
            return;
        }
        let Some(ownership) = self.active.clone() else {
            return;
        };
        let generation = self.generation.load(Ordering::Acquire);
        let active_generation = Arc::clone(&self.generation);
        let provider = Arc::clone(&self.provider);
        let store = Arc::clone(&self.store);
        let sender = self.sender.clone();
        let repaint = ctx.clone();
        let identity = ownership.identity;
        let job: TextPreviewJob = Box::new(move |scheduler_cancel| {
            let cancelled =
                || scheduler_cancel() || active_generation.load(Ordering::Acquire) != generation;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if cancelled() {
                    return Err(PreviewError::Cancelled);
                }
                if !provider.supports(&identity) {
                    return Err(PreviewError::ProviderUnavailable);
                }
                let fingerprint = provider.fingerprint(&identity)?;
                if !fingerprint.matches_request(&identity) {
                    return Err(PreviewError::Changed);
                }
                if fingerprint.has_stable_change_token()
                    && let Some(content) = crate::lock_util::recover(&store).get(&fingerprint)
                {
                    return Ok(TextPreviewSuccess {
                        fingerprint,
                        content,
                    });
                }
                let content = provider.read(&identity, &fingerprint, &cancelled)?;
                Ok(TextPreviewSuccess {
                    fingerprint,
                    content,
                })
            }))
            .unwrap_or(Err(PreviewError::ProviderPanicked));

            let result = if cancelled() {
                Err(PreviewError::Cancelled)
            } else {
                result
            };
            let _ = sender.send(TextPreviewCompletion { generation, result });
            repaint.request_repaint();
        });

        self.phase = TextPreviewPhase::Running {
            deadline: now + TEXT_PREVIEW_TIMEOUT,
        };
        match self
            .executor
            .submit(TextPreviewSubmission { generation }, job)
        {
            Ok(handle) => {
                self.task = Some(RunningTextPreviewTask { generation, handle });
            }
            Err(PreviewError::WorkerBusy) => {
                self.phase = TextPreviewPhase::WaitingForWorker;
                self.delivered = false;
            }
            Err(error) => {
                self.phase = TextPreviewPhase::Failed(error);
                self.delivered = false;
            }
        }
    }

    fn drain_completions(&mut self) {
        while let Ok(completion) = self.receiver.try_recv() {
            if self
                .task
                .as_ref()
                .is_some_and(|task| task.generation == completion.generation)
            {
                self.task = None;
            }
            if completion.generation != self.generation.load(Ordering::Acquire) {
                continue;
            }
            self.delivered = false;
            self.phase = match completion.result {
                Ok(success) => {
                    if success.fingerprint.has_stable_change_token() {
                        crate::lock_util::recover(&self.store)
                            .insert(success.fingerprint, Arc::clone(&success.content));
                    }
                    TextPreviewPhase::Ready(success.content)
                }
                Err(error) => TextPreviewPhase::Failed(error),
            };
        }
    }

    fn reap_finished_without_completion(&mut self) {
        let Some(generation) = self
            .task
            .as_ref()
            .filter(|task| task.handle.is_finished())
            .map(|task| task.generation)
        else {
            return;
        };
        // A normal worker sends before publishing `finished`; drain once more
        // after the acquire load before treating a missing completion as panic.
        self.drain_completions();
        if self
            .task
            .as_ref()
            .is_some_and(|task| task.generation == generation && task.handle.is_finished())
        {
            self.task = None;
            if generation == self.generation.load(Ordering::Acquire) {
                self.phase = TextPreviewPhase::Failed(PreviewError::ProviderPanicked);
                self.delivered = false;
            }
        }
    }

    fn expire_timeout(&mut self, now: Instant) {
        let timed_out = matches!(
            self.phase,
            TextPreviewPhase::Running { deadline } if now >= deadline
        );
        if timed_out {
            self.cancel_task();
            self.advance_generation();
            self.phase = TextPreviewPhase::Failed(PreviewError::TimedOut);
            self.delivered = false;
        }
    }

    fn schedule_repaint(&self, ctx: &Context, now: Instant) {
        match self.phase {
            TextPreviewPhase::Debouncing { ready_at } => {
                ctx.request_repaint_after(ready_at.saturating_duration_since(now));
            }
            TextPreviewPhase::WaitingForWorker => {
                ctx.request_repaint_after(TEXT_PREVIEW_POLL_INTERVAL);
            }
            TextPreviewPhase::Running { deadline } => {
                ctx.request_repaint_after(
                    TEXT_PREVIEW_POLL_INTERVAL.min(deadline.saturating_duration_since(now)),
                );
            }
            TextPreviewPhase::Idle | TextPreviewPhase::Ready(_) | TextPreviewPhase::Failed(_) => {}
        }
    }

    fn load_state(&self, path: &Path) -> TextPreviewLoadState {
        let Some(active) = self
            .active
            .as_ref()
            .filter(|active| active.identity.path == path)
        else {
            return TextPreviewLoadState::Idle;
        };
        let _ = active;
        match self.phase {
            TextPreviewPhase::Idle => TextPreviewLoadState::Idle,
            TextPreviewPhase::Debouncing { .. } => TextPreviewLoadState::Debouncing,
            TextPreviewPhase::WaitingForWorker => TextPreviewLoadState::Waiting,
            TextPreviewPhase::Running { .. } => TextPreviewLoadState::Loading,
            TextPreviewPhase::Ready(_) => TextPreviewLoadState::Ready,
            TextPreviewPhase::Failed(error) => TextPreviewLoadState::Failed(error),
        }
    }

    fn retry(&mut self, path: &Path) {
        crate::lock_util::recover(&self.store).remove_path(path);
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.identity.path == path)
        {
            self.cancel_task();
            self.advance_generation();
            self.phase = if self
                .active
                .as_ref()
                .is_some_and(|active| active.identity.size > MAX_TEXT_PREVIEW_BYTES)
            {
                TextPreviewPhase::Failed(PreviewError::TooLarge)
            } else {
                TextPreviewPhase::Debouncing {
                    ready_at: self.clock.now() + TEXT_PREVIEW_DEBOUNCE,
                }
            };
            self.delivered = false;
        }
    }

    fn cancel(&mut self) {
        if self.active.is_some() || self.task.is_some() {
            self.cancel_task();
            self.advance_generation();
        }
        self.active = None;
        self.phase = TextPreviewPhase::Idle;
        self.delivered = false;
    }

    fn cancel_if_active(&mut self, identity: &crate::panel::PreviewIdentity) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.identity == *identity)
        {
            self.cancel();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewFailure {
    TooLarge,
    Unreadable,
    Unsupported,
    TimedOut,
    Busy,
    Decode,
}

impl PreviewFailure {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "This image exceeds the preview memory limit.",
            Self::Unreadable => "Commander cannot read this file.",
            Self::Unsupported => "This image format is not supported.",
            Self::TimedOut => "The preview provider exceeded its time budget.",
            Self::Busy => "All preview decoders are busy. Retry in a moment.",
            Self::Decode => "The image is incomplete or damaged.",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProviderTally {
    pub attempts: u64,
    pub successes: u64,
    pub failures: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PreviewProviderStats {
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
pub(crate) enum PreviewLoadState {
    Disabled,
    Loading,
    Failed(PreviewFailure),
    Idle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ImageCacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub pending: usize,
    pub failed: usize,
    pub providers: PreviewProviderStats,
}

pub struct ImageCache {
    entries: HashMap<PathBuf, CacheEntry>,
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

    /// Request preloading in priority order, with fixed worker concurrency.
    pub fn preload(&mut self, ctx: &Context, paths: &[PathBuf], dir: &Path) {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ImagePreview) {
            let mut pending = crate::lock_util::recover(&self.pending);
            if !pending.is_empty() {
                pending.clear();
                self.generation.fetch_add(1, Ordering::AcqRel);
            }
            drop(pending);
            self.entries.clear();
            self.total_bytes = 0;
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
            self.entries.clear();
            self.total_bytes = 0;
            self.generation.fetch_add(1, Ordering::AcqRel);
            crate::lock_util::recover(&self.pending).clear();
            crate::lock_util::recover(&self.failed).clear();
        }

        // Publish finished decodes before calculating free worker slots.
        {
            let mut pending = crate::lock_util::recover(&self.pending);
            let completed: Vec<_> = pending
                .iter()
                .filter(|(_, value)| value.is_some())
                .map(|(path, _)| path.clone())
                .collect();
            for path in completed {
                if let Some(Some(decoded)) = pending.remove(&path) {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let texture = ctx.load_texture(&name, decoded.image, TextureOptions::LINEAR);
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
            }
        }

        let target = self.preview_target;
        let failed = crate::lock_util::recover(&self.failed).clone();
        let mut available_slots =
            MAX_PRELOAD_WORKERS.saturating_sub(crate::lock_util::recover(&self.pending).len());
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
            if let Some(entry) = self.entries.remove(path) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
            }
            if available_slots == 0 || failed.contains_key(path) {
                continue;
            }
            {
                let mut pending = crate::lock_util::recover(&self.pending);
                if pending.contains_key(path) {
                    continue;
                }
                pending.insert(path.clone(), None);
            }
            let path_clone = path.clone();
            let pending_clone = Arc::clone(&self.pending);
            let failed_clone = Arc::clone(&self.failed);
            let active_generation = Arc::clone(&self.generation);
            let generation = active_generation.load(Ordering::Acquire);
            let ctx_clone = ctx.clone();
            let task = crate::workload::submit(
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
                        crate::lock_util::recover(&pending_clone).remove(&path_clone);
                        return;
                    }
                    let result = load_image_with_timeout(&path_clone, target);
                    match result {
                        Ok(decoded) => {
                            let mut pending = crate::lock_util::recover(&pending_clone);
                            if !cancelled() {
                                pending.insert(path_clone.clone(), Some(decoded));
                                drop(pending);
                                ctx_clone.request_repaint();
                            } else {
                                pending.remove(&path_clone);
                            }
                        }
                        Err(error) => {
                            let mut pending = crate::lock_util::recover(&pending_clone);
                            if !cancelled() {
                                pending.remove(&path_clone);
                                drop(pending);
                                crate::lock_util::recover(&failed_clone)
                                    .insert(path_clone, classify_failure(&error));
                                ctx_clone.request_repaint();
                            }
                        }
                    }
                },
            );
            if task.is_ok() {
                available_slots -= 1;
            } else {
                crate::lock_util::recover(&self.pending).remove(path);
            }
        }

        // Evict entries not in the keep set and over budget
        let keep_set: std::collections::HashSet<&PathBuf> = paths.iter().collect();
        self.evict(&keep_set);
    }

    fn evict(&mut self, keep: &std::collections::HashSet<&PathBuf>) {
        if self.total_bytes <= MAX_CACHE_BYTES {
            return; // Under budget — keep everything
        }

        // Over budget — first remove entries from other directories (not in keep set)
        let to_remove: Vec<PathBuf> = self
            .entries
            .keys()
            .filter(|k| !keep.contains(k))
            .cloned()
            .collect();
        for path in to_remove {
            if let Some(entry) = self.entries.remove(&path) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
            }
        }

        // Still over budget — evict oldest from current set
        while self.total_bytes > MAX_CACHE_BYTES && !self.entries.is_empty() {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            if let Some(path) = oldest
                && let Some(entry) = self.entries.remove(&path)
            {
                self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
            }
        }
    }

    pub fn load_state(&self, path: &Path) -> PreviewLoadState {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ImagePreview) {
            return PreviewLoadState::Disabled;
        }
        if let Some(failure) = crate::lock_util::recover(&self.failed).get(path).copied() {
            return PreviewLoadState::Failed(failure);
        }
        if crate::lock_util::recover(&self.pending).contains_key(path) {
            PreviewLoadState::Loading
        } else {
            PreviewLoadState::Idle
        }
    }

    /// Clear a negative cache entry so the next preload pass can try again.
    pub fn retry(&mut self, path: &Path) {
        crate::lock_util::recover(&self.failed).remove(path);
        crate::lock_util::recover(&self.pending).remove(path);
        if let Some(entry) = self.entries.remove(path) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
        }
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

fn classify_failure(error: &str) -> PreviewFailure {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("still busy") || normalized.contains("decoder capacity") {
        PreviewFailure::Busy
    } else if normalized.contains("timed out") || normalized.contains("time budget") {
        PreviewFailure::TimedOut
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

#[cfg(target_os = "macos")]
fn is_video_ext(path: &Path) -> bool {
    matches!(
        path.extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .as_deref(),
        Some("mp4" | "mov" | "avi" | "mkv" | "webm" | "m4v" | "wmv" | "flv")
    )
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
    let path = path.to_path_buf();
    run_decode_with_timeout(DECODE_TIMEOUT, move || load_image_from_disk(&path, target))
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
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    img.apply_orientation(orientation);
    let img = if img.width() > target.width || img.height() > target.height {
        img.thumbnail(target.width, target.height)
    } else {
        img
    };
    let rgba = img.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();
    let (image, byte_size) = color_image_from_rgba(size, pixels)?;
    Ok(DecodedPreview {
        image,
        byte_size,
        decoded_for: target,
    })
}

/// Load image via macOS CoreGraphics/ImageIO.
/// Supports: DNG, CR2, NEF, ARW, ORF, RAF, RW2, HEIC, TIFF, and all standard formats.
#[cfg(target_os = "macos")]
fn load_via_imageio(path: &Path, target: PreviewTarget) -> Result<DecodedPreview, String> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CString;

    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];

        // Create CFURL from path
        let path_str = path.to_str().ok_or("invalid path")?;
        let c_path = CString::new(path_str).map_err(|e| e.to_string())?;
        let cf_str: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: c_path.as_ptr()];
        if cf_str.is_null() {
            let _: () = msg_send![pool, drain];
            return Err("failed to create NSString".into());
        }
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: cf_str];
        if url.is_null() {
            let _: () = msg_send![pool, drain];
            return Err("failed to create NSURL".into());
        }

        // CGImageSourceCreateWithURL
        type CGImageSourceRef = *mut Object;
        unsafe extern "C" {
            fn CGImageSourceCreateWithURL(
                url: *mut Object,
                options: *mut Object,
            ) -> CGImageSourceRef;
            fn CGImageSourceCreateThumbnailAtIndex(
                source: CGImageSourceRef,
                index: usize,
                options: *mut Object,
            ) -> *mut Object;
            fn CGImageGetWidth(image: *mut Object) -> usize;
            fn CGImageGetHeight(image: *mut Object) -> usize;
            fn CGColorSpaceCreateDeviceRGB() -> *mut Object;
            fn CGBitmapContextCreate(
                data: *mut u8,
                width: usize,
                height: usize,
                bits_per_component: usize,
                bytes_per_row: usize,
                space: *mut Object,
                bitmap_info: u32,
            ) -> *mut Object;
            fn CGContextDrawImage(ctx: *mut Object, rect: CGRect, image: *mut Object);
            fn CGContextRelease(ctx: *mut Object);
            fn CGColorSpaceRelease(space: *mut Object);
            fn CGImageRelease(image: *mut Object);
            fn CFRelease(cf: *mut Object);
        }

        #[repr(C)]
        struct CGRect {
            x: f64,
            y: f64,
            w: f64,
            h: f64,
        }

        let source = CGImageSourceCreateWithURL(url, std::ptr::null_mut());
        if source.is_null() {
            let _: () = msg_send![pool, drain];
            return Err("CGImageSourceCreateWithURL failed".into());
        }

        let options: *mut Object = msg_send![class!(NSMutableDictionary), dictionary];
        let always_key = CString::new("kCGImageSourceCreateThumbnailFromImageAlways").unwrap();
        let transform_key = CString::new("kCGImageSourceCreateThumbnailWithTransform").unwrap();
        let size_key = CString::new("kCGImageSourceThumbnailMaxPixelSize").unwrap();
        let always_key: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: always_key.as_ptr()];
        let transform_key: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: transform_key.as_ptr()];
        let size_key: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: size_key.as_ptr()];
        let yes: *mut Object = msg_send![class!(NSNumber), numberWithBool: 1_i8];
        let max_size: *mut Object = msg_send![
            class!(NSNumber),
            numberWithUnsignedLongLong: u64::from(target.max_dimension())
        ];
        if options.is_null()
            || always_key.is_null()
            || transform_key.is_null()
            || size_key.is_null()
            || yes.is_null()
            || max_size.is_null()
        {
            CFRelease(source);
            let _: () = msg_send![pool, drain];
            return Err("ImageIO thumbnail options could not be created".into());
        }
        let _: () = msg_send![options, setObject: yes forKey: always_key];
        let _: () = msg_send![options, setObject: yes forKey: transform_key];
        let _: () = msg_send![options, setObject: max_size forKey: size_key];

        let cg_image = CGImageSourceCreateThumbnailAtIndex(source, 0, options);
        if cg_image.is_null() {
            CFRelease(source);
            let _: () = msg_send![pool, drain];
            return Err("CGImageSourceCreateThumbnailAtIndex failed".into());
        }

        let w = CGImageGetWidth(cg_image);
        let h = CGImageGetHeight(cg_image);
        if w == 0 || h == 0 {
            CGImageRelease(cg_image);
            CFRelease(source);
            let _: () = msg_send![pool, drain];
            return Err("image has zero dimensions".into());
        }

        // Draw into RGBA bitmap context
        let (bytes_per_row, mut pixels) = match allocate_rgba_pixels(w, h) {
            Ok(layout) => layout,
            Err(error) => {
                CGImageRelease(cg_image);
                CFRelease(source);
                let _: () = msg_send![pool, drain];
                return Err(error);
            }
        };
        let color_space = CGColorSpaceCreateDeviceRGB();
        if color_space.is_null() {
            CGImageRelease(cg_image);
            CFRelease(source);
            let _: () = msg_send![pool, drain];
            return Err("CGColorSpaceCreateDeviceRGB failed".into());
        }
        // kCGImageAlphaPremultipliedLast = 1
        let bitmap_info: u32 = 1;
        let cg_ctx = CGBitmapContextCreate(
            pixels.as_mut_ptr(),
            w,
            h,
            8,
            bytes_per_row,
            color_space,
            bitmap_info,
        );

        if cg_ctx.is_null() {
            CGColorSpaceRelease(color_space);
            CGImageRelease(cg_image);
            CFRelease(source);
            let _: () = msg_send![pool, drain];
            return Err("CGBitmapContextCreate failed".into());
        }

        let rect = CGRect {
            x: 0.0,
            y: 0.0,
            w: w as f64,
            h: h as f64,
        };
        CGContextDrawImage(cg_ctx, rect, cg_image);

        // Unpremultiply alpha (premultiplied → straight)
        for chunk in pixels.chunks_exact_mut(4) {
            let a = chunk[3] as u16;
            if a > 0 && a < 255 {
                chunk[0] = ((chunk[0] as u16 * 255) / a).min(255) as u8;
                chunk[1] = ((chunk[1] as u16 * 255) / a).min(255) as u8;
                chunk[2] = ((chunk[2] as u16 * 255) / a).min(255) as u8;
            }
        }

        CGContextRelease(cg_ctx);
        CGColorSpaceRelease(color_space);
        CGImageRelease(cg_image);
        CFRelease(source);
        let _: () = msg_send![pool, drain];

        let (image, byte_size) = color_image_from_rgba([w, h], pixels)?;
        Ok(DecodedPreview {
            image,
            byte_size,
            decoded_for: target,
        })
    }
}

// Link AVFoundation/CoreMedia for video thumbnail extraction.
#[cfg(target_os = "macos")]
#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

#[cfg(target_os = "macos")]
#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {}

/// Extract first frame from video via AVFoundation's AVAssetImageGenerator.
#[cfg(target_os = "macos")]
fn load_video_thumbnail(path: &Path, target: PreviewTarget) -> Result<DecodedPreview, String> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CString;

    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];

        // NSURL
        let path_str = path.to_str().ok_or("invalid path")?;
        let c_path = CString::new(path_str).map_err(|e| e.to_string())?;
        let ns_str: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: c_path.as_ptr()];
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: ns_str];
        if url.is_null() {
            let _: () = msg_send![pool, drain];
            return Err("failed to create NSURL".into());
        }

        // AVAsset
        let asset: *mut Object = msg_send![class!(AVAsset), assetWithURL: url];
        if asset.is_null() {
            let _: () = msg_send![pool, drain];
            return Err("AVAsset creation failed".into());
        }

        // AVAssetImageGenerator
        let generator: *mut Object = msg_send![class!(AVAssetImageGenerator), alloc];
        let generator: *mut Object = msg_send![generator, initWithAsset: asset];
        if generator.is_null() {
            let _: () = msg_send![pool, drain];
            return Err("AVAssetImageGenerator creation failed".into());
        }

        // appliesPreferredTrackTransform = YES (correct rotation)
        let _: () = msg_send![generator, setAppliesPreferredTrackTransform: true];

        #[repr(C)]
        struct CGSize {
            width: f64,
            height: f64,
        }
        let maximum_size = CGSize {
            width: f64::from(target.width),
            height: f64::from(target.height),
        };
        let _: () = msg_send![generator, setMaximumSize: maximum_size];

        // CMTime for 1 second (or 0 if short video)
        // CMTimeMake(1, 1) = 1 second
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct CMTime {
            value: i64,
            timescale: i32,
            flags: u32,
            epoch: i64,
        }
        let time = CMTime {
            value: 1,
            timescale: 1,
            flags: 1,
            epoch: 0,
        };

        // copyCGImageAtTime:actualTime:error:
        let mut actual_time = CMTime {
            value: 0,
            timescale: 0,
            flags: 0,
            epoch: 0,
        };
        let mut error: *mut Object = std::ptr::null_mut();
        let cg_image: *mut Object = msg_send![generator,
            copyCGImageAtTime: time
            actualTime: &mut actual_time as *mut CMTime
            error: &mut error as *mut *mut Object
        ];

        let _: () = msg_send![generator, release];

        if cg_image.is_null() {
            let _: () = msg_send![pool, drain];
            return Err("copyCGImageAtTime failed".into());
        }

        // Convert CGImage to RGBA pixels (same as imageio path)
        unsafe extern "C" {
            fn CGImageGetWidth(image: *mut Object) -> usize;
            fn CGImageGetHeight(image: *mut Object) -> usize;
            fn CGColorSpaceCreateDeviceRGB() -> *mut Object;
            fn CGBitmapContextCreate(
                data: *mut u8,
                width: usize,
                height: usize,
                bits_per_component: usize,
                bytes_per_row: usize,
                space: *mut Object,
                bitmap_info: u32,
            ) -> *mut Object;
            fn CGContextDrawImage(ctx: *mut Object, rect: CGRect, image: *mut Object);
            fn CGContextRelease(ctx: *mut Object);
            fn CGColorSpaceRelease(space: *mut Object);
            fn CGImageRelease(image: *mut Object);
        }

        #[repr(C)]
        struct CGRect {
            x: f64,
            y: f64,
            w: f64,
            h: f64,
        }

        let w = CGImageGetWidth(cg_image);
        let h = CGImageGetHeight(cg_image);
        if w == 0 || h == 0 {
            CGImageRelease(cg_image);
            let _: () = msg_send![pool, drain];
            return Err("video frame has zero dimensions".into());
        }

        let (bytes_per_row, mut pixels) = match allocate_rgba_pixels(w, h) {
            Ok(layout) => layout,
            Err(error) => {
                CGImageRelease(cg_image);
                let _: () = msg_send![pool, drain];
                return Err(error);
            }
        };
        let color_space = CGColorSpaceCreateDeviceRGB();
        if color_space.is_null() {
            CGImageRelease(cg_image);
            let _: () = msg_send![pool, drain];
            return Err("CGColorSpaceCreateDeviceRGB failed for video".into());
        }
        let bitmap_info: u32 = 1; // kCGImageAlphaPremultipliedLast
        let cg_ctx = CGBitmapContextCreate(
            pixels.as_mut_ptr(),
            w,
            h,
            8,
            bytes_per_row,
            color_space,
            bitmap_info,
        );

        if cg_ctx.is_null() {
            CGColorSpaceRelease(color_space);
            CGImageRelease(cg_image);
            let _: () = msg_send![pool, drain];
            return Err("CGBitmapContextCreate failed for video".into());
        }

        let rect = CGRect {
            x: 0.0,
            y: 0.0,
            w: w as f64,
            h: h as f64,
        };
        CGContextDrawImage(cg_ctx, rect, cg_image);

        // Unpremultiply
        for chunk in pixels.chunks_exact_mut(4) {
            let a = chunk[3] as u16;
            if a > 0 && a < 255 {
                chunk[0] = ((chunk[0] as u16 * 255) / a).min(255) as u8;
                chunk[1] = ((chunk[1] as u16 * 255) / a).min(255) as u8;
                chunk[2] = ((chunk[2] as u16 * 255) / a).min(255) as u8;
            }
        }

        CGContextRelease(cg_ctx);
        CGColorSpaceRelease(color_space);
        CGImageRelease(cg_image);
        let _: () = msg_send![pool, drain];

        let (image, byte_size) = color_image_from_rgba([w, h], pixels)?;
        Ok(DecodedPreview {
            image,
            byte_size,
            decoded_for: target,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FsTextPreviewProvider, ImageCache, MAX_DECODED_IMAGE_BYTES, MAX_IMAGE_DIMENSION,
        MAX_TEXT_PREVIEW_BYTES, PreviewError, PreviewFailure, PreviewTarget, TEXT_PREVIEW_DEBOUNCE,
        TEXT_PREVIEW_TIMEOUT, TextPreviewClock, TextPreviewExecutor, TextPreviewFingerprint,
        TextPreviewJob, TextPreviewPipeline, TextPreviewPoll, TextPreviewStore,
        TextPreviewSubmission, TextPreviewTaskHandle, allocate_rgba_pixels, classify_failure,
        color_image_from_rgba, decode_text_bytes, load_via_image_crate,
    };
    use crate::panel::PreviewIdentity;
    use crate::testutil::TempDir;
    use std::collections::VecDeque;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::{Duration, Instant};

    type TestJobQueue = Arc<Mutex<VecDeque<(TextPreviewJob, Arc<AtomicBool>, Arc<AtomicBool>)>>>;

    #[derive(Clone)]
    struct TestClock {
        now: Arc<Mutex<Instant>>,
    }

    impl TestClock {
        fn new() -> Self {
            Self {
                now: Arc::new(Mutex::new(Instant::now())),
            }
        }

        fn advance(&self, duration: Duration) {
            let mut now = crate::lock_util::recover(&self.now);
            *now += duration;
        }
    }

    impl TextPreviewClock for TestClock {
        fn now(&self) -> Instant {
            *crate::lock_util::recover(&self.now)
        }
    }

    #[derive(Clone)]
    struct TestExecutor {
        jobs: TestJobQueue,
        fail_next: Arc<AtomicBool>,
        cancellations: Arc<AtomicUsize>,
        cancel_finishes: bool,
    }

    impl Default for TestExecutor {
        fn default() -> Self {
            Self {
                jobs: Default::default(),
                fail_next: Default::default(),
                cancellations: Default::default(),
                cancel_finishes: true,
            }
        }
    }

    impl TestExecutor {
        fn fail_next(&self) {
            self.fail_next.store(true, Ordering::Release);
        }

        fn pending_count(&self) -> usize {
            crate::lock_util::recover(&self.jobs)
                .iter()
                .filter(|(_, cancelled, _)| !cancelled.load(Ordering::Acquire))
                .count()
        }

        fn run_next(&self) -> bool {
            let Some((job, cancelled, finished)) =
                crate::lock_util::recover(&self.jobs).pop_front()
            else {
                return false;
            };
            let probe: Arc<dyn Fn() -> bool + Send + Sync> =
                Arc::new(move || cancelled.load(Ordering::Acquire));
            job(probe);
            finished.store(true, Ordering::Release);
            true
        }

        fn run_all(&self) {
            while self.run_next() {}
        }
    }

    struct TestTaskHandle {
        cancelled: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        cancellations: Arc<AtomicUsize>,
        cancel_finishes: bool,
    }

    impl TextPreviewTaskHandle for TestTaskHandle {
        fn cancel(&self) -> bool {
            if !self.cancelled.swap(true, Ordering::AcqRel) {
                self.cancellations.fetch_add(1, Ordering::Relaxed);
            }
            if self.cancel_finishes {
                // Admission-queued test jobs can model immediate cancellation.
                self.finished.store(true, Ordering::Release);
            }
            true
        }

        fn is_finished(&self) -> bool {
            self.finished.load(Ordering::Acquire)
        }
    }

    impl TextPreviewExecutor for TestExecutor {
        type Handle = TestTaskHandle;

        fn submit(
            &self,
            _submission: TextPreviewSubmission,
            job: TextPreviewJob,
        ) -> Result<Self::Handle, PreviewError> {
            if self.fail_next.swap(false, Ordering::AcqRel) {
                return Err(PreviewError::SchedulerUnavailable);
            }
            let cancelled = Arc::new(AtomicBool::new(false));
            let finished = Arc::new(AtomicBool::new(false));
            crate::lock_util::recover(&self.jobs).push_back((
                job,
                Arc::clone(&cancelled),
                Arc::clone(&finished),
            ));
            Ok(TestTaskHandle {
                cancelled,
                finished,
                cancellations: Arc::clone(&self.cancellations),
                cancel_finishes: self.cancel_finishes,
            })
        }
    }

    enum ScriptedAction {
        Text(&'static str),
        Error(PreviewError),
        Panic,
    }

    #[derive(Default)]
    struct ScriptedProvider {
        actions: Mutex<VecDeque<ScriptedAction>>,
        reads: AtomicUsize,
    }

    impl ScriptedProvider {
        fn push(&self, action: ScriptedAction) {
            crate::lock_util::recover(&self.actions).push_back(action);
        }
    }

    impl super::TextPreviewProvider for ScriptedProvider {
        fn fingerprint(
            &self,
            identity: &PreviewIdentity,
        ) -> Result<TextPreviewFingerprint, PreviewError> {
            let file_id = identity
                .path
                .as_os_str()
                .as_encoded_bytes()
                .iter()
                .fold(0u64, |hash, byte| {
                    hash.wrapping_mul(16777619) ^ u64::from(*byte)
                });
            Ok(TextPreviewFingerprint {
                path: identity.path.clone(),
                volume: Some(1),
                file_id: Some(file_id),
                size: identity.size,
                modified: identity.modified,
                change_seconds: Some(1),
                change_nanoseconds: Some(file_id as i64),
            })
        }

        fn read(
            &self,
            _identity: &PreviewIdentity,
            _expected: &TextPreviewFingerprint,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<Arc<str>, PreviewError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if cancelled() {
                return Err(PreviewError::Cancelled);
            }
            match crate::lock_util::recover(&self.actions).pop_front() {
                Some(ScriptedAction::Text(content)) => Ok(Arc::from(content)),
                Some(ScriptedAction::Error(error)) => Err(error),
                Some(ScriptedAction::Panic) => panic!("scripted provider panic"),
                None => Ok(Arc::from("default")),
            }
        }
    }

    fn identity(path: impl Into<PathBuf>, size: u64) -> PreviewIdentity {
        PreviewIdentity {
            path: path.into(),
            size,
            modified: None,
        }
    }

    fn fingerprint(path: &str, file_id: u64, size: u64) -> TextPreviewFingerprint {
        TextPreviewFingerprint {
            path: PathBuf::from(path),
            volume: Some(1),
            file_id: Some(file_id),
            size,
            modified: None,
            change_seconds: Some(1),
            change_nanoseconds: Some(file_id as i64),
        }
    }

    fn pipeline_harness() -> (
        TextPreviewPipeline<TestExecutor>,
        TestExecutor,
        Arc<ScriptedProvider>,
        TestClock,
        egui::Context,
    ) {
        let executor = TestExecutor::default();
        let provider = Arc::new(ScriptedProvider::default());
        let clock = TestClock::new();
        let pipeline = TextPreviewPipeline::with_parts(
            executor.clone(),
            provider.clone(),
            Arc::new(clock.clone()),
        );
        (
            pipeline,
            executor,
            provider,
            clock,
            egui::Context::default(),
        )
    }

    fn submit_after_debounce(
        pipeline: &mut TextPreviewPipeline<TestExecutor>,
        executor: &TestExecutor,
        clock: &TestClock,
        ctx: &egui::Context,
        identity: &PreviewIdentity,
    ) {
        assert_eq!(pipeline.request(ctx, identity), TextPreviewPoll::Loading);
        assert_eq!(executor.pending_count(), 0);
        clock.advance(TEXT_PREVIEW_DEBOUNCE);
        assert_eq!(pipeline.request(ctx, identity), TextPreviewPoll::Loading);
        assert_eq!(executor.pending_count(), 1);
    }

    #[test]
    fn rgba_allocation_checks_both_dimension_products() {
        let (row, pixels) = allocate_rgba_pixels(3, 2).unwrap();
        assert_eq!(row, 12);
        assert_eq!(pixels.len(), 24);
        assert!(allocate_rgba_pixels(usize::MAX, 1).is_err());
        assert!(allocate_rgba_pixels(usize::MAX / 4, 5).is_err());
        assert!(allocate_rgba_pixels(MAX_DECODED_IMAGE_BYTES / 4 + 1, 1).is_err());
        assert!(allocate_rgba_pixels(MAX_IMAGE_DIMENSION as usize + 1, 1).is_err());
    }

    #[test]
    fn rgba_conversion_rejects_incomplete_and_wrapping_buffers() {
        assert!(color_image_from_rgba([2, 1], vec![0; 7]).is_err());
        assert!(color_image_from_rgba([usize::MAX, 2], Vec::new()).is_err());

        let (image, bytes) = color_image_from_rgba([1, 1], vec![10, 20, 30, 255]).unwrap();
        assert_eq!(image.size, [1, 1]);
        assert_eq!(bytes, 4);
    }

    #[test]
    fn standard_decoder_streams_a_small_png() {
        let dir = TempDir::new();
        let path = dir.path().join("preview.png");
        let pixels = image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 255]));
        pixels.save(&path).unwrap();

        let decoded = load_via_image_crate(
            &path,
            PreviewTarget {
                width: 10,
                height: 10,
            },
        )
        .unwrap();
        assert_eq!(decoded.image.size, [3, 2]);
        assert_eq!(decoded.byte_size, 3 * 2 * 4);
    }

    #[test]
    fn standard_decoder_downsamples_to_preview_target() {
        let dir = TempDir::new();
        let path = dir.path().join("large-preview.png");
        let pixels = image::RgbaImage::from_pixel(120, 60, image::Rgba([12, 34, 56, 255]));
        pixels.save(&path).unwrap();

        let decoded = load_via_image_crate(
            &path,
            PreviewTarget {
                width: 30,
                height: 30,
            },
        )
        .unwrap();

        assert_eq!(decoded.image.size, [30, 15]);
        assert_eq!(decoded.byte_size, 30 * 15 * 4);
        assert!(decoded.decoded_for.covers(PreviewTarget {
            width: 20,
            height: 10,
        }));
    }

    #[test]
    fn decoder_errors_are_classified_for_stable_ui_copy() {
        assert_eq!(
            classify_failure("image pixel buffer allocation failed"),
            PreviewFailure::TooLarge
        );
        assert_eq!(
            classify_failure("Permission denied"),
            PreviewFailure::Unreadable
        );
        assert_eq!(
            classify_failure("unsupported image format"),
            PreviewFailure::Unsupported
        );
        assert_eq!(
            classify_failure("preview providers are still busy"),
            PreviewFailure::Busy
        );
        assert_eq!(
            classify_failure("preview provider timed out"),
            PreviewFailure::TimedOut
        );
        assert_eq!(classify_failure("invalid checksum"), PreviewFailure::Decode);
    }

    #[test]
    fn retry_clears_negative_and_pending_cache_entries() {
        let mut cache = ImageCache::new();
        let path = std::path::PathBuf::from("broken.png");
        crate::lock_util::recover(&cache.failed).insert(path.clone(), PreviewFailure::Decode);
        crate::lock_util::recover(&cache.pending).insert(path.clone(), None);
        assert_eq!(cache.stats().failed, 1);
        assert_eq!(cache.stats().pending, 1);

        cache.retry(&path);

        assert_eq!(cache.stats().failed, 0);
        assert_eq!(cache.stats().pending, 0);
    }

    #[test]
    fn text_preview_store_is_bounded_and_uses_lru_order() {
        let mut store = TextPreviewStore::new(2, 5);
        let first = fingerprint("first.txt", 1, 2);
        let second = fingerprint("second.txt", 2, 2);
        let third = fingerprint("third.txt", 3, 2);
        store.insert(first.clone(), Arc::from("aa"));
        store.insert(second.clone(), Arc::from("bb"));
        assert!(store.get(&first).is_some());

        store.insert(third.clone(), Arc::from("cc"));

        assert!(store.entries.contains_key(&first));
        assert!(!store.entries.contains_key(&second));
        assert!(store.entries.contains_key(&third));
        assert_eq!(store.entries.len(), 2);
        assert_eq!(store.total_bytes, 4);

        let mut byte_bounded = TextPreviewStore::new(4, 3);
        byte_bounded.insert(first.clone(), Arc::from("aa"));
        byte_bounded.insert(second.clone(), Arc::from("bb"));
        assert!(!byte_bounded.entries.contains_key(&first));
        assert!(byte_bounded.entries.contains_key(&second));
        assert_eq!(byte_bounded.total_bytes, 2);
    }

    #[test]
    fn text_preview_store_replaces_an_old_file_fingerprint() {
        let mut store = TextPreviewStore::new(4, 32);
        let old = fingerprint("same.txt", 1, 3);
        let new = fingerprint("same.txt", 2, 3);
        store.insert(old.clone(), Arc::from("old"));
        store.insert(new.clone(), Arc::from("new"));

        assert!(!store.entries.contains_key(&old));
        assert!(store.entries.contains_key(&new));
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.total_bytes, 3);
    }

    #[test]
    fn first_open_is_debounced_async_and_cache_hits_keep_arc_content() {
        let (mut pipeline, executor, provider, clock, ctx) = pipeline_harness();
        let request = identity("first.txt", 5);
        provider.push(ScriptedAction::Text("alpha"));

        submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &request);
        executor.run_next();
        let first = match pipeline.request(&ctx, &request) {
            TextPreviewPoll::Ready(content) => content,
            other => panic!("expected ready first preview, got {other:?}"),
        };
        assert_eq!(first.as_ref(), "alpha");
        assert_eq!(provider.reads.load(Ordering::Relaxed), 1);

        pipeline.cancel();
        submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &request);
        executor.run_next();
        let cached = match pipeline.request(&ctx, &request) {
            TextPreviewPoll::Ready(content) => content,
            other => panic!("expected cached preview, got {other:?}"),
        };
        assert!(Arc::ptr_eq(&first, &cached));
        assert_eq!(provider.reads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn latest_wins_cancels_a_to_b_to_a_without_queueing_stale_reads() {
        let (mut pipeline, executor, provider, clock, ctx) = pipeline_harness();
        let a = identity("a.txt", 5);
        let b = identity("b.txt", 4);
        provider.push(ScriptedAction::Text("alpha"));

        submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &a);
        assert_eq!(pipeline.request(&ctx, &b), TextPreviewPoll::Loading);
        assert_eq!(pipeline.request(&ctx, &a), TextPreviewPoll::Loading);
        assert_eq!(executor.pending_count(), 0);
        assert_eq!(executor.cancellations.load(Ordering::Relaxed), 1);

        clock.advance(TEXT_PREVIEW_DEBOUNCE);
        assert_eq!(pipeline.request(&ctx, &a), TextPreviewPoll::Loading);
        assert_eq!(executor.pending_count(), 1);
        executor.run_all();
        match pipeline.request(&ctx, &a) {
            TextPreviewPoll::Ready(content) => assert_eq!(content.as_ref(), "alpha"),
            other => panic!("expected latest A preview, got {other:?}"),
        }
        assert_eq!(provider.reads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn panic_scheduler_failure_and_timeout_are_terminal_failures() {
        let (mut pipeline, executor, provider, clock, ctx) = pipeline_harness();
        let request = identity("panic.txt", 4);
        provider.push(ScriptedAction::Panic);
        submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &request);
        executor.run_next();
        assert_eq!(
            pipeline.request(&ctx, &request),
            TextPreviewPoll::Failed(PreviewError::ProviderPanicked)
        );

        pipeline.retry(&request.path);
        executor.fail_next();
        assert_eq!(pipeline.request(&ctx, &request), TextPreviewPoll::Loading);
        clock.advance(TEXT_PREVIEW_DEBOUNCE);
        assert_eq!(
            pipeline.request(&ctx, &request),
            TextPreviewPoll::Failed(PreviewError::SchedulerUnavailable)
        );

        pipeline.retry(&request.path);
        submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &request);
        clock.advance(TEXT_PREVIEW_TIMEOUT);
        assert_eq!(
            pipeline.request(&ctx, &request),
            TextPreviewPoll::Failed(PreviewError::TimedOut)
        );
        assert!(executor.cancellations.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn timeout_waits_for_the_retiring_worker_before_retry_submission() {
        let executor = TestExecutor {
            cancel_finishes: false,
            ..TestExecutor::default()
        };
        let provider = Arc::new(ScriptedProvider::default());
        let clock = TestClock::new();
        let mut pipeline = TextPreviewPipeline::with_parts(
            executor.clone(),
            provider.clone(),
            Arc::new(clock.clone()),
        );
        let ctx = egui::Context::default();
        let request = identity("blocked.txt", 5);

        submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &request);
        clock.advance(TEXT_PREVIEW_TIMEOUT);
        assert_eq!(
            pipeline.request(&ctx, &request),
            TextPreviewPoll::Failed(PreviewError::TimedOut)
        );

        provider.push(ScriptedAction::Text("fresh"));
        pipeline.retry(&request.path);
        assert_eq!(pipeline.request(&ctx, &request), TextPreviewPoll::Loading);
        clock.advance(TEXT_PREVIEW_DEBOUNCE);
        assert_eq!(pipeline.request(&ctx, &request), TextPreviewPoll::Loading);
        assert!(matches!(
            pipeline.phase,
            super::TextPreviewPhase::WaitingForWorker
        ));
        assert_eq!(executor.pending_count(), 0, "retry is not admitted yet");

        assert!(executor.run_next(), "retiring job reaches terminal state");
        assert_eq!(pipeline.request(&ctx, &request), TextPreviewPoll::Loading);
        assert_eq!(executor.pending_count(), 1, "retry is admitted after reap");
        executor.run_next();
        match pipeline.request(&ctx, &request) {
            TextPreviewPoll::Ready(content) => assert_eq!(content.as_ref(), "fresh"),
            other => panic!("expected retried preview, got {other:?}"),
        }
    }

    #[test]
    fn explicit_failure_retry_and_close_have_complete_lifecycles() {
        let (mut pipeline, executor, provider, clock, ctx) = pipeline_harness();
        let request = identity("retry.txt", 5);
        provider.push(ScriptedAction::Error(PreviewError::Unreadable));
        submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &request);
        executor.run_next();
        assert_eq!(
            pipeline.request(&ctx, &request),
            TextPreviewPoll::Failed(PreviewError::Unreadable)
        );

        pipeline.retry(&request.path);
        provider.push(ScriptedAction::Text("ready"));
        submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &request);
        executor.run_next();
        match pipeline.request(&ctx, &request) {
            TextPreviewPoll::Ready(content) => assert_eq!(content.as_ref(), "ready"),
            other => panic!("expected retried preview, got {other:?}"),
        }

        let other = identity("close.txt", 5);
        submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &other);
        pipeline.cancel_if_active(&request);
        assert!(pipeline.active.is_some());
        assert!(pipeline.task.is_some());
        pipeline.cancel_if_active(&other);
        assert!(pipeline.active.is_none());
        assert!(
            pipeline.task.is_some(),
            "the retiring handle stays observable"
        );
        pipeline.reap_finished_without_completion();
        assert!(pipeline.task.is_none());
        assert_eq!(executor.pending_count(), 0);
    }

    #[test]
    fn early_size_regular_file_and_encoding_policies_are_typed() {
        let (mut pipeline, executor, _provider, _clock, ctx) = pipeline_harness();
        let too_large = identity("large.txt", MAX_TEXT_PREVIEW_BYTES + 1);
        assert_eq!(
            pipeline.request(&ctx, &too_large),
            TextPreviewPoll::Failed(PreviewError::TooLarge)
        );
        assert_eq!(executor.pending_count(), 0);

        let temp = TempDir::new();
        let directory = identity(temp.path(), 0);
        let fs_provider = FsTextPreviewProvider::default();
        assert!(matches!(
            super::TextPreviewProvider::fingerprint(&fs_provider, &directory),
            Err(PreviewError::NotRegular)
        ));

        assert_eq!(
            decode_text_bytes(b"\xef\xbb\xbfhello".to_vec())
                .unwrap()
                .as_ref(),
            "hello"
        );
        assert_eq!(
            decode_text_bytes(vec![0xff, 0xfe, b'a', 0]),
            Err(PreviewError::UnsupportedEncoding)
        );
        assert_eq!(
            decode_text_bytes(b"a\0b".to_vec()),
            Err(PreviewError::Binary)
        );
        assert_eq!(
            decode_text_bytes(vec![1, 2, 3, b'a']),
            Err(PreviewError::Binary)
        );
        assert_eq!(
            decode_text_bytes(vec![0xff]),
            Err(PreviewError::InvalidUtf8)
        );
    }

    #[test]
    fn chunked_read_observes_cancellation_between_chunks() {
        let temp = TempDir::new();
        let path = temp.file(
            "cancel.txt",
            &"a".repeat(super::TEXT_PREVIEW_CHUNK_BYTES * 2),
        );
        let metadata = std::fs::metadata(&path).unwrap();
        let request = PreviewIdentity {
            path,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let observer_cancelled = Arc::clone(&cancelled);
        let provider = FsTextPreviewProvider {
            chunk_observer: Some(Arc::new(move |chunk| {
                if chunk == 1 {
                    observer_cancelled.store(true, Ordering::Release);
                }
            })),
        };
        let expected = super::TextPreviewProvider::fingerprint(&provider, &request).unwrap();

        assert_eq!(
            super::TextPreviewProvider::read(&provider, &request, &expected, &|| {
                cancelled.load(Ordering::Acquire)
            }),
            Err(PreviewError::Cancelled)
        );
    }

    #[test]
    fn rewrite_during_chunked_read_is_rejected_without_sleeping() {
        let temp = TempDir::new();
        let path = temp.file(
            "rewrite.txt",
            &"a".repeat(super::TEXT_PREVIEW_CHUNK_BYTES * 2),
        );
        let metadata = std::fs::metadata(&path).unwrap();
        let request = PreviewIdentity {
            path: path.clone(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        };
        let reached_chunk = Arc::new(Barrier::new(2));
        let continue_read = Arc::new(Barrier::new(2));
        let observer_reached = Arc::clone(&reached_chunk);
        let observer_continue = Arc::clone(&continue_read);
        let provider = Arc::new(FsTextPreviewProvider {
            chunk_observer: Some(Arc::new(move |chunk| {
                if chunk == 1 {
                    observer_reached.wait();
                    observer_continue.wait();
                }
            })),
        });
        let expected =
            super::TextPreviewProvider::fingerprint(provider.as_ref(), &request).unwrap();
        let worker_provider = Arc::clone(&provider);
        let worker_request = request.clone();
        let worker = std::thread::spawn(move || {
            super::TextPreviewProvider::read(
                worker_provider.as_ref(),
                &worker_request,
                &expected,
                &|| false,
            )
        });

        reached_chunk.wait();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        continue_read.wait();

        assert_eq!(worker.join().unwrap(), Err(PreviewError::Changed));
    }

    #[cfg(unix)]
    #[test]
    fn same_size_rewrite_with_restored_mtime_changes_the_fingerprint() {
        let temp = TempDir::new();
        let path = temp.file("same-size.txt", "aaaa");
        let original_metadata = std::fs::metadata(&path).unwrap();
        let original_modified = original_metadata.modified().unwrap();
        let request = PreviewIdentity {
            path: path.clone(),
            size: original_metadata.len(),
            modified: Some(original_modified),
        };
        let provider = FsTextPreviewProvider::default();
        let before = super::TextPreviewProvider::fingerprint(&provider, &request).unwrap();

        std::fs::write(&path, b"bbbb").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(original_modified))
            .unwrap();

        let after_metadata = std::fs::metadata(&path).unwrap();
        let after = super::TextPreviewProvider::fingerprint(&provider, &request).unwrap();
        assert_eq!(after_metadata.len(), original_metadata.len());
        assert_eq!(after_metadata.modified().unwrap(), original_modified);
        assert_ne!(
            (before.change_seconds, before.change_nanoseconds),
            (after.change_seconds, after.change_nanoseconds)
        );
        assert_ne!(before, after);
    }
}
