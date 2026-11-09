//! macOS `ImageIO` and `AVFoundation` decoders behind the preview loader.

use super::*;

/// Bitmap color space for CoreGraphics draws. egui treats texture bytes as
/// sRGB, so drawing into an explicit sRGB space makes CoreGraphics convert
/// wide-gamut (Display P3) sources instead of passing device values through.
#[cfg(target_os = "macos")]
unsafe fn create_srgb_color_space() -> *mut objc::runtime::Object {
    unsafe extern "C" {
        static kCGColorSpaceSRGB: *const std::ffi::c_void;
        fn CGColorSpaceCreateWithName(name: *const std::ffi::c_void) -> *mut objc::runtime::Object;
    }
    // SAFETY: kCGColorSpaceSRGB is an immutable CoreGraphics constant and the
    // caller releases the returned color space with CGColorSpaceRelease.
    unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) }
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct CGRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn CGImageGetWidth(image: *mut objc::runtime::Object) -> usize;
    fn CGImageGetHeight(image: *mut objc::runtime::Object) -> usize;
    fn CGBitmapContextCreate(
        data: *mut u8,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *mut objc::runtime::Object,
        bitmap_info: u32,
    ) -> *mut objc::runtime::Object;
    fn CGContextDrawImage(
        ctx: *mut objc::runtime::Object,
        rect: CGRect,
        image: *mut objc::runtime::Object,
    );
    fn CGContextRelease(ctx: *mut objc::runtime::Object);
    fn CGColorSpaceRelease(space: *mut objc::runtime::Object);
    fn CGImageRelease(image: *mut objc::runtime::Object);
}

/// An `NSAutoreleasePool` drained when dropped, so every return path of an
/// Objective-C loader (including `?`) releases its autoreleased objects.
#[cfg(target_os = "macos")]
struct AutoreleasePool(*mut objc::runtime::Object);

#[cfg(target_os = "macos")]
impl AutoreleasePool {
    fn new() -> Self {
        use objc::{class, msg_send, sel, sel_impl};
        // SAFETY: `+new` on NSAutoreleasePool takes no arguments; the pool
        // is drained exactly once, by Drop, on the thread that created it.
        Self(unsafe { msg_send![class!(NSAutoreleasePool), new] })
    }
}

#[cfg(target_os = "macos")]
impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        use objc::{msg_send, sel, sel_impl};
        if !self.0.is_null() {
            // SAFETY: the pool came from `new` and has not been drained yet.
            unsafe {
                let _: () = msg_send![self.0, drain];
            }
        }
    }
}

/// Premultiplied RGBA (the only 8-bit RGBA layout CoreGraphics draws) to
/// straight alpha, in place. Opaque and fully transparent pixels are left
/// alone; rounding overshoot clamps to 255.
#[cfg(any(target_os = "macos", test))]
pub(super) fn unpremultiply_rgba(pixels: &mut [u8]) {
    for chunk in pixels.as_chunks_mut::<4>().0 {
        let a = chunk[3] as u16;
        if a > 0 && a < 255 {
            chunk[0] = ((chunk[0] as u16 * 255) / a).min(255) as u8;
            chunk[1] = ((chunk[1] as u16 * 255) / a).min(255) as u8;
            chunk[2] = ((chunk[2] as u16 * 255) / a).min(255) as u8;
        }
    }
}

/// Draw `cg_image` into a straight-alpha sRGB RGBA buffer and release it,
/// on success and on every failure. `what` names the source in the
/// zero-dimension error.
///
/// # Safety
/// `cg_image` must be a valid, owned (+1) `CGImageRef`.
#[cfg(target_os = "macos")]
unsafe fn cgimage_to_rgba(
    cg_image: *mut objc::runtime::Object,
    what: &str,
) -> Result<([usize; 2], Vec<u8>), String> {
    // SAFETY: the caller hands over a valid CGImage; the color space and
    // bitmap context are released here, and the context draws into
    // `pixels`, which outlives it.
    unsafe {
        let draw = || {
            let w = CGImageGetWidth(cg_image);
            let h = CGImageGetHeight(cg_image);
            if w == 0 || h == 0 {
                return Err(format!("{what} has zero dimensions"));
            }
            let (bytes_per_row, mut pixels) = allocate_rgba_pixels(w, h)?;
            let color_space = create_srgb_color_space();
            if color_space.is_null() {
                return Err("sRGB color space could not be created".to_string());
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
                return Err("CGBitmapContextCreate failed".to_string());
            }
            let rect = CGRect {
                x: 0.0,
                y: 0.0,
                w: w as f64,
                h: h as f64,
            };
            CGContextDrawImage(cg_ctx, rect, cg_image);
            CGContextRelease(cg_ctx);
            CGColorSpaceRelease(color_space);
            unpremultiply_rgba(&mut pixels);
            Ok(([w, h], pixels))
        };
        let result = draw();
        CGImageRelease(cg_image);
        result
    }
}

/// Load image via macOS CoreGraphics/ImageIO.
/// Supports: DNG, CR2, NEF, ARW, ORF, RAF, RW2, HEIC, TIFF, and all standard formats.
#[cfg(target_os = "macos")]
pub(super) fn load_via_imageio(
    path: &Path,
    target: PreviewTarget,
) -> Result<DecodedPreview, String> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CString;

    let path_str = path.to_str().ok_or("invalid path")?;
    let c_path = CString::new(path_str).map_err(|e| e.to_string())?;

    unsafe {
        let _pool = AutoreleasePool::new();

        // Create CFURL from path
        let cf_str: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: c_path.as_ptr()];
        if cf_str.is_null() {
            return Err("failed to create NSString".into());
        }
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: cf_str];
        if url.is_null() {
            return Err("failed to create NSURL".into());
        }

        // CGImageSourceCreateWithURL
        type CGImageSourceRef = *mut Object;
        unsafe extern "C" {
            fn CGImageSourceCreateWithURL(
                url: *mut Object,
                options: *mut Object,
            ) -> CGImageSourceRef;
            fn CGImageSourceGetPrimaryImageIndex(source: CGImageSourceRef) -> usize;
            fn CGImageSourceCreateThumbnailAtIndex(
                source: CGImageSourceRef,
                index: usize,
                options: *mut Object,
            ) -> *mut Object;
            fn CFRelease(cf: *mut Object);
        }

        let source = CGImageSourceCreateWithURL(url, std::ptr::null_mut());
        if source.is_null() {
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
            return Err("ImageIO thumbnail options could not be created".into());
        }
        let _: () = msg_send![options, setObject: yes forKey: always_key];
        let _: () = msg_send![options, setObject: yes forKey: transform_key];
        let _: () = msg_send![options, setObject: max_size forKey: size_key];

        // HEIF containers may store the primary image at a non-zero index.
        let primary_index = CGImageSourceGetPrimaryImageIndex(source);
        let cg_image = CGImageSourceCreateThumbnailAtIndex(source, primary_index, options);
        if cg_image.is_null() {
            CFRelease(source);
            return Err("CGImageSourceCreateThumbnailAtIndex failed".into());
        }

        let rgba = cgimage_to_rgba(cg_image, "image");
        CFRelease(source);
        let (size, pixels) = rgba?;

        let (image, byte_size) = color_image_from_rgba(size, pixels)?;
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

/// Copy an `NSString` into Rust; nil or non-UTF-8 strings become empty.
#[cfg(target_os = "macos")]
unsafe fn nsstring_lossy(string: *mut objc::runtime::Object) -> String {
    use objc::{msg_send, sel, sel_impl};
    if string.is_null() {
        return String::new();
    }
    let utf8: *const std::ffi::c_char = unsafe { msg_send![string, UTF8String] };
    if utf8.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(utf8) }
        .to_string_lossy()
        .into_owned()
}

/// Containers `AVFoundation` cannot open at all; a failure on them is an
/// unsupported format, not a damaged file.
const AVFOUNDATION_UNSUPPORTED: &[&str] = &["mkv", "webm", "wmv", "flv"];

/// Video thumbnail frame times, in order of preference: 1 s skips black
/// lead-in frames, 0 s covers clips shorter than that.
#[cfg(target_os = "macos")]
const VIDEO_THUMBNAIL_SECONDS: [i64; 2] = [1, 0];

/// `AVErrorFileFormatNotRecognized` and `AVErrorDecoderNotFound`.
const AV_UNSUPPORTED_CODES: &[i64] = &[-11828, -11833];

/// The error for a failed video frame grab, from `AVFoundation`'s `NSError`
/// (domain, code, localized description) when it gave one. Wording that
/// [`classify_failure`] reads as unsupported is used for containers and
/// codes `AVFoundation` cannot handle, so the preview does not claim the
/// file is damaged.
#[cfg(any(target_os = "macos", test))]
pub(super) fn video_failure_message(path: &Path, error: Option<(&str, i64, &str)>) -> String {
    let unsupported_container = has_extension_in(path, AVFOUNDATION_UNSUPPORTED);
    match error {
        Some((domain, code, description)) => {
            if unsupported_container
                || (domain == "AVFoundationErrorDomain" && AV_UNSUPPORTED_CODES.contains(&code))
            {
                format!("unsupported video format: {description} ({domain} {code})")
            } else {
                format!("video frame could not be read: {description} ({domain} {code})")
            }
        }
        None if unsupported_container => "unsupported video format".to_string(),
        None => "video frame could not be read".to_string(),
    }
}

/// Extract first frame from video via `AVFoundation`'s `AVAssetImageGenerator`.
#[cfg(target_os = "macos")]
pub(super) fn load_video_thumbnail(
    path: &Path,
    target: PreviewTarget,
) -> Result<DecodedPreview, String> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CString;

    let path_str = path.to_str().ok_or("invalid path")?;
    let c_path = CString::new(path_str).map_err(|e| e.to_string())?;

    unsafe {
        let _pool = AutoreleasePool::new();

        // NSURL
        let ns_str: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: c_path.as_ptr()];
        // `fileURLWithPath:` raises an ObjC exception on nil, which aborts
        // the process across the FFI boundary.
        if ns_str.is_null() {
            return Err("failed to create NSString".into());
        }
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: ns_str];
        if url.is_null() {
            return Err("failed to create NSURL".into());
        }

        // AVAsset
        let asset: *mut Object = msg_send![class!(AVAsset), assetWithURL: url];
        if asset.is_null() {
            return Err("AVAsset creation failed".into());
        }

        // AVAssetImageGenerator
        let generator: *mut Object = msg_send![class!(AVAssetImageGenerator), alloc];
        let generator: *mut Object = msg_send![generator, initWithAsset: asset];
        if generator.is_null() {
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

        // Grab the frame at 1 s, past black lead-in frames. A clip shorter
        // than that can yield no image there, so fall back to the first
        // frame before reporting a failure.
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct CMTime {
            value: i64,
            timescale: i32,
            flags: u32,
            epoch: i64,
        }
        let at_seconds = |value: i64| CMTime {
            value,
            timescale: 1,
            // kCMTimeFlags_Valid
            flags: 1,
            epoch: 0,
        };
        let mut actual_time = CMTime {
            value: 0,
            timescale: 0,
            flags: 0,
            epoch: 0,
        };
        let mut error: *mut Object = std::ptr::null_mut();
        let mut cg_image: *mut Object = std::ptr::null_mut();
        for seconds in VIDEO_THUMBNAIL_SECONDS {
            // copyCGImageAtTime:actualTime:error:
            error = std::ptr::null_mut();
            cg_image = msg_send![generator,
                copyCGImageAtTime: at_seconds(seconds)
                actualTime: &mut actual_time as *mut CMTime
                error: &mut error as *mut *mut Object
            ];
            if !cg_image.is_null() || has_extension_in(path, AVFOUNDATION_UNSUPPORTED) {
                break;
            }
        }

        let _: () = msg_send![generator, release];

        if cg_image.is_null() {
            // The NSError is autoreleased: read it while the pool is alive.
            let failure = if error.is_null() {
                video_failure_message(path, None)
            } else {
                let domain: *mut Object = msg_send![error, domain];
                let code: isize = msg_send![error, code];
                let description: *mut Object = msg_send![error, localizedDescription];
                video_failure_message(
                    path,
                    Some((
                        &nsstring_lossy(domain),
                        code as i64,
                        &nsstring_lossy(description),
                    )),
                )
            };
            return Err(failure);
        }

        let (size, pixels) = cgimage_to_rgba(cg_image, "video frame")?;
        let (image, byte_size) = color_image_from_rgba(size, pixels)?;
        Ok(DecodedPreview {
            image,
            byte_size,
            decoded_for: target,
        })
    }
}
