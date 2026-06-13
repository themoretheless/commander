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

/// True when `dest` is the same path as `src` or lives inside `src`'s subtree.
///
/// Used to reject destructive transfers: copying or moving a directory into
/// itself or its own subtree, or a file onto itself. Resolves symlinks and
/// `.`/`..` via `canonicalize` where possible (canonicalizing `dest`'s
/// existing parent, since `dest` itself may not exist yet), and falls back to
/// a lexical, component-wise prefix check when canonicalization fails.
pub fn is_within_or_equal(dest: &Path, src: &Path) -> bool {
    let src_c = src.canonicalize();
    // Prefer canonicalizing the full destination (resolves a symlink in its
    // final component and any `.`/`..`); if it doesn't exist yet, canonicalize
    // its parent and re-attach the file name.
    let dest_c = dest.canonicalize().or_else(|_| match dest.parent() {
        Some(parent) => parent.canonicalize().map(|p| match dest.file_name() {
            Some(name) => p.join(name),
            None => p,
        }),
        None => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
    });
    match (src_c, dest_c) {
        (Ok(s), Ok(d)) => d.starts_with(&s),
        // Canonicalization failed; fall back to a lexical check.
        _ => dest.starts_with(src),
    }
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

/// Duplicate a file or directory next to itself, Finder-style:
/// "name copy.ext", then "name copy 2.ext", ... Returns the new path.
pub fn duplicate(path: &Path) -> std::io::Result<PathBuf> {
    let parent = path.parent().unwrap_or(Path::new("/"));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let dest = first_available(|i| {
        if i == 0 {
            parent.join(format!("{} copy{}", stem, ext))
        } else {
            parent.join(format!("{} copy {}{}", stem, i + 1, ext))
        }
    });
    if path.is_dir() {
        copy_dir_all(path, &dest)?;
    } else {
        std::fs::copy(path, &dest)?;
    }
    Ok(dest)
}

/// Compress a file or directory into "<name>.zip" next to it.
/// Runs `ditto` in the background; returns once the process is spawned.
pub fn compress_to_zip(path: &Path) -> std::io::Result<()> {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no file name",
        ));
    };
    let parent = path.parent().unwrap_or(Path::new("/"));
    let archive = parent.join(format!("{}.zip", name));
    std::process::Command::new("ditto")
        .arg("-c")
        .arg("-k")
        .arg("--sequesterRsrc")
        .arg(path)
        .arg(&archive)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn dir_size_sums_nested_files() {
        let tmp = TempDir::new();
        tmp.file("a.bin", "12345");
        tmp.file("sub/b.bin", "123");
        assert_eq!(dir_size_recursive(tmp.path()), 8);
    }

    #[test]
    fn copy_dir_all_replicates_tree() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        src.file("a.txt", "1");
        src.file("sub/b.txt", "22");

        copy_dir_all(src.path(), &dst.path().join("copy")).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.path().join("copy/a.txt")).unwrap(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("copy/sub/b.txt")).unwrap(),
            "22"
        );
    }

    #[test]
    fn first_available_skips_taken_names() {
        let tmp = TempDir::new();
        tmp.dir("New Folder");
        tmp.dir("New Folder 1");

        let picked = first_available(|i| {
            if i == 0 {
                tmp.path().join("New Folder")
            } else {
                tmp.path().join(format!("New Folder {}", i))
            }
        });
        assert_eq!(picked, tmp.path().join("New Folder 2"));
    }

    #[test]
    fn duplicate_appends_copy_suffix_and_counts_up() {
        let tmp = TempDir::new();
        let f = tmp.file("doc.txt", "data");

        let first = duplicate(&f).unwrap();
        assert_eq!(first, tmp.path().join("doc copy.txt"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "data");

        let second = duplicate(&f).unwrap();
        assert_eq!(second, tmp.path().join("doc copy 2.txt"));
    }

    #[test]
    fn duplicate_copies_directories_recursively() {
        let tmp = TempDir::new();
        let dir = tmp.dir("sub");
        tmp.file("sub/inner.txt", "x");

        let copy = duplicate(&dir).unwrap();
        assert_eq!(copy, tmp.path().join("sub copy"));
        assert_eq!(
            std::fs::read_to_string(copy.join("inner.txt")).unwrap(),
            "x"
        );
    }

    #[test]
    fn is_within_or_equal_detects_self_and_subtree() {
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        tmp.dir("a/sub");
        let b = tmp.dir("b");

        // Same path, and a path inside the subtree.
        assert!(is_within_or_equal(&a, &a));
        assert!(is_within_or_equal(&a.join("sub").join("a"), &a));
        // A sibling is not inside.
        assert!(!is_within_or_equal(&b, &a));
        // A name that is a string prefix but not a path prefix.
        assert!(!is_within_or_equal(&tmp.path().join("ab"), &a));
    }

    #[test]
    fn is_within_or_equal_handles_nonexistent_dest() {
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        // dest does not exist yet but its parent (a/sub) does.
        tmp.dir("a/sub");
        assert!(is_within_or_equal(&a.join("sub").join("new"), &a));
    }
}
