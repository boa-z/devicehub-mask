//! HTTP adapter for the runtime-owned Developer Disk Image lifecycle.
//!
//! Host paths remain opaque command values. The host-injected runtime asset
//! loader performs all filesystem validation and reads outside this adapter.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::extract::{Extension, Multipart, Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use bytes::{Bytes, BytesMut};
use tokio::sync::oneshot;

use crate::device_scope::DeviceScope;

use devicehub_core::{
    DeveloperImageMountSlot, DeveloperImageMountStatus, DeveloperImageSetDescriptor,
};
use devicehub_runtime::{
    DeveloperImageMountCommand, DeveloperImageMountRequest, DeviceSessionCommand,
    RuntimeManagerClient, SessionCommandSlot, SessionControlCommand,
};

type InputCmd = DeviceSessionCommand<PathBuf>;
type InputSink = SessionCommandSlot<PathBuf>;
type RequestSession = Option<Extension<devicehub_runtime::DeviceSessionClient<PathBuf>>>;

const DEVELOPER_IMAGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DEVELOPER_IMAGE_IMPORT_BYTES: usize = 768 * 1024 * 1024;
const MAX_DEVELOPER_IMAGE_FILE_BYTES: usize = 700 * 1024 * 1024;
const MAX_DEVELOPER_IMAGE_AUXILIARY_BYTES: usize = 64 * 1024 * 1024;

pub type DeveloperImageCatalogFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

#[derive(Debug)]
pub struct DeveloperImageImportFile {
    pub name: String,
    pub bytes: Bytes,
}

pub trait DeveloperImageCatalog: Send + Sync + 'static {
    fn snapshot(&self) -> Result<Vec<DeveloperImageSetDescriptor>, String>;
    fn refresh(&self) -> DeveloperImageCatalogFuture<Vec<DeveloperImageSetDescriptor>>;
    fn import(
        &self,
        files: Vec<DeveloperImageImportFile>,
    ) -> DeveloperImageCatalogFuture<DeveloperImageSetDescriptor>;
    fn resolve(
        &self,
        id: String,
    ) -> DeveloperImageCatalogFuture<DeveloperImageMountRequest<PathBuf>>;
    fn remove(&self, id: String) -> DeveloperImageCatalogFuture<()>;
}

#[derive(Clone, Default)]
pub struct DeveloperImageHttpState {
    input: InputSink,
    status: DeveloperImageMountSlot,
    catalog: Option<Arc<dyn DeveloperImageCatalog>>,
    manager: Option<RuntimeManagerClient>,
}

impl DeveloperImageHttpState {
    pub fn new(input: InputSink, status: DeveloperImageMountSlot) -> Self {
        Self {
            input,
            status,
            catalog: None,
            manager: None,
        }
    }

    pub fn with_catalog(mut self, catalog: impl DeveloperImageCatalog) -> Self {
        self.catalog = Some(Arc::new(catalog));
        self
    }

    pub fn with_manager(mut self, manager: RuntimeManagerClient) -> Self {
        self.manager = Some(manager);
        self
    }

    fn catalog(&self) -> Result<Arc<dyn DeveloperImageCatalog>, (StatusCode, String)> {
        self.catalog.clone().ok_or_else(|| {
            (
                StatusCode::NOT_IMPLEMENTED,
                "developer image catalog is unavailable on this host".into(),
            )
        })
    }

    fn input(&self, session: &RequestSession) -> InputSink {
        session
            .as_ref()
            .map(|session| session.commands.clone())
            .unwrap_or_else(|| self.input.clone())
    }

    fn status(&self, session: &RequestSession) -> DeveloperImageMountSlot {
        session
            .as_ref()
            .map(|session| session.developer_image.clone())
            .unwrap_or_else(|| self.status.clone())
    }
}

pub fn router<S>(state: DeveloperImageHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/device/developer-image",
            get(developer_image_status).delete(stop_developer_image_mount),
        )
        .route(
            "/api/device/developer-image/{id}",
            axum::routing::put(start_developer_image_mount),
        )
        .route(
            "/api/device/developer-image/unmount",
            axum::routing::put(unmount_developer_image),
        )
        .route(
            "/api/device/developer-images",
            get(list_developer_images).post(refresh_developer_images),
        )
        .route(
            "/api/device/developer-images/import",
            axum::routing::post(import_developer_image),
        )
        .route(
            "/api/device/developer-images/{id}",
            axum::routing::delete(remove_developer_image),
        )
        .layer(DefaultBodyLimit::max(MAX_DEVELOPER_IMAGE_IMPORT_BYTES))
        .with_state(state)
}

async fn list_developer_images(
    State(state): State<DeveloperImageHttpState>,
) -> Result<Json<Vec<DeveloperImageSetDescriptor>>, (StatusCode, String)> {
    state.catalog()?.snapshot().map(Json).map_err(catalog_error)
}

async fn refresh_developer_images(
    State(state): State<DeveloperImageHttpState>,
) -> Result<Json<Vec<DeveloperImageSetDescriptor>>, (StatusCode, String)> {
    state
        .catalog()?
        .refresh()
        .await
        .map(Json)
        .map_err(catalog_error)
}

async fn import_developer_image(
    State(state): State<DeveloperImageHttpState>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<DeveloperImageSetDescriptor>), (StatusCode, String)> {
    let mut files = Vec::new();
    let mut names = std::collections::HashSet::new();
    while let Some(mut field) = multipart.next_field().await.map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid developer image upload: {error}"),
        )
    })? {
        let Some(name) = field.file_name().map(ToOwned::to_owned) else {
            continue;
        };
        let limit =
            developer_image_asset_limit(&name).map_err(|error| (StatusCode::BAD_REQUEST, error))?;
        if !names.insert(name.to_ascii_lowercase()) {
            return Err((
                StatusCode::BAD_REQUEST,
                "developer image upload contains duplicate file names".into(),
            ));
        }
        let mut bytes = BytesMut::new();
        while let Some(chunk) = field.chunk().await.map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                format!("cannot read developer image upload: {error}"),
            )
        })? {
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("developer image asset {name} exceeds its size limit"),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        files.push(DeveloperImageImportFile {
            name,
            bytes: bytes.freeze(),
        });
    }
    if files.is_empty() || files.len() > 3 {
        return Err((
            StatusCode::BAD_REQUEST,
            "upload one complete legacy or personalized developer image set".into(),
        ));
    }
    state
        .catalog()?
        .import(files)
        .await
        .map(|descriptor| (StatusCode::CREATED, Json(descriptor)))
        .map_err(catalog_error)
}

fn developer_image_asset_limit(name: &str) -> Result<usize, String> {
    if name.is_empty() || name.len() > 255 || name.contains(['/', '\\', '\0']) {
        return Err("developer image upload contains an unsafe file name".into());
    }
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".dmg") {
        Ok(MAX_DEVELOPER_IMAGE_FILE_BYTES)
    } else if lower.ends_with(".signature")
        || lower.ends_with(".trustcache")
        || lower == "buildmanifest.plist"
    {
        Ok(MAX_DEVELOPER_IMAGE_AUXILIARY_BYTES)
    } else {
        Err("developer image uploads accept only .dmg, .signature, .trustcache, and BuildManifest.plist files".into())
    }
}

async fn remove_developer_image(
    State(state): State<DeveloperImageHttpState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .catalog()?
        .remove(id)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(catalog_error)
}

async fn developer_image_status(
    State(state): State<DeveloperImageHttpState>,
    session: RequestSession,
) -> Json<DeveloperImageMountStatus> {
    Json(state.status(&session).get())
}

async fn start_developer_image_mount(
    State(state): State<DeveloperImageHttpState>,
    session: RequestSession,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let request = state.catalog()?.resolve(id).await.map_err(catalog_error)?;
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::DeveloperImageMount(
                DeveloperImageMountCommand::Start { request, reply },
            )),
    )?;
    await_developer_image_command(response, "start developer image mount").await?;
    Ok(StatusCode::NO_CONTENT)
}

fn catalog_error(error: String) -> (StatusCode, String) {
    let status = if error.contains("not found") {
        StatusCode::NOT_FOUND
    } else if error.contains("cannot remove") || error.contains("already exists") {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, error)
}

async fn stop_developer_image_mount(
    State(state): State<DeveloperImageHttpState>,
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::DeveloperImageMount(
                DeveloperImageMountCommand::Stop { reply },
            )),
    )?;
    await_developer_image_command(response, "stop developer image mount").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unmount_developer_image(
    State(state): State<DeveloperImageHttpState>,
    Extension(scope): Extension<DeviceScope>,
) -> Result<StatusCode, (StatusCode, String)> {
    let manager = state.manager.as_ref().ok_or((
        StatusCode::NOT_IMPLEMENTED,
        "developer image unmount management is unavailable on this host".into(),
    ))?;
    let (reply, response) = oneshot::channel();
    manager
        .control
        .send(SessionControlCommand::UnmountDeveloperImage {
            selection_id: scope.selection_id.to_string(),
            status: scope.session.developer_image.clone(),
            reply,
        })
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session manager is unavailable".into(),
            )
        })?;
    await_developer_image_command(response, "unmount developer image").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn await_developer_image_command(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), (StatusCode, String)> {
    let result = tokio::time::timeout(DEVELOPER_IMAGE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                format!("{operation} request timed out"),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?;
    result.map_err(|error| {
        let status = if error.contains("already running") || error.contains("no developer image") {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        (status, error)
    })
}

fn require_active_session(sent: bool) -> Result<(), (StatusCode, String)> {
    sent.then_some(()).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::{DeveloperImageKind, DeveloperImageMountState, DeveloperImageSourceKind};

    #[derive(Clone)]
    struct TestCatalog;

    impl DeveloperImageCatalog for TestCatalog {
        fn snapshot(&self) -> Result<Vec<DeveloperImageSetDescriptor>, String> {
            Ok(vec![test_descriptor()])
        }

        fn refresh(&self) -> DeveloperImageCatalogFuture<Vec<DeveloperImageSetDescriptor>> {
            Box::pin(async { Ok(vec![test_descriptor()]) })
        }

        fn import(
            &self,
            _files: Vec<DeveloperImageImportFile>,
        ) -> DeveloperImageCatalogFuture<DeveloperImageSetDescriptor> {
            Box::pin(async { Ok(test_descriptor()) })
        }

        fn resolve(
            &self,
            id: String,
        ) -> DeveloperImageCatalogFuture<DeveloperImageMountRequest<PathBuf>> {
            Box::pin(async move {
                if id != "ddi-0123456789abcdef01234567" {
                    return Err("developer image set not found".into());
                }
                Ok(DeveloperImageMountRequest::Personalized {
                    manifest: PathBuf::from("/BuildManifest.plist"),
                    variants: vec![devicehub_runtime::DeveloperImageVariant {
                        image: PathBuf::from("/DeveloperDiskImage.dmg"),
                        auxiliary: PathBuf::from("/DeveloperDiskImage.dmg.trustcache"),
                    }],
                })
            })
        }

        fn remove(&self, _id: String) -> DeveloperImageCatalogFuture<()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn test_descriptor() -> DeveloperImageSetDescriptor {
        DeveloperImageSetDescriptor {
            id: "ddi-0123456789abcdef01234567".into(),
            kind: DeveloperImageKind::Personalized,
            source: DeveloperImageSourceKind::Managed,
            display_name: "Test DDI".into(),
            platform: "iOS".into(),
            product_version: Some("27.0".into()),
            product_build_version: Some("27A1".into()),
            image_name: "DeveloperDiskImage.dmg".into(),
            auxiliary_name: "DeveloperDiskImage.dmg.trustcache".into(),
            manifest_name: Some("BuildManifest.plist".into()),
            size_bytes: 3,
            removable: true,
        }
    }

    fn test_state() -> (
        DeveloperImageHttpState,
        devicehub_runtime::DeviceSessionClient<PathBuf>,
        tokio::sync::mpsc::UnboundedReceiver<InputCmd>,
        tokio::sync::mpsc::UnboundedReceiver<SessionControlCommand>,
    ) {
        let input = InputSink::default();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(sender));
        let (application, manager_commands) =
            devicehub_runtime::RuntimeClientFixture::<PathBuf>::default().build();
        (
            DeveloperImageHttpState::new(input, DeveloperImageMountSlot::default())
                .with_manager(application.manager.clone())
                .with_catalog(TestCatalog),
            application.device,
            receiver,
            manager_commands,
        )
    }

    #[tokio::test]
    async fn lifecycle_routes_dispatch_opaque_host_sources() {
        let (state, session, mut commands, mut manager_commands) = test_state();
        assert_eq!(
            developer_image_status(State(state.clone()), None)
                .await
                .0
                .state,
            DeveloperImageMountState::Idle
        );
        let start = tokio::spawn(start_developer_image_mount(
            State(state.clone()),
            None,
            Path("ddi-0123456789abcdef01234567".into()),
        ));
        let InputCmd::DeveloperImageMount(DeveloperImageMountCommand::Start { request, reply }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected start command");
        };
        let DeveloperImageMountRequest::Personalized { manifest, variants } = request else {
            panic!("expected personalized developer image set");
        };
        assert_eq!(manifest, PathBuf::from("/BuildManifest.plist"));
        assert_eq!(variants[0].image, PathBuf::from("/DeveloperDiskImage.dmg"));
        assert_eq!(
            variants[0].auxiliary,
            PathBuf::from("/DeveloperDiskImage.dmg.trustcache")
        );
        reply.send(Ok(())).unwrap();
        assert_eq!(start.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let stop = tokio::spawn(stop_developer_image_mount(State(state.clone()), None));
        let InputCmd::DeveloperImageMount(DeveloperImageMountCommand::Stop { reply }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected stop command");
        };
        reply.send(Ok(())).unwrap();
        assert_eq!(stop.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let scope = DeviceScope::new("phone::usb", session);
        let unmount = tokio::spawn(unmount_developer_image(State(state), Extension(scope)));
        let SessionControlCommand::UnmountDeveloperImage {
            selection_id,
            status,
            reply,
        } = manager_commands.recv().await.unwrap()
        else {
            panic!("expected unmount command");
        };
        assert_eq!(selection_id, "phone::usb");
        assert_eq!(status.get().state, DeveloperImageMountState::Idle);
        reply.send(Ok(())).unwrap();
        assert_eq!(unmount.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn lifecycle_routes_require_an_active_session() {
        let state = DeveloperImageHttpState::default().with_catalog(TestCatalog);
        assert!(matches!(
            start_developer_image_mount(
                State(state),
                None,
                Path("ddi-0123456789abcdef01234567".into()),
            )
            .await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }

    #[test]
    fn router_constructs_without_filesystem_or_runtime_owner() {
        let _: Router = router(DeveloperImageHttpState::default());
    }

    #[test]
    fn browser_import_accepts_only_bounded_developer_image_assets() {
        assert_eq!(
            developer_image_asset_limit("DeveloperDiskImage.dmg").unwrap(),
            MAX_DEVELOPER_IMAGE_FILE_BYTES
        );
        assert_eq!(
            developer_image_asset_limit("BuildManifest.plist").unwrap(),
            MAX_DEVELOPER_IMAGE_AUXILIARY_BYTES
        );
        assert!(developer_image_asset_limit("../BuildManifest.plist").is_err());
        assert!(developer_image_asset_limit("image.zip").is_err());
    }
}
