//! Bounded data-only profile exchange; every object rejects duplicate keys.
use super::{WorkspaceProfile, validate_profile};
use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
pub const MAX_BYTES: usize = 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    format: String,
    version: u32,
    platform: String,
    profile: WorkspaceProfile,
}
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> Result<Strict, M::Error> {
                let mut object = serde_json::Map::new();
                while let Some((key, Strict(value))) = m.next_entry::<String, Strict>()? {
                    if object.insert(key, value).is_some() {
                        return Err(de::Error::custom("Duplicate JSON field"));
                    }
                }
                Ok(Strict(Value::Object(object)))
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut s: S) -> Result<Strict, S::Error> {
                let mut values = Vec::new();
                while let Some(Strict(v)) = s.next_element::<Strict>()? {
                    values.push(v);
                }
                Ok(Strict(Value::Array(values)))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Strict, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| de::Error::custom("Non-finite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
        }
        d.deserialize_any(V)
    }
}
pub fn encode(profile: &WorkspaceProfile) -> Result<String, String> {
    validate_profile(profile)?;
    let text = serde_json::to_string_pretty(&Bundle {
        format: "commander.workspace-profile".into(),
        version: 1,
        platform: std::env::consts::FAMILY.into(),
        profile: profile.clone(),
    })
    .map_err(|e| e.to_string())?;
    if text.len() > MAX_BYTES {
        return Err("Profile exchange JSON exceeds 1 MiB".into());
    }
    Ok(text)
}
pub fn decode(text: &str) -> Result<WorkspaceProfile, String> {
    if text.len() > MAX_BYTES {
        return Err("Profile exchange JSON exceeds 1 MiB".into());
    }
    let Strict(value) = serde_json::from_str::<Strict>(text).map_err(|e| e.to_string())?;
    let fields = value
        .get("profile")
        .and_then(Value::as_object)
        .ok_or("Missing profile object")?;
    if !fields.keys().all(|k| {
        [
            "name",
            "left_root",
            "right_root",
            "left_filter",
            "right_filter",
            "durability",
            "name_policy",
            "symlink_policy",
            "trusted_command_templates",
            "tab_layout",
        ]
        .contains(&k.as_str())
    }) {
        return Err("Unknown profile field".into());
    }
    let doc: Bundle = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if doc.format != "commander.workspace-profile" || doc.version != 1 {
        return Err("Expected commander.workspace-profile version 1".into());
    }
    if doc.platform != std::env::consts::FAMILY {
        return Err("Profile paths belong to a different platform family".into());
    }
    validate_profile(&doc.profile)?;
    Ok(doc.profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_profile_exchange_preserves_saved_data_without_probing_paths() {
        let temp = crate::testutil::TempDir::new();
        let mut ws = crate::workspace::Workspace::new(temp.path().into(), temp.path().into());
        assert!(ws.new_background_tab(
            crate::workspace::ActivePanel::Left,
            temp.path().join("not-present")
        ));
        let mut profile = ws.capture_named_profile("Portable".into()).unwrap();
        profile.left_filter = "*.rs".into();
        profile.trusted_command_templates = vec!["metadata only".into()];
        assert_eq!(decode(&encode(&profile).unwrap()).unwrap(), profile);
    }
    #[test]
    fn malformed_ambiguous_and_oversized_exchange_is_refused() {
        let profile = WorkspaceProfile::new("Work", "/left".into(), "/right".into());
        let text = encode(&profile).unwrap();
        let original: Value = serde_json::from_str(&text).unwrap();
        for (key, value) in [
            ("format", Value::from("other")),
            ("version", Value::from(2)),
            ("platform", Value::from("unsupported")),
            ("unknown", Value::from(true)),
        ] {
            let mut bad = original.clone();
            bad[key] = value;
            assert!(decode(&bad.to_string()).is_err());
        }
        let mut bad = original.clone();
        bad["profile"]["unknown"] = true.into();
        assert!(decode(&bad.to_string()).is_err());
        let mut bad = original;
        bad["profile"]["left_root"] = "relative".into();
        assert!(decode(&bad.to_string()).is_err());
        let duplicate = text.replacen("\"version\": 1", "\"version\":1,\"version\":1", 1);
        assert!(decode(&duplicate).is_err());
        let duplicate = text.replacen("\"collision\":", "\"collision\":\"Ask\",\"collision\":", 1);
        assert!(decode(&duplicate).is_err());
        assert!(decode(&" ".repeat(MAX_BYTES + 1)).is_err());
        assert!(decode(&format!("{}0{}", "[".repeat(200), "]".repeat(200))).is_err());
        let mut huge = profile;
        huge.trusted_command_templates = vec!["x".repeat(MAX_BYTES)];
        assert!(encode(&huge).is_err());
    }
}
