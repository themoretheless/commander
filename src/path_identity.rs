//! Stable-enough filesystem observations used to reject stale plans and detect
//! changes between preflight, copy, and final placement.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathIdentity {
    pub path: PathBuf,
    pub exists: bool,
    pub kind: Option<PathKind>,
    pub volume: Option<u64>,
    pub file_id: Option<u64>,
    pub size: u64,
    pub modified_nanos: Option<u128>,
}

impl PathIdentity {
    pub fn observe(path: &Path) -> std::io::Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => Ok(Self::from_metadata(path.to_path_buf(), &metadata)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::missing(path)),
            Err(error) => Err(error),
        }
    }

    pub fn missing(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            exists: false,
            kind: None,
            volume: None,
            file_id: None,
            size: 0,
            modified_nanos: None,
        }
    }

    pub fn still_matches(&self) -> std::io::Result<bool> {
        Self::observe(&self.path).map(|current| current.same_version(self))
    }

    pub fn same_version(&self, other: &Self) -> bool {
        self.exists == other.exists
            && self.kind == other.kind
            && self.volume == other.volume
            && self.file_id == other.file_id
            && self.size == other.size
            && self.modified_nanos == other.modified_nanos
    }

    fn from_metadata(path: PathBuf, metadata: &std::fs::Metadata) -> Self {
        let file_type = metadata.file_type();
        let kind = if file_type.is_file() {
            PathKind::File
        } else if file_type.is_dir() {
            PathKind::Directory
        } else if file_type.is_symlink() {
            PathKind::Symlink
        } else {
            PathKind::Other
        };
        let (volume, file_id) = native_identity(metadata);
        Self {
            path,
            exists: true,
            kind: Some(kind),
            volume,
            file_id,
            size: metadata.len(),
            modified_nanos: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos()),
        }
    }
}

fn native_identity(metadata: &std::fs::Metadata) -> (Option<u64>, Option<u64>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return (Some(metadata.dev()), Some(metadata.ino()));
    }
    #[allow(unreachable_code)]
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn missing_and_existing_paths_have_distinct_versions() {
        let temp = TempDir::new();
        let path = temp.path().join("file.txt");
        let missing = PathIdentity::observe(&path).unwrap();
        std::fs::write(&path, "one").unwrap();
        let existing = PathIdentity::observe(&path).unwrap();
        assert!(!missing.same_version(&existing));
        assert!(existing.still_matches().unwrap());
    }

    #[test]
    fn replacement_with_same_size_is_detected_by_identity_or_version() {
        let temp = TempDir::new();
        let path = temp.path().join("file.txt");
        std::fs::write(&path, "one").unwrap();
        let before = PathIdentity::observe(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "two").unwrap();
        let after = PathIdentity::observe(&path).unwrap();
        assert!(!before.same_version(&after));
    }
}
