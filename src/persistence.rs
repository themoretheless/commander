//! Shared fault-tolerant JSON loading and privacy-safe recovery diagnostics.

use serde::de::DeserializeOwned;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PersistenceHealth {
    pub issue_generation: u64,
    pub recovered_stores: u64,
    pub recovered_items: u64,
    pub rejected_items: u64,
    pub unreadable_stores: u64,
    pub last_issue: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedItems<T> {
    pub items: Vec<T>,
    pub rejected: usize,
}

static HEALTH: OnceLock<Mutex<PersistenceHealth>> = OnceLock::new();
static ISSUE_GENERATION: AtomicU64 = AtomicU64::new(0);

fn health_state() -> &'static Mutex<PersistenceHealth> {
    HEALTH.get_or_init(|| Mutex::new(PersistenceHealth::default()))
}

fn publish_issue(health: &mut PersistenceHealth) {
    let previous = ISSUE_GENERATION
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
            Some(generation.saturating_add(1))
        })
        .unwrap_or_else(|generation| generation);
    health.issue_generation = previous.saturating_add(1);
}

fn record_recovery(store: &'static str, recovered: usize, rejected: usize) {
    let mut health = crate::lock_util::recover(health_state());
    health.recovered_stores = health.recovered_stores.saturating_add(1);
    health.recovered_items = health
        .recovered_items
        .saturating_add(u64::try_from(recovered).unwrap_or(u64::MAX));
    health.rejected_items = health
        .rejected_items
        .saturating_add(u64::try_from(rejected).unwrap_or(u64::MAX));
    health.last_issue = Some(format!(
        "{store}: recovered {recovered} item(s), skipped {rejected} invalid item(s)"
    ));
    publish_issue(&mut health);
}

fn record_unreadable(store: &'static str) {
    let mut health = crate::lock_util::recover(health_state());
    health.unreadable_stores = health.unreadable_stores.saturating_add(1);
    health.last_issue = Some(format!(
        "{store}: settings could not be read; defaults are active"
    ));
    publish_issue(&mut health);
}

pub fn issue_generation() -> u64 {
    ISSUE_GENERATION.load(Ordering::Acquire)
}

pub fn health_snapshot() -> PersistenceHealth {
    crate::lock_util::recover(health_state()).clone()
}

/// Decode an object containing an `items` array one element at a time. A bad
/// element cannot erase valid siblings; malformed roots still fail closed.
pub fn decode_item_store<T: DeserializeOwned>(json: &str) -> Result<DecodedItems<T>, String> {
    let root: serde_json::Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let values = root
        .get("items")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "settings root does not contain an items array".to_string())?;
    let mut items = Vec::with_capacity(values.len());
    let mut rejected = 0usize;
    for value in values {
        match serde_json::from_value(value.clone()) {
            Ok(item) => items.push(item),
            Err(_) => rejected = rejected.saturating_add(1),
        }
    }
    Ok(DecodedItems { items, rejected })
}

pub fn load_item_store<T: DeserializeOwned>(path: &Path, store: &'static str) -> Vec<T> {
    let json = match std::fs::read_to_string(path) {
        Ok(json) => json,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(_) => {
            record_unreadable(store);
            return Vec::new();
        }
    };
    match decode_item_store(&json) {
        Ok(decoded) => {
            if decoded.rejected > 0 {
                record_recovery(store, decoded.items.len(), decoded.rejected);
            }
            decoded.items
        }
        Err(_) => {
            record_unreadable(store);
            Vec::new()
        }
    }
}

pub fn load_json<T: DeserializeOwned>(path: &Path, store: &'static str) -> Option<T> {
    let json = match std::fs::read_to_string(path) {
        Ok(json) => json,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            record_unreadable(store);
            return None;
        }
    };
    match serde_json::from_str(&json) {
        Ok(value) => Some(value),
        Err(_) => {
            record_unreadable(store);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct Item {
        name: String,
        count: u8,
    }

    #[test]
    fn item_decoder_preserves_valid_siblings_around_a_bad_item() {
        let decoded = decode_item_store::<Item>(
            r#"{
                "items": [
                    {"name": "first", "count": 1},
                    {"name": "broken", "count": "many"},
                    {"name": "last", "count": 3}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(decoded.rejected, 1);
        assert_eq!(
            decoded.items,
            vec![
                Item {
                    name: "first".to_string(),
                    count: 1,
                },
                Item {
                    name: "last".to_string(),
                    count: 3,
                },
            ]
        );
    }

    #[test]
    fn item_decoder_rejects_a_malformed_or_wrong_shaped_root() {
        assert!(decode_item_store::<Item>("not json").is_err());
        assert!(decode_item_store::<Item>(r#"{"entries": []}"#).is_err());
        assert!(decode_item_store::<Item>(r#"{"items": {}}"#).is_err());
    }
}
