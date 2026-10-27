//! Read-only paged binary inspection and cancellable streaming byte search.
use crate::path_identity::PathIdentity;
use crate::workload::{Priority, TaskHandle, TaskKind, TaskSpec, WorkloadHandle};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

pub const PAGE_BYTES: usize = 4096;
const ROW_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ByteFormat {
    #[default]
    Hex,
    Decimal,
}

#[derive(Clone, Debug)]
struct Stamp {
    identity: PathIdentity,
    #[cfg(unix)]
    changed: (i64, i64),
}
impl Stamp {
    fn from_metadata(path: &Path, metadata: &std::fs::Metadata) -> Self {
        Self {
            identity: PathIdentity::from_metadata(path.into(), metadata),
            #[cfg(unix)]
            changed: {
                use std::os::unix::fs::MetadataExt;
                (metadata.ctime(), metadata.ctime_nsec())
            },
        }
    }
    fn matches(&self, other: &Self) -> bool {
        self.identity.same_shallow_binding(&other.identity) && {
            #[cfg(unix)]
            {
                self.changed == other.changed
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
    }
}

#[derive(Debug)]
struct Page {
    stamp: Stamp,
    offset: u64,
    bytes: Vec<u8>,
}
impl Page {
    fn next_offset(&self) -> Option<u64> {
        let next = self.offset.saturating_add(self.bytes.len() as u64);
        (next < self.stamp.identity.size).then_some(next)
    }
    #[cfg(test)]
    fn row(&self, row: usize) -> String {
        self.row_in_format(row, ByteFormat::Hex)
    }
    fn row_in_format(&self, row: usize, format: ByteFormat) -> String {
        use std::fmt::Write;
        let start = row * ROW_BYTES;
        let bytes = &self.bytes[start..self.bytes.len().min(start + ROW_BYTES)];
        let mut text = format!("{:016X}  ", self.offset + start as u64);
        for index in 0..ROW_BYTES {
            if index == 8 {
                text.push(' ');
            }
            if let Some(byte) = bytes.get(index) {
                match format {
                    ByteFormat::Hex => {
                        let _ = write!(text, "{byte:02X} ");
                    }
                    ByteFormat::Decimal => {
                        let _ = write!(text, "{byte:03} ");
                    }
                }
            } else {
                text.push_str(match format {
                    ByteFormat::Hex => "   ",
                    ByteFormat::Decimal => "    ",
                });
            }
        }
        text.push_str(" | ");
        for byte in bytes {
            text.push(if (0x20..=0x7e).contains(byte) {
                char::from(*byte)
            } else {
                '.'
            });
        }
        text
    }
    #[cfg(test)]
    fn text(&self) -> String {
        self.text_in_format(ByteFormat::Hex)
    }
    fn text_in_format(&self, format: ByteFormat) -> String {
        (0..self.bytes.len().div_ceil(ROW_BYTES))
            .map(|row| self.row_in_format(row, format))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn changed() -> String {
    "The file changed. Reload to inspect the current file.".into()
}
fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("Hex read cancelled".into())
    } else {
        Ok(())
    }
}
fn open_bound(
    path: &Path,
    expected: Option<&Stamp>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(std::fs::File, Stamp), String> {
    check_cancelled(cancelled)?;
    let before = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !before.is_file() {
        return Err("Hex preview accepts regular files only, without following symlinks.".into());
    }
    let before = Stamp::from_metadata(path, &before);
    if expected.is_some_and(|expected| !expected.matches(&before)) {
        return Err(changed());
    }
    check_cancelled(cancelled)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY);
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    let opened_meta = file.metadata().map_err(|error| error.to_string())?;
    let opened = Stamp::from_metadata(path, &opened_meta);
    if !opened_meta.is_file() || !before.matches(&opened) {
        return Err(changed());
    }
    Ok((file, before))
}
fn validate_bound(path: &Path, file: &std::fs::File, before: &Stamp) -> Result<(), String> {
    let after_fd = Stamp::from_metadata(path, &file.metadata().map_err(|error| error.to_string())?);
    let after_path = Stamp::from_metadata(
        path,
        &std::fs::symlink_metadata(path).map_err(|error| error.to_string())?,
    );
    if !before.matches(&after_fd) || !before.matches(&after_path) {
        return Err(changed());
    }
    Ok(())
}

fn read_page(
    path: &Path,
    offset: u64,
    expected: Option<&Stamp>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Page, String> {
    let (mut file, before) = open_bound(path, expected, cancelled)?;
    if offset > before.identity.size {
        return Err("Offset is beyond the end of the file.".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let length = (before.identity.size - offset).min(PAGE_BYTES as u64) as usize;
    let mut bytes = vec![0; length];
    let mut read = 0;
    while read < length {
        check_cancelled(cancelled)?;
        match file.read(&mut bytes[read..]) {
            Ok(0) => return Err(changed()),
            Ok(count) => read += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    check_cancelled(cancelled)?;
    validate_bound(path, &file, &before)?;
    check_cancelled(cancelled)?;
    Ok(Page {
        stamp: before,
        offset,
        bytes,
    })
}

fn search_page(
    path: &Path,
    expected: &Stamp,
    start: u64,
    pattern: &[u8],
    direction: crate::hex_search::Direction,
    cancelled: &dyn Fn() -> bool,
    progress: &std::sync::atomic::AtomicU64,
) -> Result<Option<Page>, String> {
    let (mut file, before) = open_bound(path, Some(expected), cancelled)?;
    let found = crate::hex_search::find(
        &mut file,
        before.identity.size,
        start,
        pattern,
        direction,
        cancelled,
        progress,
    )?;
    check_cancelled(cancelled)?;
    validate_bound(path, &file, &before)?;
    found
        .map(|offset| read_page(path, offset, Some(&before), cancelled))
        .transpose()
}

fn parse_offset(input: &str) -> Result<u64, &'static str> {
    let input = input.trim();
    let value = if let Some(hex) = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
    } else {
        input.parse()
    };
    value.map_err(|_| "Enter a byte offset in decimal or hexadecimal with 0x.")
}

enum Completion {
    Page(Page),
    Search(Option<Page>),
}

struct ReadTask {
    receiver: Receiver<Result<Completion, String>>,
    handle: TaskHandle,
    cancel_on_drop: bool,
    progress: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}
impl Drop for ReadTask {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.handle.cancel();
        }
    }
}

pub struct HexViewer {
    byte_format: ByteFormat,
    path: PathBuf,
    stamp: Option<Stamp>,
    page: Option<Page>,
    error: Option<String>,
    task: Option<ReadTask>,
    started: bool,
    generation: u64,
    offset_input: String,
    search_input: String,
    last_match: Option<(u64, Vec<u8>)>,
    search_message: Option<String>,
    pending_pattern: Option<Vec<u8>>,
}
impl HexViewer {
    pub fn new(path: PathBuf) -> Self {
        Self {
            byte_format: ByteFormat::default(),
            path,
            stamp: None,
            page: None,
            error: None,
            task: None,
            started: false,
            generation: 0,
            offset_input: "0".into(),
            search_input: String::new(),
            last_match: None,
            search_message: None,
            pending_pattern: None,
        }
    }
    fn request(&mut self, offset: u64, ctx: &egui::Context, workload: &WorkloadHandle) {
        if self.task.is_some() {
            return;
        }
        self.started = true;
        self.last_match = None;
        self.search_message = None;
        self.pending_pattern = None;
        self.error = None;
        self.page = None;
        self.generation = self.generation.wrapping_add(1).max(1);
        let path = self.path.clone();
        let expected = self.stamp.clone();
        let repaint = ctx.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let spec = TaskSpec::new(
            TaskKind::Preview,
            self.path.parent().unwrap_or(Path::new(".")),
            self.generation,
        )
        .priority(Priority::Interactive)
        .estimated_bytes(PAGE_BYTES as u64);
        match workload.submit(spec, move |cancelled| {
            let result = read_page(&path, offset, expected.as_ref(), &|| {
                cancelled.is_cancelled()
            });
            let _ = sender.send(result.map(Completion::Page));
            repaint.request_repaint();
        }) {
            Ok(handle) => {
                self.task = Some(ReadTask {
                    receiver,
                    handle,
                    cancel_on_drop: true,
                    progress: None,
                })
            }
            Err(error) => self.error = Some(format!("Could not start hex preview: {error}")),
        }
    }
    fn poll(&mut self) {
        let Some(task) = self.task.as_ref() else {
            return;
        };
        let result = match task.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("Hex read stopped before returning a page. Reload to retry.".into())
            }
        };
        if let Some(mut task) = self.task.take() {
            task.cancel_on_drop = false;
        }
        match result {
            Ok(Completion::Page(page)) => self.accept_page(page),
            Ok(Completion::Search(Some(page))) => {
                self.search_message = Some(format!("Match at 0x{:X}", page.offset));
                self.last_match = self
                    .pending_pattern
                    .take()
                    .map(|pattern| (page.offset, pattern));
                self.accept_page(page);
            }
            Ok(Completion::Search(None)) => {
                self.pending_pattern = None;
                self.search_message =
                    Some("No match in this direction; search does not wrap.".into());
                self.error = None;
            }
            Err(error) => {
                self.page = None;
                self.pending_pattern = None;
                self.error = Some(error);
            }
        }
    }
    fn accept_page(&mut self, page: Page) {
        self.stamp = Some(page.stamp.clone());
        self.offset_input = format!("0x{:X}", page.offset);
        self.page = Some(page);
        self.error = None;
    }
    fn search(
        &mut self,
        direction: crate::hex_search::Direction,
        ctx: &egui::Context,
        workload: &WorkloadHandle,
    ) {
        use crate::hex_search::Direction;
        if self.task.is_some() {
            return;
        }
        let pattern = match crate::hex_search::parse(&self.search_input) {
            Ok(pattern) => pattern,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        let Some(stamp) = self.stamp.clone() else {
            return;
        };
        let start = if let Some((offset, _)) =
            self.last_match.as_ref().filter(|(_, old)| old == &pattern)
        {
            match direction {
                Direction::Forward => offset.checked_add(1),
                Direction::Backward => offset.checked_sub(1),
            }
        } else {
            self.page.as_ref().map(|page| page.offset)
        };
        let Some(start) = start else {
            self.search_message = Some("No match in this direction; search does not wrap.".into());
            return;
        };
        self.error = None;
        self.search_message = None;
        self.pending_pattern = Some(pattern.clone());
        self.generation = self.generation.wrapping_add(1).max(1);
        let path = self.path.clone();
        let repaint = ctx.clone();
        let progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let worker_progress = std::sync::Arc::clone(&progress);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let spec = TaskSpec::new(
            TaskKind::Search,
            self.path.parent().unwrap_or(Path::new(".")),
            self.generation,
        )
        .priority(Priority::Interactive)
        .estimated_bytes(crate::hex_search::CHUNK_BYTES as u64 + PAGE_BYTES as u64);
        match workload.submit(spec, move |cancelled| {
            let result = search_page(
                &path,
                &stamp,
                start,
                &pattern,
                direction,
                &|| cancelled.is_cancelled(),
                &worker_progress,
            );
            let _ = sender.send(result.map(Completion::Search));
            repaint.request_repaint();
        }) {
            Ok(handle) => {
                self.task = Some(ReadTask {
                    receiver,
                    handle,
                    cancel_on_drop: true,
                    progress: Some(progress),
                })
            }
            Err(error) => {
                self.pending_pattern = None;
                self.error = Some(format!("Could not start hex search: {error}"));
            }
        }
    }

    /// Returns (window open, explicitly requested clipboard text).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        workload: &WorkloadHandle,
    ) -> (bool, Option<String>) {
        self.poll();
        if !self.started {
            self.request(0, ctx, workload);
        }
        let mut open = true;
        let mut copy = None;
        let mut navigate = None;
        let mut reload = false;
        let mut cancel = false;
        let mut search = None;
        let busy = self.task.is_some();
        let viewport = ctx.input(|input| input.viewport_rect());
        egui::Window::new("Hex preview")
            .id(egui::Id::new("hex-preview-window"))
            .default_width(760.0)
            .default_pos(egui::pos2((viewport.right() - 790.0).max(16.0), 96.0))
            .resizable(true)
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("hex-sheet")
                    .max_height((viewport.height() - 160.0).max(100.0))
                    .show(ui, |ui| {
                        let path = self.path.to_string_lossy().escape_debug().to_string();
                        ui.add(egui::Label::new(&path).truncate())
                            .on_hover_text(&path);
                        ui.small(
                            "Read-only · 4096 bytes per page · offsets are absolute byte positions",
                        );
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Bytes");
                            for (format, label) in
                                [(ByteFormat::Hex, "HEX"), (ByteFormat::Decimal, "Decimal")]
                            {
                                let response =
                                    ui.selectable_value(&mut self.byte_format, format, label);
                                #[cfg(any(test, feature = "visual-qa"))]
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(
                                        egui::Id::new(("hex-byte-format", label)),
                                        response.rect,
                                    )
                                });
                                #[cfg(not(any(test, feature = "visual-qa")))]
                                let _ = response;
                            }
                            ui.small("Offsets stay hexadecimal; search uses hex bytes");
                        });
                        ui.horizontal_wrapped(|ui| {
                            let first = ui.add_enabled(
                                !busy && self.page.as_ref().is_some_and(|page| page.offset > 0),
                                egui::Button::new("First"),
                            );
                            if first.clicked() {
                                navigate = Some(0);
                            }
                            let previous = ui.add_enabled(
                                !busy && self.page.as_ref().is_some_and(|page| page.offset > 0),
                                egui::Button::new("Previous"),
                            );
                            if previous.clicked() {
                                navigate = self
                                    .page
                                    .as_ref()
                                    .map(|page| page.offset.saturating_sub(PAGE_BYTES as u64));
                            }
                            let next = ui.add_enabled(
                                !busy && self.page.as_ref().and_then(Page::next_offset).is_some(),
                                egui::Button::new("Next"),
                            );
                            if next.clicked() {
                                navigate = self.page.as_ref().and_then(Page::next_offset);
                            }
                            let last = ui.add_enabled(
                                !busy && self.page.is_some(),
                                egui::Button::new("Last"),
                            );
                            if last.clicked() {
                                navigate = self.stamp.as_ref().map(|stamp| {
                                    stamp.identity.size.saturating_sub(1) / PAGE_BYTES as u64
                                        * PAGE_BYTES as u64
                                });
                            }
                            #[cfg(feature = "visual-qa")]
                            ctx.data_mut(|data| {
                                data.insert_temp(egui::Id::new("hex-next"), next.rect);
                                data.insert_temp(egui::Id::new("hex-previous"), previous.rect);
                            });
                            if ui
                                .add_enabled(!busy, egui::Button::new("Reload"))
                                .on_hover_text("Accept the current file and return to offset zero")
                                .clicked()
                            {
                                reload = true;
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Offset");
                            let offset = ui.add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut self.offset_input)
                                    .desired_width(180.0)
                                    .char_limit(32)
                                    .hint_text("1234 or 0x4D2"),
                            );
                            offset.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::TextEdit,
                                    offset.enabled(),
                                    "Byte offset",
                                )
                            });
                            let go = ui.add_enabled(!busy, egui::Button::new("Go"));
                            #[cfg(feature = "visual-qa")]
                            ctx.data_mut(|data| {
                                data.insert_temp(egui::Id::new("hex-offset"), offset.rect);
                                data.insert_temp(egui::Id::new("hex-go"), go.rect);
                            });
                            if go.clicked()
                                || (!busy
                                    && offset.lost_focus()
                                    && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                            {
                                match parse_offset(&self.offset_input) {
                                    Ok(value) => navigate = Some(value),
                                    Err(error) => self.error = Some(error.into()),
                                }
                            }
                            let copy_button = ui
                                .add_enabled(
                                    !busy
                                        && self
                                            .page
                                            .as_ref()
                                            .is_some_and(|page| !page.bytes.is_empty()),
                                    egui::Button::new("Copy page"),
                                )
                                .on_hover_text(
                                    "Copy this page's offsets, displayed byte values and ASCII",
                                );
                            #[cfg(test)]
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(egui::Id::new("hex-copy-page"), copy_button.rect)
                            });
                            if copy_button.clicked() {
                                copy = self
                                    .page
                                    .as_ref()
                                    .map(|page| page.text_in_format(self.byte_format));
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Hex bytes");
                            let query = ui.add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut self.search_input)
                                    .char_limit(1024)
                                    .desired_width(260.0)
                                    .hint_text("DE AD BE EF"),
                            );
                            query.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::TextEdit,
                                    query.enabled(),
                                    "Hexadecimal search bytes",
                                )
                            });
                            if query.changed() {
                                self.last_match = None;
                                self.search_message = None;
                            }
                            let next = ui.add_enabled(
                                !busy && self.page.is_some(),
                                egui::Button::new("Find next"),
                            );
                            let previous = ui.add_enabled(
                                !busy && self.page.is_some(),
                                egui::Button::new("Find previous"),
                            );
                            if next.clicked()
                                || (!busy
                                    && query.lost_focus()
                                    && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                            {
                                search = Some(crate::hex_search::Direction::Forward);
                            }
                            if previous.clicked() {
                                search = Some(crate::hex_search::Direction::Backward);
                            }
                            #[cfg(feature = "visual-qa")]
                            ctx.data_mut(|data| {
                                data.insert_temp(egui::Id::new("hex-query"), query.rect);
                                data.insert_temp(egui::Id::new("hex-find-next"), next.rect);
                                data.insert_temp(egui::Id::new("hex-find-previous"), previous.rect);
                            });
                        });
                        if let Some(message) = &self.search_message {
                            ui.label(message);
                        }
                        if busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                if let Some(progress) =
                                    self.task.as_ref().and_then(|task| task.progress.as_ref())
                                {
                                    ui.label(format!(
                                        "Searching… read {} bytes",
                                        progress.load(std::sync::atomic::Ordering::Relaxed)
                                    ));
                                } else {
                                    ui.label("Reading page…");
                                }
                                if ui.button("Cancel").clicked() {
                                    cancel = true;
                                }
                            });
                            ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        }
                        if let Some(error) = &self.error {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        if let Some(page) = &self.page {
                            ui.label(format!(
                                "Offset 0x{:X} ({}) · {} bytes shown · {} bytes in file",
                                page.offset,
                                page.offset,
                                page.bytes.len(),
                                page.stamp.identity.size
                            ));
                            if page.bytes.is_empty() {
                                ui.label(if page.stamp.identity.size == 0 {
                                    "Empty file"
                                } else {
                                    "End of file"
                                });
                            }
                            let height = ui.text_style_height(&egui::TextStyle::Monospace)
                                + ui.spacing().item_spacing.y;
                            egui::ScrollArea::both()
                                .id_salt(("hex-bytes", page.offset, self.generation))
                                .max_height(400.0)
                                .show_rows(
                                    ui,
                                    height,
                                    page.bytes.len().div_ceil(ROW_BYTES),
                                    |ui, range| {
                                        for row in range {
                                            ui.add(
                                                egui::Label::new({
                                                    let text = egui::RichText::new(
                                                        page.row_in_format(row, self.byte_format),
                                                    )
                                                    .monospace();
                                                    if self.last_match.as_ref().is_some_and(
                                                        |(offset, pattern)| {
                                                            *offset == page.offset
                                                                && row * ROW_BYTES < pattern.len()
                                                        },
                                                    ) {
                                                        text.background_color(
                                                            ui.visuals().selection.bg_fill,
                                                        )
                                                        .color(ui.visuals().strong_text_color())
                                                    } else {
                                                        text
                                                    }
                                                })
                                                .extend()
                                                .selectable(true),
                                            );
                                        }
                                    },
                                );
                        }
                    });
            });
        if cancel {
            self.task = None;
            self.page = None;
            self.pending_pattern = None;
            self.last_match = None;
            self.search_message = None;
            self.error = Some("Hex operation cancelled".into());
        }
        if reload {
            self.stamp = None;
            self.request(0, ctx, workload);
        } else if let Some(offset) = navigate {
            self.request(offset, ctx, workload);
        } else if let Some(direction) = search {
            self.search(direction, ctx, workload);
        }
        (open, copy)
    }
    #[cfg(feature = "visual-qa")]
    pub fn matched_offset(&self) -> Option<u64> {
        self.last_match
            .as_ref()
            .map(|(offset, _)| *offset)
            .filter(|offset| self.shown_offset() == Some(*offset))
    }
    #[cfg(feature = "visual-qa")]
    pub fn decimal_mode(&self) -> bool {
        self.byte_format == ByteFormat::Decimal
    }
    #[cfg(feature = "visual-qa")]
    pub fn shown_offset(&self) -> Option<u64> {
        self.page
            .as_ref()
            .map(|page| page.offset)
            .filter(|_| self.task.is_none() && self.error.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn byte_search_moves_both_directions_keeps_the_page_on_no_match_and_does_not_wrap() {
        use crate::hex_search::Direction;
        let temp = TempDir::new();
        let path = temp.path().join("search");
        let mut bytes = vec![0x11; 131104];
        for offset in [4094, 65534] {
            bytes[offset..offset + 4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        }
        std::fs::write(&path, &bytes).unwrap();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path.clone());
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        viewer.search_input = "de ad be ef".into();
        for (direction, offset) in [
            (Direction::Forward, 4094),
            (Direction::Forward, 65534),
            (Direction::Forward, 65534),
            (Direction::Backward, 4094),
            (Direction::Backward, 4094),
        ] {
            viewer.search(direction, &ctx, &runtime.handle());
            assert!(viewer.task.as_ref().unwrap().progress.is_some());
            assert!(runtime.run_next());
            viewer.poll();
            assert_eq!(viewer.page.as_ref().unwrap().offset, offset);
            assert_eq!(viewer.last_match.as_ref().unwrap().0, offset);
            assert_eq!(
                &viewer.page.as_ref().unwrap().bytes[..4],
                &[0xde, 0xad, 0xbe, 0xef]
            );
        }
        assert!(
            viewer
                .search_message
                .as_deref()
                .unwrap()
                .contains("No match")
        );
        assert_eq!(runtime.stats().cancelled, 0);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn byte_search_handles_overlap_and_its_last_completion_cannot_survive_cancel() {
        use crate::hex_search::Direction;
        let temp = TempDir::new();
        let path = temp.file("overlap", "AAAAA");
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path);
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        viewer.search_input = "41 41 41".into();
        for expected in [0, 1, 2] {
            viewer.search(Direction::Forward, &ctx, &runtime.handle());
            assert!(runtime.run_next());
            viewer.poll();
            assert_eq!(viewer.last_match.as_ref().unwrap().0, expected);
        }
        viewer.search(Direction::Backward, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.task = None; // discard a completed search before its response is consumed
        viewer.request(4, &ctx, &runtime.handle());
        viewer.poll();
        assert!(viewer.page.is_none());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.page.as_ref().unwrap().offset, 4);
        assert!(viewer.last_match.is_none());
    }

    #[test]
    fn file_replacement_during_search_does_not_publish_match_or_no_match() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let temp = TempDir::new();
        for matching in [false, true] {
            let path = temp.path().join(format!("changing-{matching}"));
            let mut data = vec![0; crate::hex_search::CHUNK_BYTES * 2];
            if matching {
                data[crate::hex_search::CHUNK_BYTES + 2] = 1;
            }
            std::fs::write(&path, data).unwrap();
            let stamp = read_page(&path, 0, None, &|| false).unwrap().stamp;
            let progress = AtomicU64::new(0);
            let replaced = std::cell::Cell::new(false);
            let result = search_page(
                &path,
                &stamp,
                0,
                &[1],
                crate::hex_search::Direction::Forward,
                &|| {
                    if progress.load(Ordering::Relaxed) >= crate::hex_search::CHUNK_BYTES as u64
                        && !replaced.replace(true)
                    {
                        std::fs::rename(&path, temp.path().join(format!("old-{matching}")))
                            .unwrap();
                        std::fs::write(&path, "new file").unwrap();
                    }
                    false
                },
                &progress,
            );
            assert!(result.unwrap_err().contains("changed"));
        }
    }

    #[test]
    fn viewer_has_one_pending_read_and_closing_cancels_only_its_task() {
        let temp = TempDir::new();
        let path = temp.file("pages", &"a".repeat(PAGE_BYTES * 2));
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let handle = runtime.handle();
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path.clone());
        viewer.request(0, &ctx, &handle);
        viewer.request(4096, &ctx, &handle);
        assert_eq!(runtime.stats().queued, 1);
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
        assert_eq!(runtime.stats().cancelled, 0);
        viewer.request(4096, &ctx, &handle);
        assert!(viewer.page.is_none());
        drop(viewer);
        assert_eq!(runtime.stats().cancelled, 1);
        assert!(!runtime.run_next());
        let mut next = HexViewer::new(path);
        next.request(4096, &ctx, &handle);
        assert!(runtime.run_next());
        next.poll();
        assert_eq!(next.page.as_ref().unwrap().offset, 4096);
        assert_eq!(runtime.stats().cancelled, 1);
    }

    #[test]
    fn cancelled_completion_cannot_replace_a_newer_page_and_abandonment_is_terminal() {
        let temp = TempDir::new();
        let path = temp.file("pages", &"b".repeat(PAGE_BYTES * 2));
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let handle = runtime.handle();
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path);
        viewer.request(0, &ctx, &handle);
        assert!(runtime.run_next()); // completion is queued in the old receiver
        viewer.task = None; // Cancel/close discards that receiver
        viewer.request(4096, &ctx, &handle);
        viewer.poll();
        assert!(viewer.page.is_none() && viewer.error.is_none());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.page.as_ref().unwrap().offset, 4096);
        viewer.request(0, &ctx, &handle);
        assert!(viewer.task.as_ref().unwrap().handle.cancel());
        viewer.poll();
        assert!(viewer.task.is_none() && viewer.page.is_none());
        assert!(viewer.error.as_deref().unwrap().contains("stopped"));
    }

    #[test]
    fn admission_failure_does_not_leave_a_spinner_or_a_stale_page() {
        let temp = TempDir::new();
        let path = temp.file("file", "abc");
        let runtime =
            crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits {
                max_queued: 0,
                ..Default::default()
            });
        let mut viewer = HexViewer::new(path);
        viewer.request(0, &egui::Context::default(), &runtime.handle());
        assert!(viewer.task.is_none() && viewer.page.is_none());
        assert!(viewer.error.as_deref().unwrap().contains("queue is full"));
    }

    #[test]
    fn pages_preserve_all_bytes_and_absolute_offsets() {
        let temp = TempDir::new();
        let path = temp.path().join("binary");
        let bytes: Vec<u8> = (0..8197).map(|index| index as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let first = read_page(&path, 0, None, &|| false).unwrap();
        assert_eq!(first.bytes, bytes[..PAGE_BYTES]);
        assert_eq!(first.next_offset(), Some(4096));
        let next = read_page(&path, 4096, Some(&first.stamp), &|| false).unwrap();
        assert_eq!(next.bytes, bytes[4096..8192]);
        assert!(next.row(0).starts_with("0000000000001000  00 01 02"));
        let last = read_page(&path, 8192, Some(&first.stamp), &|| false).unwrap();
        assert_eq!(last.bytes, [0, 1, 2, 3, 4]);
        assert_eq!(last.next_offset(), None);
        assert!(last.text().starts_with("0000000000002000  00 01 02 03 04"));
        assert!(last.text().ends_with(" | ....."));
        let unaligned = read_page(&path, 31, Some(&first.stamp), &|| false).unwrap();
        assert_eq!(unaligned.bytes, bytes[31..31 + PAGE_BYTES]);
        assert!(unaligned.row(1).starts_with("000000000000002F"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn decimal_mode_pointer_switch_and_copy_use_loaded_page_without_new_io() {
        let temp = TempDir::new();
        let path = temp.path().join("bytes");
        std::fs::write(&path, [0, 1, 32, 127, 128, 255]).unwrap();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path.clone());
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.byte_format, ByteFormat::Hex);
        let mut copied = None;
        for frame in 0..9 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 760.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4, 6, 7].contains(&frame) {
                let key = if frame < 6 {
                    egui::Id::new(("hex-byte-format", "Decimal"))
                } else {
                    egui::Id::new("hex-copy-page")
                };
                let rect = ctx.data(|data| data.get_temp::<egui::Rect>(key)).unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 3 || frame == 6,
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
                let (_, text) = viewer.show(&ctx, &runtime.handle());
                if text.is_some() {
                    copied = text;
                }
            }));
        }
        assert_eq!(viewer.byte_format, ByteFormat::Decimal);
        assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
        assert_eq!(
            copied.as_deref(),
            Some(
                viewer
                    .page
                    .as_ref()
                    .unwrap()
                    .text_in_format(ByteFormat::Decimal)
                    .as_str()
            )
        );
        assert!(copied.unwrap().contains("000 001 032 127 128 255"));
        assert!(
            !runtime.run_next(),
            "changing representation must not reread the file"
        );
        assert_eq!(std::fs::read(path).unwrap(), [0, 1, 32, 127, 128, 255]);
        assert_eq!(
            HexViewer::new(temp.path().join("other")).byte_format,
            ByteFormat::Hex
        );
    }

    #[test]
    fn decimal_bytes_round_trip_all_values_keep_ascii_and_align_short_rows() {
        let temp = TempDir::new();
        let path = temp.path().join("bytes");
        std::fs::write(&path, (0..=255).collect::<Vec<u8>>()).unwrap();
        let page = read_page(&path, 0, None, &|| false).unwrap();
        for row in 0..16 {
            let decimal = page.row_in_format(row, ByteFormat::Decimal);
            let (values, ascii) = decimal[18..].split_once(" | ").unwrap();
            assert_eq!(
                values
                    .split_whitespace()
                    .map(|value| value.parse::<u8>().unwrap())
                    .collect::<Vec<_>>(),
                page.bytes[row * 16..row * 16 + 16]
            );
            assert_eq!(ascii, page.row(row).split_once(" | ").unwrap().1);
        }
        let tail = read_page(&path, 253, Some(&page.stamp), &|| false).unwrap();
        let text = tail.text_in_format(ByteFormat::Decimal);
        assert!(text.starts_with("00000000000000FD  253 254 255 "));
        assert!(text.ends_with(" | ..."));
        assert_eq!(
            text.find(" | "),
            page.row_in_format(0, ByteFormat::Decimal).find(" | ")
        );
        assert_eq!(std::fs::read(path).unwrap(), page.bytes);
    }

    #[test]
    fn ascii_column_never_emits_controls_and_hex_is_exact() {
        let temp = TempDir::new();
        let path = temp.path().join("all-bytes");
        std::fs::write(&path, (0..=255).collect::<Vec<u8>>()).unwrap();
        let page = read_page(&path, 0, None, &|| false).unwrap();
        for row in 0..16 {
            let text = page.row(row);
            let (hex, ascii) = text[18..].split_once(" | ").unwrap();
            let reconstructed: Vec<u8> = hex
                .split_whitespace()
                .map(|byte| u8::from_str_radix(byte, 16).unwrap())
                .collect();
            assert_eq!(reconstructed, page.bytes[row * 16..row * 16 + 16]);
            assert_eq!(ascii.len(), 16);
            assert!(ascii.bytes().all(|byte| (0x20..=0x7e).contains(&byte)));
        }
        assert!(page.row(2).ends_with(" |  !\"#$%&'()*+,-./"));
    }

    #[test]
    fn empty_files_eof_and_out_of_range_are_distinct() {
        let temp = TempDir::new();
        let path = temp.file("empty", "");
        let empty = read_page(&path, 0, None, &|| false).unwrap();
        assert!(empty.bytes.is_empty() && empty.text().is_empty());
        assert_eq!(empty.next_offset(), None);
        assert!(
            read_page(&path, 1, None, &|| false)
                .unwrap_err()
                .contains("beyond")
        );
        let path = temp.file("three", "abc");
        let eof = read_page(&path, 3, None, &|| false).unwrap();
        assert!(eof.bytes.is_empty());
        assert_eq!(eof.stamp.identity.size, 3);
    }

    #[test]
    fn offsets_are_strict_and_checked_without_integer_wrap() {
        assert_eq!(parse_offset(" 0x2000 "), Ok(8192));
        assert_eq!(parse_offset("0XfF"), Ok(255));
        assert_eq!(parse_offset("4096"), Ok(4096));
        assert_eq!(parse_offset("18446744073709551615"), Ok(u64::MAX));
        for value in [
            "",
            "-1",
            "0x",
            "1.5",
            "12tail",
            "0x10000000000000000",
            "18446744073709551616",
        ] {
            assert!(parse_offset(value).is_err(), "{value}");
        }
    }

    #[test]
    fn sparse_files_beyond_four_gib_use_u64_seeking_and_bounded_memory() {
        use std::io::Write;
        let temp = TempDir::new();
        let path = temp.path().join("sparse");
        let mut file = std::fs::File::create(&path).unwrap();
        let offset = (1u64 << 32) + 37;
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(b"tail").unwrap();
        drop(file);
        let page = read_page(&path, offset, None, &|| false).unwrap();
        assert_eq!(page.bytes, b"tail");
        assert_eq!(page.stamp.identity.size, offset + 4);
        assert!(page.text().starts_with("0000000100000025"));
        for direction in [
            crate::hex_search::Direction::Forward,
            crate::hex_search::Direction::Backward,
        ] {
            let found = search_page(
                &path,
                &page.stamp,
                offset,
                b"tail",
                direction,
                &|| false,
                &std::sync::atomic::AtomicU64::new(0),
            )
            .unwrap()
            .unwrap();
            assert_eq!(found.offset, offset);
        }
    }

    #[test]
    fn cancellation_and_replacement_never_publish_a_partial_page() {
        let temp = TempDir::new();
        let path = temp.file("input", &"a".repeat(PAGE_BYTES * 2));
        let first = read_page(&path, 0, None, &|| false).unwrap();
        let ticks = std::cell::Cell::new(0);
        assert!(
            read_page(&path, 0, None, &|| {
                ticks.set(ticks.get() + 1);
                ticks.get() == 3
            })
            .unwrap_err()
            .contains("cancelled")
        );
        let ticks = std::cell::Cell::new(0);
        assert!(
            read_page(&path, 0, None, &|| {
                ticks.set(ticks.get() + 1);
                if ticks.get() == 4 {
                    std::fs::rename(&path, temp.path().join("old")).unwrap();
                    std::fs::write(&path, "replacement").unwrap();
                }
                false
            })
            .unwrap_err()
            .contains("changed")
        );
        assert!(
            read_page(&path, 0, Some(&first.stamp), &|| false)
                .unwrap_err()
                .contains("changed")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_directories_and_fifo_are_not_opened_as_files() {
        let temp = TempDir::new();
        let path = temp.file("input", "abc");
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(
            read_page(&link, 0, None, &|| false)
                .unwrap_err()
                .contains("regular")
        );
        assert!(
            read_page(temp.path(), 0, None, &|| false)
                .unwrap_err()
                .contains("regular")
        );
        let fifo = temp.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(
            read_page(&fifo, 0, None, &|| false)
                .unwrap_err()
                .contains("regular")
        );
        let changed = std::cell::Cell::new(0);
        assert!(
            read_page(&path, 0, None, &|| {
                changed.set(changed.get() + 1);
                if changed.get() == 2 {
                    std::fs::remove_file(&path).unwrap();
                    std::os::unix::fs::symlink(&link, &path).unwrap();
                }
                false
            })
            .is_err()
        );
    }
}
