//! Typed, short-lived filesystem capability profiles keyed by volume identity.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const PROFILE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    LocalFast,
    LocalSlow,
    Removable,
    Remote,
    Unknown,
}

impl BackendKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::LocalFast => "Local fast",
            Self::LocalSlow => "Local",
            Self::Removable => "Removable",
            Self::Remote => "Remote",
            Self::Unknown => "Unknown",
        }
    }

    pub fn is_slow_link(self) -> bool {
        matches!(self, Self::LocalSlow | Self::Removable | Self::Remote)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeCapabilities {
    pub atomic_rename: bool,
    pub clone: bool,
    pub sparse: bool,
    pub resumable: bool,
    pub delta: bool,
    pub max_concurrency: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeProfile {
    pub volume_id: u64,
    /// Changes when the observed mount/backend contract changes. Cache users
    /// include it in their key so a changed filesystem never reuses old data.
    pub generation: u64,
    pub backend: BackendKind,
    pub filesystem: String,
    pub mount_point: PathBuf,
    pub read_only: bool,
    pub capabilities: VolumeCapabilities,
    pub reason: String,
}

struct CachedProfile {
    profile: VolumeProfile,
    expires_at: Instant,
}

static PROFILES: OnceLock<Mutex<HashMap<u64, CachedProfile>>> = OnceLock::new();
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn profile(path: &Path) -> VolumeProfile {
    let probe_path = nearest_existing(path);
    let volume_id =
        native_volume_id(&probe_path).unwrap_or_else(|| fallback_volume_id(&probe_path));
    let cache = PROFILES.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let profiles = crate::lock_util::recover(cache);
        if let Some(cached) = profiles.get(&volume_id)
            && cached.expires_at > Instant::now()
        {
            return cached.profile.clone();
        }
    }

    let mut observed = probe(&probe_path, volume_id);
    let mut profiles = crate::lock_util::recover(cache);
    observed.generation = profiles.get(&volume_id).map_or_else(
        || NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
        |cached| {
            if same_contract(&cached.profile, &observed) {
                cached.profile.generation
            } else {
                NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
            }
        },
    );
    profiles.insert(
        volume_id,
        CachedProfile {
            profile: observed.clone(),
            expires_at: Instant::now() + PROFILE_TTL,
        },
    );
    observed
}

pub fn invalidate(volume_id: u64) {
    if let Some(cache) = PROFILES.get() {
        crate::lock_util::recover(cache).remove(&volume_id);
    }
}

fn same_contract(left: &VolumeProfile, right: &VolumeProfile) -> bool {
    left.volume_id == right.volume_id
        && left.backend == right.backend
        && left.filesystem == right.filesystem
        && left.mount_point == right.mount_point
        && left.read_only == right.read_only
        && left.capabilities == right.capabilities
}

fn nearest_existing(path: &Path) -> PathBuf {
    let mut candidate = path;
    while std::fs::symlink_metadata(candidate).is_err() {
        let Some(parent) = candidate.parent() else {
            break;
        };
        candidate = parent;
    }
    candidate.to_path_buf()
}

#[cfg(unix)]
fn native_volume_id(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|metadata| metadata.dev())
}

#[cfg(not(unix))]
fn native_volume_id(_path: &Path) -> Option<u64> {
    None
}

fn fallback_volume_id(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.components().next().hash(&mut hasher);
    hasher.finish()
}

fn capabilities(backend: BackendKind, filesystem: &str, read_only: bool) -> VolumeCapabilities {
    let local = !matches!(backend, BackendKind::Remote | BackendKind::Unknown);
    let sparse = local
        && !matches!(
            filesystem.to_ascii_lowercase().as_str(),
            "msdos" | "exfat" | "fat" | "fat32"
        );
    VolumeCapabilities {
        atomic_rename: local && !read_only,
        clone: backend == BackendKind::LocalFast
            && filesystem.eq_ignore_ascii_case("apfs")
            && !read_only,
        sparse: sparse && !read_only,
        resumable: !read_only,
        delta: backend.is_slow_link() && !read_only,
        max_concurrency: match backend {
            BackendKind::LocalFast => 4,
            BackendKind::LocalSlow | BackendKind::Removable | BackendKind::Remote => 2,
            BackendKind::Unknown => 1,
        },
    }
}

fn classify(local: Option<bool>, filesystem: &str, mount_point: &Path) -> BackendKind {
    if local == Some(false) {
        return BackendKind::Remote;
    }
    if local.is_none() {
        return BackendKind::Unknown;
    }
    if mount_point.starts_with("/Volumes") {
        return BackendKind::Removable;
    }
    if filesystem.eq_ignore_ascii_case("apfs") {
        BackendKind::LocalFast
    } else {
        BackendKind::LocalSlow
    }
}

fn probe(path: &Path, volume_id: u64) -> VolumeProfile {
    let native = native_mount(path);
    let filesystem = native
        .as_ref()
        .map_or_else(|| "unknown".to_string(), |mount| mount.filesystem.clone());
    let mount_point = native
        .as_ref()
        .map_or_else(|| nearest_existing(path), |mount| mount.mount_point.clone());
    let read_only = native.as_ref().is_some_and(|mount| mount.read_only);
    let backend = classify(
        native.as_ref().map(|mount| mount.local),
        &filesystem,
        &mount_point,
    );
    VolumeProfile {
        volume_id,
        generation: 0,
        backend,
        filesystem: filesystem.clone(),
        mount_point: mount_point.clone(),
        read_only,
        capabilities: capabilities(backend, &filesystem, read_only),
        reason: format!(
            "{} filesystem at {}{}",
            filesystem,
            mount_point.display(),
            if read_only { " (read-only)" } else { "" }
        ),
    }
}

struct NativeMount {
    filesystem: String,
    mount_point: PathBuf,
    local: bool,
    read_only: bool,
}

#[cfg(target_os = "macos")]
fn native_mount(path: &Path) -> Option<NativeMount> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    let filesystem = unsafe { CStr::from_ptr(stats.f_fstypename.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let mount_point = PathBuf::from(
        unsafe { CStr::from_ptr(stats.f_mntonname.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    );
    Some(NativeMount {
        filesystem,
        mount_point,
        local: stats.f_flags & libc::MNT_LOCAL as u32 != 0,
        read_only: stats.f_flags & libc::MNT_RDONLY as u32 != 0,
    })
}

#[cfg(not(target_os = "macos"))]
fn native_mount(_path: &Path) -> Option<NativeMount> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn backend_classification_is_typed_and_conservative() {
        assert_eq!(
            classify(Some(false), "smbfs", Path::new("/Volumes/share")),
            BackendKind::Remote
        );
        assert_eq!(
            classify(Some(true), "apfs", Path::new("/")),
            BackendKind::LocalFast
        );
        assert_eq!(
            classify(Some(true), "exfat", Path::new("/Volumes/card")),
            BackendKind::Removable
        );
        assert_eq!(
            classify(None, "unknown", Path::new("/missing")),
            BackendKind::Unknown
        );
    }

    #[test]
    fn profile_cache_keeps_generation_for_the_same_mount_contract() {
        let temp = TempDir::new();
        let first = profile(temp.path());
        let second = profile(temp.path());
        assert_eq!(first.volume_id, second.volume_id);
        assert_eq!(first.generation, second.generation);
        assert!(!first.reason.is_empty());
    }

    #[test]
    fn fat_like_filesystems_never_claim_sparse_support() {
        assert!(!capabilities(BackendKind::Removable, "exfat", false).sparse);
        assert!(!capabilities(BackendKind::LocalFast, "apfs", true).clone);
    }
}
