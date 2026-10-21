//! Bounded archive inspection and extraction shared by the archive browser and
//! search. ZIP is in-process; `.tar.gz`/`.tgz` use system `tar`. Auto-inspect
//! callers should consult `crate::trust` (J003 hooks).

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
}

impl ArchiveKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Zip => "ZIP",
            Self::TarGz => "tar.gz",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ArchiveMember {
    pub index: usize,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub compressed_size: u64,
    /// The member name exactly as `tar` listed it (for example with a `./`
    /// prefix). GNU tar matches extract names literally, so extraction must
    /// pass this rather than the cleaned-up `path`. `None` for zip members.
    pub tar_name: Option<String>,
    /// The member's own modification time when the archive records one
    /// (ZIP); `None` for tar members and ZIP entries with no usable time,
    /// where callers fall back to the archive file's mtime.
    pub modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug)]
pub struct ArchiveListing {
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
    /// The archive's members were not searched at all because content search
    /// is not supported for this format (tar.gz would need a full decompress).
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
    } else if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
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
        Some(ArchiveKind::TarGz) => {
            if needs_content {
                budget.archives += 1;
                return Ok(VisitSummary {
                    members_not_searched: true,
                    ..Default::default()
                });
            }
            return visit_tar_gz_list(archive_path, budget, cancelled, |member| {
                emit(SearchMember {
                    member,
                    content: None,
                })
            });
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
    let mut archive = zip::ZipArchive::new(file)
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
                size: raw.size(),
                compressed_size: raw.compressed_size(),
                tar_name: None,
                modified: zip_member_modified(&raw),
            }
        };

        let content = if needs_content && !member.is_dir {
            match read_member_text(&mut archive, &member, budget) {
                Ok(Some(content)) => Some(content),
                Ok(None) | Err(_) => {
                    summary.content_skipped += 1;
                    None
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
    Ok(summary)
}

fn visit_tar_gz_list(
    archive_path: &Path,
    budget: &mut SearchBudget,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(ArchiveMember) -> bool,
) -> Result<VisitSummary, String> {
    budget.archives += 1;
    // Verbose listing so member sizes are known; `LC_ALL=C` keeps bsdtar's
    // date columns in a fixed shape for `parse_tar_verbose_line`. The output
    // is read incrementally so cancellation and the member budget stop a
    // multi-GB archive early instead of waiting for tar to finish.
    let mut child = std::process::Command::new("tar")
        .args(["-tvzf"])
        .arg(archive_path)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not list {}: {error}", archive_path.display()))?;
    // Drain stderr on a helper thread so a chatty tar cannot block on a
    // full stderr pipe while stdout is still being read.
    let stderr = child.stderr.take();
    let stderr_reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut text);
        }
        text
    });
    // One line beyond the remaining budget is enough to mark truncation.
    let max_lines = MAX_MEMBERS_PER_RUN
        .saturating_sub(budget.members)
        .saturating_add(1);
    let collected = match child.stdout.take() {
        Some(stdout) => {
            collect_listing_lines(std::io::BufReader::new(stdout), max_lines, &cancelled)
        }
        None => Ok(ListingLines::default()),
    };
    let stopped_early = collected
        .as_ref()
        .map_or(true, |lines| lines.cancelled || lines.limited);
    if stopped_early {
        // Stopped at the member budget or on cancel: tar is killed, so its
        // exit status and stderr are deliberately ignored. A gzip or tar
        // error past the cut-off is not reported for a truncated listing.
        let _ = child.kill();
    }
    let status = child.wait();
    let stderr = stderr_reader.join().unwrap_or_default();
    let collected =
        collected.map_err(|error| format!("Could not list {}: {error}", archive_path.display()))?;
    if collected.cancelled {
        return Ok(VisitSummary {
            cancelled: true,
            ..Default::default()
        });
    }
    if !stopped_early {
        let status = status
            .map_err(|error| format!("Could not list {}: {error}", archive_path.display()))?;
        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr);
            return Err(format!(
                "Could not list {}: {}",
                archive_path.display(),
                stderr.trim()
            ));
        }
    }
    let mut summary = VisitSummary::default();
    for (index, line) in collected.lines.iter().map(String::as_str).enumerate() {
        if cancelled() {
            summary.cancelled = true;
            break;
        }
        if budget.members >= MAX_MEMBERS_PER_RUN {
            budget.truncated = true;
            break;
        }
        budget.members += 1;
        summary.declared_members += 1;
        let Some((name, size)) = parse_tar_verbose_line(line.strip_suffix('\r').unwrap_or(line))
        else {
            summary.unreadable_members += 1;
            continue;
        };
        // Only the `./` prefix is stripped: leading/trailing spaces are part
        // of the member name and `tar` needs them to find it on extract.
        let trimmed = name.trim_start_matches("./");
        if trimmed.is_empty() || trimmed == "." {
            continue;
        }
        let path = PathBuf::from(trimmed);
        if !is_enclosed_tar_path(&path) {
            summary.unsafe_members += 1;
            continue;
        }
        let is_dir = trimmed.ends_with('/');
        let member = ArchiveMember {
            index,
            path: if is_dir {
                PathBuf::from(trimmed.trim_end_matches('/'))
            } else {
                path
            },
            is_dir,
            size: if is_dir { 0 } else { size },
            compressed_size: 0,
            tar_name: Some(name.to_string()),
            modified: None,
        };
        if !emit(member) {
            break;
        }
        summary.emitted += 1;
    }
    Ok(summary)
}

/// Raw `tar -tv` output lines, read until EOF, cancellation or `max_lines`.
#[derive(Debug, Default)]
struct ListingLines {
    lines: Vec<String>,
    cancelled: bool,
    /// `max_lines` was reached before EOF; the rest of the output was not read.
    limited: bool,
}

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
fn is_enclosed_tar_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
}

/// Split one `tar -tv` line into the member name and its size. GNU tar
/// prints `mode owner/group size date time name`; bsdtar (macOS, Windows)
/// prints `mode links owner group size month day time-or-year name`. The
/// name follows a single space, so leading spaces in it survive. Device
/// entries show `major,minor` instead of a size and report 0.
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

fn read_member_text(
    archive: &mut zip::ZipArchive<fs::File>,
    member: &ArchiveMember,
    budget: &mut SearchBudget,
) -> Result<Option<Arc<str>>, String> {
    if member.size > MAX_MEMBER_TEXT_BYTES
        || budget.content_bytes.saturating_add(member.size) > MAX_TOTAL_TEXT_BYTES
        || suspicious_ratio(member.size, member.compressed_size)
    {
        return Ok(None);
    }
    let file = archive
        .by_index(member.index)
        .map_err(|error| format!("Could not read archive member: {error}"))?;
    let remaining = MAX_TOTAL_TEXT_BYTES.saturating_sub(budget.content_bytes);
    let read_limit = MAX_MEMBER_TEXT_BYTES.min(remaining);
    let mut bytes = Vec::with_capacity(member.size.min(read_limit) as usize);
    let result = file.take(read_limit + 1).read_to_end(&mut bytes);
    budget.content_bytes = budget.content_bytes.saturating_add(bytes.len() as u64);
    result.map_err(|error| format!("Could not decompress archive member: {error}"))?;
    if bytes.len() as u64 > read_limit || bytes.iter().take(8_192).any(|byte| *byte == 0) {
        return Ok(None);
    }
    Ok(Some(Arc::from(
        String::from_utf8_lossy(&bytes).into_owned(),
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
    Ok(ArchiveListing {
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
            extract_zip_members(archive_path, dest_dir, member_indexes, cancelled)
        }
        Some(ArchiveKind::TarGz) => {
            extract_tar_gz_members(archive_path, dest_dir, member_indexes, cancelled)
        }
        None => Err(format!(
            "Unsupported archive format: {}",
            archive_path.display()
        )),
    }
}

fn extract_zip_members(
    archive_path: &Path,
    dest_dir: &Path,
    member_indexes: &[usize],
    cancelled: &AtomicBool,
) -> Result<ExtractReport, String> {
    let file = fs::File::open(archive_path)
        .map_err(|error| format!("Could not open archive {}: {error}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
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
        let mut entry = match archive.by_index(index) {
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

fn extract_zip_symlink(
    entry: &mut zip::read::ZipFile<'_, fs::File>,
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
fn apply_zip_metadata(out: &fs::File, entry: &zip::read::ZipFile<'_, fs::File>) {
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
fn zip_member_mtime(entry: &zip::read::ZipFile<'_, fs::File>) -> Option<std::time::SystemTime> {
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

fn extract_tar_gz_members(
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
                .unwrap_or_else(|| member.path.to_string_lossy().into_owned()),
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
        names.extend_from_slice(operand.as_bytes());
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
    let output = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive_path)
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
fn tar_extract_operand(name: &str) -> std::borrow::Cow<'_, str> {
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    {
        if name.contains(['\\', '*', '?', '[']) {
            let mut escaped = String::with_capacity(name.len() + 4);
            for ch in name.chars() {
                if matches!(ch, '\\' | '*' | '?' | '[') {
                    escaped.push('\\');
                }
                escaped.push(ch);
            }
            return std::borrow::Cow::Owned(escaped);
        }
    }
    std::borrow::Cow::Borrowed(name)
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
    notify: Notify,
) -> ExtractRun {
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = extract_members(&archive, &dest, &member_indexes, &worker_cancelled);
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
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::io::Write;

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn zip_members_carry_their_own_modified_time() {
        let temp = TempDir::new();
        let path = temp.path().join("dated.zip");
        let stamp = zip::DateTime::from_date_and_time(2001, 2, 3, 4, 5, 6).unwrap();
        let file = fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let dated = zip::write::SimpleFileOptions::default().last_modified_time(stamp);
        writer.start_file("dated.txt", dated).unwrap();
        writer.write_all(b"d").unwrap();
        // No time recorded: the DOS default, which must not read as 1980.
        let undated =
            zip::write::SimpleFileOptions::default().last_modified_time(zip::DateTime::default());
        writer.start_file("undated.txt", undated).unwrap();
        writer.write_all(b"u").unwrap();
        writer.finish().unwrap();

        let cancelled = AtomicBool::new(false);
        let listing = list_archive(path, &cancelled).unwrap();
        let modified = |name: &str| {
            listing
                .members
                .iter()
                .find(|member| member.path == Path::new(name))
                .unwrap()
                .modified
        };
        let expected: std::time::SystemTime = {
            use chrono::TimeZone;
            chrono::Local
                .with_ymd_and_hms(2001, 2, 3, 4, 5, 6)
                .earliest()
                .unwrap()
                .into()
        };
        assert_eq!(modified("dated.txt"), Some(expected));
        assert_eq!(modified("undated.txt"), None);
    }

    #[test]
    fn supported_archive_detection_is_case_insensitive() {
        assert!(is_supported(Path::new("bundle.ZIP")));
        assert_eq!(kind_of(Path::new("bundle.tgz")), Some(ArchiveKind::TarGz));
        assert_eq!(
            kind_of(Path::new("bundle.tar.gz")),
            Some(ArchiveKind::TarGz)
        );
        assert!(!is_supported(Path::new("bundle.tar")));
    }

    #[test]
    fn extract_writes_selected_zip_members_without_clobber() {
        let temp = TempDir::new();
        let archive = temp.path().join("pack.zip");
        write_zip(
            &archive,
            &[("readme.txt", b"hello"), ("nested/a.txt", b"nested")],
        );
        let dest = temp.path().join("out");
        let cancelled = AtomicBool::new(false);
        let listing = list_archive(archive.clone(), &cancelled).unwrap();
        let indexes: Vec<usize> = listing
            .members
            .iter()
            .filter(|member| !member.is_dir)
            .map(|member| member.index)
            .collect();
        let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();
        assert_eq!(report.extracted, 2);
        assert_eq!(
            fs::read_to_string(dest.join("readme.txt")).unwrap(),
            "hello"
        );
        let again = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();
        assert_eq!(again.extracted, 0);
        assert_eq!(again.skipped_existing, 2);
    }

    #[test]
    fn listing_rejects_parent_traversal_members() {
        let temp = TempDir::new();
        let path = temp.path().join("unsafe.zip");
        write_zip(
            &path,
            &[("../escape.txt", b"bad"), ("safe/readme.txt", b"ok")],
        );
        let cancelled = AtomicBool::new(false);

        let listing = list_archive(path, &cancelled).unwrap();

        assert_eq!(listing.unsafe_members, 1);
        assert_eq!(listing.members.len(), 1);
        assert_eq!(listing.members[0].path, PathBuf::from("safe/readme.txt"));
    }

    #[test]
    fn member_visit_reads_text_and_skips_binary_content() {
        let temp = TempDir::new();
        let path = temp.path().join("content.zip");
        write_zip(
            &path,
            &[("notes.txt", b"searchable"), ("image.bin", b"a\0b")],
        );
        let mut budget = SearchBudget::default();
        let mut visited = Vec::new();

        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |member| {
                visited.push(member);
                true
            },
        )
        .unwrap();

        assert_eq!(visited.len(), 2);
        assert_eq!(summary.content_skipped, 1);
        assert_eq!(budget.content_bytes, 13);
        assert!(visited.iter().any(|member| {
            member.member.path == Path::new("notes.txt")
                && member.content.as_deref() == Some("searchable")
        }));
    }

    #[test]
    fn suspicious_compression_ratio_is_not_decompressed_for_search() {
        let temp = TempDir::new();
        let path = temp.path().join("ratio.zip");
        let repeated = vec![b'a'; 512 * 1024];
        write_zip(&path, &[("large.txt", &repeated)]);
        let mut budget = SearchBudget::default();
        let mut content = None;

        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |member| {
                content = member.content;
                true
            },
        )
        .unwrap();

        assert!(content.is_none());
        assert_eq!(summary.content_skipped, 1);
        assert_eq!(budget.content_bytes, 0);
    }

    #[test]
    fn cancellation_stops_before_visiting_members() {
        let temp = TempDir::new();
        let path = temp.path().join("cancel.zip");
        write_zip(&path, &[("one.txt", b"one")]);
        let mut budget = SearchBudget::default();

        let summary = visit_members(
            &path,
            false,
            &mut budget,
            || true,
            |_| panic!("cancelled visit emitted a member"),
        )
        .unwrap();

        assert!(summary.cancelled);
        assert_eq!(summary.emitted, 0);
    }

    #[test]
    fn member_budget_cap_truncates_mid_archive() {
        let temp = TempDir::new();
        let path = temp.path().join("cap.zip");
        write_zip(&path, &[("one.txt", b"1"), ("two.txt", b"2")]);
        // One member left in the run's budget: the second is never visited.
        let mut budget = SearchBudget {
            members: MAX_MEMBERS_PER_RUN - 1,
            ..Default::default()
        };
        let mut visited = Vec::new();

        let summary = visit_members(
            &path,
            false,
            &mut budget,
            || false,
            |member| {
                visited.push(member.member.path);
                true
            },
        )
        .unwrap();

        assert_eq!(visited, [PathBuf::from("one.txt")]);
        assert_eq!(summary.emitted, 1);
        assert!(budget.truncated);
        assert_eq!(budget.members, MAX_MEMBERS_PER_RUN);
    }

    #[test]
    fn exhausted_budget_skips_the_archive_without_opening_it() {
        // Not even a zip: a spent budget must return before reading it.
        let temp = TempDir::new();
        let path = temp.path().join("garbage.zip");
        fs::write(&path, b"not a zip").unwrap();
        for mut budget in [
            SearchBudget {
                archives: MAX_ARCHIVES_PER_SEARCH,
                ..Default::default()
            },
            SearchBudget {
                members: MAX_MEMBERS_PER_RUN,
                ..Default::default()
            },
        ] {
            let before = budget.archives;

            let summary = visit_members(
                &path,
                true,
                &mut budget,
                || false,
                |_| panic!("budget-capped visit emitted a member"),
            )
            .unwrap();

            assert_eq!(summary, VisitSummary::default());
            assert!(budget.truncated);
            assert_eq!(budget.archives, before, "a skipped archive was counted");
        }
    }

    #[test]
    fn zip_extract_skips_unsafe_members_and_keeps_going() {
        let temp = TempDir::new();
        let archive = temp.path().join("mixed.zip");
        write_zip(&archive, &[("../escape.txt", b"bad"), ("ok.txt", b"ok")]);
        let dest = temp.path().join("out");
        let cancelled = AtomicBool::new(false);

        let report = extract_members(&archive, &dest, &[0, 1], &cancelled).unwrap();

        assert_eq!(report.extracted, 1);
        assert_eq!(report.errors, ["Skipped unsafe member ../escape.txt"]);
        assert_eq!(fs::read(dest.join("ok.txt")).unwrap(), b"ok");
        assert!(!temp.path().join("escape.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn zip_extract_does_not_write_through_dangling_symlink() {
        let temp = TempDir::new();
        let archive = temp.path().join("pack.zip");
        write_zip(&archive, &[("readme.txt", b"hello")]);
        let dest = temp.path().join("out");
        fs::create_dir(&dest).unwrap();
        let target = temp.path().join("elsewhere.txt");
        std::os::unix::fs::symlink(&target, dest.join("readme.txt")).unwrap();
        let cancelled = AtomicBool::new(false);

        let report = extract_members(&archive, &dest, &[0], &cancelled).unwrap();

        assert_eq!(report.extracted, 0);
        assert_eq!(report.skipped_existing, 1);
        assert!(!target.exists(), "extraction followed the dangling link");
    }

    #[cfg(unix)]
    #[test]
    fn zip_extract_creates_only_symlinks_that_stay_inside_the_destination() {
        let temp = TempDir::new();
        let archive = temp.path().join("links.zip");
        let file = fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .add_symlink("dir/link", "../target.txt", options)
            .unwrap();
        writer
            .add_symlink("escape", "../../outside", options)
            .unwrap();
        writer.finish().unwrap();
        let cancelled = AtomicBool::new(false);
        let listing = list_archive(archive.clone(), &cancelled).unwrap();
        let indexes: Vec<_> = listing.members.iter().map(|member| member.index).collect();
        let dest = temp.path().join("out");

        let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

        assert_eq!(report.extracted, 1);
        assert_eq!(
            fs::read_link(dest.join("dir/link")).unwrap(),
            PathBuf::from("../target.txt")
        );
        assert!(!dest.join("escape").exists());
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("unsafe symlink"))
        );
    }

    #[test]
    fn listing_lines_stop_at_limit_and_on_cancel() {
        // An endless listing: without the incremental reader this never ends.
        let endless = std::io::BufReader::new(std::io::repeat(b'\n'));
        let limited = collect_listing_lines(endless, 3, || false).unwrap();
        assert_eq!(limited.lines.len(), 3);
        assert!(limited.limited && !limited.cancelled);

        let calls = std::cell::Cell::new(0);
        let endless = std::io::BufReader::new(std::io::repeat(b'\n'));
        let cancelled = collect_listing_lines(endless, usize::MAX, || {
            calls.set(calls.get() + 1);
            calls.get() > 5
        })
        .unwrap();
        assert!(cancelled.cancelled);
        assert_eq!(cancelled.lines.len(), 5);

        let finite = collect_listing_lines(&b"a\nb c\r\n"[..], 10, || false).unwrap();
        assert_eq!(finite.lines, ["a", "b c\r"]);
        assert!(!finite.limited && !finite.cancelled);
    }

    #[test]
    fn tar_member_paths_must_be_plain_relative_names() {
        assert!(is_enclosed_tar_path(Path::new("dir/file.txt")));
        assert!(is_enclosed_tar_path(Path::new("dir/")));
        assert!(!is_enclosed_tar_path(Path::new("/etc/passwd")));
        assert!(!is_enclosed_tar_path(Path::new("a/../../b")));
    }

    #[cfg(unix)]
    #[test]
    fn tar_listing_rejects_absolute_members() {
        let temp = TempDir::new();
        let source = temp.path().join("abs.txt");
        fs::write(&source, b"abs").unwrap();
        fs::write(temp.path().join("rel.txt"), b"rel").unwrap();
        let archive = temp.path().join("abs.tgz");
        let status = std::process::Command::new("tar")
            .arg("-czPf")
            .arg(&archive)
            .arg(&source)
            .arg("-C")
            .arg(temp.path())
            .arg("rel.txt")
            .status()
            .unwrap();
        assert!(status.success());
        let cancelled = AtomicBool::new(false);

        let listing = list_archive(archive, &cancelled).unwrap();

        assert_eq!(listing.unsafe_members, 1);
        assert_eq!(listing.members.len(), 1);
        assert_eq!(listing.members[0].path, Path::new("rel.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn tar_extract_reports_unknown_member_indexes() {
        let temp = TempDir::new();
        let source = temp.path().join("src");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("a.txt"), b"a").unwrap();
        let archive = temp.path().join("one.tgz");
        write_tar_gz(&source, &archive, &["a.txt"]);
        let dest = temp.path().join("out");
        let cancelled = AtomicBool::new(false);

        let report = extract_members(&archive, &dest, &[0, 7], &cancelled).unwrap();

        assert_eq!(report.extracted, 1);
        assert_eq!(report.errors, ["Unknown archive member #7"]);
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"a");
    }

    #[test]
    fn chunked_copy_stops_when_cancelled_mid_member() {
        /// Sets the cancel flag after the first chunk, like a dialog closed
        /// while a large member is being written.
        struct CancelAfterFirst<'a>(&'a AtomicBool);
        impl Read for CancelAfterFirst<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.0.store(true, Ordering::Release);
                buf.fill(b'x');
                Ok(buf.len())
            }
        }
        let cancelled = AtomicBool::new(false);
        let mut out = Vec::new();

        let finished =
            copy_until_cancelled(&mut CancelAfterFirst(&cancelled), &mut out, &cancelled).unwrap();

        assert!(!finished);
        assert_eq!(out.len(), 256 * 1024, "copy kept reading after cancel");

        let idle = AtomicBool::new(false);
        let mut out = Vec::new();
        assert!(copy_until_cancelled(&mut &b"done"[..], &mut out, &idle).unwrap());
        assert_eq!(out, b"done");
    }

    #[test]
    fn zip_extract_names_unsafe_members() {
        let temp = TempDir::new();
        let archive = temp.path().join("unsafe.zip");
        write_zip(&archive, &[("../escape.txt", b"bad")]);
        let dest = temp.path().join("out");
        let cancelled = AtomicBool::new(false);

        let report = extract_members(&archive, &dest, &[0], &cancelled).unwrap();

        assert_eq!(report.errors, ["Skipped unsafe member ../escape.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn zip_extract_keeps_unix_mode_and_mtime() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new();
        let archive = temp.path().join("script.zip");
        let file = fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let stamp = zip::DateTime::from_date_and_time(2020, 3, 4, 5, 6, 8).unwrap();
        let options = zip::write::SimpleFileOptions::default()
            .unix_permissions(0o755)
            .last_modified_time(stamp);
        writer.start_file("run.sh", options).unwrap();
        writer.write_all(b"#!/bin/sh\n").unwrap();
        writer.finish().unwrap();
        let dest = temp.path().join("out");
        let cancelled = AtomicBool::new(false);

        let report = extract_members(&archive, &dest, &[0], &cancelled).unwrap();

        assert_eq!(report.extracted, 1);
        let meta = fs::metadata(dest.join("run.sh")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o755);
        assert_eq!(meta.modified().unwrap(), dos_time_to_system(stamp).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn child_wait_kills_process_on_cancel() {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let cancelled = AtomicBool::new(true);
        let started = std::time::Instant::now();

        let output = wait_child_or_cancel(child, &cancelled).unwrap();

        assert!(output.is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(10));

        let child = std::process::Command::new("sh")
            .args(["-c", "echo oops >&2; exit 3"])
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let output = wait_child_or_cancel(child, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stderr, b"oops\n");
    }

    /// Build `<dir>/<name>.tgz` from `names` under `source` with system tar,
    /// without a `./` prefix so members extract under their listed names.
    #[cfg(unix)]
    fn write_tar_gz(source: &Path, archive: &Path, names: &[&str]) {
        let status = std::process::Command::new("tar")
            .arg("-czf")
            .arg(archive)
            .arg("-C")
            .arg(source)
            .args(names)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn verbose_tar_lines_parse_gnu_and_bsd_layouts() {
        assert_eq!(
            parse_tar_verbose_line("-rw-r--r-- root/root         5 2026-10-14 18:17 ./hl"),
            Some(("./hl", 5))
        );
        assert_eq!(
            parse_tar_verbose_line("-rw-r--r-- u/g 1 2026-10-14 18:16  sp "),
            Some((" sp ", 1))
        );
        assert_eq!(
            parse_tar_verbose_line("lrwxrwxrwx root/root 0 2026-10-14 18:17 sl -> f"),
            Some(("sl", 0))
        );
        assert_eq!(
            parse_tar_verbose_line("hrw-r--r-- root/root 0 2026-10-14 18:17 f link to hl"),
            Some(("f", 0))
        );
        assert_eq!(
            parse_tar_verbose_line("-rw-r--r--  0 user   staff    1234 Oct  4  2024 dir/a b.txt"),
            Some(("dir/a b.txt", 1234))
        );
        assert_eq!(parse_tar_verbose_line("garbage"), None);
    }

    #[cfg(unix)]
    #[test]
    fn tar_listing_keeps_member_sizes_and_edge_spaces() {
        let temp = TempDir::new();
        let source = temp.path().join("src");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("hello.txt"), b"hello").unwrap();
        fs::write(source.join(" padded "), b"pad").unwrap();
        let archive = temp.path().join("pack.tgz");
        write_tar_gz(&source, &archive, &["hello.txt", " padded "]);
        let cancelled = AtomicBool::new(false);

        let listing = list_archive(archive.clone(), &cancelled).unwrap();

        let hello = listing
            .members
            .iter()
            .find(|member| member.path == Path::new("hello.txt"))
            .unwrap();
        assert_eq!(hello.size, 5);
        let padded = listing
            .members
            .iter()
            .find(|member| member.path == Path::new(" padded "))
            .expect("edge spaces are part of the member name");
        let indexes = [padded.index];
        let dest = temp.path().join("out");
        let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(fs::read(dest.join(" padded ")).unwrap(), b"pad");
    }

    #[cfg(unix)]
    #[test]
    fn tar_lists_and_extracts_option_like_member_names() {
        // Names that look like tar flags must stay member names, both in the
        // listing and in the extract name list.
        let temp = TempDir::new();
        let source = temp.path().join("src");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("-v"), b"dash").unwrap();
        fs::write(source.join("--null"), b"double").unwrap();
        let archive = temp.path().join("flags.tgz");
        let status = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .args(["--", "-v", "--null"])
            .status()
            .unwrap();
        assert!(status.success());
        let cancelled = AtomicBool::new(false);
        let listing = list_archive(archive.clone(), &cancelled).unwrap();
        let mut names: Vec<_> = listing
            .members
            .iter()
            .map(|member| member.path.clone())
            .collect();
        names.sort();
        assert_eq!(names, [PathBuf::from("--null"), PathBuf::from("-v")]);
        let indexes: Vec<usize> = listing.members.iter().map(|member| member.index).collect();
        let dest = temp.path().join("out");

        let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.extracted, 2);
        assert_eq!(fs::read(dest.join("-v")).unwrap(), b"dash");
        assert_eq!(fs::read(dest.join("--null")).unwrap(), b"double");
    }

    #[cfg(unix)]
    #[test]
    fn tar_extracts_glob_metacharacters_as_literal_member_names() {
        let temp = TempDir::new();
        let source = temp.path().join("src");
        fs::create_dir(&source).unwrap();
        for (name, content) in [
            ("a*b", b"star".as_slice()),
            ("axb", b"decoy".as_slice()),
            ("q?.txt", b"question".as_slice()),
            ("[x].txt", b"bracket".as_slice()),
        ] {
            fs::write(source.join(name), content).unwrap();
        }
        let archive = temp.path().join("patterns.tgz");
        write_tar_gz(&source, &archive, &["a*b", "axb", "q?.txt", "[x].txt"]);
        let cancelled = AtomicBool::new(false);
        let listing = list_archive(archive.clone(), &cancelled).unwrap();
        let indexes: Vec<_> = listing
            .members
            .iter()
            .filter(|member| matches!(member.path.to_str(), Some("a*b" | "q?.txt" | "[x].txt")))
            .map(|member| member.index)
            .collect();
        let dest = temp.path().join("out");

        let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.extracted, 3);
        assert_eq!(fs::read(dest.join("a*b")).unwrap(), b"star");
        assert_eq!(fs::read(dest.join("q?.txt")).unwrap(), b"question");
        assert_eq!(fs::read(dest.join("[x].txt")).unwrap(), b"bracket");
        assert!(
            !dest.join("axb").exists(),
            "glob matched an unrequested member"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_tar_extract_still_counts_the_members_it_wrote() {
        let temp = TempDir::new();
        let source = temp.path().join("src");
        fs::create_dir_all(source.join("sub")).unwrap();
        fs::write(source.join("a.txt"), b"a").unwrap();
        fs::write(source.join("sub/b.txt"), b"b").unwrap();
        let archive = temp.path().join("pack.tgz");
        write_tar_gz(&source, &archive, &["a.txt", "sub/b.txt"]);
        let dest = temp.path().join("out");
        fs::create_dir(&dest).unwrap();
        // A file where `sub/` must go makes tar fail for b.txt only.
        fs::write(dest.join("sub"), b"in the way").unwrap();
        let cancelled = AtomicBool::new(false);
        let listing = list_archive(archive.clone(), &cancelled).unwrap();
        let indexes = listing
            .members
            .iter()
            .filter(|member| !member.is_dir)
            .map(|member| member.index)
            .collect::<Vec<_>>();

        let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert_eq!(report.extracted, 1);
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"a");
    }

    #[cfg(unix)]
    #[test]
    fn tar_extract_handles_dot_slash_prefixed_members() {
        // `tar -czf x.tgz -C src .` stores every member as `./name`.
        let temp = TempDir::new();
        let source = temp.path().join("src");
        fs::create_dir_all(source.join("sub")).unwrap();
        fs::write(source.join("top.txt"), b"top").unwrap();
        fs::write(source.join("sub/inner.txt"), b"inner").unwrap();
        let archive = temp.path().join("dot.tgz");
        write_tar_gz(&source, &archive, &["."]);
        let cancelled = AtomicBool::new(false);
        let listing = list_archive(archive.clone(), &cancelled).unwrap();
        assert!(
            listing
                .members
                .iter()
                .any(|member| member.path == Path::new("sub/inner.txt"))
        );
        let indexes: Vec<usize> = listing.members.iter().map(|member| member.index).collect();
        let dest = temp.path().join("out");

        let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.extracted, 2);
        assert_eq!(fs::read(dest.join("top.txt")).unwrap(), b"top");
        assert_eq!(fs::read(dest.join("sub/inner.txt")).unwrap(), b"inner");
    }

    #[cfg(unix)]
    #[test]
    fn tar_extract_all_does_not_overflow_argument_list() {
        // ~2.2 MB of member paths: more than Linux's 2 MB ARG_MAX when the
        // names were passed as tar arguments. Each path stays well under
        // macOS's 1024-byte PATH_MAX.
        let temp = TempDir::new();
        let source = temp.path().join("src");
        let mut deep = PathBuf::new();
        for level in 0..3 {
            deep.push(format!("{level:02}{}", "d".repeat(200)));
        }
        fs::create_dir_all(source.join(&deep)).unwrap();
        for index in 0..3_600 {
            fs::write(source.join(&deep).join(format!("f{index}")), b"x").unwrap();
        }
        let archive = temp.path().join("many.tgz");
        let top = deep.iter().next().unwrap().to_str().unwrap().to_string();
        write_tar_gz(&source, &archive, &[&top]);
        let cancelled = AtomicBool::new(false);
        let listing = list_archive(archive.clone(), &cancelled).unwrap();
        let indexes: Vec<usize> = listing.members.iter().map(|member| member.index).collect();
        let dest = temp.path().join("out");

        let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.extracted, 3_600);
        assert!(dest.join(&deep).join("f3599").is_file());
    }
}
