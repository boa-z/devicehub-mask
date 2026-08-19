//! Native-host asset binding for the runtime-owned Developer Disk Image service.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use devicehub_core::{DeveloperImageKind, DeveloperImageSetDescriptor, DeveloperImageSourceKind};
use devicehub_runtime::DeveloperImageMountRequest;
use devicehub_runtime::DeveloperImageVariant;
use devicehub_server::http::{
    DeveloperImageCatalog, DeveloperImageCatalogFuture, DeveloperImageImportFile,
};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

const MAX_PATH_BYTES: usize = 4_096;
const MAX_CUSTOM_ROOTS: usize = 16;
const MAX_CUSTOM_ROOT_CHILDREN: usize = 256;

#[derive(Clone)]
pub struct TokioDeveloperImageCatalog {
    inner: Arc<DeveloperImageCatalogInner>,
    preferences: devicehub_runtime::RuntimePreferences,
}

struct DeveloperImageCatalogInner {
    managed_root: PathBuf,
    custom_roots: RwLock<Vec<PathBuf>>,
    sets: RwLock<HashMap<String, CatalogEntry>>,
    #[cfg(target_os = "macos")]
    discover_xcode: bool,
}

#[derive(Clone)]
struct CatalogEntry {
    descriptor: DeveloperImageSetDescriptor,
    request: DeveloperImageMountRequest<PathBuf>,
    managed_directory: Option<PathBuf>,
}

impl TokioDeveloperImageCatalog {
    pub fn new(
        managed_root: PathBuf,
        custom_roots: Vec<PathBuf>,
        preferences: devicehub_runtime::RuntimePreferences,
    ) -> Result<Self, String> {
        Self::new_with_discovery(managed_root, custom_roots, preferences, true)
    }

    fn new_with_discovery(
        managed_root: PathBuf,
        custom_roots: Vec<PathBuf>,
        preferences: devicehub_runtime::RuntimePreferences,
        discover_xcode: bool,
    ) -> Result<Self, String> {
        let custom_roots = validate_custom_roots(custom_roots)?;
        #[cfg(not(target_os = "macos"))]
        let _ = discover_xcode;
        std::fs::create_dir_all(&managed_root).map_err(|error| {
            format!(
                "cannot create developer image catalog {}: {error}",
                managed_root.display()
            )
        })?;
        let catalog = Self {
            inner: Arc::new(DeveloperImageCatalogInner {
                managed_root,
                custom_roots: RwLock::new(custom_roots),
                sets: RwLock::new(HashMap::new()),
                #[cfg(target_os = "macos")]
                discover_xcode,
            }),
            preferences,
        };
        catalog.refresh_sync()?;
        Ok(catalog)
    }

    fn refresh_sync(&self) -> Result<Vec<DeveloperImageSetDescriptor>, String> {
        let mut entries = HashMap::new();
        for root in managed_roots(&self.inner.managed_root)? {
            match scan_managed_set(&root) {
                Ok(entry) => {
                    entries.insert(entry.descriptor.id.clone(), entry);
                }
                Err(error) => {
                    tracing::warn!(path = %root.display(), %error, "ignoring invalid managed developer image set");
                }
            }
        }
        let custom_roots = self
            .inner
            .custom_roots
            .read()
            .map_err(|_| "developer image custom root lock poisoned".to_owned())?
            .clone();
        for entry in scan_custom_sets(&custom_roots) {
            entries.entry(entry.descriptor.id.clone()).or_insert(entry);
        }
        #[cfg(target_os = "macos")]
        if self.inner.discover_xcode {
            for entry in scan_xcode_sets() {
                entries.entry(entry.descriptor.id.clone()).or_insert(entry);
            }
        }
        *self
            .inner
            .sets
            .write()
            .map_err(|_| "developer image catalog lock poisoned".to_owned())? = entries;
        self.snapshot()
    }

    pub fn custom_roots(&self) -> Result<Vec<PathBuf>, String> {
        self.inner
            .custom_roots
            .read()
            .map_err(|_| "developer image custom root lock poisoned".to_owned())
            .map(|roots| roots.clone())
    }

    pub fn set_custom_roots(
        &self,
        roots: Vec<PathBuf>,
    ) -> Result<Vec<DeveloperImageSetDescriptor>, String> {
        let roots = validate_custom_roots(roots)?;
        *self
            .inner
            .custom_roots
            .write()
            .map_err(|_| "developer image custom root lock poisoned".to_owned())? = roots;
        self.refresh_sync()
    }
}

impl DeveloperImageCatalog for TokioDeveloperImageCatalog {
    fn snapshot(&self) -> Result<Vec<DeveloperImageSetDescriptor>, String> {
        let mut values = self
            .inner
            .sets
            .read()
            .map_err(|_| "developer image catalog lock poisoned".to_owned())?
            .values()
            .map(|entry| entry.descriptor.clone())
            .collect::<Vec<_>>();
        values.sort_by(|left, right| {
            source_rank(left.source)
                .cmp(&source_rank(right.source))
                .then_with(|| right.product_build_version.cmp(&left.product_build_version))
                .then_with(|| left.display_name.cmp(&right.display_name))
        });
        Ok(values)
    }

    fn refresh(&self) -> DeveloperImageCatalogFuture<Vec<DeveloperImageSetDescriptor>> {
        let catalog = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || catalog.refresh_sync())
                .await
                .map_err(|error| format!("developer image catalog refresh failed: {error}"))?
        })
    }

    fn import(
        &self,
        files: Vec<DeveloperImageImportFile>,
    ) -> DeveloperImageCatalogFuture<DeveloperImageSetDescriptor> {
        let catalog = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || catalog.import_sync(files))
                .await
                .map_err(|error| format!("developer image import failed: {error}"))?
        })
    }

    fn resolve(
        &self,
        id: String,
    ) -> DeveloperImageCatalogFuture<DeveloperImageMountRequest<PathBuf>> {
        let catalog = self.clone();
        Box::pin(async move {
            validate_set_id(&id)?;
            catalog
                .inner
                .sets
                .read()
                .map_err(|_| "developer image catalog lock poisoned".to_owned())?
                .get(&id)
                .map(|entry| entry.request.clone())
                .ok_or_else(|| "developer image set not found".into())
        })
    }

    fn remove(&self, id: String) -> DeveloperImageCatalogFuture<()> {
        let catalog = self.clone();
        Box::pin(async move {
            validate_set_id(&id)?;
            let directory = catalog
                .inner
                .sets
                .read()
                .map_err(|_| "developer image catalog lock poisoned".to_owned())?
                .get(&id)
                .ok_or_else(|| "developer image set not found".to_owned())?
                .managed_directory
                .clone()
                .ok_or_else(|| "cannot remove a developer image owned by Xcode".to_owned())?;
            tokio::fs::remove_dir_all(&directory)
                .await
                .map_err(|error| format!("cannot remove developer image set: {error}"))?;
            catalog.refresh().await?;
            Ok(())
        })
    }
}

impl TokioDeveloperImageCatalog {
    fn import_sync(
        &self,
        files: Vec<DeveloperImageImportFile>,
    ) -> Result<DeveloperImageSetDescriptor, String> {
        let imported = ImportedSet::parse(files)?;
        let id = imported.id();
        let destination = self.inner.managed_root.join(&id);
        if destination.exists() {
            return Err("developer image set already exists".into());
        }
        let temporary = self
            .inner
            .managed_root
            .join(format!(".import-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&temporary)
            .map_err(|error| format!("cannot create developer image import directory: {error}"))?;
        let result: Result<(), String> = (|| {
            for file in &imported.files {
                std::fs::write(temporary.join(&file.name), &file.bytes)
                    .map_err(|error| format!("cannot write developer image asset: {error}"))?;
            }
            let metadata = ManagedSetMetadata {
                id: id.clone(),
                kind: imported.kind,
                image_name: imported.image_name.clone(),
                auxiliary_name: imported.auxiliary_name.clone(),
                manifest_name: imported.manifest_name.clone(),
            };
            std::fs::write(
                temporary.join("devicehub-ddi.json"),
                serde_json::to_vec_pretty(&metadata).map_err(|error| {
                    format!("cannot serialize developer image metadata: {error}")
                })?,
            )
            .map_err(|error| format!("cannot write developer image metadata: {error}"))?;
            std::fs::rename(&temporary, &destination)
                .map_err(|error| format!("cannot commit developer image import: {error}"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&temporary);
        }
        result?;
        self.refresh_sync()?
            .into_iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or_else(|| "imported developer image set was not indexed".into())
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
struct ManagedSetMetadata {
    id: String,
    kind: DeveloperImageKind,
    image_name: String,
    auxiliary_name: String,
    manifest_name: Option<String>,
}

struct ImportedSet {
    kind: DeveloperImageKind,
    image_name: String,
    auxiliary_name: String,
    manifest_name: Option<String>,
    files: Vec<DeveloperImageImportFile>,
}

impl ImportedSet {
    fn parse(files: Vec<DeveloperImageImportFile>) -> Result<Self, String> {
        let mut safe = Vec::with_capacity(files.len());
        for file in files {
            let name = safe_asset_name(&file.name)?;
            if file.bytes.is_empty() {
                return Err(format!("developer image asset {name} is empty"));
            }
            safe.push(DeveloperImageImportFile {
                name,
                bytes: file.bytes,
            });
        }
        let image = safe
            .iter()
            .find(|file| file.name.to_ascii_lowercase().ends_with(".dmg"))
            .ok_or_else(|| "developer image upload has no .dmg file".to_owned())?;
        let signature = safe
            .iter()
            .find(|file| file.name.to_ascii_lowercase().ends_with(".signature"));
        let trust_cache = safe
            .iter()
            .find(|file| file.name.to_ascii_lowercase().ends_with(".trustcache"));
        let manifest = safe
            .iter()
            .find(|file| file.name.eq_ignore_ascii_case("BuildManifest.plist"));
        let (kind, auxiliary_name, manifest_name) = match (signature, trust_cache, manifest) {
            (Some(signature), None, None) if safe.len() == 2 => {
                (DeveloperImageKind::Legacy, signature.name.clone(), None)
            }
            (None, Some(trust_cache), Some(manifest)) if safe.len() == 3 => {
                validate_personalized_manifest(&manifest.bytes, &image.name, &trust_cache.name)?;
                (
                    DeveloperImageKind::Personalized,
                    trust_cache.name.clone(),
                    Some(manifest.name.clone()),
                )
            }
            _ => {
                return Err(
                    "upload exactly image+signature for legacy DDI or image+trustcache+BuildManifest.plist for personalized DDI".into(),
                );
            }
        };
        Ok(Self {
            kind,
            image_name: image.name.clone(),
            auxiliary_name,
            manifest_name,
            files: safe,
        })
    }

    fn id(&self) -> String {
        let mut files = self.files.iter().collect::<Vec<_>>();
        files.sort_by(|left, right| left.name.cmp(&right.name));
        let mut digest = Sha256::new();
        for file in files {
            digest.update(file.name.as_bytes());
            digest.update([0]);
            digest.update(&file.bytes);
        }
        format!("ddi-{}", &format!("{:x}", digest.finalize())[..24])
    }
}

impl devicehub_runtime::DeveloperImageAssetLoader for TokioDeveloperImageCatalog {
    type Source = PathBuf;

    fn file_name(&self, source: &PathBuf, label: &str) -> Result<String, String> {
        source
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("{label} has an invalid file name"))
    }

    fn load<'a>(
        &'a self,
        source: &'a PathBuf,
        label: &'a str,
        max_bytes: u64,
    ) -> devicehub_runtime::DeveloperImageAssetFuture<'a> {
        Box::pin(async move {
            if !source.is_absolute() || source.as_os_str().len() > MAX_PATH_BYTES {
                return Err(format!("{label} must be an absolute local file path"));
            }
            let metadata = tokio::fs::symlink_metadata(source)
                .await
                .map_err(|error| format!("{label} is unavailable: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "{label} must be a regular file, not a symbolic link"
                ));
            }
            if metadata.len() == 0 || metadata.len() > max_bytes {
                return Err(format!("{label} size is outside the supported range"));
            }

            let file = tokio::fs::File::open(source)
                .await
                .map_err(|error| format!("cannot open {label}: {error}"))?;
            let mut bytes = Vec::with_capacity(
                usize::try_from(metadata.len().min(max_bytes)).unwrap_or(usize::MAX),
            );
            file.take(max_bytes + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| format!("cannot read {label}: {error}"))?;
            if bytes.is_empty() || bytes.len() as u64 > max_bytes {
                return Err(format!("{label} changed while it was being read"));
            }

            Ok(bytes)
        })
    }

    fn automatic_request<'a>(
        &'a self,
        product_version: &'a str,
    ) -> devicehub_runtime::DeveloperImageAutomaticRequestFuture<'a, PathBuf> {
        Box::pin(async move {
            if self.preferences.developer_image_mount_policy()
                != devicehub_core::DeveloperImageMountPolicy::Automatic
            {
                return Ok(None);
            }
            let expected = if devicehub_core::developer_image_type_for_version(product_version)?
                == "Developer"
            {
                DeveloperImageKind::Legacy
            } else {
                DeveloperImageKind::Personalized
            };
            self.inner
                .sets
                .read()
                .map_err(|_| "developer image catalog lock poisoned".to_owned())?
                .values()
                .filter(|entry| {
                    entry.descriptor.kind == expected
                        && compatibility_rank(&entry.descriptor, product_version) < 2
                })
                .min_by_key(|entry| {
                    (
                        compatibility_rank(&entry.descriptor, product_version),
                        source_rank(entry.descriptor.source),
                    )
                })
                .map(|entry| entry.request.clone())
                .map(Some)
                .ok_or_else(|| {
                    format!(
                        "automatic developer image mounting is enabled, but no compatible {expected:?} image set is available"
                    )
                })
        })
    }

    fn automatic_enabled(&self) -> bool {
        self.preferences.developer_image_mount_policy()
            == devicehub_core::DeveloperImageMountPolicy::Automatic
    }
}

fn source_rank(source: DeveloperImageSourceKind) -> u8 {
    match source {
        DeveloperImageSourceKind::Xcode => 0,
        DeveloperImageSourceKind::Custom => 1,
        DeveloperImageSourceKind::Managed => 2,
    }
}

fn compatibility_rank(descriptor: &DeveloperImageSetDescriptor, product_version: &str) -> u8 {
    match descriptor.product_version.as_deref() {
        Some(version) if version_family(version) == version_family(product_version) => 0,
        None if descriptor.kind == DeveloperImageKind::Personalized => 0,
        None => 1,
        Some(_) => 2,
    }
}

fn version_family(version: &str) -> String {
    normalized_product_version(version)
        .split('.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".")
}

fn normalized_product_version(version: &str) -> String {
    version
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect::<String>()
        .trim_end_matches('.')
        .to_owned()
}

fn validate_custom_roots(roots: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    if roots.len() > MAX_CUSTOM_ROOTS {
        return Err(format!(
            "at most {MAX_CUSTOM_ROOTS} custom developer image directories are supported"
        ));
    }
    let mut validated = Vec::with_capacity(roots.len());
    for root in roots {
        if !root.is_absolute() || root.as_os_str().len() > MAX_PATH_BYTES {
            return Err("custom developer image directories must be absolute local paths".into());
        }
        if !validated.contains(&root) {
            validated.push(root);
        }
    }
    Ok(validated)
}

fn scan_custom_sets(roots: &[PathBuf]) -> Vec<CatalogEntry> {
    let mut entries = Vec::new();
    for configured_root in roots {
        let root = match std::fs::canonicalize(configured_root) {
            Ok(root) if root.is_dir() => root,
            Ok(_) => {
                tracing::warn!(path = %configured_root.display(), "custom developer image path is not a directory");
                continue;
            }
            Err(error) => {
                tracing::warn!(path = %configured_root.display(), %error, "custom developer image directory is unavailable");
                continue;
            }
        };
        scan_custom_candidate(&root, &mut entries);
        let Ok(children) = std::fs::read_dir(&root) else {
            continue;
        };
        for child in children
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .take(MAX_CUSTOM_ROOT_CHILDREN)
        {
            scan_custom_candidate(&child.path(), &mut entries);
        }
    }
    entries
}

fn scan_custom_candidate(root: &Path, entries: &mut Vec<CatalogEntry>) {
    entries.extend(scan_personalized_bundle(
        root,
        DeveloperImageSourceKind::Custom,
    ));
    if let Ok(entry) = scan_legacy_set(root, DeveloperImageSourceKind::Custom) {
        entries.push(entry);
    }
}

fn managed_roots(root: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(root)
        .map_err(|error| format!("cannot read developer image catalog: {error}"))?;
    Ok(entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .collect())
}

fn scan_managed_set(root: &Path) -> Result<CatalogEntry, String> {
    let metadata: ManagedSetMetadata = serde_json::from_slice(
        &std::fs::read(root.join("devicehub-ddi.json"))
            .map_err(|error| format!("cannot read managed developer image metadata: {error}"))?,
    )
    .map_err(|error| format!("invalid managed developer image metadata: {error}"))?;
    validate_set_id(&metadata.id)?;
    if root.file_name().and_then(|name| name.to_str()) != Some(metadata.id.as_str()) {
        return Err("managed developer image directory does not match its ID".into());
    }
    let image = confined_asset(root, &metadata.image_name)?;
    let auxiliary = confined_asset(root, &metadata.auxiliary_name)?;
    let manifest = metadata
        .manifest_name
        .as_deref()
        .map(|name| confined_asset(root, name))
        .transpose()?;
    if metadata.kind == DeveloperImageKind::Personalized {
        let manifest_path = manifest
            .as_ref()
            .ok_or_else(|| "personalized DDI is missing BuildManifest.plist".to_owned())?;
        validate_personalized_manifest(
            &std::fs::read(manifest_path)
                .map_err(|error| format!("cannot read developer image BuildManifest: {error}"))?,
            &metadata.image_name,
            &metadata.auxiliary_name,
        )?;
    }
    let size_bytes = file_size(&image)?
        .saturating_add(file_size(&auxiliary)?)
        .saturating_add(
            manifest
                .as_ref()
                .map(|path| file_size(path))
                .transpose()?
                .unwrap_or(0),
        );
    let product_build_version = manifest
        .as_ref()
        .and_then(|path| read_product_build_version(path).ok().flatten());
    let request = match metadata.kind {
        DeveloperImageKind::Legacy => DeveloperImageMountRequest::Legacy {
            image,
            signature: auxiliary,
        },
        DeveloperImageKind::Personalized => DeveloperImageMountRequest::Personalized {
            manifest: manifest.expect("validated personalized manifest"),
            variants: vec![DeveloperImageVariant { image, auxiliary }],
        },
    };
    Ok(CatalogEntry {
        descriptor: DeveloperImageSetDescriptor {
            id: metadata.id,
            kind: metadata.kind,
            source: DeveloperImageSourceKind::Managed,
            display_name: product_build_version.as_deref().map_or_else(
                || "Imported Developer Disk Image".into(),
                |build| format!("Imported iOS DDI {build}"),
            ),
            platform: "iOS".into(),
            product_version: None,
            product_build_version,
            image_name: metadata.image_name,
            auxiliary_name: metadata.auxiliary_name,
            manifest_name: metadata.manifest_name,
            size_bytes,
            removable: true,
        },
        request,
        managed_directory: Some(root.to_path_buf()),
    })
}

fn confined_asset(root: &Path, name: &str) -> Result<PathBuf, String> {
    let name = safe_asset_name(name)?;
    let path = root.join(name);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("developer image asset is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
        return Err("developer image asset must be a non-empty regular file".into());
    }
    Ok(path)
}

fn safe_asset_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.len() > 255
        || Path::new(trimmed)
            .file_name()
            .and_then(|value| value.to_str())
            != Some(trimmed)
        || trimmed.contains(['/', '\\', '\0'])
    {
        return Err("developer image asset has an unsafe file name".into());
    }
    Ok(trimmed.to_owned())
}

fn validate_set_id(id: &str) -> Result<(), String> {
    if id.len() != 28
        || !id.starts_with("ddi-")
        || !id[4..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid developer image set ID".into());
    }
    Ok(())
}

fn file_size(path: &Path) -> Result<u64, String> {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("cannot inspect developer image asset: {error}"))
}

fn validate_personalized_manifest(
    bytes: &[u8],
    image_name: &str,
    trust_cache_name: &str,
) -> Result<(), String> {
    let manifest = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|error| format!("invalid developer image BuildManifest: {error}"))?;
    let dictionary = manifest
        .as_dictionary()
        .ok_or_else(|| "developer image BuildManifest must be a dictionary".to_owned())?;
    let identities = dictionary
        .get("BuildIdentities")
        .and_then(plist::Value::as_array)
        .filter(|identities| !identities.is_empty())
        .ok_or_else(|| "developer image BuildManifest has no build identities".to_owned())?;
    let image_name = image_name.to_ascii_lowercase();
    let trust_cache_name = trust_cache_name.to_ascii_lowercase();
    let linked = identities.iter().any(|identity| {
        let Some(manifest) = identity
            .as_dictionary()
            .and_then(|identity| identity.get("Manifest"))
            .and_then(plist::Value::as_dictionary)
        else {
            return false;
        };
        manifest_asset_path(manifest, "PersonalizedDMG")
            .is_some_and(|path| asset_basename(path).eq_ignore_ascii_case(&image_name))
            && manifest_asset_path(manifest, "LoadableTrustCache")
                .is_some_and(|path| asset_basename(path).eq_ignore_ascii_case(&trust_cache_name))
    });
    if !linked {
        return Err(
            "developer image and trust cache are not linked by the same BuildManifest identity"
                .into(),
        );
    }
    Ok(())
}

fn manifest_asset_path<'a>(manifest: &'a plist::Dictionary, key: &str) -> Option<&'a str> {
    manifest
        .get(key)
        .and_then(plist::Value::as_dictionary)
        .and_then(|asset| asset.get("Info"))
        .and_then(plist::Value::as_dictionary)
        .and_then(|info| info.get("Path"))
        .and_then(plist::Value::as_string)
}

fn asset_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn read_product_build_version(path: &Path) -> Result<Option<String>, String> {
    let manifest = plist::Value::from_file(path)
        .map_err(|error| format!("cannot parse developer image plist: {error}"))?;
    Ok(manifest
        .as_dictionary()
        .and_then(|dictionary| dictionary.get("ProductBuildVersion"))
        .and_then(plist::Value::as_string)
        .map(ToOwned::to_owned))
}

#[cfg(target_os = "macos")]
fn scan_xcode_sets() -> Vec<CatalogEntry> {
    let mut entries = Vec::new();
    let ddi_root = PathBuf::from("/Library/Developer/DeveloperDiskImages/iOS_DDI");
    entries.extend(scan_personalized_bundle(
        &ddi_root,
        DeveloperImageSourceKind::Xcode,
    ));
    for developer_root in xcode_developer_roots() {
        let support = developer_root.join("Platforms/iPhoneOS.platform/DeviceSupport");
        let Ok(versions) = std::fs::read_dir(support) else {
            continue;
        };
        for version in versions.filter_map(Result::ok) {
            if let Ok(entry) = scan_legacy_set(&version.path(), DeveloperImageSourceKind::Xcode) {
                entries.push(entry);
            }
        }
    }
    entries
}

#[cfg(target_os = "macos")]
fn xcode_developer_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(output) = std::process::Command::new("xcode-select")
        .arg("-p")
        .output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !path.is_empty() {
            roots.push(PathBuf::from(path));
        }
    }
    if let Ok(applications) = std::fs::read_dir("/Applications") {
        for application in applications.filter_map(Result::ok) {
            let name = application.file_name().to_string_lossy().into_owned();
            if name.starts_with("Xcode") && name.ends_with(".app") {
                let developer = application.path().join("Contents/Developer");
                if !roots.contains(&developer) {
                    roots.push(developer);
                }
            }
        }
    }
    roots
}

fn scan_personalized_bundle(root: &Path, source: DeveloperImageSourceKind) -> Vec<CatalogEntry> {
    let restore = root.join("Restore");
    let assets = if restore.join("BuildManifest.plist").is_file() {
        restore
    } else {
        root.to_path_buf()
    };
    let manifest = assets.join("BuildManifest.plist");
    let version = plist::Value::from_file(root.join("version.plist")).ok();
    let platform = version
        .as_ref()
        .and_then(plist::Value::as_dictionary)
        .and_then(|dictionary| dictionary.get("Platform"))
        .and_then(plist::Value::as_string)
        .unwrap_or("iOS");
    if platform != "iOS" || !manifest.is_file() {
        return Vec::new();
    }
    let build = version
        .as_ref()
        .and_then(plist::Value::as_dictionary)
        .and_then(|dictionary| dictionary.get("ProductBuildVersion"))
        .and_then(plist::Value::as_string)
        .map(ToOwned::to_owned)
        .or_else(|| read_product_build_version(&manifest).ok().flatten());
    let manifest_bytes = match std::fs::read(&manifest) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };
    let Ok(images) = std::fs::read_dir(&assets) else {
        return Vec::new();
    };
    let variants = images
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dmg"))
        })
        .filter_map(|entry| {
            let image = entry.path();
            let image_name = image.file_name()?.to_str()?.to_owned();
            let auxiliary_name = format!("{image_name}.trustcache");
            let trust_cache = [
                assets.join("Firmware").join(&auxiliary_name),
                assets.join(&auxiliary_name),
            ]
            .into_iter()
            .find(|path| path.is_file())?;
            if !trust_cache.is_file()
                || validate_personalized_manifest(&manifest_bytes, &image_name, &auxiliary_name)
                    .is_err()
            {
                return None;
            }
            Some(DeveloperImageVariant {
                image,
                auxiliary: trust_cache,
            })
        })
        .collect::<Vec<_>>();
    if variants.is_empty() {
        return Vec::new();
    }
    match catalog_entry_from_personalized_bundle(manifest, variants, build, source) {
        Ok(entry) => vec![entry],
        Err(error) => {
            tracing::warn!(path = %root.display(), %error, "ignoring invalid personalized developer image bundle");
            Vec::new()
        }
    }
}

fn catalog_entry_from_personalized_bundle(
    manifest: PathBuf,
    variants: Vec<DeveloperImageVariant<PathBuf>>,
    product_build_version: Option<String>,
    source: DeveloperImageSourceKind,
) -> Result<CatalogEntry, String> {
    let mut files = vec![manifest.clone()];
    for variant in &variants {
        files.push(variant.image.clone());
        files.push(variant.auxiliary.clone());
    }
    let id = digest_file_set(&files)?;
    let size_bytes = files.iter().try_fold(0_u64, |total, path| {
        file_size(path).map(|size| total.saturating_add(size))
    })?;
    let first = variants
        .first()
        .ok_or_else(|| "personalized developer image bundle has no variants".to_owned())?;
    let image_name = file_name(&first.image, "developer image")?;
    let auxiliary_name = file_name(&first.auxiliary, "developer image trust cache")?;
    let source_name = match source {
        DeveloperImageSourceKind::Xcode => "Xcode",
        DeveloperImageSourceKind::Custom => "Custom",
        DeveloperImageSourceKind::Managed => "Imported",
    };
    Ok(CatalogEntry {
        descriptor: DeveloperImageSetDescriptor {
            id,
            kind: DeveloperImageKind::Personalized,
            source,
            display_name: product_build_version.as_deref().map_or_else(
                || format!("{source_name} iOS DDI"),
                |build| format!("{source_name} iOS DDI {build}"),
            ),
            platform: "iOS".into(),
            product_version: None,
            product_build_version,
            image_name,
            auxiliary_name,
            manifest_name: Some(file_name(&manifest, "BuildManifest")?),
            size_bytes,
            removable: false,
        },
        request: DeveloperImageMountRequest::Personalized { manifest, variants },
        managed_directory: None,
    })
}

fn scan_legacy_set(root: &Path, source: DeveloperImageSourceKind) -> Result<CatalogEntry, String> {
    let image = root.join("DeveloperDiskImage.dmg");
    let signature = root.join("DeveloperDiskImage.dmg.signature");
    let directory_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "legacy developer image has no version directory".to_owned())?;
    let version = normalized_product_version(directory_name);
    let product_version = (!version.is_empty()).then_some(version.clone());
    let source_name = match source {
        DeveloperImageSourceKind::Xcode => "Xcode",
        DeveloperImageSourceKind::Custom => "Custom",
        DeveloperImageSourceKind::Managed => "Imported",
    };
    catalog_entry_from_files(
        DeveloperImageKind::Legacy,
        source,
        if version.is_empty() {
            format!("{source_name} iOS Developer Disk Image")
        } else {
            format!("{source_name} iOS {version} Developer Disk Image")
        },
        product_version,
        None,
        image,
        signature,
        None,
        false,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn catalog_entry_from_files(
    kind: DeveloperImageKind,
    source: DeveloperImageSourceKind,
    display_name: String,
    product_version: Option<String>,
    product_build_version: Option<String>,
    image: PathBuf,
    auxiliary: PathBuf,
    manifest: Option<PathBuf>,
    removable: bool,
    managed_directory: Option<PathBuf>,
) -> Result<CatalogEntry, String> {
    for path in [&image, &auxiliary] {
        if !path.is_file() {
            return Err("developer image set is incomplete".into());
        }
    }
    if manifest.as_ref().is_some_and(|path| !path.is_file()) {
        return Err("developer image set is missing BuildManifest.plist".into());
    }
    let mut files = vec![image.clone(), auxiliary.clone()];
    if let Some(manifest) = &manifest {
        files.push(manifest.clone());
    }
    let id = digest_file_set(&files)?;
    let size_bytes = files.iter().try_fold(0_u64, |total, path| {
        file_size(path).map(|size| total.saturating_add(size))
    })?;
    let image_name = file_name(&image, "developer image")?;
    let auxiliary_name = file_name(&auxiliary, "developer image auxiliary file")?;
    let manifest_name = manifest
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned);
    let request = match kind {
        DeveloperImageKind::Legacy => DeveloperImageMountRequest::Legacy {
            image,
            signature: auxiliary,
        },
        DeveloperImageKind::Personalized => DeveloperImageMountRequest::Personalized {
            manifest: manifest.expect("validated personalized manifest"),
            variants: vec![DeveloperImageVariant { image, auxiliary }],
        },
    };
    Ok(CatalogEntry {
        descriptor: DeveloperImageSetDescriptor {
            id,
            kind,
            source,
            display_name,
            platform: "iOS".into(),
            product_version,
            product_build_version,
            image_name,
            auxiliary_name,
            manifest_name,
            size_bytes,
            removable,
        },
        request,
        managed_directory,
    })
}

fn file_name(path: &Path, label: &str) -> Result<String, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{label} has an invalid file name"))
}

fn digest_file_set(files: &[PathBuf]) -> Result<String, String> {
    use std::io::Read;
    let mut ordered = files.to_vec();
    ordered.sort();
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    for path in ordered {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "developer image asset has an invalid name".to_owned())?;
        digest.update(name.as_bytes());
        digest.update([0]);
        let mut file = std::fs::File::open(&path)
            .map_err(|error| format!("cannot hash developer image asset: {error}"))?;
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| format!("cannot hash developer image asset: {error}"))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
    }
    Ok(format!("ddi-{}", &format!("{:x}", digest.finalize())[..24]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use devicehub_runtime::DeveloperImageAssetLoader;

    #[tokio::test]
    async fn selected_files_are_absolute_regular_and_size_bounded() {
        let root = std::env::temp_dir().join(format!(
            "devicehub-mask-ddi-loader-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let loader = TokioDeveloperImageCatalog::new_with_discovery(
            root.clone(),
            Vec::new(),
            devicehub_runtime::RuntimePreferences::new(false, false),
            false,
        )
        .unwrap();
        assert!(
            loader
                .load(&PathBuf::from("relative.dmg"), "image", 10)
                .await
                .is_err()
        );
        let path = std::env::temp_dir().join(format!(
            "devicehub-mask-developer-image-{}",
            uuid::Uuid::new_v4().simple()
        ));
        tokio::fs::write(&path, b"image").await.unwrap();
        assert_eq!(loader.load(&path, "image", 5).await.unwrap(), b"image");
        assert!(loader.load(&path, "image", 4).await.is_err());
        tokio::fs::remove_file(path).await.unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn catalog_imports_resolves_and_removes_a_personalized_set() {
        let root = std::env::temp_dir().join(format!(
            "devicehub-mask-ddi-catalog-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let catalog = TokioDeveloperImageCatalog::new_with_discovery(
            root.clone(),
            Vec::new(),
            devicehub_runtime::RuntimePreferences::new(false, false),
            false,
        )
        .unwrap();
        let descriptor = catalog
            .import(personalized_files("Image.dmg", "Image.dmg.trustcache"))
            .await
            .unwrap();
        assert_eq!(descriptor.kind, DeveloperImageKind::Personalized);
        assert_eq!(descriptor.source, DeveloperImageSourceKind::Managed);
        assert!(descriptor.removable);
        assert_eq!(
            catalog.snapshot().unwrap(),
            std::slice::from_ref(&descriptor)
        );
        let request = catalog.resolve(descriptor.id.clone()).await.unwrap();
        let DeveloperImageMountRequest::Personalized { manifest, variants } = request else {
            panic!("expected personalized developer image set");
        };
        assert_eq!(manifest.file_name().unwrap(), "BuildManifest.plist");
        assert_eq!(variants[0].image.file_name().unwrap(), "Image.dmg");
        assert_eq!(
            variants[0].auxiliary.file_name().unwrap(),
            "Image.dmg.trustcache"
        );
        catalog.remove(descriptor.id).await.unwrap();
        assert!(catalog.snapshot().unwrap().is_empty());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[test]
    fn catalog_discovers_bounded_custom_directories_and_refreshes_configuration() {
        let root = std::env::temp_dir().join(format!(
            "devicehub-mask-ddi-custom-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let managed = root.join("managed");
        let custom = root.join("custom");
        let set = custom.join("27.0");
        std::fs::create_dir_all(&set).unwrap();
        std::fs::write(set.join("Image.dmg"), b"image").unwrap();
        std::fs::write(set.join("Image.dmg.trustcache"), b"trust").unwrap();
        std::fs::write(
            set.join("BuildManifest.plist"),
            personalized_manifest("Image.dmg", "Image.dmg.trustcache"),
        )
        .unwrap();

        let catalog = TokioDeveloperImageCatalog::new_with_discovery(
            managed,
            vec![custom.clone()],
            devicehub_runtime::RuntimePreferences::new(false, false),
            false,
        )
        .unwrap();
        let descriptor = catalog.snapshot().unwrap().pop().unwrap();
        assert_eq!(descriptor.source, DeveloperImageSourceKind::Custom);
        assert_eq!(descriptor.kind, DeveloperImageKind::Personalized);
        assert!(!descriptor.removable);
        assert_eq!(catalog.custom_roots().unwrap(), [custom]);

        assert!(catalog.set_custom_roots(Vec::new()).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn personalized_manifest_must_link_the_selected_pair() {
        let manifest = personalized_manifest("Other.dmg", "Other.dmg.trustcache");
        assert!(
            validate_personalized_manifest(&manifest, "Image.dmg", "Image.dmg.trustcache").is_err()
        );
        assert!(safe_asset_name("../Image.dmg").is_err());
    }

    #[test]
    fn legacy_catalog_compatibility_uses_the_ios_version_family() {
        let mut descriptor = DeveloperImageSetDescriptor {
            id: "ddi-0123456789abcdef01234567".into(),
            kind: DeveloperImageKind::Legacy,
            source: DeveloperImageSourceKind::Xcode,
            display_name: "Test".into(),
            platform: "iOS".into(),
            product_version: Some("16.4".into()),
            product_build_version: None,
            image_name: "DeveloperDiskImage.dmg".into(),
            auxiliary_name: "DeveloperDiskImage.dmg.signature".into(),
            manifest_name: None,
            size_bytes: 2,
            removable: false,
        };
        assert_eq!(compatibility_rank(&descriptor, "16.4.1"), 0);
        assert_eq!(compatibility_rank(&descriptor, "16.5"), 2);
        descriptor.product_version = None;
        descriptor.source = DeveloperImageSourceKind::Managed;
        assert_eq!(compatibility_rank(&descriptor, "16.5"), 1);
        assert_eq!(version_family("16.4 (20E247)"), "16.4");
    }

    #[test]
    fn custom_directories_must_be_absolute_unique_and_bounded() {
        assert!(validate_custom_roots(vec![PathBuf::from("relative")]).is_err());
        let root = std::env::temp_dir().join("devicehub-mask-ddi-root");
        assert_eq!(
            validate_custom_roots(vec![root.clone(), root.clone()]).unwrap(),
            [root]
        );
        assert!(
            validate_custom_roots(
                (0..=MAX_CUSTOM_ROOTS)
                    .map(|index| std::env::temp_dir().join(index.to_string()))
                    .collect()
            )
            .is_err()
        );
    }

    fn personalized_files(image: &str, trust_cache: &str) -> Vec<DeveloperImageImportFile> {
        vec![
            DeveloperImageImportFile {
                name: image.into(),
                bytes: Bytes::from_static(b"image"),
            },
            DeveloperImageImportFile {
                name: trust_cache.into(),
                bytes: Bytes::from_static(b"trust"),
            },
            DeveloperImageImportFile {
                name: "BuildManifest.plist".into(),
                bytes: Bytes::from(personalized_manifest(image, trust_cache)),
            },
        ]
    }

    fn personalized_manifest(image: &str, trust_cache: &str) -> Vec<u8> {
        let identity = plist::Value::Dictionary(plist::Dictionary::from_iter([(
            "Manifest".to_owned(),
            plist::Value::Dictionary(plist::Dictionary::from_iter([
                (
                    "PersonalizedDMG".to_owned(),
                    plist::Value::Dictionary(plist::Dictionary::from_iter([(
                        "Info".to_owned(),
                        plist::Value::Dictionary(plist::Dictionary::from_iter([(
                            "Path".to_owned(),
                            plist::Value::String(image.into()),
                        )])),
                    )])),
                ),
                (
                    "LoadableTrustCache".to_owned(),
                    plist::Value::Dictionary(plist::Dictionary::from_iter([(
                        "Info".to_owned(),
                        plist::Value::Dictionary(plist::Dictionary::from_iter([(
                            "Path".to_owned(),
                            plist::Value::String(format!("Firmware/{trust_cache}")),
                        )])),
                    )])),
                ),
            ])),
        )]));
        let manifest = plist::Value::Dictionary(plist::Dictionary::from_iter([(
            "BuildIdentities".to_owned(),
            plist::Value::Array(vec![identity]),
        )]));
        let mut bytes = Vec::new();
        manifest.to_writer_binary(&mut bytes).unwrap();
        bytes
    }
}
