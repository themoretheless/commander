//! Read-only paged binary inspection and cancellable streaming byte search.
use crate::path_identity::PathIdentity;
use crate::workload::{Priority, TaskHandle, TaskKind, TaskSpec, WorkloadHandle};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

pub const PAGE_BYTES: usize = 4096;

fn position_percent(offset: u64, size: u64) -> String {
    if size == 0 {
        return "n/a".into();
    }
    let hundredths = u128::from(offset.min(size)) * 10_000 / u128::from(size);
    format!("{}.{:02}%", hundredths / 100, hundredths % 100)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowWidth(u8);
impl Default for RowWidth {
    fn default() -> Self {
        Self(16)
    }
}
impl RowWidth {
    fn new(bytes: u8) -> Self {
        Self(bytes.clamp(1, 64))
    }
    fn bytes(self) -> usize {
        usize::from(self.0)
    }
}

fn recent_entry_ui(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    hint: &str,
) -> (egui::Response, egui::Response) {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 80.0).clamp(1.0, 340.0);
        let recall = ui
            .allocate_ui_with_layout(
                egui::vec2(width, ui.spacing().interact_size.y),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| ui.add_enabled(enabled, egui::Button::new(label).truncate()),
            )
            .inner
            .on_hover_text(hint);
        let remove = ui
            .add_enabled(enabled, egui::Button::new("Remove"))
            .on_hover_text("Remove this history entry without changing the current page or input.");
        (recall, remove)
    })
    .inner
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ByteFormat {
    #[default]
    Hex,
    Decimal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CharacterEncoding {
    #[default]
    Ascii,
    Windows1251,
    Windows1252,
    Koi8R,
    Koi8U,
    Ibm866,
    Utf8,
    Utf16Le,
    Utf16Be,
}
impl CharacterEncoding {
    const ALL: [(Self, &'static str); 9] = [
        (Self::Ascii, "ASCII"),
        (Self::Windows1251, "Windows-1251"),
        (Self::Windows1252, "Windows-1252"),
        (Self::Koi8R, "KOI8-R"),
        (Self::Koi8U, "KOI8-U"),
        (Self::Ibm866, "IBM866"),
        (Self::Utf8, "UTF-8"),
        (Self::Utf16Le, "UTF-16 LE"),
        (Self::Utf16Be, "UTF-16 BE"),
    ];
    fn from_bom(bytes: &[u8]) -> Option<Self> {
        // Check longer unsupported signatures before their UTF-16 prefix.
        if bytes.starts_with(&[0xff, 0xfe, 0, 0]) || bytes.starts_with(&[0, 0, 0xfe, 0xff]) {
            None
        } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            Some(Self::Utf8)
        } else if bytes.starts_with(&[0xff, 0xfe]) {
            Some(Self::Utf16Le)
        } else if bytes.starts_with(&[0xfe, 0xff]) {
            Some(Self::Utf16Be)
        } else {
            None
        }
    }
    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|(encoding, _)| *encoding == self)
            .unwrap();
        Self::ALL[(index + 1) % Self::ALL.len()].0
    }
    fn label(self) -> &'static str {
        Self::ALL
            .into_iter()
            .find(|(mode, _)| *mode == self)
            .unwrap()
            .1
    }
    #[cfg(test)]
    fn glyph_at(self, bytes: &[u8], index: usize) -> char {
        self.glyph_at_offset(bytes, index, 0)
    }
    fn glyph_at_offset(self, bytes: &[u8], index: usize, context_offset: u64) -> char {
        if matches!(self, Self::Utf16Le | Self::Utf16Be) {
            let unit = |start: usize| {
                bytes.get(start..start + 2).map(|b| {
                    if self == Self::Utf16Le {
                        u16::from_le_bytes([b[0], b[1]])
                    } else {
                        u16::from_be_bytes([b[0], b[1]])
                    }
                })
            };
            for start in index.saturating_sub(3)..=index {
                if !(context_offset + start as u64).is_multiple_of(2) {
                    continue;
                }
                let Some(first) = unit(start) else {
                    continue;
                };
                let (scalar, length) = match first {
                    0xd800..=0xdbff => {
                        let Some(second @ 0xdc00..=0xdfff) = unit(start + 2) else {
                            continue;
                        };
                        (
                            0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second)
                                - 0xdc00,
                            4,
                        )
                    }
                    0xdc00..=0xdfff => continue,
                    _ => (u32::from(first), 2),
                };
                if index >= start + length {
                    continue;
                }
                return if index == start {
                    visible_character(char::from_u32(scalar).unwrap())
                } else {
                    '›'
                };
            }
            return '.';
        }
        if self != Self::Utf8 {
            return self.glyph(bytes[index]);
        }
        for start in index.saturating_sub(3)..=index {
            let length = match bytes[start] {
                0x00..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => continue,
            };
            if index >= start + length {
                continue;
            }
            if let Some(sequence) = bytes.get(start..start + length)
                && let Ok(text) = std::str::from_utf8(sequence)
            {
                return if index == start {
                    visible_character(text.chars().next().unwrap())
                } else {
                    '›'
                };
            }
        }
        '.'
    }
    fn glyph(self, byte: u8) -> char {
        let encoding = match self {
            Self::Utf16Le | Self::Utf16Be => return '.',
            Self::Utf8 | Self::Ascii => {
                return if (0x20..=0x7e).contains(&byte) {
                    char::from(byte)
                } else {
                    '.'
                };
            }
            Self::Windows1251 => encoding_rs::WINDOWS_1251,
            Self::Windows1252 => encoding_rs::WINDOWS_1252,
            Self::Koi8R => encoding_rs::KOI8_R,
            Self::Koi8U => encoding_rs::KOI8_U,
            Self::Ibm866 => encoding_rs::IBM866,
        };
        static TABLES: [std::sync::OnceLock<[char; 256]>; 5] =
            [const { std::sync::OnceLock::new() }; 5];
        TABLES[self as usize - 1].get_or_init(|| {
            std::array::from_fn(|index| {
                let bytes = [index as u8];
                let (text, errors) = encoding.decode_without_bom_handling(&bytes);
                let ch = text.chars().next().unwrap_or('.');
                if errors { '.' } else { visible_character(ch) }
            })
        })[usize::from(byte)]
    }
}

fn visible_character(ch: char) -> char {
    if ch.is_control()
        || (0xfdd0..=0xfdef).contains(&u32::from(ch))
        || u32::from(ch) & 0xffff >= 0xfffe
        || ch == char::REPLACEMENT_CHARACTER
        || (ch.is_whitespace() && ch != ' ')
        || unicode_normalization::char::is_combining_mark(ch)
        || matches!(ch, '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'
            | '\u{e0000}'..='\u{e007f}')
    {
        '.'
    } else {
        ch
    }
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
    // Same validated snapshot: up to three bytes before/after the public page.
    character_context: Vec<u8>,
    character_prefix: usize,
}
pub fn page_navigation_key(input: &egui::InputState) -> bool {
    let modifiers = input.modifiers;
    !modifiers.alt
        && !modifiers.shift
        && !modifiers.mac_cmd
        && ((modifiers.ctrl
            && (input.key_pressed(egui::Key::ArrowLeft)
                || input.key_pressed(egui::Key::ArrowRight)))
            || input.key_pressed(egui::Key::Home)
            || input.key_pressed(egui::Key::End)
            || (!modifiers.ctrl
                && (input.key_pressed(egui::Key::PageUp)
                    || input.key_pressed(egui::Key::PageDown))))
}
pub fn focus_shortcut(input: &egui::InputState) -> bool {
    input.modifiers.is_none()
        && (input.key_pressed(egui::Key::F5) || input.key_pressed(egui::Key::F7))
}
pub fn character_shortcut(input: &egui::InputState) -> bool {
    let m = input.modifiers;
    !m.ctrl && !m.alt && !m.command && !m.mac_cmd && input.key_pressed(egui::Key::F8)
}
pub fn repeat_search_shortcut(input: &egui::InputState) -> Option<crate::hex_search::Direction> {
    use crate::hex_search::Direction;
    let m = input.modifiers;
    if m.ctrl || m.command || m.mac_cmd {
        return None;
    }
    if m.is_none() && input.key_pressed(egui::Key::Space)
        || m.shift && !m.alt && input.key_pressed(egui::Key::F7)
    {
        Some(Direction::Forward)
    } else if m.alt && !m.shift && input.key_pressed(egui::Key::F7) {
        Some(Direction::Backward)
    } else {
        None
    }
}
pub fn copy_match_shortcut(input: &egui::InputState) -> bool {
    let m = input.modifiers;
    !m.alt
        && !m.shift
        && !(m.ctrl && m.mac_cmd)
        && (input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Copy))
            || ((m.ctrl || m.command || m.mac_cmd) && input.key_pressed(egui::Key::C))
            || (m.ctrl && !m.mac_cmd && input.key_pressed(egui::Key::Insert)))
}
pub fn clear_match_shortcut(input: &egui::InputState) -> bool {
    input.modifiers.ctrl
        && !input.modifiers.alt
        && !input.modifiers.shift
        && !input.modifiers.mac_cmd
        && input.key_pressed(egui::Key::U)
}
pub fn close_shortcut(input: &egui::InputState) -> bool {
    input.modifiers.is_none()
        && (input.key_pressed(egui::Key::Escape) || input.key_pressed(egui::Key::F10))
}
impl Page {
    fn previous_offset(&self) -> u64 {
        self.offset.saturating_sub(PAGE_BYTES as u64)
    }
    fn last_offset(&self) -> u64 {
        self.stamp.identity.size.saturating_sub(1) / PAGE_BYTES as u64 * PAGE_BYTES as u64
    }
    fn next_offset(&self) -> Option<u64> {
        let next = self.offset.saturating_add(self.bytes.len() as u64);
        (next < self.stamp.identity.size).then_some(next)
    }
    #[cfg(test)]
    fn row(&self, row: usize) -> String {
        self.row_in_format(row, ByteFormat::Hex)
    }
    #[cfg(test)]
    fn row_in_format(&self, row: usize, format: ByteFormat) -> String {
        self.row_in_layout(row, format, RowWidth::default())
    }
    #[cfg(test)]
    fn row_in_layout(&self, row: usize, format: ByteFormat, width: RowWidth) -> String {
        self.row_in_encoding(row, format, width, CharacterEncoding::Ascii)
    }
    fn row_in_encoding(
        &self,
        row: usize,
        format: ByteFormat,
        width: RowWidth,
        characters: CharacterEncoding,
    ) -> String {
        use std::fmt::Write;
        let row_bytes = width.bytes();
        let start = row.saturating_mul(row_bytes).min(self.bytes.len());
        let bytes = &self.bytes[start..self.bytes.len().min(start + row_bytes)];
        let mut text = format!("{:016X}  ", self.offset + start as u64);
        for index in 0..row_bytes {
            if index > 0 && index % 8 == 0 {
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
        for index in start..start + bytes.len() {
            text.push(characters.glyph_at_offset(
                &self.character_context,
                self.character_prefix + index,
                self.offset - self.character_prefix as u64,
            ));
        }
        text
    }
    #[cfg(test)]
    fn text(&self) -> String {
        self.text_in_format(ByteFormat::Hex)
    }
    #[cfg(test)]
    fn text_in_format(&self, format: ByteFormat) -> String {
        self.text_in_layout(format, RowWidth::default())
    }
    #[cfg(test)]
    fn text_in_layout(&self, format: ByteFormat, width: RowWidth) -> String {
        self.text_in_encoding(format, width, CharacterEncoding::Ascii)
    }
    fn text_in_encoding(
        &self,
        format: ByteFormat,
        width: RowWidth,
        characters: CharacterEncoding,
    ) -> String {
        (0..self.bytes.len().div_ceil(width.bytes()))
            .map(|row| self.row_in_encoding(row, format, width, characters))
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn byte_text(&self, format: ByteFormat) -> String {
        byte_text(&self.bytes, format)
    }
}

fn byte_text(bytes: &[u8], format: ByteFormat) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 4);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        match format {
            ByteFormat::Hex => {
                let _ = write!(text, "{byte:02X}");
            }
            ByteFormat::Decimal => {
                let _ = write!(text, "{byte:03}");
            }
        }
    }
    text
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
    let context_offset = offset.saturating_sub(3);
    let character_prefix = (offset - context_offset) as usize;
    file.seek(SeekFrom::Start(context_offset))
        .map_err(|error| error.to_string())?;
    let length = (before.identity.size - offset).min(PAGE_BYTES as u64) as usize;
    let context_length = (before.identity.size - context_offset)
        .min((character_prefix + length + 3) as u64) as usize;
    let mut character_context = vec![0; context_length];
    let mut read = 0;
    while read < context_length {
        check_cancelled(cancelled)?;
        match file.read(&mut character_context[read..]) {
            Ok(0) => return Err(changed()),
            Ok(count) => read += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    check_cancelled(cancelled)?;
    validate_bound(path, &file, &before)?;
    check_cancelled(cancelled)?;
    let bytes = character_context[character_prefix..character_prefix + length].to_vec();
    Ok(Page {
        stamp: before,
        offset,
        bytes,
        character_context,
        character_prefix,
    })
}

#[cfg(test)]
fn search_page(
    path: &Path,
    expected: &Stamp,
    start: u64,
    pattern: &[u8],
    direction: crate::hex_search::Direction,
    cancelled: &dyn Fn() -> bool,
    progress: &std::sync::atomic::AtomicU64,
) -> Result<Option<Page>, String> {
    search_page_options(
        path,
        expected,
        pattern,
        crate::hex_search::Options {
            start: Some(start),
            direction,
            wrap: false,
        },
        cancelled,
        progress,
    )
    .map(|(page, _)| page)
}

fn search_page_options(
    path: &Path,
    expected: &Stamp,
    pattern: &[u8],
    options: crate::hex_search::Options,
    cancelled: &dyn Fn() -> bool,
    progress: &std::sync::atomic::AtomicU64,
) -> Result<(Option<Page>, bool), String> {
    let (mut file, before) = open_bound(path, Some(expected), cancelled)?;
    let found = crate::hex_search::find_options(
        &mut file,
        before.identity.size,
        pattern,
        options,
        cancelled,
        progress,
    )?;
    check_cancelled(cancelled)?;
    validate_bound(path, &file, &before)?;
    let wrapped = found.is_some_and(|(_, wrapped)| wrapped);
    found
        .map(|(offset, _)| read_page(path, offset, Some(&before), cancelled))
        .transpose()
        .map(|page| (page, wrapped))
}

fn parse_offset(input: &str, file_size: Option<u64>) -> Result<u64, &'static str> {
    let input = input.trim();
    if let Some(percent) = input.strip_suffix('%') {
        let percent = percent
            .parse::<u8>()
            .ok()
            .filter(|value| *value <= 100)
            .filter(|_| !percent.is_empty() && percent.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or("Enter a whole percentage from 0% to 100%.")?;
        let size = file_size.ok_or("Load the file before using a percentage offset.")?;
        return Ok((u128::from(size) * u128::from(percent) / 100) as u64);
    }
    let error = "Use decimal bytes (optionally m), or hexadecimal with 0x, $ or a trailing h.";
    if let Some(decimal) = input.strip_suffix('m').or_else(|| input.strip_suffix('M')) {
        if decimal.is_empty() || !decimal.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(error);
        }
        return decimal.parse().map_err(|_| error);
    }
    let hex_suffix = input.strip_suffix('h').or_else(|| input.strip_suffix('H'));
    let number = hex_suffix.unwrap_or(input);
    let hex_prefix = number
        .strip_prefix("0x")
        .or_else(|| number.strip_prefix("0X"))
        .or_else(|| number.strip_prefix('$'));
    let value = if let Some(hex) = hex_prefix {
        if hex.is_empty() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(error);
        }
        u64::from_str_radix(hex, 16)
    } else if hex_suffix.is_some() {
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(error);
        }
        u64::from_str_radix(number, 16)
    } else {
        input.parse()
    };
    value.map_err(|_| error)
}

fn parse_position(
    input: &str,
    file_size: Option<u64>,
    current: Option<u64>,
) -> Result<u64, &'static str> {
    let input = input.trim();
    let Some(sign) = input
        .as_bytes()
        .first()
        .copied()
        .filter(|ch| matches!(ch, b'+' | b'-'))
    else {
        return parse_offset(input, file_size);
    };
    let rest = &input[1..];
    if rest.is_empty()
        || rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_whitespace() || ch == '+' || ch == '-')
    {
        return Err("Put a decimal, hexadecimal or whole-percent magnitude directly after + or -.");
    }
    let current = current.ok_or("Load a page before using a relative offset.")?;
    let delta = parse_offset(rest, file_size)?;
    let offset = if sign == b'+' {
        current.checked_add(delta)
    } else {
        current.checked_sub(delta)
    }
    .ok_or("Relative offset would leave the valid byte-address range.")?;
    if file_size.is_some_and(|size| offset > size) {
        return Err("Relative offset is beyond the end of the file.");
    }
    Ok(offset)
}

enum Completion {
    Page(Page),
    Search {
        page: Option<Page>,
        wrapped: bool,
        cyclic: bool,
    },
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

#[derive(Clone, PartialEq, Eq)]
struct RecentSearch {
    query: String,
    mode: crate::hex_search::InputMode,
    wrap: bool,
}
#[derive(Default)]
struct SearchHistory(std::collections::VecDeque<RecentSearch>);
impl SearchHistory {
    fn record(&mut self, query: &str, mode: crate::hex_search::InputMode, wrap: bool) {
        const MAX_TEXT_BYTES: usize = 16 * 1024;
        if query.len() > MAX_TEXT_BYTES {
            return;
        }
        self.0
            .retain(|entry| entry.query != query || entry.mode != mode || entry.wrap != wrap);
        self.0.push_front(RecentSearch {
            query: query.to_owned(),
            mode,
            wrap,
        });
        while self.0.len() > 20
            || self.0.iter().map(|entry| entry.query.len()).sum::<usize>() > MAX_TEXT_BYTES
        {
            self.0.pop_back();
        }
    }
}

#[derive(Default)]
struct PositionHistory {
    current: Option<u64>,
    past: std::collections::VecDeque<u64>,
    future: std::collections::VecDeque<u64>,
}
impl PositionHistory {
    fn target(&self, back: bool) -> Option<u64> {
        if back {
            self.past.back()
        } else {
            self.future.back()
        }
        .copied()
    }
    fn trim(&mut self) {
        while self.past.len() + self.future.len() > 64 {
            if self.past.pop_front().is_none() {
                self.future.pop_front();
            }
        }
    }
    fn record(&mut self, offset: u64) {
        if self.current == Some(offset) {
            return;
        }
        if let Some(current) = self.current {
            self.past.push_back(current);
        }
        self.future.clear();
        self.current = Some(offset);
        self.trim();
    }
    fn commit(&mut self, back: bool, offset: u64) {
        if self.target(back) != Some(offset) {
            return;
        }
        if back {
            self.past.pop_back();
        } else {
            self.future.pop_back();
        }
        if let Some(current) = self.current {
            if back {
                self.future.push_back(current);
            } else {
                self.past.push_back(current);
            }
        }
        self.current = Some(offset);
        self.trim();
    }
}

pub struct HexViewer {
    restore_search_focus: bool,
    row_width: RowWidth,
    recent_offsets: std::collections::VecDeque<String>,
    recent_searches: SearchHistory,
    history: PositionHistory,
    pending_history: Option<bool>,
    byte_format: ByteFormat,
    character_encoding: CharacterEncoding,
    character_encoding_explicit: bool,
    detected_bom: Option<CharacterEncoding>,
    bom_known: bool,
    path: PathBuf,
    stamp: Option<Stamp>,
    page: Option<Page>,
    error: Option<String>,
    task: Option<ReadTask>,
    started: bool,
    generation: u64,
    offset_input: String,
    bookmarks: [Option<u64>; 10],
    bookmark_slot: usize,
    search_input: String,
    search_mode: crate::hex_search::InputMode,
    wrap_search: bool,
    last_match: Option<(u64, Vec<u8>)>,
    match_selected: bool,
    search_message: Option<String>,
    pending_pattern: Option<Vec<u8>>,
}
impl HexViewer {
    pub fn new(path: PathBuf) -> Self {
        Self {
            restore_search_focus: false,
            row_width: RowWidth::default(),
            recent_offsets: Default::default(),
            recent_searches: SearchHistory::default(),
            history: PositionHistory::default(),
            pending_history: None,
            byte_format: ByteFormat::default(),
            character_encoding: CharacterEncoding::default(),
            character_encoding_explicit: false,
            detected_bom: None,
            bom_known: false,
            path,
            stamp: None,
            page: None,
            error: None,
            task: None,
            started: false,
            generation: 0,
            offset_input: "0".into(),
            bookmarks: [None; 10],
            bookmark_slot: 0,
            search_input: String::new(),
            search_mode: crate::hex_search::InputMode::default(),
            wrap_search: false,
            last_match: None,
            match_selected: false,
            search_message: None,
            pending_pattern: None,
        }
    }
    fn set_search_mode(&mut self, mode: crate::hex_search::InputMode) {
        if self.task.is_none() && self.search_mode != mode {
            self.search_mode = mode;
            self.last_match = None;
            self.search_message = None;
            self.error = None;
        }
    }
    fn recall_search(&mut self, recent: RecentSearch) {
        if self.task.is_some() {
            return;
        }
        self.set_search_mode(recent.mode);
        self.search_input = recent.query;
        self.wrap_search = recent.wrap;
        self.last_match = None;
        self.search_message = None;
        self.error = None;
    }

    fn go_to_input(&mut self, ctx: &egui::Context, workload: &WorkloadHandle) {
        if self.task.is_some() {
            return;
        }
        match parse_position(
            &self.offset_input,
            self.stamp.as_ref().map(|stamp| stamp.identity.size),
            self.page.as_ref().map(|page| page.offset),
        ) {
            Ok(offset) => {
                let expression = self.offset_input.clone();
                self.request(offset, ctx, workload);
                // UI input is capped at 32 characters; keep a byte cap at the owner too.
                if self.task.is_some() && expression.len() <= 256 {
                    self.recent_offsets.retain(|entry| entry != &expression);
                    self.recent_offsets.push_front(expression);
                    self.recent_offsets.truncate(20);
                }
            }
            Err(error) => self.error = Some(error.into()),
        }
    }

    fn request(&mut self, offset: u64, ctx: &egui::Context, workload: &WorkloadHandle) {
        if self.task.is_some() {
            return;
        }
        self.started = true;
        self.pending_history = None;
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
            Ok(Completion::Search {
                page: Some(page),
                wrapped,
                ..
            }) => {
                self.search_message = Some(format!(
                    "{}Match at 0x{:X}",
                    if wrapped { "Wrapped · " } else { "" },
                    page.offset
                ));
                self.last_match = self
                    .pending_pattern
                    .take()
                    .map(|pattern| (page.offset, pattern));
                self.match_selected = true;
                self.accept_page(page);
            }
            Ok(Completion::Search {
                page: None, cyclic, ..
            }) => {
                self.pending_pattern = None;
                self.search_message = Some(
                    if cyclic {
                        "No match in this file."
                    } else {
                        "No match in this direction; search does not wrap."
                    }
                    .into(),
                );
                self.error = None;
            }
            Err(error) => {
                self.page = None;
                self.pending_history = None;
                self.pending_pattern = None;
                self.error = Some(error);
            }
        }
    }
    fn accept_page(&mut self, page: Page) {
        if page.offset == 0 {
            self.detected_bom = CharacterEncoding::from_bom(&page.bytes);
            self.bom_known = true;
            if !self.character_encoding_explicit {
                self.character_encoding = self.detected_bom.unwrap_or_default();
            }
        }

        if let Some(back) = self.pending_history.take() {
            self.history.commit(back, page.offset);
        } else {
            self.history.record(page.offset);
        }
        self.stamp = Some(page.stamp.clone());
        self.offset_input = format!("0x{:X}", page.offset);
        self.page = Some(page);
        self.error = None;
    }
    fn select_character_encoding(&mut self, encoding: CharacterEncoding) {
        self.character_encoding = encoding;
        self.character_encoding_explicit = true;
    }
    fn use_bom_encoding(&mut self) {
        if self.bom_known {
            self.character_encoding_explicit = false;
            self.character_encoding = self.detected_bom.unwrap_or_default();
        }
    }
    fn clear_match(&mut self) {
        self.match_selected = false;
        self.search_message = None;
    }
    fn matched_bytes(&self) -> Option<&[u8]> {
        if !self.match_selected || self.task.is_some() || self.error.is_some() {
            return None;
        }
        let page = self.page.as_ref()?;
        let (offset, pattern) = self.last_match.as_ref()?;
        let bytes = page.bytes.get(..pattern.len())?;
        (*offset == page.offset && !pattern.is_empty() && bytes == pattern).then_some(bytes)
    }
    fn set_bookmark(&mut self) {
        if self.task.is_none()
            && self.error.is_none()
            && let Some(page) = &self.page
        {
            self.bookmarks[self.bookmark_slot] = Some(page.offset);
        }
    }
    fn reload(&mut self, ctx: &egui::Context, workload: &WorkloadHandle) {
        if self.task.is_some() {
            return;
        }
        self.history = Default::default();
        self.bookmarks.fill(None);
        self.stamp = None;
        self.detected_bom = None;
        self.bom_known = false;
        if !self.character_encoding_explicit {
            self.character_encoding = CharacterEncoding::Ascii;
        }
        self.request(0, ctx, workload);
    }
    fn request_history(&mut self, back: bool, ctx: &egui::Context, workload: &WorkloadHandle) {
        if self.task.is_some() {
            return;
        }
        if let Some(target) = self.history.target(back) {
            self.request(target, ctx, workload);
            if self.task.is_some() {
                self.pending_history = Some(back);
            }
        }
    }
    fn cancel(&mut self) {
        self.restore_search_focus = false;
        self.task = None;
        self.page = None;
        self.pending_history = None;
        self.pending_pattern = None;
        self.last_match = None;
        self.search_message = None;
        self.error = Some("Hex operation cancelled".into());
    }
    fn search(
        &mut self,
        direction: crate::hex_search::Direction,
        ctx: &egui::Context,
        workload: &WorkloadHandle,
    ) {
        self.search_from(direction, false, ctx, workload);
    }

    fn search_from(
        &mut self,
        direction: crate::hex_search::Direction,
        file_boundary: bool,
        ctx: &egui::Context,
        workload: &WorkloadHandle,
    ) {
        use crate::hex_search::Direction;
        if self.task.is_some() {
            return;
        }
        self.pending_history = None;
        let pattern = match self.search_mode.parse(&self.search_input) {
            Ok(pattern) => pattern,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        let Some(stamp) = self.stamp.clone() else {
            return;
        };
        let start = if file_boundary {
            Some(match direction {
                Direction::Forward => 0,
                Direction::Backward => stamp.identity.size.saturating_sub(1),
            })
        } else if let Some((offset, _)) =
            self.last_match.as_ref().filter(|(_, old)| old == &pattern)
        {
            match direction {
                Direction::Forward => offset.checked_add(1),
                Direction::Backward => offset.checked_sub(1),
            }
        } else {
            self.page.as_ref().map(|page| page.offset)
        };
        if start.is_none() && !self.wrap_search {
            self.search_message = Some("No match in this direction; search does not wrap.".into());
            return;
        }
        let options = crate::hex_search::Options {
            start,
            direction,
            wrap: self.wrap_search,
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
            let result = search_page_options(
                &path,
                &stamp,
                &pattern,
                options,
                &|| cancelled.is_cancelled(),
                &worker_progress,
            );
            let _ = sender.send(result.map(|(page, wrapped)| Completion::Search {
                page,
                wrapped,
                cyclic: options.wrap,
            }));
            repaint.request_repaint();
        }) {
            Ok(handle) => {
                self.task = Some(ReadTask {
                    receiver,
                    handle,
                    cancel_on_drop: true,
                    progress: Some(progress),
                });
                self.recent_searches
                    .record(&self.search_input, self.search_mode, self.wrap_search);
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
        if !egui::Popup::is_any_open(ctx) && ctx.input(close_shortcut) {
            ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                input.consume_key(egui::Modifiers::NONE, egui::Key::F10);
            });
            self.cancel();
            return (false, None);
        }
        if !self.started {
            self.request(0, ctx, workload);
        }
        let mut open = true;
        let mut copy = None;
        let mut navigate = None;
        let mut go_requested = false;
        let mut reload = false;
        let mut cancel = false;
        let mut search = None;
        let mut boundary_search = None;
        let mut history_navigation = None;
        let busy = self.task.is_some();
        let open_characters = if !egui::Popup::is_any_open(ctx) && ctx.input(character_shortcut) {
            let menu = ctx.input(|input| input.modifiers.shift);
            ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                input.consume_key(modifiers, egui::Key::F8);
            });
            if !menu {
                self.select_character_encoding(self.character_encoding.next());
            }
            menu
        } else {
            false
        };

        if !busy
            && !egui::Popup::is_any_open(ctx)
            && !ctx.egui_wants_keyboard_input()
            && let Some(direction) = ctx.input(repeat_search_shortcut)
        {
            ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                input.consume_key(modifiers, egui::Key::F7);
                input.consume_key(modifiers, egui::Key::Space);
            });
            search = Some(direction);
        }
        if !busy
            && !egui::Popup::is_any_open(ctx)
            && !ctx.egui_wants_keyboard_input()
            && !ctx
                .plugin::<egui::text_selection::LabelSelectionState>()
                .lock()
                .has_selection()
            && ctx.input(copy_match_shortcut)
            && let Some(bytes) = self.matched_bytes()
        {
            copy = Some(byte_text(bytes, ByteFormat::Hex));
            ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                input.consume_key(modifiers, egui::Key::C);
                input.consume_key(modifiers, egui::Key::Insert);
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Copy));
            });
        }
        if !busy
            && !egui::Popup::is_any_open(ctx)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(clear_match_shortcut)
        {
            ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::CTRL, egui::Key::U);
            });
            self.clear_match();
        }
        if !busy
            && !egui::Popup::is_any_open(ctx)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(page_navigation_key)
            && let Some(page) = &self.page
        {
            navigate = ctx
                .input_mut(|input| {
                    let modifiers = input.modifiers;
                    if modifiers.alt || modifiers.shift || modifiers.mac_cmd {
                        return None;
                    }
                    if input.consume_key(modifiers, egui::Key::Home) {
                        Some(0)
                    } else if input.consume_key(modifiers, egui::Key::End) {
                        Some(page.last_offset())
                    } else if modifiers.ctrl && input.consume_key(modifiers, egui::Key::ArrowLeft) {
                        page.offset.checked_sub(1)
                    } else if modifiers.ctrl && input.consume_key(modifiers, egui::Key::ArrowRight)
                    {
                        page.offset
                            .checked_add(1)
                            .filter(|offset| *offset <= page.stamp.identity.size)
                    } else if !modifiers.ctrl && input.consume_key(modifiers, egui::Key::PageUp) {
                        Some(page.previous_offset())
                    } else if !modifiers.ctrl && input.consume_key(modifiers, egui::Key::PageDown) {
                        page.next_offset()
                    } else {
                        None
                    }
                })
                .filter(|offset| *offset != page.offset);
        }
        if !busy && !egui::Popup::is_any_open(ctx) && !ctx.egui_wants_keyboard_input() {
            history_navigation = ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                if modifiers.mac_cmd {
                    return None;
                }
                if modifiers.ctrl && !modifiers.alt && input.consume_key(modifiers, egui::Key::Z) {
                    Some(!modifiers.shift)
                } else if modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.shift
                    && input.consume_key(modifiers, egui::Key::Backspace)
                {
                    Some(true)
                } else {
                    None
                }
            });
        }
        if !busy && !egui::Popup::is_any_open(ctx) && !ctx.egui_wants_keyboard_input() {
            let adjustment = ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                if !modifiers.alt || modifiers.shift || modifiers.mac_cmd {
                    return None;
                }
                if input.consume_key(modifiers, egui::Key::ArrowLeft) {
                    Some((false, modifiers.ctrl))
                } else if input.consume_key(modifiers, egui::Key::ArrowRight) {
                    Some((true, modifiers.ctrl))
                } else {
                    None
                }
            });
            if let Some((increase, block)) = adjustment {
                let width = self.row_width.0;
                self.row_width = RowWidth::new(match (increase, block) {
                    (true, false) => width.saturating_add(1),
                    (false, false) => width.saturating_sub(1),
                    (true, true) => (width / 16 + 1) * 16,
                    (false, true) => (width.saturating_sub(1) / 16) * 16,
                });
            }
        }
        if !busy && !egui::Popup::is_any_open(ctx) {
            let bookmark_key = ctx.input_mut(|input| {
                let modifiers = input.modifiers;
                if !modifiers.ctrl || modifiers.alt || modifiers.mac_cmd {
                    return None;
                }
                [
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                    egui::Key::Num6,
                    egui::Key::Num7,
                    egui::Key::Num8,
                    egui::Key::Num9,
                    egui::Key::Num0,
                ]
                .into_iter()
                .enumerate()
                .find_map(|(slot, key)| {
                    input
                        .consume_key(modifiers, key)
                        .then_some((slot, modifiers.shift))
                })
            });
            if let Some((slot, set)) = bookmark_key {
                if set {
                    if self.error.is_none() && self.page.is_some() {
                        self.bookmark_slot = slot;
                        self.set_bookmark();
                    }
                } else if let Some(offset) = self.bookmarks[slot] {
                    self.bookmark_slot = slot;
                    navigate = Some(offset);
                }
            }
        }
        let (focus_offset, mut focus_search) =
            if !busy && !egui::Popup::is_any_open(ctx) && ctx.input(focus_shortcut) {
                ctx.input_mut(|input| {
                    if !input.modifiers.is_none() {
                        return (false, false);
                    }
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::F5),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::F7),
                    )
                })
            } else {
                (false, false)
            };
        if self.restore_search_focus && !busy {
            self.restore_search_focus = false;
            let focus = ctx.memory(|memory| memory.focused());
            if !egui::Popup::is_any_open(ctx)
                && !focus_offset
                && focus.is_none_or(|id| id == egui::Id::new("hex-search-query"))
                && !ctx.input(|input| input.pointer.any_pressed())
            {
                focus_search = true;
            }
        }
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
                        let path_label = ui.add(egui::Label::new(&path).truncate()).on_hover_text(&path);
                        #[cfg(any(test, feature = "visual-qa"))]
                        ctx.data_mut(|data| data.insert_temp(egui::Id::new("hex-path-label"), path_label.rect));
                        let _ = path_label;
                        ui.small(
                            "Read-only · 4096 bytes per page · F5 Go to offset · F7 Search · Esc/F10 Close",
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
                            ui.small("Offsets stay hexadecimal");
                            let characters = ui.add(egui::Button::new(format!("Characters: {}{}", if self.character_encoding_explicit { "" } else { "Auto " }, self.character_encoding.label())).wrap_mode(egui::TextWrapMode::Extend))
                                .on_hover_text("F8 cycles character encodings; Shift+F8 opens this menu. Character column and Copy page only. UTF-8/UTF-16 show characters at leading bytes and › at continuation bytes across rows/pages. UTF-16 code units are aligned to even absolute file offsets; byte order is explicit. Search and raw exports remain independent. Invalid/control/invisible characters use dots. Switching does not read the file.");
                            if open_characters { egui::Popup::open_id(ctx, egui::Id::new("hex-character-popup")); }
                            egui::Popup::menu(&characters).id(egui::Id::new("hex-character-popup")).show(|ui| {
                                let auto = ui.add_enabled(self.bom_known, egui::Button::new("Auto (Unicode BOM)").selected(!self.character_encoding_explicit))
                                    .on_hover_text("Use UTF-8/UTF-16 LE/BE only when identified by a BOM at byte zero of the accepted snapshot; otherwise ASCII. No heuristic or new read. UTF-32 is not supported.");
                                #[cfg(any(test, feature = "visual-qa"))]
                                ctx.data_mut(|data| data.insert_temp(egui::Id::new(("hex-character-encoding", "Auto (Unicode BOM)")), auto.rect));
                                if auto.clicked() { self.use_bom_encoding(); ui.close(); }
                                for (encoding, label) in CharacterEncoding::ALL {
                                    let response = ui.selectable_label(self.character_encoding_explicit && self.character_encoding == encoding, label);
                                    #[cfg(any(test, feature = "visual-qa"))]
                                    ctx.data_mut(|data| data.insert_temp(egui::Id::new(("hex-character-encoding", label)), response.rect));
                                    if response.clicked() { self.select_character_encoding(encoding); ui.close(); }
                                }
                            });
                            #[cfg(any(test, feature = "visual-qa"))]
                            ctx.data_mut(|data| data.insert_temp(egui::Id::new("hex-character-menu"), characters.rect));

                            ui.label("Bytes per row");
                            for bytes in [8, 16, 32] {
                                let response = ui.selectable_label(self.row_width.0 == bytes, bytes.to_string());
                                if response.clicked() { self.row_width = RowWidth::new(bytes); }
                                #[cfg(any(test, feature = "visual-qa"))]
                                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("hex-row-width-preset", bytes)), response.rect));
                            }
                            let mut width = self.row_width.0;
                            let row_width = ui.add(egui::DragValue::new(&mut width).range(1..=64));
                            row_width.clone().on_hover_text("Ctrl+Left/Right shifts the page start by one byte without changing this width. 1–64 bytes per displayed/copied row. Alt+Left/Right changes by one; Ctrl+Alt+Left/Right moves to the next lower/higher multiple of 16 (bounded to 1–64). Shortcuts require no focused widget or open menu and no active work. Page reads and byte-copy formats stay unchanged.");
                            row_width.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, row_width.enabled(), "Hex bytes per row"));
                            if row_width.changed() { self.row_width = RowWidth::new(width); }
                            #[cfg(any(test, feature = "visual-qa"))]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("hex-row-width"), row_width.rect));
                        });
                        ui.horizontal_wrapped(|ui| {
                            for (back, label) in [(true, "Back"), (false, "Forward")] {
                                if ui.add_enabled(!busy && self.history.target(back).is_some(), egui::Button::new(label))
                                    .on_hover_text("Revisit a successfully loaded byte position. Ctrl+Z / Alt+Backspace: Back; Ctrl+Shift+Z: Forward. Shortcuts require no focused widget, open menu or active work.")
                                    .clicked() { history_navigation = Some(back); }
                            }
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
                            ).on_hover_text("PageUp loads the previous 4 KiB block when no widget has keyboard focus.");
                            if previous.clicked() {
                                navigate = self
                                    .page
                                    .as_ref()
                                    .map(Page::previous_offset);
                            }
                            let next = ui.add_enabled(
                                !busy && self.page.as_ref().and_then(Page::next_offset).is_some(),
                                egui::Button::new("Next"),
                            ).on_hover_text("PageDown loads the next 4 KiB block when no widget has keyboard focus.");
                            if next.clicked() {
                                navigate = self.page.as_ref().and_then(Page::next_offset);
                            }
                            let last = ui.add_enabled(
                                !busy && self.page.is_some(),
                                egui::Button::new("Last"),
                            ).on_hover_text("End / Ctrl+End loads the last 4 KiB block; Home / Ctrl+Home loads the first. Requires no focused widget or open menu and no active work.");
                            if last.clicked() {
                                navigate = self.page.as_ref().map(Page::last_offset);
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
                                    .id(egui::Id::new("hex-offset-input"))
                                    .desired_width(180.0)
                                    .char_limit(32)
                                    .hint_text("1234, 0x4D2, 50%, +64 or -0x10"),
                            ).on_hover_text("Decimal: 255 or 255m. Hexadecimal: 0xFF, $FF or FFh. Signed values are relative to the loaded page's first byte; signed percentages step by that percentage of full file size.");
                            if focus_offset {
                                offset.request_focus();
                                offset.scroll_to_me(Some(egui::Align::Center));
                            }
                            offset.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::TextEdit,
                                    offset.enabled(),
                                    "Byte offset",
                                )
                            });
                            let go = ui.add_enabled(!busy, egui::Button::new("Go"));
                            #[cfg(any(test, feature = "visual-qa"))]
                            ctx.data_mut(|data| {
                                data.insert_temp(egui::Id::new("hex-offset"), offset.rect);
                                data.insert_temp(egui::Id::new("hex-go"), go.rect);
                            });
                            if go.clicked()
                                || (!busy
                                    && offset.lost_focus()
                                    && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                            {
                                go_requested = true;
                            }
                            let mut recall = None;
                            let mut clear = false;
                            let mut remove_offset = None;
                            let menu = ui.add_enabled(!busy && !self.recent_offsets.is_empty(), egui::Button::new("Recent offsets").wrap_mode(egui::TextWrapMode::Extend));
                            egui::Popup::menu(&menu).show(|ui| {
                                ui.set_max_width(420.0);
                                for (index, expression) in self.recent_offsets.iter().enumerate() {
                                    let label = expression.escape_debug().to_string();
                                    let (response, remove) = recent_entry_ui(ui, !busy, &label,
                                        &format!("{label}\nRecall without reading. Go evaluates this expression again; relative offsets use the current page."));
                                    #[cfg(any(test, feature = "visual-qa"))]
                                    ui.ctx().data_mut(|data| {
                                        data.insert_temp(egui::Id::new(("hex-recent-offset", index)), response.rect);
                                        data.insert_temp(egui::Id::new(("hex-remove-recent-offset", index)), remove.rect);
                                    });
                                    #[cfg(not(any(test, feature = "visual-qa")))]
                                    let _ = index;
                                    if response.clicked() { recall = Some(expression.clone()); ui.close(); }
                                    if remove.clicked() { remove_offset = Some(expression.clone()); ui.close(); }
                                }
                                ui.separator();
                                if ui.add_enabled(!busy, egui::Button::new("Clear recent offsets")).clicked() { clear = true; ui.close(); }
                            });
                            #[cfg(any(test, feature = "visual-qa"))]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("hex-recent-offsets-menu"), menu.rect));
                            if clear && !busy { self.recent_offsets.clear(); }
                            if let Some(expression) = remove_offset.filter(|_| !busy) {
                                self.recent_offsets.retain(|entry| entry != &expression);
                                recall = None;
                            }
                            if let Some(expression) = recall.filter(|_| !busy) {
                                self.offset_input = expression;
                                self.error = None;
                                offset.request_focus();
                                go_requested = false;
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
                                    "Copy this page's offsets, displayed byte values and selected character column",
                                );
                            #[cfg(test)]
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(egui::Id::new("hex-copy-page"), copy_button.rect)
                            });
                            if copy_button.clicked() {
                                copy = self
                                    .page
                                    .as_ref()
                                    .map(|page| page.text_in_encoding(self.byte_format, self.row_width, self.character_encoding));
                            }
                            ui.add_enabled_ui(!busy && self.page.as_ref().is_some_and(|page| !page.bytes.is_empty()), |ui| {
                                let menu = ui.menu_button("Copy bytes", |ui| {
                                    if let Some(page) = &self.page {
                                        for (format, label) in [(ByteFormat::Hex, "HEX bytes"), (ByteFormat::Decimal, "Decimal bytes")] {
                                            let response = ui.button(label).on_hover_text("Copy loaded page bytes separated by spaces, without offsets or character columns. No new file read.");
                                            #[cfg(test)]
                                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("hex-copy-bytes", label)), response.rect));
                                            if response.clicked() {
                                                copy = Some(page.byte_text(format));
                                                ui.close();
                                            }
                                        }
                                        let utf8 = std::str::from_utf8(&page.bytes);
                                        if ui.add_enabled(utf8.is_ok(), egui::Button::new("UTF-8 text"))
                                            .on_hover_text("Copy exact UTF-8 page text, preserving whitespace and controls. Disabled if this page is not valid UTF-8; no replacement characters.")
                                            .clicked()
                                        {
                                            copy = utf8.ok().map(str::to_owned);
                                            ui.close();
                                        }
                                    }
                                });
                                #[cfg(test)]
                                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("hex-copy-bytes-menu"), menu.response.rect));
                                let _ = menu;
                            });
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Bookmark");
                            egui::ComboBox::from_id_salt("hex-bookmark-slot")
                                .selected_text(format!("{}", self.bookmark_slot + 1))
                                .show_ui(ui, |ui| {
                                    for slot in 0..self.bookmarks.len() {
                                        let label = match self.bookmarks[slot] {
                                            Some(offset) => format!("{} · 0x{offset:X}", slot + 1),
                                            None => format!("{} · empty", slot + 1),
                                        };
                                        ui.selectable_value(&mut self.bookmark_slot, slot, label);
                                    }
                                });
                            let set = ui.add_enabled(
                                !busy && self.error.is_none() && self.page.is_some(),
                                egui::Button::new("Set here"),
                            ).on_hover_text("Save the current page start in this slot for this open viewer. Ctrl+Shift+1–9 sets slots 1–9; Ctrl+Shift+0 sets slot 10.");
                            if set.clicked() {
                                self.set_bookmark();
                            }
                            let saved = self.bookmarks[self.bookmark_slot];
                            let jump = ui.add_enabled(!busy && saved.is_some(), egui::Button::new("Jump"))
                                .on_hover_text("Ctrl+1–9 jumps to slots 1–9; Ctrl+0 jumps to slot 10. Busy work and open menus suppress bookmark shortcuts.");
                            if jump.clicked() {
                                navigate = saved;
                            }
                            let clear = ui.add_enabled(!busy && saved.is_some(), egui::Button::new("Clear"));
                            if clear.clicked() {
                                self.bookmarks[self.bookmark_slot] = None;
                            }
                            if let Some(offset) = saved {
                                ui.monospace(format!("0x{offset:X}"));
                            }
                            ui.small("Session only · Reload clears all");
                            #[cfg(feature = "visual-qa")]
                            ctx.data_mut(|data| {
                                data.insert_temp(egui::Id::new("hex-bookmark-set"), set.rect);
                                data.insert_temp(egui::Id::new("hex-bookmark-jump"), jump.rect);
                                data.insert_temp(egui::Id::new("hex-bookmark-clear"), clear.rect);
                            });
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Search");
                            for (mode, label) in [
                                (crate::hex_search::InputMode::Hex, "Hex bytes"),
                                (crate::hex_search::InputMode::Utf8, "UTF-8 text"),
                                (crate::hex_search::InputMode::Utf16Le, "UTF-16 LE"),
                                (crate::hex_search::InputMode::Utf16Be, "UTF-16 BE"),
                                (crate::hex_search::InputMode::Windows1251, "Windows-1251"),
                                (crate::hex_search::InputMode::Windows1252, "Windows-1252"),
                                (crate::hex_search::InputMode::Mixed, "Mixed text/hex"),
                            ] {
                                let response = ui.add_enabled(!busy, egui::Button::selectable(self.search_mode == mode, label));
                                if response.clicked() { self.set_search_mode(mode); }
                                #[cfg(feature = "visual-qa")]
                                ctx.data_mut(|data| data.insert_temp(egui::Id::new(("hex-search-mode", label)), response.rect));
                            }
                            let legacy_label = match self.search_mode {
                                crate::hex_search::InputMode::Koi8R => "KOI8-R",
                                crate::hex_search::InputMode::Koi8U => "KOI8-U",
                                crate::hex_search::InputMode::Ibm866 => "IBM866",
                                _ => "More encodings",
                            };
                            let menu = ui.add_enabled(!busy, egui::Button::new(legacy_label).wrap_mode(egui::TextWrapMode::Extend));
                            egui::Popup::menu(&menu).show(|ui| {
                                    for (mode, label) in [(crate::hex_search::InputMode::Koi8R, "KOI8-R"), (crate::hex_search::InputMode::Koi8U, "KOI8-U"), (crate::hex_search::InputMode::Ibm866, "IBM866")] {
                                        let response = ui.selectable_label(self.search_mode == mode, label);
                                        #[cfg(test)]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("hex-legacy-mode", label)), response.rect));
                                        if response.clicked() {
                                            self.set_search_mode(mode);
                                            ui.close();
                                        }
                                    }
                                });
                            #[cfg(test)]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("hex-legacy-menu"), menu.rect));
                        });
                        ui.horizontal_wrapped(|ui| {
                            let query_id = egui::Id::new("hex-search-query");
                            let keyboard_search = if !busy && self.page.is_some() && ui.memory(|memory| memory.has_focus(query_id)) {
                                ui.input_mut(|input| {
                                    if input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter)
                                        || input.consume_key(egui::Modifiers::SHIFT, egui::Key::F3) {
                                        Some(crate::hex_search::Direction::Backward)
                                    } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                                        || input.consume_key(egui::Modifiers::NONE, egui::Key::F3) {
                                        Some(crate::hex_search::Direction::Forward)
                                    } else { None }
                                })
                            } else { None };
                            let (hint, accessible_label) = match self.search_mode {
                                crate::hex_search::InputMode::Hex => ("DE AD BE EF", "Hexadecimal search bytes"),
                                crate::hex_search::InputMode::Utf8 => ("Exact UTF-8 text", "UTF-8 search text"),
                                crate::hex_search::InputMode::Utf16Le => ("Exact UTF-16 LE text", "UTF-16 little-endian search text"),
                                crate::hex_search::InputMode::Utf16Be => ("Exact UTF-16 BE text", "UTF-16 big-endian search text"),
                                crate::hex_search::InputMode::Windows1251 => ("Exact Windows-1251 text", "Windows-1251 search text"),
                                crate::hex_search::InputMode::Windows1252 => ("Exact Windows-1252 text", "Windows-1252 search text"),
                                crate::hex_search::InputMode::Mixed => ("\"Text\" 00 FF", "Mixed quoted UTF-8 and hexadecimal search bytes"),
                                crate::hex_search::InputMode::Koi8R => ("Exact KOI8-R text", "KOI8-R search text"),
                                crate::hex_search::InputMode::Koi8U => ("Exact KOI8-U text", "KOI8-U search text"),
                                crate::hex_search::InputMode::Ibm866 => ("Exact IBM866 text", "IBM866 search text"),
                            };
                            let query = ui.add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut self.search_input)
                                    .id(query_id)
                                    .char_limit(1024)
                                    .desired_width(260.0)
                                    .hint_text(hint),
                            );
                            query.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::TextEdit,
                                    query.enabled(),
                                    accessible_label,
                                )
                            });
                            if focus_search {
                                query.request_focus();
                                query.scroll_to_me(Some(egui::Align::Center));
                            }
                            if query.changed() {
                                self.last_match = None;
                                self.search_message = None;
                            }
                            let wrap = ui.add_enabled(!busy, egui::Checkbox::new(&mut self.wrap_search, "Wrap search"))
                                .on_hover_text("Continue from the other end after reaching a file boundary.");
                            if wrap.changed() { self.search_message = None; }
                            #[cfg(feature = "visual-qa")]
                            ctx.data_mut(|data| data.insert_temp(egui::Id::new("hex-wrap-search"), wrap.rect));
                            let next = ui.add_enabled(
                                !busy && self.page.is_some(),
                                egui::Button::new("Find next"),
                            ).on_hover_text("F7 focuses search; F3/Enter finds next in that field. Shift+F7 or Space repeats forward when no widget owns keyboard focus.");
                            let previous = ui.add_enabled(
                                !busy && self.page.is_some(),
                                egui::Button::new("Find previous"),
                            ).on_hover_text("Shift+F3/Shift+Enter searches backward in the search field; Alt+F7 repeats backward when no widget owns keyboard focus.");
                            if let Some(direction) = keyboard_search {
                                self.restore_search_focus = true;
                                search = Some(direction);
                                query.request_focus();
                            }
                            if next.clicked() {
                                search = Some(crate::hex_search::Direction::Forward);
                            }
                            if previous.clicked() {
                                search = Some(crate::hex_search::Direction::Backward);
                            }
                            for (label, direction) in [
                                ("Find first", crate::hex_search::Direction::Forward),
                                ("Find last", crate::hex_search::Direction::Backward),
                            ] {
                                let response = ui.add_enabled(!busy && self.page.is_some(), egui::Button::new(label))
                                    .on_hover_text("Search the whole file from its boundary, ignoring the current page and previous match.");
                                #[cfg(any(test, feature = "visual-qa"))]
                                ctx.data_mut(|data| data.insert_temp(egui::Id::new(("hex-boundary-search", label)), response.rect));
                                if response.clicked() { boundary_search = Some(direction); }
                            }
                            let mut clear_match = false;
                            let matched = self.matched_bytes();
                            ui.add_enabled_ui(matched.is_some(), |ui| {
                                let menu = ui.add(egui::Button::new("Copy match").wrap_mode(egui::TextWrapMode::Extend));
                                egui::Popup::menu(&menu).show(|ui| {
                                    if let Some(bytes) = matched {
                                        for (format, label) in [(ByteFormat::Hex, "HEX bytes"), (ByteFormat::Decimal, "Decimal bytes")] {
                                            let response = ui.button(label).on_hover_text("Copy only the found bytes from the loaded snapshot, without offsets or surrounding bytes. Ctrl+C / Cmd+C / Ctrl+Insert copies HEX when no field, menu or manual text selection owns copying.");
                                            #[cfg(any(test, feature = "visual-qa"))]
                                            ctx.data_mut(|data| data.insert_temp(egui::Id::new(("hex-copy-match", label)), response.rect));
                                            if response.clicked() { copy = Some(byte_text(bytes, format)); ui.close(); }
                                        }
                                        let text = std::str::from_utf8(bytes);
                                        let response = ui.add_enabled(text.is_ok(), egui::Button::new("UTF-8 text"))
                                            .on_hover_text("Exact UTF-8 found bytes, preserving whitespace and controls; disabled for invalid UTF-8. No new read.");
                                        #[cfg(any(test, feature = "visual-qa"))]
                                        ctx.data_mut(|data| data.insert_temp(egui::Id::new(("hex-copy-match", "UTF-8 text")), response.rect));
                                        if response.clicked() { copy = text.ok().map(str::to_owned); ui.close(); }
                                        let clear = ui.button("Clear match").on_hover_text("Remove the search highlight and disable Copy match. Ctrl+U when no widget owns keyboard focus. Keeps the query, page and next-search position.");
                                        #[cfg(any(test, feature = "visual-qa"))]
                                        ctx.data_mut(|data| data.insert_temp(egui::Id::new(("hex-copy-match", "Clear match")), clear.rect));
                                        if clear.clicked() { clear_match = true; ui.close(); }
                                    }
                                });
                                #[cfg(any(test, feature = "visual-qa"))]
                                ctx.data_mut(|data| data.insert_temp(egui::Id::new("hex-copy-match-menu"), menu.rect));
                                let _ = menu;
                            });
                            if clear_match { self.clear_match(); }
                            let mut recall = None;
                            let mut clear_recent = false;
                            let mut remove_recent = None;
                            let menu = ui.add_enabled(!busy && !self.recent_searches.0.is_empty(), egui::Button::new("Recent searches").wrap_mode(egui::TextWrapMode::Extend));
                            egui::Popup::menu(&menu).show(|ui| {
                                    ui.set_max_width(420.0);
                                    for (index, entry) in self.recent_searches.0.iter().enumerate() {
                                        let text = format!("{}{}: {}", entry.mode.label(), if entry.wrap { " · Wrap" } else { "" }, entry.query.escape_debug());
                                        let (response, remove) = recent_entry_ui(ui, !busy, &text, &text);
                                        #[cfg(any(test, feature = "visual-qa"))]
                                        ui.ctx().data_mut(|data| {
                                            data.insert_temp(egui::Id::new(("hex-recent-search", index)), response.rect);
                                            data.insert_temp(egui::Id::new(("hex-remove-recent-search", index)), remove.rect);
                                        });
                                        let _ = index;
                                        if response.clicked() { recall = Some(entry.clone()); ui.close(); }
                                        if remove.clicked() { remove_recent = Some(entry.clone()); ui.close(); }
                                    }
                                    ui.separator();
                                    if ui.add_enabled(!busy, egui::Button::new("Clear recent searches")).clicked() { clear_recent = true; ui.close(); }
                                });
                            #[cfg(any(test, feature = "visual-qa"))]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("hex-recent-searches-menu"), menu.rect));
                            if clear_recent && !busy { self.recent_searches = Default::default(); }
                            if let Some(recent) = remove_recent.filter(|_| !busy) {
                                self.recent_searches.0.retain(|entry| entry != &recent);
                                recall = None;
                            }
                            if let Some(recent) = recall.filter(|_| !busy) {
                                self.recall_search(recent);
                                query.request_focus();
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
                            let status = format!(
                                "Offset 0x{:X} ({}) · Position {} · {} bytes shown · {} bytes in file",
                                page.offset,
                                page.offset,
                                position_percent(page.offset, page.stamp.identity.size),
                                page.bytes.len(),
                                page.stamp.identity.size
                            );
                            ui.label(&status).on_hover_text("Position of the loaded page's first byte in this file snapshot, rounded down to 0.01%. n/a for an empty file. It is not read/search progress or a live size monitor.");
                            #[cfg(any(test, feature = "visual-qa"))]
                            ctx.data_mut(|data| data.insert_temp(egui::Id::new("hex-position-status"), status));
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
                                .id_salt(("hex-bytes", page.offset, self.generation, self.row_width.0))
                                .max_height(400.0)
                                .show_rows(
                                    ui,
                                    height,
                                    page.bytes.len().div_ceil(self.row_width.bytes()),
                                    |ui, range| {
                                        for row in range {
                                            ui.add(
                                                egui::Label::new({
                                                    let text = egui::RichText::new(
                                                        page.row_in_encoding(row, self.byte_format, self.row_width, self.character_encoding),
                                                    )
                                                    .monospace();
                                                    if self.match_selected && self.last_match.as_ref().is_some_and(
                                                        |(offset, pattern)| {
                                                            *offset == page.offset
                                                                && row * self.row_width.bytes() < pattern.len()
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
            self.cancel();
        }
        if reload {
            self.reload(ctx, workload);
        } else if let Some(back) = history_navigation {
            self.request_history(back, ctx, workload);
        } else if go_requested {
            self.go_to_input(ctx, workload);
        } else if let Some(offset) = navigate {
            self.request(offset, ctx, workload);
        } else if let Some(direction) = boundary_search {
            self.search_from(direction, true, ctx, workload);
        } else if let Some(direction) = search {
            self.search(direction, ctx, workload);
        }
        (open, copy)
    }
    #[cfg(feature = "visual-qa")]
    pub fn row_width_bytes(&self) -> usize {
        self.row_width.bytes()
    }
    #[cfg(feature = "visual-qa")]
    pub fn recalled_offset_is(&self, expression: &str) -> bool {
        self.offset_input == expression && self.recent_offsets.iter().any(|s| s == expression)
    }
    #[cfg(feature = "visual-qa")]
    pub fn submitted_offset_is(&self, expression: &str) -> bool {
        self.recent_offsets.front().is_some_and(|s| s == expression)
    }
    #[cfg(feature = "visual-qa")]
    pub fn wrap_search_enabled(&self) -> bool {
        self.wrap_search
    }
    #[cfg(feature = "visual-qa")]
    pub fn wrapped_search(&self) -> bool {
        self.wrap_search
            && self
                .search_message
                .as_ref()
                .is_some_and(|s| s.starts_with("Wrapped"))
    }
    #[cfg(feature = "visual-qa")]
    pub fn search_input_mode(&self) -> crate::hex_search::InputMode {
        self.search_mode
    }
    #[cfg(feature = "visual-qa")]
    pub fn bookmark_at(&self, slot: usize) -> Option<u64> {
        self.bookmarks.get(slot).copied().flatten()
    }
    #[cfg(feature = "visual-qa")]
    pub fn character_encoding_is_auto(&self) -> bool {
        !self.character_encoding_explicit
    }
    #[cfg(feature = "visual-qa")]
    pub fn character_encoding_label(&self) -> &'static str {
        self.character_encoding.label()
    }
    #[cfg(feature = "visual-qa")]
    pub fn first_displayed_row(&self) -> Option<String> {
        self.page.as_ref().map(|page| {
            page.row_in_encoding(0, self.byte_format, self.row_width, self.character_encoding)
        })
    }
    #[cfg(feature = "visual-qa")]
    pub fn match_is_selected(&self) -> bool {
        self.matched_bytes().is_some()
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
mod tests;
