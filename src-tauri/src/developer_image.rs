//! Developer disk image readiness checks shared by device details and XCTest startup.

use std::future::pending;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use idevice::services::lockdown::LockdownClient;
use idevice::services::mobile_image_mounter::ImageMounter;
use idevice::{IdeviceError, IdeviceService, provider::IdeviceProvider};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::supervisor::ServiceReporter;

const MAX_PATH_BYTES: usize = 4_096;
const MAX_IMAGE_BYTES: u64 = 1_500_000_000;
const MAX_SIGNATURE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TRUST_CACHE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ERROR_CHARS: usize = 1_024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperImageMountState {
    #[default]
    Idle,
    Validating,
    Personalizing,
    Uploading,
    Mounting,
    Unmounting,
    Mounted,
    Unmounted,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DeveloperImageMountStatus {
    pub state: DeveloperImageMountState,
    pub progress_percent: Option<f64>,
    pub product_version: Option<String>,
    pub image_type: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct DeveloperImageMountSlot(Arc<Mutex<DeveloperImageMountStatus>>);

impl DeveloperImageMountSlot {
    pub fn set(&self, status: DeveloperImageMountStatus) {
        *self.0.lock().expect("developer image status lock poisoned") = status;
    }

    pub fn update(&self, update: impl FnOnce(&mut DeveloperImageMountStatus)) {
        update(&mut self.0.lock().expect("developer image status lock poisoned"));
    }

    pub fn get(&self) -> DeveloperImageMountStatus {
        self.0
            .lock()
            .expect("developer image status lock poisoned")
            .clone()
    }

    pub fn reset(&self) {
        self.set(DeveloperImageMountStatus::default());
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeveloperImageMountRequest {
    pub image: PathBuf,
    pub signature: Option<PathBuf>,
    pub trust_cache: Option<PathBuf>,
    pub manifest: Option<PathBuf>,
}

#[derive(Debug)]
pub enum DeveloperImageMountCommand {
    Start {
        request: DeveloperImageMountRequest,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
    Unmount {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub fn image_type_for_version(product_version: &str) -> Result<&'static str, String> {
    let major = product_version
        .split('.')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| format!("invalid iOS version {product_version:?}"))?;
    Ok(if major < 17 {
        "Developer"
    } else {
        "Personalized"
    })
}

pub async fn read_product_version(provider: &dyn IdeviceProvider) -> Result<String, String> {
    let mut lockdown = LockdownClient::connect(provider)
        .await
        .map_err(|error| format!("cannot connect Lockdown: {error:?}"))?;
    lockdown
        .get_value(Some("ProductVersion"), None)
        .await
        .map_err(|error| format!("cannot read iOS version: {error:?}"))?
        .into_string()
        .ok_or_else(|| "device returned an invalid iOS version".to_string())
}

pub async fn is_mounted(
    provider: &dyn IdeviceProvider,
    product_version: &str,
) -> Result<bool, String> {
    let image_type = image_type_for_version(product_version)?;
    let mut mounter = ImageMounter::connect(provider)
        .await
        .map_err(|error| format!("cannot connect mobile image mounter: {error:?}"))?;
    match mounter.lookup_image(image_type).await {
        Ok(_) => Ok(true),
        Err(IdeviceError::NotFound) => Ok(false),
        Err(error) => Err(format!("cannot query developer image: {error:?}")),
    }
}

pub async fn is_mounted_for_device(provider: &dyn IdeviceProvider) -> Result<bool, String> {
    let product_version = read_product_version(provider).await?;
    is_mounted(provider, &product_version).await
}

pub async fn serve(
    provider: Arc<dyn IdeviceProvider>,
    mut commands: mpsc::Receiver<DeveloperImageMountCommand>,
    status: DeveloperImageMountSlot,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut active: Option<JoinHandle<Result<DeveloperImageMountState, String>>> = None;
    let mut attempt = 0;
    status.reset();
    reporter.stopped(attempt);

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    if let Some(task) = active.take() {
                        task.abort();
                        mark_cancelled(&status, "device session ended");
                    }
                    reporter.stopped(attempt);
                    return;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    if let Some(task) = active.take() {
                        task.abort();
                        mark_cancelled(&status, "device session ended");
                    }
                    reporter.stopped(attempt);
                    return;
                };
                match command {
                    DeveloperImageMountCommand::Start { request, reply } => {
                        if active.is_some() {
                            let _ = reply.send(Err("a developer image operation is already running".into()));
                            continue;
                        }
                        attempt += 1;
                        status.set(DeveloperImageMountStatus {
                            state: DeveloperImageMountState::Validating,
                            ..DeveloperImageMountStatus::default()
                        });
                        reporter.connecting(attempt);
                        let mount_provider = provider.clone();
                        let mount_status = status.clone();
                        active = Some(tokio::spawn(async move {
                            mount_image(mount_provider.as_ref(), request, mount_status)
                                .await
                                .map(|_| DeveloperImageMountState::Mounted)
                        }));
                        let _ = reply.send(Ok(()));
                    }
                    DeveloperImageMountCommand::Stop { reply } => {
                        if let Some(task) = active.take() {
                            task.abort();
                            mark_cancelled(&status, "cancelled by user");
                            reporter.stopped(attempt);
                            let _ = reply.send(Ok(()));
                        } else {
                            let _ = reply.send(Err("no developer image operation is running".into()));
                        }
                    }
                    DeveloperImageMountCommand::Unmount { reply } => {
                        if active.is_some() {
                            let _ = reply.send(Err("a developer image operation is already running".into()));
                            continue;
                        }
                        attempt += 1;
                        status.set(DeveloperImageMountStatus {
                            state: DeveloperImageMountState::Unmounting,
                            ..DeveloperImageMountStatus::default()
                        });
                        reporter.connecting(attempt);
                        let unmount_provider = provider.clone();
                        let unmount_status = status.clone();
                        active = Some(tokio::spawn(async move {
                            unmount_image(unmount_provider.as_ref(), unmount_status).await
                        }));
                        let _ = reply.send(Ok(()));
                    }
                }
            }
            result = wait_for_mount(&mut active) => {
                active.take();
                match result {
                    Ok(Ok(completed)) => {
                        status.update(|current| {
                            current.state = completed;
                            current.progress_percent = (completed == DeveloperImageMountState::Mounted)
                                .then_some(100.0);
                            current.error = None;
                        });
                        reporter.stopped(attempt);
                        tracing::info!(state = ?completed, "developer image operation completed");
                    }
                    Ok(Err(error)) => {
                        fail(&status, &reporter, attempt, error);
                    }
                    Err(error) if error.is_cancelled() => {
                        mark_cancelled(&status, "developer image operation cancelled");
                        reporter.stopped(attempt);
                    }
                    Err(error) => {
                        fail(
                            &status,
                            &reporter,
                            attempt,
                            format!("developer image task failed: {error}"),
                        );
                    }
                }
            }
        }
    }
}

async fn wait_for_mount(
    active: &mut Option<JoinHandle<Result<DeveloperImageMountState, String>>>,
) -> Result<Result<DeveloperImageMountState, String>, tokio::task::JoinError> {
    match active.as_mut() {
        Some(task) => task.await,
        None => pending().await,
    }
}

async fn unmount_image(
    provider: &dyn IdeviceProvider,
    status: DeveloperImageMountSlot,
) -> Result<DeveloperImageMountState, String> {
    let product_version = read_product_version(provider).await?;
    let image_type = image_type_for_version(&product_version)?;
    status.update(|current| {
        current.product_version = Some(product_version.clone());
        current.image_type = Some(image_type.to_string());
    });
    if !is_mounted(provider, &product_version).await? {
        return Err("no compatible Developer Disk Image is mounted".into());
    }
    let mount_path = mount_path_for_image_type(image_type);
    let mut mounter = ImageMounter::connect(provider)
        .await
        .map_err(|error| format!("cannot connect mobile image mounter: {error:?}"))?;
    mounter
        .unmount_image(mount_path)
        .await
        .map_err(|error| format!("cannot unmount developer image: {error:?}"))?;
    Ok(DeveloperImageMountState::Unmounted)
}

fn mount_path_for_image_type(image_type: &str) -> &'static str {
    if image_type == "Developer" {
        "/Developer"
    } else {
        "/System/Developer"
    }
}

async fn mount_image(
    provider: &dyn IdeviceProvider,
    request: DeveloperImageMountRequest,
    status: DeveloperImageMountSlot,
) -> Result<(), String> {
    let product_version = read_product_version(provider).await?;
    let image_type = image_type_for_version(&product_version)?;
    status.update(|current| {
        current.product_version = Some(product_version.clone());
        current.image_type = Some(image_type.to_string());
    });
    validate_request_shape(image_type, &request)?;

    validate_file_suffix(&request.image, "developer image", ".dmg")?;
    let assets = if image_type == "Developer" {
        validate_file_suffix(
            request.signature.as_deref().expect("validated signature"),
            "developer image signature",
            ".signature",
        )?;
        let signature = read_bounded_file(
            request.signature.as_deref().expect("validated signature"),
            "developer image signature",
            MAX_SIGNATURE_BYTES,
        )
        .await?;
        MountAssets::Developer { signature }
    } else {
        validate_file_suffix(
            request
                .trust_cache
                .as_deref()
                .expect("validated trust cache"),
            "developer image trust cache",
            ".trustcache",
        )?;
        validate_file_suffix(
            request.manifest.as_deref().expect("validated manifest"),
            "developer image BuildManifest",
            "buildmanifest.plist",
        )?;
        let trust_cache = read_bounded_file(
            request
                .trust_cache
                .as_deref()
                .expect("validated trust cache"),
            "developer image trust cache",
            MAX_TRUST_CACHE_BYTES,
        )
        .await?;
        let manifest = read_bounded_file(
            request.manifest.as_deref().expect("validated manifest"),
            "developer image BuildManifest",
            MAX_MANIFEST_BYTES,
        )
        .await?;
        validate_manifest(&manifest)?;
        let unique_chip_id = read_unique_chip_id(provider).await?;
        MountAssets::Personalized {
            trust_cache,
            manifest,
            unique_chip_id,
        }
    };

    let image = read_bounded_file(&request.image, "developer image", MAX_IMAGE_BYTES).await?;
    let mut mounter = ImageMounter::connect(provider)
        .await
        .map_err(|error| format!("cannot connect mobile image mounter: {error:?}"))?;

    match assets {
        MountAssets::Developer { signature } => {
            status.update(|current| current.state = DeveloperImageMountState::Uploading);
            mounter
                .upload_image_with_progress(
                    "Developer",
                    &image,
                    signature.clone(),
                    update_upload_progress,
                    status.clone(),
                )
                .await
                .map_err(|error| format!("cannot upload developer image: {error:?}"))?;
            status.update(|current| {
                current.state = DeveloperImageMountState::Mounting;
                current.progress_percent = None;
            });
            mounter
                .mount_image("Developer", signature, None, None)
                .await
                .map_err(|error| format!("cannot mount developer image: {error:?}"))?;
        }
        MountAssets::Personalized {
            trust_cache,
            manifest,
            unique_chip_id,
        } => {
            status.update(|current| {
                current.state = DeveloperImageMountState::Personalizing;
                current.progress_percent = None;
            });
            mounter
                .mount_personalized_with_callback(
                    provider,
                    image,
                    trust_cache,
                    &manifest,
                    None,
                    unique_chip_id,
                    update_upload_progress,
                    status.clone(),
                )
                .await
                .map_err(|error| {
                    format!("cannot personalize or mount developer image: {error:?}")
                })?;
        }
    }

    Ok(())
}

enum MountAssets {
    Developer {
        signature: Vec<u8>,
    },
    Personalized {
        trust_cache: Vec<u8>,
        manifest: Vec<u8>,
        unique_chip_id: u64,
    },
}

fn validate_file_suffix(path: &Path, label: &str, suffix: &str) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| format!("{label} has an invalid file name"))?;
    if !file_name.ends_with(suffix) {
        return Err(format!("{label} must end with {suffix}"));
    }
    Ok(())
}

async fn update_upload_progress(
    ((completed, total), status): ((usize, usize), DeveloperImageMountSlot),
) {
    status.update(|current| {
        if total > 0 && completed >= total {
            current.state = DeveloperImageMountState::Mounting;
            current.progress_percent = None;
        } else {
            current.state = DeveloperImageMountState::Uploading;
            current.progress_percent = (total > 0).then(|| completed as f64 * 100.0 / total as f64);
        }
    });
}

fn validate_request_shape(
    image_type: &str,
    request: &DeveloperImageMountRequest,
) -> Result<(), String> {
    if image_type == "Developer" {
        if request.signature.is_none()
            || request.trust_cache.is_some()
            || request.manifest.is_some()
        {
            return Err("iOS 16 and earlier require only an image and signature".into());
        }
    } else if request.signature.is_some()
        || request.trust_cache.is_none()
        || request.manifest.is_none()
    {
        return Err("iOS 17 and later require an image, trust cache, and BuildManifest".into());
    }
    Ok(())
}

async fn read_bounded_file(path: &Path, label: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    if !path.is_absolute() || path.as_os_str().len() > MAX_PATH_BYTES {
        return Err(format!("{label} must be an absolute local file path"));
    }
    let link_metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| format!("{label} is unavailable: {error}"))?;
    if link_metadata.file_type().is_symlink() || !link_metadata.is_file() {
        return Err(format!(
            "{label} must be a regular file, not a symbolic link"
        ));
    }
    if link_metadata.len() == 0 || link_metadata.len() > max_bytes {
        return Err(format!("{label} size is outside the supported range"));
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("cannot open {label}: {error}"))?;
    let mut contents = Vec::with_capacity(
        usize::try_from(link_metadata.len().min(max_bytes)).unwrap_or(usize::MAX),
    );
    file.take(max_bytes + 1)
        .read_to_end(&mut contents)
        .await
        .map_err(|error| format!("cannot read {label}: {error}"))?;
    if contents.is_empty() || contents.len() as u64 > max_bytes {
        return Err(format!("{label} changed while it was being read"));
    }
    Ok(contents)
}

fn validate_manifest(contents: &[u8]) -> Result<(), String> {
    let manifest = plist::Value::from_reader(std::io::Cursor::new(contents))
        .map_err(|error| format!("invalid developer image BuildManifest: {error}"))?;
    let valid = manifest
        .as_dictionary()
        .and_then(|dictionary| dictionary.get("BuildIdentities"))
        .and_then(plist::Value::as_array)
        .is_some_and(|identities| !identities.is_empty());
    if !valid {
        return Err("developer image BuildManifest has no build identities".into());
    }
    Ok(())
}

async fn read_unique_chip_id(provider: &dyn IdeviceProvider) -> Result<u64, String> {
    let mut lockdown = LockdownClient::connect(provider)
        .await
        .map_err(|error| format!("cannot connect Lockdown: {error:?}"))?;
    lockdown
        .get_value(Some("UniqueChipID"), None)
        .await
        .map_err(|error| format!("cannot read device chip identifier: {error:?}"))?
        .as_unsigned_integer()
        .ok_or_else(|| "device returned an invalid chip identifier".to_string())
}

fn fail(status: &DeveloperImageMountSlot, reporter: &ServiceReporter, attempt: u32, error: String) {
    let error = bound_error(error);
    status.update(|current| {
        current.state = DeveloperImageMountState::Failed;
        current.progress_percent = None;
        current.error = Some(error.clone());
    });
    reporter.unavailable(attempt, error.clone());
    tracing::warn!(%error, "developer image mount failed");
}

fn mark_cancelled(status: &DeveloperImageMountSlot, reason: &str) {
    status.update(|current| {
        current.state = DeveloperImageMountState::Cancelled;
        current.progress_percent = None;
        current.error = Some(reason.into());
    });
}

fn bound_error(error: impl Into<String>) -> String {
    error.into().chars().take(MAX_ERROR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> DeveloperImageMountRequest {
        DeveloperImageMountRequest {
            image: PathBuf::from("/DeveloperDiskImage.dmg"),
            signature: None,
            trust_cache: None,
            manifest: None,
        }
    }

    #[test]
    fn image_type_tracks_personalized_ddi_transition() {
        assert_eq!(image_type_for_version("16.7.12").unwrap(), "Developer");
        assert_eq!(image_type_for_version("17.0").unwrap(), "Personalized");
        assert_eq!(image_type_for_version("27.0").unwrap(), "Personalized");
        assert!(image_type_for_version("").is_err());
        assert!(image_type_for_version("future").is_err());
        assert_eq!(mount_path_for_image_type("Developer"), "/Developer");
        assert_eq!(
            mount_path_for_image_type("Personalized"),
            "/System/Developer"
        );
    }

    #[test]
    fn mount_request_requires_only_the_version_specific_files() {
        let mut legacy = request();
        legacy.signature = Some(PathBuf::from("/DeveloperDiskImage.dmg.signature"));
        assert!(validate_request_shape("Developer", &legacy).is_ok());
        legacy.trust_cache = Some(PathBuf::from("/unexpected.trustcache"));
        assert!(validate_request_shape("Developer", &legacy).is_err());

        let mut personalized = request();
        personalized.trust_cache = Some(PathBuf::from("/DeveloperDiskImage.dmg.trustcache"));
        personalized.manifest = Some(PathBuf::from("/BuildManifest.plist"));
        assert!(validate_request_shape("Personalized", &personalized).is_ok());
        personalized.signature = Some(PathBuf::from("/unexpected.signature"));
        assert!(validate_request_shape("Personalized", &personalized).is_err());
    }

    #[test]
    fn selected_file_names_match_their_image_roles() {
        assert!(
            validate_file_suffix(Path::new("/DeveloperDiskImage.dmg"), "image", ".dmg").is_ok()
        );
        assert!(
            validate_file_suffix(
                Path::new("/DeveloperDiskImage.dmg.signature"),
                "signature",
                ".signature"
            )
            .is_ok()
        );
        assert!(
            validate_file_suffix(
                Path::new("/BuildManifest.plist"),
                "manifest",
                "buildmanifest.plist"
            )
            .is_ok()
        );
        assert!(validate_file_suffix(Path::new("/image.zip"), "image", ".dmg").is_err());
    }

    #[test]
    fn build_manifest_requires_nonempty_build_identities() {
        let mut valid = plist::Dictionary::new();
        valid.insert(
            "BuildIdentities".into(),
            plist::Value::Array(vec![plist::Value::Dictionary(plist::Dictionary::new())]),
        );
        let mut bytes = Vec::new();
        plist::to_writer_xml(&mut bytes, &valid).unwrap();
        assert!(validate_manifest(&bytes).is_ok());

        let mut empty = plist::Dictionary::new();
        empty.insert("BuildIdentities".into(), plist::Value::Array(Vec::new()));
        bytes.clear();
        plist::to_writer_xml(&mut bytes, &empty).unwrap();
        assert!(validate_manifest(&bytes).is_err());
        assert!(validate_manifest(b"not a plist").is_err());
    }

    #[tokio::test]
    async fn selected_files_are_absolute_regular_and_size_bounded() {
        assert!(
            read_bounded_file(Path::new("relative.dmg"), "image", 10)
                .await
                .is_err()
        );
        let path = std::env::temp_dir().join(format!(
            "devicehub-mask-developer-image-{}",
            uuid::Uuid::new_v4().simple()
        ));
        tokio::fs::write(&path, b"image").await.unwrap();
        assert_eq!(
            read_bounded_file(&path, "image", 5).await.unwrap(),
            b"image"
        );
        assert!(read_bounded_file(&path, "image", 4).await.is_err());
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[tokio::test]
    async fn upload_progress_moves_to_mounting_after_the_last_byte() {
        let status = DeveloperImageMountSlot::default();
        update_upload_progress(((5, 10), status.clone())).await;
        assert_eq!(status.get().state, DeveloperImageMountState::Uploading);
        assert_eq!(status.get().progress_percent, Some(50.0));

        update_upload_progress(((10, 10), status.clone())).await;
        assert_eq!(status.get().state, DeveloperImageMountState::Mounting);
        assert_eq!(status.get().progress_percent, None);
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn reads_developer_image_status_from_hardware() {
        use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider = device.to_provider(
            UsbmuxdAddr::default(),
            "devicehub-mask-developer-image-test",
        );
        let product_version = read_product_version(&provider).await.unwrap();
        let mounted = is_mounted(&provider, &product_version).await.unwrap();
        println!("iOS {product_version} developer image mounted: {mounted}");
    }
}
