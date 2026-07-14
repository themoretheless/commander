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
    pub tree_fingerprint: Option<u64>,
}

impl PathIdentity {
    pub fn observe(path: &Path) -> std::io::Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => Ok(Self::from_metadata(path.to_path_buf(), &metadata)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::missing(path)),
            Err(error) => Err(error),
        }
    }

    /// Observe a path and, for a real directory, fingerprint every descendant's
    /// relative name and metadata. Symlinks are leaves and are never followed.
    pub fn observe_deep(path: &Path) -> std::io::Result<Self> {
        let before = Self::observe(path)?;
        if before.kind != Some(PathKind::Directory) {
            return Ok(before);
        }
        let fingerprint = tree_fingerprint(path)?;
        let mut after = Self::observe(path)?;
        if !before.same_metadata(&after) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "path changed while its identity was captured",
            ));
        }
        after.tree_fingerprint = Some(fingerprint);
        Ok(after)
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
            tree_fingerprint: None,
        }
    }

    pub fn same_version(&self, other: &Self) -> bool {
        self.same_metadata(other) && self.tree_fingerprint == other.tree_fingerprint
    }

    fn same_metadata(&self, other: &Self) -> bool {
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
            tree_fingerprint: None,
        }
    }
}

fn tree_fingerprint(root: &Path) -> std::io::Result<u64> {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut stack = vec![(PathBuf::new(), root.to_path_buf())];
    while let Some((relative, path)) = stack.pop() {
        let metadata = std::fs::symlink_metadata(&path)?;
        hash_bytes(&mut hash, relative.to_string_lossy().as_bytes());
        let file_type = metadata.file_type();
        hash_bytes(
            &mut hash,
            &[if file_type.is_symlink() {
                3
            } else if metadata.is_dir() {
                2
            } else if metadata.is_file() {
                1
            } else {
                4
            }],
        );
        hash_bytes(&mut hash, &metadata.len().to_le_bytes());
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        hash_bytes(&mut hash, &modified.to_le_bytes());
        let (volume, file_id) = native_identity(&metadata);
        hash_bytes(&mut hash, &volume.unwrap_or_default().to_le_bytes());
        hash_bytes(&mut hash, &file_id.unwrap_or_default().to_le_bytes());

        if metadata.is_dir() && !file_type.is_symlink() {
            let mut children = std::fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
            children.sort_by_key(|entry| entry.file_name());
            for entry in children.into_iter().rev() {
                let name = entry.file_name();
                stack.push((relative.join(&name), entry.path()));
            }
        }
    }
    Ok(hash)
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    *hash ^= 0xff;
    *hash = hash.wrapping_mul(0x100000001b3);
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
        assert!(existing.same_version(&PathIdentity::observe(&path).unwrap()));
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

    #[test]
    fn deep_identity_detects_a_nested_file_change() {
        let temp = TempDir::new();
        let root = temp.dir("folder");
        let nested = temp.file("folder/deep/file.txt", "one");
        let before = PathIdentity::observe_deep(&root).unwrap();
        std::fs::write(nested, "a longer value").unwrap();
        let after = PathIdentity::observe_deep(&root).unwrap();
        assert!(!before.same_version(&after));
    }
}
