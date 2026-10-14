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
                return Ok(VisitSummary::default());
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
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(error) => {
                report
                    .errors
                    .push(format!("Could not open member #{index}: {error}"));
                continue;
            }
        };
        let Some(enclosed) = entry.enclosed_name() else {
            report
                .errors
                .push(format!("Skipped unsafe member #{index}"));
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
            Ok(mut out) => match std::io::copy(&mut entry, &mut out) {
                Ok(_) => report.extracted += 1,
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
        if dest_dir.join(&member.path).exists() {
            report.skipped_existing += 1;
            continue;
        }
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
    // Member names go through stdin (`--null -T -`), not argv: "Extract all"
    // on a large archive would otherwise exceed ARG_MAX and tar never runs.
    let mut child = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive_path)
        .arg("-C")
        .arg(dest_dir)
        .args(["--null", "-T", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not extract {}: {error}", archive_path.display()))?;
    let mut names = Vec::new();
    for member in &wanted {
        names.extend_from_slice(member.as_bytes());
        names.push(0);
    }
    // Feed stdin from a helper thread so a chatty tar cannot deadlock on a
    // full stderr pipe while we are still writing names.
    let stdin = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = std::io::Write::write_all(&mut stdin, &names);
        }
    });
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Could not extract {}: {error}", archive_path.display()))?;
    let _ = writer.join();
    if output.status.success() {
        report.extracted += wanted.len();
    } else {
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
        // ~3 MB of member paths: more than Linux's 2 MB ARG_MAX when the
        // names were passed as tar arguments.
        let temp = TempDir::new();
        let source = temp.path().join("src");
        let mut deep = PathBuf::new();
        for level in 0..12 {
            deep.push(format!("{level:02}{}", "d".repeat(200)));
        }
        fs::create_dir_all(source.join(&deep)).unwrap();
        for index in 0..1_200 {
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
        assert_eq!(report.extracted, 1_200);
        assert!(dest.join(&deep).join("f1199").is_file());
    }
}
