//! Bounded, read-only ZIP inspection shared by the archive browser and search.

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

#[derive(Clone, Debug)]
pub struct ArchiveMember {
    pub index: usize,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub compressed_size: u64,
}

#[derive(Clone, Debug)]
pub struct ArchiveListing {
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

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
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
        members,
        declared_members: summary.declared_members,
        declared_uncompressed_bytes: summary.declared_uncompressed_bytes,
        unsafe_members: summary.unsafe_members,
        unreadable_members: summary.unreadable_members,
        truncated: budget.truncated,
    })
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
        assert!(!is_supported(Path::new("bundle.tar")));
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
}
