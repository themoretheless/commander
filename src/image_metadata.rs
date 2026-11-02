//! Bounded read-only EXIF snapshots. Parsing and all filesystem IO stay on a
//! shared preview worker; each inspector owns an independently cancellable slot.
use crate::panel::{FileEntry, PreviewIdentity};
use std::{
    fmt::Write as _,
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};
const MAX_INPUT: usize = 2 * 1024 * 1024;
const MAX_VALUE: usize = 512;
const TIMEOUT: Duration = Duration::from_secs(10);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    preview: PreviewIdentity,
    object: Option<(u64, u64)>,
    changed: Option<SystemTime>,
}
impl Identity {
    pub fn from_entry(entry: &FileEntry) -> Option<Self> {
        let extension = entry.path.extension()?.to_str()?.to_ascii_lowercase();
        if entry.is_dir
            || entry.is_symlink()
            || !matches!(
                extension.as_str(),
                "jpg"
                    | "jpeg"
                    | "jpe"
                    | "png"
                    | "tif"
                    | "tiff"
                    | "webp"
                    | "heic"
                    | "heif"
                    | "avif"
                    | "dng"
            )
        {
            return None;
        }
        Some(Self {
            preview: PreviewIdentity::from_entry(entry),
            object: entry.filesystem_id(),
            changed: entry.changed,
        })
    }
    fn matches(&self, meta: &std::fs::Metadata) -> bool {
        #[cfg(unix)]
        let object = {
            use std::os::unix::fs::MetadataExt;
            Some((meta.dev(), meta.ino()))
        };
        #[cfg(not(unix))]
        let object = None;
        meta.is_file()
            && meta.len() == self.preview.size
            && meta.modified().ok() == self.preview.modified
            && object == self.object
            && crate::panel::metadata_change_time(meta) == self.changed
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub rows: Vec<(&'static str, String)>,
    pub note: Option<String>,
}
const MAX_EXPORT: usize = 64 * 1024;
fn export_json(identity: &Identity, report: &Report) -> Result<String, String> {
    let path = &identity.preview.path;
    if !path.is_absolute()
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || path.as_os_str().as_encoded_bytes().len() > MAX_EXPORT / 4
    {
        return Err("Metadata JSON needs an absolute native path within the report budget".into());
    }
    if report.rows.len() > 64
        || report
            .rows
            .iter()
            .any(|(name, value)| name.len() > 64 || value.len() > MAX_VALUE + 3)
        || report.note.as_ref().is_some_and(|note| note.len() > 1024)
    {
        return Err("Metadata JSON exceeds the snapshot budget".into());
    }
    let encoded = if let Some(text) = path.to_str() {
        serde_json::json!({"encoding":"utf8","text":text})
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bytes = path.as_os_str().as_bytes();
            let mut hex = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                write!(hex, "{byte:02x}").map_err(|e| e.to_string())?;
            }
            serde_json::json!({"encoding":"unix_bytes_hex","hex":hex})
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            serde_json::json!({"encoding":"windows_utf16","units":path.as_os_str().encode_wide().collect::<Vec<_>>()})
        }
        #[cfg(not(any(unix, windows)))]
        {
            return Err("Native path encoding is unavailable on this platform".into());
        }
    };
    let timestamp = |time: Option<SystemTime>| {
        time.map(|time| match time.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos().to_string(),
            Err(e) => format!("-{}", e.duration().as_nanos()),
        })
    };
    let value = serde_json::json!({"format":"commander.image-metadata","version":1,"snapshot":"retained inspector display values","read_limit_bytes":MAX_INPUT,
        "source":{"path":encoded,"size_bytes":identity.preview.size,"device_id":identity.object.map(|x|x.0),"file_id":identity.object.map(|x|x.1),"modified_unix_ns":timestamp(identity.preview.modified),"changed_unix_ns":timestamp(identity.changed)},
        "properties":report.rows.iter().map(|(name,value)|serde_json::json!({"name":name,"display_value":value})).collect::<Vec<_>>(),"note":report.note});
    struct Output(Vec<u8>);
    impl std::io::Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_EXPORT.saturating_sub(self.0.len()) {
                return Err(std::io::Error::other("Metadata JSON is limited to 64 KiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Output(Vec::new());
    serde_json::to_writer_pretty(&mut output, &value).map_err(|e| e.to_string())?;
    String::from_utf8(output.0).map_err(|e| e.to_string())
}
struct BoundedText(String);
impl std::fmt::Write for BoundedText {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        for c in s.chars() {
            if self.0.len() + c.len_utf8() > MAX_VALUE {
                return Err(std::fmt::Error);
            }
            self.0.push(if c.is_control() { ' ' } else { c });
        }
        Ok(())
    }
}
// Primary encoded dimensions only; no pixel buffers or raster decoder are
// created. Every offset/length is checked against the bounded byte snapshot.
fn header(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    let valid = |format, w, h| (w > 0 && h > 0).then_some((format, w, h));
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        if bytes.len() < 33
            || bytes[8..12] != 13u32.to_be_bytes()
            || &bytes[12..16] != b"IHDR"
            || crc32fast::hash(&bytes[12..29]) != u32::from_be_bytes(bytes[29..33].try_into().ok()?)
        {
            return None;
        }
        return valid(
            "PNG",
            u32::from_be_bytes(bytes[16..20].try_into().ok()?),
            u32::from_be_bytes(bytes[20..24].try_into().ok()?),
        );
    }
    if bytes.starts_with(b"\xff\xd8") {
        let mut p = 2usize;
        while p < bytes.len() {
            if *bytes.get(p)? != 255 {
                return None;
            }
            while *bytes.get(p)? == 255 {
                p += 1;
            }
            let marker = *bytes.get(p)?;
            p += 1;
            if matches!(marker, 0xd9 | 0xda) {
                return None;
            }
            if matches!(marker, 0x01 | 0xd0..=0xd8) {
                continue;
            }
            let size = u16::from_be_bytes(bytes.get(p..p + 2)?.try_into().ok()?) as usize;
            if size < 2 {
                return None;
            }
            if matches!(marker,0xc0..=0xc3|0xc5..=0xc7|0xc9..=0xcb|0xcd..=0xcf) {
                if size < 8 {
                    return None;
                }
                let data = bytes.get(p + 2..p + 8)?;
                return valid(
                    "JPEG",
                    u16::from_be_bytes([data[3], data[4]]) as u32,
                    u16::from_be_bytes([data[1], data[2]]) as u32,
                );
            }
            p = p.checked_add(size)?;
        }
        return None;
    }
    if bytes.starts_with(b"II\x2a\0") || bytes.starts_with(b"MM\0\x2a") {
        let little = bytes[0] == b'I';
        let short = |p: usize| -> Option<u16> {
            let a = bytes.get(p..p.checked_add(2)?)?.try_into().ok()?;
            Some(if little {
                u16::from_le_bytes(a)
            } else {
                u16::from_be_bytes(a)
            })
        };
        let long = |p: usize| -> Option<u32> {
            let a = bytes.get(p..p.checked_add(4)?)?.try_into().ok()?;
            Some(if little {
                u32::from_le_bytes(a)
            } else {
                u32::from_be_bytes(a)
            })
        };
        let p = usize::try_from(long(4)?).ok()?;
        let count = short(p)? as usize;
        bytes.get(p..p.checked_add(2 + 12 * count + 4)?)?;
        let (mut w, mut h) = (None, None);
        for n in 0..count {
            let p = p + 2 + 12 * n;
            let tag = short(p)?;
            if matches!(tag, 256 | 257) && long(p + 4)? == 1 {
                let value = match short(p + 2)? {
                    3 => Some(short(p + 8)? as u32),
                    4 => Some(long(p + 8)?),
                    _ => None,
                };
                if tag == 256 {
                    w = value;
                } else {
                    h = value;
                }
            }
        }
        return valid("TIFF", w?, h?);
    }
    if bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        let end =
            (u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize).checked_add(8)?;
        let mut p = 12usize;
        while p.checked_add(8)? <= end && p + 8 <= bytes.len() {
            let tag = bytes.get(p..p + 4)?;
            let size = u32::from_le_bytes(bytes.get(p + 4..p + 8)?.try_into().ok()?) as usize;
            if p.checked_add(8)?.checked_add(size)? > end {
                return None;
            }
            let minimum = match tag {
                b"VP8X" => 10,
                b"VP8L" => 5,
                b"VP8 " => 10,
                _ => 0,
            };
            if minimum > 0 {
                if size < minimum {
                    return None;
                }
                let data = bytes.get(p + 8..p + 8 + minimum)?;
                let (w, h) = match tag {
                    b"VP8X" => (
                        1 + u32::from_le_bytes([data[4], data[5], data[6], 0]),
                        1 + u32::from_le_bytes([data[7], data[8], data[9], 0]),
                    ),
                    b"VP8L" if data[0] == 0x2f => {
                        let bits = u32::from_le_bytes(data[1..5].try_into().ok()?);
                        if bits >> 29 != 0 {
                            return None;
                        }
                        ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
                    }
                    b"VP8 " if data[3..6] == [0x9d, 0x01, 0x2a] => (
                        (u16::from_le_bytes([data[6], data[7]]) & 0x3fff) as u32,
                        (u16::from_le_bytes([data[8], data[9]]) & 0x3fff) as u32,
                    ),
                    _ => return None,
                };
                return valid("WebP", w, h);
            }
            p = p.checked_add(8)?.checked_add(size)?.checked_add(size % 2)?;
        }
    }
    None
}
fn parse(bytes: &[u8], limited: bool) -> Result<Report, String> {
    let mut rows = Vec::new();
    if let Some((format, w, h)) = header(bytes) {
        rows.push(("Format", format.into()));
        rows.push(("Header dimensions", format!("{w} × {h} pixels")));
    }
    let header_rows = rows.len();
    let result = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(bytes));
    let exif = match result {
        Ok(value) => value,
        Err(exif::Error::NotFound(_)) if !limited => {
            return Ok(Report {
                rows,
                note: Some("No EXIF metadata found".into()),
            });
        }
        Err(error) => {
            let message = if limited {
                "EXIF metadata unavailable within the first 2 MiB".into()
            } else {
                format!("EXIF metadata unavailable: {error}")
            };
            return if rows.is_empty() {
                Err(message)
            } else {
                Ok(Report {
                    rows,
                    note: Some(message),
                })
            };
        }
    };
    use exif::Tag;
    for (label, tag) in [
        ("Camera maker", Tag::Make),
        ("Camera model", Tag::Model),
        ("Lens", Tag::LensModel),
        ("Artist", Tag::Artist),
        ("Copyright", Tag::Copyright),
        ("Software", Tag::Software),
        ("Taken", Tag::DateTimeOriginal),
        ("Taken UTC offset", Tag::OffsetTimeOriginal),
        ("Taken subsecond digits", Tag::SubSecTimeOriginal),
        ("Digitized", Tag::DateTimeDigitized),
        ("Exposure program", Tag::ExposureProgram),
        ("Metering mode", Tag::MeteringMode),
        ("Exposure", Tag::ExposureTime),
        ("Aperture", Tag::FNumber),
        ("ISO", Tag::PhotographicSensitivity),
        ("Focal length", Tag::FocalLength),
        ("35 mm equivalent", Tag::FocalLengthIn35mmFilm),
        ("Exposure bias", Tag::ExposureBiasValue),
        ("Flash", Tag::Flash),
        ("White balance", Tag::WhiteBalance),
        ("Orientation", Tag::Orientation),
        ("Pixel width", Tag::PixelXDimension),
        ("Pixel height", Tag::PixelYDimension),
        ("Color space", Tag::ColorSpace),
        ("GPS latitude", Tag::GPSLatitude),
        ("GPS longitude", Tag::GPSLongitude),
        ("GPS altitude", Tag::GPSAltitude),
        ("GPS date", Tag::GPSDateStamp),
    ] {
        if let Some(field) = exif.get_field(tag, exif::In::PRIMARY) {
            let mut text = BoundedText(String::new());
            if write!(text, "{}", field.display_value().with_unit(&exif)).is_err() {
                text.0.push('…');
            }
            rows.push((label, text.0));
        }
    }
    let note = if limited {
        Some("EXIF snapshot from the first 2 MiB".into())
    } else if rows.len() == header_rows {
        Some("No supported EXIF fields found".into())
    } else {
        None
    };
    Ok(Report { rows, note })
}
fn load(identity: &Identity, cancelled: impl Fn() -> bool) -> Result<Report, String> {
    if IN_FLIGHT
        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < 2).then_some(n + 1)
        })
        .is_err()
    {
        return Err("Image metadata workers are busy; retry later".into());
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
        }
    }
    let _guard = Guard;
    let check = || {
        if cancelled() {
            Err("Image metadata read cancelled".to_string())
        } else {
            Ok(())
        }
    };
    check()?;
    let path = &identity.preview.path;
    let changed =
        || "Image changed since Get Info was opened; refresh and reopen Get Info".to_string();
    let initial = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !identity.matches(&initial) {
        return Err(changed());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    if !identity.matches(&file.metadata().map_err(|e| e.to_string())?) {
        return Err(changed());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 32 * 1024];
    while bytes.len() < MAX_INPUT {
        check()?;
        let remaining = (MAX_INPUT - bytes.len()).min(chunk.len());
        let n = file
            .read(&mut chunk[..remaining])
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    check()?;
    let report = parse(&bytes, identity.preview.size > MAX_INPUT as u64)?;
    check()?;
    if !identity.matches(&file.metadata().map_err(|e| e.to_string())?)
        || !identity.matches(&std::fs::symlink_metadata(path).map_err(|e| e.to_string())?)
    {
        return Err(changed());
    }
    Ok(report)
}
struct Run {
    receiver: mpsc::Receiver<Result<Report, String>>,
    cancelled: Arc<AtomicBool>,
    started: Instant,
}
impl Drop for Run {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
fn start(identity: Identity, ctx: &egui::Context) -> Result<Run, String> {
    crate::provider_runtime::activate_builtin_explained(
        "native-preview",
        &crate::provider_runtime::ActivationRequest {
            capability: crate::provider_runtime::ProviderCapability::PreviewImage,
            root: identity.preview.path.parent().unwrap_or(Path::new("/")),
            extension: identity.preview.path.extension().and_then(|x| x.to_str()),
            bytes: Some(identity.preview.size.min(MAX_INPUT as u64)),
        },
    )?;
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel = cancelled.clone();
    let repaint = ctx.clone();
    crate::workload::submit(
        crate::workload::TaskSpec::new(
            crate::workload::TaskKind::Preview,
            &identity.preview.path,
            0,
        )
        .priority(crate::workload::Priority::Interactive)
        .estimated_bytes(MAX_INPUT as u64),
        move |token| {
            let result = load(&identity, || {
                token.is_cancelled() || cancel.load(Ordering::Acquire)
            });
            let _ = sender.send(result);
            repaint.request_repaint();
        },
    )
    .map_err(|e| format!("Could not start image metadata reader: {e:?}"))?;
    Ok(Run {
        receiver,
        cancelled,
        started: Instant::now(),
    })
}
#[derive(Default)]
struct Slot {
    identity: Option<Identity>,
    run: Option<Run>,
    result: Option<Result<Report, String>>,
    export_error: Option<String>,
}
#[derive(Default)]
pub struct Cache {
    slots: [Slot; 2],
}
impl Cache {
    pub fn retain(
        &mut self,
        ctx: &egui::Context,
        left: Option<&Identity>,
        right: Option<&Identity>,
    ) {
        for (slot, identity) in self.slots.iter_mut().zip([left, right]) {
            if slot.identity.as_ref() != identity {
                *slot = Slot {
                    identity: identity.cloned(),
                    ..Default::default()
                };
            }
            if let Some(run) = &slot.run {
                let result = match run.receiver.try_recv() {
                    Ok(value) => Some(value),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Image metadata worker stopped".into()))
                    }
                    Err(mpsc::TryRecvError::Empty) if run.started.elapsed() > TIMEOUT => {
                        Some(Err("Image metadata read timed out".into()))
                    }
                    Err(mpsc::TryRecvError::Empty) => None,
                };
                if let Some(result) = result {
                    slot.run = None;
                    slot.result = Some(result);
                }
            }
            if let Some(identity) = identity
                && slot.run.is_none()
                && slot.result.is_none()
            {
                match start(identity.clone(), ctx) {
                    Ok(run) => slot.run = Some(run),
                    Err(e) => slot.result = Some(Err(e)),
                }
            }
        }
    }
    #[cfg(feature = "visual-qa")]
    pub fn report(&self, side: &str) -> Option<&Report> {
        self.slots[usize::from(side == "right")]
            .result
            .as_ref()?
            .as_ref()
            .ok()
    }
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        identity: &Identity,
        side: &str,
    ) -> Option<Arc<str>> {
        let slot = &mut self.slots[usize::from(side == "right")];
        #[cfg(feature = "visual-qa")]
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new(("image-metadata-clip", side)), ui.clip_rect())
        });
        ui.separator();
        let mut copied = None;
        ui.horizontal(|ui| {
            ui.strong("Image metadata");
            let copy = ui.add_enabled(
                slot.identity.as_ref() == Some(identity) && matches!(&slot.result, Some(Ok(_))),
                egui::Button::new("Copy metadata JSON"),
            );
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new(("image-metadata-copy-json", side)), copy.rect)
            });
            if copy
                .on_hover_text("Copy the loaded inspector snapshot without rereading the image")
                .clicked()
                && let Some(Ok(report)) = &slot.result
            {
                match export_json(identity, report) {
                    Ok(text) => {
                        copied = Some(Arc::from(text));
                        slot.export_error = None;
                    }
                    Err(error) => slot.export_error = Some(error),
                }
            }
        });
        if slot.identity.as_ref() != Some(identity) {
            ui.label("Loading image metadata…");
            return None;
        }
        match &slot.result {
            Some(Ok(report)) => {
                for (label, value) in &report.rows {
                    let response = ui.horizontal_wrapped(|ui| {
                        ui.label(format!("{label}:"));
                        ui.add(egui::Label::new(value).wrap());
                    });
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            egui::Id::new(("image-metadata-row", side, *label)),
                            response.response.rect,
                        )
                    });
                    let _ = response;
                }
                if let Some(note) = &report.note {
                    ui.small(note);
                }
                if report.rows.is_empty() && report.note.is_none() {
                    ui.small("No supported EXIF fields found");
                }
            }
            Some(Err(error)) => {
                ui.label(error);
                if ui.button("Retry image metadata").clicked() {
                    slot.result = None;
                    ui.ctx().request_repaint();
                }
            }
            None => {
                ui.label("Loading image metadata…");
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        }
        if let Some(error) = &slot.export_error {
            ui.label(error);
        }
        copied
    }
}

#[cfg(any(test, feature = "visual-qa"))]
fn fixture_tiff() -> Vec<u8> {
    fn ifd(buf: &mut Vec<u8>, entries: Vec<(u16, u16, u32, Vec<u8>)>) -> u32 {
        let offset = buf.len();
        let header = 2 + 12 * entries.len() + 4;
        buf.resize(offset + header, 0);
        buf[offset..offset + 2].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        for (i, (tag, kind, count, value)) in entries.into_iter().enumerate() {
            let p = offset + 2 + 12 * i;
            buf[p..p + 2].copy_from_slice(&tag.to_le_bytes());
            buf[p + 2..p + 4].copy_from_slice(&kind.to_le_bytes());
            buf[p + 4..p + 8].copy_from_slice(&count.to_le_bytes());
            if value.len() <= 4 {
                buf[p + 8..p + 8 + value.len()].copy_from_slice(&value);
            } else {
                let data = buf.len() as u32;
                buf[p + 8..p + 12].copy_from_slice(&data.to_le_bytes());
                buf.extend(value);
            }
        }
        offset as u32
    }
    fn ascii(tag: u16, value: &str) -> (u16, u16, u32, Vec<u8>) {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        (tag, 2, bytes.len() as u32, bytes)
    }
    fn rational(tag: u16, n: u32, d: u32) -> (u16, u16, u32, Vec<u8>) {
        (tag, 5, 1, [n.to_le_bytes(), d.to_le_bytes()].concat())
    }
    let mut tiff = b"II\x2a\0\0\0\0\0".to_vec();
    let exif = ifd(
        &mut tiff,
        vec![
            ascii(36867, "2026:11:02 12:34:56"),
            ascii(36881, "+05:30"),
            ascii(37521, "250"),
            (34850, 3, 1, 3u16.to_le_bytes().to_vec()),
            (37383, 3, 1, 5u16.to_le_bytes().to_vec()),
            rational(33434, 1, 125),
            rational(33437, 28, 10),
            (34855, 3, 1, 200u16.to_le_bytes().to_vec()),
            rational(37386, 50, 1),
            ascii(42036, "QA 50mm"),
            (40962, 4, 1, 4u32.to_le_bytes().to_vec()),
            (40963, 4, 1, 3u32.to_le_bytes().to_vec()),
        ],
    );
    let gps = ifd(
        &mut tiff,
        vec![
            ascii(1, "N"),
            (
                2,
                5,
                3,
                [41u32, 1, 43, 1, 0, 1]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect(),
            ),
            ascii(3, "E"),
            (
                4,
                5,
                3,
                [44u32, 1, 47, 1, 0, 1]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect(),
            ),
            rational(6, 450, 1),
        ],
    );
    let pixels = tiff.len() as u32;
    tiff.extend_from_slice(&[60u8; 36]);
    let root = ifd(
        &mut tiff,
        vec![
            (256, 4, 1, 4u32.to_le_bytes().to_vec()),
            (257, 4, 1, 3u32.to_le_bytes().to_vec()),
            (
                258,
                3,
                3,
                [8u16, 8, 8]
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ),
            (259, 3, 1, 1u16.to_le_bytes().to_vec()),
            (262, 3, 1, 2u16.to_le_bytes().to_vec()),
            (273, 4, 1, pixels.to_le_bytes().to_vec()),
            (277, 3, 1, 3u16.to_le_bytes().to_vec()),
            (278, 4, 1, 3u32.to_le_bytes().to_vec()),
            (279, 4, 1, 36u32.to_le_bytes().to_vec()),
            ascii(315, "QA Photographer"),
            ascii(33432, "QA rights reserved"),
            ascii(305, "Commander QA encoder"),
            ascii(271, "Commander QA"),
            ascii(272, "Fixture Camera"),
            (274, 3, 1, 6u16.to_le_bytes().to_vec()),
            (34665, 4, 1, exif.to_le_bytes().to_vec()),
            (34853, 4, 1, gps.to_le_bytes().to_vec()),
        ],
    );
    tiff[4..8].copy_from_slice(&root.to_le_bytes());
    tiff
}
#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture() -> Vec<u8> {
    container_fixture("jpeg", true)
}
#[cfg(any(test, feature = "visual-qa"))]
pub fn container_fixture(format: &str, with_exif: bool) -> Vec<u8> {
    use image::ImageEncoder;
    let tiff = fixture_tiff();
    if format == "tiff" && with_exif {
        return tiff;
    }
    let mut bytes = Vec::new();
    match format {
        "png" => {
            let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
            if with_exif {
                encoder.set_exif_metadata(tiff).unwrap();
            }
            encoder
                .write_image(&[60u8; 36], 4, 3, image::ExtendedColorType::Rgb8)
                .unwrap();
            return bytes;
        }
        "webp" => {
            let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut bytes);
            if with_exif {
                encoder.set_exif_metadata(tiff).unwrap();
            }
            encoder
                .write_image(&[60u8; 36], 4, 3, image::ExtendedColorType::Rgb8)
                .unwrap();
            return bytes;
        }
        "tiff" => {
            let mut cursor = std::io::Cursor::new(&mut bytes);
            image::codecs::tiff::TiffEncoder::new(&mut cursor)
                .write_image(&[60u8; 36], 4, 3, image::ExtendedColorType::Rgb8)
                .unwrap();
            return bytes;
        }
        "jpeg" => {}
        _ => panic!("unknown fixture format"),
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode(&[60u8; 36], 4, 3, image::ExtendedColorType::Rgb8)
        .unwrap();
    if !with_exif {
        return jpeg;
    }
    let payload = [b"Exif\0\0".as_slice(), &tiff].concat();
    let app = [
        vec![0xff, 0xe1],
        ((payload.len() + 2) as u16).to_be_bytes().to_vec(),
        payload,
    ]
    .concat();
    jpeg.splice(2..2, app);
    jpeg
}

#[cfg(test)]
mod tests {
    use super::*;
    static IO_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn identity(path: &Path) -> Identity {
        Identity::from_entry(&FileEntry::observe_lexical(path.to_path_buf()).unwrap()).unwrap()
    }
    #[test]
    fn exif_snapshot_reports_camera_exposure_orientation_dimensions_and_gps() {
        let report = parse(&fixture(), false).unwrap();
        let value = |label| {
            report
                .rows
                .iter()
                .find(|(k, _)| *k == label)
                .unwrap()
                .1
                .as_str()
        };
        assert!(value("Camera maker").contains("Commander QA"));
        assert!(value("Lens").contains("QA 50mm"));
        assert!(value("Taken").contains("2026"));
        assert!(value("Exposure").contains("1/125"));
        assert!(value("Aperture").contains("2.8"));
        assert_eq!(value("ISO"), "200");
        assert_eq!(value("Pixel width"), "4 pixels");
        assert_eq!(value("Pixel height"), "3 pixels");
        assert!(value("GPS latitude").contains("41") && value("GPS latitude").contains('N'));
        assert!(value("GPS longitude").contains("44") && value("GPS longitude").contains('E'));
        assert!(report.note.is_none());
    }
    #[test]
    fn attribution_and_capture_offset_preserve_recorded_values_without_timezone_inference() {
        let report = parse(&fixture(), false).unwrap();
        let value = |key| {
            report
                .rows
                .iter()
                .find(|(k, _)| *k == key)
                .unwrap()
                .1
                .as_str()
        };
        assert!(value("Artist").contains("QA Photographer"));
        assert!(value("Copyright").contains("QA rights reserved"));
        assert!(value("Software").contains("Commander QA encoder"));
        assert_eq!(value("Taken"), "2026-11-02 12:34:56");
        assert!(value("Taken UTC offset").contains("+05:30"));
        assert!(value("Taken subsecond digits").contains("250"));
        assert!(value("Exposure program").contains("aperture priority"));
        assert!(value("Metering mode").contains("pattern"));
    }
    #[test]
    fn json_export_is_a_bounded_retained_snapshot_with_exact_paths_and_timestamps() {
        let tmp = crate::testutil::TempDir::new();
        let path = tmp.path().join("quoted-\"界\n.jpg");
        std::fs::write(&path, fixture()).unwrap();
        let mut id = identity(&path);
        let report = parse(&fixture(), false).unwrap();
        std::fs::remove_file(&path).unwrap();
        id.preview.modified = Some(std::time::UNIX_EPOCH - Duration::from_nanos(5));
        let text = export_json(&id, &report).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["format"], "commander.image-metadata");
        assert_eq!(value["version"], 1);
        assert_eq!(value["source"]["path"]["text"], path.to_str().unwrap());
        assert_eq!(value["source"]["modified_unix_ns"], "-5");
        assert_eq!(value["properties"].as_array().unwrap().len(), 23);
        assert!(text.len() <= MAX_EXPORT);
        assert_eq!(
            value["properties"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["name"] == "Taken UTC offset")
                .unwrap()["display_value"],
            "\"+05:30\""
        );
        let mut oversized = report.clone();
        oversized.rows[0].1 = "x".repeat(MAX_EXPORT);
        assert!(export_json(&id, &oversized).is_err());
        id.preview.path = "relative.jpg".into();
        assert!(export_json(&id, &report).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn json_export_preserves_non_utf8_path_bytes_without_replacement_characters() {
        use std::os::unix::ffi::OsStringExt;
        let tmp = crate::testutil::TempDir::new();
        let path = tmp.path().join("image.jpg");
        std::fs::write(&path, fixture()).unwrap();
        let mut id = identity(&path);
        id.preview.path = std::ffi::OsString::from_vec(b"/image-\xff.jpg".to_vec()).into();
        let value: serde_json::Value =
            serde_json::from_str(&export_json(&id, &parse(&fixture(), false).unwrap()).unwrap())
                .unwrap();
        assert_eq!(value["source"]["path"]["encoding"], "unix_bytes_hex");
        assert_eq!(value["source"]["path"]["hex"], "2f696d6167652dff2e6a7067");
    }
    #[test]
    fn copy_json_pointer_requires_the_matching_ready_inspector_and_never_reloads() {
        let tmp = crate::testutil::TempDir::new();
        let path = tmp.path().join("image.jpg");
        std::fs::write(&path, fixture()).unwrap();
        let id = identity(&path);
        let report = parse(&fixture(), false).unwrap();
        std::fs::remove_file(&path).unwrap();
        for state in 0..4 {
            let mut cache = Cache::default();
            cache.slots[1] = Slot {
                identity: Some(id.clone()),
                result: match state {
                    1 => None,
                    2 => Some(Err("failed".into())),
                    _ => Some(Ok(report.clone())),
                },
                ..Default::default()
            };
            let mut shown = id.clone();
            if state == 3 {
                shown.preview.path = Path::new("/another.jpg").into();
            }
            let ctx = egui::Context::default();
            let mut time = 0.0;
            let mut frame = |events| {
                time += 0.1;
                let mut copied = None;
                crate::testutil::discard_egui_output(ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(800.0, 900.0),
                        )),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        copied = cache.render(ui, &shown, "right");
                    },
                ));
                copied
            };
            assert!(frame(vec![]).is_none());
            let pos = ctx
                .data(|d| {
                    d.get_temp::<egui::Rect>(egui::Id::new(("image-metadata-copy-json", "right")))
                })
                .unwrap()
                .center();
            assert!(
                frame(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default()
                    }
                ])
                .is_none()
            );
            let copied = frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }]);
            assert_eq!(copied.is_some(), state == 0);
            if let Some(text) = copied {
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&text).unwrap()["source"]["size_bytes"],
                    id.preview.size
                );
            }
            assert!(cache.slots[1].run.is_none());
        }
    }
    #[test]
    fn malformed_missing_metadata_and_output_bounds_are_explicit() {
        for bytes in [
            b"garbage".as_slice(),
            b"II\x2a\0\xff\xff\xff\xff",
            b"\x89PNG\r\n\x1a\n\xff\xff\xff\xffeXIf",
        ] {
            assert!(parse(bytes, false).is_err());
        }
        assert!(parse(b"\xff\xd8\xff\xd9", false).unwrap().rows.is_empty());
        assert!(parse(b"\xff\xd8\xff\xd9", true).is_err());
        let mut text = BoundedText(String::new());
        assert!(write!(text, "{}", "界".repeat(1000)).is_err());
        assert!(text.0.len() <= MAX_VALUE);
        let mut text = BoundedText(String::new());
        write!(text, "a\n\tb").unwrap();
        assert_eq!(text.0, "a  b");
    }
    #[test]
    fn worker_rejects_changed_replaced_links_cancel_and_never_reads_beyond_budget() {
        let _lock = IO_LOCK.lock().unwrap();
        let tmp = crate::testutil::TempDir::new();
        let path = tmp.path().join("image.jpg");
        std::fs::write(&path, fixture()).unwrap();
        let id = identity(&path);
        assert!(!load(&id, || false).unwrap().rows.is_empty());
        assert!(load(&id, || true).is_err());
        std::fs::write(&path, b"changed").unwrap();
        assert!(load(&id, || false).is_err());
        let mut bytes = fixture();
        bytes.resize(MAX_INPUT + 1000, 7);
        std::fs::write(&path, bytes).unwrap();
        let report = load(&identity(&path), || false).unwrap();
        assert!(report.note.unwrap().contains("first 2 MiB"));
        #[cfg(unix)]
        {
            let old = identity(&path);
            let moved = tmp.path().join("moved.jpg");
            std::fs::rename(&path, &moved).unwrap();
            std::os::unix::fs::symlink(&moved, &path).unwrap();
            assert!(load(&old, || false).is_err());
            assert!(Identity::from_entry(&FileEntry::observe_lexical(path).unwrap()).is_none());
        }
    }
    #[test]
    fn real_containers_keep_header_properties_with_and_without_exif() {
        let _lock = IO_LOCK.lock().unwrap();
        let tmp = crate::testutil::TempDir::new();
        for (format, label) in [
            ("jpeg", "JPEG"),
            ("png", "PNG"),
            ("tiff", "TIFF"),
            ("webp", "WebP"),
        ] {
            for with_exif in [false, true] {
                let bytes = container_fixture(format, with_exif);
                let image = image::load_from_memory(&bytes).unwrap();
                assert_eq!((image.width(), image.height()), (4, 3), "{format}");
                let path = tmp.path().join(format!("{format}-{with_exif}.{format}"));
                std::fs::write(&path, bytes).unwrap();
                let report = load(&identity(&path), || false).unwrap();
                assert_eq!(report.rows[0], ("Format", label.to_string()));
                assert_eq!(
                    report.rows[1],
                    ("Header dimensions", "4 × 3 pixels".to_string())
                );
                if with_exif {
                    assert_eq!(report.rows.len(), 23);
                    assert!(report.note.is_none());
                    assert!(
                        report
                            .rows
                            .iter()
                            .any(|(k, v)| *k == "Camera model" && v.contains("Fixture Camera"))
                    );
                } else {
                    assert_eq!(report.rows.len(), 2);
                    assert!(report.note.as_ref().unwrap().contains("EXIF"));
                }
            }
        }
    }
    #[test]
    fn header_properties_survive_bad_exif_without_allocating_pixel_buffers() {
        let plain = container_fixture("png", false);
        let mut broken = plain.clone();
        let data = b"broken TIFF";
        let chunk = [(data.len() as u32).to_be_bytes().as_slice(), b"eXIf", data].concat();
        let checksum = crc32fast::hash(&chunk[4..]);
        let chunk = [chunk, checksum.to_be_bytes().to_vec()].concat();
        broken.splice(33..33, chunk);
        assert_eq!(image::load_from_memory(&broken).unwrap().width(), 4);
        let report = parse(&broken, false).unwrap();
        assert_eq!(report.rows.len(), 2);
        assert!(report.note.unwrap().contains("unavailable"));
        let mut huge = plain.clone();
        huge[16..20].copy_from_slice(&1_000_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&1_000_000u32.to_be_bytes());
        let crc = crc32fast::hash(&huge[12..29]);
        huge[29..33].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            parse(&huge, false).unwrap().rows[1].1,
            "1000000 × 1000000 pixels"
        );
        huge[29] ^= 1;
        assert!(header(&huge).is_none());
        let mut big = b"MM\0\x2a\0\0\0\x08\0\x02".to_vec();
        big.extend_from_slice(&[256u16.to_be_bytes(), 3u16.to_be_bytes()].concat());
        big.extend_from_slice(&1u32.to_be_bytes());
        big.extend_from_slice(&[0, 17, 0, 0]);
        big.extend_from_slice(&[257u16.to_be_bytes(), 4u16.to_be_bytes()].concat());
        big.extend_from_slice(&1u32.to_be_bytes());
        big.extend_from_slice(&23u32.to_be_bytes());
        big.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(header(&big), Some(("TIFF", 17, 23)));
        let mut lossy = b"RIFF".to_vec();
        lossy.extend_from_slice(&22u32.to_le_bytes());
        lossy.extend_from_slice(b"WEBPVP8 ");
        lossy.extend_from_slice(&10u32.to_le_bytes());
        lossy.extend_from_slice(&[0x10, 0, 0, 0x9d, 1, 0x2a]);
        lossy.extend_from_slice(&63u16.to_le_bytes());
        lossy.extend_from_slice(&17u16.to_le_bytes());
        assert_eq!(header(&lossy), Some(("WebP", 63, 17)));
        assert!(header(b"MM\0\x2a\xff\xff\xff\xff").is_none());
        assert!(header(b"RIFF\xff\xff\xff\xffWEBPVP8X\0\0\0\0").is_none());
    }
    #[test]
    fn read_limit_cannot_reach_exif_after_the_prefix() {
        let _lock = IO_LOCK.lock().unwrap();
        let mut late = vec![0xff, 0xd8];
        let padding = [
            vec![0xff, 0xe2],
            60002u16.to_be_bytes().to_vec(),
            vec![0; 60000],
        ]
        .concat();
        while late.len() <= MAX_INPUT {
            late.extend_from_slice(&padding);
        }
        late.extend_from_slice(&fixture()[2..]);
        assert!(
            parse(&late, false)
                .unwrap()
                .rows
                .iter()
                .any(|(k, _)| *k == "Camera model")
        );
        let tmp = crate::testutil::TempDir::new();
        let path = tmp.path().join("late.jpg");
        std::fs::write(&path, late).unwrap();
        assert!(
            load(&identity(&path), || false)
                .unwrap_err()
                .contains("first 2 MiB")
        );
    }
    #[test]
    fn retiring_an_inspector_cancels_its_owner_without_accepting_late_results() {
        let _lock = IO_LOCK.lock().unwrap();
        let tmp = crate::testutil::TempDir::new();
        let path = tmp.path().join("image.jpg");
        std::fs::write(&path, fixture()).unwrap();
        let identity = identity(&path);
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut cache = Cache::default();
        cache.slots[0] = Slot {
            identity: Some(identity),
            run: Some(Run {
                receiver,
                cancelled: cancelled.clone(),
                started: Instant::now(),
            }),
            result: None,
            export_error: None,
        };
        cache.retain(&egui::Context::default(), None, None);
        assert!(cancelled.load(Ordering::Acquire));
        assert!(sender.send(Ok(parse(&fixture(), false).unwrap())).is_err());
        assert!(cache.slots[0].result.is_none());
    }
}
