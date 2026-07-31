use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::{
    ffi::{CString, OsStr},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::ffi::OsStrExt,
    os::unix::fs::OpenOptionsExt,
    path::Component,
};

const MAX_ATTESTATION_BYTES: u64 = 256 * 1024;

pub(super) enum AttestationReadError {
    Missing(String),
    Invalid(String),
}

pub(super) struct ArtifactDirectory {
    root: PathBuf,
    #[cfg(unix)]
    directory: File,
}

impl ArtifactDirectory {
    pub(super) fn prepare(root: &Path) -> Result<Self, String> {
        #[cfg(not(unix))]
        match std::fs::symlink_metadata(root) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "native QA output must not be a symlink: {}",
                    root.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "native QA output is not a directory: {}",
                    root.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(root)
                    .map_err(|error| format!("could not create native QA output: {error}"))?;
            }
            Err(error) => {
                return Err(format!(
                    "could not inspect native QA output {}: {error}",
                    root.display()
                ));
            }
        }

        #[cfg(unix)]
        let directory = open_private_output_directory(root)?;

        let output = Self {
            root: root.to_path_buf(),
            #[cfg(unix)]
            directory,
        };
        for name in ["evidence.json", "manual-attestation.template.json"] {
            output.remove_stale(name)?;
        }
        Ok(output)
    }

    pub(super) fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub(super) fn write_json(&self, name: &str, value: &impl Serialize) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
            format!("could not serialize {}: {error}", self.path(name).display())
        })?;
        self.write_atomic(name, &bytes)
    }

    fn remove_stale(&self, name: &str) -> Result<(), String> {
        #[cfg(unix)]
        {
            let name = artifact_name(name)?;
            if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(format!(
                    "could not remove stale native QA artifact: {error}"
                ))
            }
        }

        #[cfg(not(unix))]
        {
            let path = self.path(name);
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!(
                    "could not remove stale native QA artifact {}: {error}",
                    path.display()
                )),
            }
        }
    }

    #[cfg(unix)]
    fn write_atomic(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

        let destination = artifact_name(name)?;
        let temp_name = format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        );
        let temp = artifact_name(&temp_name)?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                temp.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(format!(
                "could not create private native QA artifact: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let write_result = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = write_result {
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), temp.as_ptr(), 0);
            }
            return Err(format!("could not write native QA artifact: {error}"));
        }
        if unsafe {
            libc::renameat(
                self.directory.as_raw_fd(),
                temp.as_ptr(),
                self.directory.as_raw_fd(),
                destination.as_ptr(),
            )
        } != 0
        {
            let error = std::io::Error::last_os_error();
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), temp.as_ptr(), 0);
            }
            return Err(format!("could not publish native QA artifact: {error}"));
        }
        if unsafe { libc::fsync(self.directory.as_raw_fd()) } != 0 {
            return Err(format!(
                "could not make native QA artifact rename durable: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn write_atomic(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

        let destination = self.path(name);
        let temp = self.path(&format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("could not create {}: {error}", temp.display()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("could not write {}: {error}", temp.display()))?;
        std::fs::rename(&temp, &destination)
            .map_err(|error| format!("could not publish {}: {error}", destination.display()))
    }
}

pub(super) fn read_attestation(path: &Path) -> Result<Vec<u8>, AttestationReadError> {
    #[cfg(unix)]
    let open = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path);
    #[cfg(not(unix))]
    let open = OpenOptions::new().read(true).open(path);
    let mut file = open.map_err(|error| {
        let detail = format!("could not securely open {}: {error}", path.display());
        if error.kind() == std::io::ErrorKind::NotFound {
            AttestationReadError::Missing(detail)
        } else {
            AttestationReadError::Invalid(detail)
        }
    })?;

    let metadata = file.metadata().map_err(|error| {
        AttestationReadError::Invalid(format!("could not inspect {}: {error}", path.display()))
    })?;
    if !metadata.is_file() {
        return Err(AttestationReadError::Invalid(format!(
            "attestation is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_ATTESTATION_BYTES {
        return Err(AttestationReadError::Invalid(format!(
            "attestation exceeds {MAX_ATTESTATION_BYTES} bytes: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_ATTESTATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            AttestationReadError::Invalid(format!("could not read {}: {error}", path.display()))
        })?;
    if bytes.len() as u64 > MAX_ATTESTATION_BYTES {
        return Err(AttestationReadError::Invalid(format!(
            "attestation grew beyond {MAX_ATTESTATION_BYTES} bytes while reading: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn open_private_output_directory(root: &Path) -> Result<File, String> {
    let normal_components = root
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count();
    if normal_components == 0
        || root
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(format!(
            "native QA output path is not a bounded directory: {}",
            root.display()
        ));
    }

    let final_component = root.file_name().ok_or_else(|| {
        format!(
            "native QA output has no creatable component: {}",
            root.display()
        )
    })?;
    let mut missing = vec![final_component.to_os_string()];
    let mut existing = root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_existing = loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => {
                let canonical = std::fs::canonicalize(existing).map_err(|error| {
                    format!(
                        "could not resolve native QA output ancestor {}: {error}",
                        existing.display()
                    )
                })?;
                if !std::fs::metadata(&canonical).is_ok_and(|metadata| metadata.is_dir()) {
                    return Err(format!(
                        "native QA output ancestor is not a directory: {}",
                        existing.display()
                    ));
                }
                break canonical;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or_else(|| {
                    format!(
                        "native QA output has no creatable component: {}",
                        root.display()
                    )
                })?;
                missing.push(name.to_os_string());
                existing = existing.parent().ok_or_else(|| {
                    format!(
                        "native QA output has no existing ancestor: {}",
                        root.display()
                    )
                })?;
            }
            Err(error) => {
                return Err(format!(
                    "could not inspect native QA output {}: {error}",
                    existing.display()
                ));
            }
        }
    };

    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&canonical_existing)
        .map_err(|error| {
            format!(
                "could not securely open native QA output ancestor {}: {error}",
                canonical_existing.display()
            )
        })?;
    for component in missing.iter().rev() {
        let name = path_component(component)?;
        if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(format!(
                    "could not create private native QA output component: {error}"
                ));
            }
        }
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(format!(
                "could not securely open native QA output component: {}",
                std::io::Error::last_os_error()
            ));
        }
        directory = unsafe { File::from_raw_fd(fd) };
    }
    if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
        return Err(format!(
            "could not make native QA output private: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(directory)
}

#[cfg(unix)]
fn artifact_name(name: &str) -> Result<CString, String> {
    if name.is_empty() || name.as_bytes().contains(&b'/') {
        return Err(format!("invalid native QA artifact name: {name:?}"));
    }
    CString::new(name.as_bytes()).map_err(|_| "artifact name contains NUL".to_string())
}

#[cfg(unix)]
fn path_component(name: &OsStr) -> Result<CString, String> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.contains(&b'/') {
        return Err("invalid native QA output path component".to_string());
    }
    CString::new(bytes).map_err(|_| "native QA output path contains NUL".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "commander-native-qa-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[cfg(unix)]
    #[test]
    fn output_rejects_root_symlinks_and_replaces_leaf_symlinks() {
        use std::os::unix::fs::{MetadataExt, symlink};

        let root = temp_root("atomic");
        let victim_root = temp_root("victim");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&victim_root);
        std::fs::create_dir_all(&victim_root).unwrap();
        symlink(&victim_root, &root).unwrap();
        assert!(ArtifactDirectory::prepare(&root).is_err());
        std::fs::remove_file(&root).unwrap();
        assert!(ArtifactDirectory::prepare(&root.join("../escape")).is_err());

        let output = ArtifactDirectory::prepare(&root).unwrap();
        assert_eq!(std::fs::metadata(&root).unwrap().mode() & 0o777, 0o700);
        assert!(
            output
                .write_json("../escape.json", &serde_json::json!({}))
                .is_err()
        );
        let victim = victim_root.join("victim.json");
        std::fs::write(&victim, b"untouched").unwrap();
        symlink(&victim, root.join("evidence.json")).unwrap();
        output
            .write_json("evidence.json", &serde_json::json!({"verdict": "blocked"}))
            .unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"untouched");
        assert!(
            std::fs::symlink_metadata(root.join("evidence.json"))
                .unwrap()
                .is_file()
        );
        assert_eq!(
            std::fs::metadata(root.join("evidence.json"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );

        let attestation_link = root.join("attestation-link.json");
        symlink(root.join("evidence.json"), &attestation_link).unwrap();
        assert!(matches!(
            read_attestation(&attestation_link),
            Err(AttestationReadError::Invalid(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(victim_root).unwrap();
    }
}
