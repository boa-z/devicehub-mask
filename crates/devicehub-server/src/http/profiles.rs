//! HTTP adapter for key-mapping profile persistence.
//!
//! Profiles are desktop-local files. This module owns their HTTP validation and
//! storage rules without access to a device session or other application state.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;

use devicehub_core::{
    KeyMappingProfile as Profile, KeyMappingResolution, validate_key_mapping_profile,
    validate_key_mapping_profile_name,
};
use devicehub_keymap::validate_profile_scripts;

pub type ProfileRepositoryFuture<T> =
    Pin<Box<dyn Future<Output = Result<T, ProfileRepositoryError>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileRepositoryError {
    NotFound,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProfile {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileRepositorySnapshot {
    pub profiles: Vec<StoredProfile>,
    pub active: Option<String>,
}

pub trait ProfileRepository: Send + Sync + 'static {
    fn snapshot(&self) -> ProfileRepositoryFuture<ProfileRepositorySnapshot>;
    fn read(&self, name: String) -> ProfileRepositoryFuture<Vec<u8>>;
    fn write(&self, name: String, bytes: Vec<u8>) -> ProfileRepositoryFuture<()>;
    fn exists(&self, name: String) -> ProfileRepositoryFuture<bool>;
    fn active(&self) -> ProfileRepositoryFuture<Option<String>>;
    fn set_active(&self, name: String) -> ProfileRepositoryFuture<()>;
    fn delete(&self, name: String) -> ProfileRepositoryFuture<()>;
}

#[derive(Clone)]
pub struct ProfileHttpState {
    repository: Arc<dyn ProfileRepository>,
}

impl ProfileHttpState {
    pub fn new(repository: impl ProfileRepository) -> Self {
        Self {
            repository: Arc::new(repository),
        }
    }
}

#[derive(Debug, Serialize)]
struct AppProfileBinding {
    bundle_id: String,
    profile: String,
    target_resolution: Option<KeyMappingResolution>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct AppBindingConflict {
    bundle_id: String,
    target_resolution: Option<KeyMappingResolution>,
}

#[derive(Debug, Serialize)]
struct ProfileList {
    profiles: Vec<String>,
    active: String,
    app_bindings: Vec<AppProfileBinding>,
    binding_conflicts: Vec<AppBindingConflict>,
}

/// Injects profile-only state before these routes join the private API.
pub fn router<S>(state: ProfileHttpState) -> Router<S>
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

fn validate_profile_name(name: &str) -> Result<(), StatusCode> {
    validate_key_mapping_profile_name(name).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(())
}

async fn list_profiles(
    State(state): State<ProfileHttpState>,
) -> Result<Json<ProfileList>, StatusCode> {
    let mut snapshot = state
        .repository
        .snapshot()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    snapshot
        .profiles
        .sort_by(|left, right| left.name.cmp(&right.name));
    let mut profiles = Vec::new();
    let mut app_bindings = BTreeMap::new();
    let mut binding_conflicts = BTreeSet::new();
    for stored in snapshot.profiles {
        if validate_profile_name(&stored.name).is_err() {
            continue;
        }
        let Ok(profile) = serde_json::from_slice::<Profile>(&stored.bytes) else {
            continue;
        };
        if profile.name != stored.name
            || validate_key_mapping_profile(&profile).is_err()
            || validate_profile_scripts(&profile).is_err()
        {
            continue;
        }
        profiles.push(stored.name.clone());
        for bundle_id in profile.bundle_identifiers {
            let key = (bundle_id, profile.target_resolution);
            if binding_conflicts.contains(&key) {
                continue;
            }
            if app_bindings
                .insert(key.clone(), stored.name.clone())
                .is_some()
            {
                app_bindings.remove(&key);
                binding_conflicts.insert(key);
            }
        }
    }
    let requested_active = snapshot
        .active
        .map(|name| name.trim().to_string())
        .filter(|name| validate_profile_name(name).is_ok())
        .unwrap_or_else(|| "default".into());
    let active = if profiles.contains(&requested_active) {
        requested_active
    } else {
        profiles
            .first()
            .cloned()
            .unwrap_or_else(|| "default".into())
    };
    let app_bindings = app_bindings
        .into_iter()
        .map(
            |((bundle_id, target_resolution), profile)| AppProfileBinding {
                bundle_id,
                profile,
                target_resolution,
            },
        )
        .collect();
    let binding_conflicts = binding_conflicts
        .into_iter()
        .map(|(bundle_id, target_resolution)| AppBindingConflict {
            bundle_id,
            target_resolution,
        })
        .collect();
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
    validate_profile_name(&name)?;
    let bytes = state
        .repository
        .read(name.clone())
        .await
        .map_err(repository_status)?;
    let profile: Profile =
        serde_json::from_slice(&bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    validate_key_mapping_profile(&profile).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    if profile.name != name {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    validate_profile_scripts(&profile).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    Ok(Json(profile))
}

async fn save_profile(
    State(state): State<ProfileHttpState>,
    Path(name): Path<String>,
    Json(profile): Json<Profile>,
) -> Result<StatusCode, StatusCode> {
    validate_profile_name(&name)?;
    if profile.name != name {
        return Err(StatusCode::BAD_REQUEST);
    }
    validate_key_mapping_profile(&profile).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    validate_profile_scripts(&profile).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    let bytes = serde_json::to_vec_pretty(&profile).map_err(|_| StatusCode::BAD_REQUEST)?;
    state
        .repository
        .write(name, bytes)
        .await
        .map_err(repository_status)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn activate_profile(
    State(state): State<ProfileHttpState>,
    Path(name): Path<String>,
) -> Result<StatusCode, StatusCode> {
    validate_profile_name(&name)?;
    if !state
        .repository
        .exists(name.clone())
        .await
        .map_err(repository_status)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    state
        .repository
        .set_active(name)
        .await
        .map_err(repository_status)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_profile(
    State(state): State<ProfileHttpState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    validate_profile_name(&name)?;
    let active = state
        .repository
        .active()
        .await
        .map_err(repository_status)?
        .map(|active| active.trim().to_string())
        .filter(|active| validate_profile_name(active).is_ok())
        .unwrap_or_else(|| "default".into());
    if active == name {
        return Err(StatusCode::CONFLICT);
    }
    state
        .repository
        .delete(name.clone())
        .await
        .map_err(repository_status)?;
    Ok(Json(json!({ "deleted": name })))
}

fn repository_status(error: ProfileRepositoryError) -> StatusCode {
    match error {
        ProfileRepositoryError::NotFound => StatusCode::NOT_FOUND,
        ProfileRepositoryError::Unavailable => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryProfileState {
        profiles: BTreeMap<String, Vec<u8>>,
        active: Option<String>,
    }

    #[derive(Clone, Default)]
    struct MemoryProfileRepository(Arc<Mutex<MemoryProfileState>>);

    impl ProfileRepository for MemoryProfileRepository {
        fn snapshot(&self) -> ProfileRepositoryFuture<ProfileRepositorySnapshot> {
            let state = self.0.lock().unwrap();
            let snapshot = ProfileRepositorySnapshot {
                profiles: state
                    .profiles
                    .iter()
                    .map(|(name, bytes)| StoredProfile {
                        name: name.clone(),
                        bytes: bytes.clone(),
                    })
                    .collect(),
                active: state.active.clone(),
            };
            Box::pin(async move { Ok(snapshot) })
        }

        fn read(&self, name: String) -> ProfileRepositoryFuture<Vec<u8>> {
            let result = self
                .0
                .lock()
                .unwrap()
                .profiles
                .get(&name)
                .cloned()
                .ok_or(ProfileRepositoryError::NotFound);
            Box::pin(async move { result })
        }

        fn write(&self, name: String, bytes: Vec<u8>) -> ProfileRepositoryFuture<()> {
            self.0.lock().unwrap().profiles.insert(name, bytes);
            Box::pin(async { Ok(()) })
        }

        fn exists(&self, name: String) -> ProfileRepositoryFuture<bool> {
            let exists = self.0.lock().unwrap().profiles.contains_key(&name);
            Box::pin(async move { Ok(exists) })
        }

        fn active(&self) -> ProfileRepositoryFuture<Option<String>> {
            let active = self.0.lock().unwrap().active.clone();
            Box::pin(async move { Ok(active) })
        }

        fn set_active(&self, name: String) -> ProfileRepositoryFuture<()> {
            self.0.lock().unwrap().active = Some(name);
            Box::pin(async { Ok(()) })
        }

        fn delete(&self, name: String) -> ProfileRepositoryFuture<()> {
            let result = self
                .0
                .lock()
                .unwrap()
                .profiles
                .remove(&name)
                .map(|_| ())
                .ok_or(ProfileRepositoryError::NotFound);
            Box::pin(async move { result })
        }
    }

    fn test_state() -> ProfileHttpState {
        ProfileHttpState::new(MemoryProfileRepository::default())
    }

    fn profile(name: &str) -> Profile {
        Profile {
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
            hardware_bindings: devicehub_core::default_hardware_bindings(),
        }
    }

    #[test]
    fn profile_names_are_confined_before_repository_access() {
        for name in ["", "../escape", "nested/name", "with space", ".hidden"] {
            assert_eq!(validate_profile_name(name), Err(StatusCode::BAD_REQUEST));
        }
        assert_eq!(validate_profile_name("game_profile-1"), Ok(()));
    }

    #[tokio::test]
    async fn profile_management_tracks_active_and_protects_it_from_delete() {
        let state = test_state();
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
        assert_eq!(list.app_bindings.len(), 1);
        assert_eq!(list.app_bindings[0].bundle_id, "com.example.game");
        assert_eq!(list.app_bindings[0].profile, "game");
        assert_eq!(
            list.app_bindings[0].target_resolution,
            Some(KeyMappingResolution {
                width: 1290,
                height: 2796,
            })
        );
        assert!(list.binding_conflicts.is_empty());

        let mut duplicate = profile("duplicate");
        duplicate.bundle_identifiers = vec!["com.example.game".into()];
        duplicate.target_resolution = Some(KeyMappingResolution {
            width: 1290,
            height: 2796,
        });
        save_profile(
            State(state.clone()),
            Path("duplicate".into()),
            Json(duplicate),
        )
        .await
        .unwrap();
        let conflicted = list_profiles(State(state.clone())).await.unwrap().0;
        assert!(conflicted.app_bindings.is_empty());
        assert_eq!(
            conflicted.binding_conflicts,
            vec![AppBindingConflict {
                bundle_id: "com.example.game".into(),
                target_resolution: Some(KeyMappingResolution {
                    width: 1290,
                    height: 2796,
                }),
            }]
        );
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
    }

    #[tokio::test]
    async fn profile_save_rejects_invalid_script_syntax_and_name_mismatches() {
        let state = test_state();
        let mut invalid_script = profile("script");
        invalid_script.mappings = vec![json!({
            "id": "macro",
            "type": "Script",
            "position": { "x": 0.5, "y": 0.5 },
            "bind": ["KeyM"],
            "interval": 20,
            "pressed_script": "if {",
            "held_script": "",
            "released_script": ""
        })];
        assert_eq!(
            save_profile(
                State(state.clone()),
                Path("script".into()),
                Json(invalid_script),
            )
            .await,
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        );
        assert_eq!(
            save_profile(
                State(state),
                Path("file-name".into()),
                Json(profile("embedded-name")),
            )
            .await,
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[tokio::test]
    async fn same_app_can_bind_distinct_device_resolutions() {
        let state = test_state();
        for (name, width, height) in [("iphone", 1290, 2796), ("ipad", 1620, 2160)] {
            let mut value = profile(name);
            value.bundle_identifiers = vec!["com.example.game".into()];
            value.target_resolution = Some(KeyMappingResolution { width, height });
            save_profile(State(state.clone()), Path(name.into()), Json(value))
                .await
                .unwrap();
        }

        let list = list_profiles(State(state)).await.unwrap().0;
        assert_eq!(list.app_bindings.len(), 2);
        assert!(list.binding_conflicts.is_empty());
        assert!(list.app_bindings.iter().any(|binding| {
            binding.profile == "iphone"
                && binding.target_resolution
                    == Some(KeyMappingResolution {
                        width: 1290,
                        height: 2796,
                    })
        }));
        assert!(list.app_bindings.iter().any(|binding| {
            binding.profile == "ipad"
                && binding.target_resolution
                    == Some(KeyMappingResolution {
                        width: 1620,
                        height: 2160,
                    })
        }));
    }

    #[derive(Clone, Copy)]
    struct FailingProfileRepository;

    impl ProfileRepository for FailingProfileRepository {
        fn snapshot(&self) -> ProfileRepositoryFuture<ProfileRepositorySnapshot> {
            Box::pin(async { Err(ProfileRepositoryError::Unavailable) })
        }
        fn read(&self, _: String) -> ProfileRepositoryFuture<Vec<u8>> {
            Box::pin(async { Err(ProfileRepositoryError::Unavailable) })
        }
        fn write(&self, _: String, _: Vec<u8>) -> ProfileRepositoryFuture<()> {
            Box::pin(async { Err(ProfileRepositoryError::Unavailable) })
        }
        fn exists(&self, _: String) -> ProfileRepositoryFuture<bool> {
            Box::pin(async { Err(ProfileRepositoryError::Unavailable) })
        }
        fn active(&self) -> ProfileRepositoryFuture<Option<String>> {
            Box::pin(async { Err(ProfileRepositoryError::Unavailable) })
        }
        fn set_active(&self, _: String) -> ProfileRepositoryFuture<()> {
            Box::pin(async { Err(ProfileRepositoryError::Unavailable) })
        }
        fn delete(&self, _: String) -> ProfileRepositoryFuture<()> {
            Box::pin(async { Err(ProfileRepositoryError::Unavailable) })
        }
    }

    #[tokio::test]
    async fn repository_failures_are_bounded_http_errors() {
        let state = ProfileHttpState::new(FailingProfileRepository);
        assert_eq!(
            list_profiles(State(state.clone())).await.unwrap_err(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            load_profile(State(state), Path("default".into()))
                .await
                .unwrap_err(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn profile_router_constructs_without_device_state() {
        let state = test_state();
        let _: Router = router(state);
    }

    #[test]
    fn profile_state_is_lightweight_and_has_no_runtime_owner() {
        assert_eq!(
            std::mem::size_of::<ProfileHttpState>(),
            std::mem::size_of::<Arc<dyn ProfileRepository>>()
        );
    }
}
