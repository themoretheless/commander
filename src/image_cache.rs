use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) const MAX_CACHE_BYTES: usize = 1024 * 1024 * 1024; // 1 GB
const MAX_PRELOAD_WORKERS: usize = 4;
const FAILED_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

struct CacheEntry {
    texture: TextureHandle,
    byte_size: usize,
    last_used: u64, // frame counter
}

/// A decoded image plus its GPU byte size, or `None` while still loading.
type PendingLoad = Option<(ColorImage, usize)>;
/// Background-load slots shared with the worker threads, keyed by path.
type PendingMap = Arc<Mutex<HashMap<PathBuf, PendingLoad>>>;

pub struct ImageCache {
    entries: HashMap<PathBuf, CacheEntry>,
    total_bytes: usize,
    frame: u64,
    /// Images currently being loaded in background
    pending: PendingMap,
    failed: Arc<Mutex<HashMap<PathBuf, std::time::Instant>>>,
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

    /// Get texture, loading synchronously if not cached. For the active preview image.
    pub fn get_or_load_sync(&mut self, ctx: &Context, path: &Path) -> Option<&TextureHandle> {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ImagePreview) {
            return None;
        }
        let fname = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if self.entries.contains_key(path) {
            return self.get(path);
        }
        {
            let mut failed = crate::lock_util::recover(&self.failed);
            if failed
                .get(path)
                .is_some_and(|when| when.elapsed() < FAILED_RETRY_AFTER)
            {
                return None;
            }
            failed.remove(path);
        }

        // Check if pending load completed
        let from_pending = {
            let mut p = crate::lock_util::recover(&self.pending);
            if p.get(path).is_some_and(Option::is_none) {
                return None;
            }
            p.remove(path).flatten()
        };

        let loaded = match from_pending {
            Some(ready) => Some(ready),
            None => match load_image_from_disk(path) {
                Ok(ready) => Some(ready),
                Err(_) => {
                    crate::lock_util::recover(&self.failed)
                        .insert(path.to_path_buf(), std::time::Instant::now());
                    None
                }
            },
        };

        if let Some((img, byte_size)) = loaded {
            let texture = ctx.load_texture(&fname, img, TextureOptions::LINEAR);
            self.total_bytes = self.total_bytes.saturating_add(byte_size);
            self.entries.insert(
                path.to_path_buf(),
                CacheEntry {
                    texture,
                    byte_size,
                    last_used: self.frame,
                },
            );
        }

        self.get(path)
    }

    /// Request preloading in priority order, with fixed worker concurrency.
    pub fn preload(&mut self, ctx: &Context, paths: &[PathBuf], dir: &Path) {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ImagePreview) {
            let mut pending = crate::lock_util::recover(&self.pending);
            if !pending.is_empty() {
                pending.clear();
                self.generation.fetch_add(1, Ordering::AcqRel);
            }
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

        let failed = {
            let mut failed = crate::lock_util::recover(&self.failed);
            failed.retain(|_, when| when.elapsed() < FAILED_RETRY_AFTER);
            failed.clone()
        };
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
                    if let Ok((img, byte_size)) = result {
                        let mut pending = crate::lock_util::recover(&pending_clone);
                        if !cancelled() {
                            pending.insert(path_clone.clone(), Some((img, byte_size)));
                            drop(pending);
                            ctx_clone.request_repaint();
                        } else {
                            pending.remove(&path_clone);
                        }
                    } else {
                        let mut pending = crate::lock_util::recover(&pending_clone);
                        let mut failed = crate::lock_util::recover(&failed_clone);
                        if !cancelled() {
                            pending.remove(&path_clone);
                            failed.insert(path_clone, std::time::Instant::now());
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

    pub fn stats(&self) -> (usize, usize) {
        (self.entries.len(), self.total_bytes)
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
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(byte_len)
        .map_err(|_| "image pixel buffer allocation failed".to_string())?;
    pixels.resize(byte_len, 0);
    Ok((bytes_per_row, pixels))
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

    // Fallback to image crate (PNG, JPEG, GIF, BMP, WebP)
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&data).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();
    let gpu_size = pixels.len();
    let color_image = ColorImage::from_rgba_unmultiplied(size, &pixels);
    Ok((color_image, gpu_size.max(data.len())))
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

        let gpu_size = pixels.len();
        let color_image = ColorImage::from_rgba_unmultiplied([w, h], &pixels);
        Ok((color_image, gpu_size))
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

        let gpu_size = pixels.len();
        let color_image = ColorImage::from_rgba_unmultiplied([w, h], &pixels);
        Ok((color_image, gpu_size))
    }
}

#[cfg(test)]
mod tests {
    use super::allocate_rgba_pixels;

    #[test]
    fn rgba_allocation_checks_both_dimension_products() {
        let (row, pixels) = allocate_rgba_pixels(3, 2).unwrap();
        assert_eq!(row, 12);
        assert_eq!(pixels.len(), 24);
        assert!(allocate_rgba_pixels(usize::MAX, 1).is_err());
        assert!(allocate_rgba_pixels(usize::MAX / 4, 5).is_err());
    }
}
