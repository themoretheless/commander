//! Shared filesystem helpers used across panels, transfers and menus.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static STORAGE_ROOT_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

#[cfg(feature = "visual-qa")]
pub fn install_storage_root_override(path: PathBuf) -> Result<(), PathBuf> {
    STORAGE_ROOT_OVERRIDE.set(path)
}

pub fn storage_root_override() -> Option<&'static Path> {
    STORAGE_ROOT_OVERRIDE.get().map(PathBuf::as_path)
}

/// The app's config directory (created if missing), where persisted state
/// (session, smart folders) lives. Falls back to the cache dir, then `/tmp`.
pub fn config_dir() -> PathBuf {
    let preferred = storage_root_override().map_or_else(
        || {
            dirs::config_dir()
                .or_else(dirs::cache_dir)
                .unwrap_or_else(std::env::temp_dir)
                .join("commander")
        },
        |root| root.join("config"),
    );
    if let Some(dir) = ensure_private_directory(&preferred) {
        return dir;
    }
    #[cfg(unix)]
    let fallback = std::env::temp_dir().join(format!(
        "commander-{}",
        // SAFETY: `geteuid` has no preconditions and does not retain pointers.
        unsafe { libc::geteuid() }
    ));
    #[cfg(not(unix))]
    let fallback = std::env::temp_dir().join(format!("commander-{}", std::process::id()));
    if let Some(dir) = ensure_private_directory(&fallback) {
        return dir;
    }
    #[cfg(unix)]
    {
        PathBuf::from("/dev/null/commander")
    }
    #[cfg(not(unix))]
    {
        fallback
    }
}

fn ensure_private_directory(path: &Path) -> Option<PathBuf> {
    std::fs::create_dir_all(path).ok()?;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        // SAFETY: `geteuid` has no preconditions and does not retain pointers.
        if metadata.uid() != unsafe { libc::geteuid() } {
            return None;
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some(path.to_path_buf())
}

/// Atomically write `contents` to `path`: write a sibling temp file, then
/// rename it over the destination, so a crash or concurrent reader mid-write
/// never sees a truncated file. Best-effort; returns whether it succeeded.
pub fn write_atomic(path: &Path, contents: &str) -> bool {
    match crate::persistence::commit_bytes_atomic(path, contents.as_bytes()) {
        Ok(crate::persistence::AtomicWriteOutcome::Durable) => true,
        Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(error)) => {
            crate::persistence::record_durability_warning("Legacy store", &error);
            true
        }
        Err(_) => false,
    }
}

/// Flush a pathname mutation in `path`'s parent directory before a durable
/// journal record is allowed to claim that the mutation survived a crash.
pub fn sync_parent_namespace(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Ok(())
    }
}

/// Total size in bytes of all files under `path` (parallel walk).
pub fn dir_size_recursive(path: &Path) -> u64 {
    jwalk::WalkDir::new(path)
        .skip_hidden(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| !m.is_dir())
        .map(|m| m.len())
        .fold(0_u64, u64::saturating_add)
}

/// Recursively copy a directory tree into a new destination. Directory
/// symlinks are recreated rather than traversed, and a failed copy removes
/// only the destination root created by this call.
pub fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir(dst)?;
    fill_new_dir(src, dst)
}

/// Copy `src`'s contents into the just-created, empty `dst`, removing `dst`
/// again if the copy fails part-way.
fn fill_new_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Err(copy_error) = copy_dir_contents(src, dst) {
        return match std::fs::remove_dir_all(dst) {
            Ok(()) => Err(copy_error),
            Err(cleanup_error) => Err(std::io::Error::new(
                copy_error.kind(),
                format!(
                    "{copy_error}; partial copy preserved at {} because cleanup failed: {cleanup_error}",
                    dst.display()
                ),
            )),
        };
    }
    Ok(())
}

fn copy_dir_contents(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let destination = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            copy_symlink(&source, &destination)?;
        } else if file_type.is_dir() {
            copy_dir_all(&source, &destination)?;
        } else if file_type.is_file() {
            std::fs::copy(source, destination)?;
        }
        // FIFOs, sockets and devices are skipped: opening a FIFO blocks
        // until a writer appears, and the others cannot be byte-copied.
    }
    Ok(())
}

/// Recreate the symlink `src` at `dst` without following it.
pub fn copy_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(src)?;
    // Windows needs to know whether the link points at a directory. Elsewhere
    // the answer is unused, so skip the following stat, which can block on a
    // dead network mount behind the link.
    #[cfg(windows)]
    let directory = std::fs::metadata(src).is_ok_and(|metadata| metadata.is_dir());
    #[cfg(not(windows))]
    let directory = false;
    create_symlink(&target, dst, directory)
}

/// Create a symlink at `destination` pointing to `target`.
#[cfg(unix)]
pub fn create_symlink(target: &Path, destination: &Path, _directory: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
pub fn create_symlink(target: &Path, destination: &Path, directory: bool) -> std::io::Result<()> {
    if directory {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn create_symlink(
    _target: &Path,
    _destination: &Path,
    _directory: bool,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks are unsupported on this platform",
    ))
}

/// True when `dest` is the same path as `src` or lives inside `src`'s subtree.
///
/// Used to reject destructive transfers: copying or moving a directory into
/// itself or its own subtree, or a file onto itself. Resolves symlinks and
/// `.`/`..` via `canonicalize`, canonicalizing `dest`'s existing parent since
/// `dest` itself may not exist yet.
///
/// Errs on the side of refusal: a lexical prefix check can be defeated by a
/// symlink (a `dest` outside `src` lexically but inside it once resolved), so
/// when the source cannot be resolved to a real path we refuse rather than
/// risk a destructive self-copy the check would miss. The source is always an
/// entry that was just scanned, so a canonicalize failure here means it raced
/// away and skipping it is correct anyway.
pub fn is_within_or_equal(dest: &Path, src: &Path) -> bool {
    let Ok(src_c) = src.canonicalize() else {
        return true;
    };
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
    match dest_c {
        Ok(d) => d.starts_with(&src_c),
        // Destination doesn't resolve (its parent is missing, so the copy would
        // fail regardless): compare the resolved source against the lexical
        // destination as a last resort instead of allowing it unchecked.
        Err(_) => dest.starts_with(&src_c) || dest.starts_with(src),
    }
}

/// True when `path` is occupied by anything: a file, a directory, or a symlink
/// (even a broken one). Unlike [`Path::exists`], this does not follow the final
/// symlink, so a name pointing at a missing target still counts as taken. Used
/// for conflict detection, where the question is "is this name free to write?"
/// not "does the target resolve?".
pub fn path_is_taken(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// First path produced by `candidate` that doesn't exist yet.
/// `candidate(0)` is the preferred name, `candidate(n)` the n-th fallback.
pub fn first_available(mut candidate: impl FnMut(usize) -> PathBuf) -> PathBuf {
    let mut i = 0;
    loop {
        let p = candidate(i);
        // `path_is_taken` (no-follow), not `exists`: a name held by a broken
        // symlink is still taken, and the EXCL write/rename that follows would
        // fail on it, so skip to the next candidate rather than return it.
        if !path_is_taken(&p) {
            return p;
        }
        i += 1;
    }
}

/// The first free Finder-style "copy" name next to `path`:
/// "name copy.ext", then "name copy 2.ext", ...
/// A directory keeps its whole name as the stem ("v1.2 copy", not
/// "v1 copy.2"), and names are built as `OsString` so non-UTF-8 bytes survive.
pub fn available_copy_name(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("/"));
    let is_dir = std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir());
    let (stem, ext) = if is_dir {
        (path.file_name().unwrap_or_default(), None)
    } else {
        (path.file_stem().unwrap_or_default(), path.extension())
    };
    first_available(|i| {
        let mut name = stem.to_os_string();
        if i == 0 {
            name.push(" copy");
        } else {
            name.push(format!(" copy {}", i + 1));
        }
        if let Some(ext) = ext {
            name.push(".");
            name.push(ext);
        }
        parent.join(name)
    })
}

/// Duplicate a file or directory next to itself, Finder-style. Returns the
/// new path.
pub fn duplicate(path: &Path) -> std::io::Result<PathBuf> {
    duplicate_to_first_free(path, || available_copy_name(path))
}

/// Attempts at claiming a copy name before giving up; each lost race to a
/// concurrent creator moves on to the next free name.
const DUPLICATE_NAME_ATTEMPTS: usize = 16;

/// Claim the name exclusively (`create_new` file, `create_dir`, symlink) before
/// writing, so an entry created between the name probe and the copy is never
/// overwritten: losing that race retries with the next free name.
fn duplicate_to_first_free(
    path: &Path,
    mut next_name: impl FnMut() -> PathBuf,
) -> std::io::Result<PathBuf> {
    let file_type = std::fs::symlink_metadata(path)?.file_type();
    if !(file_type.is_symlink() || file_type.is_dir() || file_type.is_file()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "only files, folders and links can be duplicated",
        ));
    }
    let mut last_error = None;
    for _ in 0..DUPLICATE_NAME_ATTEMPTS {
        let dest = next_name();
        let claimed = if file_type.is_symlink() {
            copy_symlink(path, &dest)
        } else if file_type.is_dir() {
            std::fs::create_dir(&dest)
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&dest)
                .map(drop)
        };
        match claimed {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
                continue;
            }
            Err(error) => return Err(error),
        }
        if file_type.is_dir() {
            fill_new_dir(path, &dest)?;
        } else if file_type.is_file() {
            // The placeholder is ours; fs::copy truncates it and keeps
            // permissions/metadata as before.
            if let Err(error) = std::fs::copy(path, &dest) {
                let _ = std::fs::remove_file(&dest);
                return Err(error);
            }
        }
        return Ok(dest);
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::AlreadyExists)))
}

/// How a planned transfer consumes space on the target volume, which decides
/// how much of `need` it actually writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpClass {
    /// A move. A same-volume move is an instant rename needing no extra space;
    /// a cross-volume move copies to the target first, so it needs the full
    /// size there until the source is removed.
    Move { same_volume: bool },
    /// A byte copy (cross-volume, or a same-volume copy without clone support):
    /// needs the full size on the target.
    Copy,
}

/// Whether a planned transfer fits on the target volume.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpaceVerdict {
    /// The probe has not completed or the platform could not establish free
    /// space. Callers may continue under policy, but must not label this Fits.
    Indeterminate,
    /// Comfortably fits.
    Fits,
    /// Fits, but only by dipping into the safety reserve.
    Tight,
    /// Does not fit; short by this many bytes.
    WontFit { short_by: u64 },
}

/// Decide whether writing `need` bytes for a transfer of class `class` fits in
/// `free` bytes on the target, after subtracting `reclaim` (bytes freed by
/// overwriting existing destinations) and keeping a `reserve` safety margin.
///
/// A same-volume move needs ~0. Copies, including clone attempts, reserve the
/// full logical size because the native clone may fall back to a byte copy.
/// `free == None` is explicitly indeterminate rather than falsely labeled Fits.
pub fn space_verdict(
    need: u64,
    free: Option<u64>,
    class: OpClass,
    reclaim: u64,
    reserve: u64,
) -> SpaceVerdict {
    let need_eff = match class {
        OpClass::Move { same_volume: true } => 0,
        OpClass::Move { same_volume: false } | OpClass::Copy => need,
    };
    let need_eff = need_eff.saturating_sub(reclaim);
    let Some(free) = free else {
        return SpaceVerdict::Indeterminate;
    };
    if need_eff == 0 {
        return SpaceVerdict::Fits;
    }
    if need_eff > free {
        SpaceVerdict::WontFit {
            short_by: need_eff - free,
        }
    } else if need_eff
        .checked_add(reserve)
        .is_none_or(|with_reserve| with_reserve > free)
    {
        SpaceVerdict::Tight
    } else {
        SpaceVerdict::Fits
    }
}

/// Compress a file or directory into "<name>.zip" next to it.
/// Runs `ditto` in the background; returns once the process is spawned.
/// An existing archive is never replaced: like Finder, the name counts up
/// ("name 2.zip", "name 3.zip", ...) because `ditto` would overwrite it.
pub fn compress_to_zip(path: &Path) -> std::io::Result<()> {
    let archive = zip_archive_path(path)?;
    std::process::Command::new("ditto")
        .arg("-c")
        .arg("-k")
        .arg("--sequesterRsrc")
        .arg(path)
        .arg(&archive)
        .spawn()
        .map(|_| ())
}

fn zip_archive_path(path: &Path) -> std::io::Result<PathBuf> {
    let Some(name) = path.file_name() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no file name",
        ));
    };
    let parent = path.parent().unwrap_or(Path::new("/"));
    Ok(first_available(|i| {
        let mut archive = name.to_os_string();
        if i > 0 {
            archive.push(format!(" {}", i + 1));
        }
        archive.push(".zip");
        parent.join(archive)
    }))
}

/// Whether two regular files have the same verified BLAKE3 content hash.
/// Hashes are cached only after identity and filesystem-generation revalidation;
/// any read or validation error fails closed.
pub fn files_equal(a: &Path, b: &Path) -> bool {
    crate::verified_hash::files_equal(a, b)
}

/// Content hash of a file, streamed in chunks so large files are not loaded
/// whole. Returns `None` if the file cannot be read. Non-cryptographic (good
/// enough to confirm byte-identity for duplicate detection after a size match).
/// The first 8 bytes of BLAKE3, so the value printed in checksum reports is
/// stable across Rust releases (`DefaultHasher` is not); `io::copy` retries
/// reads interrupted by a signal instead of dropping the file.
pub fn content_hash(path: &Path) -> Option<u64> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    let digest = hasher.finalize();
    let prefix: [u8; 8] = digest.as_bytes()[..8].try_into().ok()?;
    Some(u64::from_be_bytes(prefix))
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
    fn files_equal_compares_bytes_not_just_size() {
        let tmp = TempDir::new();
        let a = tmp.file("a.bin", "hello world");
        let b = tmp.file("b.bin", "hello world");
        let c = tmp.file("c.bin", "hello WORLD"); // same length, different bytes
        let d = tmp.file("d.bin", "hello"); // shorter
        assert!(files_equal(&a, &b), "identical bytes");
        assert!(!files_equal(&a, &c), "same length, different content");
        assert!(!files_equal(&a, &d), "different length");
        assert!(
            !files_equal(&a, Path::new("/no/such/file")),
            "missing -> false"
        );
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

    #[cfg(unix)]
    #[test]
    fn copy_dir_all_preserves_directory_symlinks_without_traversing_them() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        src.file("real/inside.txt", "inside");
        std::os::unix::fs::symlink("real", src.path().join("linked")).unwrap();

        let copy = dst.path().join("copy");
        copy_dir_all(src.path(), &copy).unwrap();

        let copied_link = copy.join("linked");
        assert!(
            std::fs::symlink_metadata(&copied_link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_link(copied_link).unwrap(), Path::new("real"));
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
    fn duplicate_never_overwrites_a_name_taken_after_the_probe() {
        let tmp = TempDir::new();
        let f = tmp.file("doc.txt", "data");
        // A concurrent creator took "doc copy.txt" after the name was probed.
        let raced = tmp.file("doc copy.txt", "someone else's");
        let fallback = tmp.path().join("doc copy 2.txt");
        let mut names = vec![fallback.clone(), raced.clone()];
        let dest = duplicate_to_first_free(&f, || names.pop().unwrap()).unwrap();
        assert_eq!(dest, fallback);
        assert_eq!(std::fs::read_to_string(&raced).unwrap(), "someone else's");
        assert_eq!(std::fs::read_to_string(&fallback).unwrap(), "data");
    }

    #[test]
    fn duplicate_of_a_folder_retries_a_raced_name() {
        let tmp = TempDir::new();
        let dir = tmp.dir("src");
        tmp.file("src/a.txt", "a");
        let raced = tmp.dir("src copy");
        tmp.file("src copy/keep.txt", "keep");
        let fallback = tmp.path().join("src copy 2");
        let mut names = vec![fallback.clone(), raced.clone()];
        let dest = duplicate_to_first_free(&dir, || names.pop().unwrap()).unwrap();
        assert_eq!(dest, fallback);
        assert!(!raced.join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(fallback.join("a.txt")).unwrap(),
            "a"
        );
    }

    #[test]
    fn content_hash_is_a_stable_blake3_prefix() {
        let tmp = TempDir::new();
        let a = tmp.file("a.bin", "hello world");
        let expected = &blake3::hash(b"hello world").to_hex()[..16];
        assert_eq!(format!("{:016x}", content_hash(&a).unwrap()), expected);
    }

    #[test]
    fn duplicate_keeps_dotted_folder_names_whole() {
        let tmp = TempDir::new();
        let dir = tmp.dir("v1.2");

        assert_eq!(duplicate(&dir).unwrap(), tmp.path().join("v1.2 copy"));
        assert_eq!(duplicate(&dir).unwrap(), tmp.path().join("v1.2 copy 2"));
    }

    #[cfg(unix)]
    #[test]
    fn duplicate_keeps_non_utf8_name_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let tmp = TempDir::new();
        let file = tmp.path().join(std::ffi::OsStr::from_bytes(b"caf\xe9.txt"));
        std::fs::write(&file, "x").unwrap();

        let copy = duplicate(&file).unwrap();

        assert_eq!(
            copy.file_name().unwrap().to_os_string().into_vec(),
            b"caf\xe9 copy.txt".to_vec()
        );
    }

    #[cfg(unix)]
    #[test]
    fn duplicate_skips_fifos_inside_folders() {
        let tmp = TempDir::new();
        let dir = tmp.dir("pipes");
        tmp.file("pipes/data.txt", "x");
        let fifo = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(
            dir.join("queue").as_os_str(),
        ))
        .unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(duplicate(&dir).map_err(|error| error.to_string()));
        });

        let copy = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("duplicate blocked on a FIFO")
            .unwrap();

        assert_eq!(std::fs::read_to_string(copy.join("data.txt")).unwrap(), "x");
        assert!(!path_is_taken(&copy.join("queue")));
    }

    #[test]
    fn zip_archive_name_counts_up_instead_of_replacing() {
        let tmp = TempDir::new();
        let dir = tmp.dir("photos");
        assert_eq!(
            zip_archive_path(&dir).unwrap(),
            tmp.path().join("photos.zip")
        );
        tmp.file("photos.zip", "old");
        assert_eq!(
            zip_archive_path(&dir).unwrap(),
            tmp.path().join("photos 2.zip")
        );
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
    fn write_atomic_replaces_contents_and_leaves_no_temp() {
        let tmp = TempDir::new();
        let path = tmp.path().join("data.json");
        assert!(write_atomic(&path, "first"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        // A second write replaces, not appends.
        assert!(write_atomic(&path, "second"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        // No stray temp sibling is left behind.
        let mut temp = path.as_os_str().to_owned();
        temp.push(".tmp");
        assert!(!Path::new(&temp).exists());
    }

    #[cfg(unix)]
    #[test]
    fn private_directory_rejects_symlinks_and_uses_owner_only_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let tmp = TempDir::new();
        let directory = tmp.path().join("private");
        assert_eq!(
            ensure_private_directory(&directory).as_deref(),
            Some(directory.as_path())
        );
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let target = tmp.dir("target");
        let link = tmp.path().join("link");
        symlink(target, &link).unwrap();
        assert!(ensure_private_directory(&link).is_none());
    }

    #[test]
    fn space_verdict_classes_and_boundaries() {
        use OpClass::*;
        use SpaceVerdict::*;

        // A same-volume move needs ~0, so it fits even when size dwarfs free.
        assert_eq!(
            space_verdict(1_000, Some(10), Move { same_volume: true }, 0, 0),
            Fits
        );

        // Cross-volume move and plain copy need the full size.
        assert_eq!(
            space_verdict(1_000, Some(10), Move { same_volume: false }, 0, 0),
            WontFit { short_by: 990 }
        );
        assert_eq!(
            space_verdict(100, Some(40), Copy, 0, 0),
            WontFit { short_by: 60 }
        );
        assert_eq!(space_verdict(40, Some(100), Copy, 0, 0), Fits);

        // Overwrites reclaim space, lowering the effective need.
        assert_eq!(space_verdict(100, Some(40), Copy, 70, 0), Fits);
        assert_eq!(
            space_verdict(100, Some(40), Copy, 30, 0),
            WontFit { short_by: 30 }
        );

        // A reserve pushes a just-fits copy into Tight, then WontFit.
        assert_eq!(space_verdict(100, Some(100), Copy, 0, 0), Fits);
        assert_eq!(space_verdict(100, Some(100), Copy, 0, 10), Tight);
        assert_eq!(
            space_verdict(100, Some(100), Copy, 0, 0),
            Fits,
            "exact fit with no reserve is Fits, not WontFit"
        );
        assert_eq!(
            space_verdict(u64::MAX, Some(u64::MAX), Copy, 0, 1),
            Tight,
            "reserve overflow cannot be mislabeled Fits"
        );

        // Unknown free space is non-blocking policy, but never mislabeled Fits.
        assert_eq!(space_verdict(u64::MAX, None, Copy, 0, 0), Indeterminate);
        assert_eq!(
            space_verdict(u64::MAX, None, Move { same_volume: true }, 0, 0),
            Indeterminate
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

    #[test]
    #[cfg(unix)]
    fn is_within_or_equal_sees_through_a_symlinked_destination() {
        // `link` points back into `a`, so `link/new` is really `a/new` even
        // though it is not a lexical prefix of `a`. Canonicalization must catch
        // it; a naive string check would not.
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        std::os::unix::fs::symlink(&a, tmp.path().join("link")).unwrap();
        assert!(is_within_or_equal(&tmp.path().join("link").join("new"), &a));
    }

    #[test]
    fn is_within_or_equal_refuses_when_source_cannot_be_resolved() {
        // A source that does not exist cannot be reasoned about; refuse rather
        // than fall back to an unverifiable lexical comparison.
        let tmp = TempDir::new();
        let missing = tmp.path().join("gone");
        assert!(is_within_or_equal(&tmp.path().join("dest"), &missing));
    }
}
