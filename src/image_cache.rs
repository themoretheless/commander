use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_CACHE_BYTES: usize = 1024 * 1024 * 1024; // 1 GB

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
    current_dir: Option<PathBuf>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            frame: 0,
            pending: Arc::new(Mutex::new(HashMap::new())),
            current_dir: None,
        }
    }

    /// Get cached texture for a path, or None if not loaded yet.
    pub fn get(&mut self, path: &Path) -> Option<&TextureHandle> {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.last_used = self.frame;
            Some(&entry.texture)
        } else {
            None
        }
    }

    /// Get texture, loading synchronously if not cached. For the active preview image.
    pub fn get_or_load_sync(&mut self, ctx: &Context, path: &Path) -> Option<&TextureHandle> {
        let fname = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if self.entries.contains_key(path) {
            return self.get(path);
        }

        // Check if pending load completed
        let from_pending = {
            let mut p = self.pending.lock().unwrap();
            p.remove(path).flatten()
        };

        let loaded = match from_pending {
            Some(ready) => Some(ready),
            None => load_image_from_disk(path).ok(),
        };

        if let Some((img, byte_size)) = loaded {
            let texture = ctx.load_texture(&fname, img, TextureOptions::LINEAR);
            self.total_bytes += byte_size;
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

    /// Request preloading of images around the cursor.
    /// `paths` should be the list of image paths to keep cached (back 10 + forward 50).
    /// `dir` is the current directory — if changed, old cache is flushed.
    pub fn preload(&mut self, ctx: &Context, paths: &[PathBuf], dir: &Path) {
        self.frame += 1;

        // Track directory change: flush the cache so a stale preview from the
        // old folder can never be served (the get() path only checks by path,
        // and two folders can hold same-named-but-different images).
        if self.current_dir.as_deref() != Some(dir) {
            self.current_dir = Some(dir.to_path_buf());
            self.entries.clear();
            self.total_bytes = 0;
        }

        // Start loading any paths not in cache and not already pending
        let pending = Arc::clone(&self.pending);
        for path in paths {
            if self.entries.contains_key(path) {
                // Already cached — update last_used
                if let Some(entry) = self.entries.get_mut(path) {
                    entry.last_used = self.frame;
                }
                continue;
            }

            let already_pending = {
                let p = pending.lock().unwrap();
                p.contains_key(path)
            };
            if already_pending {
                continue;
            }

            // Mark as pending (None = loading)
            {
                let mut p = pending.lock().unwrap();
                p.insert(path.clone(), None);
            }

            let path_clone = path.clone();
            let pending_clone = Arc::clone(&self.pending);
            let ctx_clone = ctx.clone();
            std::thread::spawn(move || {
                let result = load_image_from_disk(&path_clone);
                if let Ok((img, byte_size)) = result {
                    if let Ok(mut p) = pending_clone.lock() {
                        p.insert(path_clone.clone(), Some((img, byte_size)));
                    }
                    ctx_clone.request_repaint();
                } else {
                    // Remove from pending on error
                    if let Ok(mut p) = pending_clone.lock() {
                        p.remove(&path_clone);
                    }
                }
            });
        }

        // Collect completed loads into the cache
        {
            let mut p = self.pending.lock().unwrap();
            let completed: Vec<_> = p
                .iter()
                .filter(|(_, v)| v.is_some())
                .map(|(k, _)| k.clone())
                .collect();

            for path in completed {
                if let Some(Some((img, byte_size))) = p.remove(&path) {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let texture = ctx.load_texture(&name, img, TextureOptions::LINEAR);
                    self.total_bytes += byte_size;
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
                self.total_bytes -= entry.byte_size;
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
                self.total_bytes -= entry.byte_size;
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
    let color_image = ColorImage::from_rgba_unmultiplied(size, &pixels);
    let gpu_size = size[0] * size[1] * 4;
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
        let bytes_per_row = w * 4;
        let mut pixels = vec![0u8; h * bytes_per_row];
        let color_space = CGColorSpaceCreateDeviceRGB();
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

        let gpu_size = w * h * 4;
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

        let bytes_per_row = w * 4;
        let mut pixels = vec![0u8; h * bytes_per_row];
        let color_space = CGColorSpaceCreateDeviceRGB();
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

        let gpu_size = w * h * 4;
        let color_image = ColorImage::from_rgba_unmultiplied([w, h], &pixels);
        Ok((color_image, gpu_size))
    }
}
