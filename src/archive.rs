//! Bounded archive inspection and extraction shared by the archive browser and
//! search. ZIP and plain/gzip/bzip2/XZ/Zstandard TAR listing are in-process; selected tar extraction uses
//! system `tar`. Auto-inspect callers should consult `crate::trust` (J003 hooks).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

mod tar_archive;
pub use tar_archive::*;
mod zip_extract;
use zip_extract::*;

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
