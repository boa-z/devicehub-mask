use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use async_zip::tokio::read::fs::ZipFileReader;
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};

const MAX_ARCHIVE_ENTRIES: usize = 65_536;
const MAX_INFO_PLIST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 256;
const MAX_CAPABILITIES: usize = 64;
const MAX_CAPABILITY_CHARS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IpaOperation {
    Install,
    Upgrade,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IpaPreflightIssue {
    AlreadyInstalled,
    NotInstalled,
    MinimumOsUnsupported,
    DeviceFamilyUnsupported,
    RequiredCapabilitiesUnsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledAppMatch {
    pub name: String,
    pub version: Option<String>,
    pub bundle_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IpaCompatibility {
    pub minimum_os_supported: Option<bool>,
    pub device_family_supported: Option<bool>,
    pub capabilities_supported: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IpaPreflight {
    pub operation: IpaOperation,
    pub file_name: String,
    pub file_size_bytes: u64,
    pub bundle_id: String,
    pub name: String,
    pub version: Option<String>,
    pub bundle_version: Option<String>,
    pub minimum_os_version: Option<String>,
    pub device_families: Vec<u64>,
    pub required_capabilities: Vec<String>,
    pub prohibited_capabilities: Vec<String>,
    pub installed_app: Option<InstalledAppMatch>,
    pub compatibility: IpaCompatibility,
    pub blocking_issues: Vec<IpaPreflightIssue>,
    pub operation_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaArchiveMetadata {
    pub path: PathBuf,
    pub file_name: String,
    pub file_size_bytes: u64,
    pub bundle_id: String,
    pub name: String,
    pub version: Option<String>,
    pub bundle_version: Option<String>,
    pub minimum_os_version: Option<String>,
    pub device_families: Vec<u64>,
    pub required_capabilities: Vec<String>,
    pub prohibited_capabilities: Vec<String>,
}

pub async fn inspect(path: &Path) -> Result<IpaArchiveMetadata, String> {
    let (path, file_name, file_size_bytes) = validate_path(path).await?;
    let archive = ZipFileReader::new(&path)
        .await
        .map_err(|error| format!("unable to read IPA archive: {error}"))?;
    let entries = archive.file().entries();
    if entries.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "IPA archive contains too many entries (maximum {MAX_ARCHIVE_ENTRIES})"
        ));
    }

    let mut app_roots = BTreeSet::new();
    let mut info_entries = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Ok(name) = entry.filename().as_str() else {
            continue;
        };
        let path = name.trim_end_matches('/');
        let segments = path.split('/').collect::<Vec<_>>();
        if segments.len() < 2
            || segments[0] != "Payload"
            || segments[1].is_empty()
            || !segments[1].ends_with(".app")
        {
            continue;
        }
        app_roots.insert(segments[1].to_string());
        if segments.len() == 3 && segments[2] == "Info.plist" {
            info_entries.push((segments[1].to_string(), index));
        }
    }
    if app_roots.len() != 1 {
        return Err("IPA must contain exactly one top-level application bundle".into());
    }
    let app_root = app_roots.into_iter().next().expect("one app root");
    let matching_info = info_entries
        .into_iter()
        .filter(|(root, _)| root == &app_root)
        .map(|(_, index)| index)
        .collect::<Vec<_>>();
    if matching_info.len() != 1 {
        return Err("IPA must contain exactly one top-level application Info.plist".into());
    }
    let info_index = matching_info[0];
    if entries[info_index].uncompressed_size() > MAX_INFO_PLIST_BYTES {
        return Err("IPA application Info.plist is too large".into());
    }
    let mut reader = archive
        .reader_with_entry(info_index)
        .await
        .map_err(|error| format!("unable to open IPA application Info.plist: {error}"))?;
    let mut bytes = Vec::with_capacity(entries[info_index].uncompressed_size() as usize);
    reader
        .read_to_end_checked(&mut bytes)
        .await
        .map_err(|error| format!("unable to read IPA application Info.plist: {error}"))?;
    if bytes.len() as u64 > MAX_INFO_PLIST_BYTES {
        return Err("IPA application Info.plist is too large".into());
    }
    let value = Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|error| format!("unable to parse IPA application Info.plist: {error}"))?;
    let fields = value
        .as_dictionary()
        .ok_or_else(|| "IPA application Info.plist is not a dictionary".to_string())?;
    let bundle_id = required_text(fields, "CFBundleIdentifier", "bundle identifier")?;
    if bundle_id.len() > 255
        || bundle_id.starts_with('.')
        || bundle_id.ends_with('.')
        || bundle_id
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
    {
        return Err("IPA application has an invalid bundle identifier".into());
    }
    let name = optional_text(fields, "CFBundleDisplayName")
        .or_else(|| optional_text(fields, "CFBundleName"))
        .unwrap_or_else(|| app_root.trim_end_matches(".app").to_string());
    let (required_capabilities, prohibited_capabilities) = capabilities(fields)?;

    Ok(IpaArchiveMetadata {
        path,
        file_name,
        file_size_bytes,
        bundle_id,
        name: bounded_text(&name, MAX_TEXT_CHARS)
            .ok_or_else(|| "IPA application has no valid display name".to_string())?,
        version: optional_text(fields, "CFBundleShortVersionString"),
        bundle_version: optional_text(fields, "CFBundleVersion"),
        minimum_os_version: optional_text(fields, "MinimumOSVersion"),
        device_families: device_families(fields)?,
        required_capabilities,
        prohibited_capabilities,
    })
}

async fn validate_path(path: &Path) -> Result<(PathBuf, String, u64), String> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ipa"))
    {
        return Err("selected file must have an .ipa extension".into());
    }
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|error| format!("unable to access IPA: {error}"))?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|error| format!("unable to inspect IPA: {error}"))?;
    if !metadata.is_file() {
        return Err("selected IPA path is not a regular file".into());
    }
    let file_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| bounded_text(name, MAX_TEXT_CHARS))
        .ok_or_else(|| "selected IPA has no valid file name".to_string())?;
    Ok((canonical, file_name, metadata.len()))
}

fn required_text(fields: &Dictionary, key: &str, label: &str) -> Result<String, String> {
    optional_text(fields, key).ok_or_else(|| format!("IPA application has no valid {label}"))
}

fn optional_text(fields: &Dictionary, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(Value::as_string)
        .and_then(|value| bounded_text(value, MAX_TEXT_CHARS))
}

fn bounded_text(value: &str, max_chars: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max_chars || value.chars().any(char::is_control)
    {
        None
    } else {
        Some(value.to_string())
    }
}

fn device_families(fields: &Dictionary) -> Result<Vec<u64>, String> {
    let Some(value) = fields.get("UIDeviceFamily") else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| "IPA application UIDeviceFamily is not an array".to_string())?;
    if values.len() > 16 {
        return Err("IPA application declares too many device families".into());
    }
    let mut families = values
        .iter()
        .map(|value| {
            value
                .as_unsigned_integer()
                .filter(|value| *value <= 255)
                .ok_or_else(|| "IPA application has an invalid device family".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    families.sort_unstable();
    families.dedup();
    Ok(families)
}

fn capabilities(fields: &Dictionary) -> Result<(Vec<String>, Vec<String>), String> {
    let Some(value) = fields.get("UIRequiredDeviceCapabilities") else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut required = Vec::new();
    let mut prohibited = Vec::new();
    match value {
        Value::Array(values) => {
            if values.len() > MAX_CAPABILITIES {
                return Err("IPA application declares too many required capabilities".into());
            }
            for value in values {
                required.push(capability_text(value)?);
            }
        }
        Value::Dictionary(values) => {
            if values.len() > MAX_CAPABILITIES {
                return Err("IPA application declares too many required capabilities".into());
            }
            for (name, value) in values {
                let name = bounded_text(name, MAX_CAPABILITY_CHARS).ok_or_else(|| {
                    "IPA application has an invalid required capability".to_string()
                })?;
                match value.as_boolean() {
                    Some(true) => required.push(name),
                    Some(false) => prohibited.push(name),
                    None => {
                        return Err(
                            "IPA application capability dictionary contains a non-boolean value"
                                .into(),
                        );
                    }
                }
            }
        }
        _ => return Err("IPA application required capabilities have an invalid format".into()),
    }
    required.sort();
    required.dedup();
    prohibited.sort();
    prohibited.dedup();
    Ok((required, prohibited))
}

fn capability_text(value: &Value) -> Result<String, String> {
    value
        .as_string()
        .and_then(|value| bounded_text(value, MAX_CAPABILITY_CHARS))
        .ok_or_else(|| "IPA application has an invalid required capability".to_string())
}

pub fn version_at_least(current: &str, minimum: &str) -> Option<bool> {
    fn parse(value: &str) -> Option<Vec<u64>> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        value
            .split('.')
            .map(|part| {
                (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| part.parse::<u64>().ok())
                    .flatten()
            })
            .collect()
    }
    let current = parse(current)?;
    let minimum = parse(minimum)?;
    let length = current.len().max(minimum.len());
    for index in 0..length {
        match current
            .get(index)
            .copied()
            .unwrap_or(0)
            .cmp(&minimum.get(index).copied().unwrap_or(0))
        {
            std::cmp::Ordering::Less => return Some(false),
            std::cmp::Ordering::Greater => return Some(true),
            std::cmp::Ordering::Equal => {}
        }
    }
    Some(true)
}

pub fn device_family_supported(product_type: &str, families: &[u64]) -> Option<bool> {
    if families.is_empty() {
        return None;
    }
    let family = if product_type.starts_with("iPhone") || product_type.starts_with("iPod") {
        1
    } else if product_type.starts_with("iPad") {
        2
    } else {
        return None;
    };
    Some(families.contains(&family))
}

pub fn preflight_issues(
    operation: IpaOperation,
    installed: bool,
    compatibility: &IpaCompatibility,
) -> Vec<IpaPreflightIssue> {
    let mut issues = Vec::new();
    match (operation, installed) {
        (IpaOperation::Install, true) => issues.push(IpaPreflightIssue::AlreadyInstalled),
        (IpaOperation::Upgrade, false) => issues.push(IpaPreflightIssue::NotInstalled),
        _ => {}
    }
    if compatibility.minimum_os_supported == Some(false) {
        issues.push(IpaPreflightIssue::MinimumOsUnsupported);
    }
    if compatibility.device_family_supported == Some(false) {
        issues.push(IpaPreflightIssue::DeviceFamilyUnsupported);
    }
    if compatibility.capabilities_supported == Some(false) {
        issues.push(IpaPreflightIssue::RequiredCapabilitiesUnsupported);
    }
    issues
}

#[cfg(test)]
mod tests {
    use async_zip::{Compression, ZipEntryBuilder};

    use super::*;

    async fn write_ipa(entries: &[(&str, &[u8])]) -> (PathBuf, PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("devicehub-mask-ipa-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let path = directory.join("Example.ipa");
        let mut bytes = Vec::new();
        {
            let mut writer = async_zip::base::write::ZipFileWriter::new(&mut bytes);
            for (name, contents) in entries {
                writer
                    .write_entry_whole(
                        ZipEntryBuilder::new((*name).into(), Compression::Stored),
                        contents,
                    )
                    .await
                    .unwrap();
            }
            writer.close().await.unwrap();
        }
        tokio::fs::write(&path, bytes).await.unwrap();
        (directory, path)
    }

    fn info_plist() -> Vec<u8> {
        let mut capabilities = Dictionary::new();
        capabilities.insert("gps".into(), Value::Boolean(true));
        capabilities.insert("telephony".into(), Value::Boolean(false));
        let mut fields = Dictionary::new();
        fields.insert(
            "CFBundleIdentifier".into(),
            Value::String("com.example.game".into()),
        );
        fields.insert(
            "CFBundleDisplayName".into(),
            Value::String("Example Game".into()),
        );
        fields.insert(
            "CFBundleShortVersionString".into(),
            Value::String("2.1".into()),
        );
        fields.insert("CFBundleVersion".into(), Value::String("42".into()));
        fields.insert("MinimumOSVersion".into(), Value::String("17.0".into()));
        fields.insert(
            "UIDeviceFamily".into(),
            Value::Array(vec![
                Value::Integer(1_u64.into()),
                Value::Integer(2_u64.into()),
            ]),
        );
        fields.insert(
            "UIRequiredDeviceCapabilities".into(),
            Value::Dictionary(capabilities),
        );
        let value = Value::Dictionary(fields);
        let mut bytes = Vec::new();
        value.to_writer_xml(&mut bytes).unwrap();
        bytes
    }

    #[tokio::test]
    async fn reads_only_bounded_top_level_application_metadata() {
        let info = info_plist();
        let (directory, path) = write_ipa(&[
            ("Payload/Example.app/Info.plist", &info),
            (
                "Payload/Example.app/PlugIns/Widget.appex/Info.plist",
                b"ignored",
            ),
        ])
        .await;
        let metadata = inspect(&path).await.unwrap();
        assert_eq!(metadata.bundle_id, "com.example.game");
        assert_eq!(metadata.name, "Example Game");
        assert_eq!(metadata.device_families, vec![1, 2]);
        assert_eq!(metadata.required_capabilities, vec!["gps"]);
        assert_eq!(metadata.prohibited_capabilities, vec!["telephony"]);
        let _ = tokio::fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn rejects_ambiguous_top_level_applications() {
        let info = info_plist();
        let (directory, path) = write_ipa(&[
            ("Payload/First.app/Info.plist", &info),
            ("Payload/Second.app/Info.plist", &info),
        ])
        .await;
        assert!(inspect(&path).await.unwrap_err().contains("exactly one"));
        let _ = tokio::fs::remove_dir_all(directory).await;
    }

    #[test]
    fn compares_versions_and_device_families_structurally() {
        assert_eq!(version_at_least("17.0.1", "17.0"), Some(true));
        assert_eq!(version_at_least("16.7", "17.0"), Some(false));
        assert_eq!(version_at_least("17 beta", "17.0"), None);
        assert_eq!(device_family_supported("iPhone14,3", &[1]), Some(true));
        assert_eq!(device_family_supported("iPad12,1", &[1]), Some(false));
        assert_eq!(device_family_supported("Unknown", &[1]), None);
    }

    #[test]
    fn blocks_only_explicit_operation_and_known_compatibility_failures() {
        let unknown = IpaCompatibility {
            minimum_os_supported: None,
            device_family_supported: None,
            capabilities_supported: None,
        };
        assert_eq!(
            preflight_issues(IpaOperation::Install, true, &unknown),
            vec![IpaPreflightIssue::AlreadyInstalled]
        );
        assert_eq!(
            preflight_issues(IpaOperation::Upgrade, false, &unknown),
            vec![IpaPreflightIssue::NotInstalled]
        );
        let incompatible = IpaCompatibility {
            minimum_os_supported: Some(false),
            device_family_supported: Some(false),
            capabilities_supported: Some(false),
        };
        assert_eq!(
            preflight_issues(IpaOperation::Install, false, &incompatible),
            vec![
                IpaPreflightIssue::MinimumOsUnsupported,
                IpaPreflightIssue::DeviceFamilyUnsupported,
                IpaPreflightIssue::RequiredCapabilitiesUnsupported,
            ]
        );
    }
}
