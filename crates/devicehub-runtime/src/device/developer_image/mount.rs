//! Supervised Developer Disk Image mount lifecycle with host-injected assets.

use std::future::Future;
use std::future::pending;
use std::pin::Pin;
use std::sync::Arc;

use devicehub_core::{
    DeveloperImageMountSlot, DeveloperImageMountState, DeveloperImageMountStatus,
    DeveloperImageOperation, ManagedOperationError, ManagedOperationKind, ManagedOperationRegistry,
    OperationErrorCode, developer_image_type_for_version as image_type_for_version,
};
use idevice::services::lockdown::LockdownClient;
use idevice::services::mobile_image_mounter::ImageMounter;
use idevice::{IdeviceError, IdeviceService, provider::IdeviceProvider};
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use super::{
    is_developer_image_mounted as is_mounted, read_device_product_version as read_product_version,
};
use crate::supervisor::ServiceReporter;

const MAX_IMAGE_BYTES: u64 = 1_500_000_000;
const MAX_SIGNATURE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TRUST_CACHE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ERROR_CHARS: usize = 1_024;
const UNMOUNT_BUSY_RETRY_DELAYS: [std::time::Duration; 4] = [
    std::time::Duration::from_millis(250),
    std::time::Duration::from_millis(750),
    std::time::Duration::from_millis(1_500),
    std::time::Duration::from_millis(3_000),
];

pub type DeveloperImageAssetFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>>;
pub type DeveloperImageAutomaticRequestFuture<'a, Source> = Pin<
    Box<
        dyn Future<Output = Result<Option<DeveloperImageMountRequest<Source>>, String>> + Send + 'a,
    >,
>;

/// Loads and validates host-owned Developer Disk Image assets.
///
/// The runtime treats source handles as opaque. Desktop and headless hosts own
/// path interpretation, symbolic-link policy, and bounded local reads.
pub trait DeveloperImageAssetLoader: Clone + Send + Sync + 'static {
    type Source: Send + Sync + 'static;

    fn file_name(&self, source: &Self::Source, label: &str) -> Result<String, String>;

    fn load<'a>(
        &'a self,
        source: &'a Self::Source,
        label: &'a str,
        max_bytes: u64,
    ) -> DeveloperImageAssetFuture<'a>;

    fn automatic_enabled(&self) -> bool {
        false
    }

    /// Resolve the host-selected image set for a newly connected device.
    /// Returning `None` keeps mounting fully command-driven.
    fn automatic_request<'a>(
        &'a self,
        _product_version: &'a str,
    ) -> DeveloperImageAutomaticRequestFuture<'a, Self::Source> {
        Box::pin(async { Ok(None) })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeveloperImageVariant<Source> {
    pub image: Source,
    pub auxiliary: Source,
}

#[derive(Debug, Clone, Deserialize)]
pub enum DeveloperImageMountRequest<Source> {
    Legacy {
        image: Source,
        signature: Source,
    },
    Personalized {
        manifest: Source,
        variants: Vec<DeveloperImageVariant<Source>>,
    },
}

#[derive(Debug)]
pub enum DeveloperImageMountCommand<Source> {
    Start {
        request: DeveloperImageMountRequest<Source>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub(crate) async fn serve<Assets>(
    provider: Arc<dyn IdeviceProvider>,
    mut commands: mpsc::Receiver<DeveloperImageMountCommand<Assets::Source>>,
    status: DeveloperImageMountSlot,
    operations: ManagedOperationRegistry,
    assets: Assets,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) where
    Assets: DeveloperImageAssetLoader,
{
    let mut active: Option<ActiveDeveloperImageOperation> = None;
    let mut attempt = 0;
    status.reset();
    reporter.stopped(attempt);

    if assets.automatic_enabled() {
        match read_product_version(provider.as_ref()).await {
            Ok(product_version) => match is_mounted(provider.as_ref(), &product_version).await {
                Ok(true) => {
                    status.set(DeveloperImageMountStatus {
                        state: DeveloperImageMountState::Mounted,
                        operation: None,
                        progress_percent: Some(100.0),
                        product_version: Some(product_version),
                        image_type: None,
                        error: None,
                    });
                }
                Ok(false) => match assets.automatic_request(&product_version).await {
                    Ok(Some(request)) => {
                        match operations.begin(
                            ManagedOperationKind::DeveloperImageMount,
                            Some(product_version.clone()),
                            true,
                        ) {
                            Ok(managed_id) => {
                                attempt += 1;
                                status.set(DeveloperImageMountStatus {
                                    state: DeveloperImageMountState::Validating,
                                    operation: Some(DeveloperImageOperation::Mount),
                                    product_version: Some(product_version),
                                    ..DeveloperImageMountStatus::default()
                                });
                                reporter.connecting(attempt);
                                active = Some(ActiveDeveloperImageOperation::spawn(
                                    managed_id,
                                    provider.clone(),
                                    request,
                                    status.clone(),
                                    assets.clone(),
                                ));
                                tracing::info!(
                                    "automatically mounting recommended developer image set"
                                );
                            }
                            Err(error) => {
                                fail(&status, &reporter, attempt, error.message);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => fail(&status, &reporter, attempt, error),
                },
                Err(error) => {
                    tracing::debug!(%error, "developer image readiness unavailable at session startup")
                }
            },
            Err(error) => {
                tracing::debug!(%error, "device version unavailable for developer image policy")
            }
        }
    }

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    if let Some(operation) = active.take() {
                        operation.cancel(&operations, "device session ended").await;
                        mark_cancelled(&status, "device session ended");
                    }
                    reporter.stopped(attempt);
                    return;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    if let Some(operation) = active.take() {
                        operation.cancel(&operations, "device session ended").await;
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
                        let managed_id = match operations.begin(
                            ManagedOperationKind::DeveloperImageMount,
                            None,
                            true,
                        ) {
                            Ok(id) => id,
                            Err(error) => {
                                let _ = reply.send(Err(error.message));
                                continue;
                            }
                        };
                        status.set(DeveloperImageMountStatus {
                            state: DeveloperImageMountState::Validating,
                            operation: Some(DeveloperImageOperation::Mount),
                            ..DeveloperImageMountStatus::default()
                        });
                        reporter.connecting(attempt);
                        active = Some(ActiveDeveloperImageOperation::spawn(
                            managed_id,
                            provider.clone(),
                            request,
                            status.clone(),
                            assets.clone(),
                        ));
                        let _ = reply.send(Ok(()));
                    }
                    DeveloperImageMountCommand::Stop { reply } => {
                        if let Some(operation) = active.take() {
                            operation.cancel(&operations, "cancelled by user").await;
                            mark_cancelled(&status, "cancelled by user");
                            reporter.stopped(attempt);
                            let _ = reply.send(Ok(()));
                        } else {
                            let _ = reply.send(Err("no developer image operation is running".into()));
                        }
                    }
                }
            }
            result = wait_for_mount(&mut active) => {
                let completed_operation = active.take().expect("completed operation exists");
                match result {
                    Ok(Ok(completed)) => {
                        status.update(|current| {
                            current.state = completed;
                            current.progress_percent = (completed == DeveloperImageMountState::Mounted)
                                .then_some(100.0);
                            current.error = None;
                        });
                        reporter.stopped(attempt);
                        operations.succeed(completed_operation.managed_id);
                        tracing::info!(state = ?completed, "developer image operation completed");
                    }
                    Ok(Err(error)) => {
                        operations.fail(
                            completed_operation.managed_id,
                            ManagedOperationError::new(OperationErrorCode::Internal, error.clone()),
                        );
                        fail(&status, &reporter, attempt, error);
                    }
                    Err(error) if error.is_cancelled() => {
                        operations.cancel(
                            completed_operation.managed_id,
                            "developer image operation cancelled",
                        );
                        mark_cancelled(&status, "developer image operation cancelled");
                        reporter.stopped(attempt);
                    }
                    Err(error) => {
                        operations.fail(
                            completed_operation.managed_id,
                            ManagedOperationError::new(
                                OperationErrorCode::Internal,
                                format!("developer image task failed: {error}"),
                            ),
                        );
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

struct ActiveDeveloperImageOperation {
    managed_id: u64,
    task: JoinHandle<Result<DeveloperImageMountState, String>>,
}

impl ActiveDeveloperImageOperation {
    fn spawn<Assets>(
        managed_id: u64,
        provider: Arc<dyn IdeviceProvider>,
        request: DeveloperImageMountRequest<Assets::Source>,
        status: DeveloperImageMountSlot,
        assets: Assets,
    ) -> Self
    where
        Assets: DeveloperImageAssetLoader,
    {
        Self {
            managed_id,
            task: tokio::spawn(async move {
                mount_image(provider.as_ref(), request, status, &assets)
                    .await
                    .map(|_| DeveloperImageMountState::Mounted)
            }),
        }
    }

    async fn cancel(self, operations: &ManagedOperationRegistry, reason: &str) {
        self.task.abort();
        let _ = self.task.await;
        operations.cancel(self.managed_id, reason);
    }
}

async fn wait_for_mount(
    active: &mut Option<ActiveDeveloperImageOperation>,
) -> Result<Result<DeveloperImageMountState, String>, tokio::task::JoinError> {
    match active.as_mut() {
        Some(operation) => (&mut operation.task).await,
        None => pending().await,
    }
}

pub(crate) async fn unmount_image(
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
        .map_err(|error| developer_image_protocol_error("unmount", error))?;
    for (attempt, delay) in UNMOUNT_BUSY_RETRY_DELAYS.iter().enumerate() {
        match mounter.unmount_image(mount_path).await {
            Ok(()) => return Ok(DeveloperImageMountState::Unmounted),
            Err(error) if is_developer_service_busy_error(&error) => {
                tracing::info!(
                    attempt = attempt + 1,
                    retry_ms = delay.as_millis(),
                    "developer image services are still stopping; retrying unmount"
                );
                tokio::time::sleep(*delay).await;
            }
            Err(error) => return Err(developer_image_protocol_error("unmount", error)),
        }
    }
    mounter
        .unmount_image(mount_path)
        .await
        .map_err(|error| developer_image_protocol_error("unmount", error))?;
    Ok(DeveloperImageMountState::Unmounted)
}

fn mount_path_for_image_type(image_type: &str) -> &'static str {
    if image_type == "Developer" {
        "/Developer"
    } else {
        "/System/Developer"
    }
}

async fn mount_image<Assets>(
    provider: &dyn IdeviceProvider,
    request: DeveloperImageMountRequest<Assets::Source>,
    status: DeveloperImageMountSlot,
    asset_loader: &Assets,
) -> Result<(), String>
where
    Assets: DeveloperImageAssetLoader,
{
    let product_version = read_product_version(provider).await?;
    let image_type = image_type_for_version(&product_version)?;
    status.update(|current| {
        current.product_version = Some(product_version.clone());
        current.image_type = Some(image_type.to_string());
    });
    if is_mounted(provider, &product_version).await? {
        status.update(|current| {
            current.state = DeveloperImageMountState::Mounted;
            current.progress_percent = Some(100.0);
            current.error = None;
        });
        tracing::info!(image_type, "compatible developer image is already mounted");
        return Ok(());
    }
    let assets = match (image_type, request) {
        ("Developer", DeveloperImageMountRequest::Legacy { image, signature }) => {
            validate_file_suffix(
                &asset_loader.file_name(&image, "developer image")?,
                "developer image",
                ".dmg",
            )?;
            validate_file_suffix(
                &asset_loader.file_name(&signature, "developer image signature")?,
                "developer image signature",
                ".signature",
            )?;
            let signature = asset_loader
                .load(&signature, "developer image signature", MAX_SIGNATURE_BYTES)
                .await?;
            let image = asset_loader
                .load(&image, "developer image", MAX_IMAGE_BYTES)
                .await?;
            MountAssets::Developer { image, signature }
        }
        ("Personalized", DeveloperImageMountRequest::Personalized { manifest, variants }) => {
            if variants.is_empty() {
                return Err("personalized developer image set has no variants".into());
            }
            validate_file_suffix(
                &asset_loader.file_name(&manifest, "developer image BuildManifest")?,
                "developer image BuildManifest",
                "buildmanifest.plist",
            )?;
            let manifest = asset_loader
                .load(
                    &manifest,
                    "developer image BuildManifest",
                    MAX_MANIFEST_BYTES,
                )
                .await?;
            validate_manifest(&manifest)?;
            let variant =
                select_personalized_variant(provider, &manifest, variants, asset_loader).await?;
            validate_file_suffix(
                &asset_loader.file_name(&variant.image, "developer image")?,
                "developer image",
                ".dmg",
            )?;
            validate_file_suffix(
                &asset_loader.file_name(&variant.auxiliary, "developer image trust cache")?,
                "developer image trust cache",
                ".trustcache",
            )?;
            let trust_cache = asset_loader
                .load(
                    &variant.auxiliary,
                    "developer image trust cache",
                    MAX_TRUST_CACHE_BYTES,
                )
                .await?;
            let image = asset_loader
                .load(&variant.image, "developer image", MAX_IMAGE_BYTES)
                .await?;
            let unique_chip_id = read_unique_chip_id(provider).await?;
            MountAssets::Personalized {
                image,
                trust_cache,
                manifest,
                unique_chip_id,
            }
        }
        (_, DeveloperImageMountRequest::Personalized { .. }) => {
            return Err("iOS 16 and earlier require a legacy developer image set".into());
        }
        (_, DeveloperImageMountRequest::Legacy { .. }) => {
            return Err("iOS 17 and later require a personalized developer image set".into());
        }
    };
    let mut mounter = ImageMounter::connect(provider)
        .await
        .map_err(|error| format!("cannot connect mobile image mounter: {error:?}"))?;

    match assets {
        MountAssets::Developer { image, signature } => {
            status.update(|current| current.state = DeveloperImageMountState::Uploading);
            mounter
                .mount_developer(&image, signature)
                .await
                .map_err(|error| developer_image_protocol_error("mount", error))?;
        }
        MountAssets::Personalized {
            image,
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
                .map_err(|error| developer_image_protocol_error("personalize or mount", error))?;
        }
    }

    Ok(())
}

fn developer_image_protocol_error(operation: &str, error: IdeviceError) -> String {
    if matches!(
        error,
        IdeviceError::DeviceLocked | IdeviceError::PasswordProtected
    ) {
        format!(
            "cannot {operation} developer image: device is locked; unlock it, keep the screen awake, and try again"
        )
    } else if operation == "unmount" && is_developer_service_busy_error(&error) {
        "cannot unmount developer image because developer services are still in use; close Xcode, WebDriverAgent, XCTest, and other device tools, then try again".into()
    } else {
        format!("cannot {operation} developer image: {error:?}")
    }
}

fn is_developer_service_busy_error(error: &IdeviceError) -> bool {
    matches!(
        error,
        IdeviceError::InternalError(message) if message.contains("Failed to unload launchd jobs")
    )
}

enum MountAssets {
    Developer {
        image: Vec<u8>,
        signature: Vec<u8>,
    },
    Personalized {
        image: Vec<u8>,
        trust_cache: Vec<u8>,
        manifest: Vec<u8>,
        unique_chip_id: u64,
    },
}

fn validate_file_suffix(file_name: &str, label: &str, suffix: &str) -> Result<(), String> {
    let file_name = file_name.to_ascii_lowercase();
    if !file_name.ends_with(suffix) {
        return Err(format!("{label} must end with {suffix}"));
    }
    Ok(())
}

async fn select_personalized_variant<Assets>(
    provider: &dyn IdeviceProvider,
    manifest: &[u8],
    variants: Vec<DeveloperImageVariant<Assets::Source>>,
    asset_loader: &Assets,
) -> Result<DeveloperImageVariant<Assets::Source>, String>
where
    Assets: DeveloperImageAssetLoader,
{
    let mut mounter = ImageMounter::connect(provider)
        .await
        .map_err(|error| format!("cannot connect mobile image mounter: {error:?}"))?;
    let identifiers = mounter
        .query_personalization_identifiers(None)
        .await
        .map_err(|error| format!("cannot query DDI personalization identifiers: {error:?}"))?;
    let board_id = personalization_id(&identifiers, "BoardId")?;
    let chip_id = personalization_id(&identifiers, "ChipID")?;
    let manifest: plist::Dictionary = plist::from_bytes(manifest)
        .map_err(|error| format!("invalid developer image BuildManifest: {error}"))?;
    let identity = idevice::tss::select_build_identity(&manifest, board_id, chip_id, None)
        .map_err(|_| {
            format!(
                "developer image BuildManifest has no identity for BoardId 0x{board_id:x}, ChipID 0x{chip_id:x}"
            )
        })?;
    let identity_manifest = identity
        .get("Manifest")
        .and_then(plist::Value::as_dictionary)
        .ok_or_else(|| "selected developer image identity has no Manifest".to_owned())?;
    let image_name = identity_asset_name(identity_manifest, "PersonalizedDMG")?;
    let trust_cache_name = identity_asset_name(identity_manifest, "LoadableTrustCache")?;

    variants
        .into_iter()
        .find(|variant| {
            asset_loader
                .file_name(&variant.image, "developer image")
                .is_ok_and(|name| name.eq_ignore_ascii_case(image_name))
                && asset_loader
                    .file_name(&variant.auxiliary, "developer image trust cache")
                    .is_ok_and(|name| name.eq_ignore_ascii_case(trust_cache_name))
        })
        .ok_or_else(|| {
            format!(
                "developer image set does not contain the {image_name} and {trust_cache_name} variant required by this device"
            )
        })
}

fn personalization_id(identifiers: &plist::Dictionary, key: &str) -> Result<u64, String> {
    identifiers
        .get(key)
        .and_then(plist::Value::as_unsigned_integer)
        .ok_or_else(|| format!("device personalization identifiers have no valid {key}"))
}

fn identity_asset_name<'a>(manifest: &'a plist::Dictionary, key: &str) -> Result<&'a str, String> {
    let path = manifest
        .get(key)
        .and_then(plist::Value::as_dictionary)
        .and_then(|asset| asset.get("Info"))
        .and_then(plist::Value::as_dictionary)
        .and_then(|info| info.get("Path"))
        .and_then(plist::Value::as_string)
        .ok_or_else(|| format!("selected developer image identity has no {key} path"))?;
    Ok(path.rsplit(['/', '\\']).next().unwrap_or(path))
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
    fn selected_file_names_match_their_image_roles() {
        assert!(validate_file_suffix("DeveloperDiskImage.dmg", "image", ".dmg").is_ok());
        assert!(
            validate_file_suffix(
                "DeveloperDiskImage.dmg.signature",
                "signature",
                ".signature"
            )
            .is_ok()
        );
        assert!(
            validate_file_suffix("BuildManifest.plist", "manifest", "buildmanifest.plist").is_ok()
        );
        assert!(validate_file_suffix("image.zip", "image", ".dmg").is_err());
    }

    #[test]
    fn locked_device_errors_are_actionable() {
        let error = developer_image_protocol_error("unmount", IdeviceError::DeviceLocked);
        assert!(error.contains("cannot unmount developer image"));
        assert!(error.contains("unlock it"));
    }

    #[test]
    fn busy_developer_services_are_actionable() {
        let error = developer_image_protocol_error(
            "unmount",
            IdeviceError::InternalError(
                "Failed to unmount /System/Developer: Failed to unload launchd jobs.".into(),
            ),
        );
        assert!(error.contains("developer services are still in use"));
        assert!(error.contains("close Xcode"));
        assert!(!error.contains("MobileStorage"));
    }

    #[test]
    fn only_launchd_unload_failures_are_retried_as_busy() {
        assert!(is_developer_service_busy_error(
            &IdeviceError::InternalError("Failed to unload launchd jobs.".into(),)
        ));
        assert!(!is_developer_service_busy_error(
            &IdeviceError::InternalError("ImageMountFailed".into(),)
        ));
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
        use idevice::usbmuxd::UsbmuxdAddr;

        let device = crate::test_support::usb_test_device().await;
        let provider = device.to_provider(
            UsbmuxdAddr::default(),
            "devicehub-mask-developer-image-test",
        );
        let product_version = read_product_version(&provider).await.unwrap();
        let mounted = is_mounted(&provider, &product_version).await.unwrap();
        println!("iOS {product_version} developer image mounted: {mounted}");
    }
}
