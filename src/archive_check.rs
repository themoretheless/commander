//! Read-only archive stream checks, distinct from extraction and content search.
use crate::archive::{ArchiveKind, MAX_MEMBERS_PER_RUN};
use sha2::Digest;
use std::{
    fs,
    io::{Read, Seek},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
};
const MAX_DECODED: u64 = 128 * 1024 * 1024;

#[derive(Debug)]
pub struct Report {
    pub kind: ArchiveKind,
    pub members: usize,
    pub decoded: u64,
    pub strict_headers: bool,
    pub content_groups: Option<Vec<ContentGroup>>,
}
#[derive(Debug, serde::Serialize)]
pub struct ContentMember {
    pub index: usize,
    pub path: std::path::PathBuf,
    pub size: u64,
}
#[derive(Debug, serde::Serialize)]
pub struct ContentGroup {
    pub sha256: String,
    pub size: u64,
    pub members: Vec<ContentMember>,
}
#[derive(Clone, Copy)]
struct ScanOptions {
    limit: u64,
    max_members: usize,
    strict_headers: bool,
    content_groups: bool,
    metadata_limit: usize,
}
impl Report {
    pub fn summary(&self) -> String {
        let groups = self
            .content_groups
            .as_ref()
            .map(|groups| {
                format!(
                    " · {} SHA-256 groups ({} members)",
                    groups.len(),
                    groups.iter().map(|g| g.members.len()).sum::<usize>()
                )
            })
            .unwrap_or_default();
        format!(
            "Last {} check completed · {} members · {} decoded bytes · {}{}{}",
            self.kind.label(),
            self.members,
            self.decoded,
            if self.kind == ArchiveKind::Zip {
                "ZIP CRC/size checks; AES authentication when present"
            } else {
                "TAR headers/stream; TAR has no payload checksum"
            },
            if self.strict_headers {
                " · strict ZIP headers"
            } else {
                ""
            },
            groups
        )
    }
}
#[derive(PartialEq, Eq)]
struct Stamp {
    identity: crate::path_identity::PathIdentity,
    #[cfg(unix)]
    changed: (i64, i64),
}
impl Stamp {
    fn new(path: &Path, meta: &fs::Metadata) -> Self {
        Self {
            identity: crate::path_identity::PathIdentity::from_metadata(path.into(), meta),
            #[cfg(unix)]
            changed: {
                use std::os::unix::fs::MetadataExt;
                (meta.ctime(), meta.ctime_nsec())
            },
        }
    }
}
fn cancel(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("Archive check cancelled; no complete result.".into())
    } else {
        Ok(())
    }
}
struct ZipInput<'a> {
    file: fs::File,
    cancelled: &'a dyn Fn() -> bool,
}
impl Read for ZipInput<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        cancel(self.cancelled).map_err(std::io::Error::other)?;
        let length = bytes.len().min(64 * 1024);
        self.file.read(&mut bytes[..length])
    }
}
impl Seek for ZipInput<'_> {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        cancel(self.cancelled).map_err(std::io::Error::other)?;
        self.file.seek(position)
    }
}

struct BudgetReader<'a, R> {
    inner: R,
    cancelled: &'a dyn Fn() -> bool,
    progress: &'a AtomicU64,
    limit: u64,
}
impl<R: Read> Read for BudgetReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        cancel(self.cancelled).map_err(std::io::Error::other)?;
        let remaining = self
            .limit
            .saturating_sub(self.progress.load(Ordering::Relaxed));
        if remaining == 0 {
            let count = self.inner.read(&mut [0; 1])?;
            self.progress.fetch_add(count as u64, Ordering::Relaxed);
            return if count == 0 {
                Ok(0)
            } else {
                Err(std::io::Error::other(
                    "Archive check incomplete: 128 MiB decoded-byte limit reached.",
                ))
            };
        }
        let len = bytes.len().min(64 * 1024).min(remaining as usize);
        let count = self.inner.read(&mut bytes[..len])?;
        self.progress.fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }
}
fn drain(reader: &mut impl Read) -> Result<u64, String> {
    drain_hash(reader, None)
}
fn drain_hash(reader: &mut impl Read, mut hash: Option<&mut sha2::Sha256>) -> Result<u64, String> {
    let mut count = 0_u64;
    let mut bytes = [0; 64 * 1024];
    loop {
        match reader.read(&mut bytes) {
            Ok(0) => return Ok(count),
            Ok(length) => {
                count += length as u64;
                if let Some(hash) = hash.as_mut() {
                    hash.update(&bytes[..length]);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

pub fn check(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
) -> Result<Report, String> {
    check_with_limits(path, cancelled, progress, MAX_DECODED, MAX_MEMBERS_PER_RUN)
}
/// The borrowed password is used only during this check and never appears in
/// reports. None preserves the password-free read-only checking path.
pub fn check_with_password(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
    password: Option<&[u8]>,
) -> Result<Report, String> {
    check_with_password_limits(
        path,
        cancelled,
        progress,
        password,
        MAX_DECODED,
        MAX_MEMBERS_PER_RUN,
    )
}
/// Check ZIP header identity and payloads; non-ZIP archives keep their normal checks.
pub fn check_strict(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
    password: Option<&[u8]>,
) -> Result<Report, String> {
    check_options(
        path,
        cancelled,
        progress,
        password,
        MAX_DECODED,
        MAX_MEMBERS_PER_RUN,
        true,
    )
}
fn check_with_limits(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
    limit: u64,
    max_members: usize,
) -> Result<Report, String> {
    check_with_password_limits(path, cancelled, progress, None, limit, max_members)
}
fn check_with_password_limits(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
    password: Option<&[u8]>,
    limit: u64,
    max_members: usize,
) -> Result<Report, String> {
    check_options(
        path,
        cancelled,
        progress,
        password,
        limit,
        max_members,
        false,
    )
}
fn check_options(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
    password: Option<&[u8]>,
    limit: u64,
    max_members: usize,
    strict_headers: bool,
) -> Result<Report, String> {
    scan(
        path,
        cancelled,
        progress,
        password,
        ScanOptions {
            limit,
            max_members,
            strict_headers,
            content_groups: false,
            metadata_limit: 8 * 1024 * 1024,
        },
    )
}
fn scan(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
    password: Option<&[u8]>,
    options: ScanOptions,
) -> Result<Report, String> {
    let ScanOptions {
        limit,
        max_members,
        strict_headers,
        content_groups,
        metadata_limit,
    } = options;
    // zip-rs coalesces duplicate names: strict identity validation is required
    // before claiming a complete content scan.
    let strict_headers = strict_headers || content_groups;
    cancel(cancelled)?;
    progress.store(0, Ordering::Relaxed);
    let kind = crate::archive::kind_of(path).ok_or("Unsupported archive format.")?;
    let mut hashes = std::collections::BTreeMap::<(u64, [u8; 32]), Vec<ContentMember>>::new();
    let mut metadata_bytes = 0usize;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Archive check accepts regular files without following symlinks.".into());
    }
    let stamp = Stamp::new(path, &meta);
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if Stamp::new(path, &file.metadata().map_err(|e| e.to_string())?) != stamp {
        return Err("Archive changed; retry the check.".into());
    }
    let reader = file.try_clone().map_err(|e| e.to_string())?;
    let mut members = 0;
    if kind == ArchiveKind::Zip {
        let mut zip = zip::ZipArchive::new(ZipInput {
            file: reader,
            cancelled,
        })
        .map_err(|e| e.to_string())?;
        if zip.len() > max_members {
            return Err("Archive check incomplete: member-count limit reached.".into());
        }
        if zip.has_overlapping_files().map_err(|e| e.to_string())? {
            return Err("ZIP contains overlapping file data.".into());
        }
        if strict_headers {
            zip.validate_header_consistency()
                .map_err(|e| format!("Strict ZIP header check: {e}"))?;
        }
        for index in 0..zip.len() {
            cancel(cancelled)?;
            let candidate = if content_groups {
                let raw = zip
                    .by_index_raw(index)
                    .map_err(|e| format!("Member #{index}: {e}"))?;
                if !raw.is_dir()
                    && !raw.is_symlink()
                    && raw
                        .unix_mode()
                        .is_none_or(|mode| matches!(mode & 0o170000, 0 | 0o100000))
                {
                    if let Some(path) = raw.enclosed_name() {
                        metadata_bytes = metadata_bytes
                            .saturating_add(path.as_os_str().as_encoded_bytes().len())
                            .saturating_add(128);
                        if metadata_bytes > metadata_limit {
                            return Err(
                                "Content-group metadata exceeds 8 MiB; no complete result.".into(),
                            );
                        }
                        Some(ContentMember {
                            index,
                            path,
                            size: raw.size(),
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            let mut entry = crate::zip_reader::open_with_password(&mut zip, index, password)
                .map_err(|e| format!("Member #{index}: {e}"))?;
            if entry.size() > limit.saturating_sub(progress.load(Ordering::Relaxed)) {
                return Err("Archive check incomplete: 128 MiB decoded-byte limit reached.".into());
            }
            let mut bounded = BudgetReader {
                inner: &mut entry,
                cancelled,
                progress,
                limit,
            };
            let mut hash = candidate.as_ref().map(|_| sha2::Sha256::new());
            drain_hash(&mut bounded, hash.as_mut()).map_err(|e| format!("Member #{index}: {e}"))?;
            if let Some(candidate) = candidate {
                hashes
                    .entry((
                        candidate.size,
                        hash.expect("candidate has a hasher").finalize().into(),
                    ))
                    .or_default()
                    .push(candidate);
            }
            members += 1;
        }
    } else {
        let reader = crate::archive::tar_check_reader(reader, kind, cancelled)?;
        let reader = BudgetReader {
            inner: reader,
            cancelled,
            progress,
            limit,
        };
        let mut tar = tar::Archive::new(reader);
        tar.set_ignore_zeros(true);
        let payload = AtomicU64::new(0);
        {
            let entries = tar.entries().map_err(|e| e.to_string())?;
            for entry in entries {
                cancel(cancelled)?;
                if members >= max_members {
                    return Err("Archive check incomplete: member-count limit reached.".into());
                }
                let mut entry = entry.map_err(|e| e.to_string())?;
                let expected = entry.size();
                if expected > limit.saturating_sub(payload.load(Ordering::Relaxed)) {
                    return Err(
                        "Archive check incomplete: TAR logical-payload limit reached.".into(),
                    );
                }
                let candidate = if content_groups && entry.header().entry_type().is_file() {
                    let name = entry.path_bytes();
                    let name = name.strip_prefix(b"./").unwrap_or(&name);
                    crate::archive::tar_path_from_bytes(name)
                        .filter(|path| {
                            !name.is_empty()
                                && name != b"."
                                && crate::archive::is_enclosed_tar_path(path)
                        })
                        .map(|path| ContentMember {
                            index: members,
                            path,
                            size: expected,
                        })
                } else {
                    None
                };
                if let Some(candidate) = &candidate {
                    metadata_bytes = metadata_bytes
                        .saturating_add(candidate.path.as_os_str().as_encoded_bytes().len())
                        .saturating_add(128);
                    if metadata_bytes > metadata_limit {
                        return Err(
                            "Content-group metadata exceeds 8 MiB; no complete result.".into()
                        );
                    }
                }
                let mut hash = candidate.as_ref().map(|_| sha2::Sha256::new());
                let count = drain_hash(
                    &mut BudgetReader {
                        inner: &mut entry,
                        cancelled,
                        progress: &payload,
                        limit,
                    },
                    hash.as_mut(),
                )
                .map_err(|e| format!("Member #{members}: {e}"))?;
                if count != expected {
                    return Err(format!("Member #{members}: truncated TAR payload"));
                }
                if let Some(candidate) = candidate {
                    hashes
                        .entry((
                            candidate.size,
                            hash.expect("candidate has a hasher").finalize().into(),
                        ))
                        .or_default()
                        .push(candidate);
                }
                members += 1;
            }
        }
        // tar::Archive stops at its logical end marker. Drain the decoder too:
        // gzip/XZ/zstd trailers and later compression frames must be checked.
        drain(&mut tar.into_inner())?;
    }
    cancel(cancelled)?;
    if kind != ArchiveKind::Zip && progress.load(Ordering::Relaxed) == 0 {
        return Err("Empty stream does not contain a TAR archive.".into());
    }
    let after = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if Stamp::new(path, &after) != stamp
        || Stamp::new(path, &file.metadata().map_err(|e| e.to_string())?) != stamp
    {
        return Err("Archive changed during its check; no complete result.".into());
    }
    Ok(Report {
        kind,
        members,
        decoded: progress.load(Ordering::Relaxed),
        strict_headers: strict_headers && kind == ArchiveKind::Zip,
        content_groups: content_groups.then(|| {
            hashes
                .into_iter()
                .filter_map(|((size, sha256), members)| {
                    (members.len() > 1).then_some(ContentGroup {
                        sha256: sha256.iter().map(|b| format!("{b:02x}")).collect(),
                        size,
                        members,
                    })
                })
                .collect()
        }),
    })
}

pub struct Run {
    receiver: Receiver<Result<Report, String>>,
    cancelled: Arc<AtomicBool>,
    pub progress: Arc<AtomicU64>,
}
impl Run {
    pub fn try_recv(&self) -> Result<Result<Report, String>, TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for Run {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
pub fn start(
    path: std::path::PathBuf,
    password: Option<zeroize::Zeroizing<String>>,
    strict_headers: bool,
    notify: crate::archive::Notify,
) -> Run {
    start_mode(path, password, strict_headers, false, notify)
}
pub fn start_content_groups(
    path: std::path::PathBuf,
    password: Option<zeroize::Zeroizing<String>>,
    strict_headers: bool,
    notify: crate::archive::Notify,
) -> Run {
    start_mode(path, password, strict_headers, true, notify)
}
fn start_mode(
    path: std::path::PathBuf,
    password: Option<zeroize::Zeroizing<String>>,
    strict_headers: bool,
    content_groups: bool,
    notify: crate::archive::Notify,
) -> Run {
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let progress = Arc::new(AtomicU64::new(0));
    let worker_progress = Arc::clone(&progress);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let cancelled = || flag.load(Ordering::Acquire);
        let result = if content_groups {
            scan(
                &path,
                &cancelled,
                &worker_progress,
                password.as_ref().map(|p| p.as_bytes()),
                ScanOptions {
                    limit: MAX_DECODED,
                    max_members: MAX_MEMBERS_PER_RUN,
                    strict_headers,
                    content_groups: true,
                    metadata_limit: 8 * 1024 * 1024,
                },
            )
        } else if strict_headers {
            check_strict(
                &path,
                &cancelled,
                &worker_progress,
                password.as_ref().map(|p| p.as_bytes()),
            )
        } else if let Some(password) = &password {
            check_with_password(
                &path,
                &cancelled,
                &worker_progress,
                Some(password.as_bytes()),
            )
        } else {
            check(&path, &cancelled, &worker_progress)
        };
        drop(password);
        let _ = sender.send(result);
        notify();
    });
    Run {
        receiver,
        cancelled,
        progress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::io::Write;
    fn group_options() -> ScanOptions {
        ScanOptions {
            limit: MAX_DECODED,
            max_members: MAX_MEMBERS_PER_RUN,
            strict_headers: false,
            content_groups: true,
            metadata_limit: 8 * 1024 * 1024,
        }
    }
    #[test]
    fn content_groups_hash_real_bytes_not_crc_collisions_and_keep_entry_ids() {
        let temp = TempDir::new();
        let path = temp.path().join("groups.zip");
        assert_eq!(crc32fast::hash(b"plumless"), crc32fast::hash(b"buckeroo"));
        let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        for (name, body) in [
            ("same-a", b"abc".as_slice()),
            ("same-b", b"abc".as_slice()),
            ("empty-a", b"".as_slice()),
            ("empty-b", b"".as_slice()),
            ("collision-a", b"plumless".as_slice()),
            ("collision-b", b"buckeroo".as_slice()),
            ("../unsafe", b"abc".as_slice()),
        ] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body).unwrap();
        }
        writer
            .add_directory("folder/", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer
            .add_symlink("link", "abc", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.finish().unwrap();
        let before = fs::read(&path).unwrap();
        let report = scan(&path, &|| false, &AtomicU64::new(0), None, group_options()).unwrap();
        let groups = report.content_groups.unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].size, 0);
        assert_eq!(
            groups[0]
                .members
                .iter()
                .map(|m| m.index)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(groups[1].size, 3);
        assert_eq!(
            groups[1].sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            groups[1]
                .members
                .iter()
                .map(|m| m.index)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        let mut bytes = fs::read(&path).unwrap();
        for offset in 0..bytes.len().saturating_sub(46) {
            let name = if bytes[offset..].starts_with(b"PK\x03\x04") {
                Some(offset + 30)
            } else if bytes[offset..].starts_with(b"PK\x01\x02") {
                Some(offset + 46)
            } else {
                None
            };
            if let Some(name) = name
                && bytes[name..].starts_with(b"same-b")
            {
                bytes[name..name + 6].copy_from_slice(b"same-a");
            }
        }
        fs::write(&path, bytes).unwrap();
        let error = scan(&path, &|| false, &AtomicU64::new(0), None, group_options()).unwrap_err();
        assert!(error.contains("duplicate"), "{error}");
    }
    #[test]
    fn content_group_metadata_budget_charges_decoded_filename_bytes() {
        let temp = TempDir::new();
        let path = temp.path().join("name.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        writer
            .start_file("a", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.finish().unwrap();
        let mut bytes = fs::read(&path).unwrap();
        for offset in 0..bytes.len().saturating_sub(46) {
            let name = if bytes[offset..].starts_with(b"PK\x03\x04") {
                Some(offset + 30)
            } else if bytes[offset..].starts_with(b"PK\x01\x02") {
                Some(offset + 46)
            } else {
                None
            };
            if let Some(name) = name {
                bytes[name] = 0x82;
            }
        }
        fs::write(&path, bytes).unwrap();
        assert!(
            scan(
                &path,
                &|| false,
                &AtomicU64::new(0),
                None,
                ScanOptions {
                    metadata_limit: 129,
                    ..group_options()
                }
            )
            .is_err()
        );
        assert!(
            scan(
                &path,
                &|| false,
                &AtomicU64::new(0),
                None,
                ScanOptions {
                    metadata_limit: 130,
                    ..group_options()
                }
            )
            .is_ok()
        );
    }

    fn encrypted_group_fixture(path: &Path, corrupt: bool) {
        let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let body = include_bytes!(
            "../docs/qualification/zip-password-2026-11-03/fixtures/aes-utf8-password.txt"
        );
        writer
            .start_file("plain.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(body).unwrap();
        let bytes: &[u8] = if corrupt {
            include_bytes!(
                "../docs/qualification/zip-password-2026-11-03/fixtures/aes-ppmd-bad-tag.zipx"
            )
        } else {
            include_bytes!(
                "../docs/qualification/zip-password-2026-11-03/fixtures/aes-utf8-password.zipx"
            )
        };
        let mut encrypted = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        for name in ["enc-one.txt", "enc-two.txt"] {
            writer
                .raw_copy_file_rename(encrypted.by_index_raw(0).unwrap(), name)
                .unwrap();
        }
        writer.finish().unwrap();
    }
    #[test]
    fn content_groups_support_passwords_and_fail_closed_on_auth_limits_and_cancel() {
        let temp = TempDir::new();
        let path = temp.path().join("groups.zipx");
        encrypted_group_fixture(&path, false);
        for password in [None, Some(b"wrong".as_slice())] {
            assert!(
                scan(
                    &path,
                    &|| false,
                    &AtomicU64::new(0),
                    password,
                    group_options()
                )
                .is_err()
            );
        }
        let password = Some("fixture-пароль-ёж".as_bytes());
        let report = scan(
            &path,
            &|| false,
            &AtomicU64::new(0),
            password,
            group_options(),
        )
        .unwrap();
        assert_eq!(report.content_groups.as_ref().unwrap()[0].members.len(), 3);
        for options in [
            ScanOptions {
                limit: 5000,
                ..group_options()
            },
            ScanOptions {
                max_members: 2,
                ..group_options()
            },
            ScanOptions {
                metadata_limit: 1,
                ..group_options()
            },
        ] {
            assert!(scan(&path, &|| false, &AtomicU64::new(0), password, options).is_err());
        }
        assert!(
            scan(
                &path,
                &|| true,
                &AtomicU64::new(0),
                password,
                group_options()
            )
            .is_err()
        );
        let run = start_content_groups(
            path.clone(),
            Some(zeroize::Zeroizing::new("fixture-пароль-ёж".into())),
            false,
            Arc::new(|| {}),
        );
        let report = run
            .receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(report.content_groups.unwrap()[0].members.len(), 3);
        encrypted_group_fixture(&path, true);
        assert!(
            scan(
                &path,
                &|| false,
                &AtomicU64::new(0),
                Some(b"fixture-password"),
                group_options()
            )
            .is_err()
        );
    }
    #[test]
    fn content_groups_refuse_source_mutation_before_publishing_groups() {
        let temp = TempDir::new();
        let path = temp.path().join("groups.zipx");
        encrypted_group_fixture(&path, false);
        let progress = AtomicU64::new(0);
        let changed = std::cell::Cell::new(false);
        let cancelled = || {
            if progress.load(Ordering::Relaxed) > 0 && !changed.replace(true) {
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(0);
                fs::write(&path, bytes).unwrap();
            }
            false
        };
        assert!(
            scan(
                &path,
                &cancelled,
                &progress,
                Some("fixture-пароль-ёж".as_bytes()),
                group_options()
            )
            .is_err()
        );
        assert!(changed.get());
    }
    macro_rules! fixture {
        ($name:literal, $extension:literal) => {
            include_bytes!(concat!(
                "../docs/qualification/zip-password-2026-11-03/fixtures/",
                $name,
                $extension
            ))
            .as_slice()
        };
    }
    #[test]
    fn strict_zip_headers_keep_worker_limits_cancellation_and_duplicate_policy() {
        let temp = TempDir::new();
        let path = temp.path().join("input.zipx");
        fs::write(&path, fixture!("aes-utf8-password", ".zipx")).unwrap();
        let run = start(
            path.clone(),
            Some(zeroize::Zeroizing::new("fixture-пароль-ёж".into())),
            true,
            Arc::new(|| {}),
        );
        let report = run
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(report.strict_headers);
        let progress = AtomicU64::new(0);
        let error = check_options(
            &path,
            &|| false,
            &progress,
            Some("fixture-пароль-ёж".as_bytes()),
            1,
            100,
            true,
        )
        .unwrap_err();
        assert!(error.contains("limit"));
        assert!(
            check_strict(&path, &|| true, &progress, None)
                .unwrap_err()
                .contains("cancelled")
        );
        fs::write(
            &path,
            include_bytes!(
                "../docs/qualification/zip-header-check-2026-11-03/fixtures/duplicates.zip"
            ),
        )
        .unwrap();
        let normal = check(&path, &|| false, &progress).unwrap();
        assert!(!normal.strict_headers);
        let error = check_strict(&path, &|| false, &progress, None).unwrap_err();
        assert!(error.contains("duplicate"), "{error}");
        assert_eq!(progress.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn strict_zip_headers_reject_inconsistency_before_payload_reads() {
        for (name, bytes) in [
            (
                "name",
                include_bytes!(
                    "../docs/qualification/zip-header-check-2026-11-03/fixtures/name.zip"
                )
                .as_slice(),
            ),
            (
                "method",
                include_bytes!(
                    "../docs/qualification/zip-header-check-2026-11-03/fixtures/method.zip"
                )
                .as_slice(),
            ),
            (
                "encrypted",
                include_bytes!(
                    "../docs/qualification/zip-header-check-2026-11-03/fixtures/encrypted.zip"
                )
                .as_slice(),
            ),
            (
                "descriptor",
                include_bytes!(
                    "../docs/qualification/zip-header-check-2026-11-03/fixtures/descriptor.zip"
                )
                .as_slice(),
            ),
            (
                "timestamp",
                include_bytes!(
                    "../docs/qualification/zip-header-check-2026-11-03/fixtures/timestamp.zip"
                )
                .as_slice(),
            ),
            (
                "crc",
                include_bytes!(
                    "../docs/qualification/zip-header-check-2026-11-03/fixtures/crc.zip"
                )
                .as_slice(),
            ),
            (
                "packed-size",
                include_bytes!(
                    "../docs/qualification/zip-header-check-2026-11-03/fixtures/packed-size.zip"
                )
                .as_slice(),
            ),
            (
                "size",
                include_bytes!(
                    "../docs/qualification/zip-header-check-2026-11-03/fixtures/size.zip"
                )
                .as_slice(),
            ),
        ] {
            let temp = TempDir::new();
            let path = temp.path().join("mutant.zip");
            fs::write(&path, bytes).unwrap();
            let progress = AtomicU64::new(0);
            let normal =
                check(&path, &|| false, &progress).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!normal.strict_headers);
            let error = check_strict(&path, &|| false, &progress, None).unwrap_err();
            assert!(error.contains("header"), "{name}: {error}");
            assert_eq!(progress.load(Ordering::Relaxed), 0, "{name}");
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn strict_zip_headers_accept_valid_encryption_and_descriptors() {
        for bytes in [
            include_bytes!("../docs/qualification/zip-header-check-2026-11-03/fixtures/normal.zip")
                .as_slice(),
            fixture!("lzma-descriptor", ".zipx"),
            fixture!("aes-descriptor", ".zipx"),
            fixture!("aes-ppmd-3-v2", ".zipx"),
        ] {
            let temp = TempDir::new();
            let path = temp.path().join("valid.zipx");
            fs::write(&path, bytes).unwrap();
            let report = check_strict(
                &path,
                &|| false,
                &AtomicU64::new(0),
                Some(b"fixture-password"),
            )
            .unwrap();
            assert!(report.strict_headers);
            assert!(report.summary().contains("strict ZIP headers"));
        }
    }
    #[test]
    fn password_integrity_requires_every_member_and_aggregate_budget() {
        let temp = TempDir::new();
        let path = temp.path().join("mixed.zipx");
        let progress = AtomicU64::new(0);
        fs::write(
            &path,
            include_bytes!(
                "../docs/qualification/archive-password-check-2026-11-03/fixtures/mixed.zipx"
            ),
        )
        .unwrap();
        let report =
            check_with_password(&path, &|| false, &progress, Some(b"fixture-password")).unwrap();
        let first = fixture!("expected", ".txt").len() as u64;
        let total = first + fixture!("aes-ppmd-3-v2", ".txt").len() as u64;
        assert_eq!(report.members, 2);
        assert_eq!(report.decoded, total);
        let error = check_with_password_limits(
            &path,
            &|| false,
            &progress,
            Some(b"fixture-password"),
            first + 1,
            100,
        )
        .unwrap_err();
        assert!(error.contains("limit"));
        assert_eq!(progress.load(Ordering::Relaxed), first);
        fs::write(
            &path,
            include_bytes!(
                "../docs/qualification/archive-password-check-2026-11-03/fixtures/late-failure.zipx"
            ),
        )
        .unwrap();
        let error = check_with_password(&path, &|| false, &progress, Some(b"fixture-password"))
            .unwrap_err();
        assert!(
            error.contains("Member #1") && error.contains("authentication"),
            "{error}"
        );
        assert!(progress.load(Ordering::Relaxed) >= first);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn password_integrity_checks_all_codecs_strengths_and_versions_without_writes() {
        for (name, bytes, expected) in [
            (
                "stored",
                fixture!("stored", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-stored-128",
                fixture!("aes-stored-128", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-stored-128-v1",
                fixture!("aes-stored-128-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-stored-192",
                fixture!("aes-stored-192", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-stored-192-v1",
                fixture!("aes-stored-192-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-stored-256",
                fixture!("aes-stored-256", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-stored-256-v1",
                fixture!("aes-stored-256-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "deflate",
                fixture!("deflate", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate-128",
                fixture!("aes-deflate-128", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate-128-v1",
                fixture!("aes-deflate-128-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate-192",
                fixture!("aes-deflate-192", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate-192-v1",
                fixture!("aes-deflate-192-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate-256",
                fixture!("aes-deflate-256", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate-256-v1",
                fixture!("aes-deflate-256-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "bzip2",
                fixture!("bzip2", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-bzip2-128",
                fixture!("aes-bzip2-128", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-bzip2-128-v1",
                fixture!("aes-bzip2-128-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-bzip2-192",
                fixture!("aes-bzip2-192", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-bzip2-192-v1",
                fixture!("aes-bzip2-192-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-bzip2-256",
                fixture!("aes-bzip2-256", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-bzip2-256-v1",
                fixture!("aes-bzip2-256-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "deflate64",
                fixture!("deflate64", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate64-128",
                fixture!("aes-deflate64-128", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate64-128-v1",
                fixture!("aes-deflate64-128-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate64-192",
                fixture!("aes-deflate64-192", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate64-192-v1",
                fixture!("aes-deflate64-192-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate64-256",
                fixture!("aes-deflate64-256", ".zip"),
                fixture!("expected", ".txt"),
            ),
            (
                "aes-deflate64-256-v1",
                fixture!("aes-deflate64-256-v1", ".zip"),
                fixture!("expected", ".txt"),
            ),
            ("lzma", fixture!("lzma", ".zipx"), fixture!("lzma", ".txt")),
            ("xz", fixture!("xz", ".zipx"), fixture!("xz", ".txt")),
            ("zstd", fixture!("zstd", ".zipx"), fixture!("zstd", ".txt")),
            (
                "zstd-legacy",
                fixture!("zstd-legacy", ".zipx"),
                fixture!("zstd-legacy", ".txt"),
            ),
            ("ppmd", fixture!("ppmd", ".zipx"), fixture!("ppmd", ".txt")),
            (
                "aes-lzma-1-v1",
                fixture!("aes-lzma-1-v1", ".zipx"),
                fixture!("aes-lzma-1-v1", ".txt"),
            ),
            (
                "aes-lzma-1-v2",
                fixture!("aes-lzma-1-v2", ".zipx"),
                fixture!("aes-lzma-1-v2", ".txt"),
            ),
            (
                "aes-lzma-2-v1",
                fixture!("aes-lzma-2-v1", ".zipx"),
                fixture!("aes-lzma-2-v1", ".txt"),
            ),
            (
                "aes-lzma-2-v2",
                fixture!("aes-lzma-2-v2", ".zipx"),
                fixture!("aes-lzma-2-v2", ".txt"),
            ),
            (
                "aes-lzma-3-v1",
                fixture!("aes-lzma-3-v1", ".zipx"),
                fixture!("aes-lzma-3-v1", ".txt"),
            ),
            (
                "aes-lzma-3-v2",
                fixture!("aes-lzma-3-v2", ".zipx"),
                fixture!("aes-lzma-3-v2", ".txt"),
            ),
            (
                "aes-xz-1-v1",
                fixture!("aes-xz-1-v1", ".zipx"),
                fixture!("aes-xz-1-v1", ".txt"),
            ),
            (
                "aes-xz-1-v2",
                fixture!("aes-xz-1-v2", ".zipx"),
                fixture!("aes-xz-1-v2", ".txt"),
            ),
            (
                "aes-xz-2-v1",
                fixture!("aes-xz-2-v1", ".zipx"),
                fixture!("aes-xz-2-v1", ".txt"),
            ),
            (
                "aes-xz-2-v2",
                fixture!("aes-xz-2-v2", ".zipx"),
                fixture!("aes-xz-2-v2", ".txt"),
            ),
            (
                "aes-xz-3-v1",
                fixture!("aes-xz-3-v1", ".zipx"),
                fixture!("aes-xz-3-v1", ".txt"),
            ),
            (
                "aes-xz-3-v2",
                fixture!("aes-xz-3-v2", ".zipx"),
                fixture!("aes-xz-3-v2", ".txt"),
            ),
            (
                "aes-zstd-1-v1",
                fixture!("aes-zstd-1-v1", ".zipx"),
                fixture!("aes-zstd-1-v1", ".txt"),
            ),
            (
                "aes-zstd-1-v2",
                fixture!("aes-zstd-1-v2", ".zipx"),
                fixture!("aes-zstd-1-v2", ".txt"),
            ),
            (
                "aes-zstd-2-v1",
                fixture!("aes-zstd-2-v1", ".zipx"),
                fixture!("aes-zstd-2-v1", ".txt"),
            ),
            (
                "aes-zstd-2-v2",
                fixture!("aes-zstd-2-v2", ".zipx"),
                fixture!("aes-zstd-2-v2", ".txt"),
            ),
            (
                "aes-zstd-3-v1",
                fixture!("aes-zstd-3-v1", ".zipx"),
                fixture!("aes-zstd-3-v1", ".txt"),
            ),
            (
                "aes-zstd-3-v2",
                fixture!("aes-zstd-3-v2", ".zipx"),
                fixture!("aes-zstd-3-v2", ".txt"),
            ),
            (
                "aes-legacy-1-v1",
                fixture!("aes-legacy-1-v1", ".zipx"),
                fixture!("aes-legacy-1-v1", ".txt"),
            ),
            (
                "aes-legacy-1-v2",
                fixture!("aes-legacy-1-v2", ".zipx"),
                fixture!("aes-legacy-1-v2", ".txt"),
            ),
            (
                "aes-legacy-2-v1",
                fixture!("aes-legacy-2-v1", ".zipx"),
                fixture!("aes-legacy-2-v1", ".txt"),
            ),
            (
                "aes-legacy-2-v2",
                fixture!("aes-legacy-2-v2", ".zipx"),
                fixture!("aes-legacy-2-v2", ".txt"),
            ),
            (
                "aes-legacy-3-v1",
                fixture!("aes-legacy-3-v1", ".zipx"),
                fixture!("aes-legacy-3-v1", ".txt"),
            ),
            (
                "aes-legacy-3-v2",
                fixture!("aes-legacy-3-v2", ".zipx"),
                fixture!("aes-legacy-3-v2", ".txt"),
            ),
            (
                "aes-ppmd-1-v1",
                fixture!("aes-ppmd-1-v1", ".zipx"),
                fixture!("aes-ppmd-1-v1", ".txt"),
            ),
            (
                "aes-ppmd-1-v2",
                fixture!("aes-ppmd-1-v2", ".zipx"),
                fixture!("aes-ppmd-1-v2", ".txt"),
            ),
            (
                "aes-ppmd-2-v1",
                fixture!("aes-ppmd-2-v1", ".zipx"),
                fixture!("aes-ppmd-2-v1", ".txt"),
            ),
            (
                "aes-ppmd-2-v2",
                fixture!("aes-ppmd-2-v2", ".zipx"),
                fixture!("aes-ppmd-2-v2", ".txt"),
            ),
            (
                "aes-ppmd-3-v1",
                fixture!("aes-ppmd-3-v1", ".zipx"),
                fixture!("aes-ppmd-3-v1", ".txt"),
            ),
            (
                "aes-ppmd-3-v2",
                fixture!("aes-ppmd-3-v2", ".zipx"),
                fixture!("aes-ppmd-3-v2", ".txt"),
            ),
            (
                "lzma-descriptor",
                fixture!("lzma-descriptor", ".zipx"),
                fixture!("lzma-descriptor", ".txt"),
            ),
            (
                "aes-descriptor",
                fixture!("aes-descriptor", ".zipx"),
                fixture!("aes-descriptor", ".txt"),
            ),
            (
                "aes-lzma-empty",
                fixture!("aes-lzma-empty", ".zipx"),
                fixture!("aes-lzma-empty", ".txt"),
            ),
            (
                "aes-xz-empty",
                fixture!("aes-xz-empty", ".zipx"),
                fixture!("aes-xz-empty", ".txt"),
            ),
            (
                "aes-ppmd-empty",
                fixture!("aes-ppmd-empty", ".zipx"),
                fixture!("aes-ppmd-empty", ".txt"),
            ),
            (
                "aes-directory",
                fixture!("aes-directory", ".zipx"),
                fixture!("aes-directory", ".txt"),
            ),
        ] {
            let temp = TempDir::new();
            let path = temp.path().join("input.ZIPX");
            fs::write(&path, bytes).unwrap();
            let progress = AtomicU64::new(0);
            let report =
                check_with_password(&path, &|| false, &progress, Some(b"fixture-password"))
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(report.kind, ArchiveKind::Zip);
            assert_eq!(report.members, 1, "{name}");
            assert_eq!(report.decoded, expected.len() as u64, "{name}");
            assert_eq!(progress.load(Ordering::Relaxed), report.decoded);
            assert_eq!(fs::read(&path).unwrap(), bytes, "{name}: source modified");
            assert_eq!(
                fs::read_dir(temp.path()).unwrap().count(),
                1,
                "{name}: extraction side effects"
            );
        }
    }
    #[test]
    fn password_integrity_never_completes_on_password_or_payload_failure() {
        for (name, bytes, password, text) in [
            (
                "missing",
                fixture!("aes-ppmd-3-v2", ".zipx"),
                None,
                "password",
            ),
            (
                "wrong",
                fixture!("aes-ppmd-3-v2", ".zipx"),
                Some(b"wrong-password".as_slice()),
                "password",
            ),
            (
                "custom tail",
                fixture!("aes-ppmd-tail-bad-tag", ".zipx"),
                Some(b"fixture-password".as_slice()),
                "authentication",
            ),
            (
                "standard tag",
                fixture!("aes-stored-bad-tag", ".zip"),
                Some(b"fixture-password".as_slice()),
                "",
            ),
            (
                "standard size",
                fixture!("aes-stored-bad-size", ".zip"),
                Some(b"fixture-password".as_slice()),
                "size",
            ),
            (
                "custom CRC",
                fixture!("aes-ae1-bad-crc", ".zipx"),
                Some(b"fixture-password".as_slice()),
                "CRC",
            ),
            (
                "directory tag",
                fixture!("aes-directory-bad-tag", ".zipx"),
                Some(b"fixture-password".as_slice()),
                "",
            ),
        ] {
            let temp = TempDir::new();
            let path = temp.path().join("input.zipx");
            fs::write(&path, bytes).unwrap();
            let error =
                check_with_password(&path, &|| false, &AtomicU64::new(0), password).unwrap_err();
            assert!(error.contains(text), "{name}: {error}");
            assert!(!error.contains("fixture-password") && !error.contains("wrong-password"));
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        }
    }
    #[test]
    fn password_integrity_preserves_budgets_cancellation_and_source_stamp() {
        let temp = TempDir::new();
        let path = temp.path().join("input.zipx");
        let bytes = fixture!("aes-ppmd-tail", ".zipx");
        fs::write(&path, bytes).unwrap();
        let progress = AtomicU64::new(0);
        for (limit, members) in [(1, 100), (128 * 1024 * 1024, 0)] {
            let error = check_with_password_limits(
                &path,
                &|| false,
                &progress,
                Some(b"fixture-password"),
                limit,
                members,
            )
            .unwrap_err();
            assert!(error.contains("limit"), "{error}");
        }
        let error = check_with_password(
            &path,
            &|| progress.load(Ordering::Relaxed) > 0,
            &progress,
            Some(b"fixture-password"),
        )
        .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        fs::write(&path, fixture!("aes-ppmd-3-v2", ".zipx")).unwrap();
        progress.store(0, Ordering::Relaxed);
        let changed = std::cell::Cell::new(false);
        let error = check_with_password(
            &path,
            &|| {
                if progress.load(Ordering::Relaxed) > 0 && !changed.replace(true) {
                    fs::rename(&path, temp.path().join("old.zipx")).unwrap();
                    fs::write(&path, b"replacement").unwrap();
                }
                false
            },
            &progress,
            Some(b"fixture-password"),
        )
        .unwrap_err();
        assert!(changed.get() && error.contains("changed"), "{error}");
    }
    #[test]
    fn password_integrity_worker_accepts_empty_and_utf8_passwords() {
        for (bytes, password) in [
            (fixture!("aes-empty-password", ".zipx"), ""),
            (fixture!("aes-utf8-password", ".zipx"), "fixture-пароль-ёж"),
        ] {
            let temp = TempDir::new();
            let path = temp.path().join("input.zipx");
            fs::write(&path, bytes).unwrap();
            let run = start(
                path,
                Some(zeroize::Zeroizing::new(password.into())),
                false,
                Arc::new(|| {}),
            );
            let report = run
                .receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .unwrap();
            assert_eq!(report.members, 1);
            assert_eq!(
                report.decoded,
                fixture!("aes-utf8-password", ".txt").len() as u64
            );
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        }
    }
    #[test]
    fn ppmd_integrity_keeps_decoded_byte_limit_and_cancellation() {
        let temp = TempDir::new();
        let path = temp.path().join("extended.zipx");
        fs::write(
            &path,
            include_bytes!("../docs/qualification/zip-ppmd-2026-11-03/fixtures/normal.zipx"),
        )
        .unwrap();
        let progress = AtomicU64::new(0);
        assert!(
            check_with_limits(&path, &|| false, &progress, 1, 100)
                .unwrap_err()
                .contains("limit")
        );
        assert!(
            check(&path, &|| true, &progress)
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn zstd_integrity_keeps_decoded_byte_limit_and_cancellation() {
        let temp = TempDir::new();
        let path = temp.path().join("extended.zipx");
        fs::write(
            &path,
            include_bytes!("../docs/qualification/zip-zstd-2026-11-03/fixtures/normal.zipx"),
        )
        .unwrap();
        let progress = AtomicU64::new(0);
        assert!(
            check_with_limits(&path, &|| false, &progress, 1, 100)
                .unwrap_err()
                .contains("limit")
        );
        assert!(
            check(&path, &|| true, &progress)
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn xz_integrity_keeps_decoded_byte_limit_and_cancellation() {
        let temp = TempDir::new();
        let path = temp.path().join("extended.zipx");
        fs::write(
            &path,
            include_bytes!("../docs/qualification/zip-xz-2026-11-03/fixtures/normal.zipx"),
        )
        .unwrap();
        let progress = AtomicU64::new(0);
        assert!(
            check_with_limits(&path, &|| false, &progress, 1, 100)
                .unwrap_err()
                .contains("limit")
        );
        assert!(
            check(&path, &|| true, &progress)
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn lzma_integrity_keeps_decoded_byte_limit_and_cancellation() {
        let temp = TempDir::new();
        let path = temp.path().join("extended.zipx");
        fs::write(
            &path,
            include_bytes!("../docs/qualification/zip-lzma-2026-11-03/fixtures/normal.zipx"),
        )
        .unwrap();
        let progress = AtomicU64::new(0);
        assert!(
            check_with_limits(&path, &|| false, &progress, 1, 100)
                .unwrap_err()
                .contains("limit")
        );
        assert!(
            check(&path, &|| true, &progress)
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn deflate64_integrity_keeps_decoded_byte_limit_and_cancellation() {
        let temp = TempDir::new();
        let path = temp.path().join("extended.zipx");
        fs::write(
            &path,
            include_bytes!("../docs/qualification/zip-deflate64-2026-11-03/fixtures/extended.zipx"),
        )
        .unwrap();
        let progress = AtomicU64::new(0);
        assert!(
            check_with_limits(&path, &|| false, &progress, 1, 100)
                .unwrap_err()
                .contains("limit")
        );
        assert!(
            check(&path, &|| true, &progress)
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn bzip2_zipx_integrity_preserves_decode_budget_and_cancellation() {
        let temp = TempDir::new();
        let path = temp.path().join("mixed.zipx");
        fs::write(
            &path,
            include_bytes!("../docs/qualification/zip-bzip2-2026-11-03/fixtures/mixed.zipx"),
        )
        .unwrap();
        let progress = AtomicU64::new(0);
        assert!(
            check_with_limits(&path, &|| false, &progress, 1, 100)
                .unwrap_err()
                .contains("limit")
        );
        assert!(
            check(&path, &|| true, &progress)
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn tar_content_groups_cover_all_codecs_pax_duplicate_names_and_list_identities() {
        let temp = TempDir::new();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/qualification/tar-content-groups-2026-11-03/fixtures");
        for name in [
            "groups.tar",
            "groups.tar.gz",
            "groups.tar.bz2",
            "groups.tar.xz",
            "groups.tar.zst",
        ] {
            let path = temp.path().join(name);
            let before = fs::read(fixtures.join(name)).unwrap();
            fs::write(&path, &before).unwrap();
            let report = scan(&path, &|| false, &AtomicU64::new(0), None, group_options()).unwrap();
            assert_eq!(report.members, 11, "{name}");
            assert!(!report.strict_headers);
            assert!(report.summary().contains("no payload checksum"));
            let groups = report.content_groups.unwrap();
            assert_eq!(groups.len(), 2);
            assert_eq!(
                groups[0]
                    .members
                    .iter()
                    .map(|m| m.index)
                    .collect::<Vec<_>>(),
                [4, 5]
            );
            assert_eq!(groups[0].size, 0);
            assert_eq!(
                groups[1]
                    .members
                    .iter()
                    .map(|m| m.index)
                    .collect::<Vec<_>>(),
                [0, 2, 8, 9, 10]
            );
            assert_eq!(
                groups[1].size,
                "tar archive needle: ёж / café\n".len() as u64
            );
            let mut listed = Vec::new();
            crate::archive::visit_members(
                &path,
                false,
                &mut crate::archive::SearchBudget::default(),
                || false,
                |m| {
                    listed.push(m.member);
                    true
                },
            )
            .unwrap();
            assert_eq!(listed.len(), 11, "{name}");
            for group in groups {
                for member in group.members {
                    assert!(
                        listed.iter().any(|listed| listed.index == member.index
                            && listed.path == member.path
                            && listed.size == member.size
                            && listed.is_regular),
                        "{name}: {}",
                        member.path.display()
                    );
                }
            }
            assert_eq!(fs::read(&path).unwrap(), before);
            assert!(!temp.path().join("a-same.txt").exists());
        }
    }
    #[test]
    fn tar_content_groups_consume_later_archives_and_refuse_corruption_limits_cancel() {
        let temp = TempDir::new();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/qualification/tar-content-groups-2026-11-03/fixtures");
        for name in [
            "groups.tar",
            "groups.tar.gz",
            "groups.tar.bz2",
            "groups.tar.xz",
            "groups.tar.zst",
        ] {
            let path = temp.path().join(name);
            let original = fs::read(fixtures.join(name)).unwrap();
            let mut concat = original.clone();
            concat.extend_from_slice(&original);
            fs::write(&path, &concat).unwrap();
            let report = scan(&path, &|| false, &AtomicU64::new(0), None, group_options()).unwrap();
            assert_eq!(report.members, 22, "{name}");
            assert_eq!(
                report
                    .content_groups
                    .unwrap()
                    .iter()
                    .map(|g| g.members.len())
                    .sum::<usize>(),
                16
            );
            let mut listed = Vec::new();
            crate::archive::visit_members(
                &path,
                false,
                &mut crate::archive::SearchBudget::default(),
                || false,
                |m| {
                    listed.push(m.member.index);
                    true
                },
            )
            .unwrap();
            assert_eq!(listed, (0..22).collect::<Vec<_>>(), "{name}");
            for options in [
                ScanOptions {
                    limit: 512,
                    ..group_options()
                },
                ScanOptions {
                    max_members: 1,
                    ..group_options()
                },
                ScanOptions {
                    metadata_limit: 1,
                    ..group_options()
                },
            ] {
                assert!(
                    scan(&path, &|| false, &AtomicU64::new(0), None, options).is_err(),
                    "{name}"
                );
            }
            assert!(scan(&path, &|| true, &AtomicU64::new(0), None, group_options()).is_err());
            let offset = if name == "groups.tar" {
                concat.len() / 2
            } else if name.ends_with("bz2") {
                concat.len() - 3
            } else {
                concat.len() - 1
            };
            concat[offset] ^= 0xff;
            fs::write(&path, &concat).unwrap();
            assert!(
                scan(&path, &|| false, &AtomicU64::new(0), None, group_options()).is_err(),
                "{name}"
            );
        }
    }

    fn tar_bytes(data: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for name in ["first.bin", "second.bin"] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, name, data).unwrap();
        }
        builder.into_inner().unwrap()
    }
    fn zip_file(path: &Path, data: &[u8]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        zip.start_file(
            "payload.bin",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(data).unwrap();
        zip.finish().unwrap();
    }
    #[test]
    fn zip_binary_crc_reads_are_complete_and_bad_payload_is_rejected() {
        let temp = TempDir::new();
        let path = temp.path().join("check.zip");
        zip_file(&path, b"unique payload\0binary");
        let original = fs::read(&path).unwrap();
        let progress = AtomicU64::new(0);
        let report = check(&path, &|| false, &progress).unwrap();
        assert_eq!(report.members, 1);
        assert_eq!(report.decoded, 21);
        assert!(report.summary().contains("ZIP CRC"));
        assert_eq!(fs::read(&path).unwrap(), original);
        let mut corrupt = original;
        let offset = corrupt
            .windows(14)
            .position(|s| s == b"unique payload")
            .unwrap();
        corrupt[offset] ^= 1;
        fs::write(&path, corrupt).unwrap();
        assert!(
            check(&path, &|| false, &progress)
                .unwrap_err()
                .contains("Invalid checksum")
        );
    }
    #[test]
    fn all_tar_codecs_read_binary_members_without_modifying_sources() {
        let temp = TempDir::new();
        let raw = tar_bytes(b"unique payload\0binary");
        for name in [
            "check.tar",
            "check.tgz",
            "check.tbz2",
            "check.txz",
            "check.tzst",
        ] {
            let path = temp.path().join(name);
            let kind = crate::archive::kind_of(&path).unwrap();
            let bytes = crate::archive::encode_tar_fixture(kind, &raw).unwrap();
            fs::write(&path, &bytes).unwrap();
            let report = check(&path, &|| false, &AtomicU64::new(0)).unwrap();
            assert_eq!(report.kind, kind);
            assert_eq!(report.members, 2);
            assert_eq!(report.decoded, raw.len() as u64);
            assert!(report.summary().contains("no payload checksum"));
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
    #[test]
    fn compressed_trailers_and_later_frames_are_not_ignored() {
        let temp = TempDir::new();
        let raw = tar_bytes(b"payload");
        for name in ["bad.tgz", "bad.tbz2", "bad.txz", "bad.tzst"] {
            let path = temp.path().join(name);
            let kind = crate::archive::kind_of(&path).unwrap();
            let mut bytes = if kind == ArchiveKind::TarZstd {
                let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
                encoder.include_checksum(true).unwrap();
                encoder.write_all(&raw).unwrap();
                encoder.finish().unwrap()
            } else {
                crate::archive::encode_tar_fixture(kind, &raw).unwrap()
            };
            let offset = if kind == ArchiveKind::TarBz2 {
                bytes.len() - 3
            } else {
                bytes.len() - 1
            };
            bytes[offset] ^= 0xff;
            fs::write(&path, bytes).unwrap();
            assert!(
                check(&path, &|| false, &AtomicU64::new(0)).is_err(),
                "{name}"
            );
            let mut bytes = crate::archive::encode_tar_fixture(kind, &raw).unwrap();
            bytes.extend(crate::archive::encode_tar_fixture(kind, &raw).unwrap());
            fs::write(&path, &bytes).unwrap();
            assert_eq!(
                check(&path, &|| false, &AtomicU64::new(0)).unwrap().members,
                4,
                "{name}"
            );
            bytes.truncate(bytes.len() - 6);
            fs::write(&path, bytes).unwrap();
            assert!(
                check(&path, &|| false, &AtomicU64::new(0)).is_err(),
                "{name}"
            );
        }
    }
    #[test]
    fn tar_header_and_truncated_payload_fail_but_payload_checksums_are_not_invented() {
        let temp = TempDir::new();
        let path = temp.path().join("check.tar");
        let raw = tar_bytes(b"unique payload");
        fs::write(&path, []).unwrap();
        assert!(
            check(&path, &|| false, &AtomicU64::new(0))
                .unwrap_err()
                .contains("Empty stream")
        );
        fs::write(&path, vec![0; 1024]).unwrap();
        assert_eq!(
            check(&path, &|| false, &AtomicU64::new(0)).unwrap().members,
            0
        );
        let mut bytes = raw.clone();
        bytes[0] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(check(&path, &|| false, &AtomicU64::new(0)).is_err());
        fs::write(&path, &raw[..518]).unwrap();
        assert!(check(&path, &|| false, &AtomicU64::new(0)).is_err());
        let mut bytes = raw;
        bytes[512] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(
            check(&path, &|| false, &AtomicU64::new(0))
                .unwrap()
                .summary()
                .contains("no payload checksum")
        );
    }
    #[test]
    fn budget_fences_count_limits_and_cancellation_never_return_complete() {
        let progress = AtomicU64::new(0);
        let mut reader = BudgetReader {
            inner: std::io::Cursor::new(b"abc"),
            cancelled: &|| false,
            progress: &progress,
            limit: 3,
        };
        assert_eq!(drain(&mut reader).unwrap(), 3);
        progress.store(0, Ordering::Relaxed);
        let mut reader = BudgetReader {
            inner: std::io::Cursor::new(b"abcd"),
            cancelled: &|| false,
            progress: &progress,
            limit: 3,
        };
        assert!(drain(&mut reader).is_err());
        assert_eq!(progress.load(Ordering::Relaxed), 4);
        let temp = TempDir::new();
        for name in ["limit.zip", "limit.tar", "limit.tzst"] {
            let path = temp.path().join(name);
            let data = vec![b'x'; 128 * 1024];
            if name.ends_with("zip") {
                zip_file(&path, &data);
            } else {
                fs::write(
                    &path,
                    crate::archive::encode_tar_fixture(
                        crate::archive::kind_of(&path).unwrap(),
                        &tar_bytes(&data),
                    )
                    .unwrap(),
                )
                .unwrap();
            }
            assert!(
                check_with_limits(&path, &|| false, &progress, 1024, MAX_MEMBERS_PER_RUN)
                    .unwrap_err()
                    .contains("limit")
            );
            assert!(
                check_with_limits(&path, &|| false, &progress, MAX_DECODED, 0)
                    .unwrap_err()
                    .contains("limit")
            );
            progress.store(0, Ordering::Relaxed);
            assert!(
                check(
                    &path,
                    &|| progress.load(Ordering::Relaxed) >= 64 * 1024,
                    &progress
                )
                .unwrap_err()
                .contains("cancelled")
            );
            assert!(
                check(&path, &|| true, &progress)
                    .unwrap_err()
                    .contains("cancelled")
            );
        }
    }
    #[test]
    fn source_replacement_during_check_is_rejected_even_with_valid_old_bytes() {
        let temp = TempDir::new();
        let path = temp.path().join("check.tar");
        fs::write(&path, tar_bytes(&vec![b'x'; 128 * 1024])).unwrap();
        let progress = AtomicU64::new(0);
        let changed = std::cell::Cell::new(false);
        let result = check(
            &path,
            &|| {
                if progress.load(Ordering::Relaxed) >= 512 && !changed.replace(true) {
                    fs::rename(&path, temp.path().join("old.tar")).unwrap();
                    fs::write(&path, b"replacement").unwrap();
                }
                false
            },
            &progress,
        );
        assert!(result.unwrap_err().contains("changed"));
        assert!(changed.get());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temp.path().join("old.tar"), temp.path().join("link.tar"))
                .unwrap();
            assert!(
                check(&temp.path().join("link.tar"), &|| false, &progress)
                    .unwrap_err()
                    .contains("regular files")
            );
        }
    }
    #[test]
    fn asynchronous_check_completes_and_dropping_a_run_sets_only_its_cancel_flag() {
        let temp = TempDir::new();
        let path = temp.path().join("check.zip");
        zip_file(&path, b"abc");
        let run = start(path.clone(), None, false, Arc::new(|| {}));
        assert_eq!(
            run.receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .unwrap()
                .members,
            1
        );
        let other = start(path, None, false, Arc::new(|| {}));
        let first_flag = Arc::clone(&run.cancelled);
        let other_flag = Arc::clone(&other.cancelled);
        drop(run);
        assert!(first_flag.load(Ordering::Acquire));
        assert!(!other_flag.load(Ordering::Acquire));
        drop(other);
        assert!(other_flag.load(Ordering::Acquire));
    }
}
