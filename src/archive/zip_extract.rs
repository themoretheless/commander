//! ZIP member extraction with cancellation, symlink containment and metadata restore.

use super::*;

/// Keep cancellation responsive during encrypted authentication tails too,
/// where a codec has already reached EOS and the outer copy loop cannot poll.
pub(super) struct ZipExtractionInput<'a, R> {
    pub(super) reader: R,
    pub(super) cancelled: &'a AtomicBool,
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

pub(super) fn extract_zip_members(
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
pub(super) fn copy_until_cancelled(
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

pub(super) fn dos_time_to_system(time: zip::DateTime) -> Option<std::time::SystemTime> {
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
