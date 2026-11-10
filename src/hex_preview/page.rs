//! Validated page reads and in-page search over a bound file snapshot.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct Stamp {
    pub(super) identity: PathIdentity,
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
pub(super) struct Page {
    pub(super) stamp: Stamp,
    pub(super) offset: u64,
    pub(super) bytes: Vec<u8>,
    // Same validated snapshot: up to three bytes before/after the public page.
    pub(super) character_context: Vec<u8>,
    character_prefix: usize,
}

impl Page {
    pub(super) fn previous_offset(&self) -> u64 {
        self.offset.saturating_sub(PAGE_BYTES as u64)
    }
    pub(super) fn last_offset(&self) -> u64 {
        self.stamp.identity.size.saturating_sub(1) / PAGE_BYTES as u64 * PAGE_BYTES as u64
    }
    pub(super) fn next_offset(&self) -> Option<u64> {
        let next = self.offset.saturating_add(self.bytes.len() as u64);
        (next < self.stamp.identity.size).then_some(next)
    }
    #[cfg(test)]
    pub(super) fn row(&self, row: usize) -> String {
        self.row_in_format(row, ByteFormat::Hex)
    }
    #[cfg(test)]
    pub(super) fn row_in_format(&self, row: usize, format: ByteFormat) -> String {
        self.row_in_layout(row, format, RowWidth::default())
    }
    #[cfg(test)]
    fn row_in_layout(&self, row: usize, format: ByteFormat, width: RowWidth) -> String {
        self.row_in_encoding(row, format, width, CharacterEncoding::Ascii)
    }
    pub(super) fn row_in_encoding(
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
    pub(super) fn text(&self) -> String {
        self.text_in_format(ByteFormat::Hex)
    }
    #[cfg(test)]
    pub(super) fn text_in_format(&self, format: ByteFormat) -> String {
        self.text_in_layout(format, RowWidth::default())
    }
    #[cfg(test)]
    pub(super) fn text_in_layout(&self, format: ByteFormat, width: RowWidth) -> String {
        self.text_in_encoding(format, width, CharacterEncoding::Ascii)
    }
    pub(super) fn text_in_encoding(
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
    pub(super) fn byte_text(&self, format: ByteFormat) -> String {
        byte_text(&self.bytes, format)
    }
}

pub(super) fn byte_text(bytes: &[u8], format: ByteFormat) -> String {
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

pub(super) fn read_page(
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
pub(super) fn search_page(
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

pub(super) fn search_page_options(
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
