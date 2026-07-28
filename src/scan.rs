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
    let mut flat = Vec::new();
    let mut truncated = false;
    let mut need_bytes = 0_u64;
    let mut failure = None;
    let mut ancestors = std::collections::HashSet::new();
    for entry in entries {
        scan_transfer_entry(
            &entry.path,
            &entry.name,
            0,
            symlink_policy,
            &mut ancestors,
            &mut flat,
            &mut truncated,
            &mut need_bytes,
            &mut failure,
        );
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
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_transfer_entry(
    path: &Path,
    name: &str,
    depth: usize,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    ancestors: &mut std::collections::HashSet<std::path::PathBuf>,
    flat: &mut Vec<FlatFileEntry>,
    truncated: &mut bool,
    need_bytes: &mut u64,
    failure: &mut Option<crate::ports::NativeFailure>,
) {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            record_scan_failure(path, error, failure);
            return;
        }
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return match symlink_policy {
            crate::filesystem_policy::SymlinkPolicy::Preserve => {
                if depth <= MAX_FLAT_DEPTH {
                    push_transfer_preview(
                        flat,
                        truncated,
                        FlatFileEntry {
                            name: name.to_string(),
                            size: 0,
                            is_dir: false,
                            depth,
                        },
                    );
                }
            }
            crate::filesystem_policy::SymlinkPolicy::Skip => {
                if depth <= MAX_FLAT_DEPTH {
                    push_transfer_preview(
                        flat,
                        truncated,
                        FlatFileEntry {
                            name: name.to_string(),
                            size: 0,
                            is_dir: false,
                            depth,
                        },
                    );
                }
            }
            crate::filesystem_policy::SymlinkPolicy::Follow => {
                let followed = match std::fs::canonicalize(path) {
                    Ok(followed) => followed,
                    Err(error) => {
                        record_scan_failure(path, error, failure);
                        return;
                    }
                };
                scan_transfer_entry(
                    &followed,
                    name,
                    depth,
                    symlink_policy,
                    ancestors,
                    flat,
                    truncated,
                    need_bytes,
                    failure,
                );
            }
        };
    }
    let is_dir = file_type.is_dir();
    let size = if is_dir { 0 } else { metadata.len() };

    if depth <= MAX_FLAT_DEPTH {
        push_transfer_preview(
            flat,
            truncated,
            FlatFileEntry {
                name: name.to_string(),
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
        accumulate_logical_size(path, need_bytes, size, failure);
        return;
    }

    let canonical = match std::fs::canonicalize(path) {
        Ok(canonical) => canonical,
        Err(error) => {
            record_scan_failure(path, error, failure);
            return;
        }
    };
    if !ancestors.insert(canonical.clone()) {
        record_scan_failure(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "symlink cycle detected during transfer preflight",
            ),
            failure,
        );
        return;
    }
    let children = match std::fs::read_dir(path) {
        Ok(children) => children,
        Err(error) => {
            ancestors.remove(&canonical);
            record_scan_failure(path, error, failure);
            return;
        }
    };
    let mut children = children
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                record_scan_failure(path, error, failure);
                None
            }
        })
        .collect::<Vec<_>>();
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let child_name = child.file_name().to_string_lossy().into_owned();
        scan_transfer_entry(
            &child.path(),
            &child_name,
            depth + 1,
            symlink_policy,
            ancestors,
            flat,
            truncated,
            need_bytes,
            failure,
        );
    }
    ancestors.remove(&canonical);
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

fn record_scan_failure(
    path: &Path,
    error: std::io::Error,
    failure: &mut Option<crate::ports::NativeFailure>,
) {
    if failure.is_some() {
        return;
    }
    let mut classified = crate::ports::NativeFailure::from_io(&error);
    classified.message = format!("Could not size {}: {}", path.display(), classified.message);
    *failure = Some(classified);
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
        let meta = std::fs::metadata(path).unwrap();
        FileEntry::from_meta(path.to_path_buf(), &meta).unwrap()
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
