//! Read-only archive stream checks, distinct from extraction and content search.
use crate::archive::{ArchiveKind, MAX_MEMBERS_PER_RUN};
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
}
impl Report {
    pub fn summary(&self) -> String {
        format!(
            "Last {} check completed · {} members · {} decoded bytes · {}",
            self.kind.label(),
            self.members,
            self.decoded,
            if self.kind == ArchiveKind::Zip {
                "ZIP CRC/size checks; AES authentication when present"
            } else {
                "TAR headers/stream; TAR has no payload checksum"
            }
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
    let mut count = 0_u64;
    let mut bytes = [0; 64 * 1024];
    loop {
        match reader.read(&mut bytes) {
            Ok(0) => return Ok(count),
            Ok(bytes) => {
                count += bytes as u64;
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
    cancel(cancelled)?;
    progress.store(0, Ordering::Relaxed);
    let kind = crate::archive::kind_of(path).ok_or("Unsupported archive format.")?;
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
        for index in 0..zip.len() {
            cancel(cancelled)?;
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
            drain(&mut bounded).map_err(|e| format!("Member #{index}: {e}"))?;
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
                let count = drain(&mut BudgetReader {
                    inner: &mut entry,
                    cancelled,
                    progress: &payload,
                    limit,
                })
                .map_err(|e| format!("Member #{members}: {e}"))?;
                if count != expected {
                    return Err(format!("Member #{members}: truncated TAR payload"));
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
    notify: crate::archive::Notify,
) -> Run {
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let progress = Arc::new(AtomicU64::new(0));
    let worker_progress = Arc::clone(&progress);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = if let Some(password) = password {
            check_with_password(
                &path,
                &|| flag.load(Ordering::Acquire),
                &worker_progress,
                Some(password.as_bytes()),
            )
        } else {
            check(&path, &|| flag.load(Ordering::Acquire), &worker_progress)
        };
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
        let run = start(path.clone(), None, Arc::new(|| {}));
        assert_eq!(
            run.receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .unwrap()
                .members,
            1
        );
        let other = start(path, None, Arc::new(|| {}));
        let first_flag = Arc::clone(&run.cancelled);
        let other_flag = Arc::clone(&other.cancelled);
        drop(run);
        assert!(first_flag.load(Ordering::Acquire));
        assert!(!other_flag.load(Ordering::Acquire));
        drop(other);
        assert!(other_flag.load(Ordering::Acquire));
    }
}
