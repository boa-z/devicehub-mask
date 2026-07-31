//! HTTP adapter for the read-only remote key-mapping catalog.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use devicehub_core::{
    KeyMappingCatalog, validate_key_mapping_catalog, validate_key_mapping_profile_name,
};

pub type KeyMappingCatalogFuture<T> =
    Pin<Box<dyn Future<Output = Result<T, KeyMappingCatalogError>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMappingCatalogError {
    NotFound,
    Conflict,
    Invalid,
    InvalidSource,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyMappingCatalogInstall {
    pub entry_id: String,
    pub name: String,
    pub mappings: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyMappingCatalogSource {
    pub url: String,
    pub default_url: String,
    pub is_default: bool,
}

/// Host-owned catalog transport, cache, and local installation boundary.
pub trait KeyMappingCatalogRepository: Send + Sync + 'static {
    fn source(&self) -> KeyMappingCatalogFuture<KeyMappingCatalogSource>;
    fn set_source(&self, url: Option<String>) -> KeyMappingCatalogFuture<KeyMappingCatalogSource>;
    fn catalog(&self) -> KeyMappingCatalogFuture<KeyMappingCatalog>;
    fn refresh(&self) -> KeyMappingCatalogFuture<KeyMappingCatalog>;
    fn install(
        &self,
        entry_id: String,
        name: String,
    ) -> KeyMappingCatalogFuture<KeyMappingCatalogInstall>;
}

#[derive(Clone)]
pub struct KeyMappingCatalogHttpState {
    repository: Arc<dyn KeyMappingCatalogRepository>,
}

impl KeyMappingCatalogHttpState {
    pub fn new(repository: impl KeyMappingCatalogRepository) -> Self {
        Self {
            repository: Arc::new(repository),
        }
    }
}

pub fn router<S>(state: KeyMappingCatalogHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/keymap-catalog/source", get(source).put(update_source))
        .route("/api/keymap-catalog", get(catalog))
        .route("/api/keymap-catalog/refresh", post(refresh))
        .route(
            "/api/keymap-catalog/entries/{entry_id}/install",
            post(install),
        )
        .with_state(state)
}

async fn source(
    State(state): State<KeyMappingCatalogHttpState>,
) -> Result<Json<KeyMappingCatalogSource>, StatusCode> {
    state
        .repository
        .source()
        .await
        .map(Json)
        .map_err(repository_status)
}

#[derive(Deserialize)]
struct SourceRequest {
    url: Option<String>,
}

async fn update_source(
    State(state): State<KeyMappingCatalogHttpState>,
    Json(request): Json<SourceRequest>,
) -> Result<Json<KeyMappingCatalogSource>, StatusCode> {
    state
        .repository
        .set_source(request.url)
        .await
        .map(Json)
        .map_err(repository_status)
}

async fn catalog(
    State(state): State<KeyMappingCatalogHttpState>,
) -> Result<Json<KeyMappingCatalog>, StatusCode> {
    let catalog = state
        .repository
        .catalog()
        .await
        .map_err(repository_status)?;
    validate_key_mapping_catalog(&catalog).map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Json(catalog))
}

async fn refresh(
    State(state): State<KeyMappingCatalogHttpState>,
) -> Result<Json<KeyMappingCatalog>, StatusCode> {
    let catalog = state
        .repository
        .refresh()
        .await
        .map_err(repository_status)?;
    validate_key_mapping_catalog(&catalog).map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Json(catalog))
}

#[derive(Deserialize)]
struct InstallRequest {
    name: String,
}

async fn install(
    State(state): State<KeyMappingCatalogHttpState>,
    Path(entry_id): Path<String>,
    Json(request): Json<InstallRequest>,
) -> Result<Json<KeyMappingCatalogInstall>, StatusCode> {
    if entry_id.is_empty() || entry_id.len() > 128 {
        return Err(StatusCode::BAD_REQUEST);
    }
    validate_key_mapping_profile_name(&request.name).map_err(|_| StatusCode::BAD_REQUEST)?;
    let installed = state
        .repository
        .install(entry_id, request.name)
        .await
        .map_err(repository_status)?;
    Ok(Json(installed))
}

fn repository_status(error: KeyMappingCatalogError) -> StatusCode {
    match error {
        KeyMappingCatalogError::NotFound => StatusCode::NOT_FOUND,
        KeyMappingCatalogError::Conflict => StatusCode::CONFLICT,
        KeyMappingCatalogError::Invalid => StatusCode::BAD_GATEWAY,
        KeyMappingCatalogError::InvalidSource => StatusCode::BAD_REQUEST,
        KeyMappingCatalogError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::{
        KEY_MAPPING_CATALOG_SCHEMA_VERSION, KeyMappingCatalogEntry, KeyMappingCatalogMatch,
        KeyMappingCatalogOrientation, KeyMappingCatalogProfile,
        KeyMappingCatalogRepository as CatalogRepositoryMetadata, KeyMappingResolution,
    };

    #[derive(Clone)]
    struct MemoryCatalogRepository;

    impl KeyMappingCatalogRepository for MemoryCatalogRepository {
        fn source(&self) -> KeyMappingCatalogFuture<KeyMappingCatalogSource> {
            Box::pin(async {
                Ok(KeyMappingCatalogSource {
                    url: "https://example.invalid/catalog-v1.json".into(),
                    default_url: "https://example.invalid/catalog-v1.json".into(),
                    is_default: true,
                })
            })
        }

        fn set_source(
            &self,
            url: Option<String>,
        ) -> KeyMappingCatalogFuture<KeyMappingCatalogSource> {
            Box::pin(async move {
                let url = url.unwrap_or_else(|| "https://example.invalid/catalog-v1.json".into());
                Ok(KeyMappingCatalogSource {
                    is_default: url == "https://example.invalid/catalog-v1.json",
                    url,
                    default_url: "https://example.invalid/catalog-v1.json".into(),
                })
            })
        }

        fn catalog(&self) -> KeyMappingCatalogFuture<KeyMappingCatalog> {
            Box::pin(async { Ok(catalog()) })
        }

        fn refresh(&self) -> KeyMappingCatalogFuture<KeyMappingCatalog> {
            Box::pin(async { Ok(catalog()) })
        }

        fn install(
            &self,
            entry_id: String,
            name: String,
        ) -> KeyMappingCatalogFuture<KeyMappingCatalogInstall> {
            Box::pin(async move {
                Ok(KeyMappingCatalogInstall {
                    entry_id,
                    name,
                    mappings: 1,
                })
            })
        }
    }

    fn catalog() -> KeyMappingCatalog {
        KeyMappingCatalog {
            schema_version: KEY_MAPPING_CATALOG_SCHEMA_VERSION,
            repository: CatalogRepositoryMetadata {
                id: "com.devicehub.mask.keymaps".into(),
                name: "DeviceHub Mask Keymaps".into(),
                generated_at: "2026-08-01T00:00:00Z".into(),
            },
            entries: vec![KeyMappingCatalogEntry {
                id: "entry-1".into(),
                slug: "entry-1".into(),
                title: "Entry".into(),
                description: String::new(),
                author: String::new(),
                updated_at: String::new(),
                profile: KeyMappingCatalogProfile {
                    format: "devicehub-mask".into(),
                    format_version: 2,
                    url: "profiles/entry-1/profile.json".into(),
                    sha256: "a".repeat(64),
                    bytes: 1,
                },
                compatibility: KeyMappingCatalogMatch {
                    bundle_ids: vec!["com.example.game".into()],
                    stream_resolution: KeyMappingResolution {
                        width: 1,
                        height: 1,
                    },
                    orientation: KeyMappingCatalogOrientation::Portrait,
                    product_types: Vec::new(),
                },
            }],
        }
    }

    #[test]
    fn router_constructs_without_device_state() {
        let _: Router = router::<()>(KeyMappingCatalogHttpState::new(MemoryCatalogRepository));
    }

    #[tokio::test]
    async fn install_rejects_an_invalid_local_profile_name() {
        let state = KeyMappingCatalogHttpState::new(MemoryCatalogRepository);
        assert_eq!(
            install(
                State(state),
                Path("entry-1".into()),
                Json(InstallRequest {
                    name: "../invalid".into(),
                }),
            )
            .await
            .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn source_reset_is_exposed_without_device_state() {
        let state = KeyMappingCatalogHttpState::new(MemoryCatalogRepository);
        let source = update_source(State(state), Json(SourceRequest { url: None }))
            .await
            .unwrap()
            .0;
        assert!(source.is_default);
    }
}
