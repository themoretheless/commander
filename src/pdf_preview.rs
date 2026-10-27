//! Bounded, read-only PDF page rasterization. Native documents stay on one worker.
use crate::panel::PreviewIdentity;
use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Instant,
};

const MAX_INPUT: u64 = 64 * 1024 * 1024;
const EDGE: usize = 2048;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
pub struct Page {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub number: usize,
    pub total: usize,
}

fn dimensions(width: f64, height: f64) -> Result<[usize; 2], String> {
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width > 1e6
        || height > 1e6
    {
        return Err("PDF page dimensions are invalid or too large".into());
    }
    let scale = EDGE as f64 / width.max(height);
    Ok([
        (width * scale).round().max(1.0) as usize,
        (height * scale).round().max(1.0) as usize,
    ])
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum Mode {
    #[default]
    Image,
    Text,
}
#[derive(Debug)]
enum Content {
    Image(Page),
    Text {
        text: Arc<str>,
        number: usize,
        total: usize,
    },
}

fn load(
    identity: &PreviewIdentity,
    page: usize,
    cancelled: impl Fn() -> bool,
    mode: Mode,
) -> Result<Content, String> {
    if IN_FLIGHT
        .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < 2).then_some(count + 1)
        })
        .is_err()
    {
        return Err("PDF renderers are busy; retry after another preview finishes".into());
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
        }
    }
    let _guard = Guard;
    if cancelled() {
        return Err("PDF preview cancelled".into());
    }
    if identity.size > MAX_INPUT {
        return Err("PDF preview is limited to 64 MiB source files".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(&identity.path)
        .map_err(|error| error.to_string())?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !before.is_file()
        || before.len() != identity.size
        || before.modified().ok() != identity.modified
    {
        return Err("PDF changed since the listing; refresh the panel".into());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        if cancelled() {
            return Err("PDF preview cancelled".into());
        }
        let count = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > MAX_INPUT {
            return Err("PDF grew beyond the 64 MiB preview limit".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let after = file.metadata().map_err(|error| error.to_string())?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("PDF changed while being read; refresh the panel".into());
    }
    if cancelled() {
        return Err("PDF preview cancelled".into());
    }
    let rendered = match mode {
        Mode::Image => Content::Image(rasterize(&bytes, page)?),
        Mode::Text => {
            let (text, total) = crate::pdf_text::extract(&bytes, page)?;
            Content::Text {
                text,
                number: page,
                total,
            }
        }
    };
    if cancelled() {
        return Err("PDF preview cancelled".into());
    }
    Ok(rendered)
}

#[cfg(not(target_os = "macos"))]
fn rasterize(_bytes: &[u8], _page: usize) -> Result<Page, String> {
    Err("Native PDF page previews require macOS; open this PDF in an external viewer".into())
}

#[cfg(target_os = "macos")]
fn rasterize(bytes: &[u8], number: usize) -> Result<Page, String> {
    use std::ffi::c_void;
    type Ref = *mut objc::runtime::Object;
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    #[repr(C)]
    struct Transform {
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        tx: f64,
        ty: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDataProviderCreateWithData(
            info: *mut c_void,
            data: *const c_void,
            size: usize,
            release: Option<unsafe extern "C" fn(*mut c_void, *const c_void, usize)>,
        ) -> Ref;
        fn CGDataProviderRelease(provider: Ref);
        fn CGPDFDocumentCreateWithProvider(provider: Ref) -> Ref;
        fn CGPDFDocumentRelease(document: Ref);
        fn CGPDFDocumentIsUnlocked(document: Ref) -> bool;
        fn CGPDFDocumentGetNumberOfPages(document: Ref) -> usize;
        fn CGPDFDocumentGetPage(document: Ref, number: usize) -> Ref;
        fn CGPDFPageGetBoxRect(page: Ref, box_type: i32) -> Rect;
        fn CGPDFPageGetRotationAngle(page: Ref) -> i32;
        fn CGPDFPageGetDrawingTransform(
            page: Ref,
            box_type: i32,
            rect: Rect,
            rotate: i32,
            preserve: bool,
        ) -> Transform;
        static kCGColorSpaceSRGB: *const c_void;
        fn CGColorSpaceCreateWithName(name: *const c_void) -> Ref;
        fn CGColorSpaceRelease(space: Ref);
        fn CGBitmapContextCreate(
            data: *mut u8,
            width: usize,
            height: usize,
            bits: usize,
            stride: usize,
            space: Ref,
            info: u32,
        ) -> Ref;
        fn CGContextRelease(context: Ref);
        fn CGContextSetRGBFillColor(context: Ref, red: f64, green: f64, blue: f64, alpha: f64);
        fn CGContextFillRect(context: Ref, rect: Rect);
        fn CGContextScaleCTM(context: Ref, x: f64, y: f64);
        fn CGContextConcatCTM(context: Ref, transform: Transform);
        fn CGContextDrawPDFPage(context: Ref, page: Ref);
    }
    struct Owned(Ref, unsafe extern "C" fn(Ref));
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                (self.1)(self.0);
            }
        }
    }
    fn owned(value: Ref, release: unsafe extern "C" fn(Ref), what: &str) -> Result<Owned, String> {
        if value.is_null() {
            Err(format!("Could not create PDF {what}"))
        } else {
            Ok(Owned(value, release))
        }
    }
    if bytes.len() as u64 > MAX_INPUT {
        return Err("PDF input exceeds the preview limit".into());
    }
    // SAFETY: bytes outlive provider and document; all owned CF/CG references
    // are released in reverse order. A page is borrowed only while its document
    // lives. The initialized RGBA buffer outlives its drawing context. No native
    // document or page is shared with another thread.
    unsafe {
        let provider = owned(
            CGDataProviderCreateWithData(
                std::ptr::null_mut(),
                bytes.as_ptr().cast(),
                bytes.len(),
                None,
            ),
            CGDataProviderRelease,
            "data provider",
        )?;
        let document = owned(
            CGPDFDocumentCreateWithProvider(provider.0),
            CGPDFDocumentRelease,
            "document (the file may be damaged)",
        )?;
        if !CGPDFDocumentIsUnlocked(document.0) {
            return Err("This PDF is locked; open it in an external viewer to unlock it".into());
        }
        let total = CGPDFDocumentGetNumberOfPages(document.0);
        if total == 0 || total > 100_000 || number == 0 || number > total {
            return Err("PDF page number/count is outside the supported range".into());
        }
        let page = CGPDFDocumentGetPage(document.0, number);
        if page.is_null() {
            return Err("Could not read this PDF page".into());
        }
        let bounds = CGPDFPageGetBoxRect(page, 1); // kCGPDFCropBox
        let rotation = CGPDFPageGetRotationAngle(page).rem_euclid(180);
        let size = if rotation == 90 {
            dimensions(bounds.h, bounds.w)?
        } else {
            dimensions(bounds.w, bounds.h)?
        };
        let length = size[0]
            .checked_mul(size[1])
            .and_then(|value| value.checked_mul(4))
            .ok_or("PDF raster allocation overflow")?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(length)
            .map_err(|_| "Could not allocate the PDF page raster")?;
        rgba.resize(length, 255);
        let color = owned(
            CGColorSpaceCreateWithName(kCGColorSpaceSRGB),
            CGColorSpaceRelease,
            "sRGB color space",
        )?;
        let context = owned(
            CGBitmapContextCreate(
                rgba.as_mut_ptr(),
                size[0],
                size[1],
                8,
                size[0] * 4,
                color.0,
                1,
            ),
            CGContextRelease,
            "drawing context",
        )?;
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: size[0] as f64,
            h: size[1] as f64,
        };
        CGContextSetRGBFillColor(context.0, 1.0, 1.0, 1.0, 1.0);
        CGContextFillRect(context.0, rect);
        let (width, height) = if rotation == 90 {
            (bounds.h, bounds.w)
        } else {
            (bounds.w, bounds.h)
        };
        let scale = (size[0] as f64 / width).min(size[1] as f64 / height);
        CGContextScaleCTM(context.0, scale, scale);
        let point_rect = Rect {
            x: 0.0,
            y: 0.0,
            w: size[0] as f64 / scale,
            h: size[1] as f64 / scale,
        };
        CGContextConcatCTM(
            context.0,
            CGPDFPageGetDrawingTransform(page, 1, point_rect, 0, true),
        );
        CGContextDrawPDFPage(context.0, page);
        drop(context);
        Ok(Page {
            size,
            rgba,
            number,
            total,
        })
    }
}

struct Run {
    receiver: mpsc::Receiver<Result<Content, String>>,
    cancelled: Arc<AtomicBool>,
    started: Instant,
}
impl Drop for Run {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
fn start(
    identity: PreviewIdentity,
    number: usize,
    ctx: &egui::Context,
    mode: Mode,
) -> Result<Run, String> {
    crate::provider_runtime::activate_builtin_explained(
        "native-preview",
        &crate::provider_runtime::ActivationRequest {
            capability: crate::provider_runtime::ProviderCapability::PreviewImage,
            root: identity.path.parent().unwrap_or(std::path::Path::new("/")),
            extension: Some("pdf"),
            bytes: Some(identity.size),
        },
    )?;
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel = cancelled.clone();
    let repaint = ctx.clone();
    crate::workload::submit(
        crate::workload::TaskSpec::new(
            crate::workload::TaskKind::Preview,
            identity.path.parent().unwrap_or(std::path::Path::new("/")),
            0,
        )
        .priority(crate::workload::Priority::Interactive)
        .estimated_bytes(MAX_INPUT),
        move |token| {
            let result = load(
                &identity,
                number,
                || token.is_cancelled() || cancel.load(Ordering::Acquire),
                mode,
            );
            let _ = sender.send(result);
            repaint.request_repaint();
        },
    )
    .map_err(|error| format!("Could not start PDF preview: {error:?}"))?;
    Ok(Run {
        receiver,
        cancelled,
        started: Instant::now(),
    })
}

#[derive(Default)]
struct Slot {
    identity: Option<PreviewIdentity>,
    number: usize,
    total: usize,
    zoom: f32,
    page_input: String,
    mode: Mode,
    text: Option<(usize, Arc<str>)>,
    texture: Option<(usize, egui::TextureHandle)>,
    run: Option<Run>,
    error: Option<String>,
}
#[derive(Default)]
pub struct Cache {
    slots: [Slot; 2],
}
impl Cache {
    pub fn retain(
        &mut self,
        ctx: &egui::Context,
        left: Option<&PreviewIdentity>,
        right: Option<&PreviewIdentity>,
    ) {
        for ((slot, identity), side) in self
            .slots
            .iter_mut()
            .zip([left, right])
            .zip(["left", "right"])
        {
            if slot.identity.as_ref() != identity {
                *slot = Slot::default();
                let id = egui::Id::new(("pdf-page-text", side));
                ctx.data_mut(|data| {
                    data.remove::<TextLayout>(id);
                    data.remove::<crate::preview_find::PreviewFind>(id.with("find"));
                    data.remove::<(PreviewIdentity, usize)>(id.with("page"));
                });
            }
        }
    }
    #[cfg(any(test, feature = "visual-qa"))]
    pub fn shown_page(&self, side: &str) -> Option<usize> {
        let slot = &self.slots[usize::from(side == "right")];
        match slot.mode {
            Mode::Image => slot.texture.as_ref().map(|(page, _)| *page),
            Mode::Text => slot.text.as_ref().map(|(page, _)| *page),
        }
    }
    #[cfg(feature = "visual-qa")]
    pub fn diagnostic(&self, side: &str) -> String {
        let slot = &self.slots[usize::from(side == "right")];
        format!(
            "page={}/{}, shown={:?}, running={}, error={:?}",
            slot.number,
            slot.total,
            self.shown_page(side),
            slot.run.is_some(),
            slot.error
        )
    }
    #[cfg(feature = "visual-qa")]
    pub fn ready(&self, side: &str) -> bool {
        self.shown_page(side).is_some()
    }
    pub fn render(&mut self, ui: &mut egui::Ui, identity: &PreviewIdentity, side: &str) {
        if !cfg!(target_os = "macos") {
            ui.label("Native PDF page previews require macOS. Use Open externally to view this document.");
            return;
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        let slot = &mut self.slots[usize::from(side == "right")];
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ImagePreview) {
            *slot = Slot::default();
            ui.label("PDF preview is turned off under Developer diagnostics / Runtime controls");
            return;
        }
        if slot.identity.as_ref() != Some(identity) {
            *slot = Slot {
                identity: Some(identity.clone()),
                number: 1,
                zoom: 1.0,
                page_input: "1".into(),
                ..Default::default()
            };
            match start(identity.clone(), 1, ui.ctx(), Mode::Image) {
                Ok(run) => slot.run = Some(run),
                Err(error) => slot.error = Some(error),
            }
        }
        if let Some(run) = &slot.run {
            match run.receiver.try_recv() {
                Ok(Ok(Content::Text {
                    text,
                    number,
                    total,
                })) => {
                    slot.text = Some((number, text));
                    slot.total = total;
                    slot.run = None;
                    slot.error = None;
                }
                Ok(Ok(Content::Image(page))) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied(page.size, &page.rgba);
                    slot.texture = Some((
                        page.number,
                        ui.ctx().load_texture(
                            format!("pdf-{side}"),
                            image,
                            egui::TextureOptions::LINEAR,
                        ),
                    ));
                    slot.total = page.total;
                    slot.run = None;
                    slot.error = None;
                }
                Ok(Err(error)) => {
                    slot.run = None;
                    slot.error = Some(error);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    slot.run = None;
                    slot.error = Some("PDF worker stopped before completing".into());
                }
                Err(mpsc::TryRecvError::Empty) if run.started.elapsed() > TIMEOUT => {
                    slot.run = None;
                    slot.error = Some("PDF preview timed out; retry or open externally".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut requested = None;
        let old_mode = slot.mode;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut slot.mode, Mode::Image, "Page image");
            let text_button = ui.selectable_value(&mut slot.mode, Mode::Text, "Page text");
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new(("pdf-text-mode", side)), text_button.rect)
            });
            text_button.on_hover_text("Search and copy extracted text from this page; no OCR");
        });
        if old_mode != slot.mode {
            slot.run = None;
            slot.error = None;
            let cached = match slot.mode {
                Mode::Image => slot.texture.is_some(),
                Mode::Text => slot.text.is_some(),
            };
            if !cached {
                requested = Some(slot.number);
            }
        }
        ui.horizontal_wrapped(|ui| {
            let previous = ui.add_enabled(
                slot.run.is_none() && slot.number > 1,
                egui::Button::new("Previous page"),
            );
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new(("pdf-previous", side)), previous.rect)
            });
            if previous.clicked() {
                #[cfg(feature = "visual-qa")]
                log::info!("PDF UI Previous accepted at page {}", slot.number);
                requested = Some(slot.number - 1);
            }
            ui.label(format!(
                "Page {} / {}",
                slot.number,
                if slot.total == 0 {
                    "…".into()
                } else {
                    slot.total.to_string()
                }
            ));
            let next = ui.add_enabled(
                slot.run.is_none() && slot.number < slot.total,
                egui::Button::new("Next page"),
            );
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new(("pdf-next", side)), next.rect));
            if next.clicked() {
                #[cfg(feature = "visual-qa")]
                log::info!("PDF UI Next accepted at page {}", slot.number);
                requested = Some(slot.number + 1);
            }
            let field = ui.add_enabled(
                slot.run.is_none() && slot.total > 0,
                egui::TextEdit::singleline(&mut slot.page_input)
                    .id(egui::Id::new(("pdf-page-input", side)))
                    .desired_width(40.0)
                    .char_limit(6)
                    .hint_text("Page"),
            );
            field.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::TextEdit,
                    field.enabled(),
                    "PDF page number",
                )
            });
            if ui
                .add_enabled(
                    slot.run.is_none() && slot.total > 0,
                    egui::Button::new("Go to page"),
                )
                .clicked()
                || (field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
            {
                match slot.page_input.trim().parse::<usize>() {
                    Ok(number) if (1..=slot.total).contains(&number) => requested = Some(number),
                    _ => slot.error = Some(format!("Enter a page number from 1 to {}", slot.total)),
                }
            }
            if ui
                .add_enabled(slot.mode == Mode::Image, egui::Button::new("−"))
                .on_hover_text("Zoom PDF out")
                .clicked()
            {
                slot.zoom = (slot.zoom / 1.25).max(0.25);
            }
            if ui
                .add_enabled(slot.mode == Mode::Image, egui::Button::new("+"))
                .on_hover_text("Zoom PDF in")
                .clicked()
            {
                slot.zoom = (slot.zoom * 1.25).min(4.0);
            }
            if ui
                .add_enabled(slot.mode == Mode::Image, egui::Button::new("Fit"))
                .clicked()
            {
                slot.zoom = 1.0;
            }
            if slot.mode == Mode::Image {
                ui.label(format!("{:.0}% of Fit", slot.zoom * 100.0));
            }
        });
        if let Some(error) = &slot.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Retry PDF page").clicked() {
                requested = Some(slot.number);
            }
        }
        if let Some(number) = requested {
            if number != slot.number {
                slot.texture = None;
                slot.text = None;
            }
            slot.number = number;
            slot.page_input = number.to_string();
            slot.error = None;
            match start(identity.clone(), number, ui.ctx(), slot.mode) {
                Ok(run) => slot.run = Some(run),
                Err(error) => slot.error = Some(error),
            }
        }
        if slot.run.is_some() {
            ui.spinner();
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        if slot.mode == Mode::Text {
            if let Some((number, text)) = &slot.text {
                render_page_text(ui, identity, side, *number, text);
            }
            return;
        }
        if let Some((number, texture)) = &slot.texture {
            ui.small("Raster preview; links, forms and scripts are inactive. Page text offers search and copying.");
            let available = ui.available_size().max(egui::vec2(1.0, 1.0));
            let pixels = texture.size_vec2();
            let scale = (available.x / pixels.x).min(available.y / pixels.y) * slot.zoom;
            egui::ScrollArea::both()
                .id_salt(("pdf-page", side, number))
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add(
                        egui::Image::new((texture.id(), pixels * scale))
                            .alt_text(format!("PDF page {number} of {}", slot.total)),
                    );
                });
        }
    }
}

#[derive(Clone)]
struct TextLayout {
    text: Arc<str>,
    text_scale: f32,
    width: f32,
    ppp: f32,
    color: egui::Color32,
    galley: Arc<egui::Galley>,
}
fn render_page_text(
    ui: &mut egui::Ui,
    identity: &PreviewIdentity,
    side: &str,
    number: usize,
    text: &Arc<str>,
) {
    ui.small("Text of this page only. Reading order depends on the PDF; scanned pages need OCR.");
    if text.trim().is_empty() {
        ui.label("No extractable text on this page.");
        return;
    }
    let copy = ui.button("Copy page text");
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new(("pdf-copy-text", side)), copy.rect));
    if copy.clicked() {
        ui.ctx().copy_text(text.to_string());
    }
    let id = egui::Id::new(("pdf-page-text", side));
    let text_scale = crate::preview_typography::toolbar(ui, id);
    let width = ui.available_width();
    let ppp = ui.ctx().pixels_per_point();
    let color = ui.visuals().text_color();
    let cached = ui.ctx().data(|data| data.get_temp::<TextLayout>(id));
    let galley = match cached {
        Some(cached)
            if Arc::ptr_eq(&cached.text, text)
                && cached.width == width
                && cached.text_scale == text_scale
                && cached.ppp == ppp
                && cached.color == color =>
        {
            cached.galley
        }
        _ => {
            let galley = ui.fonts_mut(|fonts| {
                fonts.layout_job(egui::text::LayoutJob::simple(
                    text.to_string(),
                    egui::FontId::proportional(14.0 * text_scale),
                    color,
                    width,
                ))
            });
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    id,
                    TextLayout {
                        text: text.clone(),
                        text_scale,
                        width,
                        ppp,
                        color,
                        galley: galley.clone(),
                    },
                )
            });
            galley
        }
    };
    let find_id = id.with("find");
    let mut find = crate::preview_find::PreviewFind::toolbar(ui, find_id, &identity.path, &galley);
    let new_page = ui.ctx().data_mut(|data| {
        let key = id.with("page");
        let page = (identity.clone(), number);
        let changed = data.get_temp::<(PreviewIdentity, usize)>(key).as_ref() != Some(&page);
        data.insert_temp(key, page);
        changed
    });
    let mut scroll = egui::ScrollArea::both()
        .id_salt(id.with("scroll"))
        .auto_shrink([false; 2]);
    if new_page {
        scroll = scroll.scroll_offset(egui::Vec2::ZERO);
    }
    scroll.show(ui, |ui| {
        let response = ui.add(egui::Label::new(galley).selectable(true));
        find.decorate(ui, response.rect.min, color);
    });
    find.save(ui, find_id);
}

#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture() -> Vec<u8> {
    fixture_text("Commander PDF")
}

#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture_text(title: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /CropBox [50 60 350 460] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Rotate 90 /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        { let s = "1 0 0 rg 50 410 300 50 re f 0 0 1 rg 50 60 300 50 re f 0 0 0 rg BT /F1 24 Tf 80 330 Td (Commander PDF) Tj ET".replace("Commander PDF", title); format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len()) },
        { let s = "0 0.6 0 rg 0 0 400 300 re f 1 1 1 rg BT /F1 24 Tf 40 150 Td (Second page) Tj ET"; format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len()) },
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in &offsets[1..] {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn page_field_enters_a_real_worker_and_closing_releases_the_cache() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("pages.pdf");
        std::fs::write(&path, fixture()).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let identity = PreviewIdentity {
            path,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        };
        let ctx = egui::Context::default();
        let mut cache = Cache::default();
        let frame = |cache: &mut Cache, enter: bool| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                events: if enter {
                    vec![egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    }]
                } else {
                    vec![]
                },
                ..Default::default()
            };
            crate::testutil::discard_egui_output(
                ctx.run_ui(input, |ui| cache.render(ui, &identity, "right")),
            );
        };
        for number in [1, 2] {
            if number == 2 {
                cache.slots[1].page_input = "2".into();
                ctx.memory_mut(|memory| {
                    memory.request_focus(egui::Id::new(("pdf-page-input", "right")))
                });
                frame(&mut cache, true);
                assert_eq!(cache.slots[1].number, 2);
            }
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            while cache.shown_page("right") != Some(number) {
                assert!(Instant::now() < deadline, "PDF worker did not finish");
                frame(&mut cache, false);
                assert!(cache.slots[1].error.is_none(), "{:?}", cache.slots[1].error);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        cache.slots[1].page_input = "0".into();
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new(("pdf-page-input", "right"))));
        frame(&mut cache, true);
        assert_eq!(cache.shown_page("right"), Some(2));
        assert!(cache.slots[1].error.as_deref().unwrap().contains("1 to 2"));
        cache.retain(&ctx, None, None);
        assert!(cache.shown_page("right").is_none());
        assert!(cache.slots[1].identity.is_none());
    }

    #[test]
    fn page_text_search_and_copy_leave_source_unchanged_and_close_clears_layout() {
        let ctx = egui::Context::default();
        let identity = PreviewIdentity {
            path: "/pages.pdf".into(),
            size: 0,
            modified: None,
        };
        let text: Arc<str> = "Commander PDF\nЁж on this page\n".into();
        let id = egui::Id::new(("pdf-page-text", "right"));
        ctx.data_mut(|data| {
            data.insert_temp(
                id.with("find"),
                crate::preview_find::PreviewFind::with_query("ёж"),
            )
        });
        let mut copied = None;
        for frame in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                ..Default::default()
            };
            if frame > 0 {
                let rect = ctx.data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new(("pdf-copy-text", "right")))
                        .unwrap()
                });
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 1,
                        modifiers: Default::default(),
                    },
                ];
            }
            let output = crate::testutil::take_egui_output(ctx.run_ui(input, |ui| {
                render_page_text(ui, &identity, "right", 1, &text)
            }));
            for command in output.platform_output.commands {
                if let egui::OutputCommand::CopyText(value) = command {
                    copied = Some(value);
                }
            }
        }
        assert_eq!(copied.as_deref(), Some(text.as_ref()));
        let find = ctx.data(|data| {
            data.get_temp::<crate::preview_find::PreviewFind>(id.with("find"))
                .unwrap()
        });
        assert_eq!(find.match_count(), 1);
        assert_eq!(
            ctx.data(|data| data
                .get_temp::<TextLayout>(id)
                .unwrap()
                .galley
                .text()
                .to_owned()),
            text.as_ref()
        );
        {
            let original = ctx.data(|data| data.get_temp::<TextLayout>(id)).unwrap();
            ctx.data_mut(|data| data.insert_temp(id.with("text-percent"), 150u16));
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 480.0),
                    )),
                    ..Default::default()
                },
                |ui| render_page_text(ui, &identity, "right", 1, &text),
            ));
            let enlarged = ctx.data(|data| data.get_temp::<TextLayout>(id)).unwrap();
            assert_eq!(enlarged.text_scale, 1.5);
            assert!(!Arc::ptr_eq(&original.galley, &enlarged.galley));
            assert!(enlarged.galley.size().y > original.galley.size().y);
            assert_eq!(enlarged.galley.text(), text.as_ref());
            assert_eq!(
                ctx.data(|data| data.get_temp::<crate::preview_find::PreviewFind>(id.with("find")))
                    .unwrap()
                    .match_count(),
                1
            );
        }
        let mut cache = Cache::default();
        cache.slots[1] = Slot {
            identity: Some(identity),
            number: 1,
            total: 2,
            mode: Mode::Text,
            text: Some((1, text.clone())),
            ..Default::default()
        };
        cache.retain(&ctx, None, None);
        assert!(ctx.data(|data| data.get_temp::<TextLayout>(id)).is_none());
        assert!(
            ctx.data(|data| data.get_temp::<crate::preview_find::PreviewFind>(id.with("find")))
                .is_none()
        );
        assert!(cache.slots[1].text.is_none());
        assert_eq!(Arc::strong_count(&text), 1);
    }

    #[test]
    fn bounded_dimensions_reject_invalid_input() {
        assert_eq!(dimensions(300.0, 400.0).unwrap(), [1536, 2048]);
        for (w, h) in [
            (0.0, 1.0),
            (f64::NAN, 1.0),
            (1.0, f64::INFINITY),
            (1e7, 1.0),
        ] {
            assert!(dimensions(w, h).is_err());
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_pdf_pages_preserve_crop_rotation_and_upright_pixel_order() {
        let bytes = fixture();
        let first = rasterize(&bytes, 1).unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.size, [1536, 2048]);
        let pixel = |x: usize, y: usize| &first.rgba[(y * first.size[0] + x) * 4..][..4];
        assert_eq!(pixel(10, 10), [255, 0, 0, 255], "red is at the page top");
        assert_eq!(
            pixel(10, 2000),
            [0, 0, 255, 255],
            "blue is at the page bottom"
        );
        let second = rasterize(&bytes, 2).unwrap();
        assert_eq!(second.number, 2);
        assert_eq!(second.size, [1536, 2048]);
        assert_eq!(
            &second.rgba[..4],
            &[0, 153, 0, 255],
            "the rotated page must actually be rendered"
        );
        assert!(rasterize(&bytes, 0).is_err());
        assert!(rasterize(&bytes, 3).is_err());
        assert!(rasterize(b"not a PDF", 1).is_err());
    }
    #[test]
    fn worker_refuses_stale_oversized_and_cancelled_sources() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.file("test.pdf", "original");
        let metadata = std::fs::metadata(&path).unwrap();
        let identity = PreviewIdentity {
            path,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        };
        assert!(
            load(&identity, 1, || true, Mode::Image)
                .unwrap_err()
                .contains("cancelled")
        );
        let mut stale = identity.clone();
        stale.size += 1;
        assert!(
            load(&stale, 1, || false, Mode::Image)
                .unwrap_err()
                .contains("changed")
        );
        stale.size = MAX_INPUT + 1;
        assert!(
            load(&stale, 1, || false, Mode::Image)
                .unwrap_err()
                .contains("64 MiB")
        );
    }
}
