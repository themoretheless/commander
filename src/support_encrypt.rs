//! Compatibility facade for research **J005** (see [`crate::encrypted_bundle`]).
//!
//! Keeps the older `seal_support_bytes` / `write_sealed_bundle` names while
//! routing through XChaCha20-Poly1305 envelopes.

use crate::encrypted_bundle::{self, DEFAULT_TTL_SECS, EncryptRequest, EncryptedBundleManifest};
use std::path::{Path, PathBuf};

pub use crate::encrypted_bundle::{DEFAULT_TTL_SECS as TTL_DEFAULT, FORMAT, decrypt, encrypt, export_to, load_manifest};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedBundle {
    pub manifest: EncryptedBundleManifest,
    pub ciphertext: Vec<u8>,
}

pub fn seal_support_bytes(
    recipient_id: &str,
    plaintext: &[u8],
    preview: &str,
    created_at_secs: u64,
    ttl_secs: u64,
) -> Result<SealedBundle, String> {
    // Recipient id doubles as the shared secret when callers use the legacy API.
    let manifest = encrypt(EncryptRequest {
        recipient_id,
        recipient_secret: recipient_id,
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

pub fn open_support_bytes(
    sealed: &SealedBundle,
    recipient_id: &str,
    now_secs: u64,
) -> Result<Vec<u8>, String> {
    if sealed.manifest.recipient_id != recipient_id {
        return Err("recipient mismatch".to_string());
    }
    decrypt(&sealed.manifest, recipient_id, now_secs)
}

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
        Err(format!("Could not write sealed bundle to {}", path.display()))
    }
}

pub fn default_ttl_secs() -> u64 {
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

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("invalid hex length".into());
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        let hi = from_hex(bytes[index])?;
        let lo = from_hex(bytes[index + 1])?;
        out.push((hi << 4) | lo);
        index += 2;
    }
    Ok(out)
}

fn from_hex(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex digit".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn legacy_seal_api_round_trips_through_xchacha() {
        let sealed = seal_support_bytes("alice", b"secret-support", "redacted preview", 1_000, 30)
            .unwrap();
        assert!(!sealed.manifest.expired(1_010));
        assert!(sealed.manifest.expired(1_040));
        let opened = open_support_bytes(&sealed, "alice", 1_010).unwrap();
        assert_eq!(opened, b"secret-support");
        assert!(open_support_bytes(&sealed, "bob", 1_010).is_err());
    }

    #[test]
    fn writes_single_envelope_file() {
        let temp = TempDir::new();
        let sealed = seal_support_bytes("ops", b"{}", "empty", 42, 3600).unwrap();
        let path = write_sealed_bundle(temp.path(), &sealed).unwrap();
        assert!(path.exists());
        let loaded = load_manifest(&path).unwrap();
        assert_eq!(loaded.recipient_id, "ops");
    }
}
