use super::{
    FsTextPreviewProvider, ImageCache, MAX_DECODED_IMAGE_BYTES, MAX_IMAGE_DIMENSION,
    MAX_TEXT_PREVIEW_BYTES, PreviewError, PreviewFailure, PreviewTarget, TEXT_PREVIEW_DEBOUNCE,
    TEXT_PREVIEW_TIMEOUT, TextPreviewClock, TextPreviewExecutor, TextPreviewFingerprint,
    TextPreviewJob, TextPreviewPipeline, TextPreviewPoll, TextPreviewStore, TextPreviewSubmission,
    TextPreviewTaskHandle, allocate_rgba_pixels, classify_failure, color_image_from_rgba,
    counts_against_decoder, decode_text_bytes, fit_oriented, load_via_image_crate,
    unpremultiply_rgba,
};
use crate::panel::PreviewIdentity;
use crate::testutil::TempDir;
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

#[test]
fn oriented_thumbnail_matches_rotating_the_full_image_first() {
    use image::metadata::Orientation;
    // A non-square smooth gradient so a wrong box or rotation shows in
    // both dimensions and pixels.
    let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(400, 100, |x, y| {
        image::Rgba([(x * 255 / 399) as u8, (y * 255 / 99) as u8, 128, 255])
    }));
    let target = PreviewTarget {
        width: 64,
        height: 48,
    };
    for orientation in [
        Orientation::NoTransforms,
        Orientation::Rotate90,
        Orientation::Rotate180,
        Orientation::Rotate270,
        Orientation::FlipHorizontal,
        Orientation::Rotate90FlipH,
        Orientation::Rotate270FlipH,
    ] {
        let mut expected = source.clone();
        expected.apply_orientation(orientation);
        let expected = expected.thumbnail(target.width, target.height);

        let fitted = fit_oriented(source.clone(), orientation, target);

        assert_eq!(fitted.width(), expected.width(), "{orientation:?}");
        assert_eq!(fitted.height(), expected.height(), "{orientation:?}");
        assert!(fitted.width() <= target.width && fitted.height() <= target.height);
        // Downscaling before or after the exact rotation samples slightly
        // differently; the result must still be the same picture.
        let max_difference = fitted
            .to_rgba8()
            .as_raw()
            .iter()
            .zip(expected.to_rgba8().as_raw())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(max_difference <= 8, "{orientation:?}: {max_difference}");
    }
    // Already small enough: only oriented, never upscaled.
    let small = image::DynamicImage::new_rgba8(20, 10);
    let fitted = fit_oriented(small, Orientation::Rotate90, target);
    assert_eq!((fitted.width(), fitted.height()), (10, 20));
}

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
        let Some((job, cancelled, finished)) = crate::lock_util::recover(&self.jobs).pop_front()
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
fn standard_decoder_rejects_undecodable_formats_for_negative_cache() {
    let dir = TempDir::new();
    let svg = dir.path().join("vector.svg");
    std::fs::write(&svg, b"<svg xmlns='http://www.w3.org/2000/svg'></svg>").unwrap();
    let error = load_via_image_crate(
        &svg,
        PreviewTarget {
            width: 64,
            height: 64,
        },
    )
    .unwrap_err();
    assert_eq!(classify_failure(&error), PreviewFailure::Unsupported);

    let mkv = dir.path().join("clip.mkv");
    std::fs::write(&mkv, b"not a matroska container").unwrap();
    let error = load_via_image_crate(
        &mkv,
        PreviewTarget {
            width: 64,
            height: 64,
        },
    )
    .unwrap_err();
    assert_eq!(classify_failure(&error), PreviewFailure::Unsupported);

    let webm = dir.path().join("clip.webm");
    std::fs::write(&webm, b"not a webm container").unwrap();
    let error = load_via_image_crate(
        &webm,
        PreviewTarget {
            width: 64,
            height: 64,
        },
    )
    .unwrap_err();
    assert_eq!(classify_failure(&error), PreviewFailure::Unsupported);
}

#[test]
fn standard_decoder_applies_exif_orientation_from_jpeg() {
    // Minimal baseline JPEG (2x1) with EXIF Orientation=6 (rotate 90 CW).
    // Portable proof that the standard decoder path honours EXIF without
    // needing ImageIO fixtures; HEIF/ImageIO color parity remains macOS-only.
    let dir = TempDir::new();
    let path = dir.path().join("oriented.jpg");
    std::fs::write(&path, oriented_jpeg_2x1_rotate90()).unwrap();

    let decoded = load_via_image_crate(
        &path,
        PreviewTarget {
            width: 16,
            height: 16,
        },
    )
    .unwrap();
    assert_eq!(
        decoded.image.size,
        [1, 2],
        "Orientation=6 must swap the 2x1 source into a 1x2 preview"
    );
}

/// Tiny JPEG with EXIF Orientation tag = 6 (90° CW). Built by hand so the
/// test stays fixture-free and runs on Linux CI without `ImageIO`.
fn oriented_jpeg_2x1_rotate90() -> Vec<u8> {
    let pixels = image::RgbImage::from_fn(2, 1, |x, _| {
        if x == 0 {
            image::Rgb([255, 0, 0])
        } else {
            image::Rgb([0, 0, 255])
        }
    });
    let mut jpeg = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut jpeg);
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 90);
        encoder
            .encode(pixels.as_raw(), 2, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
    }

    // APP1 / Exif: "Exif\0\0" + TIFF LE IFD with Orientation SHORT = 6.
    let app1: &[u8] = &[
        0xFF, 0xE1, 0x00, 0x22, // APP1 + length 34
        b'E', b'x', b'i', b'f', 0x00, 0x00, // Exif header
        b'I', b'I', 0x2A, 0x00, // TIFF LE
        0x08, 0x00, 0x00, 0x00, // IFD0 offset
        0x01, 0x00, // one entry
        0x12, 0x01, // Orientation
        0x03, 0x00, // SHORT
        0x01, 0x00, 0x00, 0x00, // count
        0x06, 0x00, 0x00, 0x00, // value = 6
        0x00, 0x00, 0x00, 0x00, // next IFD
    ];
    let mut out = Vec::with_capacity(jpeg.len() + app1.len());
    out.extend_from_slice(&jpeg[..2]); // SOI
    out.extend_from_slice(app1);
    out.extend_from_slice(&jpeg[2..]);
    out
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
fn growing_the_pane_keeps_the_smaller_texture_until_the_redecode_lands() {
    let ctx = egui::Context::default();
    let mut cache = ImageCache::new();
    let dir = PathBuf::from("/preview-dir");
    let path = dir.join("photo.png");
    cache.current_dir = Some(dir.clone());
    let small = PreviewTarget {
        width: 10,
        height: 10,
    };
    cache.entries.insert(
        path.clone(),
        super::CacheEntry {
            texture: ctx.load_texture(
                "small",
                egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            ),
            byte_size: 4,
            last_used: 0,
            decoded_for: small,
        },
    );
    cache.total_bytes = 4;
    // The re-decode is already in flight, so preload starts no worker.
    let generation = cache.generation.load(Ordering::Acquire);
    crate::lock_util::recover(&cache.pending).insert(
        path.clone(),
        super::PendingLoad {
            generation,
            decoded: None,
        },
    );
    cache.set_preview_extent(400.0, 300.0, 1.0);

    cache.preload(&ctx, std::slice::from_ref(&path), &dir);

    assert!(
        cache.get(&path).is_some(),
        "preview blanked while re-decoding"
    );
    assert_eq!(cache.total_bytes, 4);

    let sharp = cache.preview_target;
    crate::lock_util::recover(&cache.pending)
        .get_mut(&path)
        .unwrap()
        .decoded = Some(super::DecodedPreview {
        image: egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
        byte_size: 16,
        decoded_for: sharp,
    });
    cache.preload(&ctx, std::slice::from_ref(&path), &dir);

    assert_eq!(cache.entries[&path].decoded_for, sharp);
    assert_eq!(cache.total_bytes, 16, "replaced texture bytes not released");
}

#[test]
fn an_in_place_edit_drops_the_stale_texture_failure_and_decode() {
    let ctx = egui::Context::default();
    let mut cache = ImageCache::new();
    let edited = PathBuf::from("/dir/edited.png");
    let same = PathBuf::from("/dir/same.png");
    for (name, path) in [("edited", &edited), ("same", &same)] {
        cache.entries.insert(
            path.clone(),
            super::CacheEntry {
                texture: ctx.load_texture(
                    name,
                    egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
                    egui::TextureOptions::LINEAR,
                ),
                byte_size: 4,
                last_used: 0,
                decoded_for: PreviewTarget::DEFAULT,
            },
        );
    }
    cache.total_bytes = 8;
    let then = std::time::SystemTime::UNIX_EPOCH;
    let later = then + std::time::Duration::from_secs(60);
    cache.invalidate_changed(&[
        (edited.clone(), (10, Some(then))),
        (same.clone(), (5, None)),
    ]);
    assert_eq!(cache.stats().entries, 2, "first sighting keeps the cache");

    crate::lock_util::recover(&cache.failed).insert(
        edited.clone(),
        super::FailedLoad::new(PreviewFailure::Decode, Instant::now()),
    );
    crate::lock_util::recover(&cache.pending).insert(
        edited.clone(),
        super::PendingLoad {
            generation: 0,
            decoded: None,
        },
    );
    cache.invalidate_changed(&[
        (edited.clone(), (10, Some(later))),
        (same.clone(), (5, None)),
    ]);

    assert!(cache.get(&edited).is_none(), "stale texture still served");
    assert!(cache.get(&same).is_some());
    assert_eq!(cache.total_bytes, 4);
    assert_eq!(cache.stats().failed, 0);
    assert_eq!(cache.stats().pending, 0);
    assert_eq!(
        cache.generation.load(Ordering::Acquire),
        1,
        "the old decode's generation must not own the next slot"
    );
}

#[test]
fn retry_clears_negative_and_pending_cache_entries() {
    let mut cache = ImageCache::new();
    let path = std::path::PathBuf::from("broken.png");
    crate::lock_util::recover(&cache.failed).insert(
        path.clone(),
        super::FailedLoad::new(PreviewFailure::Decode, Instant::now()),
    );
    crate::lock_util::recover(&cache.pending).insert(
        path.clone(),
        super::PendingLoad {
            generation: 0,
            decoded: None,
        },
    );
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
        super::TextPreviewPhase::WaitingForWorker { .. }
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
fn a_hung_retiring_worker_fails_the_waiting_request_after_the_timeout() {
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
    let hung = identity("hung.txt", 5);
    submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &hung);
    clock.advance(TEXT_PREVIEW_TIMEOUT);
    assert_eq!(
        pipeline.request(&ctx, &hung),
        TextPreviewPoll::Failed(PreviewError::TimedOut)
    );

    // The next file waits behind the read that never returns.
    let next = identity("next.txt", 5);
    assert_eq!(pipeline.request(&ctx, &next), TextPreviewPoll::Loading);
    clock.advance(TEXT_PREVIEW_DEBOUNCE);
    assert_eq!(pipeline.request(&ctx, &next), TextPreviewPoll::Loading);
    assert_eq!(
        pipeline.load_state(&next.path),
        super::TextPreviewLoadState::Waiting
    );
    clock.advance(TEXT_PREVIEW_TIMEOUT - Duration::from_millis(1));
    assert_eq!(pipeline.request(&ctx, &next), TextPreviewPoll::Loading);

    clock.advance(Duration::from_millis(1));
    assert_eq!(
        pipeline.request(&ctx, &next),
        TextPreviewPoll::Failed(PreviewError::WorkerBusy)
    );
    assert_eq!(executor.pending_count(), 0);
}

#[test]
fn timeout_copy_avoids_internal_jargon() {
    for message in [
        PreviewError::TimedOut.message(),
        PreviewFailure::TimedOut.message(),
        PreviewError::WorkerBusy.message(),
    ] {
        assert!(
            !message.contains("provider") && !message.contains("budget"),
            "{message}"
        );
        assert!(message.contains("Retry"), "{message}");
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

fn unicode_fixture(text: &str, unit: usize, little: bool) -> Vec<u8> {
    let mut bytes = match (unit, little) {
        (2, true) => vec![0xff, 0xfe],
        (2, false) => vec![0xfe, 0xff],
        (4, true) => vec![0xff, 0xfe, 0, 0],
        (4, false) => vec![0, 0, 0xfe, 0xff],
        _ => unreachable!(),
    };
    if unit == 2 {
        for value in text.encode_utf16() {
            bytes.extend_from_slice(&if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            });
        }
    } else {
        for value in text.chars().map(u32::from) {
            bytes.extend_from_slice(&if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            });
        }
    }
    bytes
}

#[test]
fn unicode_boms_decode_both_byte_orders_and_preserve_text_exactly() {
    let text = "ASCII / café / ёж / 日本語 / 🦀\r\n\tline\n\u{feff}inside";
    for unit in [2, 4] {
        for little in [true, false] {
            assert_eq!(
                decode_text_bytes(unicode_fixture(text, unit, little))
                    .unwrap()
                    .as_ref(),
                text
            );
            assert_eq!(
                decode_text_bytes(unicode_fixture("", unit, little))
                    .unwrap()
                    .as_ref(),
                ""
            );
            // Only the first signature is consumed; a second U+FEFF is content.
            assert_eq!(
                decode_text_bytes(unicode_fixture("\u{feff}second", unit, little))
                    .unwrap()
                    .as_ref(),
                "\u{feff}second"
            );
        }
    }
}

#[test]
fn malformed_unicode_is_rejected_without_replacement_characters() {
    for bytes in [
        vec![0xff, 0xfe, 0x61],
        vec![0xfe, 0xff, 0],
        vec![0xff, 0xfe, 0x00, 0xd8],          // isolated high surrogate
        vec![0xfe, 0xff, 0xdc, 0x00],          // isolated low surrogate
        vec![0xff, 0xfe, 0, 0, 0x61],          // partial UTF-32 unit
        vec![0, 0, 0xfe, 0xff, 0, 0, 0xd8, 0], // surrogate scalar
        vec![0xff, 0xfe, 0, 0, 0, 0, 0x11, 0], // scalar > U+10FFFF
    ] {
        assert_eq!(decode_text_bytes(bytes), Err(PreviewError::InvalidUnicode));
    }
}

#[test]
fn unicode_binary_policy_and_decoded_output_budget_are_retained() {
    for unit in [2, 4] {
        for little in [true, false] {
            assert_eq!(
                decode_text_bytes(unicode_fixture("a\0b", unit, little)),
                Err(PreviewError::Binary)
            );
            assert_eq!(
                decode_text_bytes(unicode_fixture("\u{1}\u{2}a", unit, little)),
                Err(PreviewError::Binary)
            );
        }
    }
    let limit = super::MAX_TEXT_PREVIEW_BYTES as usize;
    let oversized_output = unicode_fixture(&"漢".repeat(limit / 3 + 1), 2, true);
    assert!(oversized_output.len() < limit);
    assert_eq!(
        decode_text_bytes(oversized_output),
        Err(PreviewError::TooLarge)
    );
    let mut lossy = vec![b'a'; limit];
    lossy[limit - 1] = 0xff;
    assert_eq!(decode_text_bytes(lossy), Err(PreviewError::TooLarge));
    let exact = "漢".repeat(limit / 3) + "a";
    assert_eq!(exact.len(), limit);
    assert_eq!(
        decode_text_bytes(unicode_fixture(&exact, 2, false))
            .unwrap()
            .len(),
        limit
    );
    assert_eq!(
        decode_text_bytes(vec![b'a'; limit + 1]),
        Err(PreviewError::TooLarge)
    );
    assert_eq!(
        decode_text_bytes(vec![b'a', 0, b'b', 0]),
        Err(PreviewError::Binary)
    );
}

#[test]
fn office_preview_provider_uses_identity_guards_cancellation_and_distinct_input_budget() {
    let temp = TempDir::new();
    let provider = FsTextPreviewProvider::default();
    for docx in [true, false] {
        let path = temp
            .path()
            .join(if docx { "sample.docx" } else { "sample.odt" });
        std::fs::write(&path, crate::office_preview::fixture(docx)).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let identity = PreviewIdentity {
            path: path.clone(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        };
        let stamp = super::TextPreviewProvider::fingerprint(&provider, &identity).unwrap();
        assert_eq!(
            super::TextPreviewProvider::read(&provider, &identity, &stamp, &|| false)
                .unwrap()
                .as_ref(),
            "Office preview\nmessage: café / ёж 🌍\nA\tB\n"
        );
        assert_eq!(
            super::TextPreviewProvider::read(&provider, &identity, &stamp, &|| true),
            Err(PreviewError::Cancelled)
        );
        std::fs::write(&path, b"changed").unwrap();
        assert_eq!(
            super::TextPreviewProvider::read(&provider, &identity, &stamp, &|| false),
            Err(PreviewError::Changed)
        );
        assert_eq!(
            super::text_input_limit(&path),
            crate::office_preview::MAX_PACKAGE_BYTES
        );
    }
    assert_eq!(
        super::text_input_limit(std::path::Path::new("ordinary.txt")),
        MAX_TEXT_PREVIEW_BYTES
    );
}

#[test]
fn book_collection_cache_retains_typed_owner_and_counts_all_text_and_names() {
    let mut store = TextPreviewStore::new(2, 32);
    let key = fingerprint("book.fb2.zip", 1, 2);
    let books = Arc::new(crate::fictionbook_preview::BookCollection {
        books: vec![
            crate::fictionbook_preview::Book {
                name: "one.fb2".into(),
                text: Arc::from("hello"),
            },
            crate::fictionbook_preview::Book {
                name: "two.fb2".into(),
                text: Arc::from("world"),
            },
        ],
    });
    store.insert_document(
        key.clone(),
        super::TextPreviewDocument::Books(Arc::clone(&books)),
    );
    assert_eq!(store.total_bytes, 24);
    let Some(super::TextPreviewDocument::Books(cached)) = store.get_document(&key) else {
        panic!("typed collection was lost")
    };
    assert!(Arc::ptr_eq(&books, &cached));
    store.insert(fingerprint("other.txt", 1, 20), Arc::from("x".repeat(20)));
    assert!(store.get_document(&key).is_none());
    assert_eq!(books.books[1].text.as_ref(), "world");
}

#[test]
fn filesystem_provider_publishes_multi_book_document_under_one_fingerprint() {
    use std::io::{Cursor, Write};
    let temp = TempDir::new();
    let path = temp.path().join("books.fb2.zip");
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["one.fb2", "two.fb2"] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&crate::fictionbook_preview::fixture())
            .unwrap();
    }
    std::fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    let identity = PreviewIdentity {
        path: path.clone(),
        size: meta.len(),
        modified: meta.modified().ok(),
    };
    let provider = FsTextPreviewProvider::default();
    let stamp = super::TextPreviewProvider::fingerprint(&provider, &identity).unwrap();
    let super::TextPreviewDocument::Books(books) =
        super::TextPreviewProvider::read_document(&provider, &identity, &stamp, &|| false).unwrap()
    else {
        panic!("book choice was discarded")
    };
    assert_eq!(books.books.len(), 2);
    assert_eq!(books.books[1].name, "two.fb2");
    assert_eq!(
        books.books[1].text.as_ref(),
        crate::fictionbook_preview::FIXTURE_TEXT
    );
    std::fs::write(&path, b"changed source").unwrap();
    assert!(matches!(
        super::TextPreviewProvider::read_document(&provider, &identity, &stamp, &|| false),
        Err(PreviewError::Changed)
    ));
    assert_eq!(
        books.books[1].text.as_ref(),
        crate::fictionbook_preview::FIXTURE_TEXT
    );
}

#[test]
fn zipped_fb2_provider_uses_guarded_snapshot_and_package_budget() {
    use std::io::{Cursor, Write};
    let temp = TempDir::new();
    let path = temp.path().join("book.FB2.ZIP");
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("book.fb2", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&crate::fictionbook_preview::fixture())
        .unwrap();
    std::fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let identity = PreviewIdentity {
        path: path.clone(),
        size: metadata.len(),
        modified: metadata.modified().ok(),
    };
    let provider = FsTextPreviewProvider::default();
    let stamp = super::TextPreviewProvider::fingerprint(&provider, &identity).unwrap();
    assert_eq!(
        super::TextPreviewProvider::read(&provider, &identity, &stamp, &|| false)
            .unwrap()
            .as_ref(),
        crate::fictionbook_preview::FIXTURE_TEXT
    );
    assert_eq!(
        super::text_input_limit(&path),
        crate::office_preview::MAX_PACKAGE_BYTES
    );
    assert_eq!(
        super::TextPreviewProvider::read(&provider, &identity, &stamp, &|| true),
        Err(PreviewError::Cancelled)
    );
    std::fs::write(&path, b"changed source").unwrap();
    assert_eq!(
        super::TextPreviewProvider::read(&provider, &identity, &stamp, &|| false),
        Err(PreviewError::Changed)
    );
}

#[test]
fn rtf_preview_provider_extracts_text_after_identity_checks() {
    let temp = TempDir::new();
    let path = temp.path().join("book.RTF");
    std::fs::write(&path, crate::rtf_preview::fixture()).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let identity = PreviewIdentity {
        path: path.clone(),
        size: metadata.len(),
        modified: metadata.modified().ok(),
    };
    let provider = FsTextPreviewProvider::default();
    let stamp = super::TextPreviewProvider::fingerprint(&provider, &identity).unwrap();
    assert_eq!(
        super::TextPreviewProvider::read(&provider, &identity, &stamp, &|| false)
            .unwrap()
            .as_ref(),
        crate::rtf_preview::FIXTURE_TEXT
    );
    assert_eq!(
        super::text_input_limit(&path),
        crate::rtf_preview::MAX_INPUT_BYTES as u64
    );
    assert_eq!(
        super::TextPreviewProvider::read(&provider, &identity, &stamp, &|| true),
        Err(PreviewError::Cancelled)
    );
    std::fs::write(&path, b"changed source").unwrap();
    assert_eq!(
        super::TextPreviewProvider::read(&provider, &identity, &stamp, &|| false),
        Err(PreviewError::Changed)
    );
}

#[test]
fn filesystem_provider_decodes_unicode_on_the_existing_bounded_read_path() {
    let temp = TempDir::new();
    let provider = FsTextPreviewProvider::default();
    for unit in [2, 4] {
        for little in [true, false] {
            let path = temp.path().join(format!("unicode-{unit}-{little}.txt"));
            let text = "Hello café / ёж / 🦀\n";
            std::fs::write(&path, unicode_fixture(text, unit, little)).unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            let request = PreviewIdentity {
                path,
                size: metadata.len(),
                modified: metadata.modified().ok(),
            };
            let expected = super::TextPreviewProvider::fingerprint(&provider, &request).unwrap();
            assert_eq!(
                super::TextPreviewProvider::read(&provider, &request, &expected, &|| false)
                    .unwrap()
                    .as_ref(),
                text
            );
        }
    }
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
        decode_text_bytes(vec![0xff, 0xfe, b'a', 0])
            .unwrap()
            .as_ref(),
        "a"
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
    // One stray CP1252 byte in a page of UTF-8 text still previews.
    let mut stray = "line of plain text\n".repeat(20).into_bytes();
    stray.extend_from_slice(b"caf\xe9\n");
    let text = decode_text_bytes(stray).unwrap();
    assert!(text.ends_with("caf\u{fffd}\n"));
    // A file in a legacy encoding throughout is still rejected.
    assert_eq!(
        decode_text_bytes(b"caf\xe9 cr\xe8me br\xfbl\xe9e".repeat(20)),
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
    let expected = super::TextPreviewProvider::fingerprint(provider.as_ref(), &request).unwrap();
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

#[cfg(unix)]
#[test]
fn symlinked_text_file_fingerprints_and_reads_its_target() {
    let temp = TempDir::new();
    let target = temp.file("target.txt", "linked text");
    let link = temp.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let metadata = std::fs::metadata(&link).unwrap();
    let request = PreviewIdentity {
        path: link,
        size: metadata.len(),
        modified: metadata.modified().ok(),
    };
    let provider = FsTextPreviewProvider::default();

    let expected = super::TextPreviewProvider::fingerprint(&provider, &request).unwrap();
    assert_eq!(
        super::TextPreviewProvider::read(&provider, &request, &expected, &|| false)
            .unwrap()
            .as_ref(),
        "linked text"
    );
}

#[test]
fn ansi_colored_text_is_not_rejected_as_binary() {
    let listing = "\x1b[01;34mdir\x1b[0m\n".repeat(8);
    assert_eq!(
        decode_text_bytes(listing.clone().into_bytes())
            .unwrap()
            .as_ref(),
        listing
    );
}

#[test]
fn interrupted_reads_are_retried() {
    struct InterruptOnce {
        interrupted: bool,
        data: std::io::Cursor<Vec<u8>>,
    }
    impl std::io::Read for InterruptOnce {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            self.data.read(buf)
        }
    }
    let mut reader = InterruptOnce {
        interrupted: false,
        data: std::io::Cursor::new(b"hello".to_vec()),
    };
    assert_eq!(
        super::read_preview_bytes(&mut reader, 5, &|| false, None).unwrap(),
        b"hello"
    );
}

#[test]
fn empty_and_capacity_errors_get_accurate_failure_kinds() {
    assert_eq!(
        classify_failure("image has zero dimensions"),
        PreviewFailure::Decode
    );
    assert_eq!(
        classify_failure("ImageIO: video frame has zero dimensions; standard decoder: x"),
        PreviewFailure::Decode
    );
    assert_eq!(
        classify_failure("image dimensions exceed the preview limit"),
        PreviewFailure::TooLarge
    );
    assert_eq!(
        classify_failure("preview deferred under battery/thermal pressure"),
        PreviewFailure::Busy
    );
    assert_eq!(
        classify_failure("preview provider thread could not start: EAGAIN"),
        PreviewFailure::Busy
    );
    assert_eq!(
        classify_failure(
            "preview format is temporarily quarantined after repeated decode failures"
        ),
        PreviewFailure::Quarantined
    );
}

#[test]
fn changed_text_preview_tells_the_user_to_refresh_first() {
    assert!(
        PreviewError::Changed
            .message()
            .contains("Refresh the folder")
    );
}

#[test]
fn video_failures_name_the_cause_and_spot_unsupported_formats() {
    let unsupported = super::video_failure_message(
        std::path::Path::new("clip.MKV"),
        Some((
            "AVFoundationErrorDomain",
            -11800,
            "The operation could not be completed",
        )),
    );
    assert!(
        unsupported.contains("could not be completed"),
        "{unsupported}"
    );
    assert_eq!(classify_failure(&unsupported), PreviewFailure::Unsupported);

    let unrecognized = super::video_failure_message(
        std::path::Path::new("clip.mp4"),
        Some(("AVFoundationErrorDomain", -11828, "Cannot Open")),
    );
    assert_eq!(classify_failure(&unrecognized), PreviewFailure::Unsupported);

    let damaged = super::video_failure_message(
        std::path::Path::new("clip.mov"),
        Some(("AVFoundationErrorDomain", -11829, "Cannot Open")),
    );
    assert!(damaged.contains("-11829"), "{damaged}");
    assert_eq!(classify_failure(&damaged), PreviewFailure::Decode);

    assert_eq!(
        classify_failure(&super::video_failure_message(
            std::path::Path::new("clip.webm"),
            None
        )),
        PreviewFailure::Unsupported
    );
}

#[test]
fn capacity_refusals_do_not_trip_the_decoder_breaker() {
    assert!(!super::counts_against_decoder(
        "preview providers are still busy after an earlier timeout"
    ));
    assert!(!super::counts_against_decoder(
        "preview deferred under battery/thermal pressure"
    ));
    assert!(super::counts_against_decoder("preview provider timed out"));
    assert!(super::counts_against_decoder("invalid checksum"));
}

#[test]
fn busy_failures_expire_but_decode_failures_stay() {
    let cache = ImageCache::new();
    let now = Instant::now();
    let busy = PathBuf::from("busy.png");
    let broken = PathBuf::from("broken.png");
    {
        let mut failed = crate::lock_util::recover(&cache.failed);
        failed.insert(
            busy.clone(),
            super::FailedLoad::new(PreviewFailure::Busy, now),
        );
        failed.insert(
            broken.clone(),
            super::FailedLoad::new(PreviewFailure::Decode, now),
        );
    }

    cache.purge_expired_failures(now);
    assert_eq!(cache.stats().failed, 2);

    cache.purge_expired_failures(now + super::TRANSIENT_FAILURE_RETRY);
    let failed = crate::lock_util::recover(&cache.failed);
    assert!(!failed.contains_key(&busy));
    assert!(failed.contains_key(&broken));
}

#[test]
fn mixed_case_video_extensions_are_recognized() {
    assert!(super::is_undecodable_standard_ext(std::path::Path::new(
        "clip.Mov"
    )));
    assert!(super::is_undecodable_standard_ext(std::path::Path::new(
        "logo.Svg"
    )));
    assert!(!super::is_undecodable_standard_ext(std::path::Path::new(
        "photo.Png"
    )));
}

#[test]
fn stale_generation_cannot_release_or_fill_a_newer_slot() {
    let pending: super::PendingMap = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let path = PathBuf::from("same.png");
    crate::lock_util::recover(&pending).insert(
        path.clone(),
        super::PendingLoad {
            generation: 2,
            decoded: None,
        },
    );
    let decoded = || super::DecodedPreview {
        image: egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK]),
        byte_size: 4,
        decoded_for: PreviewTarget::DEFAULT,
    };

    assert!(!super::release_pending(&pending, &path, 1));
    assert!(!super::publish_pending(&pending, &path, 1, decoded()));
    assert!(
        crate::lock_util::recover(&pending)
            .get(&path)
            .is_some_and(|slot| slot.generation == 2 && slot.decoded.is_none())
    );

    assert!(super::publish_pending(&pending, &path, 2, decoded()));
    assert!(super::release_pending(&pending, &path, 2));
    assert!(crate::lock_util::recover(&pending).is_empty());
}

#[test]
fn decoders_outliving_their_timeout_hold_preload_slots() {
    assert_eq!(super::free_preload_slots(0, 0), super::MAX_PRELOAD_WORKERS);
    assert_eq!(
        super::free_preload_slots(1, 0),
        super::MAX_PRELOAD_WORKERS - 1
    );
    assert_eq!(super::free_preload_slots(0, super::MAX_PRELOAD_WORKERS), 0);
    assert_eq!(
        super::free_preload_slots(1, 3),
        super::MAX_PRELOAD_WORKERS - 3
    );
}

#[test]
fn eviction_drops_other_folders_first_then_least_recently_used() {
    let [a, b, c, other, stale] =
        ["a.png", "b.png", "c.png", "other.png", "stale.png"].map(PathBuf::from);
    let entries = [
        (&a, 10, 3),
        (&b, 10, 1),
        (&c, 10, 2),
        (&other, 5, 9),
        (&stale, 5, 0),
    ];
    let keep: std::collections::HashSet<&PathBuf> = [&a, &b, &c].into_iter().collect();

    // Within budget nothing is evicted, not even entries outside the keep set.
    assert!(super::eviction_order(entries.into_iter(), &keep, 40, 40).is_empty());

    // Every non-kept entry goes even when dropping one would be enough.
    let mut victims = super::eviction_order(entries.into_iter(), &keep, 40, 39);
    victims.sort();
    assert_eq!(victims, vec![other.clone(), stale.clone()]);

    // Still over budget: kept entries go oldest first, and only as many
    // as needed.
    let victims = super::eviction_order(entries.into_iter(), &keep, 40, 15);
    assert_eq!(victims.len(), 4);
    assert_eq!(victims[2..], [b.clone(), c.clone()]);

    // A budget nothing fits under drains the whole cache.
    assert_eq!(
        super::eviction_order(entries.into_iter(), &keep, 40, 0).len(),
        5
    );
}

#[test]
fn removing_an_entry_keeps_the_byte_total_in_sync() {
    let ctx = egui::Context::default();
    let mut cache = ImageCache::new();
    for (name, bytes) in [("a.png", 4), ("b.png", 16)] {
        cache.entries.insert(
            PathBuf::from(name),
            super::CacheEntry {
                texture: ctx.load_texture(
                    name,
                    egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
                    egui::TextureOptions::LINEAR,
                ),
                byte_size: bytes,
                last_used: 0,
                decoded_for: PreviewTarget::DEFAULT,
            },
        );
    }
    cache.total_bytes = 20;

    cache.retry(&PathBuf::from("b.png"));
    assert_eq!(cache.stats().entries, 1);
    assert_eq!(cache.stats().bytes, 4);
    // A path that is not cached leaves the total alone.
    cache.retry(&PathBuf::from("missing.png"));
    assert_eq!(cache.stats().bytes, 4);
}

#[test]
fn oversized_text_fails_at_once_and_the_rest_debounces() {
    let now = Instant::now();
    assert!(matches!(
        super::initial_text_phase(MAX_TEXT_PREVIEW_BYTES + 1, now, MAX_TEXT_PREVIEW_BYTES),
        super::TextPreviewPhase::Failed(PreviewError::TooLarge)
    ));
    // The cap itself is still previewable.
    for size in [0, MAX_TEXT_PREVIEW_BYTES] {
        assert!(matches!(
            super::initial_text_phase(size, now, MAX_TEXT_PREVIEW_BYTES),
            super::TextPreviewPhase::Debouncing { ready_at }
                if ready_at == now + TEXT_PREVIEW_DEBOUNCE
        ));
    }
}

#[test]
fn preview_target_floors_clamps_and_survives_non_finite_extents() {
    let target = |w, h, ppp| {
        let target = PreviewTarget::from_points(w, h, ppp);
        (target.width, target.height)
    };
    assert_eq!(target(300.0, 200.0, 2.0), (600, 400));
    // Fractional pixels round up so the texture never undershoots.
    assert_eq!(target(10.2, 10.0, 1.0), (11, 10));
    // Sub-point extents and scale factors floor at one.
    assert_eq!(target(0.0, 0.5, 2.0), (2, 2));
    assert_eq!(target(100.0, 50.0, 0.5), (100, 50));
    // Huge panes clamp to the decode ceiling on each axis independently.
    assert_eq!(target(100_000.0, 300.0, 2.0), (4_096, 600));
    // NaN or infinite inputs fall back to a single pixel.
    assert_eq!(target(f32::NAN, 10.0, 1.0), (1, 10));
    assert_eq!(target(f32::INFINITY, 10.0, 1.0), (1, 10));
    assert_eq!(target(300.0, 200.0, f32::NAN), (1, 1));
}

#[test]
fn fingerprint_matches_request_only_when_path_size_and_known_mtime_agree() {
    let mtime = std::time::UNIX_EPOCH + Duration::from_secs(1_000);
    let mut stamped = fingerprint("notes.txt", 1, 5);
    stamped.modified = Some(mtime);
    let request = |path: &str, size, modified| PreviewIdentity {
        path: PathBuf::from(path),
        size,
        modified,
    };

    assert!(stamped.matches_request(&request("notes.txt", 5, Some(mtime))));
    // A listing without an mtime trusts path and size alone.
    assert!(stamped.matches_request(&request("notes.txt", 5, None)));
    assert!(!stamped.matches_request(&request(
        "notes.txt",
        5,
        Some(mtime + Duration::from_secs(1))
    )));
    assert!(!stamped.matches_request(&request("notes.txt", 6, Some(mtime))));
    assert!(!stamped.matches_request(&request("other.txt", 5, Some(mtime))));
    // A file whose mtime cannot be read no longer matches a stamped row.
    let unstamped = fingerprint("notes.txt", 1, 5);
    assert!(!unstamped.matches_request(&request("notes.txt", 5, Some(mtime))));
    assert!(unstamped.matches_request(&request("notes.txt", 5, None)));
}

#[test]
fn control_character_threshold_allows_exactly_one_in_twenty() {
    let mut one_in_twenty = b"abcdefghijklmnopqrs".to_vec();
    one_in_twenty.push(0x01);
    assert_eq!(one_in_twenty.len(), 20);
    assert!(decode_text_bytes(one_in_twenty.clone()).is_ok());

    // DEL counts as a control too: two in twenty is over the line.
    let mut two_in_twenty = one_in_twenty.clone();
    two_in_twenty[0] = 0x7f;
    assert_eq!(decode_text_bytes(two_in_twenty), Err(PreviewError::Binary));

    // One control in nineteen bytes is also just over 5 %.
    let one_in_nineteen = one_in_twenty[1..].to_vec();
    assert_eq!(
        decode_text_bytes(one_in_nineteen),
        Err(PreviewError::Binary)
    );
}

#[test]
fn unpremultiply_restores_straight_alpha_and_skips_opaque_and_clear() {
    let mut pixels = [
        // Half-transparent mid grey: 64 / 128 * 255 rounds down to 127.
        64, 64, 64, 128, //
        // Opaque and fully transparent pixels are untouched.
        10, 20, 30, 255, //
        7, 8, 9, 0, //
        // Overshoot from rounding in the premultiplied value clamps.
        200, 1, 0, 100,
    ];
    unpremultiply_rgba(&mut pixels);
    assert_eq!(
        pixels,
        [
            127, 127, 127, 128, 10, 20, 30, 255, 7, 8, 9, 0, 255, 2, 0, 100
        ]
    );
}

#[test]
fn retry_of_a_ready_preview_rereads_instead_of_serving_the_cache() {
    let (mut pipeline, executor, provider, clock, ctx) = pipeline_harness();
    let request = identity("edited.txt", 5);
    provider.push(ScriptedAction::Text("old"));
    provider.push(ScriptedAction::Text("new"));

    submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &request);
    executor.run_all();
    match pipeline.request(&ctx, &request) {
        TextPreviewPoll::Ready(content) => assert_eq!(content.as_ref(), "old"),
        other => panic!("expected ready preview, got {other:?}"),
    }

    pipeline.retry(&request.path);
    submit_after_debounce(&mut pipeline, &executor, &clock, &ctx, &request);
    executor.run_all();
    match pipeline.request(&ctx, &request) {
        TextPreviewPoll::Ready(content) => assert_eq!(content.as_ref(), "new"),
        other => panic!("expected re-read preview, got {other:?}"),
    }
    assert_eq!(provider.reads.load(Ordering::Relaxed), 2);
}

/// Run `probe` until it holds or about two seconds pass.
fn eventually(probe: impl Fn() -> bool) -> bool {
    for _ in 0..400 {
        if probe() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    probe()
}

#[test]
fn isolated_executor_refuses_a_second_job_and_recovers_after_a_panic() {
    let executor = super::IsolatedTextPreviewExecutor::default();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let (saw_cancel_tx, saw_cancel) = std::sync::mpsc::channel();
    let job: TextPreviewJob = Box::new(move |cancelled| {
        let _ = gate.recv();
        let _ = saw_cancel_tx.send(cancelled());
    });
    let first = executor
        .submit(TextPreviewSubmission { generation: 1 }, job)
        .unwrap();

    let refused = executor.submit(TextPreviewSubmission { generation: 2 }, Box::new(|_| {}));
    assert!(matches!(refused, Err(PreviewError::WorkerBusy)));

    assert!(first.cancel(), "first cancel reports the transition");
    assert!(!first.cancel(), "repeat cancel is a no-op");
    drop(release);
    assert_eq!(saw_cancel.recv_timeout(Duration::from_secs(2)), Ok(true));
    assert!(eventually(|| first.is_finished()));

    let panicking = executor
        .submit(
            TextPreviewSubmission { generation: 3 },
            Box::new(|_| panic!("text preview job panic")),
        )
        .unwrap();
    assert!(eventually(|| panicking.is_finished()));
    // The worker slot is free again after the panic.
    let after = executor
        .submit(TextPreviewSubmission { generation: 4 }, Box::new(|_| {}))
        .unwrap();
    assert!(eventually(|| after.is_finished()));
}

#[test]
fn decode_timeout_keeps_the_permit_until_the_decoder_returns() {
    // Holds every global decoder permit for a moment; keep other decode-path
    // tests out while it runs.
    let _permits = super::DECODER_PERMITS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    fn decoded() -> Result<super::DecodedPreview, String> {
        Ok(super::DecodedPreview {
            image: egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            byte_size: 4,
            decoded_for: PreviewTarget::DEFAULT,
        })
    }
    let stats = super::preview_provider_stats;

    assert!(super::run_decode_with_timeout(Duration::from_secs(2), decoded).is_ok());

    // Hung decoders time out but keep their permits until they return.
    let timeouts_before = stats().timeouts;
    let mut releases = Vec::new();
    for _ in 0..super::MAX_PRELOAD_WORKERS {
        let (release, gate) = std::sync::mpsc::channel::<()>();
        releases.push(release);
        let error = super::run_decode_with_timeout(Duration::from_millis(10), move || {
            let _ = gate.recv();
            decoded()
        })
        .unwrap_err();
        assert_eq!(classify_failure(&error), PreviewFailure::TimedOut);
    }
    assert!(stats().timeouts >= timeouts_before + super::MAX_PRELOAD_WORKERS as u64);

    // With every permit held the next decode is refused without running.
    let saturated_before = stats().saturated;
    let ran = Arc::new(AtomicBool::new(false));
    let ran_probe = Arc::clone(&ran);
    let error = super::run_decode_with_timeout(Duration::from_secs(2), move || {
        ran_probe.store(true, Ordering::Release);
        decoded()
    })
    .unwrap_err();
    assert_eq!(classify_failure(&error), PreviewFailure::Busy);
    assert!(!counts_against_decoder(&error));
    assert!(stats().saturated > saturated_before);
    assert!(!ran.load(Ordering::Acquire));

    // Once the hung decoders finish, their permits come back.
    drop(releases);
    assert!(eventually(|| super::run_decode_with_timeout(
        Duration::from_secs(2),
        decoded
    )
    .is_ok()));
}
