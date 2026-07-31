//! Pre-flight scan for the confirmation dialog: flatten the selected
//! entries into an indented list and detect name conflicts at the target.

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::panel::FileEntry;

/// A flat entry for the confirmation dialog, with depth for indentation.
#[derive(Clone)]
pub struct FlatFileEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub depth: usize,
}

/// Scan result shared with the UI; `None` while the scan is running.
pub type FlatList = Arc<Mutex<Option<Vec<FlatFileEntry>>>>;

const MAX_FLAT_DEPTH: usize = 5;
const MAX_FLAT_ENTRIES: usize = 5000;

pub struct TransferPreflightScan {
    pub flat: Vec<FlatFileEntry>,
    pub need_bytes: Result<u64, crate::ports::NativeFailure>,
    pub source_identities:
        Vec<Result<crate::path_identity::TransferSourceIdentity, crate::ports::NativeFailure>>,
}

pub fn pending_flat_list() -> FlatList {
    Arc::new(Mutex::new(None))
}

/// Names of top-level entries whose name is already taken at `target`. A name
/// occupied by a broken symlink counts: it cannot be written without clobbering
/// and must be surfaced as a conflict, which `Path::exists` (it follows the
/// link and reports the missing target as absent) would miss.
pub fn find_conflicts(entries: &[FileEntry], target: &Path) -> Vec<String> {
    entries
        .iter()
        .filter(|e| crate::fs_util::path_is_taken(&target.join(&e.name)))
        .map(|e| e.name.clone())
        .collect()
}

/// Flatten entries on a background thread; the returned list fills in
/// once the scan completes.
pub fn spawn_scan(entries: Vec<FileEntry>) -> FlatList {
    let flat = pending_flat_list();
    let flat_clone = flat.clone();
    let fallback = shallow_preview(&entries);
    let worker_fallback = fallback.clone();
    let spawn = std::thread::Builder::new()
        .name("file-preview-scan".to_string())
        .spawn(move || publish_scan(entries, flat_clone, worker_fallback));
    if spawn.is_err() {
        *crate::lock_util::recover(&flat) = Some(fallback);
    }
    flat
}

fn publish_scan(entries: Vec<FileEntry>, flat: FlatList, fallback: Vec<FlatFileEntry>) {
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| flatten_entries(&entries)))
            .unwrap_or(fallback);
    *crate::lock_util::recover(&flat) = Some(result);
}

pub(crate) fn shallow_preview(entries: &[FileEntry]) -> Vec<FlatFileEntry> {
    let mut result = entries
        .iter()
        .take(MAX_FLAT_ENTRIES)
        .map(|entry| FlatFileEntry {
            name: entry.name.clone(),
            size: entry.size,
            is_dir: entry.is_dir,
            depth: 0,
        })
        .collect::<Vec<_>>();
    if entries.len() > MAX_FLAT_ENTRIES {
        result.push(FlatFileEntry {
            name: format!("... (truncated at {} entries)", MAX_FLAT_ENTRIES),
            size: 0,
            is_dir: false,
            depth: 0,
        });
    }
    result
}

/// One full symlink-safe traversal for transfer preview and size budgeting.
/// The preview is bounded, while size accounting continues through the full
/// tree. Any unreadable node makes the size untrustworthy.
pub fn transfer_preflight(
    entries: &[FileEntry],
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
) -> TransferPreflightScan {
    transfer_preflight_cancellable(entries, symlink_policy, &|| false)
}

pub(crate) fn transfer_preflight_cancellable(
    entries: &[FileEntry],
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    cancelled: &dyn Fn() -> bool,
) -> TransferPreflightScan {
    let mut flat = Vec::new();
    let mut truncated = false;
    let mut need_bytes = 0_u64;
    let mut failure = None;
    let mut source_identities = Vec::with_capacity(entries.len());
    for entry in entries {
        if cancelled() {
            let cancelled = cancelled_failure();
            failure.get_or_insert_with(|| cancelled.clone());
            source_identities.push(Err(cancelled));
            continue;
        }
        let source = capture_listing_source(
            entry,
            symlink_policy,
            &mut flat,
            &mut truncated,
            &mut need_bytes,
            &mut failure,
            cancelled,
        );
        if let Err(source_failure) = &source
            && failure.is_none()
        {
            failure = Some(source_failure.clone());
        }
        source_identities.push(source);
    }
    if truncated {
        flat.push(FlatFileEntry {
            name: format!("... (truncated at {} entries)", MAX_FLAT_ENTRIES),
            size: 0,
            is_dir: false,
            depth: 0,
        });
    }
    TransferPreflightScan {
        flat,
        need_bytes: failure.map_or(Ok(need_bytes), Err),
        source_identities,
    }
}

/// Capture the exact source proof the transfer worker will later revalidate.
/// This uses the same traversal as resource preflight, including followed
/// symlink targets, but does not require a UI listing observation.
pub(crate) fn capture_transfer_source(
    path: &Path,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
) -> Result<crate::path_identity::TransferSourceIdentity, crate::ports::NativeFailure> {
    let mut flat = Vec::new();
    let mut truncated = false;
    let mut need_bytes = 0;
    let mut failure = None;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let source = capture_source(
        path,
        &name,
        symlink_policy,
        None,
        &mut flat,
        &mut truncated,
        &mut need_bytes,
        &mut failure,
        &|| false,
    )?;
    if let Some(failure) = failure {
        Err(failure)
    } else {
        Ok(source)
    }
}

fn capture_listing_source(
    entry: &FileEntry,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    flat: &mut Vec<FlatFileEntry>,
    truncated: &mut bool,
    need_bytes: &mut u64,
    failure: &mut Option<crate::ports::NativeFailure>,
    cancelled: &dyn Fn() -> bool,
) -> Result<crate::path_identity::TransferSourceIdentity, crate::ports::NativeFailure> {
    let expected = match &entry.identity {
        crate::panel::ListingIdentity::Captured(identity) => identity,
        crate::panel::ListingIdentity::CaptureFailed(failure) => return Err(failure.clone()),
        crate::panel::ListingIdentity::Unavailable => {
            return Err(crate::ports::NativeFailure {
                kind: crate::ports::NativeFailureKind::Unsupported,
                message: format!(
                    "The visible listing did not capture an identity for {}",
                    entry.path.display()
                ),
            });
        }
    };
    let source = capture_source(
        &entry.path,
        &entry.name,
        symlink_policy,
        Some(expected),
        flat,
        truncated,
        need_bytes,
        failure,
        cancelled,
    )?;
    if !expected.same_shallow_binding(&source.lexical) {
        return Err(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Stale,
            message: format!(
                "{} changed after it was shown in the listing",
                entry.path.display()
            ),
        });
    }
    Ok(source)
}

#[allow(clippy::too_many_arguments)]
fn capture_source(
    path: &Path,
    name: &str,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    expected_root: Option<&crate::path_identity::PathIdentity>,
    flat: &mut Vec<FlatFileEntry>,
    truncated: &mut bool,
    need_bytes: &mut u64,
    failure: &mut Option<crate::ports::NativeFailure>,
    cancelled: &dyn Fn() -> bool,
) -> Result<crate::path_identity::TransferSourceIdentity, crate::ports::NativeFailure> {
    let bytes_before = *need_bytes;
    struct ProofCapture {
        path: std::path::PathBuf,
        before: crate::path_identity::PathIdentity,
        fingerprint: Option<crate::path_identity::TreeFingerprint>,
        link: Option<std::path::PathBuf>,
        identity: Option<crate::path_identity::PathIdentity>,
    }

    enum Task {
        Visit {
            proof: usize,
            path: std::path::PathBuf,
            relative: std::path::PathBuf,
            display_name: String,
            depth: usize,
            metadata: Option<Box<std::fs::Metadata>>,
        },
        ExitDirectory(std::path::PathBuf),
        FinishProof(usize),
    }

    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| classify_scan_failure(path, "inspect", error))?;
    let before = crate::path_identity::PathIdentity::from_metadata(path.to_path_buf(), &metadata);
    if expected_root.is_some_and(|expected| !expected.same_shallow_binding(&before)) {
        return Err(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Stale,
            message: format!(
                "{} changed after it was shown in the listing",
                path.display()
            ),
        });
    }
    let mut proofs = vec![ProofCapture {
        path: path.to_path_buf(),
        before,
        fingerprint: metadata
            .is_dir()
            .then(crate::path_identity::TreeFingerprint::new),
        link: None,
        identity: None,
    }];
    let mut tasks = vec![
        Task::FinishProof(0),
        Task::Visit {
            proof: 0,
            path: path.to_path_buf(),
            relative: std::path::PathBuf::new(),
            display_name: name.to_string(),
            depth: 0,
            metadata: Some(Box::new(metadata)),
        },
    ];
    let mut ancestors = std::collections::HashSet::new();
    while let Some(task) = tasks.pop() {
        if cancelled() {
            return Err(cancelled_failure());
        }
        match task {
            Task::FinishProof(proof) => {
                let capture = &mut proofs[proof];
                let after = crate::path_identity::PathIdentity::observe(&capture.path)
                    .map_err(|error| classify_scan_failure(&capture.path, "recheck", error))?;
                if !capture.before.same_shallow_binding(&after) {
                    return Err(crate::ports::NativeFailure {
                        kind: crate::ports::NativeFailureKind::Stale,
                        message: format!(
                            "{} changed while it was being scanned",
                            capture.path.display()
                        ),
                    });
                }
                capture.identity = Some(match capture.fingerprint.take() {
                    Some(fingerprint) => after.with_tree_fingerprint(fingerprint.finish()),
                    None => after,
                });
            }
            Task::ExitDirectory(canonical) => {
                ancestors.remove(&canonical);
            }
            Task::Visit {
                proof,
                path,
                relative,
                display_name,
                depth,
                metadata,
            } => {
                let metadata = match metadata {
                    Some(metadata) => *metadata,
                    None => std::fs::symlink_metadata(&path)
                        .map_err(|error| classify_scan_failure(&path, "inspect", error))?,
                };
                if let Some(fingerprint) = &mut proofs[proof].fingerprint {
                    fingerprint.record(&relative, &metadata);
                }
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    match symlink_policy {
                        crate::filesystem_policy::SymlinkPolicy::Preserve
                        | crate::filesystem_policy::SymlinkPolicy::Skip => {
                            if depth <= MAX_FLAT_DEPTH {
                                push_transfer_preview(
                                    flat,
                                    truncated,
                                    FlatFileEntry {
                                        name: display_name,
                                        size: 0,
                                        is_dir: false,
                                        depth,
                                    },
                                );
                            }
                        }
                        crate::filesystem_policy::SymlinkPolicy::Follow => {
                            let target = std::fs::canonicalize(&path)
                                .map_err(|error| classify_scan_failure(&path, "follow", error))?;
                            let target_metadata =
                                std::fs::symlink_metadata(&target).map_err(|error| {
                                    classify_scan_failure(&target, "inspect", error)
                                })?;
                            let before = crate::path_identity::PathIdentity::from_metadata(
                                target.clone(),
                                &target_metadata,
                            );
                            let target_proof = proofs.len();
                            proofs.push(ProofCapture {
                                path: target.clone(),
                                before,
                                fingerprint: target_metadata
                                    .is_dir()
                                    .then(crate::path_identity::TreeFingerprint::new),
                                link: Some(path),
                                identity: None,
                            });
                            tasks.push(Task::FinishProof(target_proof));
                            tasks.push(Task::Visit {
                                proof: target_proof,
                                path: target,
                                relative: std::path::PathBuf::new(),
                                display_name,
                                depth,
                                metadata: Some(Box::new(target_metadata)),
                            });
                        }
                    }
                    continue;
                }

                let is_dir = file_type.is_dir();
                let size = if is_dir { 0 } else { metadata.len() };
                if depth <= MAX_FLAT_DEPTH {
                    push_transfer_preview(
                        flat,
                        truncated,
                        FlatFileEntry {
                            name: display_name,
                            size,
                            is_dir,
                            depth,
                        },
                    );
                    if is_dir && depth == MAX_FLAT_DEPTH {
                        push_transfer_preview(
                            flat,
                            truncated,
                            FlatFileEntry {
                                name: "...".to_string(),
                                size: 0,
                                is_dir: false,
                                depth: depth + 1,
                            },
                        );
                    }
                }
                if !is_dir {
                    accumulate_logical_size(&path, need_bytes, size, failure);
                    continue;
                }

                let canonical = std::fs::canonicalize(&path)
                    .map_err(|error| classify_scan_failure(&path, "resolve", error))?;
                if !ancestors.insert(canonical.clone()) {
                    return Err(crate::ports::NativeFailure {
                        kind: crate::ports::NativeFailureKind::Stale,
                        message: format!(
                            "Could not scan {}: symlink cycle detected",
                            path.display()
                        ),
                    });
                }
                let read_dir = std::fs::read_dir(&path)
                    .map_err(|error| classify_scan_failure(&path, "read", error))?;
                let mut children = Vec::new();
                for child in read_dir {
                    children
                        .push(child.map_err(|error| classify_scan_failure(&path, "read", error))?);
                }
                children.sort_by_key(|entry| entry.file_name());
                tasks.push(Task::ExitDirectory(canonical));
                for child in children.into_iter().rev() {
                    let child_name = child.file_name();
                    tasks.push(Task::Visit {
                        proof,
                        path: child.path(),
                        relative: relative.join(&child_name),
                        display_name: child_name.to_string_lossy().into_owned(),
                        depth: depth + 1,
                        metadata: None,
                    });
                }
            }
        }
    }

    let lexical = proofs
        .first_mut()
        .and_then(|proof| proof.identity.take())
        .ok_or_else(|| crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Unknown,
            message: format!(
                "Transfer preflight did not finish scanning {}",
                path.display()
            ),
        })?;
    let followed = proofs
        .into_iter()
        .skip(1)
        .map(|proof| {
            let link = proof.link.ok_or_else(|| crate::ports::NativeFailure {
                kind: crate::ports::NativeFailureKind::Unknown,
                message: "Transfer preflight lost a followed-link binding".to_string(),
            })?;
            let target = proof.identity.ok_or_else(|| crate::ports::NativeFailure {
                kind: crate::ports::NativeFailureKind::Unknown,
                message: format!(
                    "Transfer preflight did not finish scanning {}",
                    proof.path.display()
                ),
            })?;
            Ok(crate::path_identity::FollowedPathIdentity { link, target })
        })
        .collect::<Result<Vec<_>, crate::ports::NativeFailure>>()?;
    Ok(crate::path_identity::TransferSourceIdentity {
        lexical,
        followed,
        logical_bytes: need_bytes.saturating_sub(bytes_before),
    })
}

fn cancelled_failure() -> crate::ports::NativeFailure {
    crate::ports::NativeFailure {
        kind: crate::ports::NativeFailureKind::Cancelled,
        message: "Transfer preflight was cancelled".to_string(),
    }
}

fn classify_scan_failure(
    path: &Path,
    action: &str,
    error: std::io::Error,
) -> crate::ports::NativeFailure {
    let mut classified = crate::ports::NativeFailure::from_io(&error);
    classified.message = format!(
        "Could not {action} {} during transfer preflight: {}",
        path.display(),
        classified.message
    );
    classified
}

fn accumulate_logical_size(
    path: &Path,
    total: &mut u64,
    size: u64,
    failure: &mut Option<crate::ports::NativeFailure>,
) {
    if let Some(next) = total.checked_add(size) {
        *total = next;
        return;
    }
    *total = u64::MAX;
    if failure.is_none() {
        *failure = Some(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Overflow,
            message: format!(
                "Could not size {}: logical size exceeds the supported range",
                path.display()
            ),
        });
    }
}

fn push_transfer_preview(
    flat: &mut Vec<FlatFileEntry>,
    truncated: &mut bool,
    entry: FlatFileEntry,
) {
    if flat.len() < MAX_FLAT_ENTRIES {
        flat.push(entry);
    } else {
        *truncated = true;
    }
}

/// Recursively collect all files/dirs into a flat list with depth.
/// Limited to MAX_FLAT_DEPTH levels and MAX_FLAT_ENTRIES total.
pub fn flatten_entries(entries: &[FileEntry]) -> Vec<FlatFileEntry> {
    let mut result = Vec::new();
    let mut truncated = false;
    for entry in entries {
        flatten_entry(
            &entry.path,
            &entry.name,
            entry.is_dir,
            entry.size,
            0,
            &mut result,
            &mut truncated,
        );
        if truncated {
            break;
        }
    }
    if truncated {
        result.push(FlatFileEntry {
            name: format!("... (truncated at {} entries)", MAX_FLAT_ENTRIES),
            size: 0,
            is_dir: false,
            depth: 0,
        });
    }
    result
}

fn flatten_entry(
    path: &Path,
    name: &str,
    is_dir: bool,
    size: u64,
    depth: usize,
    result: &mut Vec<FlatFileEntry>,
    truncated: &mut bool,
) {
    if result.len() >= MAX_FLAT_ENTRIES {
        *truncated = true;
        return;
    }

    result.push(FlatFileEntry {
        name: name.to_string(),
        size,
        is_dir,
        depth,
    });

    if is_dir && depth < MAX_FLAT_DEPTH {
        if let Ok(rd) = std::fs::read_dir(path) {
            let mut children: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            children.sort_by_key(|a| a.file_name());
            for child in children {
                if *truncated {
                    return;
                }
                let cp = child.path();
                let cn = child.file_name().to_string_lossy().to_string();
                // Use the directory entry's own type, which does not follow
                // symlinks: a symlinked directory is shown as a leaf, never
                // recursed into. That keeps the preview bounded (no escaping the
                // subtree, no cycles) and matches how the copy treats links.
                let child_is_dir = child.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let child_size = if child_is_dir {
                    0
                } else {
                    child.metadata().map(|m| m.len()).unwrap_or(0)
                };
                flatten_entry(
                    &cp,
                    &cn,
                    child_is_dir,
                    child_size,
                    depth + 1,
                    result,
                    truncated,
                );
            }
        }
    } else if is_dir && depth >= MAX_FLAT_DEPTH {
        // Show placeholder for deep dirs
        result.push(FlatFileEntry {
            name: "...".to_string(),
            size: 0,
            is_dir: false,
            depth: depth + 1,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::FileEntry;
    use crate::testutil::TempDir;

    fn entry_for(path: &std::path::Path) -> FileEntry {
        let lexical = std::fs::symlink_metadata(path).unwrap();
        let display = if lexical.file_type().is_symlink() {
            std::fs::metadata(path).unwrap()
        } else {
            lexical.clone()
        };
        let mut entry = FileEntry::from_meta(path.to_path_buf(), &display).unwrap();
        entry.identity = crate::panel::ListingIdentity::Captured(
            crate::path_identity::PathIdentity::from_metadata(path.to_path_buf(), &lexical),
        );
        entry
    }

    #[test]
    fn find_conflicts_reports_existing_names_only() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let a = src.file("a.txt", "x");
        let b = src.file("b.txt", "x");
        dst.file("a.txt", "y");

        let conflicts = find_conflicts(&[entry_for(&a), entry_for(&b)], dst.path());
        assert_eq!(conflicts, vec!["a.txt".to_string()]);
    }

    #[test]
    fn flatten_walks_dirs_with_depth_and_sizes() {
        let tmp = TempDir::new();
        let dir = tmp.dir("folder");
        tmp.file("folder/b.txt", "22");
        tmp.file("folder/a.txt", "1");
        tmp.dir("folder/sub");
        tmp.file("folder/sub/c.txt", "333");

        let flat = flatten_entries(&[entry_for(&dir)]);
        let view: Vec<(String, usize, bool, u64)> = flat
            .iter()
            .map(|f| (f.name.clone(), f.depth, f.is_dir, f.size))
            .collect();

        assert_eq!(
            view,
            vec![
                ("folder".to_string(), 0, true, 0),
                ("a.txt".to_string(), 1, false, 1),
                ("b.txt".to_string(), 1, false, 2),
                ("sub".to_string(), 1, true, 0),
                ("c.txt".to_string(), 2, false, 3),
            ]
        );
    }

    #[test]
    fn scan_job_publishes_a_complete_result_without_timing() {
        let tmp = TempDir::new();
        let f = tmp.file("a.txt", "x");
        let entries = vec![entry_for(&f)];
        let flat = pending_flat_list();

        publish_scan(entries.clone(), flat.clone(), shallow_preview(&entries));

        let result = crate::lock_util::recover(&flat);
        let result = result.as_ref().expect("published scan");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "a.txt");
    }

    #[test]
    fn logical_size_overflow_is_classified_instead_of_wrapping() {
        let mut total = u64::MAX - 2;
        let mut failure = None;
        accumulate_logical_size(Path::new("/overflow.bin"), &mut total, 3, &mut failure);
        assert_eq!(total, u64::MAX);
        assert_eq!(
            failure.as_ref().map(|failure| failure.kind),
            Some(crate::ports::NativeFailureKind::Overflow)
        );
    }

    #[cfg(unix)]
    #[test]
    fn transfer_preflight_sizes_the_selected_symlink_policy() {
        let tmp = TempDir::new();
        tmp.file("target/value.bin", "1234567");
        let link = tmp.path().join("linked");
        std::os::unix::fs::symlink("target", &link).unwrap();
        let entry = entry_for(&link);

        let skipped = transfer_preflight(
            std::slice::from_ref(&entry),
            crate::filesystem_policy::SymlinkPolicy::Skip,
        );
        assert_eq!(skipped.need_bytes, Ok(0));

        let followed = transfer_preflight(
            std::slice::from_ref(&entry),
            crate::filesystem_policy::SymlinkPolicy::Follow,
        );
        assert_eq!(followed.need_bytes, Ok(7));
        assert!(followed.flat.first().is_some_and(|item| item.is_dir));
        assert_eq!(followed.source_identities.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn followed_target_changes_invalidate_the_transfer_source_proof() {
        let tmp = TempDir::new();
        let target = tmp.file("target/value.bin", "first");
        let link = tmp.path().join("linked");
        std::os::unix::fs::symlink("target", &link).unwrap();
        let entry = entry_for(&link);
        let planned = transfer_preflight(
            std::slice::from_ref(&entry),
            crate::filesystem_policy::SymlinkPolicy::Follow,
        )
        .source_identities
        .into_iter()
        .next()
        .unwrap()
        .unwrap();

        std::fs::write(target, "changed-content").unwrap();
        let current =
            capture_transfer_source(&link, crate::filesystem_policy::SymlinkPolicy::Follow)
                .unwrap();

        assert!(!planned.same_binding(&current));
        assert_eq!(planned.followed.len(), 1);
        assert_eq!(current.logical_bytes, 15);
    }

    #[cfg(unix)]
    #[test]
    fn stale_listing_root_is_rejected_before_replacement_traversal() {
        let tmp = TempDir::new();
        let source = tmp.file("source", "visible");
        let entry = entry_for(&source);
        std::fs::remove_file(&source).unwrap();
        std::fs::create_dir(&source).unwrap();
        std::os::unix::fs::symlink(".", source.join("cycle")).unwrap();

        let scan = transfer_preflight(&[entry], crate::filesystem_policy::SymlinkPolicy::Follow);

        assert!(matches!(
            &scan.source_identities[0],
            Err(failure) if failure.kind == crate::ports::NativeFailureKind::Stale
        ));
        assert!(scan.flat.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn hardlinked_paths_each_contribute_their_logical_copy_size() {
        let tmp = TempDir::new();
        let root = tmp.dir("root");
        let first = tmp.file("root/first.bin", "12345");
        std::fs::hard_link(first, root.join("second.bin")).unwrap();

        let scan = transfer_preflight(
            &[entry_for(&root)],
            crate::filesystem_policy::SymlinkPolicy::Preserve,
        );

        assert_eq!(scan.need_bytes, Ok(10));
    }

    #[test]
    fn iterative_preflight_handles_deep_trees_without_recursive_stack_growth() {
        let tmp = TempDir::new();
        let root = tmp.dir("root");
        let mut current = root.clone();
        for _ in 0..128 {
            current = current.join("d");
            std::fs::create_dir(&current).unwrap();
        }
        std::fs::write(current.join("value.bin"), "x").unwrap();

        let scan = transfer_preflight(
            &[entry_for(&root)],
            crate::filesystem_policy::SymlinkPolicy::Preserve,
        );

        assert_eq!(scan.need_bytes, Ok(1));
    }

    #[cfg(unix)]
    #[test]
    fn followed_symlink_cycle_is_a_typed_size_failure() {
        let tmp = TempDir::new();
        let directory = tmp.dir("folder");
        std::os::unix::fs::symlink(".", directory.join("again")).unwrap();

        let scan = transfer_preflight(
            &[entry_for(&directory)],
            crate::filesystem_policy::SymlinkPolicy::Follow,
        );

        assert!(scan.need_bytes.is_err());
    }
}
