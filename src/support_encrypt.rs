//! Recipient-encrypted support-bundle envelope (research **J005**).
//!
//! Bundles stay redacted/capped in [`crate::support_bundle`]. This module adds
//! the transport/privacy envelope: a plaintext preview, explicit expiry, and a
//! recipient-bound ciphertext payload. Encryption uses a documented
//! XOR-of-BLAKE3 keystream derived from recipient id + nonce — enough to keep
//! accidental disclosure out of shared logs while remaining dependency-free;
//! a production age/NaCl backend can replace `seal_bytes` without changing the
//! on-disk manifest shape.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"CMDRSB01";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedBundleManifest {
    pub recipient_id: String,
    pub created_at_secs: u64,
    pub expires_at_secs: u64,
    pub plaintext_preview: String,
    pub ciphertext_bytes: u64,
    pub nonce_hex: String,
}

impl EncryptedBundleManifest {
    pub fn new(
        recipient_id: impl Into<String>,
        created_at_secs: u64,
        ttl_secs: u64,
        plaintext_preview: impl Into<String>,
        ciphertext_bytes: u64,
        nonce_hex: impl Into<String>,
    ) -> Self {
        Self {
            recipient_id: recipient_id.into(),
            created_at_secs,
            expires_at_secs: created_at_secs.saturating_add(ttl_secs),
            plaintext_preview: plaintext_preview.into(),
            ciphertext_bytes,
            nonce_hex: nonce_hex.into(),
        }
    }

    pub fn expired(&self, now_secs: u64) -> bool {
        now_secs >= self.expires_at_secs
    }
}

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
) -> SealedBundle {
    let nonce = blake3::hash(
        format!("commander.support.nonce|{recipient_id}|{created_at_secs}|{}", plaintext.len())
            .as_bytes(),
    );
    let nonce_hex = hex32(nonce.as_bytes());
    let ciphertext = seal_bytes(recipient_id, nonce.as_bytes(), plaintext);
    let manifest = EncryptedBundleManifest::new(
        recipient_id,
        created_at_secs,
        ttl_secs,
        truncate_preview(preview, 240),
        ciphertext.len() as u64,
        nonce_hex,
    );
    SealedBundle {
        manifest,
        ciphertext,
    }
}

pub fn open_support_bytes(
    sealed: &SealedBundle,
    recipient_id: &str,
    now_secs: u64,
) -> Result<Vec<u8>, String> {
    if sealed.manifest.recipient_id != recipient_id {
        return Err("recipient mismatch".to_string());
    }
    if sealed.manifest.expired(now_secs) {
        return Err("encrypted support bundle expired".to_string());
    }
    let nonce = parse_hex32(&sealed.manifest.nonce_hex)?;
    Ok(seal_bytes(recipient_id, &nonce, &sealed.ciphertext))
}

pub fn write_sealed_bundle(dir: &Path, sealed: &SealedBundle) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let stem = format!(
        "support-{}-{}",
        sanitize_component(&sealed.manifest.recipient_id),
        sealed.manifest.created_at_secs
    );
    let manifest_path = dir.join(format!("{stem}.json"));
    let cipher_path = dir.join(format!("{stem}.bin"));
    let json = serde_json::to_vec_pretty(&sealed.manifest).map_err(|error| error.to_string())?;
    std::fs::write(&manifest_path, json).map_err(|error| error.to_string())?;
    let mut blob = Vec::with_capacity(MAGIC.len() + sealed.ciphertext.len());
    blob.extend_from_slice(MAGIC);
    blob.extend_from_slice(&sealed.ciphertext);
    std::fs::write(&cipher_path, blob).map_err(|error| error.to_string())?;
    Ok(manifest_path)
}

fn seal_bytes(recipient_id: &str, nonce: &[u8; 32], input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut counter = 0u64;
    let mut offset = 0usize;
    while offset < input.len() {
        let block = blake3::hash(
            format!("commander.support.stream|{recipient_id}|{counter}|{}", hex32(nonce)).as_bytes(),
        );
        let key = block.as_bytes();
        let end = (offset + 32).min(input.len());
        for (index, byte) in input[offset..end].iter().enumerate() {
            out.push(byte ^ key[index]);
        }
        offset = end;
        counter = counter.saturating_add(1);
    }
    out
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

fn hex32(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn parse_hex32(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err("nonce must be 32 bytes hex".to_string());
    }
    let mut out = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).map_err(|error| error.to_string())?;
        out[index] = u8::from_str_radix(text, 16).map_err(|error| error.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn expiry_is_explicit() {
        let sealed = seal_support_bytes("alice", b"payload", "preview", 100, 50);
        assert!(!sealed.manifest.expired(140));
        assert!(sealed.manifest.expired(150));
    }

    #[test]
    fn round_trip_rejects_wrong_recipient_and_expiry() {
        let sealed = seal_support_bytes("alice", b"secret-support", "redacted preview", 1_000, 30);
        let opened = open_support_bytes(&sealed, "alice", 1_010).unwrap();
        assert_eq!(opened, b"secret-support");
        assert!(open_support_bytes(&sealed, "bob", 1_010).is_err());
        assert!(open_support_bytes(&sealed, "alice", 1_040).is_err());
    }

    #[test]
    fn writes_manifest_and_ciphertext_blob() {
        let temp = TempDir::new();
        let sealed = seal_support_bytes("ops", b"{}", "empty", 42, 3600);
        let path = write_sealed_bundle(temp.path(), &sealed).unwrap();
        assert!(path.exists());
        let bin = temp.path().join("support-ops-42.bin");
        let bytes = std::fs::read(bin).unwrap();
        assert!(bytes.starts_with(MAGIC));
    }
}
