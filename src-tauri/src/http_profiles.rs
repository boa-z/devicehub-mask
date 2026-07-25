//! HTTP adapter for key-mapping profile persistence.
//!
//! Profiles are desktop-local files. This module owns their HTTP validation and
//! storage rules without access to a device session or other application state.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::protocol::HARDWARE_BUTTON_NAMES;

#[derive(Clone)]
pub(crate) struct ProfileHttpState {
    profile_dir: Arc<PathBuf>,
}

impl ProfileHttpState {
    pub(crate) fn new(profile_dir: PathBuf) -> Self {
        Self {
            profile_dir: Arc::new(profile_dir),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Profile {
    version: u8,
    name: String,
    mappings: Vec<serde_json::Value>,
    #[serde(default, rename = "bundleIdentifiers")]
    bundle_identifiers: Vec<String>,
    #[serde(default = "default_hardware_bindings", rename = "hardwareBindings")]
    hardware_bindings: BTreeMap<String, String>,
}

fn default_hardware_bindings() -> BTreeMap<String, String> {
    HARDWARE_BUTTON_NAMES
        .into_iter()
        .map(|name| (name.to_string(), String::new()))
        .collect()
}

#[derive(Serialize)]
struct ProfileList {
    profiles: Vec<String>,
    active: String,
    app_bindings: BTreeMap<String, String>,
    binding_conflicts: Vec<String>,
}

/// Injects profile-only state before these routes join the private API.
pub(crate) fn router<S>(state: ProfileHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/profiles", get(list_profiles))
        .route("/api/profiles/{name}", get(load_profile).put(save_profile))
        .route("/api/profiles/{name}/activate", put(activate_profile))
        .route("/api/profiles/{name}/delete", put(delete_profile))
        .with_state(state)
}

fn profile_path(state: &ProfileHttpState, name: &str) -> Result<PathBuf, StatusCode> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(state.profile_dir.join(format!("{name}.json")))
}

fn active_profile_path(state: &ProfileHttpState) -> PathBuf {
    state.profile_dir.join(".active-profile")
}

async fn active_profile_name(state: &ProfileHttpState) -> String {
    tokio::fs::read_to_string(active_profile_path(state))
        .await
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| profile_path(state, name).is_ok())
        .unwrap_or_else(|| "default".into())
}

async fn list_profiles(
    State(state): State<ProfileHttpState>,
) -> Result<Json<ProfileList>, StatusCode> {
    tokio::fs::create_dir_all(state.profile_dir.as_ref())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut entries = tokio::fs::read_dir(state.profile_dir.as_ref())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut profiles = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        if let Some(name) = path.file_stem().and_then(|name| name.to_str())
            && profile_path(&state, name).is_ok()
        {
            profiles.push(name.to_string());
        }
    }
    profiles.sort();
    let requested_active = active_profile_name(&state).await;
    let active = if profiles.contains(&requested_active) {
        requested_active
    } else {
        profiles
            .first()
            .cloned()
            .unwrap_or_else(|| "default".into())
    };
    let mut app_bindings = BTreeMap::new();
    let mut binding_conflicts = HashSet::new();
    for name in &profiles {
        let Ok(bytes) = tokio::fs::read(profile_path(&state, name)?).await else {
            continue;
        };
        let Ok(profile) = serde_json::from_slice::<Profile>(&bytes) else {
            continue;
        };
        if validate_profile(&profile).is_err() {
            continue;
        }
        for bundle_id in profile.bundle_identifiers {
            if binding_conflicts.contains(&bundle_id) {
                continue;
            }
            if app_bindings
                .insert(bundle_id.clone(), name.clone())
                .is_some()
            {
                app_bindings.remove(&bundle_id);
                binding_conflicts.insert(bundle_id);
            }
        }
    }
    let mut binding_conflicts = binding_conflicts.into_iter().collect::<Vec<_>>();
    binding_conflicts.sort();
    Ok(Json(ProfileList {
        profiles,
        active,
        app_bindings,
        binding_conflicts,
    }))
}

async fn load_profile(
    State(state): State<ProfileHttpState>,
    Path(name): Path<String>,
) -> Result<Json<Profile>, StatusCode> {
    let path = profile_path(&state, &name)?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
    let profile: Profile =
        serde_json::from_slice(&bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    validate_profile(&profile)?;
    Ok(Json(profile))
}

async fn save_profile(
    State(state): State<ProfileHttpState>,
    Path(name): Path<String>,
    Json(profile): Json<Profile>,
) -> Result<StatusCode, StatusCode> {
    let path = profile_path(&state, &name)?;
    validate_profile(&profile)?;
    tokio::fs::create_dir_all(state.profile_dir.as_ref())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let bytes = serde_json::to_vec_pretty(&profile).map_err(|_| StatusCode::BAD_REQUEST)?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn activate_profile(
    State(state): State<ProfileHttpState>,
    Path(name): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let path = profile_path(&state, &name)?;
    if !tokio::fs::try_exists(path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    tokio::fs::write(active_profile_path(&state), name)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_profile(
    State(state): State<ProfileHttpState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let path = profile_path(&state, &name)?;
    if active_profile_name(&state).await == name {
        return Err(StatusCode::CONFLICT);
    }
    tokio::fs::remove_file(path)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
    Ok(Json(json!({ "deleted": name })))
}

fn validate_profile(profile: &Profile) -> Result<(), StatusCode> {
    if profile.version != 1
        || profile.name.is_empty()
        || profile.mappings.len() > 512
        || profile.hardware_bindings.len() != HARDWARE_BUTTON_NAMES.len()
        || profile.bundle_identifiers.len() > 32
        || HARDWARE_BUTTON_NAMES
            .iter()
            .any(|name| !profile.hardware_bindings.contains_key(*name))
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let mut bundle_identifiers = HashSet::new();
    if profile.bundle_identifiers.iter().any(|bundle_id| {
        !valid_bundle_identifier(bundle_id) || !bundle_identifiers.insert(bundle_id.as_str())
    }) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let mut ids = HashSet::new();
    for mapping in &profile.mappings {
        let Some(mapping) = mapping.as_object() else {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        };
        let id = mapping.get("id").and_then(serde_json::Value::as_str);
        let mapping_type = mapping.get("type").and_then(serde_json::Value::as_str);
        if id.is_none_or(str::is_empty)
            || !ids.insert(id.unwrap())
            || !mapping_type.is_some_and(valid_mapping_type)
            || !valid_mapping_positions(mapping)
        {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
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
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
    }
    Ok(())
}

fn valid_bundle_identifier(bundle_id: &str) -> bool {
    !bundle_id.is_empty()
        && bundle_id.len() <= 255
        && bundle_id.contains('.')
        && bundle_id.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
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

    fn test_state() -> (ProfileHttpState, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-profile-test-{}",
            uuid::Uuid::new_v4()
        ));
        (ProfileHttpState::new(directory.clone()), directory)
    }

    fn profile(name: &str) -> Profile {
        Profile {
            version: 1,
            name: name.into(),
            mappings: Vec::new(),
            bundle_identifiers: if name == "game" {
                vec!["com.example.game".into()]
            } else {
                Vec::new()
            },
            hardware_bindings: default_hardware_bindings(),
        }
    }

    #[test]
    fn legacy_profile_gets_empty_hardware_bindings() {
        let profile: Profile = serde_json::from_value(json!({
            "version": 1,
            "name": "legacy",
            "mappings": []
        }))
        .unwrap();

        assert_eq!(profile.hardware_bindings, default_hardware_bindings());
        assert!(validate_profile(&profile).is_ok());
    }

    #[test]
    fn profile_rejects_duplicate_hardware_shortcuts() {
        let mut profile = profile("duplicate");
        profile
            .hardware_bindings
            .insert("home".into(), "KeyH".into());
        profile
            .hardware_bindings
            .insert("lock".into(), "KeyH".into());

        assert!(validate_profile(&profile).is_err());
    }

    #[test]
    fn profile_rejects_hardware_and_touch_shortcut_conflict() {
        let mut profile = profile("conflict");
        profile.mappings = vec![json!({
            "id": "touch", "type": "touch", "label": "Touch",
            "contactId": 0, "x": 0.5, "y": 0.5, "key": "KeyH"
        })];
        profile
            .hardware_bindings
            .insert("home".into(), "KeyH".into());

        assert!(validate_profile(&profile).is_err());
    }

    #[test]
    fn profile_names_are_confined_to_the_profile_directory() {
        let (state, _) = test_state();
        for name in ["", "../escape", "nested/name", "with space", ".hidden"] {
            assert_eq!(profile_path(&state, name), Err(StatusCode::BAD_REQUEST));
        }
        assert_eq!(
            profile_path(&state, "game_profile-1").unwrap(),
            state.profile_dir.join("game_profile-1.json")
        );
    }

    #[tokio::test]
    async fn profile_management_tracks_active_and_protects_it_from_delete() {
        let (state, directory) = test_state();
        save_profile(
            State(state.clone()),
            Path("default".into()),
            Json(profile("default")),
        )
        .await
        .unwrap();
        save_profile(
            State(state.clone()),
            Path("game".into()),
            Json(profile("game")),
        )
        .await
        .unwrap();
        activate_profile(State(state.clone()), Path("game".into()))
            .await
            .unwrap();

        let list = list_profiles(State(state.clone())).await.unwrap().0;
        assert_eq!(list.profiles, vec!["default", "game"]);
        assert_eq!(list.active, "game");
        assert_eq!(
            list.app_bindings
                .get("com.example.game")
                .map(String::as_str),
            Some("game")
        );
        assert!(list.binding_conflicts.is_empty());

        let mut duplicate = profile("duplicate");
        duplicate.bundle_identifiers = vec!["com.example.game".into()];
        save_profile(
            State(state.clone()),
            Path("duplicate".into()),
            Json(duplicate),
        )
        .await
        .unwrap();
        let conflicted = list_profiles(State(state.clone())).await.unwrap().0;
        assert!(!conflicted.app_bindings.contains_key("com.example.game"));
        assert_eq!(conflicted.binding_conflicts, vec!["com.example.game"]);
        let _ = delete_profile(State(state.clone()), Path("duplicate".into()))
            .await
            .unwrap();
        assert!(matches!(
            delete_profile(State(state.clone()), Path("game".into())).await,
            Err(StatusCode::CONFLICT)
        ));

        activate_profile(State(state.clone()), Path("default".into()))
            .await
            .unwrap();
        let deleted = delete_profile(State(state.clone()), Path("game".into()))
            .await
            .unwrap();
        assert_eq!(deleted.0["deleted"], "game");
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[test]
    fn profile_router_constructs_without_device_state() {
        let (state, _) = test_state();
        let _: Router = router(state);
    }

    #[test]
    fn profile_state_is_lightweight_and_has_no_runtime_owner() {
        assert_eq!(
            std::mem::size_of::<ProfileHttpState>(),
            std::mem::size_of::<Arc<PathBuf>>()
        );
    }
}
