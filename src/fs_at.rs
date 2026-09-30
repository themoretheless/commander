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
                crate::native_copy::rename_noreplace(&source, &destination)
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
                return Err(dir_error);
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
    error
        .raw_os_error()
        .is_some_and(|code| code == libc::ENOTSUP || code == libc::EOPNOTSUPP)
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
            crate::native_copy::rename_noreplace(from, to)
        };
    }
    let provider = crate::ports::NativeFileSystemProvider;
    let bound = BoundDirectory::bind(&provider, from_parent)?;
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
    fn bound_directory_open_refuses_symlink_roots() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        let real = temp.dir("real");
        let link = temp.path().join("link");
        symlink(&real, &link).unwrap();
        assert!(BoundDirectory::open(&link).is_err());
    }
}
