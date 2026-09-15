//! Recipient-encrypted support-bundle envelope (research **J005**).
//!
//! # Format `commander-encrypted-bundle-v1`
//!
//! Cleartext fields (inspectable without decrypting): recipient id, created /
//! expires timestamps, and a redacted plaintext preview.
//!
//! Ciphertext uses **XChaCha20-Poly1305**. The payload key is
//! `BLAKE3::derive_key("commander encrypted support bundle v1", salt || secret)`
//! where `secret` is the recipient shared secret / passphrase supplied at export.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "commander-encrypted-bundle-v1";
pub const DEFAULT_TTL_SECS: u64 = 7 * 24 * 60 * 60;
const DERIVE_CONTEXT: &str = "commander encrypted support bundle v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedBundleManifest {
    pub format: String,
    pub recipient_id: String,
    pub created_at_secs: u64,
    pub expires_at_secs: u64,
    pub plaintext_preview: String,
    pub ciphertext_bytes: u64,
    pub salt_hex: String,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
}

impl EncryptedBundleManifest {
    pub fn expired(&self, now_secs: u64) -> bool {
        now_secs >= self.expires_at_secs
    }

    pub fn preview_line(&self) -> String {
        format!(
            "recipient={} expires_at={} preview={}",
            self.recipient_id, self.expires_at_secs, self.plaintext_preview
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptRequest<'a> {
    pub recipient_id: &'a str,
    pub recipient_secret: &'a str,
    pub created_at_secs: u64,
    pub ttl_secs: u64,
    pub plaintext_preview: &'a str,
    pub plaintext: &'a [u8],
}

pub fn encrypt(request: EncryptRequest<'_>) -> Result<EncryptedBundleManifest, String> {
    if request.recipient_id.trim().is_empty() {
        return Err("encrypted support bundle requires a recipient id".into());
    }
    if request.recipient_secret.is_empty() {
        return Err("encrypted support bundle requires a recipient secret".into());
    }
    let mut salt = [0_u8; 32];
    let mut nonce_bytes = [0_u8; 24];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let key = derive_key(request.recipient_secret.as_bytes(), &salt);
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| "could not initialize XChaCha20-Poly1305".to_string())?;
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, request.plaintext)
        .map_err(|_| "support bundle encryption failed".to_string())?;
    Ok(EncryptedBundleManifest {
        format: FORMAT.to_string(),
        recipient_id: request.recipient_id.to_string(),
        created_at_secs: request.created_at_secs,
        expires_at_secs: request.created_at_secs.saturating_add(request.ttl_secs),
        plaintext_preview: truncate_preview(request.plaintext_preview, 240),
        ciphertext_bytes: ciphertext.len() as u64,
        salt_hex: hex_encode(&salt),
        nonce_hex: hex_encode(&nonce_bytes),
        ciphertext_hex: hex_encode(&ciphertext),
    })
}

pub fn decrypt(
    manifest: &EncryptedBundleManifest,
    recipient_secret: &str,
    now_secs: u64,
) -> Result<Vec<u8>, String> {
    if manifest.format != FORMAT {
        return Err(format!(
            "unsupported encrypted bundle format {}",
            manifest.format
        ));
    }
    if manifest.expired(now_secs) {
        return Err("encrypted support bundle has expired".into());
    }
    let salt = hex_decode(&manifest.salt_hex)?;
    let nonce_bytes = hex_decode(&manifest.nonce_hex)?;
    let ciphertext = hex_decode(&manifest.ciphertext_hex)?;
    if salt.len() != 32 || nonce_bytes.len() != 24 {
        return Err("encrypted support bundle header is malformed".into());
    }
    let mut salt_arr = [0_u8; 32];
    salt_arr.copy_from_slice(&salt);
    let key = derive_key(recipient_secret.as_bytes(), &salt_arr);
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| "could not initialize XChaCha20-Poly1305".to_string())?;
    let nonce = XNonce::from_slice(&nonce_bytes);
    cipher.decrypt(nonce, ciphertext.as_ref()).map_err(|_| {
        "support bundle decryption failed (wrong secret or tampered ciphertext)".into()
    })
}

pub fn export_to(
    path: &Path,
    plaintext: &[u8],
    recipient_id: &str,
    recipient_secret: &str,
    plaintext_preview: &str,
    created_at_secs: u64,
    ttl_secs: u64,
) -> Result<EncryptedBundleManifest, String> {
    let manifest = encrypt(EncryptRequest {
        recipient_id,
        recipient_secret,
        created_at_secs,
        ttl_secs,
        plaintext_preview,
        plaintext,
    })?;
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("Could not encode encrypted support bundle: {error}"))?;
    if crate::fs_util::write_atomic(path, &json) {
        Ok(manifest)
    } else {
        Err(format!(
            "Could not write encrypted support bundle to {}",
            path.display()
        ))
    }
}

pub fn load_manifest(path: &Path) -> Result<EncryptedBundleManifest, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Could not read encrypted support bundle: {error}"))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Could not parse encrypted support bundle: {error}"))
}

pub fn default_export_path(config_dir: &Path, stamp_millis: u128) -> PathBuf {
    config_dir
        .join("support-bundles")
        .join(format!("commander-support-{stamp_millis}.cmeb.json"))
}

fn derive_key(secret: &[u8], salt: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(DERIVE_CONTEXT);
    hasher.update(salt);
    hasher.update(secret);
    *hasher.finalize().as_bytes()
}

fn truncate_preview(preview: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (count, ch) in preview.chars().enumerate() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
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
    fn round_trip_encrypts_and_honours_expiry() {
        let plaintext = br#"{"schema":2,"note":"redacted"}"#;
        let manifest = encrypt(EncryptRequest {
            recipient_id: "alice",
            recipient_secret: "test-secret",
            created_at_secs: 1_000,
            ttl_secs: 60,
            plaintext_preview: "schema=2 ops=0",
            plaintext,
        })
        .unwrap();
        assert_eq!(manifest.format, FORMAT);
        assert!(!manifest.expired(1_050));
        assert!(manifest.expired(1_060));
        assert!(manifest.plaintext_preview.contains("schema=2"));
        let recovered = decrypt(&manifest, "test-secret", 1_050).unwrap();
        assert_eq!(recovered, plaintext);
        assert!(decrypt(&manifest, "wrong", 1_050).is_err());
        assert!(decrypt(&manifest, "test-secret", 2_000).is_err());
    }

    #[test]
    fn export_writes_loadable_manifest() {
        let temp = TempDir::new();
        let path = temp.path().join("bundle.cmeb.json");
        let written = export_to(
            &path,
            b"payload-bytes",
            "ops",
            "secret",
            "preview-line",
            10,
            DEFAULT_TTL_SECS,
        )
        .unwrap();
        let loaded = load_manifest(&path).unwrap();
        assert_eq!(loaded.recipient_id, written.recipient_id);
        assert_eq!(decrypt(&loaded, "secret", 10).unwrap(), b"payload-bytes");
    }
}
