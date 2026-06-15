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

/// The first free Finder-style "copy" name next to `path`:
/// "name copy.ext", then "name copy 2.ext", ...
pub fn available_copy_name(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("/"));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    first_available(|i| {
        if i == 0 {
            parent.join(format!("{stem} copy{ext}"))
        } else {
            parent.join(format!("{stem} copy {}{ext}", i + 1))
        }
    })
}

/// The first free Finder-style name for `name` against a provided set of taken
/// names (rather than the live filesystem): the name itself if free, else
/// "stem copy.ext", "stem copy 2.ext", ... Pure, so a drain/paste plan can be
/// built and tested before anything touches disk.
pub fn free_name_against(name: &str, taken: &std::collections::HashSet<String>) -> String {
    if !taken.contains(name) {
        return name.to_string();
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut i = 0;
    loop {
        let cand = if i == 0 {
            format!("{stem} copy{ext}")
        } else {
            format!("{stem} copy {}{ext}", i + 1)
        };
        if !taken.contains(&cand) {
            return cand;
        }
        i += 1;
    }
}

/// Duplicate a file or directory next to itself, Finder-style. Returns the
/// new path.
pub fn duplicate(path: &Path) -> std::io::Result<PathBuf> {
    let dest = available_copy_name(path);
    if path.is_dir() {
        copy_dir_all(path, &dest)?;
    } else {
        std::fs::copy(path, &dest)?;
    }
    Ok(dest)
}

/// Parse the available bytes from `df -k` output (the Available column,
/// index 3, is in 1024-byte blocks). Pure, so it is unit-testable.
pub fn parse_df_avail_bytes(out: &str) -> Option<u64> {
    let line = out.lines().nth(1)?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    let avail_kib: u64 = cols.get(3)?.parse().ok()?;
    Some(avail_kib * 1024)
}

/// Free space in bytes on the volume containing `path`, via `df -k`.
pub fn free_space(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .arg("-k")
        .arg(path)
        .output()
        .ok()?;
    parse_df_avail_bytes(&String::from_utf8_lossy(&out.stdout))
}

/// Whether two paths live on the same filesystem (so a move is an instant
/// rename needing no extra space).
#[cfg(unix)]
pub fn same_volume(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) => ma.dev() == mb.dev(),
        _ => false,
    }
}

#[cfg(not(unix))]
pub fn same_volume(_a: &Path, _b: &Path) -> bool {
    false
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

/// Content hash of a file, streamed in chunks so large files are not loaded
/// whole. Returns `None` if the file cannot be read. Non-cryptographic (good
/// enough to confirm byte-identity for duplicate detection after a size match).
pub fn content_hash(path: &Path) -> Option<u64> {
    use std::hash::Hasher;
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
    }
    Some(hasher.finish())
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
    fn free_name_against_suffixes_on_collision() {
        let taken: std::collections::HashSet<String> =
            ["a.txt".to_string(), "a copy.txt".to_string()]
                .into_iter()
                .collect();
        assert_eq!(free_name_against("b.txt", &taken), "b.txt"); // free -> unchanged
        assert_eq!(free_name_against("a.txt", &taken), "a copy 2.txt"); // first two taken
        // No extension and dotfiles still get a "copy" suffix.
        let taken2: std::collections::HashSet<String> =
            ["README".to_string()].into_iter().collect();
        assert_eq!(free_name_against("README", &taken2), "README copy");
    }

    #[test]
    fn content_hash_matches_for_identical_bytes() {
        let tmp = TempDir::new();
        let a = tmp.file("a.bin", "hello world");
        let b = tmp.file("b.bin", "hello world");
        let c = tmp.file("c.bin", "hello WORLD");
        assert_eq!(content_hash(&a), content_hash(&b), "identical -> same hash");
        assert_ne!(content_hash(&a), content_hash(&c), "different -> differ");
        assert!(content_hash(Path::new("/no/such/file")).is_none());
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
    fn parse_df_avail_reads_available_column() {
        let out = "Filesystem 1024-blocks      Used Available Capacity  Mounted on\n\
                   /dev/disk3s1 971350180 100000000 800000000      12%  /\n";
        assert_eq!(parse_df_avail_bytes(out), Some(800_000_000 * 1024));
        assert_eq!(parse_df_avail_bytes("only a header line\n"), None);
        assert_eq!(parse_df_avail_bytes(""), None);
    }

    #[test]
    fn same_volume_true_within_one_filesystem() {
        let tmp = TempDir::new();
        let a = tmp.file("a.txt", "x");
        let b = tmp.dir("sub");
        // Both live under the same temp dir, hence the same device.
        assert!(same_volume(&a, &b));
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
