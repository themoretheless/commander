//! Named, validated rename rules. Loading never executes a filesystem operation.
use crate::rename::RenameRule;
use serde::{Deserialize, Deserializer, Serialize};
use unicode_normalization::UnicodeNormalization;

pub const MAX_PRESETS: usize = 32;
const MAX_FIELD: usize = 4096;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub rule: RenameRule,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Presets(Vec<Preset>);
fn key(name: &str) -> String {
    name.trim().to_lowercase().nfc().collect()
}
fn name(name: &str) -> Result<String, String> {
    let name: String = name.trim().nfc().collect();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("Preset names need 1–64 characters without control characters".into());
    }
    Ok(name)
}
fn validate(rule: &RenameRule) -> Result<(), String> {
    if [
        &rule.find,
        &rule.replace,
        &rule.prefix,
        &rule.suffix,
        &rule.remove_prefix,
        &rule.remove_suffix,
    ]
    .iter()
    .any(|field| field.len() > MAX_FIELD)
    {
        return Err("Preset fields are limited to 4096 UTF-8 bytes each".into());
    }
    if rule
        .stem_limit
        .is_some_and(|limit| !(1..=255).contains(&limit))
    {
        return Err("Preset stem limit must be between 1 and 255".into());
    }
    if rule.numbering.is_some_and(|numbering| {
        numbering.pad > 6
            || !(-1000..=1000).contains(&numbering.step)
            || !(-100_000..=100_000).contains(&numbering.start)
    }) {
        return Err("Preset numbering is outside the editor's supported range".into());
    }
    if let Some(error) = crate::rename::regex_error(rule) {
        return Err(format!("Invalid preset regex: {error}"));
    }
    Ok(())
}
impl Presets {
    pub fn items(&self) -> &[Preset] {
        &self.0
    }
    pub fn get(&self, label: &str) -> Option<&Preset> {
        let wanted = key(label);
        self.0.iter().find(|preset| key(&preset.name) == wanted)
    }
    pub fn add(&mut self, label: &str, rule: RenameRule) -> Result<String, String> {
        let label = name(label)?;
        validate(&rule)?;
        if self.get(&label).is_some() {
            return Err("A preset with this name exists; use Update selected".into());
        }
        if self.0.len() == MAX_PRESETS {
            return Err("At most 32 rename presets can be saved".into());
        }
        self.0.push(Preset {
            name: label.clone(),
            rule,
        });
        Ok(label)
    }
    pub fn update(&mut self, label: &str, rule: RenameRule) -> Result<(), String> {
        validate(&rule)?;
        let wanted = key(label);
        let preset = self
            .0
            .iter_mut()
            .find(|preset| key(&preset.name) == wanted)
            .ok_or("Select a saved preset first")?;
        preset.rule = rule;
        Ok(())
    }
    pub fn remove(&mut self, label: &str) -> bool {
        let wanted = key(label);
        let before = self.0.len();
        self.0.retain(|preset| key(&preset.name) != wanted);
        self.0.len() != before
    }
}
impl<'de> Deserialize<'de> for Presets {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = Vec::<Preset>::deserialize(deserializer)?;
        if entries.len() > MAX_PRESETS {
            return Err(serde::de::Error::custom("too many rename presets"));
        }
        let mut presets = Self::default();
        for entry in entries {
            presets
                .add(&entry.name, entry.rule)
                .map_err(serde::de::Error::custom)?;
        }
        Ok(presets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn number_position_round_trips_in_named_presets() {
        let mut presets = Presets::default();
        for number_position in [
            crate::rename::NumberPosition::BeforeName,
            crate::rename::NumberPosition::AfterName,
            crate::rename::NumberPosition::CounterOnly,
        ] {
            presets
                .add(
                    number_position.label(),
                    RenameRule {
                        number_position,
                        numbering: Some(crate::rename::Numbering {
                            start: -3,
                            step: -1,
                            pad: 2,
                        }),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let restored: Presets =
            serde_json::from_str(&serde_json::to_string(&presets).unwrap()).unwrap();
        assert_eq!(restored, presets);
    }

    #[test]
    fn signed_numbering_round_trips_in_presets() {
        let mut presets = Presets::default();
        for (name, start, step) in [
            ("descending", 10, -2),
            ("negative", -10, -1),
            ("constant", -1, 0),
        ] {
            presets
                .add(
                    name,
                    RenameRule {
                        numbering: Some(crate::rename::Numbering {
                            start,
                            step,
                            pad: 3,
                        }),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let restored: Presets =
            serde_json::from_str(&serde_json::to_string(&presets).unwrap()).unwrap();
        assert_eq!(restored, presets);
    }

    #[test]
    fn independent_extension_case_round_trips_in_named_presets() {
        let mut presets = Presets::default();
        for (name, extension_case) in [
            ("keep", crate::rename::ExtensionCase::Keep),
            ("lower", crate::rename::ExtensionCase::Lower),
            ("upper", crate::rename::ExtensionCase::Upper),
        ] {
            presets
                .add(
                    name,
                    RenameRule {
                        case: crate::rename::CaseMode::Capitalize,
                        extension_case,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let restored: Presets =
            serde_json::from_str(&serde_json::to_string(&presets).unwrap()).unwrap();
        assert_eq!(restored, presets);
    }

    #[test]
    fn stem_limit_presets_round_trip_and_reject_unsupported_bounds() {
        let mut presets = Presets::default();
        for limit in [1, 32, 255] {
            presets
                .add(
                    &format!("limit-{limit}"),
                    RenameRule {
                        stem_limit: Some(limit),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let restored: Presets =
            serde_json::from_str(&serde_json::to_string(&presets).unwrap()).unwrap();
        assert_eq!(restored, presets);
        for limit in [0, 256, usize::MAX] {
            assert!(
                presets
                    .add(
                        "bad",
                        RenameRule {
                            stem_limit: Some(limit),
                            ..Default::default()
                        }
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn removal_affixes_round_trip_and_enforce_field_bounds() {
        let mut presets = Presets::default();
        let rule = RenameRule {
            remove_prefix: "backup_".into(),
            remove_suffix: ".old".into(),
            ..Default::default()
        };
        presets.add("cleanup", rule.clone()).unwrap();
        let restored: Presets =
            serde_json::from_str(&serde_json::to_string(&presets).unwrap()).unwrap();
        assert_eq!(restored, presets);
        for prefix in [true, false] {
            let mut oversized = rule.clone();
            if prefix {
                oversized.remove_prefix = "x".repeat(MAX_FIELD + 1);
            } else {
                oversized.remove_suffix = "x".repeat(MAX_FIELD + 1);
            }
            assert!(presets.add("oversized", oversized).is_err());
        }
    }

    #[test]
    fn normalization_presets_round_trip_all_forms() {
        use crate::rename::NameNormalization;
        let mut presets = Presets::default();
        for normalization in [
            NameNormalization::Keep,
            NameNormalization::Nfc,
            NameNormalization::Nfd,
            NameNormalization::Nfkc,
            NameNormalization::Nfkd,
        ] {
            presets
                .add(
                    normalization.label(),
                    RenameRule {
                        normalization,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let restored: Presets =
            serde_json::from_str(&serde_json::to_string(&presets).unwrap()).unwrap();
        assert_eq!(restored, presets);
    }

    #[test]
    fn capitalization_presets_round_trip() {
        let mut presets = Presets::default();
        for (name, case) in [
            ("first", crate::rename::CaseMode::Capitalize),
            ("words", crate::rename::CaseMode::TitleWords),
        ] {
            presets
                .add(
                    name,
                    RenameRule {
                        case,
                        trim_spaces: true,
                        collapse_repeats: true,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let restored: Presets =
            serde_json::from_str(&serde_json::to_string(&presets).unwrap()).unwrap();
        assert_eq!(restored, presets);
    }

    #[test]
    fn first_only_and_ignore_case_are_persisted_with_component_scope() {
        let mut presets = Presets::default();
        let rule = RenameRule {
            find: "txt".into(),
            replace: "$1".into(),
            ignore_case: true,
            first_only: true,
            replace_target: crate::rename::ReplaceTarget::Stem,
            ..Default::default()
        };
        presets.add("targeted", rule.clone()).unwrap();
        let encoded = serde_json::to_string(&presets).unwrap();
        let restored: Presets = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.get("targeted").unwrap().rule, rule);
        assert!(
            serde_json::from_str::<Presets>(r#"[{"name":"bad","rule":{"first_only":"true"}}]"#)
                .is_err()
        );
    }

    #[test]
    fn replacement_target_round_trips_in_presets_and_old_rules_keep_whole_name() {
        let mut presets = Presets::default();
        for target in [
            crate::rename::ReplaceTarget::WholeName,
            crate::rename::ReplaceTarget::Stem,
            crate::rename::ReplaceTarget::Extension,
        ] {
            presets
                .add(
                    target.label(),
                    RenameRule {
                        find: "txt".into(),
                        replace: "log".into(),
                        replace_target: target,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let encoded = serde_json::to_string(&presets).unwrap();
        let restored: Presets = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored, presets);
        let legacy: Presets =
            serde_json::from_str(r#"[{"name":"old","rule":{"find":"txt"}}]"#).unwrap();
        assert_eq!(
            legacy.get("old").unwrap().rule.replace_target,
            crate::rename::ReplaceTarget::WholeName
        );
        assert!(
            serde_json::from_str::<Presets>(
                r#"[{"name":"bad","rule":{"replace_target":"Unknown"}}]"#
            )
            .is_err()
        );
    }

    #[test]
    fn presets_round_trip_and_update_only_when_explicitly_requested() {
        let mut presets = Presets::default();
        let rule = RenameRule {
            prefix: "Photo_".into(),
            numbering: Some(crate::rename::Numbering {
                start: 1,
                step: 2,
                pad: 3,
            }),
            ..Default::default()
        };
        presets.add(" Photos ", rule.clone()).unwrap();
        assert!(presets.add("PHOTOS", RenameRule::default()).is_err());
        assert_eq!(presets.get("photos").unwrap().rule, rule);
        let replacement = RenameRule {
            suffix: "_done".into(),
            ..Default::default()
        };
        presets.update("PHOTOS", replacement.clone()).unwrap();
        let restored: Presets =
            serde_json::from_str(&serde_json::to_string(&presets).unwrap()).unwrap();
        assert_eq!(restored.get("Photos").unwrap().rule, replacement);
        assert!(presets.remove("photos"));
        assert!(presets.items().is_empty());
    }
    #[test]
    fn imported_rules_cannot_request_unbounded_padding_or_invalid_patterns() {
        for rule in [
            RenameRule {
                numbering: Some(crate::rename::Numbering {
                    start: 1,
                    step: 1,
                    pad: usize::MAX,
                }),
                ..Default::default()
            },
            RenameRule {
                find: "[".into(),
                regex: true,
                ..Default::default()
            },
            RenameRule {
                prefix: "x".repeat(MAX_FIELD + 1),
                ..Default::default()
            },
        ] {
            let encoded = serde_json::to_string(&vec![Preset {
                name: "unsafe".into(),
                rule,
            }])
            .unwrap();
            assert!(serde_json::from_str::<Presets>(&encoded).is_err());
        }
        let mut presets = Presets::default();
        presets.add("é", RenameRule::default()).unwrap();
        assert!(presets.add("e\u{301}", RenameRule::default()).is_err());
        for i in 1..MAX_PRESETS {
            presets.add(&i.to_string(), RenameRule::default()).unwrap();
        }
        assert!(presets.add("overflow", RenameRule::default()).is_err());
    }
}
