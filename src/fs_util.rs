//! Shared filesystem helpers used across panels, transfers and menus.

use std::path::{Path, PathBuf};

/// Total size in bytes of all files under `path` (parallel walk).
pub fn dir_size_recursive(path: &Path) -> u64 {
    jwalk::WalkDir::new(path)
        .skip_hidden(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| !m.is_dir())
        .map(|m| m.len())
        .sum()
}

/// Recursively copy a directory tree (no progress reporting).
pub fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// First path produced by `candidate` that doesn't exist yet.
/// `candidate(0)` is the preferred name, `candidate(n)` the n-th fallback.
pub fn first_available(mut candidate: impl FnMut(usize) -> PathBuf) -> PathBuf {
    let mut i = 0;
    loop {
        let p = candidate(i);
        if !p.exists() {
            return p;
        }
        i += 1;
    }
}
