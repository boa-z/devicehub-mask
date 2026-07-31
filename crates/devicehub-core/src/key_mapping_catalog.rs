//! Shared model and validation for published key-mapping catalogs.
//!
//! A catalog is discovery metadata for immutable profile downloads. It is not
//! the user's editable profile store; installed entries always become local
//! key-mapping profiles.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{KeyMappingResolution, validate_app_bundle_id, validate_key_mapping_profile_name};

pub const KEY_MAPPING_CATALOG_SCHEMA_VERSION: u8 = 1;
pub const MAX_KEY_MAPPING_CATALOG_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_KEY_MAPPING_CATALOG_ENTRIES: usize = 2_048;
pub const MAX_REMOTE_KEY_MAPPING_PROFILE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyMappingCatalog {
    pub schema_version: u8,
    pub repository: KeyMappingCatalogRepository,
    #[serde(default)]
    pub entries: Vec<KeyMappingCatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyMappingCatalogRepository {
    pub id: String,
    pub name: String,
    pub generated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyMappingCatalogEntry {
    pub id: String,
    /// ASCII local-profile-safe suggestion. The user may choose another name.
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub updated_at: String,
    pub profile: KeyMappingCatalogProfile,
    #[serde(rename = "match")]
    pub compatibility: KeyMappingCatalogMatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyMappingCatalogProfile {
    pub format: String,
    pub format_version: u8,
    /// Relative to the catalog document. Absolute URLs are intentionally rejected.
    pub url: String,
    pub sha256: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyMappingCatalogMatch {
    pub bundle_ids: Vec<String>,
    pub stream_resolution: KeyMappingResolution,
    pub orientation: KeyMappingCatalogOrientation,
    #[serde(default)]
    pub product_types: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyMappingCatalogOrientation {
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidKeyMappingCatalog;

impl fmt::Display for InvalidKeyMappingCatalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid key-mapping catalog")
    }
}

impl std::error::Error for InvalidKeyMappingCatalog {}

pub fn validate_key_mapping_catalog(
    catalog: &KeyMappingCatalog,
) -> Result<(), InvalidKeyMappingCatalog> {
    if catalog.schema_version != KEY_MAPPING_CATALOG_SCHEMA_VERSION
        || !valid_repository_id(&catalog.repository.id)
        || !valid_text(&catalog.repository.name, 160, false)
        || !valid_text(&catalog.repository.generated_at, 64, false)
        || catalog.entries.len() > MAX_KEY_MAPPING_CATALOG_ENTRIES
    {
        return Err(InvalidKeyMappingCatalog);
    }

    let mut entry_ids = HashSet::new();
    let mut slugs = HashSet::new();
    for entry in &catalog.entries {
        if !valid_entry_id(&entry.id)
            || validate_key_mapping_profile_name(&entry.slug).is_err()
            || !entry_ids.insert(entry.id.as_str())
            || !slugs.insert(entry.slug.as_str())
            || !valid_text(&entry.title, 160, false)
            || !valid_text(&entry.description, 2_048, true)
            || !valid_text(&entry.author, 160, true)
            || !valid_text(&entry.updated_at, 64, true)
            || !valid_profile_descriptor(&entry.profile)
            || !valid_match(&entry.compatibility)
        {
            return Err(InvalidKeyMappingCatalog);
        }
    }
    Ok(())
}

fn valid_repository_id(value: &str) -> bool {
    (3..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn valid_entry_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_text(value: &str, max_characters: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.trim().is_empty())
        && value.chars().count() <= max_characters
        && !value.chars().any(char::is_control)
}

fn valid_profile_descriptor(profile: &KeyMappingCatalogProfile) -> bool {
    profile.format == "devicehub-mask"
        && profile.format_version == 2
        && (1..=MAX_REMOTE_KEY_MAPPING_PROFILE_BYTES).contains(&profile.bytes)
        && profile.sha256.len() == 64
        && profile.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        && valid_relative_catalog_path(&profile.url)
}

fn valid_relative_catalog_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && !path.starts_with('/')
        && !path.contains(['\\', '?', '#'])
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && !matches!(segment, "." | "..")
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        })
}

fn valid_match(compatibility: &KeyMappingCatalogMatch) -> bool {
    let resolution = compatibility.stream_resolution;
    if compatibility.bundle_ids.is_empty()
        || compatibility.bundle_ids.len() > 32
        || resolution.width == 0
        || resolution.height == 0
        || resolution.width > 16_384
        || resolution.height > 16_384
        || compatibility.product_types.len() > 64
    {
        return false;
    }
    let mut bundle_ids = HashSet::new();
    if compatibility.bundle_ids.iter().any(|bundle_id| {
        validate_app_bundle_id(bundle_id).is_err() || !bundle_ids.insert(bundle_id.as_str())
    }) {
        return false;
    }
    let mut product_types = HashSet::new();
    !compatibility.product_types.iter().any(|product_type| {
        product_type.is_empty()
            || product_type.len() > 128
            || !product_type
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b',' | b'-' | b'_'))
            || !product_types.insert(product_type.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> KeyMappingCatalog {
        KeyMappingCatalog {
            schema_version: KEY_MAPPING_CATALOG_SCHEMA_VERSION,
            repository: KeyMappingCatalogRepository {
                id: "com.devicehub.mask.keymaps".into(),
                name: "DeviceHub Mask Keymaps".into(),
                generated_at: "2026-08-01T00:00:00Z".into(),
            },
            entries: vec![KeyMappingCatalogEntry {
                id: "9c5d48c1-8d31-4a55-89f3-6d7f8fd77777".into(),
                slug: "example-game-landscape".into(),
                title: "Example Game - Landscape".into(),
                description: String::new(),
                author: String::new(),
                updated_at: String::new(),
                profile: KeyMappingCatalogProfile {
                    format: "devicehub-mask".into(),
                    format_version: 2,
                    url: "profiles/9c5d48c1-8d31-4a55-89f3-6d7f8fd77777/profile.json".into(),
                    sha256: "a".repeat(64),
                    bytes: 42,
                },
                compatibility: KeyMappingCatalogMatch {
                    bundle_ids: vec!["com.example.game".into()],
                    stream_resolution: KeyMappingResolution {
                        width: 2796,
                        height: 1290,
                    },
                    orientation: KeyMappingCatalogOrientation::LandscapeLeft,
                    product_types: vec!["iPhone16,1".into()],
                },
            }],
        }
    }

    #[test]
    fn accepts_a_bounded_catalog_entry() {
        assert!(validate_key_mapping_catalog(&catalog()).is_ok());
    }

    #[test]
    fn rejects_absolute_and_traversal_profile_urls() {
        for url in [
            "https://example.invalid/profile.json",
            "../profile.json",
            "/profile.json",
        ] {
            let mut value = catalog();
            value.entries[0].profile.url = url.into();
            assert!(validate_key_mapping_catalog(&value).is_err());
        }
    }

    #[test]
    fn rejects_duplicate_entry_ids_and_invalid_matching_data() {
        let mut value = catalog();
        value.entries.push(value.entries[0].clone());
        assert!(validate_key_mapping_catalog(&value).is_err());

        let mut value = catalog();
        value.entries[0].compatibility.bundle_ids.clear();
        assert!(validate_key_mapping_catalog(&value).is_err());
    }
}
