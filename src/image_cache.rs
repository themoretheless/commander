use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) const MAX_CACHE_BYTES: usize = 1024 * 1024 * 1024; // 1 GB
const MAX_DECODED_IMAGE_BYTES: usize = 256 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 32_768;
const MAX_PRELOAD_WORKERS: usize = 4;

struct CacheEntry {
    texture: TextureHandle,
    byte_size: usize,
    last_used: u64, // frame counter
}

/// A decoded image plus its GPU byte size, or `None` while still loading.
type PendingLoad = Option<(ColorImage, usize)>;
/// Background-load slots shared with the worker threads, keyed by path.
type PendingMap = Arc<Mutex<HashMap<PathBuf, PendingLoad>>>;
type FailedMap = Arc<Mutex<HashMap<PathBuf, PreviewFailure>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewFailure {
    TooLarge,
    Unreadable,
    Unsupported,
    Decode,
}

impl PreviewFailure {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "This image exceeds the preview memory limit.",
            Self::Unreadable => "Commander cannot read this file.",
            Self::Unsupported => "This image format is not supported.",
            Self::Decode => "The image is incomplete or damaged.",
        }
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
        }
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
                if let Some(Some((image, byte_size))) = pending.remove(&path) {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let texture = ctx.load_texture(&name, image, TextureOptions::LINEAR);
                    self.total_bytes = self.total_bytes.saturating_add(byte_size);
                    self.entries.insert(
                        path,
                        CacheEntry {
                            texture,
                            byte_size,
                            last_used: self.frame,
                        },
                    );
                }
            }
        }

        let failed = crate::lock_util::recover(&self.failed).clone();
        let mut available_slots =
            MAX_PRELOAD_WORKERS.saturating_sub(crate::lock_util::recover(&self.pending).len());
        for path in paths {
            if self.entries.contains_key(path) {
                if let Some(entry) = self.entries.get_mut(path) {
                    entry.last_used = self.frame;
                }
                continue;
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
                    let result = load_image_from_disk(&path_clone);
                    match result {
                        Ok((img, byte_size)) => {
                            let mut pending = crate::lock_util::recover(&pending_clone);
                            if !cancelled() {
                                pending.insert(path_clone.clone(), Some((img, byte_size)));
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
        }
    }
}

fn classify_failure(error: &str) -> PreviewFailure {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("too large")
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
fn allocate_rgba_pixels(width: usize, height: usize) -> Result<(usize, Vec<u8>), String> {
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
        rgba.chunks_exact(4)
            .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])),
    );
    Ok((ColorImage::new(size, pixels), byte_size))
}

fn load_image_from_disk(path: &Path) -> Result<(ColorImage, usize), String> {
    // Video — extract first frame via AVFoundation
    #[cfg(target_os = "macos")]
    if is_video_ext(path) {
        return load_video_thumbnail(path);
    }

    // Try macOS ImageIO first (supports DNG, CR2, NEF, ARW, HEIC, etc.)
    #[cfg(target_os = "macos")]
    if let Ok(result) = load_via_imageio(path) {
        return Ok(result);
    }

    load_via_image_crate(path)
}

/// Stream a standard image decoder from the file instead of retaining a second
/// full compressed copy beside the decoded pixels.
fn load_via_image_crate(path: &Path) -> Result<(ColorImage, usize), String> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES as u64);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?;
    let rgba = img.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();
    color_image_from_rgba(size, pixels)
}

/// Load image via macOS CoreGraphics/ImageIO.
/// Supports: DNG, CR2, NEF, ARW, ORF, RAF, RW2, HEIC, TIFF, and all standard formats.
#[cfg(target_os = "macos")]
fn load_via_imageio(path: &Path) -> Result<(ColorImage, usize), String> {
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
            fn CGImageSourceCreateImageAtIndex(
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

        let cg_image = CGImageSourceCreateImageAtIndex(source, 0, std::ptr::null_mut());
        if cg_image.is_null() {
            CFRelease(source);
            let _: () = msg_send![pool, drain];
            return Err("CGImageSourceCreateImageAtIndex failed".into());
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

        color_image_from_rgba([w, h], pixels)
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
fn load_video_thumbnail(path: &Path) -> Result<(ColorImage, usize), String> {
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

        color_image_from_rgba([w, h], pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ImageCache, MAX_DECODED_IMAGE_BYTES, PreviewFailure, allocate_rgba_pixels,
        classify_failure, color_image_from_rgba, load_via_image_crate,
    };
    use crate::testutil::TempDir;

    #[test]
    fn rgba_allocation_checks_both_dimension_products() {
        let (row, pixels) = allocate_rgba_pixels(3, 2).unwrap();
        assert_eq!(row, 12);
        assert_eq!(pixels.len(), 24);
        assert!(allocate_rgba_pixels(usize::MAX, 1).is_err());
        assert!(allocate_rgba_pixels(usize::MAX / 4, 5).is_err());
        assert!(allocate_rgba_pixels(MAX_DECODED_IMAGE_BYTES / 4 + 1, 1).is_err());
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

        let (decoded, bytes) = load_via_image_crate(&path).unwrap();
        assert_eq!(decoded.size, [3, 2]);
        assert_eq!(bytes, 3 * 2 * 4);
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
}
