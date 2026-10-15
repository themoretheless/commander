//! Descriptor-relative (`*at`) filesystem helpers.
//!
//! After a directory binding is proven (`observe` + open with
//! `O_DIRECTORY | O_NOFOLLOW`), subsequent create/write/rename/remove
//! effects use the held dirfd and a single relative name component instead
//! of re-resolving absolute paths.

use crate::ports::FileSystemProvider;
use std::ffi::{CString, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd, RawFd},
    unix::{ffi::OsStrExt, fs::OpenOptionsExt},
};

/// A directory held open by descriptor after a proven binding.
///
/// Relative names passed to `*at` operations must be a single path component
/// (no `/`, `.`, or `..`).
#[derive(Debug)]
pub struct BoundDirectory {
    root: PathBuf,
    #[cfg(unix)]
    directory: File,
}

impl BoundDirectory {
    /// Open `path` as a no-follow directory descriptor.
    ///
    /// Callers should [`observe`](crate::ports::FileSystemProvider::observe)
    /// the path first; this open is the proven binding that later `*at`
    /// effects reuse.
    pub fn open(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let directory = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)?;
            Ok(Self {
                root: path.to_path_buf(),
                directory,
            })
        }

        #[cfg(not(unix))]
        {
            let metadata = std::fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "bound path is not a directory",
                ));
            }
            Ok(Self {
                root: path.to_path_buf(),
            })
        }
    }

    /// Observe `path` through `provider`, then open a descriptor-relative
    /// binding when the observation still names an existing directory.
    pub fn bind(provider: &dyn crate::ports::FileSystemProvider, path: &Path) -> io::Result<Self> {
        let identity = provider.observe(path)?;
        if !identity.exists || identity.kind != Some(crate::path_identity::PathKind::Directory) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("cannot bind non-directory path {}", path.display()),
            ));
        }
        Self::open(path)
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    #[cfg(unix)]
    pub fn as_raw_fd(&self) -> RawFd {
        self.directory.as_raw_fd()
    }

    pub fn create_directory(&self, name: &OsStr) -> io::Result<()> {
        #[cfg(unix)]
        {
            let name = relative_name(name)?;
            if unsafe { libc::mkdirat(self.directory.as_raw_fd(), name.as_ptr(), 0o755) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        #[cfg(not(unix))]
        {
            std::fs::create_dir(self.root.join(name))
        }
    }

    pub fn write_file(&self, name: &OsStr, bytes: &[u8]) -> io::Result<()> {
        #[cfg(unix)]
        {
            let name = relative_name(name)?;
            let fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_TRUNC
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o644,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut file = unsafe { File::from_raw_fd(fd) };
            file.write_all(bytes)?;
            Ok(())
        }

        #[cfg(not(unix))]
        {
            let mut file = File::create(self.root.join(name))?;
            file.write_all(bytes)
        }
    }

    pub fn rename(&self, source: &OsStr, destination: &OsStr, replace: bool) -> io::Result<()> {
        #[cfg(unix)]
        {
            let source = relative_name(source)?;
            let destination = relative_name(destination)?;
            if replace {
                renameat_replace(self.directory.as_raw_fd(), &source, &destination)
            } else {
                renameat_noreplace(self.directory.as_raw_fd(), &source, &destination)
            }
        }

        #[cfg(not(unix))]
        {
            let source = self.root.join(source);
            let destination = self.root.join(destination);
            if replace {
                std::fs::rename(source, destination)
            } else {
                rename_noreplace(&source, &destination)
            }
        }
    }

    pub fn remove(&self, name: &OsStr) -> io::Result<()> {
        #[cfg(unix)]
        {
            let c_name = relative_name(name)?;
            // Prefer unlinking a non-directory entry; fall back to rmdir for
            // empty directories. Recursive trees stay on the path-based port.
            if unsafe { libc::unlinkat(self.directory.as_raw_fd(), c_name.as_ptr(), 0) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EISDIR)
                || error.raw_os_error() == Some(libc::EPERM)
            {
                if unsafe {
                    libc::unlinkat(
                        self.directory.as_raw_fd(),
                        c_name.as_ptr(),
                        libc::AT_REMOVEDIR,
                    )
                } == 0
                {
                    return Ok(());
                }
                let dir_error = io::Error::last_os_error();
                if dir_error.kind() == io::ErrorKind::NotFound {
                    return Ok(());
                }
                return Err(remove_retry_error(error, dir_error));
            }
            if error.kind() == io::ErrorKind::NotFound {
                return Ok(());
            }
            Err(error)
        }

        #[cfg(not(unix))]
        {
            let path = self.root.join(name);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => std::fs::remove_dir(&path),
                Ok(_) => std::fs::remove_file(&path),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        }
    }
}

/// Pick the error to report when `unlinkat` and its `AT_REMOVEDIR` retry both
/// fail. The EPERM retry exists for macOS, which reports EPERM when unlinking
/// a directory; for a real non-directory the retry only adds ENOTDIR, which
/// would hide the permission problem (sticky folder, immutable file).
#[cfg(unix)]
fn remove_retry_error(unlink_error: io::Error, rmdir_error: io::Error) -> io::Error {
    if unlink_error.raw_os_error() == Some(libc::EPERM)
        && rmdir_error.raw_os_error() == Some(libc::ENOTDIR)
    {
        unlink_error
    } else {
        rmdir_error
    }
}

/// Single-component relative name used with a [`BoundDirectory`].
pub fn relative_name(name: &OsStr) -> io::Result<CString> {
    #[cfg(unix)]
    {
        let bytes = name.as_bytes();
        if bytes.is_empty()
            || bytes.contains(&b'/')
            || bytes == b"."
            || bytes == b".."
            || bytes.contains(&0)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "relative filesystem effect name must be a single path component",
            ));
        }
        CString::new(bytes.to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
    }

    #[cfg(not(unix))]
    {
        let text = name.to_string_lossy();
        if text.is_empty()
            || text.contains('/')
            || text.contains('\\')
            || text == "."
            || text == ".."
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "relative filesystem effect name must be a single path component",
            ));
        }
        CString::new(text.into_owned())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
    }
}

#[cfg(unix)]
fn renameat_replace(dirfd: RawFd, source: &CString, destination: &CString) -> io::Result<()> {
    if unsafe { libc::renameat(dirfd, source.as_ptr(), dirfd, destination.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn renameat_noreplace(dirfd: RawFd, source: &CString, destination: &CString) -> io::Result<()> {
    match exclusive_renameat(dirfd, source, destination) {
        Ok(()) => Ok(()),
        Err(error) if is_exclusive_rename_unsupported(&error) => {
            renameat_noreplace_fallback(dirfd, source, destination)
        }
        Err(error) => Err(error),
    }
}

#[cfg(all(unix, target_os = "macos"))]
fn exclusive_renameat(dirfd: RawFd, source: &CString, destination: &CString) -> io::Result<()> {
    if unsafe {
        libc::renameatx_np(
            dirfd,
            source.as_ptr(),
            dirfd,
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(unix, target_os = "linux"))]
fn exclusive_renameat(dirfd: RawFd, source: &CString, destination: &CString) -> io::Result<()> {
    if unsafe {
        libc::renameat2(
            dirfd,
            source.as_ptr(),
            dirfd,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn exclusive_renameat(dirfd: RawFd, source: &CString, destination: &CString) -> io::Result<()> {
    let _ = (dirfd, source, destination);
    Err(io::Error::from_raw_os_error(libc::ENOTSUP))
}

#[cfg(unix)]
fn is_exclusive_rename_unsupported(error: &io::Error) -> bool {
    error.raw_os_error().is_some_and(|code| {
        code == libc::ENOTSUP
            || code == libc::EOPNOTSUPP
            // Linux reports a filesystem without RENAME_NOREPLACE (NFS, some
            // FUSE) as EINVAL, and a kernel or sandbox without renameat2 as
            // ENOSYS. A genuine EINVAL (a folder into itself) resurfaces from
            // the fallback's plain rename.
            || (cfg!(target_os = "linux") && (code == libc::EINVAL || code == libc::ENOSYS))
    })
}

#[cfg(unix)]
fn renameat_noreplace_fallback(
    dirfd: RawFd,
    source: &CString,
    destination: &CString,
) -> io::Result<()> {
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    let rc = unsafe {
        libc::fstatat(
            dirfd,
            destination.as_ptr(),
            st.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc == 0 {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    let probe = io::Error::last_os_error();
    if probe.kind() != io::ErrorKind::NotFound {
        return Err(probe);
    }
    renameat_replace(dirfd, source, destination)
}

/// Rename `from` to `to` without ever replacing an existing destination.
///
/// The one no-clobber rename for whole paths: the OS exclusive rename
/// (`renameatx_np(RENAME_EXCL)` on macOS, `renameat2(RENAME_NOREPLACE)` on
/// Linux), falling back to check-then-rename where the volume cannot honor it.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let from = CString::new(from.as_os_str().as_bytes())?;
        let to = CString::new(to.as_os_str().as_bytes())?;
        renameat_noreplace(libc::AT_FDCWD, &from, &to)
    }
    #[cfg(not(unix))]
    {
        if crate::fs_util::path_is_taken(to) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination already exists",
            ));
        }
        std::fs::rename(from, to)
    }
}

/// Leaf name of `path` suitable for a relative `*at` effect.
pub fn leaf_name(path: &Path) -> io::Result<OsString> {
    path.file_name().map(OsStr::to_os_string).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no leaf name: {}", path.display()),
        )
    })
}

/// Rename `from` → `to` under a proven parent binding when both share a parent.
///
/// Same-directory placement (staging → landing, overwrite quarantine, undo)
/// stays descriptor-relative. Cross-directory callers fall back to the
/// path-based exclusive rename.
pub fn rename_sibling(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    let from_parent = from.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("source has no parent: {}", from.display()),
        )
    })?;
    let to_parent = to.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("destination has no parent: {}", to.display()),
        )
    })?;
    if from_parent != to_parent {
        return if replace {
            std::fs::rename(from, to)
        } else {
            rename_noreplace(from, to)
        };
    }
    // Shallow bind: the no-follow directory open is the proof we need. A
    // deep observe would walk the whole destination folder and fail with
    // WouldBlock whenever anything else writes into it mid-walk (or on any
    // unreadable subfolder), aborting an otherwise fine placement.
    let provider = crate::ports::NativeFileSystemProvider;
    let bound = BoundDirectory::open(from_parent)?;
    provider.apply_at(
        &bound,
        &crate::ports::RelativeFileSystemEffect::Rename {
            source: leaf_name(from)?,
            destination: leaf_name(to)?,
            replace,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::NativeFileSystemProvider;
    use crate::testutil::TempDir;

    #[test]
    fn rename_noreplace_moves_into_a_free_name() {
        let tmp = TempDir::new();
        let src = tmp.file("a.txt", "hi");
        let dst = tmp.path().join("b.txt");
        rename_noreplace(&src, &dst).unwrap();
        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hi");
    }

    #[test]
    fn rename_noreplace_refuses_to_clobber_an_existing_destination() {
        let tmp = TempDir::new();
        let src = tmp.file("a.txt", "new");
        let dst = tmp.file("b.txt", "old");
        let err = rename_noreplace(&src, &dst).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(src.exists());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn noreplace_fallback_moves_and_still_refuses_to_clobber() {
        let tmp = TempDir::new();
        let c = |path: &Path| CString::new(path.as_os_str().as_bytes()).unwrap();
        let src = tmp.file("a.txt", "new");
        let free = tmp.path().join("free.txt");
        renameat_noreplace_fallback(libc::AT_FDCWD, &c(&src), &c(&free)).unwrap();
        assert_eq!(std::fs::read_to_string(&free).unwrap(), "new");
        let taken = tmp.file("taken.txt", "old");
        let err = renameat_noreplace_fallback(libc::AT_FDCWD, &c(&free), &c(&taken)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn remove_keeps_the_permission_error_when_the_rmdir_retry_says_not_a_directory() {
        let os = io::Error::from_raw_os_error;
        let chosen = remove_retry_error(os(libc::EPERM), os(libc::ENOTDIR));
        assert_eq!(chosen.raw_os_error(), Some(libc::EPERM));
        // A directory that really failed to go keeps the rmdir reason.
        let chosen = remove_retry_error(os(libc::EPERM), os(libc::ENOTEMPTY));
        assert_eq!(chosen.raw_os_error(), Some(libc::ENOTEMPTY));
        let chosen = remove_retry_error(os(libc::EISDIR), os(libc::EACCES));
        assert_eq!(chosen.raw_os_error(), Some(libc::EACCES));
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_rename_falls_back_only_for_unsupported_codes() {
        let os = io::Error::from_raw_os_error;
        assert!(is_exclusive_rename_unsupported(&os(libc::ENOTSUP)));
        assert!(is_exclusive_rename_unsupported(&os(libc::EOPNOTSUPP)));
        assert_eq!(
            is_exclusive_rename_unsupported(&os(libc::EINVAL)),
            cfg!(target_os = "linux")
        );
        assert_eq!(
            is_exclusive_rename_unsupported(&os(libc::ENOSYS)),
            cfg!(target_os = "linux")
        );
        assert!(!is_exclusive_rename_unsupported(&os(libc::EEXIST)));
        assert!(!is_exclusive_rename_unsupported(&os(libc::EACCES)));
    }

    #[cfg(unix)]
    #[test]
    fn noreplace_fallback_still_rejects_moving_a_folder_into_itself() {
        let tmp = TempDir::new();
        let c = |path: &Path| CString::new(path.as_os_str().as_bytes()).unwrap();
        let folder = tmp.dir("folder");
        let inside = folder.join("inside");
        let err =
            renameat_noreplace_fallback(libc::AT_FDCWD, &c(&folder), &c(&inside)).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));
        assert!(folder.is_dir());
    }

    #[test]
    fn bound_directory_rejects_escape_names() {
        let temp = TempDir::new();
        let bound = BoundDirectory::open(temp.path()).unwrap();
        assert!(bound.write_file(OsStr::new("../escape"), b"x").is_err());
        assert!(bound.write_file(OsStr::new("a/b"), b"x").is_err());
        assert!(bound.write_file(OsStr::new("."), b"x").is_err());
        assert!(bound.write_file(OsStr::new(".."), b"x").is_err());
    }

    #[test]
    fn bound_directory_create_write_rename_remove_under_dirfd() {
        let temp = TempDir::new();
        let provider = NativeFileSystemProvider;
        let bound = BoundDirectory::bind(&provider, temp.path()).unwrap();

        bound.create_directory(OsStr::new("nested")).unwrap();
        let nested = BoundDirectory::bind(&provider, &temp.path().join("nested")).unwrap();
        nested
            .write_file(OsStr::new("staging"), b"payload")
            .unwrap();
        nested
            .rename(OsStr::new("staging"), OsStr::new("final"), false)
            .unwrap();
        assert_eq!(
            std::fs::read(temp.path().join("nested/final")).unwrap(),
            b"payload"
        );
        nested.remove(OsStr::new("final")).unwrap();
        assert!(!temp.path().join("nested/final").exists());
    }

    #[test]
    fn bound_directory_remove_handles_empty_dirs_full_dirs_and_missing_names() {
        let temp = TempDir::new();
        let bound = BoundDirectory::open(temp.path()).unwrap();

        temp.dir("empty");
        bound.remove(OsStr::new("empty")).unwrap();
        assert!(!temp.path().join("empty").exists());

        temp.file("full/inner.txt", "keep");
        let error = bound.remove(OsStr::new("full")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::DirectoryNotEmpty);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("full/inner.txt")).unwrap(),
            "keep"
        );

        bound.remove(OsStr::new("never-existed")).unwrap();
    }

    #[test]
    fn rename_sibling_uses_bound_parent() {
        let temp = crate::testutil::TempDir::new();
        let provider = NativeFileSystemProvider;
        let bound = BoundDirectory::bind(&provider, temp.path()).unwrap();
        bound.write_file(OsStr::new("staged"), b"payload").unwrap();
        let from = temp.path().join("staged");
        let to = temp.path().join("final");
        rename_sibling(&from, &to, false).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"payload");
        assert!(!from.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rename_sibling_ignores_unreadable_subfolders_of_the_parent() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new();
        let locked = temp.dir("locked");
        temp.file("locked/inner.txt", "x");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let from = temp.file("staged", "payload");
        let to = temp.path().join("final");

        let result = rename_sibling(&from, &to, false);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        result.unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"payload");
    }

    #[cfg(unix)]
    #[test]
    fn bound_directory_open_refuses_symlink_roots() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        let real = temp.dir("real");
        let link = temp.path().join("link");
        symlink(&real, &link).unwrap();
        assert!(BoundDirectory::open(&link).is_err());
    }
}
