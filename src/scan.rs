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

/// Names of top-level entries that already exist at `target`.
pub fn find_conflicts(entries: &[FileEntry], target: &Path) -> Vec<String> {
    entries
        .iter()
        .filter(|e| target.join(&e.name).exists())
        .map(|e| e.name.clone())
        .collect()
}

/// Flatten entries on a background thread; the returned list fills in
/// once the scan completes.
pub fn spawn_scan(entries: Vec<FileEntry>) -> FlatList {
    let flat: FlatList = Arc::new(Mutex::new(None));
    let flat_clone = flat.clone();
    std::thread::spawn(move || {
        let result = flatten_entries(&entries);
        *flat_clone.lock().unwrap() = Some(result);
    });
    flat
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
            children.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
            for child in children {
                if *truncated {
                    return;
                }
                let cp = child.path();
                let cn = child.file_name().to_string_lossy().to_string();
                let child_is_dir = cp.is_dir();
                let child_size = if child_is_dir {
                    0
                } else {
                    cp.metadata().map(|m| m.len()).unwrap_or(0)
                };
                flatten_entry(&cp, &cn, child_is_dir, child_size, depth + 1, result, truncated);
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
    fn spawn_scan_fills_result_in_background() {
        let tmp = TempDir::new();
        let f = tmp.file("a.txt", "x");

        let flat = spawn_scan(vec![entry_for(&f)]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(result) = flat.lock().unwrap().as_ref() {
                assert_eq!(result.len(), 1);
                assert_eq!(result[0].name, "a.txt");
                break;
            }
            assert!(std::time::Instant::now() < deadline, "scan timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
