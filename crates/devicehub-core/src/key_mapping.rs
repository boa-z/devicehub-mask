//! Host-independent key-mapping profile model and validation policy.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{HARDWARE_BUTTON_NAMES, validate_app_bundle_id};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct KeyMappingResolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyMappingProfile {
    pub version: u8,
    pub name: String,
    pub mappings: Vec<serde_json::Value>,
    #[serde(default, rename = "bundleIdentifiers")]
    pub bundle_identifiers: Vec<String>,
    #[serde(default, rename = "targetResolution")]
    pub target_resolution: Option<KeyMappingResolution>,
    #[serde(default = "default_hardware_bindings", rename = "hardwareBindings")]
    pub hardware_bindings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidKeyMappingProfile;

impl fmt::Display for InvalidKeyMappingProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid key-mapping profile")
    }
}

impl std::error::Error for InvalidKeyMappingProfile {}

pub fn default_hardware_bindings() -> BTreeMap<String, String> {
    HARDWARE_BUTTON_NAMES
        .into_iter()
        .map(|name| (name.to_string(), String::new()))
        .collect()
}

pub fn validate_key_mapping_profile_name(name: &str) -> Result<(), InvalidKeyMappingProfile> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(InvalidKeyMappingProfile);
    }
    Ok(())
}

pub fn validate_key_mapping_profile(
    profile: &KeyMappingProfile,
) -> Result<(), InvalidKeyMappingProfile> {
    if profile.version != 2
        || profile.name.is_empty()
        || profile.mappings.len() > 512
        || profile.hardware_bindings.len() != HARDWARE_BUTTON_NAMES.len()
        || profile.bundle_identifiers.len() > 32
        || profile.bundle_identifiers.is_empty() != profile.target_resolution.is_none()
        || profile.target_resolution.is_some_and(|resolution| {
            resolution.width == 0
                || resolution.height == 0
                || resolution.width > 16_384
                || resolution.height > 16_384
        })
        || HARDWARE_BUTTON_NAMES
            .iter()
            .any(|name| !profile.hardware_bindings.contains_key(*name))
    {
        return Err(InvalidKeyMappingProfile);
    }
    let mut bundle_identifiers = HashSet::new();
    if profile.bundle_identifiers.iter().any(|bundle_id| {
        validate_app_bundle_id(bundle_id).is_err() || !bundle_identifiers.insert(bundle_id.as_str())
    }) {
        return Err(InvalidKeyMappingProfile);
    }
    let mut ids = HashSet::new();
    for mapping in &profile.mappings {
        let Some(mapping) = mapping.as_object() else {
            return Err(InvalidKeyMappingProfile);
        };
        let id = mapping.get("id").and_then(serde_json::Value::as_str);
        let mapping_type = mapping.get("type").and_then(serde_json::Value::as_str);
        if id.is_none_or(str::is_empty)
            || !ids.insert(id.expect("mapping id checked above"))
            || !mapping_type.is_some_and(valid_mapping_type)
            || !valid_mapping_positions(mapping)
        {
            return Err(InvalidKeyMappingProfile);
        }
    }
    let mut mapping_keys = HashSet::new();
    for mapping in &profile.mappings {
        collect_mapping_keys(mapping, &mut mapping_keys);
    }
    let mut hardware_keys = HashSet::new();
    for key in profile.hardware_bindings.values() {
        if key.len() > 64
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
            || (!key.is_empty() && mapping_keys.contains(key.as_str()))
            || (!key.is_empty() && !hardware_keys.insert(key))
        {
            return Err(InvalidKeyMappingProfile);
        }
    }
    Ok(())
}

fn valid_mapping_type(mapping_type: &str) -> bool {
    matches!(
        mapping_type,
        "touch"
            | "dpad"
            | "SingleTap"
            | "RepeatTap"
            | "MultipleTap"
            | "Swipe"
            | "DirectionPad"
            | "MouseCastSpell"
            | "PadCastSpell"
            | "CancelCast"
            | "Observation"
            | "Fps"
            | "Fire"
            | "RawInput"
            | "Script"
    )
}

fn valid_mapping_positions(mapping: &serde_json::Map<String, serde_json::Value>) -> bool {
    fn valid_position(value: &serde_json::Value) -> bool {
        let Some(point) = value.as_object() else {
            return false;
        };
        let Some(x) = point.get("x").and_then(serde_json::Value::as_f64) else {
            return false;
        };
        let Some(y) = point.get("y").and_then(serde_json::Value::as_f64) else {
            return false;
        };
        x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)
    }
    let primary = if mapping.contains_key("position") {
        mapping.get("position").is_some_and(valid_position)
    } else {
        mapping
            .get("x")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|x| (0.0..=1.0).contains(&x))
            && mapping
                .get("y")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|y| (0.0..=1.0).contains(&y))
    };
    primary
        && mapping.get("center").is_none_or(valid_position)
        && mapping.get("positions").is_none_or(|values| {
            values
                .as_array()
                .is_some_and(|values| values.iter().all(valid_position))
        })
        && mapping.get("items").is_none_or(|values| {
            values.as_array().is_some_and(|values| {
                values
                    .iter()
                    .all(|item| item.get("position").is_some_and(valid_position))
            })
        })
}

fn collect_mapping_keys<'a>(value: &'a serde_json::Value, keys: &mut HashSet<&'a str>) {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .for_each(|value| collect_mapping_keys(value, keys)),
        serde_json::Value::Object(values) => {
            for (name, value) in values {
                if name == "key"
                    || name == "bind"
                    || name.ends_with("_bind")
                    || matches!(name.as_str(), "up" | "down" | "left" | "right")
                {
                    collect_mapping_keys(value, keys);
                }
            }
        }
        serde_json::Value::String(value) if !value.is_empty() => {
            keys.insert(value);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(name: &str) -> KeyMappingProfile {
        KeyMappingProfile {
            version: 2,
            name: name.into(),
            mappings: Vec::new(),
            bundle_identifiers: if name == "game" {
                vec!["com.example.game".into()]
            } else {
                Vec::new()
            },
            target_resolution: if name == "game" {
                Some(KeyMappingResolution {
                    width: 1290,
                    height: 2796,
                })
            } else {
                None
            },
            hardware_bindings: default_hardware_bindings(),
        }
    }

    #[test]
    fn version_one_profiles_are_rejected() {
        let profile: KeyMappingProfile = serde_json::from_value(json!({
            "version": 1,
            "name": "legacy",
            "mappings": []
        }))
        .unwrap();

        assert!(validate_key_mapping_profile(&profile).is_err());
    }

    #[test]
    fn app_bindings_require_a_bounded_target_resolution() {
        let mut value = profile("game");
        value.target_resolution = None;
        assert!(validate_key_mapping_profile(&value).is_err());

        value.bundle_identifiers.clear();
        value.target_resolution = Some(KeyMappingResolution {
            width: 1290,
            height: 2796,
        });
        assert!(validate_key_mapping_profile(&value).is_err());

        value.target_resolution = None;
        assert!(validate_key_mapping_profile(&value).is_ok());
    }

    #[test]
    fn hardware_shortcuts_are_unique_and_do_not_overlap_touch_mappings() {
        let mut duplicate = profile("duplicate");
        duplicate
            .hardware_bindings
            .insert("home".into(), "KeyH".into());
        duplicate
            .hardware_bindings
            .insert("lock".into(), "KeyH".into());
        assert!(validate_key_mapping_profile(&duplicate).is_err());

        let mut conflict = profile("conflict");
        conflict.mappings = vec![json!({
            "id": "touch", "type": "touch", "label": "Touch",
            "contactId": 0, "x": 0.5, "y": 0.5, "key": "KeyH"
        })];
        conflict
            .hardware_bindings
            .insert("home".into(), "KeyH".into());
        assert!(validate_key_mapping_profile(&conflict).is_err());
    }

    #[test]
    fn profile_names_are_bounded_path_components() {
        for name in ["", "../escape", "nested/name", "with space", ".hidden"] {
            assert!(validate_key_mapping_profile_name(name).is_err());
        }
        assert!(validate_key_mapping_profile_name("game_profile-1").is_ok());
    }
}
