//! Bounded BLAKE3 cache for files whose identity stayed stable while hashing.

use crate::path_identity::{PathIdentity, PathKind};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const HASH_BUFFER_SIZE: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 4096;

pub type VerifiedHash = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    volume_id: u64,
    file_id: Option<u64>,
    size: u64,
    modified_nanos: Option<u128>,
    filesystem_generation: u64,
    path_fallback: Option<PathBuf>,
}

#[derive(Clone, Copy)]
struct CacheEntry {
    hash: VerifiedHash,
    last_used: u64,
}

#[derive(Default)]
struct HashCache {
    entries: HashMap<CacheKey, CacheEntry>,
    clock: u64,
    hits: u64,
    misses: u64,
}

static CACHE: OnceLock<Mutex<HashCache>> = OnceLock::new();

pub fn file(path: &Path) -> std::io::Result<VerifiedHash> {
    let profile = crate::volume_profile::profile(path);
    let before = PathIdentity::observe(path)?;
    if before.kind != Some(PathKind::File) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "verified hashing requires a regular file",
        ));
    }
    let key = cache_key(path, &before, &profile);
    if let Some(hash) = cached(&key) {
        return Ok(hash);
    }

    let mut source = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let after = PathIdentity::observe(path)?;
    let after_profile = crate::volume_profile::profile(path);
    if !before.same_version(&after) || profile.generation != after_profile.generation {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "file or filesystem generation changed while hashing",
        ));
    }
    let hash = *hasher.finalize().as_bytes();
    insert(key, hash);
    Ok(hash)
}

pub fn files_equal(left: &Path, right: &Path) -> bool {
    files_equal_with(crate::ports::default_hasher(), left, right)
}

pub fn files_equal_with(hasher: &dyn crate::ports::Hasher, left: &Path, right: &Path) -> bool {
    let (Ok(left_metadata), Ok(right_metadata)) = (left.metadata(), right.metadata()) else {
        return false;
    };
    if left_metadata.len() != right_metadata.len() {
        return false;
    }
    match (hasher.hash(left), hasher.hash(right)) {
        (Ok(left_hash), Ok(right_hash)) => left_hash == right_hash,
        _ => false,
    }
}

fn cache_key(
    path: &Path,
    identity: &PathIdentity,
    profile: &crate::volume_profile::VolumeProfile,
) -> CacheKey {
    let stable_identity = identity.volume.is_some() && identity.file_id.is_some();
    CacheKey {
        volume_id: identity.volume.unwrap_or(profile.volume_id),
        file_id: identity.file_id,
        size: identity.size,
        modified_nanos: identity.modified_nanos,
        filesystem_generation: profile.generation,
        path_fallback: (!stable_identity)
            .then(|| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())),
    }
}

fn cached(key: &CacheKey) -> Option<VerifiedHash> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashCache::default()));
    let mut cache = crate::lock_util::recover(cache);
    let next = cache.clock.saturating_add(1);
    cache.clock = next;
    let hash = cache.entries.get_mut(key).map(|entry| {
        entry.last_used = next;
        entry.hash
    });
    if hash.is_some() {
        cache.hits = cache.hits.saturating_add(1);
    } else {
        cache.misses = cache.misses.saturating_add(1);
    }
    hash
}

fn insert(key: CacheKey, hash: VerifiedHash) {
    let cache = CACHE.get_or_init(|| Mutex::new(HashCache::default()));
    let mut cache = crate::lock_util::recover(cache);
    cache.clock = cache.clock.saturating_add(1);
    let last_used = cache.clock;
    cache.entries.insert(key, CacheEntry { hash, last_used });
    while cache.entries.len() > MAX_ENTRIES {
        let Some(oldest) = cache
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.entries.remove(&oldest);
    }
}

#[cfg(test)]
fn contains(key: &CacheKey) -> bool {
    let cache = CACHE.get_or_init(|| Mutex::new(HashCache::default()));
    let cache = crate::lock_util::recover(cache);
    cache.entries.contains_key(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn repeated_hash_uses_the_stable_identity_cache() {
        let temp = TempDir::new();
        let path = temp.file("same.bin", "same contents");
        let identity = PathIdentity::observe(&path).unwrap();
        let profile = crate::volume_profile::profile(&path);
        let key = cache_key(&path, &identity, &profile);

        let first = file(&path).unwrap();
        assert!(contains(&key));
        let second = file(&path).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn changed_size_cannot_reuse_a_verified_hash() {
        let temp = TempDir::new();
        let path = temp.file("changed.bin", "before");
        let before_identity = PathIdentity::observe(&path).unwrap();
        let profile = crate::volume_profile::profile(&path);
        let before_key = cache_key(&path, &before_identity, &profile);
        let before = file(&path).unwrap();
        std::fs::write(&path, "after and longer").unwrap();
        let after_identity = PathIdentity::observe(&path).unwrap();
        let after_key = cache_key(&path, &after_identity, &profile);
        let after = file(&path).unwrap();

        assert_ne!(before, after);
        assert_ne!(before_key, after_key);
        assert!(contains(&before_key));
        assert!(contains(&after_key));
    }

    #[test]
    fn filesystem_generation_is_part_of_the_key() {
        let temp = TempDir::new();
        let path = temp.file("generation.bin", "contents");
        let identity = PathIdentity::observe(&path).unwrap();
        let mut profile = crate::volume_profile::profile(&path);
        let first = cache_key(&path, &identity, &profile);
        profile.generation = profile.generation.saturating_add(1);
        let second = cache_key(&path, &identity, &profile);

        assert_ne!(first, second);
    }
}
