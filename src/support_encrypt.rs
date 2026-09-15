//! Recipient-encrypted support bundle envelope metadata (research **J005**).
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedBundleManifest {
    pub recipient_id: String,
    pub created_at_secs: u64,
    pub expires_at_secs: u64,
    pub plaintext_preview: String,
    pub ciphertext_bytes: u64,
}
impl EncryptedBundleManifest {
    pub fn new(recipient_id: impl Into<String>, created_at_secs: u64, ttl_secs: u64, plaintext_preview: impl Into<String>, ciphertext_bytes: u64) -> Self {
        Self { recipient_id: recipient_id.into(), created_at_secs, expires_at_secs: created_at_secs.saturating_add(ttl_secs), plaintext_preview: plaintext_preview.into(), ciphertext_bytes }
    }
    pub fn expired(&self, now_secs: u64) -> bool { now_secs >= self.expires_at_secs }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expiry_is_explicit() {
        let m = EncryptedBundleManifest::new("alice", 100, 50, "preview", 2048);
        assert!(!m.expired(140));
        assert!(m.expired(150));
    }
}
