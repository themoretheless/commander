//! Compatibility facade for research **J005** (see [`crate::encrypted_bundle`]).
//!
//! Keeps the test-facing sealed-bundle helpers while routing through
//! XChaCha20-Poly1305 envelopes.

use crate::encrypted_bundle::{DEFAULT_TTL_SECS, EncryptedBundleManifest};
#[cfg(test)]
use crate::encrypted_bundle::{EncryptRequest, hex_decode};
use std::path::{Path, PathBuf};

#[cfg(test)]
pub use crate::encrypted_bundle::{decrypt, encrypt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedBundle {
    pub manifest: EncryptedBundleManifest,
    pub ciphertext: Vec<u8>,
}

#[cfg(test)]
pub fn seal_support_bytes_with_secret(
    recipient_id: &str,
    recipient_secret: &str,
    plaintext: &[u8],
    preview: &str,
    created_at_secs: u64,
    ttl_secs: u64,
) -> Result<SealedBundle, String> {
    let manifest = encrypt(EncryptRequest {
        recipient_id,
        recipient_secret,
        created_at_secs,
        ttl_secs,
        plaintext_preview: preview,
        plaintext,
    })?;
    let ciphertext = hex_decode(&manifest.ciphertext_hex)?;
    Ok(SealedBundle {
        manifest,
        ciphertext,
    })
}

#[cfg(test)]
pub fn open_support_bytes(
    sealed: &SealedBundle,
    recipient_id: &str,
    recipient_secret: &str,
    now_secs: u64,
) -> Result<Vec<u8>, String> {
    if sealed.manifest.recipient_id != recipient_id {
        return Err("recipient mismatch".to_string());
    }
    decrypt(&sealed.manifest, recipient_secret, now_secs)
}

#[allow(dead_code)] // Alternate J005 writer for tests; production uses encrypted_bundle::export_to
pub fn write_sealed_bundle(dir: &Path, sealed: &SealedBundle) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let stem = format!(
        "support-{}-{}",
        sanitize_component(&sealed.manifest.recipient_id),
        sealed.manifest.created_at_secs
    );
    let path = dir.join(format!("{stem}.cmeb.json"));
    let json = serde_json::to_string_pretty(&sealed.manifest).map_err(|error| error.to_string())?;
    if crate::fs_util::write_atomic(&path, &json) {
        Ok(path)
    } else {
        Err(format!(
            "Could not write sealed bundle to {}",
            path.display()
        ))
    }
}

pub const fn default_ttl_secs() -> u64 {
    DEFAULT_TTL_SECS
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn sealed_bundle_round_trips_through_xchacha() {
        let sealed = seal_support_bytes_with_secret(
            "alice",
            "separate-secret",
            b"secret-support",
            "redacted preview",
            1_000,
            30,
        )
        .unwrap();
        assert!(!sealed.manifest.expired(1_010));
        assert!(sealed.manifest.expired(1_040));
        let opened = open_support_bytes(&sealed, "alice", "separate-secret", 1_010).unwrap();
        assert_eq!(opened, b"secret-support");
        assert!(open_support_bytes(&sealed, "bob", "separate-secret", 1_010).is_err());
        assert!(open_support_bytes(&sealed, "alice", "wrong", 1_010).is_err());
    }

    #[test]
    fn writes_single_envelope_file() {
        let temp = TempDir::new();
        let sealed =
            seal_support_bytes_with_secret("ops", "separate-secret", b"{}", "empty", 42, 3600)
                .unwrap();
        let path = write_sealed_bundle(temp.path(), &sealed).unwrap();
        assert!(path.exists());
        let loaded = crate::encrypted_bundle::load_manifest(&path).unwrap();
        assert_eq!(loaded.recipient_id, "ops");
    }
}
