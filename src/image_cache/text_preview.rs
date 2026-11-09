//! Bounded, cancellable text preview loading and decoding.

use super::*;

#[derive(Clone)]
pub(super) enum TextPreviewDocument {
    Text(Arc<str>),
    Books(Arc<crate::fictionbook_preview::BookCollection>),
}
impl TextPreviewDocument {
    fn byte_size(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Books(books) => books.byte_size(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TextPreviewPoll {
    Current,
    Loading,
    Ready(Arc<str>),
    Books(Arc<crate::fictionbook_preview::BookCollection>),
    Failed(PreviewError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPreviewLoadState {
    Idle,
    Debouncing,
    Waiting,
    Loading,
    Ready,
    Failed(PreviewError),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct TextPreviewFingerprint {
    pub(super) path: PathBuf,
    pub(super) volume: Option<u64>,
    pub(super) file_id: Option<u64>,
    pub(super) size: u64,
    pub(super) modified: Option<SystemTime>,
    pub(super) change_seconds: Option<i64>,
    pub(super) change_nanoseconds: Option<i64>,
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

    pub(super) fn matches_request(&self, identity: &crate::panel::PreviewIdentity) -> bool {
        self.path == identity.path
            && self.size == identity.size
            && identity
                .modified
                .is_none_or(|modified| self.modified == Some(modified))
    }
}

pub(super) struct TextPreviewEntry {
    content: TextPreviewDocument,
    byte_size: usize,
    last_used: u64,
}

pub(super) struct TextPreviewStore {
    pub(super) entries: HashMap<TextPreviewFingerprint, TextPreviewEntry>,
    pub(super) total_bytes: usize,
    max_bytes: usize,
    max_entries: usize,
    tick: u64,
}

impl TextPreviewStore {
    pub(super) fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            max_bytes,
            max_entries,
            tick: 0,
        }
    }

    pub(super) fn get_document(
        &mut self,
        key: &TextPreviewFingerprint,
    ) -> Option<TextPreviewDocument> {
        self.tick = self.tick.saturating_add(1);
        let entry = self.entries.get_mut(key)?;
        entry.last_used = self.tick;
        Some(entry.content.clone())
    }

    #[cfg(test)]
    pub(super) fn get(&mut self, key: &TextPreviewFingerprint) -> Option<Arc<str>> {
        match self.get_document(key)? {
            TextPreviewDocument::Text(text) => Some(text),
            TextPreviewDocument::Books(_) => None,
        }
    }
    #[cfg(test)]
    pub(super) fn insert(&mut self, key: TextPreviewFingerprint, content: Arc<str>) {
        self.insert_document(key, TextPreviewDocument::Text(content));
    }
    pub(super) fn insert_document(
        &mut self,
        key: TextPreviewFingerprint,
        content: TextPreviewDocument,
    ) {
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

        let byte_size = content.byte_size();
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
pub(super) struct TextPreviewOwnership {
    identity: crate::panel::PreviewIdentity,
}

struct TextPreviewSuccess {
    fingerprint: TextPreviewFingerprint,
    content: TextPreviewDocument,
}

struct TextPreviewCompletion {
    generation: u64,
    result: Result<TextPreviewSuccess, PreviewError>,
}

pub(super) enum TextPreviewPhase {
    Idle,
    Debouncing {
        ready_at: Instant,
    },
    /// The previous read still holds the worker; after `deadline` the
    /// request fails as `WorkerBusy` instead of spinning forever.
    WaitingForWorker {
        deadline: Instant,
    },
    Running {
        deadline: Instant,
    },
    Ready(TextPreviewDocument),
    Failed(PreviewError),
}

pub(super) trait TextPreviewClock: Send + Sync {
    fn now(&self) -> Instant;
}

struct SystemTextPreviewClock;

impl TextPreviewClock for SystemTextPreviewClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub(super) trait TextPreviewProvider: Send + Sync {
    fn read_document(
        &self,
        identity: &crate::panel::PreviewIdentity,
        expected: &TextPreviewFingerprint,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<TextPreviewDocument, PreviewError> {
        self.read(identity, expected, cancelled)
            .map(TextPreviewDocument::Text)
    }

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
pub(super) struct FsTextPreviewProvider {
    pub(super) chunk_observer: Option<Arc<dyn Fn(usize) + Send + Sync>>,
}

#[cfg(test)]
pub(super) fn read_preview_bytes(
    reader: &mut impl Read,
    reserve_bytes: usize,
    cancelled: &dyn Fn() -> bool,
    chunk_observer: Option<&(dyn Fn(usize) + Send + Sync)>,
) -> Result<Vec<u8>, PreviewError> {
    read_preview_bytes_with_limit(
        reader,
        reserve_bytes,
        cancelled,
        chunk_observer,
        MAX_TEXT_PREVIEW_BYTES as usize,
    )
}

fn read_preview_bytes_with_limit(
    reader: &mut impl Read,
    reserve_bytes: usize,
    cancelled: &dyn Fn() -> bool,
    chunk_observer: Option<&(dyn Fn(usize) + Send + Sync)>,
    max_bytes: usize,
) -> Result<Vec<u8>, PreviewError> {
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
        let count = match reader.read(&mut chunk) {
            Ok(count) => count,
            // A signal interrupting read() is not a file error; retry it.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(PreviewError::Unreadable),
        };
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

/// Fingerprint an open handle (fstat), so a rename or rewrite after the
/// path check is still caught.
fn handle_fingerprint(
    file: &std::fs::File,
    path: &Path,
) -> Result<TextPreviewFingerprint, PreviewError> {
    let metadata = file.metadata().map_err(|_| PreviewError::Unreadable)?;
    TextPreviewFingerprint::from_metadata(path, &metadata)
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
        // Follow symlinks: the file is opened through the link, so the path
        // fingerprint must describe the same target as the handle's fstat.
        let metadata = std::fs::metadata(&identity.path).map_err(|_| PreviewError::Unreadable)?;
        TextPreviewFingerprint::from_metadata(&identity.path, &metadata)
    }

    fn read(
        &self,
        identity: &crate::panel::PreviewIdentity,
        expected: &TextPreviewFingerprint,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Arc<str>, PreviewError> {
        match self.read_document(identity, expected, cancelled)? {
            TextPreviewDocument::Text(text) => Ok(text),
            TextPreviewDocument::Books(_) => Err(PreviewError::InvalidDocument),
        }
    }
    fn read_document(
        &self,
        identity: &crate::panel::PreviewIdentity,
        expected: &TextPreviewFingerprint,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<TextPreviewDocument, PreviewError> {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let before = self.fingerprint(identity)?;
        if &before != expected {
            return Err(PreviewError::Changed);
        }

        let mut file = std::fs::File::open(&identity.path).map_err(|_| PreviewError::Unreadable)?;
        let opened = handle_fingerprint(&file, &identity.path)?;
        if &opened != expected {
            return Err(PreviewError::Changed);
        }

        let bytes = read_preview_bytes_with_limit(
            &mut file,
            expected.size as usize,
            cancelled,
            self.chunk_observer.as_deref(),
            text_input_limit(&identity.path) as usize,
        )?;

        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let after_handle = handle_fingerprint(&file, &identity.path)?;
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
            let verification_opened = handle_fingerprint(&verification_file, &identity.path)?;
            if &verification_opened != expected {
                return Err(PreviewError::Changed);
            }
            let verification = read_preview_bytes_with_limit(
                &mut verification_file,
                expected.size as usize,
                cancelled,
                None,
                text_input_limit(&identity.path) as usize,
            )?;
            let verification_after = self.fingerprint(identity)?;
            if &verification_after != expected || verification != bytes {
                return Err(PreviewError::Changed);
            }
        }
        let text = if crate::rtf_preview::supported(&identity.path) {
            crate::rtf_preview::extract(&bytes, cancelled).map(Arc::from)
        } else if crate::fictionbook_preview::compressed(&identity.path) {
            let books = crate::fictionbook_preview::collection(bytes, cancelled)?;
            return if books.books.len() == 1 {
                Ok(TextPreviewDocument::Text(Arc::clone(&books.books[0].text)))
            } else {
                Ok(TextPreviewDocument::Books(Arc::new(books)))
            };
        } else if crate::fictionbook_preview::supported(&identity.path) {
            crate::fictionbook_preview::extract_file(&identity.path, bytes, cancelled)
                .map(Arc::from)
        } else if crate::office_preview::supported(&identity.path) {
            crate::office_preview::extract(&identity.path, bytes, cancelled).map(Arc::from)
        } else {
            decode_text_bytes(bytes)
        };
        text.map(TextPreviewDocument::Text)
    }
}

/// Decode only Unicode signatures, never guess byte order from a filename or
/// a NUL pattern. Input and decoded UTF-8 both retain the existing byte bound.
pub(super) fn decode_text_bytes(mut bytes: Vec<u8>) -> Result<Arc<str>, PreviewError> {
    if bytes.len() > MAX_TEXT_PREVIEW_BYTES as usize {
        return Err(PreviewError::TooLarge);
    }
    let signature = if bytes.starts_with(&[0xff, 0xfe, 0, 0]) {
        Some((4, true))
    } else if bytes.starts_with(&[0, 0, 0xfe, 0xff]) {
        Some((4, false))
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        Some((2, true))
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        Some((2, false))
    } else {
        None
    };
    if let Some((unit, little_endian)) = signature {
        let body = &bytes[unit..];
        if !body.len().is_multiple_of(unit) {
            return Err(PreviewError::InvalidUnicode);
        }
        let mut text = String::new();
        let mut push = |character: char| -> Result<(), PreviewError> {
            if text.len().saturating_add(character.len_utf8()) > MAX_TEXT_PREVIEW_BYTES as usize {
                return Err(PreviewError::TooLarge);
            }
            text.push(character);
            Ok(())
        };
        if unit == 2 {
            let units = body.as_chunks::<2>().0.iter().map(|pair| {
                let pair = *pair;
                if little_endian {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            });
            for character in char::decode_utf16(units) {
                push(character.map_err(|_| PreviewError::InvalidUnicode)?)?;
            }
        } else {
            for quad in body.as_chunks::<4>().0 {
                let quad = *quad;
                let value = if little_endian {
                    u32::from_le_bytes(quad)
                } else {
                    u32::from_be_bytes(quad)
                };
                push(char::from_u32(value).ok_or(PreviewError::InvalidUnicode)?)?;
            }
        }
        bytes = text.into_bytes();
    }
    if signature.is_none() && bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        bytes.drain(..3);
    }
    if bytes.contains(&0) {
        return Err(PreviewError::Binary);
    }
    let controls = bytes
        .iter()
        .filter(|byte| {
            let byte = **byte;
            // ESC is allowed so saved ANSI-colored output (ls --color, logs)
            // still previews as text.
            (byte < 0x20 && !matches!(byte, b'\n' | b'\r' | b'\t' | 0x08 | 0x0c | 0x1b))
                || byte == 0x7f
        })
        .count();
    if !bytes.is_empty() && controls.saturating_mul(20) > bytes.len() {
        return Err(PreviewError::Binary);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Arc::from(text)),
        // A stray Latin-1/CP1252 byte in otherwise UTF-8 text (a pasted
        // "café") shows as U+FFFD instead of rejecting the whole preview.
        Err(error) if mostly_utf8(error.as_bytes()) => {
            let text = String::from_utf8_lossy(error.as_bytes());
            if text.len() > MAX_TEXT_PREVIEW_BYTES as usize {
                return Err(PreviewError::TooLarge);
            }
            Ok(Arc::from(text))
        }
        Err(_) => Err(PreviewError::InvalidUtf8),
    }
}

/// At most one invalid UTF-8 sequence per `MIN_BYTES_PER_INVALID_SEQUENCE`
/// bytes: few enough that the replacement characters stay incidental. A
/// file in a legacy encoding throughout still fails as `InvalidUtf8`.
fn mostly_utf8(bytes: &[u8]) -> bool {
    const MIN_BYTES_PER_INVALID_SEQUENCE: usize = 200;
    let invalid = bytes
        .utf8_chunks()
        .filter(|chunk| !chunk.invalid().is_empty())
        .count();
    invalid.saturating_mul(MIN_BYTES_PER_INVALID_SEQUENCE) <= bytes.len()
}

pub(super) trait TextPreviewTaskHandle {
    fn cancel(&self) -> bool;

    fn is_finished(&self) -> bool;
}

pub(super) type TextPreviewJob =
    Box<dyn FnOnce(Arc<dyn Fn() -> bool + Send + Sync>) + Send + 'static>;

pub(super) struct TextPreviewSubmission {
    pub(super) generation: u64,
}

pub(super) trait TextPreviewExecutor: Send + Sync {
    type Handle: TextPreviewTaskHandle;

    fn submit(
        &self,
        submission: TextPreviewSubmission,
        job: TextPreviewJob,
    ) -> Result<Self::Handle, PreviewError>;
}

#[derive(Default)]
pub(super) struct IsolatedTextPreviewExecutor {
    active: Arc<AtomicBool>,
}

pub(super) struct IsolatedTextPreviewTaskHandle {
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

pub(super) struct RunningTextPreviewTask<H> {
    generation: u64,
    handle: H,
}

pub(super) struct TextPreviewPipeline<E: TextPreviewExecutor> {
    executor: E,
    provider: Arc<dyn TextPreviewProvider>,
    clock: Arc<dyn TextPreviewClock>,
    store: Arc<Mutex<TextPreviewStore>>,
    generation: Arc<AtomicU64>,
    pub(super) active: Option<TextPreviewOwnership>,
    pub(super) phase: TextPreviewPhase,
    pub(super) task: Option<RunningTextPreviewTask<E::Handle>>,
    delivered: bool,
    sender: Sender<TextPreviewCompletion>,
    receiver: Receiver<TextPreviewCompletion>,
}

impl TextPreviewPipeline<IsolatedTextPreviewExecutor> {
    pub(super) fn production() -> Self {
        Self::with_parts(
            IsolatedTextPreviewExecutor::default(),
            Arc::new(FsTextPreviewProvider::default()),
            Arc::new(SystemTextPreviewClock),
        )
    }
}

/// The phase a fresh or retried request starts in: files over the preview
/// cap fail at once, everything else waits out the debounce first.
pub(super) fn text_input_limit(path: &Path) -> u64 {
    if crate::rtf_preview::supported(path) {
        crate::rtf_preview::MAX_INPUT_BYTES as u64
    } else if crate::fictionbook_preview::compressed(path) {
        crate::office_preview::MAX_PACKAGE_BYTES
    } else if crate::fictionbook_preview::supported(path) {
        crate::fictionbook_preview::MAX_INPUT_BYTES as u64
    } else if crate::office_preview::supported(path) {
        crate::office_preview::MAX_PACKAGE_BYTES
    } else {
        MAX_TEXT_PREVIEW_BYTES
    }
}

pub(super) fn initial_text_phase(size: u64, now: Instant, limit: u64) -> TextPreviewPhase {
    if size > limit {
        TextPreviewPhase::Failed(PreviewError::TooLarge)
    } else {
        TextPreviewPhase::Debouncing {
            ready_at: now + TEXT_PREVIEW_DEBOUNCE,
        }
    }
}

impl<E: TextPreviewExecutor> TextPreviewPipeline<E> {
    pub(super) fn with_parts(
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
        self.phase = initial_text_phase(
            ownership.identity.size,
            now,
            text_input_limit(&ownership.identity.path),
        );
    }

    pub(super) fn request(
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
        ) || matches!(self.phase, TextPreviewPhase::WaitingForWorker { .. })
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
            | TextPreviewPhase::WaitingForWorker { .. }
            | TextPreviewPhase::Running { .. } => TextPreviewPoll::Loading,
            TextPreviewPhase::Ready(content) => {
                self.delivered = true;
                match content {
                    TextPreviewDocument::Text(text) => TextPreviewPoll::Ready(Arc::clone(text)),
                    TextPreviewDocument::Books(books) => TextPreviewPoll::Books(Arc::clone(books)),
                }
            }
            TextPreviewPhase::Failed(error) => {
                self.delivered = true;
                TextPreviewPoll::Failed(*error)
            }
        }
    }

    fn submit(&mut self, ctx: &Context, now: Instant) {
        if self.task.is_some() {
            self.wait_for_worker(now);
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
                    && let Some(content) =
                        crate::lock_util::recover(&store).get_document(&fingerprint)
                {
                    return Ok(TextPreviewSuccess {
                        fingerprint,
                        content,
                    });
                }
                let content = provider.read_document(&identity, &fingerprint, &cancelled)?;
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

        let previous_wait = std::mem::replace(
            &mut self.phase,
            TextPreviewPhase::Running {
                deadline: now + TEXT_PREVIEW_TIMEOUT,
            },
        );
        match self
            .executor
            .submit(TextPreviewSubmission { generation }, job)
        {
            Ok(handle) => {
                self.task = Some(RunningTextPreviewTask { generation, handle });
            }
            Err(PreviewError::WorkerBusy) => {
                self.phase = previous_wait;
                self.wait_for_worker(now);
            }
            Err(error) => {
                self.phase = TextPreviewPhase::Failed(error);
                self.delivered = false;
            }
        }
    }

    /// Enter (or stay in) `WaitingForWorker`, keeping the deadline of a wait
    /// already under way so per-frame resubmission cannot extend it.
    fn wait_for_worker(&mut self, now: Instant) {
        if !matches!(self.phase, TextPreviewPhase::WaitingForWorker { .. }) {
            self.phase = TextPreviewPhase::WaitingForWorker {
                deadline: now + TEXT_PREVIEW_TIMEOUT,
            };
        }
        self.delivered = false;
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
                            .insert_document(success.fingerprint, success.content.clone());
                    }
                    TextPreviewPhase::Ready(success.content)
                }
                Err(error) => TextPreviewPhase::Failed(error),
            };
        }
    }

    pub(super) fn reap_finished_without_completion(&mut self) {
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
        // Nothing of this request was submitted, so there is no task of ours
        // to cancel; the hung read keeps the worker until it returns.
        let worker_stuck = matches!(
            self.phase,
            TextPreviewPhase::WaitingForWorker { deadline } if now >= deadline
        );
        if worker_stuck {
            self.phase = TextPreviewPhase::Failed(PreviewError::WorkerBusy);
            self.delivered = false;
        }
    }

    fn schedule_repaint(&self, ctx: &Context, now: Instant) {
        match self.phase {
            TextPreviewPhase::Debouncing { ready_at } => {
                ctx.request_repaint_after(ready_at.saturating_duration_since(now));
            }
            TextPreviewPhase::WaitingForWorker { deadline } => {
                ctx.request_repaint_after(
                    TEXT_PREVIEW_POLL_INTERVAL.min(deadline.saturating_duration_since(now)),
                );
            }
            TextPreviewPhase::Running { deadline } => {
                ctx.request_repaint_after(
                    TEXT_PREVIEW_POLL_INTERVAL.min(deadline.saturating_duration_since(now)),
                );
            }
            TextPreviewPhase::Idle | TextPreviewPhase::Ready(_) | TextPreviewPhase::Failed(_) => {}
        }
    }

    pub(super) fn load_state(&self, path: &Path) -> TextPreviewLoadState {
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
            TextPreviewPhase::WaitingForWorker { .. } => TextPreviewLoadState::Waiting,
            TextPreviewPhase::Running { .. } => TextPreviewLoadState::Loading,
            TextPreviewPhase::Ready(_) => TextPreviewLoadState::Ready,
            TextPreviewPhase::Failed(error) => TextPreviewLoadState::Failed(error),
        }
    }

    pub(super) fn retry(&mut self, path: &Path) {
        crate::lock_util::recover(&self.store).remove_path(path);
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.identity.path == path)
        {
            self.cancel_task();
            self.advance_generation();
            let size = self
                .active
                .as_ref()
                .map_or(0, |active| active.identity.size);
            self.phase = initial_text_phase(size, self.clock.now(), text_input_limit(path));
            self.delivered = false;
        }
    }

    pub(super) fn cancel(&mut self) {
        if self.active.is_some() || self.task.is_some() {
            self.cancel_task();
            self.advance_generation();
        }
        self.active = None;
        self.phase = TextPreviewPhase::Idle;
        self.delivered = false;
    }

    pub(super) fn cancel_if_active(&mut self, identity: &crate::panel::PreviewIdentity) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.identity == *identity)
        {
            self.cancel();
        }
    }
}
