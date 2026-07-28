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
    /// Hash of the complete observation. Older journals default this to zero
    /// and fall back to field-by-field comparison during migration.
    #[serde(default)]
    pub observed_version: u64,
}

/// One immutable proof for bytes reached through `SymlinkPolicy::Follow`.
///
/// The lexical link itself remains part of the enclosing tree identity. This
/// companion proof binds the canonical object whose bytes will actually be
/// copied, including its full tree when it is a directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowedPathIdentity {
    pub link: PathBuf,
    pub target: PathIdentity,
}

/// Source proof used by transfer plans. `lexical` never follows symlinks;
/// `followed` binds every separately traversed target in deterministic order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSourceIdentity {
    pub lexical: PathIdentity,
    #[serde(default)]
    pub followed: Vec<FollowedPathIdentity>,
    #[serde(default)]
    pub logical_bytes: u64,
}

impl TransferSourceIdentity {
    pub fn same_binding(&self, other: &Self) -> bool {
        self.lexical.same_binding(&other.lexical)
            && self.logical_bytes == other.logical_bytes
            && self.followed.len() == other.followed.len()
            && self
                .followed
                .iter()
                .zip(&other.followed)
                .all(|(expected, current)| {
                    expected.link == current.link && expected.target.same_binding(&current.target)
                })
    }

    pub fn same_version(&self, other: &Self) -> bool {
        self.lexical.same_version(&other.lexical)
            && self.logical_bytes == other.logical_bytes
            && self.followed.len() == other.followed.len()
            && self
                .followed
                .iter()
                .zip(&other.followed)
                .all(|(expected, current)| {
                    expected.link == current.link && expected.target.same_binding(&current.target)
                })
    }
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
        after.refresh_observed_version();
        Ok(after)
    }

    pub fn missing(path: &Path) -> Self {
        let mut identity = Self {
            path: path.to_path_buf(),
            exists: false,
            kind: None,
            volume: None,
            file_id: None,
            size: 0,
            modified_nanos: None,
            tree_fingerprint: None,
            observed_version: 0,
        };
        identity.refresh_observed_version();
        identity
    }

    pub fn same_version(&self, other: &Self) -> bool {
        if self.observed_version != 0 && other.observed_version != 0 {
            self.observed_version == other.observed_version
        } else {
            self.same_metadata(other) && self.tree_fingerprint == other.tree_fingerprint
        }
    }

    /// Compare both the pathname binding and the observed object version.
    ///
    /// `same_version` deliberately ignores the path so an object can be
    /// followed across a rename. Journal contracts that refer to a particular
    /// namespace entry must use this stricter comparison instead.
    pub fn same_binding(&self, other: &Self) -> bool {
        self.path == other.path && self.same_version(other)
    }

    /// Compare a lexical listing observation with another shallow observation.
    /// Tree fingerprints are deliberately ignored so the identity captured by
    /// a directory listing can bind the root of a later deep scan.
    pub(crate) fn same_shallow_binding(&self, other: &Self) -> bool {
        self.path == other.path && self.same_metadata(other)
    }

    /// Prove that two observations refer to the same filesystem object even
    /// when it has been atomically renamed to a quarantine path.
    pub fn same_object(&self, other: &Self) -> bool {
        if !self.exists || !other.exists || self.kind != other.kind {
            return false;
        }
        match (
            self.volume.zip(self.file_id),
            other.volume.zip(other.file_id),
        ) {
            (Some(expected), Some(current)) => expected == current,
            _ => self.same_version(other),
        }
    }

    pub(crate) fn same_metadata(&self, other: &Self) -> bool {
        self.exists == other.exists
            && self.kind == other.kind
            && self.volume == other.volume
            && self.file_id == other.file_id
            && self.size == other.size
            && self.modified_nanos == other.modified_nanos
    }

    pub(crate) fn with_tree_fingerprint(mut self, fingerprint: u64) -> Self {
        self.tree_fingerprint = Some(fingerprint);
        self.refresh_observed_version();
        self
    }

    pub(crate) fn from_metadata(path: PathBuf, metadata: &std::fs::Metadata) -> Self {
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
        let mut identity = Self {
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
            observed_version: 0,
        };
        identity.refresh_observed_version();
        identity
    }

    fn refresh_observed_version(&mut self) {
        let mut hash = 0xcbf29ce484222325_u64;
        hash_bytes(&mut hash, &[u8::from(self.exists)]);
        hash_bytes(
            &mut hash,
            &[match self.kind {
                None => 0,
                Some(PathKind::File) => 1,
                Some(PathKind::Directory) => 2,
                Some(PathKind::Symlink) => 3,
                Some(PathKind::Other) => 4,
            }],
        );
        hash_bytes(&mut hash, &self.volume.unwrap_or_default().to_le_bytes());
        hash_bytes(&mut hash, &self.file_id.unwrap_or_default().to_le_bytes());
        hash_bytes(&mut hash, &self.size.to_le_bytes());
        hash_bytes(
            &mut hash,
            &self.modified_nanos.unwrap_or_default().to_le_bytes(),
        );
        hash_bytes(
            &mut hash,
            &self.tree_fingerprint.unwrap_or_default().to_le_bytes(),
        );
        self.observed_version = hash.max(1);
    }
}

pub(crate) struct TreeFingerprint {
    hash: u64,
}

impl TreeFingerprint {
    pub(crate) fn new() -> Self {
        Self {
            hash: 0xcbf29ce484222325_u64,
        }
    }

    pub(crate) fn record(&mut self, relative: &Path, metadata: &std::fs::Metadata) {
        hash_bytes(&mut self.hash, relative.to_string_lossy().as_bytes());
        let file_type = metadata.file_type();
        hash_bytes(
            &mut self.hash,
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
        hash_bytes(&mut self.hash, &metadata.len().to_le_bytes());
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        hash_bytes(&mut self.hash, &modified.to_le_bytes());
        let (volume, file_id) = native_identity(metadata);
        hash_bytes(&mut self.hash, &volume.unwrap_or_default().to_le_bytes());
        hash_bytes(&mut self.hash, &file_id.unwrap_or_default().to_le_bytes());
    }

    pub(crate) fn finish(self) -> u64 {
        self.hash
    }
}

fn tree_fingerprint(root: &Path) -> std::io::Result<u64> {
    let mut fingerprint = TreeFingerprint::new();
    let mut stack = vec![(PathBuf::new(), root.to_path_buf())];
    while let Some((relative, path)) = stack.pop() {
        let metadata = std::fs::symlink_metadata(&path)?;
        fingerprint.record(&relative, &metadata);
        let file_type = metadata.file_type();

        if metadata.is_dir() && !file_type.is_symlink() {
            let mut children = std::fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
            children.sort_by_key(|entry| entry.file_name());
            for entry in children.into_iter().rev() {
                let name = entry.file_name();
                stack.push((relative.join(&name), entry.path()));
            }
        }
    }
    Ok(fingerprint.finish())
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
        assert_ne!(existing.observed_version, 0);
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
