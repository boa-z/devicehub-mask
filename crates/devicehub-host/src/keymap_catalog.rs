//! Native HTTPS, cache, and source-preference implementation for key-mapping catalogs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use devicehub_core::{
    KeyMappingCatalog, KeyMappingProfile, MAX_KEY_MAPPING_CATALOG_BYTES,
    MAX_REMOTE_KEY_MAPPING_PROFILE_BYTES, validate_key_mapping_catalog,
    validate_key_mapping_profile,
};
use devicehub_server::http::{
    KeyMappingCatalogError, KeyMappingCatalogFuture, KeyMappingCatalogInstall,
    KeyMappingCatalogRepository, KeyMappingCatalogSource, ProfileRepository,
    ProfileRepositoryError,
};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::profile_files::TokioProfileRepository;

pub const OFFICIAL_KEY_MAPPING_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/boa-z/devicehub-mask-keymaps/main/catalog-v1.json";

const MAX_CATALOG_SOURCE_URL_BYTES: usize = 2_048;
const CATALOG_CACHE_DIRECTORY: &str = "catalogs";
const SOURCE_CONFIG_FILE: &str = "source.json";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogSourceConfig {
    url: String,
}

#[derive(Clone)]
pub struct TokioKeyMappingCatalogRepository {
    cache_dir: Arc<PathBuf>,
    catalog_url: Arc<RwLock<reqwest::Url>>,
    profiles: TokioProfileRepository,
    client: reqwest::Client,
}

impl TokioKeyMappingCatalogRepository {
    pub fn official(cache_dir: PathBuf, profiles: TokioProfileRepository) -> Self {
        let catalog_url = load_catalog_source(&cache_dir);
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .redirect(Policy::limited(3))
            .build()
            .expect("build key-mapping catalog HTTP client");
        Self {
            cache_dir: Arc::new(cache_dir),
            catalog_url: Arc::new(RwLock::new(catalog_url)),
            profiles,
            client,
        }
    }

    async fn source_url(&self) -> reqwest::Url {
        self.catalog_url.read().await.clone()
    }

    async fn source_status(&self) -> KeyMappingCatalogSource {
        let url = self.source_url().await;
        KeyMappingCatalogSource {
            is_default: url.as_str() == OFFICIAL_KEY_MAPPING_CATALOG_URL,
            url: url.into(),
            default_url: OFFICIAL_KEY_MAPPING_CATALOG_URL.into(),
        }
    }

    async fn update_source(
        &self,
        url: Option<String>,
    ) -> Result<KeyMappingCatalogSource, KeyMappingCatalogError> {
        let url = match url.map(|value| value.trim().to_string()) {
            Some(value) if !value.is_empty() => parse_catalog_source(&value)?,
            _ => official_catalog_url(),
        };
        self.write_source_config(&url).await?;
        *self.catalog_url.write().await = url;
        Ok(self.source_status().await)
    }

    fn cache_path(&self, catalog_url: &reqwest::Url) -> PathBuf {
        let fingerprint = format!("{:x}", Sha256::digest(catalog_url.as_str().as_bytes()));
        self.cache_dir
            .join(CATALOG_CACHE_DIRECTORY)
            .join(format!("{fingerprint}.json"))
    }

    fn source_config_path(&self) -> PathBuf {
        self.cache_dir.join(SOURCE_CONFIG_FILE)
    }

    async fn read_cached_catalog(
        &self,
        catalog_url: &reqwest::Url,
    ) -> Result<KeyMappingCatalog, KeyMappingCatalogError> {
        read_cached_catalog_file(&self.cache_path(catalog_url)).await
    }

    async fn fetch_bytes(
        &self,
        url: reqwest::Url,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, KeyMappingCatalogError> {
        if url.scheme() != "https" {
            return Err(KeyMappingCatalogError::Invalid);
        }
        let mut response = self.client.get(url).send().await.map_err(|error| {
            tracing::warn!(%error, "key-mapping catalog request failed");
            KeyMappingCatalogError::Unavailable
        })?;
        if response.url().scheme() != "https" {
            return Err(KeyMappingCatalogError::Invalid);
        }
        if !response.status().is_success() {
            tracing::warn!(status = %response.status(), "key-mapping catalog request was rejected");
            return Err(KeyMappingCatalogError::Unavailable);
        }
        if response
            .content_length()
            .is_some_and(|length| length > maximum_bytes as u64)
        {
            return Err(KeyMappingCatalogError::Invalid);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            tracing::warn!(%error, "key-mapping catalog response stopped early");
            KeyMappingCatalogError::Unavailable
        })? {
            if bytes.len().saturating_add(chunk.len()) > maximum_bytes {
                return Err(KeyMappingCatalogError::Invalid);
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(KeyMappingCatalogError::Invalid);
        }
        Ok(bytes)
    }

    async fn write_cached_catalog(
        &self,
        catalog_url: &reqwest::Url,
        bytes: &[u8],
    ) -> Result<(), KeyMappingCatalogError> {
        write_local_file(&self.cache_path(catalog_url), bytes, "keymap-catalog-cache").await
    }

    async fn write_source_config(
        &self,
        catalog_url: &reqwest::Url,
    ) -> Result<(), KeyMappingCatalogError> {
        let bytes = serde_json::to_vec_pretty(&CatalogSourceConfig {
            url: catalog_url.to_string(),
        })
        .map_err(|_| KeyMappingCatalogError::Unavailable)?;
        write_local_file(&self.source_config_path(), &bytes, "keymap-catalog-source").await
    }

    async fn refresh_catalog(&self) -> Result<KeyMappingCatalog, KeyMappingCatalogError> {
        let catalog_url = self.source_url().await;
        let bytes = self
            .fetch_bytes(catalog_url.clone(), MAX_KEY_MAPPING_CATALOG_BYTES)
            .await?;
        let catalog = parse_catalog(&bytes)?;
        self.write_cached_catalog(&catalog_url, &bytes).await?;
        Ok(catalog)
    }

    async fn install_entry(
        &self,
        entry_id: String,
        name: String,
    ) -> Result<KeyMappingCatalogInstall, KeyMappingCatalogError> {
        let catalog_url = self.source_url().await;
        let catalog = self.read_cached_catalog(&catalog_url).await?;
        let entry = catalog
            .entries
            .into_iter()
            .find(|entry| entry.id == entry_id)
            .ok_or(KeyMappingCatalogError::NotFound)?;
        if self
            .profiles
            .exists(name.clone())
            .await
            .map_err(profile_error)?
        {
            return Err(KeyMappingCatalogError::Conflict);
        }
        let profile_url = catalog_url
            .join(&entry.profile.url)
            .map_err(|_| KeyMappingCatalogError::Invalid)?;
        let bytes = self.fetch_bytes(profile_url, entry.profile.bytes).await?;
        if bytes.len() != entry.profile.bytes
            || bytes.len() > MAX_REMOTE_KEY_MAPPING_PROFILE_BYTES
            || !sha256_matches(&bytes, &entry.profile.sha256)
        {
            return Err(KeyMappingCatalogError::Invalid);
        }
        let mut profile: KeyMappingProfile =
            serde_json::from_slice(&bytes).map_err(|_| KeyMappingCatalogError::Invalid)?;
        profile.name = name.clone();
        validate_key_mapping_profile(&profile).map_err(|_| KeyMappingCatalogError::Invalid)?;
        let mappings = profile.mappings.len();
        let persisted =
            serde_json::to_vec_pretty(&profile).map_err(|_| KeyMappingCatalogError::Invalid)?;
        self.profiles
            .write(name.clone(), persisted)
            .await
            .map_err(profile_error)?;
        Ok(KeyMappingCatalogInstall {
            entry_id,
            name,
            mappings,
        })
    }
}

async fn read_cached_catalog_file(
    path: &Path,
) -> Result<KeyMappingCatalog, KeyMappingCatalogError> {
    let metadata = tokio::fs::metadata(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            KeyMappingCatalogError::NotFound
        } else {
            KeyMappingCatalogError::Unavailable
        }
    })?;
    if !metadata.is_file() || metadata.len() > MAX_KEY_MAPPING_CATALOG_BYTES as u64 {
        return Err(KeyMappingCatalogError::Invalid);
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| KeyMappingCatalogError::Unavailable)?;
    parse_catalog(&bytes)
}

impl KeyMappingCatalogRepository for TokioKeyMappingCatalogRepository {
    fn source(&self) -> KeyMappingCatalogFuture<KeyMappingCatalogSource> {
        let repository = self.clone();
        Box::pin(async move { Ok(repository.source_status().await) })
    }

    fn set_source(&self, url: Option<String>) -> KeyMappingCatalogFuture<KeyMappingCatalogSource> {
        let repository = self.clone();
        Box::pin(async move { repository.update_source(url).await })
    }

    fn catalog(&self) -> KeyMappingCatalogFuture<KeyMappingCatalog> {
        let repository = self.clone();
        Box::pin(async move {
            let catalog_url = repository.source_url().await;
            repository.read_cached_catalog(&catalog_url).await
        })
    }

    fn refresh(&self) -> KeyMappingCatalogFuture<KeyMappingCatalog> {
        let repository = self.clone();
        Box::pin(async move { repository.refresh_catalog().await })
    }

    fn install(
        &self,
        entry_id: String,
        name: String,
    ) -> KeyMappingCatalogFuture<KeyMappingCatalogInstall> {
        let repository = self.clone();
        Box::pin(async move { repository.install_entry(entry_id, name).await })
    }
}

fn official_catalog_url() -> reqwest::Url {
    reqwest::Url::parse(OFFICIAL_KEY_MAPPING_CATALOG_URL)
        .expect("official key-mapping catalog URL must be valid")
}

fn load_catalog_source(cache_dir: &Path) -> reqwest::Url {
    let source_path = cache_dir.join(SOURCE_CONFIG_FILE);
    let Ok(bytes) = std::fs::read(source_path) else {
        return official_catalog_url();
    };
    let Ok(config) = serde_json::from_slice::<CatalogSourceConfig>(&bytes) else {
        return official_catalog_url();
    };
    parse_catalog_source(&config.url).unwrap_or_else(|_| official_catalog_url())
}

fn parse_catalog_source(value: &str) -> Result<reqwest::Url, KeyMappingCatalogError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_CATALOG_SOURCE_URL_BYTES {
        return Err(KeyMappingCatalogError::InvalidSource);
    }
    let url = reqwest::Url::parse(value).map_err(|_| KeyMappingCatalogError::InvalidSource)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().is_empty()
        || url.path() == "/"
    {
        return Err(KeyMappingCatalogError::InvalidSource);
    }
    Ok(url)
}

async fn write_local_file(
    path: &Path,
    bytes: &[u8],
    temporary_label: &str,
) -> Result<(), KeyMappingCatalogError> {
    let Some(parent) = path.parent() else {
        return Err(KeyMappingCatalogError::Unavailable);
    };
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|_| KeyMappingCatalogError::Unavailable)?;
    let temporary = crate::host_files::temporary_sibling(path, temporary_label)
        .map_err(|_| KeyMappingCatalogError::Unavailable)?;
    if let Err(error) = tokio::fs::write(&temporary, bytes).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        tracing::warn!(%error, "cannot write key-mapping catalog file");
        return Err(KeyMappingCatalogError::Unavailable);
    }
    if let Err(error) = crate::host_files::replace_local_file(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        tracing::warn!(%error, "cannot replace key-mapping catalog file");
        return Err(KeyMappingCatalogError::Unavailable);
    }
    Ok(())
}

fn parse_catalog(bytes: &[u8]) -> Result<KeyMappingCatalog, KeyMappingCatalogError> {
    let catalog = serde_json::from_slice(bytes).map_err(|error| {
        tracing::warn!(%error, "key-mapping catalog JSON is invalid");
        KeyMappingCatalogError::Invalid
    })?;
    validate_key_mapping_catalog(&catalog).map_err(|_| KeyMappingCatalogError::Invalid)?;
    Ok(catalog)
}

fn sha256_matches(bytes: &[u8], expected: &str) -> bool {
    let actual = format!("{:x}", Sha256::digest(bytes));
    actual.eq_ignore_ascii_case(expected)
}

fn profile_error(error: ProfileRepositoryError) -> KeyMappingCatalogError {
    match error {
        ProfileRepositoryError::NotFound => KeyMappingCatalogError::NotFound,
        ProfileRepositoryError::Unavailable => KeyMappingCatalogError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_sha256_digests_without_a_hex_dependency() {
        assert!(sha256_matches(
            b"DeviceHub Mask",
            "94074407a9154dffd386a6431c1c47cf3331775f70a6d2c0aafc4fb096d811e3"
        ));
        assert!(!sha256_matches(b"DeviceHub Mask", "a".repeat(64).as_str()));
    }

    #[test]
    fn catalog_source_requires_a_public_https_document_url() {
        assert_eq!(
            parse_catalog_source("https://example.invalid/catalog-v1.json")
                .unwrap()
                .as_str(),
            "https://example.invalid/catalog-v1.json"
        );
        for value in [
            "http://example.invalid/catalog-v1.json",
            "https://example.invalid/",
            "https://user@example.invalid/catalog-v1.json",
            "https://example.invalid/catalog-v1.json?token=secret",
            "https://example.invalid/catalog-v1.json#section",
        ] {
            assert_eq!(
                parse_catalog_source(value),
                Err(KeyMappingCatalogError::InvalidSource)
            );
        }
    }

    #[tokio::test]
    async fn custom_catalog_source_persists_across_repository_construction() {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-keymap-catalog-source-test-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = TokioKeyMappingCatalogRepository::official(
            directory.clone(),
            TokioProfileRepository::new(directory.join("profiles")),
        );
        let source = repository
            .update_source(Some("https://example.invalid/team/catalog.json".into()))
            .await
            .unwrap();
        assert_eq!(source.url, "https://example.invalid/team/catalog.json");
        assert!(!source.is_default);

        let reloaded = TokioKeyMappingCatalogRepository::official(
            directory.clone(),
            TokioProfileRepository::new(directory.join("profiles")),
        );
        assert_eq!(
            reloaded.source_status().await.url,
            "https://example.invalid/team/catalog.json"
        );
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
