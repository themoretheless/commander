//! `PDFKit` page text extraction; all Objective-C objects remain on one worker.
use std::sync::Arc;
pub const MAX_TEXT: usize = 256 * 1024;

#[cfg(not(target_os = "macos"))]
pub fn extract(_bytes: &[u8], _number: usize) -> Result<(Arc<str>, usize), String> {
    Err("Native PDF text extraction requires macOS".into())
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc::{
        class, msg_send,
        runtime::{Class, Object},
        sel, sel_impl,
    };
    #[link(name = "PDFKit", kind = "framework")]
    unsafe extern "C" {
        #[link_name = "OBJC_CLASS_$_PDFDocument"]
        static PDF_DOCUMENT_CLASS: Class;
    }
    struct Owned(*mut Object);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0, release];
            }
        }
    }
    struct Pool(*mut Object);
    impl Drop for Pool {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0, drain];
            }
        }
    }
    fn own(object: *mut Object, description: &str) -> Result<Owned, String> {
        if object.is_null() {
            Err(format!("Could not read PDF {description}"))
        } else {
            Ok(Owned(object))
        }
    }
    // SAFETY: string is nil or an NSString valid until the returned text is copied.
    unsafe fn copy_string(string: *mut Object) -> Result<Arc<str>, String> {
        if string.is_null() {
            return Ok(Arc::from(""));
        }
        unsafe {
            let units: usize = msg_send![string, length];
            if units > MAX_TEXT {
                return Err("PDF page text exceeds the 256 KiB copy limit".into());
            }
            let length: usize = msg_send![string, lengthOfBytesUsingEncoding: 4usize]; // NSUTF8StringEncoding
            if length > MAX_TEXT {
                return Err("PDF page text exceeds the 256 KiB copy limit".into());
            }
            if length == 0 {
                if units != 0 {
                    return Err("PDF page text could not be converted to UTF-8".into());
                }
                return Ok(Arc::from(""));
            }
            let pointer: *const u8 = msg_send![string, UTF8String];
            if pointer.is_null() {
                return Err("PDF page text could not be converted to UTF-8".into());
            }
            // NSString's length excludes the terminator. Copy that exact length,
            // preserving embedded NULs rather than truncating with CStr.
            let bytes = std::slice::from_raw_parts(pointer, length);
            let text =
                std::str::from_utf8(bytes).map_err(|_| "PDF page text is not valid UTF-8")?;
            Ok(Arc::from(text))
        }
    }
    fn with_document<T>(
        bytes: &[u8],
        work: impl FnOnce(*mut Object, usize) -> Result<T, String>,
    ) -> Result<T, String> {
        // SAFETY: NSData owns a copy of the input; native retention cannot outlive
        // a borrowed Rust buffer. Document is released before data and the pool.
        // Page/string references are borrowed only during this scope. Page bounds
        // are checked before pageAtIndex; no PDFKit object crosses thread boundaries.
        unsafe {
            let pool = Pool(msg_send![class!(NSAutoreleasePool), new]);
            if pool.0.is_null() {
                return Err("Could not create a PDF text autorelease pool".into());
            }
            let data: *mut Object = msg_send![class!(NSData), alloc];
            let data = own(
                msg_send![data, initWithBytes: bytes.as_ptr() length: bytes.len()],
                "input data",
            )?;
            let document: *mut Object = msg_send![&PDF_DOCUMENT_CLASS, alloc];
            let document = own(msg_send![document, initWithData: data.0], "text document")?;
            let locked: bool = msg_send![document.0, isLocked];
            if locked {
                return Err("This PDF is locked; open externally to unlock it".into());
            }
            let copying: bool = msg_send![document.0, allowsCopying];
            if !copying {
                return Err(
                    "This PDF restricts text copying; use Page image or open externally".into(),
                );
            }
            let total: usize = msg_send![document.0, pageCount];
            if total == 0 || total > 100_000 {
                return Err("PDF text page number/count is outside the supported range".into());
            }
            work(document.0, total)
        }
    }
    fn page_text(document: *mut Object, number: usize) -> Result<Arc<str>, String> {
        // The document is owned by with_document on this worker. A per-page pool
        // releases autoreleased text after its exact UTF-8 copy is complete.
        unsafe {
            let pool = Pool(msg_send![class!(NSAutoreleasePool), new]);
            if pool.0.is_null() {
                return Err("Could not create a PDF page text pool".into());
            }
            let page: *mut Object = msg_send![document, pageAtIndex: number - 1];
            if page.is_null() {
                return Err("Could not read this PDF text page".into());
            }
            let string: *mut Object = msg_send![page, string];
            copy_string(string)
        }
    }
    pub fn extract(bytes: &[u8], number: usize) -> Result<(Arc<str>, usize), String> {
        with_document(bytes, |document, total| {
            if number == 0 || number > total {
                return Err("PDF text page number/count is outside the supported range".into());
            }
            Ok((page_text(document, number)?, total))
        })
    }
    pub fn search(
        bytes: &[u8],
        query: &crate::pdf_search::Query,
        cancelled: &dyn Fn() -> bool,
        progress: &std::sync::atomic::AtomicUsize,
    ) -> Result<crate::pdf_search::Report, String> {
        with_document(bytes, |document, total| {
            crate::pdf_search::scan(total, query, cancelled, progress, |number| {
                page_text(document, number)
            })
        })
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn native_document_search_reaches_later_pages_and_honors_cancel_and_invalid_input() {
            let progress = std::sync::atomic::AtomicUsize::new(0);
            let query = crate::pdf_search::Query {
                text: "Second page".into(),
                case_sensitive: true,
                ..Default::default()
            };
            let report =
                search(&crate::pdf_preview::fixture(), &query, &|| false, &progress).unwrap();
            assert_eq!(report.total, 2);
            assert_eq!(
                report.hits,
                [crate::pdf_search::Hit {
                    page: 2,
                    count: 1,
                    limited: false
                }]
            );
            assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), 2);
            assert!(
                search(&crate::pdf_preview::fixture(), &query, &|| true, &progress)
                    .unwrap_err()
                    .contains("cancelled")
            );
            assert!(search(b"invalid PDF", &query, &|| false, &progress).is_err());
        }
        #[test]
        fn native_pdf_extracts_requested_page_and_reports_blank_or_invalid_pages() {
            let bytes = crate::pdf_preview::fixture();
            let (first, total) = extract(&bytes, 1).unwrap();
            assert_eq!(total, 2);
            assert!(first.contains("Commander PDF"), "{first:?}");
            let (second, _) = extract(&bytes, 2).unwrap();
            assert!(second.contains("Second page"), "{second:?}");
            assert!(!second.contains("Commander"));
            let (empty, _) = extract(&crate::pdf_preview::fixture_text(""), 1).unwrap();
            assert!(empty.trim().is_empty());
            assert!(extract(&bytes, 0).is_err());
            assert!(extract(&bytes, 3).is_err());
            assert!(extract(b"not a PDF", 1).is_err());
        }

        #[test]
        fn native_string_copy_preserves_unicode_nul_and_rejects_oversized_output() {
            unsafe {
                let _pool = Pool(msg_send![class!(NSAutoreleasePool), new]);
                for text in [
                    "ёж\0日本語".to_string(),
                    "x".repeat(MAX_TEXT + 1),
                    "é".repeat(MAX_TEXT),
                ] {
                    let string: *mut Object = msg_send![class!(NSString), alloc];
                    let string = own(msg_send![string, initWithBytes: text.as_ptr() length: text.len() encoding: 4usize], "test string").unwrap();
                    let result = copy_string(string.0);
                    if text.len() <= MAX_TEXT {
                        assert_eq!(&*result.unwrap(), text);
                    } else {
                        assert!(result.unwrap_err().contains("256 KiB"));
                    }
                }
            }
        }
    }
}
#[cfg(target_os = "macos")]
pub use native::{extract, search};

#[cfg(not(target_os = "macos"))]
pub fn search(
    _bytes: &[u8],
    _query: &crate::pdf_search::Query,
    _cancelled: &dyn Fn() -> bool,
    _progress: &std::sync::atomic::AtomicUsize,
) -> Result<crate::pdf_search::Report, String> {
    Err("Native PDF text search requires macOS".into())
}
