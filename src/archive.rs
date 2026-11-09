//! Bounded archive inspection and extraction shared by the archive browser and
//! search. ZIP and plain/gzip/bzip2/XZ/Zstandard TAR listing are in-process; selected tar extraction uses
//! system `tar`. Auto-inspect callers should consult `crate::trust` (J003 hooks).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const MAX_ARCHIVES_PER_SEARCH: usize = 1_024;
pub const MAX_MEMBERS_PER_RUN: usize = 100_000;
pub const MAX_MEMBER_TEXT_BYTES: u64 = 1024 * 1024;
pub const MAX_TOTAL_TEXT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_COMPRESSION_RATIO: u64 = 200;
const RATIO_GUARD_MIN_BYTES: u64 = 64 * 1024;

pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    TarGz,
    TarBz2,
    TarXz,
    TarZstd,
    Tar,
}

impl ArchiveKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Zip => "ZIP",
            Self::TarGz => "tar.gz",
            Self::TarBz2 => "tar.bz2",
            Self::TarXz => "tar.xz",
            Self::TarZstd => "tar.zst",
            Self::Tar => "TAR",
        }
    }
}

/// Central-directory metadata, available without opening or decoding payloads.
#[derive(Clone, Debug)]
pub struct ZipMemberInfo {
    pub compression: String,
    pub encrypted: bool,
    pub encryption: String,
    /// Declared CRC, not payload verification; AE-2 has no CRC.
    pub crc32: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct ArchiveMember {
    pub index: usize,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_regular: bool,
    pub unix_mode: Option<u32>,
    pub owner_uid: Option<u64>,
    pub owner_gid: Option<u64>,
    pub file_type_bits: u32,
    pub is_executable: Option<bool>,
    pub size: u64,
    pub compressed_size: u64,
    pub zip_info: Option<ZipMemberInfo>,
    /// The member name's original bytes. Tar extraction must pass this rather
    /// than a display conversion of `path`. `None` for zip members.
    pub tar_name: Option<std::ffi::OsString>,
    /// The member's own modification time when the archive records one
    /// (ZIP or tar); `None` when no usable time was recorded, where callers
    /// fall back to the archive file's mtime.
    pub modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug)]
pub struct ArchiveListing {
    pub tree: Result<Arc<crate::archive_tree::Tree>, String>,
    pub kind: ArchiveKind,
    pub members: Vec<ArchiveMember>,
    pub declared_members: usize,
    pub declared_uncompressed_bytes: u128,
    pub unsafe_members: usize,
    pub unreadable_members: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub struct SearchMember {
    pub member: ArchiveMember,
    pub content: Option<Arc<str>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchBudget {
    pub archives: usize,
    pub members: usize,
    pub content_bytes: u64,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VisitSummary {
    pub declared_members: usize,
    pub declared_uncompressed_bytes: u128,
    pub emitted: usize,
    pub unsafe_members: usize,
    pub unreadable_members: usize,
    pub content_skipped: usize,
    /// A provider could not search the archive members for this format.
    pub members_not_searched: bool,
    pub cancelled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractReport {
    pub extracted: usize,
    pub skipped_dirs: usize,
    pub skipped_existing: usize,
    pub errors: Vec<String>,
}

pub fn kind_of(path: &Path) -> Option<ArchiveKind> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".tar.bz2") || name.ends_with(".tbz2") || name.ends_with(".tbz") {
        Some(ArchiveKind::TarBz2)
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        Some(ArchiveKind::TarXz)
    } else if name.ends_with(".tar.zst") || name.ends_with(".tar.zstd") || name.ends_with(".tzst") {
        Some(ArchiveKind::TarZstd)
    } else if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else if path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("zip") || extension.eq_ignore_ascii_case("zipx")
    }) {
        Some(ArchiveKind::Zip)
    } else {
        None
    }
}

pub fn is_supported(path: &Path) -> bool {
    kind_of(path).is_some()
}

pub fn virtual_member_path(archive: &Path, member: &Path) -> PathBuf {
    let mut archive_marker = archive.as_os_str().to_owned();
    archive_marker.push("!");
    PathBuf::from(archive_marker).join(member)
}

pub fn visit_members(
    archive_path: &Path,
    needs_content: bool,
    budget: &mut SearchBudget,
    cancelled: impl Fn() -> bool,
    emit: impl FnMut(SearchMember) -> bool,
) -> Result<VisitSummary, String> {
    visit_members_with_password(archive_path, needs_content, budget, None, cancelled, emit)
}

pub fn visit_members_with_password(
    archive_path: &Path,
    needs_content: bool,
    budget: &mut SearchBudget,
    password: Option<&[u8]>,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(SearchMember) -> bool,
) -> Result<VisitSummary, String> {
    if cancelled() {
        return Ok(VisitSummary {
            cancelled: true,
            ..Default::default()
        });
    }
    if budget.archives >= MAX_ARCHIVES_PER_SEARCH || budget.members >= MAX_MEMBERS_PER_RUN {
        budget.truncated = true;
        return Ok(VisitSummary::default());
    }
    match kind_of(archive_path) {
        Some(
            kind @ (ArchiveKind::TarGz
            | ArchiveKind::TarBz2
            | ArchiveKind::TarXz
            | ArchiveKind::TarZstd
            | ArchiveKind::Tar),
        ) => {
            return visit_tar(
                archive_path,
                kind,
                needs_content,
                MAX_TAR_STREAM_BYTES,
                budget,
                cancelled,
                emit,
            );
        }
        Some(ArchiveKind::Zip) => {}
        None => {
            return Err(format!(
                "Unsupported archive format: {}",
                archive_path.display()
            ));
        }
    }
    budget.archives += 1;

    let file = fs::File::open(archive_path)
        .map_err(|error| format!("Could not open archive {}: {error}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(SearchZipInput {
        file,
        cancelled: &cancelled,
    })
    .map_err(|error| format!("Could not read archive {}: {error}", archive_path.display()))?;
    if archive.has_overlapping_files().map_err(|error| {
        format!(
            "Could not validate archive {}: {error}",
            archive_path.display()
        )
    })? {
        return Err(format!(
            "Archive {} contains overlapping file data",
            archive_path.display()
        ));
    }

    let mut summary = VisitSummary {
        declared_members: archive.len(),
        declared_uncompressed_bytes: archive.decompressed_size().unwrap_or_default(),
        ..Default::default()
    };
    for index in 0..archive.len() {
        if cancelled() {
            summary.cancelled = true;
            break;
        }
        if budget.members >= MAX_MEMBERS_PER_RUN {
            budget.truncated = true;
            break;
        }
        budget.members += 1;

        let member = {
            let raw = match archive.by_index_raw(index) {
                Ok(raw) => raw,
                Err(_) => {
                    summary.unreadable_members += 1;
                    continue;
                }
            };
            let Some(path) = raw.enclosed_name() else {
                summary.unsafe_members += 1;
                continue;
            };
            if path.as_os_str().is_empty() {
                summary.unsafe_members += 1;
                continue;
            }
            ArchiveMember {
                index,
                path,
                is_dir: raw.is_dir(),
                is_symlink: raw.is_symlink(),
                unix_mode: raw.unix_mode().map(|mode| mode & 0o7777),
                owner_uid: None,
                owner_gid: None,
                file_type_bits: raw.unix_mode().unwrap_or(0) & 0o170000,
                is_regular: !raw.is_dir()
                    && !raw.is_symlink()
                    && raw
                        .unix_mode()
                        .is_none_or(|mode| matches!(mode & 0o170000, 0 | 0o100000)),
                is_executable: if raw.is_dir() || raw.is_symlink() {
                    Some(false)
                } else {
                    raw.unix_mode().map(|mode| {
                        (mode & 0o170000 == 0 || mode & 0o170000 == 0o100000) && mode & 0o111 != 0
                    })
                },
                size: raw.size(),
                compressed_size: raw.compressed_size(),
                zip_info: Some(ZipMemberInfo {
                    compression: if raw.compression() == zip::CompressionMethod::LZMA {
                        "Lzma".into()
                    } else if raw.compression() == zip::CompressionMethod::XZ {
                        "Xz".into()
                    } else if raw.compression() == zip::CompressionMethod::PPMD {
                        "Ppmd".into()
                    } else if raw.compression() == zip::CompressionMethod::ZSTD {
                        "Zstd".into()
                    } else if raw.compression() == zip::CompressionMethod::ZSTD_DEPRECATED {
                        "Zstd (legacy method 20)".into()
                    } else {
                        format!("{:?}", raw.compression())
                    },
                    encrypted: raw.encrypted(),
                    encryption: crate::zip_reader::encryption_label(&raw),
                    crc32: crate::zip_reader::declared_crc32(&raw),
                }),
                tar_name: None,
                modified: zip_member_modified(&raw),
            }
        };

        let content = if needs_content && !member.is_dir {
            match read_member_text(&mut archive, &member, budget, password) {
                Ok(Some(content)) => Some(content),
                Ok(None) | Err(_) => {
                    summary.content_skipped += 1;
                    None
                }
            }
        } else {
            None
        };
        if cancelled() {
            summary.cancelled = true;
            break;
        }
        if !emit(SearchMember { member, content }) {
            break;
        }
        summary.emitted += 1;
    }
    Ok(summary)
}

/// Includes skipped payloads: sequential TAR scans must read past them to
/// reach later headers. A large/bomb stream produces an explicit partial listing.
const MAX_TAR_STREAM_BYTES: u64 = 128 * 1024 * 1024;

struct TarStream<'a, R> {
    inner: R,
    remaining: &'a std::cell::Cell<u64>,
    limited: &'a std::cell::Cell<bool>,
    cancelled: &'a dyn Fn() -> bool,
}

impl<R: Read> Read for TarStream<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if (self.cancelled)() {
            return Err(std::io::Error::other("Archive scan cancelled"));
        }
        if self.remaining.get() == 0 {
            self.limited.set(true);
            return Err(std::io::Error::other("Archive stream limit reached"));
        }
        let length = buffer
            .len()
            .min(64 * 1024)
            .min(self.remaining.get() as usize);
        let count = self.inner.read(&mut buffer[..length])?;
        self.remaining.set(self.remaining.get() - count as u64);
        Ok(count)
    }
}

struct CancellableInput<'a> {
    file: fs::File,
    cancelled: &'a dyn Fn() -> bool,
}
impl Read for CancellableInput<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if (self.cancelled)() {
            return Err(std::io::Error::other("Archive scan cancelled"));
        }
        let length = buffer.len().min(64 * 1024);
        self.file.read(&mut buffer[..length])
    }
}

/// Decoder-owned limits complement the decoded-stream budget below. XZ's
/// limit covers liblzma's decoder allocation; zstd bounds back-reference distance.
pub fn tar_reader<'a>(
    file: fs::File,
    kind: ArchiveKind,
    cancelled: &'a dyn Fn() -> bool,
) -> Result<Box<dyn Read + 'a>, String> {
    let file = CancellableInput { file, cancelled };
    match kind {
        ArchiveKind::Tar => Ok(Box::new(file)),
        ArchiveKind::TarGz => Ok(Box::new(flate2::read::MultiGzDecoder::new(file))),
        ArchiveKind::TarBz2 => Ok(Box::new(bzip2::read::MultiBzDecoder::new(file))),
        ArchiveKind::TarXz => {
            let stream = xz2::stream::Stream::new_stream_decoder(
                64 * 1024 * 1024,
                xz2::stream::CONCATENATED,
            )
            .map_err(|e| e.to_string())?;
            Ok(Box::new(xz2::read::XzDecoder::new_stream(file, stream)))
        }
        ArchiveKind::TarZstd => {
            let mut decoder = zstd::stream::read::Decoder::new(file).map_err(|e| e.to_string())?;
            decoder.window_log_max(26).map_err(|e| e.to_string())?;
            Ok(Box::new(decoder))
        }
        ArchiveKind::Zip => Err("Expected a TAR archive".into()),
    }
}

/// Integrity checks and listings share complete concatenated-stream decoding.
pub fn tar_check_reader<'a>(
    file: fs::File,
    kind: ArchiveKind,
    cancelled: &'a dyn Fn() -> bool,
) -> Result<Box<dyn Read + 'a>, String> {
    tar_reader(file, kind, cancelled)
}

fn visit_tar(
    archive_path: &Path,
    kind: ArchiveKind,
    needs_content: bool,
    stream_limit: u64,
    budget: &mut SearchBudget,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(SearchMember) -> bool,
) -> Result<VisitSummary, String> {
    budget.archives += 1;
    let file = fs::File::open(archive_path)
        .map_err(|error| format!("Could not open archive {}: {error}", archive_path.display()))?;
    let reader = tar_reader(file, kind, &cancelled)?;
    let remaining = std::cell::Cell::new(stream_limit);
    let limited = std::cell::Cell::new(false);
    let reader = TarStream {
        inner: reader,
        remaining: &remaining,
        limited: &limited,
        cancelled: &cancelled,
    };
    let mut archive = tar::Archive::new(reader);
    archive.set_ignore_zeros(true);
    let entries = archive
        .entries()
        .map_err(|error| format!("Could not list {}: {error}", archive_path.display()))?;
    let mut summary = VisitSummary::default();
    for (index, entry) in entries.enumerate() {
        if cancelled() {
            summary.cancelled = true;
            break;
        }
        if budget.members >= MAX_MEMBERS_PER_RUN {
            budget.truncated = true;
            break;
        }
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                if limited.get() {
                    budget.truncated = true;
                    break;
                }
                if cancelled() {
                    summary.cancelled = true;
                    break;
                }
                return Err(format!(
                    "Could not read TAR archive {}: {error}",
                    archive_path.display()
                ));
            }
        };
        budget.members += 1;
        summary.declared_members += 1;
        let size = entry.header().size().unwrap_or(0);
        summary.declared_uncompressed_bytes += u128::from(size);
        let raw_name = entry.path_bytes();
        let raw_name = raw_name.strip_prefix(b"./").unwrap_or(&raw_name);
        if raw_name.is_empty() || raw_name == b"." {
            continue;
        }
        let Some(path) = tar_path_from_bytes(raw_name) else {
            summary.unreadable_members += 1;
            continue;
        };
        if !is_enclosed_tar_path(&path) {
            summary.unsafe_members += 1;
            continue;
        }
        let tar_name = Some(tar_name_from_bytes(raw_name));
        let is_dir = entry.header().entry_type().is_dir();
        let is_regular = entry.header().entry_type().is_file();
        let modified = entry.header().mtime().ok().and_then(|seconds| {
            std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds))
        });
        let member = ArchiveMember {
            index,
            path,
            is_dir,
            is_symlink: entry.header().entry_type().is_symlink(),
            is_regular,
            unix_mode: entry.header().mode().ok().map(|mode| mode & 0o7777),
            owner_uid: entry.header().uid().ok(),
            owner_gid: entry.header().gid().ok(),
            file_type_bits: match entry.header().entry_type().as_byte() {
                b'3' => 0o020000,
                b'4' => 0o060000,
                b'6' => 0o010000,
                _ => 0,
            },
            is_executable: if is_regular {
                entry.header().mode().ok().map(|mode| mode & 0o111 != 0)
            } else {
                Some(false)
            },
            size: if is_dir { 0 } else { size },
            compressed_size: 0,
            zip_info: None,
            tar_name,
            modified,
        };
        let content = if needs_content && !is_dir {
            if !is_regular
                || size > MAX_MEMBER_TEXT_BYTES
                || budget.content_bytes.saturating_add(size) > MAX_TOTAL_TEXT_BYTES
            {
                summary.content_skipped += 1;
                None
            } else {
                let mut bytes = Vec::with_capacity(size as usize);
                let result = (&mut entry)
                    .take(MAX_MEMBER_TEXT_BYTES + 1)
                    .read_to_end(&mut bytes);
                budget.content_bytes = budget.content_bytes.saturating_add(bytes.len() as u64);
                if cancelled() {
                    summary.cancelled = true;
                    break;
                }
                if limited.get() {
                    budget.truncated = true;
                    break;
                }
                if result.is_err()
                    || bytes.len() as u64 != size
                    || crate::search_text::binary_head(&bytes)
                {
                    summary.content_skipped += 1;
                    None
                } else {
                    Some(Arc::from(crate::search_text::decode(&bytes).into_owned()))
                }
            }
        } else {
            None
        };
        if !emit(SearchMember { member, content }) {
            break;
        }
        summary.emitted += 1;
    }
    budget.truncated |= limited.get();
    Ok(summary)
}

#[cfg(unix)]
fn tar_name_from_bytes(bytes: &[u8]) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(bytes.to_vec())
}

#[cfg(not(unix))]
fn tar_name_from_bytes(bytes: &[u8]) -> std::ffi::OsString {
    String::from_utf8_lossy(bytes).into_owned().into()
}

#[cfg(unix)]
pub fn tar_path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    Some(PathBuf::from(tar_name_from_bytes(bytes)))
}

#[cfg(not(unix))]
pub fn tar_path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    String::from_utf8(bytes.to_vec()).ok().map(PathBuf::from)
}

/// Raw `tar -tv` output lines, read until EOF, cancellation or `max_lines`.
#[cfg(test)]
#[derive(Debug, Default)]
struct ListingLines {
    lines: Vec<String>,
    cancelled: bool,
    /// `max_lines` was reached before EOF; the rest of the output was not read.
    limited: bool,
}

#[cfg(test)]
fn collect_listing_lines(
    mut reader: impl std::io::BufRead,
    max_lines: usize,
    cancelled: impl Fn() -> bool,
) -> std::io::Result<ListingLines> {
    let mut collected = ListingLines::default();
    let mut buffer = Vec::new();
    loop {
        if cancelled() {
            collected.cancelled = true;
            return Ok(collected);
        }
        if collected.lines.len() >= max_lines {
            collected.limited = true;
            return Ok(collected);
        }
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 {
            return Ok(collected);
        }
        let line = buffer.strip_suffix(b"\n").unwrap_or(&buffer);
        collected
            .lines
            .push(String::from_utf8_lossy(line).into_owned());
    }
}

/// A tar member name is safe to show and extract only when every component
/// is a plain name: no `..`, no root or drive prefix (GNU tar lists absolute
/// members with their leading `/`), matching ZIP's `enclosed_name`.
pub fn is_enclosed_tar_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
}

/// Split one `tar -tv` line into the member name and its size. GNU tar
/// prints `mode owner/group size date time name`; bsdtar (macOS, Windows)
/// prints `mode links owner group size month day time-or-year name`. The
/// name follows a single space, so leading spaces in it survive. Device
/// entries show `major,minor` instead of a size and report 0.
#[cfg(test)]
fn parse_tar_verbose_line(line: &str) -> Option<(&str, u64)> {
    fn next_field(rest: &str) -> Option<(&str, &str)> {
        let rest = rest.trim_start_matches(' ');
        let end = rest.find(' ')?;
        Some((&rest[..end], &rest[end..]))
    }
    let (mode, mut rest) = next_field(line)?;
    let (second, after_second) = next_field(rest)?;
    let (size_index, field_count) = if second.contains('/') { (2, 5) } else { (4, 8) };
    let mut size = 0;
    rest = after_second;
    for index in 2..field_count {
        let (field, after) = next_field(rest)?;
        if index == size_index {
            size = field.parse().unwrap_or(0);
        }
        rest = after;
    }
    let mut name = rest.strip_prefix(' ')?;
    let link_marker = match mode.as_bytes().first() {
        Some(b'l') => Some(" -> "),
        Some(b'h') => Some(" link to "),
        _ => None,
    };
    if let Some(marker) = link_marker
        && let Some(at) = name.find(marker)
    {
        name = &name[..at];
    }
    Some((name, size))
}

struct SearchZipInput<'a> {
    file: fs::File,
    cancelled: &'a dyn Fn() -> bool,
}
impl Read for SearchZipInput<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if (self.cancelled)() {
            return Err(std::io::Error::other("Archive search cancelled"));
        }
        let count = bytes.len().min(64 * 1024);
        self.file.read(&mut bytes[..count])
    }
}
impl std::io::Seek for SearchZipInput<'_> {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        if (self.cancelled)() {
            return Err(std::io::Error::other("Archive search cancelled"));
        }
        std::io::Seek::seek(&mut self.file, position)
    }
}

fn read_member_text<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    member: &ArchiveMember,
    budget: &mut SearchBudget,
    password: Option<&[u8]>,
) -> Result<Option<Arc<str>>, String> {
    if matches!(
        member.file_type_bits,
        0o010000 | 0o020000 | 0o060000 | 0o140000
    ) || member.size > MAX_MEMBER_TEXT_BYTES
        || budget.content_bytes.saturating_add(member.size) > MAX_TOTAL_TEXT_BYTES
        || suspicious_ratio(member.size, member.compressed_size)
    {
        return Ok(None);
    }
    let file = crate::zip_reader::open_with_password(archive, member.index, password)
        .map_err(|error| format!("Could not read archive member: {error}"))?;
    let remaining = MAX_TOTAL_TEXT_BYTES.saturating_sub(budget.content_bytes);
    let read_limit = MAX_MEMBER_TEXT_BYTES.min(remaining);
    let mut bytes = Vec::with_capacity(member.size.min(read_limit) as usize);
    let result = file.take(read_limit + 1).read_to_end(&mut bytes);
    budget.content_bytes = budget.content_bytes.saturating_add(bytes.len() as u64);
    result.map_err(|error| format!("Could not decompress archive member: {error}"))?;
    if bytes.len() as u64 > read_limit || crate::search_text::binary_head(&bytes) {
        return Ok(None);
    }
    Ok(Some(Arc::from(
        crate::search_text::decode(&bytes).into_owned(),
    )))
}

fn suspicious_ratio(size: u64, compressed_size: u64) -> bool {
    size >= RATIO_GUARD_MIN_BYTES
        && (compressed_size == 0 || size / compressed_size.max(1) > MAX_COMPRESSION_RATIO)
}

fn list_archive(path: PathBuf, cancelled: &AtomicBool) -> Result<ArchiveListing, String> {
    let kind =
        kind_of(&path).ok_or_else(|| format!("Unsupported archive format: {}", path.display()))?;
    let mut budget = SearchBudget::default();
    let mut members = Vec::new();
    let summary = visit_members(
        &path,
        false,
        &mut budget,
        || cancelled.load(Ordering::Acquire),
        |member| {
            members.push(member.member);
            true
        },
    )?;
    if summary.cancelled {
        return Err("Archive listing cancelled".to_string());
    }
    members.sort_by(|left, right| left.path.cmp(&right.path));
    let tree = crate::archive_tree::Tree::build(&members, || cancelled.load(Ordering::Acquire));
    if cancelled.load(Ordering::Acquire) {
        return Err("Archive listing cancelled".into());
    }
    Ok(ArchiveListing {
        tree,
        kind,
        members,
        declared_members: summary.declared_members,
        declared_uncompressed_bytes: summary.declared_uncompressed_bytes,
        unsafe_members: summary.unsafe_members,
        unreadable_members: summary.unreadable_members,
        truncated: budget.truncated,
    })
}

pub fn extract_members(
    archive_path: &Path,
    dest_dir: &Path,
    member_indexes: &[usize],
    cancelled: &AtomicBool,
) -> Result<ExtractReport, String> {
    extract_members_with_password(archive_path, dest_dir, member_indexes, cancelled, None)
}

/// Extract using an ephemeral password. The caller owns the password lifetime;
/// reports contain member names and decoder errors, never password contents.
pub fn extract_members_with_password(
    archive_path: &Path,
    dest_dir: &Path,
    member_indexes: &[usize],
    cancelled: &AtomicBool,
    password: Option<&[u8]>,
) -> Result<ExtractReport, String> {
    if member_indexes.is_empty() {
        return Ok(ExtractReport::default());
    }
    fs::create_dir_all(dest_dir).map_err(|error| {
        format!(
            "Could not create extract folder {}: {error}",
            dest_dir.display()
        )
    })?;
    match kind_of(archive_path) {
        Some(ArchiveKind::Zip) => {
            extract_zip_members(archive_path, dest_dir, member_indexes, cancelled, password)
        }
        Some(
            ArchiveKind::TarGz
            | ArchiveKind::TarBz2
            | ArchiveKind::TarXz
            | ArchiveKind::TarZstd
            | ArchiveKind::Tar,
        ) => extract_tar_members(archive_path, dest_dir, member_indexes, cancelled),
        None => Err(format!(
            "Unsupported archive format: {}",
            archive_path.display()
        )),
    }
}

/// Keep cancellation responsive during encrypted authentication tails too,
/// where a codec has already reached EOS and the outer copy loop cannot poll.
struct ZipExtractionInput<'a, R> {
    reader: R,
    cancelled: &'a AtomicBool,
}
impl<R: Read> Read for ZipExtractionInput<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.cancelled.load(Ordering::Acquire) {
            // Interrupted would be retried by read_exact; cancellation is terminal.
            return Err(std::io::Error::other("Extraction cancelled"));
        }
        let length = bytes.len().min(64 * 1024);
        self.reader.read(&mut bytes[..length])
    }
}
impl<R: std::io::Seek> std::io::Seek for ZipExtractionInput<'_, R> {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        self.reader.seek(position)
    }
}

fn extract_zip_members(
    archive_path: &Path,
    dest_dir: &Path,
    member_indexes: &[usize],
    cancelled: &AtomicBool,
    password: Option<&[u8]>,
) -> Result<ExtractReport, String> {
    let file = fs::File::open(archive_path)
        .map_err(|error| format!("Could not open archive {}: {error}", archive_path.display()))?;
    if cancelled.load(Ordering::Acquire) {
        return Ok(ExtractReport {
            errors: vec!["Extraction cancelled".into()],
            ..ExtractReport::default()
        });
    }
    let input = ZipExtractionInput {
        reader: file,
        cancelled,
    };
    let mut archive = zip::ZipArchive::new(input)
        .map_err(|error| format!("Could not read archive {}: {error}", archive_path.display()))?;
    let mut report = ExtractReport::default();
    for &index in member_indexes {
        if cancelled.load(Ordering::Acquire) {
            report.errors.push("Extraction cancelled".to_string());
            break;
        }
        // The browser lists members by name, never by index, so errors name
        // the member even when decoding cannot start (encrypted or an
        // unsupported method).
        let label = archive
            .name_for_index(index)
            .and_then(Result::ok)
            .map_or_else(|| format!("#{index}"), |name| name.into_owned());
        let mut entry = match crate::zip_reader::open_with_password(&mut archive, index, password) {
            Ok(entry) => entry,
            Err(error) => {
                report
                    .errors
                    .push(format!("Could not open member {label}: {error}"));
                continue;
            }
        };
        let Some(enclosed) = entry.enclosed_name() else {
            report.errors.push(format!("Skipped unsafe member {label}"));
            continue;
        };
        let out_path = dest_dir.join(&enclosed);
        if entry.is_dir() {
            // Even an empty encrypted directory has an authentication tag.
            // Verify it before creating the directory; the password verifier
            // alone is not proof that the password/data is correct.
            if entry.encrypted()
                && let Err(error) = std::io::copy(&mut entry, &mut std::io::sink())
            {
                report
                    .errors
                    .push(format!("Could not verify directory {label}: {error}"));
                continue;
            }
            if let Err(error) = fs::create_dir_all(&out_path) {
                report
                    .errors
                    .push(format!("Could not create {}: {error}", out_path.display()));
            } else {
                report.skipped_dirs += 1;
            }
            continue;
        }
        if let Some(parent) = out_path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            report
                .errors
                .push(format!("Could not create {}: {error}", parent.display()));
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            extract_zip_symlink(&mut entry, &enclosed, &out_path, &mut report);
            continue;
        }
        // `create_new` never clobbers and never writes through a symlink
        // (even a dangling one) already at the destination, and it closes
        // the race an `exists()` pre-check would leave open.
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out_path)
        {
            // Windows reports an existing directory as access denied, so any
            // entry already present there also counts as skipped.
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    || fs::symlink_metadata(&out_path).is_ok() =>
            {
                report.skipped_existing += 1;
            }
            Ok(mut out) => match copy_until_cancelled(&mut entry, &mut out, cancelled) {
                Ok(true) => {
                    apply_zip_metadata(&out, &entry);
                    report.extracted += 1;
                }
                Ok(false) => {
                    // Cancelled mid-member: never leave a truncated file.
                    drop(out);
                    let _ = fs::remove_file(&out_path);
                    report.errors.push("Extraction cancelled".to_string());
                    break;
                }
                Err(error) => {
                    let _ = fs::remove_file(&out_path);
                    report
                        .errors
                        .push(format!("Could not write {}: {error}", out_path.display()));
                }
            },
            Err(error) => report
                .errors
                .push(format!("Could not create {}: {error}", out_path.display())),
        }
    }
    Ok(report)
}

fn symlink_target_stays_inside(member: &Path, target: &Path) -> bool {
    if target.as_os_str().is_empty() || target.is_absolute() {
        return false;
    }
    let mut depth = member
        .parent()
        .map_or(0, |parent| parent.components().count());
    for component in target.components() {
        match component {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if depth > 0 => depth -= 1,
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return false,
        }
    }
    true
}

fn extract_zip_symlink<R: Read>(
    entry: &mut crate::zip_reader::Member<'_, R>,
    member: &Path,
    out_path: &Path,
    report: &mut ExtractReport,
) {
    let oversized = entry.size() > 4_096;
    let mut bytes = Vec::new();
    if let Err(error) = (&mut *entry).take(4_097).read_to_end(&mut bytes) {
        report.errors.push(format!(
            "Could not read symlink {}: {error}",
            member.display()
        ));
        return;
    }
    let Ok(target) = String::from_utf8(bytes) else {
        report.errors.push(format!(
            "Skipped symlink {} with a non-UTF-8 target",
            member.display()
        ));
        return;
    };
    let target = Path::new(&target);
    if oversized || !symlink_target_stays_inside(member, target) {
        report.errors.push(format!(
            "Skipped unsafe symlink {} -> {}",
            member.display(),
            target.display()
        ));
        return;
    }
    if fs::symlink_metadata(out_path).is_ok() {
        report.skipped_existing += 1;
        return;
    }
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, out_path);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(target, out_path);
    #[cfg(not(any(unix, windows)))]
    let result: std::io::Result<()> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symbolic links are unsupported on this platform",
    ));
    match result {
        Ok(()) => report.extracted += 1,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            report.skipped_existing += 1;
        }
        Err(error) => report.errors.push(format!(
            "Could not create symlink {}: {error}",
            out_path.display()
        )),
    }
}

/// Copy `reader` to `writer` in chunks, checking `cancelled` between them so
/// closing the dialog stops a large member promptly. `Ok(false)` = cancelled.
fn copy_until_cancelled(
    reader: &mut impl Read,
    writer: &mut impl std::io::Write,
    cancelled: &AtomicBool,
) -> std::io::Result<bool> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Ok(false);
        }
        let read = match reader.read(&mut buf) {
            Ok(0) => return Ok(true),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        writer.write_all(&buf[..read])?;
    }
}

/// Restore the member's permission bits and modification time, as the tar
/// path does, so extracted scripts stay executable and files keep their
/// dates. Best effort: a failure leaves the defaults and is only logged.
fn apply_zip_metadata<R: Read>(out: &fs::File, entry: &zip::read::ZipFile<'_, R>) {
    let name = entry
        .name()
        .map_or_else(|_| "<invalid member name>".into(), |name| name.into_owned());
    #[cfg(unix)]
    if let Some(mode) = entry.unix_mode() {
        use std::os::unix::fs::PermissionsExt;
        // Only rwx bits: setuid/setgid/sticky from an archive are not kept.
        if let Err(error) = out.set_permissions(fs::Permissions::from_mode(mode & 0o777)) {
            log::debug!("zip extract: could not set mode of {}: {error}", name);
        }
    }
    if let Some(modified) = zip_member_mtime(entry)
        && let Err(error) = out.set_modified(modified)
    {
        log::debug!("zip extract: could not set mtime of {}: {error}", name);
    }
}

/// A member's modification time: the UTC extended-timestamp field when the
/// archiver wrote one, else the MS-DOS field, which is local time.
fn zip_member_mtime<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> Option<std::time::SystemTime> {
    let extended = entry.extra_data_fields().find_map(|field| match field {
        zip::extra_fields::ExtraField::ExtendedTimestamp(ts) => ts.mod_time(),
        _ => None,
    });
    if let Some(secs) = extended {
        return Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(secs)));
    }
    dos_time_to_system(entry.last_modified()?)
}

fn dos_time_to_system(time: zip::DateTime) -> Option<std::time::SystemTime> {
    use chrono::TimeZone;
    let naive = chrono::NaiveDate::from_ymd_opt(
        i32::from(time.year()),
        u32::from(time.month()),
        u32::from(time.day()),
    )?
    .and_hms_opt(
        u32::from(time.hour()),
        u32::from(time.minute()),
        u32::from(time.second()),
    )?;
    let local = chrono::Local.from_local_datetime(&naive).earliest()?;
    Some(local.into())
}

struct PreparedTar {
    path: PathBuf,
}
impl Drop for PreparedTar {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Stage only decoder output, never extracted member paths. Keeping external tar
/// on raw TAR avoids depending on the host's optional compression libraries.
fn prepare_tar(
    path: &Path,
    kind: ArchiveKind,
    cancelled: &AtomicBool,
    stream_limit: u64,
) -> Result<PreparedTar, String> {
    use std::io::Write;
    let cancel = || cancelled.load(Ordering::Acquire);
    let mut reader = tar_reader(
        fs::File::open(path).map_err(|e| e.to_string())?,
        kind,
        &cancel,
    )?;
    let (prepared, mut output) = loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("Extraction cancelled".into());
        }
        let path = std::env::temp_dir().join(format!(
            "commander-decoded-tar-{}-{:032x}.tar",
            std::process::id(),
            rand::random::<u128>()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => break (PreparedTar { path }, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    };
    let mut total = 0_u64;
    let mut chunk = [0; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("Extraction cancelled".into());
        }
        let count = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > stream_limit {
            return Err(format!(
                "Decoded TAR exceeds the extraction staging limit ({stream_limit} bytes)"
            ));
        }
        output
            .write_all(&chunk[..count])
            .map_err(|e| e.to_string())?;
    }
    drop(output);
    Ok(prepared)
}

#[cfg(any(test, feature = "visual-qa"))]
pub fn encode_tar_fixture(kind: ArchiveKind, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    match kind {
        ArchiveKind::Tar => Ok(bytes.to_vec()),
        ArchiveKind::TarGz => {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(bytes)?;
            encoder.finish()
        }
        ArchiveKind::TarBz2 => {
            let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            encoder.write_all(bytes)?;
            encoder.finish()
        }
        ArchiveKind::TarXz => {
            let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 1);
            encoder.write_all(bytes)?;
            encoder.finish()
        }
        ArchiveKind::TarZstd => zstd::stream::encode_all(bytes, 1),
        ArchiveKind::Zip => Err(std::io::Error::other("Not a TAR fixture")),
    }
}

fn extract_tar_members(
    archive_path: &Path,
    dest_dir: &Path,
    member_indexes: &[usize],
    cancelled: &AtomicBool,
) -> Result<ExtractReport, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Extraction cancelled".to_string());
    }
    let listing = list_archive(archive_path.to_path_buf(), cancelled)?;
    let mut report = ExtractReport::default();
    let mut wanted = Vec::new();
    // Where each wanted member lands, to count what a failed run still wrote.
    let mut wanted_outputs = Vec::new();
    // Index once: "Extract all" passes every member, and a per-index scan of
    // the listing would be quadratic on large archives.
    let by_index: std::collections::HashMap<usize, &ArchiveMember> = listing
        .members
        .iter()
        .map(|member| (member.index, member))
        .collect();
    for &index in member_indexes {
        let Some(member) = by_index.get(&index).copied() else {
            report
                .errors
                .push(format!("Unknown archive member #{index}"));
            continue;
        };
        if member.is_dir {
            let out = dest_dir.join(&member.path);
            if let Err(error) = fs::create_dir_all(&out) {
                report
                    .errors
                    .push(format!("Could not create {}: {error}", out.display()));
            } else {
                report.skipped_dirs += 1;
            }
            continue;
        }
        let out = dest_dir.join(&member.path);
        if out.exists() {
            report.skipped_existing += 1;
            continue;
        }
        wanted_outputs.push(out);
        wanted.push(
            member
                .tar_name
                .clone()
                .unwrap_or_else(|| member.path.as_os_str().to_owned()),
        );
    }
    if wanted.is_empty() {
        return Ok(report);
    }
    // Member names go through a NUL-separated list file (`--null -T`), not
    // argv: "Extract all" on a large archive would otherwise exceed ARG_MAX
    // and tar never runs. A file rather than stdin because bsdtar (macOS,
    // Windows) does not read `-T -` from stdin when extracting.
    let mut names = Vec::new();
    for member in &wanted {
        let operand = tar_extract_operand(member);
        names.extend_from_slice(&operand);
        names.push(0);
    }
    static LIST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let list_path = std::env::temp_dir().join(format!(
        "commander-tar-names-{}-{}",
        std::process::id(),
        LIST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::write(&list_path, &names)
        .map_err(|error| format!("Could not extract {}: {error}", archive_path.display()))?;
    let prepared = match kind_of(archive_path) {
        Some(kind @ (ArchiveKind::TarBz2 | ArchiveKind::TarXz | ArchiveKind::TarZstd)) => {
            match prepare_tar(archive_path, kind, cancelled, MAX_TAR_STREAM_BYTES) {
                Ok(prepared) => Some(prepared),
                Err(error) => {
                    let _ = fs::remove_file(&list_path);
                    return Err(error);
                }
            }
        }
        _ => None,
    };
    let tar_path = prepared
        .as_ref()
        .map_or(archive_path, |prepared| prepared.path.as_path());
    let output = std::process::Command::new("tar")
        .arg(
            if prepared.is_none() && kind_of(archive_path) == Some(ArchiveKind::TarGz) {
                "-xzf"
            } else {
                "-xf"
            },
        )
        .arg(tar_path)
        .arg("-C")
        .arg(dest_dir)
        .arg("--null")
        .arg("-T")
        .arg(&list_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|child| wait_child_or_cancel(child, cancelled));
    let _ = fs::remove_file(&list_path);
    let output =
        output.map_err(|error| format!("Could not extract {}: {error}", archive_path.display()))?;
    let Some(output) = output else {
        // tar was killed mid-member, so any of this run's outputs may be
        // truncated; none existed before the run, so removing them is safe.
        for out in &wanted_outputs {
            let _ = fs::remove_file(out);
        }
        report.errors.push("Extraction cancelled".to_string());
        return Ok(report);
    };
    if output.status.success() {
        report.extracted += wanted.len();
    } else {
        // tar keeps going past a failed member, so some files may have
        // landed; none existed before this run, so whatever exists now did.
        report.extracted += wanted_outputs
            .iter()
            .filter(|out| fs::symlink_metadata(out).is_ok())
            .count();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        report.errors.push(format!(
            "tar extract failed: {}",
            if detail.is_empty() {
                "unknown error"
            } else {
                detail
            }
        ));
    }
    Ok(report)
}

/// bsdtar treats extraction operands as glob patterns even when they arrive
/// through a NUL-delimited `-T` file. GNU tar makes `--null` verbatim, so only
/// libarchive targets receive pattern escaping.
fn tar_extract_operand(name: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        name.as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = name.to_string_lossy().as_bytes().to_vec();
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    {
        if bytes
            .iter()
            .any(|byte| matches!(byte, b'\\' | b'*' | b'?' | b'['))
        {
            let mut escaped = Vec::with_capacity(bytes.len() + 4);
            for &byte in &bytes {
                if matches!(byte, b'\\' | b'*' | b'?' | b'[') {
                    escaped.push(b'\\');
                }
                escaped.push(byte);
            }
            return escaped;
        }
    }
    bytes
}

/// Wait for `child`, killing it when `cancelled` is set: closing the dialog
/// must not leave tar extracting a large archive in the background.
/// `Ok(None)` = cancelled. Stderr is drained on a thread so a chatty tar
/// cannot block on a full pipe.
fn wait_child_or_cancel(
    mut child: std::process::Child,
    cancelled: &AtomicBool,
) -> std::io::Result<Option<std::process::Output>> {
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    });
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    Ok(Some(std::process::Output {
        status,
        stdout: Vec::new(),
        stderr,
    }))
}

pub struct ExtractRun {
    receiver: Receiver<Result<ExtractReport, String>>,
    cancelled: Arc<AtomicBool>,
}

impl ExtractRun {
    pub fn try_recv(&self) -> Result<Result<ExtractReport, String>, TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for ExtractRun {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub fn start_extract(
    archive: PathBuf,
    dest: PathBuf,
    member_indexes: Vec<usize>,
    password: Option<zeroize::Zeroizing<String>>,
    notify: Notify,
) -> ExtractRun {
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = if let Some(password) = password {
            extract_members_with_password(
                &archive,
                &dest,
                &member_indexes,
                &worker_cancelled,
                Some(password.as_bytes()),
            )
        } else {
            extract_members(&archive, &dest, &member_indexes, &worker_cancelled)
        };
        let _ = sender.send(result);
        notify();
    });
    ExtractRun {
        receiver,
        cancelled,
    }
}

pub struct ListingRun {
    receiver: Receiver<Result<ArchiveListing, String>>,
    cancelled: Arc<AtomicBool>,
}

impl ListingRun {
    pub fn try_recv(&self) -> Result<Result<ArchiveListing, String>, TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for ListingRun {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub fn start_listing(path: PathBuf, notify: Notify) -> ListingRun {
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = list_archive(path, &worker_cancelled);
        let _ = sender.send(result);
        notify();
    });
    ListingRun {
        receiver,
        cancelled,
    }
}

/// A ZIP member's modification time: the extended-timestamp field (UTC
/// seconds) when present, else the DOS date and time, which ZIP stores in
/// the writer's local time and so is read as local time here.
fn zip_member_modified<R: Read>(raw: &zip::read::ZipFile<'_, R>) -> Option<std::time::SystemTime> {
    let extended = raw.extra_data_fields().find_map(|field| match field {
        zip::ExtraField::ExtendedTimestamp(stamp) => stamp.mod_time(),
        zip::ExtraField::Ntfs(_) => None,
        _ => None,
    });
    if let Some(seconds) = extended {
        return Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(seconds)));
    }
    raw.last_modified().and_then(dos_local_time)
}

/// The DOS date and time as local time. `None` for the 1980-01-01 00:00:00
/// default that writers store when they record no time, and for a
/// date that does not exist locally.
fn dos_local_time(time: zip::DateTime) -> Option<std::time::SystemTime> {
    use chrono::TimeZone;
    if time == zip::DateTime::default() {
        return None;
    }
    let local = chrono::Local
        .with_ymd_and_hms(
            i32::from(time.year()),
            u32::from(time.month()),
            u32::from(time.day()),
            u32::from(time.hour()),
            u32::from(time.minute()),
            u32::from(time.second()),
        )
        .earliest()?;
    Some(local.into())
}

#[cfg(test)]
mod tests;
