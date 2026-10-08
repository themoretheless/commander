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
    #[serde(default)]
    pub case_sensitive: Option<bool>,
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
    profile_with_refresh(path, false)
}

/// Re-probe the mount contract even when its short-lived cache entry is still
/// fresh. Used by disconnect guards; generation remains stable when the
/// observed contract is unchanged.
pub fn refresh(path: &Path) -> VolumeProfile {
    profile_with_refresh(path, true)
}

fn profile_with_refresh(path: &Path, force: bool) -> VolumeProfile {
    let probe_path = nearest_existing(path);
    let volume_id =
        native_volume_id(&probe_path).unwrap_or_else(|| fallback_volume_id(&probe_path));
    let cache = PROFILES.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let profiles = crate::lock_util::recover(cache);
        if !force
            && let Some(cached) = profiles.get(&volume_id)
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
    if let Some(cache) = PROFILES.get()
        && let Some(cached) = crate::lock_util::recover(cache).get_mut(&volume_id)
    {
        // Force the next normal lookup to probe again, but retain the last
        // contract so an unchanged mount keeps its generation. Dropping
        // the entry here made unrelated concurrent transfers interpret a
        // transient I/O error as a remount.
        cached.expires_at = Instant::now();
    }
}

fn same_contract(left: &VolumeProfile, right: &VolumeProfile) -> bool {
    left.volume_id == right.volume_id
        && left.backend == right.backend
        && left.filesystem == right.filesystem
        && left.mount_point == right.mount_point
        && left.read_only == right.read_only
        && left.case_sensitive == right.case_sensitive
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
    if ["/Volumes", "/media", "/run/media"]
        .iter()
        .any(|root| mount_point.starts_with(root))
    {
        return BackendKind::Removable;
    }
    if is_fast_local_filesystem(filesystem) {
        BackendKind::LocalFast
    } else {
        BackendKind::LocalSlow
    }
}

/// Journaling or copy-on-write filesystems that sit on fixed local storage.
fn is_fast_local_filesystem(filesystem: &str) -> bool {
    matches!(
        filesystem.to_ascii_lowercase().as_str(),
        "apfs" | "ext4" | "btrfs" | "xfs" | "f2fs" | "zfs" | "tmpfs" | "overlay"
    )
}

fn probe(path: &Path, volume_id: u64) -> VolumeProfile {
    let native = native_mount(path);
    let filesystem = native
        .as_ref()
        .map_or_else(|| "unknown".to_string(), |mount| mount.filesystem.clone());
    // The mount point is a property of the volume, never of the queried path:
    // the profile cache is keyed by volume, so every path on it must agree.
    let mount_point = native
        .as_ref()
        .map_or_else(|| mount_root(path), |mount| mount.mount_point.clone());
    let read_only = native.as_ref().is_some_and(|mount| mount.read_only);
    let case_sensitive = native.as_ref().and_then(|mount| mount.case_sensitive);
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
        case_sensitive,
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
    case_sensitive: Option<bool>,
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
        case_sensitive: {
            let value = unsafe { libc::pathconf(path.as_ptr(), libc::_PC_CASE_SENSITIVE) };
            (value >= 0).then_some(value != 0)
        },
    })
}

#[cfg(target_os = "linux")]
fn native_mount(path: &Path) -> Option<NativeMount> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: `c_path` is NUL-terminated and `stats` is a writable statfs.
    if unsafe { libc::statfs(c_path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statfs returned 0, so the struct is initialized.
    let magic = unsafe { stats.assume_init() }.f_type as u64 as u32;
    let mut vfs = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: as above, for statvfs.
    let read_only = unsafe { libc::statvfs(c_path.as_ptr(), vfs.as_mut_ptr()) } == 0
        && unsafe { vfs.assume_init() }.f_flag & libc::ST_RDONLY != 0;
    let (filesystem, local) = linux_filesystem(magic);
    Some(NativeMount {
        filesystem: filesystem.to_string(),
        mount_point: mount_root(path),
        local,
        read_only,
        case_sensitive: None,
    })
}

/// Name and locality for a Linux `statfs` magic number (`linux/magic.h`).
/// Unrecognised filesystems are treated as local but not fast.
#[cfg(target_os = "linux")]
fn linux_filesystem(magic: u32) -> (&'static str, bool) {
    match magic {
        0xEF53 => ("ext4", true),
        0x9123_683E => ("btrfs", true),
        0x5846_5342 => ("xfs", true),
        0xF2F5_2010 => ("f2fs", true),
        0x2FC1_2FC1 => ("zfs", true),
        0x0102_1994 => ("tmpfs", true),
        0x794C_7630 => ("overlay", true),
        0x4D44 => ("msdos", true),
        0x2011_BAB0 => ("exfat", true),
        0x5346_544E | 0x7366_746E => ("ntfs", true),
        0x9660 => ("iso9660", true),
        0x7371_7368 => ("squashfs", true),
        0x6969 => ("nfs", false),
        0x517B => ("smb", false),
        0xFF53_4D42 => ("cifs", false),
        0xFE53_4D42 => ("smb2", false),
        0x6573_5546 => ("fuse", false),
        0x0102_1997 => ("9p", false),
        0x00C3_6400 => ("ceph", false),
        _ => ("unknown", true),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn native_mount(_path: &Path) -> Option<NativeMount> {
    None
}

/// The root of the volume holding `path`: the highest ancestor still on the
/// same device. Used where the OS gives no mount table entry directly.
#[cfg(unix)]
fn mount_root(path: &Path) -> PathBuf {
    use std::os::unix::fs::MetadataExt;

    let start = nearest_existing(path);
    let start = std::fs::canonicalize(&start).unwrap_or(start);
    let Ok(device) = std::fs::metadata(&start).map(|metadata| metadata.dev()) else {
        return start;
    };
    let mut root = start.as_path();
    while let Some(parent) = root.parent() {
        match std::fs::metadata(parent) {
            Ok(metadata) if metadata.dev() == device => root = parent,
            _ => break,
        }
    }
    root.to_path_buf()
}

/// The drive or UNC share root of `path` (`C:\`, `\\server\share\`).
#[cfg(not(unix))]
fn mount_root(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut root = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => root.push(component),
            _ => break,
        }
    }
    if root.as_os_str().is_empty() {
        nearest_existing(path)
    } else {
        root
    }
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
    fn forced_invalidation_keeps_generation_when_the_mount_is_unchanged() {
        let temp = TempDir::new();
        let first = profile(temp.path());
        invalidate(first.volume_id);
        let second = profile(temp.path());
        assert_eq!(first.volume_id, second.volume_id);
        assert_eq!(first.generation, second.generation);
    }

    #[test]
    fn every_path_on_a_volume_reports_the_same_mount_point() {
        let (first_dir, second_dir) = (TempDir::new(), TempDir::new());
        let first = profile(first_dir.path());
        drop(first_dir);
        let second = profile(second_dir.path());
        assert_eq!(first.volume_id, second.volume_id);
        assert_eq!(first.mount_point, second.mount_point);
        assert!(second.mount_point.exists());
        let canonical = std::fs::canonicalize(second_dir.path()).unwrap();
        assert!(canonical.starts_with(&second.mount_point));
        // A guard captured after an earlier folder on the volume was deleted
        // must still see its volume as available.
        let guard = crate::mount_guard::MountGuard::capture(
            second_dir.path(),
            crate::mount_guard::ReconnectPolicy::default(),
        );
        assert_eq!(
            guard.check(),
            crate::mount_guard::MountAvailability::Available
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_probe_names_the_filesystem_and_marks_it_local() {
        let temp = TempDir::new();
        let mount = native_mount(temp.path()).expect("statfs on a temp dir");
        assert!(mount.mount_point.is_absolute());
        assert_eq!(linux_filesystem(0x6969), ("nfs", false));
        assert_eq!(linux_filesystem(0xEF53), ("ext4", true));
    }

    #[test]
    fn linux_and_mac_removable_roots_classify_as_removable() {
        assert_eq!(
            classify(Some(true), "exfat", Path::new("/run/media/me/card")),
            BackendKind::Removable
        );
        assert_eq!(
            classify(Some(true), "ext4", Path::new("/")),
            BackendKind::LocalFast
        );
        assert_eq!(
            classify(Some(true), "unknown", Path::new("/")),
            BackendKind::LocalSlow
        );
    }

    #[test]
    fn fat_like_filesystems_never_claim_sparse_support() {
        assert!(!capabilities(BackendKind::Removable, "exfat", false).sparse);
        assert!(!capabilities(BackendKind::LocalFast, "apfs", true).clone);
    }
}
