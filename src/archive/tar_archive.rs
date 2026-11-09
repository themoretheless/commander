//! TAR streams: bounded decoding, member listing and extraction through the system tar.

use super::*;

/// Includes skipped payloads: sequential TAR scans must read past them to
/// reach later headers. A large/bomb stream produces an explicit partial listing.
pub(super) const MAX_TAR_STREAM_BYTES: u64 = 128 * 1024 * 1024;

pub(super) struct TarStream<'a, R> {
    pub(super) inner: R,
    pub(super) remaining: &'a std::cell::Cell<u64>,
    pub(super) limited: &'a std::cell::Cell<bool>,
    pub(super) cancelled: &'a dyn Fn() -> bool,
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

pub(super) fn visit_tar(
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
pub(super) struct ListingLines {
    pub(super) lines: Vec<String>,
    pub(super) cancelled: bool,
    /// `max_lines` was reached before EOF; the rest of the output was not read.
    pub(super) limited: bool,
}

#[cfg(test)]
pub(super) fn collect_listing_lines(
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
pub(super) fn parse_tar_verbose_line(line: &str) -> Option<(&str, u64)> {
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

pub(super) struct PreparedTar {
    pub(super) path: PathBuf,
}
impl Drop for PreparedTar {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Stage only decoder output, never extracted member paths. Keeping external tar
/// on raw TAR avoids depending on the host's optional compression libraries.
pub(super) fn prepare_tar(
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

pub(super) fn extract_tar_members(
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
pub(super) fn wait_child_or_cancel(
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
