//! HTTP adapter for public AFC and per-application storage.
//!
//! The active session owns AFC clients and transfer tasks. This module only
//! validates HTTP requests, dispatches typed commands, and maps responses.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use image::{ImageFormat, ImageReader};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::oneshot;

use super::browser_transfers::{BrowserTransferStore, binary_download, validate_file_name};

use devicehub_core::{
    AppDocumentActivitySlot, AppDocumentActivityView, AppDocumentEntry, AppDocumentList,
    AppStorageScope, DeviceFileActivitySlot, DeviceFileActivityView, DeviceFileEntry,
    DeviceFileList, is_app_document_transfer_cancelled, is_device_file_transfer_cancelled,
    validate_app_bundle_id,
};
type InputCmd = devicehub_runtime::DeviceSessionCommand<PathBuf>;
type InputSink = devicehub_runtime::SessionCommandSlot<PathBuf>;
type RequestSession = Option<Extension<devicehub_runtime::DeviceSessionClient<PathBuf>>>;
type AppDocumentCommand = devicehub_runtime::AppDocumentCommand<PathBuf>;
type DeviceFileCommand = devicehub_runtime::DeviceFileCommand<PathBuf>;

const APP_DOCUMENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(11 * 60);
const DEVICE_FILE_REQUEST_TIMEOUT: Duration = Duration::from_secs(31 * 60);
const BROWSER_UPLOAD_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 64 * 1024 * 1024;
const MAX_PREVIEW_DIMENSION: u32 = 8_192;
const MAX_PREVIEW_PIXELS: u64 = 64 * 1024 * 1024;

/// Narrow capability set for storage HTTP routes. Activity cancellation bypasses
/// the serialized session command queue through the session-owned slots.
#[derive(Clone, Default)]
pub struct StorageHttpState {
    input: InputSink,
    app_document_activity: AppDocumentActivitySlot,
    device_file_activity: DeviceFileActivitySlot,
    browser_transfers: Option<Arc<dyn BrowserTransferStore>>,
}

impl StorageHttpState {
    pub fn new(
        input: InputSink,
        app_document_activity: AppDocumentActivitySlot,
        device_file_activity: DeviceFileActivitySlot,
    ) -> Self {
        Self {
            input,
            app_document_activity,
            device_file_activity,
            browser_transfers: None,
        }
    }

    pub fn with_browser_transfers(mut self, store: impl BrowserTransferStore) -> Self {
        self.browser_transfers = Some(Arc::new(store));
        self
    }

    fn input(&self, session: &RequestSession) -> InputSink {
        session
            .as_ref()
            .map(|session| session.commands.clone())
            .unwrap_or_else(|| self.input.clone())
    }

    fn app_document_activity(&self, session: &RequestSession) -> AppDocumentActivitySlot {
        session
            .as_ref()
            .map(|session| session.app_documents.clone())
            .unwrap_or_else(|| self.app_document_activity.clone())
    }

    fn device_file_activity(&self, session: &RequestSession) -> DeviceFileActivitySlot {
        session
            .as_ref()
            .map(|session| session.device_files.clone())
            .unwrap_or_else(|| self.device_file_activity.clone())
    }
}

/// Injects storage-only state before the routes join the private API.
pub fn router<S>(state: StorageHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/device/files",
            get(device_files).delete(delete_device_file),
        )
        .route("/api/device/files/preview", get(preview_device_file))
        .route(
            "/api/device/files/activity",
            get(device_file_activity).delete(cancel_device_file_activity),
        )
        .route("/api/device/files/export", put(export_device_file))
        .route("/api/device/files/import", put(import_device_file))
        .route(
            "/api/device/files/browser-import",
            put(browser_import_device_file)
                .layer(DefaultBodyLimit::max(BROWSER_UPLOAD_LIMIT_BYTES)),
        )
        .route(
            "/api/device/files/browser-export",
            get(browser_export_device_file),
        )
        .route(
            "/api/device/files/directory",
            put(create_device_file_directory),
        )
        .route("/api/device/files/rename", put(rename_device_file))
        .route(
            "/api/device/apps/{bundle_id}/documents",
            get(app_documents).delete(delete_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/documents/preview",
            get(preview_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/documents/export",
            put(export_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/documents/import",
            put(import_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/documents/directory",
            put(create_app_document_directory),
        )
        .route(
            "/api/device/apps/{bundle_id}/documents/rename",
            put(rename_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/storage",
            get(app_documents).delete(delete_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/storage/preview",
            get(preview_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/storage/export",
            put(export_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/storage/import",
            put(import_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/storage/browser-import",
            put(browser_import_app_document)
                .layer(DefaultBodyLimit::max(BROWSER_UPLOAD_LIMIT_BYTES)),
        )
        .route(
            "/api/device/apps/{bundle_id}/storage/browser-export",
            get(browser_export_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/storage/directory",
            put(create_app_document_directory),
        )
        .route(
            "/api/device/apps/{bundle_id}/storage/rename",
            put(rename_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/storage/activity",
            get(app_document_activity).delete(cancel_app_document_activity),
        )
        .with_state(state)
}

#[derive(Deserialize)]
struct AppDocumentQuery {
    #[serde(default = "storage_root")]
    path: String,
    #[serde(default)]
    scope: AppStorageScope,
    #[serde(default)]
    recursive: bool,
}

fn storage_root() -> String {
    "/".into()
}

#[derive(Deserialize)]
struct ExportAppDocumentRequest {
    path: String,
    destination: PathBuf,
    #[serde(default)]
    scope: AppStorageScope,
}

#[derive(Deserialize)]
struct ImportAppDocumentRequest {
    directory: String,
    source: PathBuf,
    #[serde(default)]
    scope: AppStorageScope,
}

#[derive(Deserialize)]
struct CreateAppDocumentDirectoryRequest {
    directory: String,
    name: String,
    #[serde(default)]
    scope: AppStorageScope,
}

#[derive(Deserialize)]
struct RenameAppDocumentRequest {
    path: String,
    name: String,
    #[serde(default)]
    scope: AppStorageScope,
}

async fn app_documents(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
    Query(query): Query<AppDocumentQuery>,
) -> Result<Json<AppDocumentList>, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state.input(&session),
        AppDocumentCommand::List {
            bundle_id,
            scope: query.scope,
            path: query.path,
            reply,
        },
    )?;
    Ok(Json(
        await_app_document_response(response, "application document listing").await?,
    ))
}

async fn preview_app_document(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
    Query(query): Query<AppDocumentQuery>,
) -> Result<Response, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state.input(&session),
        AppDocumentCommand::Preview {
            bundle_id,
            scope: query.scope,
            path: query.path,
            reply,
        },
    )?;
    let bytes = await_app_document_response(response, "application document preview").await?;
    preview_response(bytes)
}

async fn app_document_activity(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
) -> Result<Json<AppDocumentActivityView>, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    Ok(Json(state.app_document_activity(&session).get(&bundle_id)))
}

async fn cancel_app_document_activity(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    if state.app_document_activity(&session).cancel(&bundle_id) {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err((
            StatusCode::CONFLICT,
            "no application storage transfer is running for this app".into(),
        ))
    }
}

async fn export_app_document(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
    Json(request): Json<ExportAppDocumentRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state.input(&session),
        AppDocumentCommand::Export {
            bundle_id,
            scope: request.scope,
            path: request.path,
            destination: request.destination,
            reply,
        },
    )?;
    let transfer = await_app_document_response(response, "application document export").await?;
    Ok(Json(json!({
        "bytes_written": transfer.bytes_transferred,
        "files_written": transfer.files_transferred,
        "directories_written": transfer.directories_transferred,
    })))
}

async fn import_app_document(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
    Json(request): Json<ImportAppDocumentRequest>,
) -> Result<Json<AppDocumentEntry>, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state.input(&session),
        AppDocumentCommand::Import {
            bundle_id,
            scope: request.scope,
            directory: request.directory,
            source: request.source,
            reply,
        },
    )?;
    Ok(Json(
        await_app_document_response(response, "application document upload").await?,
    ))
}

#[derive(Deserialize)]
struct BrowserAppDocumentQuery {
    directory: String,
    name: String,
    #[serde(default)]
    scope: AppStorageScope,
}

#[derive(Deserialize)]
struct BrowserAppDocumentExportQuery {
    path: String,
    name: String,
    #[serde(default)]
    scope: AppStorageScope,
}

async fn browser_import_app_document(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
    Query(query): Query<BrowserAppDocumentQuery>,
    bytes: Bytes,
) -> Result<Json<AppDocumentEntry>, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    validate_browser_file_name(&query.name)?;
    let store = browser_transfer_store(&state)?;
    let source = store
        .stage_upload(query.name, bytes)
        .await
        .map_err(browser_transfer_error)?;
    let cleanup = source.clone();
    let (reply, response) = oneshot::channel();
    let result = dispatch_app_document_command(
        &state.input(&session),
        AppDocumentCommand::Import {
            bundle_id,
            scope: query.scope,
            directory: query.directory,
            source,
            reply,
        },
    )
    .map(|_| response);
    let result = match result {
        Ok(response) => await_app_document_response(response, "browser application upload").await,
        Err(error) => Err(error),
    };
    let _ = store.remove(cleanup).await;
    result.map(Json)
}

async fn browser_export_app_document(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
    Query(query): Query<BrowserAppDocumentExportQuery>,
) -> Result<Response, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    validate_browser_file_name(&query.name)?;
    let store = browser_transfer_store(&state)?;
    let destination = store
        .prepare_download(query.name)
        .await
        .map_err(browser_transfer_error)?;
    let cleanup = destination.clone();
    let (reply, response) = oneshot::channel();
    if let Err(error) = dispatch_app_document_command(
        &state.input(&session),
        AppDocumentCommand::Export {
            bundle_id,
            scope: query.scope,
            path: query.path,
            destination,
            reply,
        },
    ) {
        let _ = store.remove(cleanup).await;
        return Err(error);
    }
    if let Err(error) = await_app_document_response(response, "browser application download").await
    {
        let _ = store.remove(cleanup).await;
        return Err(error);
    }
    let bytes = store
        .read_and_remove(cleanup)
        .await
        .map_err(browser_transfer_error)?;
    Ok(binary_download(bytes))
}

async fn create_app_document_directory(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
    Json(request): Json<CreateAppDocumentDirectoryRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state.input(&session),
        AppDocumentCommand::CreateDirectory {
            bundle_id,
            scope: request.scope,
            directory: request.directory,
            name: request.name,
            reply,
        },
    )?;
    await_app_document_response(response, "application directory creation").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rename_app_document(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
    Json(request): Json<RenameAppDocumentRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state.input(&session),
        AppDocumentCommand::Rename {
            bundle_id,
            scope: request.scope,
            path: request.path,
            name: request.name,
            reply,
        },
    )?;
    await_app_document_response(response, "application document rename").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_app_document(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Path(bundle_id): Path<String>,
    Query(query): Query<AppDocumentQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state.input(&session),
        AppDocumentCommand::Delete {
            bundle_id,
            scope: query.scope,
            path: query.path,
            recursive: query.recursive,
            reply,
        },
    )?;
    await_app_document_response(response, "application document deletion").await?;
    Ok(StatusCode::NO_CONTENT)
}

fn validate_app_document_bundle(bundle_id: &str) -> Result<(), (StatusCode, String)> {
    validate_app_bundle_id(bundle_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid bundle identifier".into()))
}

fn dispatch_app_document_command(
    input: &InputSink,
    command: AppDocumentCommand,
) -> Result<(), (StatusCode, String)> {
    if input.try_send(InputCmd::AppDocuments(command)) {
        Ok(())
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ))
    }
}

async fn await_app_document_response<T>(
    response: oneshot::Receiver<Result<T, String>>,
    operation: &str,
) -> Result<T, (StatusCode, String)> {
    tokio::time::timeout(APP_DOCUMENT_REQUEST_TIMEOUT, response)
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
        })?
        .map_err(|error| {
            let status = if is_app_document_transfer_cancelled(&error)
                || error.contains("already exists")
                || error.contains("changed during recursive deletion")
            {
                StatusCode::CONFLICT
            } else if error.contains("too many entries")
                || error.contains("exceeds the maximum nesting depth")
                || error.contains("preview file exceeds")
            {
                StatusCode::PAYLOAD_TOO_LARGE
            } else if error.starts_with("invalid ")
                || error.contains("root cannot be modified")
                || error.contains("must be a regular file")
                || error.contains("only regular application")
                || error.contains("destination")
                || error.contains("import source")
                || error.contains("symbolic link")
                || error.contains("unsupported")
                || error.contains("cannot traverse symbolic links")
                || error.contains("non-directory component")
            {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, error)
        })
}

#[derive(Deserialize)]
struct DeviceFileQuery {
    #[serde(default = "storage_root")]
    path: String,
}

#[derive(Deserialize)]
struct ExportDeviceFileRequest {
    path: String,
    destination: PathBuf,
}

#[derive(Deserialize)]
struct ImportDeviceFileRequest {
    directory: String,
    source: PathBuf,
}

#[derive(Deserialize)]
struct CreateDeviceFileDirectoryRequest {
    directory: String,
    name: String,
}

#[derive(Deserialize)]
struct RenameDeviceFileRequest {
    path: String,
    name: String,
}

async fn device_files(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Query(query): Query<DeviceFileQuery>,
) -> Result<Json<DeviceFileList>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    dispatch_device_file_command(
        &state.input(&session),
        DeviceFileCommand::List {
            path: query.path,
            reply,
        },
    )?;
    Ok(Json(
        await_device_file_response(response, "device file listing").await?,
    ))
}

async fn preview_device_file(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Query(query): Query<DeviceFileQuery>,
) -> Result<Response, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    dispatch_device_file_command(
        &state.input(&session),
        DeviceFileCommand::Preview {
            path: query.path,
            reply,
        },
    )?;
    let bytes = await_device_file_response(response, "device file preview").await?;
    preview_response(bytes)
}

async fn device_file_activity(
    State(state): State<StorageHttpState>,
    session: RequestSession,
) -> Json<DeviceFileActivityView> {
    Json(state.device_file_activity(&session).get())
}

async fn cancel_device_file_activity(
    State(state): State<StorageHttpState>,
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    if state.device_file_activity(&session).cancel() {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err((
            StatusCode::CONFLICT,
            "no device file transfer is running".into(),
        ))
    }
}

async fn export_device_file(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Json(request): Json<ExportDeviceFileRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    dispatch_device_file_command(
        &state.input(&session),
        DeviceFileCommand::Export {
            path: request.path,
            destination: request.destination,
            reply,
        },
    )?;
    let transfer = await_device_file_response(response, "device file export").await?;
    Ok(Json(json!({
        "bytes_written": transfer.bytes_transferred,
        "files_written": transfer.files_transferred,
        "directories_written": transfer.directories_transferred,
    })))
}

async fn import_device_file(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Json(request): Json<ImportDeviceFileRequest>,
) -> Result<Json<DeviceFileEntry>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    dispatch_device_file_command(
        &state.input(&session),
        DeviceFileCommand::Import {
            directory: request.directory,
            source: request.source,
            reply,
        },
    )?;
    Ok(Json(
        await_device_file_response(response, "device file import").await?,
    ))
}

#[derive(Deserialize)]
struct BrowserDeviceImportQuery {
    directory: String,
    name: String,
}

#[derive(Deserialize)]
struct BrowserDeviceExportQuery {
    path: String,
    name: String,
}

async fn browser_import_device_file(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Query(query): Query<BrowserDeviceImportQuery>,
    bytes: Bytes,
) -> Result<Json<DeviceFileEntry>, (StatusCode, String)> {
    validate_browser_file_name(&query.name)?;
    let store = browser_transfer_store(&state)?;
    let source = store
        .stage_upload(query.name, bytes)
        .await
        .map_err(browser_transfer_error)?;
    let cleanup = source.clone();
    let (reply, response) = oneshot::channel();
    let result = dispatch_device_file_command(
        &state.input(&session),
        DeviceFileCommand::Import {
            directory: query.directory,
            source,
            reply,
        },
    )
    .map(|_| response);
    let result = match result {
        Ok(response) => await_device_file_response(response, "browser device file upload").await,
        Err(error) => Err(error),
    };
    let _ = store.remove(cleanup).await;
    result.map(Json)
}

async fn browser_export_device_file(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Query(query): Query<BrowserDeviceExportQuery>,
) -> Result<Response, (StatusCode, String)> {
    validate_browser_file_name(&query.name)?;
    let store = browser_transfer_store(&state)?;
    let destination = store
        .prepare_download(query.name)
        .await
        .map_err(browser_transfer_error)?;
    let cleanup = destination.clone();
    let (reply, response) = oneshot::channel();
    if let Err(error) = dispatch_device_file_command(
        &state.input(&session),
        DeviceFileCommand::Export {
            path: query.path,
            destination,
            reply,
        },
    ) {
        let _ = store.remove(cleanup).await;
        return Err(error);
    }
    if let Err(error) = await_device_file_response(response, "browser device file download").await {
        let _ = store.remove(cleanup).await;
        return Err(error);
    }
    let bytes = store
        .read_and_remove(cleanup)
        .await
        .map_err(browser_transfer_error)?;
    Ok(binary_download(bytes))
}

fn validate_browser_file_name(name: &str) -> Result<(), (StatusCode, String)> {
    validate_file_name(name).map_err(|error| (StatusCode::BAD_REQUEST, error))
}

fn browser_transfer_store(
    state: &StorageHttpState,
) -> Result<Arc<dyn BrowserTransferStore>, (StatusCode, String)> {
    state.browser_transfers.clone().ok_or((
        StatusCode::NOT_IMPLEMENTED,
        "browser file transfer is unavailable in this host".into(),
    ))
}

fn browser_transfer_error(error: String) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error)
}

async fn create_device_file_directory(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Json(request): Json<CreateDeviceFileDirectoryRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    dispatch_device_file_command(
        &state.input(&session),
        DeviceFileCommand::CreateDirectory {
            directory: request.directory,
            name: request.name,
            reply,
        },
    )?;
    await_device_file_response(response, "device directory creation").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rename_device_file(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Json(request): Json<RenameDeviceFileRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    dispatch_device_file_command(
        &state.input(&session),
        DeviceFileCommand::Rename {
            path: request.path,
            name: request.name,
            reply,
        },
    )?;
    await_device_file_response(response, "device file rename").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_device_file(
    State(state): State<StorageHttpState>,
    session: RequestSession,
    Query(query): Query<DeviceFileQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    dispatch_device_file_command(
        &state.input(&session),
        DeviceFileCommand::Delete {
            path: query.path,
            reply,
        },
    )?;
    await_device_file_response(response, "device file deletion").await?;
    Ok(StatusCode::NO_CONTENT)
}

fn dispatch_device_file_command(
    input: &InputSink,
    command: DeviceFileCommand,
) -> Result<(), (StatusCode, String)> {
    if input.try_send(InputCmd::DeviceFiles(command)) {
        Ok(())
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ))
    }
}

async fn await_device_file_response<T>(
    response: oneshot::Receiver<Result<T, String>>,
    operation: &str,
) -> Result<T, (StatusCode, String)> {
    tokio::time::timeout(DEVICE_FILE_REQUEST_TIMEOUT, response)
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
        })?
        .map_err(|error| {
            let status =
                if is_device_file_transfer_cancelled(&error) || error.contains("already exists") {
                    StatusCode::CONFLICT
                } else if error.contains("preview file exceeds") {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else if error.starts_with("invalid ")
                    || error.contains("cannot be exported")
                    || error.contains("cannot be modified")
                    || error.contains("only regular device files")
                    || error.contains("destination")
                    || error.contains("import source")
                    || error.contains("symbolic link")
                    || error.contains("unsupported")
                {
                    StatusCode::BAD_REQUEST
                } else {
                    StatusCode::BAD_GATEWAY
                };
            (status, error)
        })
}

fn preview_response(bytes: Vec<u8>) -> Result<Response, (StatusCode, String)> {
    if bytes.len() > MAX_PREVIEW_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "preview file exceeds the 64 MiB limit".into(),
        ));
    }
    let format = image::guess_format(&bytes).map_err(|_| {
        (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "preview supports PNG, JPEG, WebP, GIF, and BMP files".into(),
        )
    })?;
    let content_type = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Gif => "image/gif",
        ImageFormat::Bmp => "image/bmp",
        _ => {
            return Err((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "preview supports PNG, JPEG, WebP, GIF, and BMP files".into(),
            ));
        }
    };
    let reader = ImageReader::with_format(Cursor::new(&bytes), format);
    let (width, height) = reader.into_dimensions().map_err(|_| {
        (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the file is not a readable image".into(),
        )
    })?;
    if width == 0
        || height == 0
        || width > MAX_PREVIEW_DIMENSION
        || height > MAX_PREVIEW_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PREVIEW_PIXELS
    {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "preview image dimensions exceed the supported limit".into(),
        ));
    }

    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_DISPOSITION, "inline"),
            (header::CACHE_CONTROL, "no-store"),
            (
                header::HeaderName::from_static("x-content-type-options"),
                "nosniff",
            ),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn test_state() -> (StorageHttpState, UnboundedReceiver<InputCmd>) {
        let input = InputSink::default();
        let (sender, receiver) = unbounded_channel();
        input.set(Some(sender));
        (
            StorageHttpState::new(
                input,
                AppDocumentActivitySlot::default(),
                DeviceFileActivitySlot::default(),
            ),
            receiver,
        )
    }

    fn encoded_preview_image(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(width, height)
            .write_to(&mut output, format)
            .unwrap();
        output.into_inner()
    }

    #[test]
    fn preview_response_accepts_supported_image_formats() {
        for (format, content_type) in [
            (ImageFormat::Png, "image/png"),
            (ImageFormat::Jpeg, "image/jpeg"),
            (ImageFormat::WebP, "image/webp"),
            (ImageFormat::Gif, "image/gif"),
            (ImageFormat::Bmp, "image/bmp"),
        ] {
            let response = preview_response(encoded_preview_image(format, 2, 3)).unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .unwrap()
                    .to_str()
                    .unwrap(),
                content_type
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_DISPOSITION)
                    .unwrap()
                    .to_str()
                    .unwrap(),
                "inline"
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .unwrap()
                    .to_str()
                    .unwrap(),
                "no-store"
            );
            assert_eq!(
                response
                    .headers()
                    .get("x-content-type-options")
                    .unwrap()
                    .to_str()
                    .unwrap(),
                "nosniff"
            );
        }
    }

    #[test]
    fn preview_response_rejects_invalid_and_oversized_images() {
        assert_eq!(
            preview_response(b"not an image".to_vec()).unwrap_err().0,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(
            preview_response(encoded_preview_image(
                ImageFormat::Png,
                MAX_PREVIEW_DIMENSION + 1,
                1,
            ))
            .unwrap_err()
            .0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            preview_response(vec![0; MAX_PREVIEW_BYTES + 1])
                .unwrap_err()
                .0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[tokio::test]
    async fn preview_endpoints_dispatch_typed_commands_and_return_images() {
        use devicehub_core::AppStorageScope;

        let (state, mut input_rx) = test_state();
        let app = tokio::spawn(preview_app_document(
            State(state.clone()),
            None,
            Path("com.example.game".into()),
            Query(AppDocumentQuery {
                path: "/Images/photo.png".into(),
                scope: AppStorageScope::Container,
                recursive: false,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::Preview {
                bundle_id,
                scope,
                path,
                reply,
            }) => {
                assert_eq!(bundle_id, "com.example.game");
                assert_eq!(scope, AppStorageScope::Container);
                assert_eq!(path, "/Images/photo.png");
                reply
                    .send(Ok(encoded_preview_image(ImageFormat::Png, 2, 3)))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let app_response = app.await.unwrap().unwrap();
        assert_eq!(app_response.status(), StatusCode::OK);
        assert_eq!(
            app_response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "image/png"
        );

        let device = tokio::spawn(preview_device_file(
            State(state),
            None,
            Query(DeviceFileQuery {
                path: "/DCIM/photo.webp".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceFiles(DeviceFileCommand::Preview { path, reply }) => {
                assert_eq!(path, "/DCIM/photo.webp");
                reply
                    .send(Ok(encoded_preview_image(ImageFormat::WebP, 3, 2)))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let device_response = device.await.unwrap().unwrap();
        assert_eq!(device_response.status(), StatusCode::OK);
        assert_eq!(
            device_response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "image/webp"
        );
    }

    #[tokio::test]
    async fn storage_queries_require_an_active_session() {
        let (state, _) = test_state();
        state.input.set(None);
        assert!(matches!(
            device_files(
                State(state.clone()),
                None,
                Query(DeviceFileQuery { path: "/".into() }),
            )
            .await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            app_documents(
                State(state),
                None,
                Path("com.example.game".into()),
                Query(AppDocumentQuery {
                    path: "/".into(),
                    scope: AppStorageScope::Documents,
                    recursive: false,
                }),
            )
            .await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }

    #[tokio::test]
    async fn app_storage_endpoints_dispatch_scoped_commands() {
        use devicehub_core::{
            AppDocumentEntry, AppDocumentKind, AppDocumentList, AppDocumentTransfer,
            AppStorageScope,
        };

        let (cancel_state, _) = test_state();
        assert_eq!(
            cancel_app_document_activity(
                State(cancel_state.clone()),
                None,
                Path("com.example.game".into()),
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        let (state, mut input_rx) = test_state();
        let activity =
            app_document_activity(State(state.clone()), None, Path("com.example.game".into()))
                .await
                .unwrap()
                .0;
        assert_eq!(
            activity.state,
            devicehub_core::AppDocumentActivityState::Idle
        );
        assert!(
            app_document_activity(State(state.clone()), None, Path("invalid".into()))
                .await
                .is_err()
        );
        let list = tokio::spawn(app_documents(
            State(state.clone()),
            None,
            Path("com.example.game".into()),
            Query(AppDocumentQuery {
                path: "/Saves".into(),
                scope: AppStorageScope::Container,
                recursive: false,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::List {
                bundle_id,
                scope,
                path,
                reply,
            }) => {
                assert_eq!(bundle_id, "com.example.game");
                assert_eq!(scope, AppStorageScope::Container);
                assert_eq!(path, "/Saves");
                reply
                    .send(Ok(AppDocumentList {
                        path,
                        entries: Vec::new(),
                        truncated: false,
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(list.await.unwrap().unwrap().0.path, "/Saves");

        let upload = tokio::spawn(import_app_document(
            State(state.clone()),
            None,
            Path("com.example.game".into()),
            Json(ImportAppDocumentRequest {
                directory: "/Saves".into(),
                source: PathBuf::from("slot.dat"),
                scope: AppStorageScope::Documents,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::Import {
                directory,
                source,
                reply,
                ..
            }) => {
                assert_eq!(directory, "/Saves");
                assert_eq!(source, PathBuf::from("slot.dat"));
                reply
                    .send(Ok(AppDocumentEntry {
                        name: "slot.dat".into(),
                        path: "/Saves/slot.dat".into(),
                        kind: AppDocumentKind::File,
                        size_bytes: 42,
                        modified: "2026-07-24T00:00:00Z".into(),
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(upload.await.unwrap().unwrap().0.size_bytes, 42);

        let create = tokio::spawn(create_app_document_directory(
            State(state.clone()),
            None,
            Path("com.example.game".into()),
            Json(CreateAppDocumentDirectoryRequest {
                directory: "/".into(),
                name: "Saves".into(),
                scope: AppStorageScope::Documents,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::CreateDirectory { name, reply, .. }) => {
                assert_eq!(name, "Saves");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(create.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let rename = tokio::spawn(rename_app_document(
            State(state.clone()),
            None,
            Path("com.example.game".into()),
            Json(RenameAppDocumentRequest {
                path: "/Saves/slot.dat".into(),
                name: "slot-2.dat".into(),
                scope: AppStorageScope::Documents,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::Rename { name, reply, .. }) => {
                assert_eq!(name, "slot-2.dat");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(rename.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let delete = tokio::spawn(delete_app_document(
            State(state.clone()),
            None,
            Path("com.example.game".into()),
            Query(AppDocumentQuery {
                path: "/Saves/slot-2.dat".into(),
                scope: AppStorageScope::Documents,
                recursive: true,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::Delete {
                path,
                recursive,
                reply,
                ..
            }) => {
                assert_eq!(path, "/Saves/slot-2.dat");
                assert!(recursive);
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(delete.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let export = tokio::spawn(export_app_document(
            State(state),
            None,
            Path("com.example.game".into()),
            Json(ExportAppDocumentRequest {
                path: "/Saves/slot-2.dat".into(),
                destination: PathBuf::from("slot-2.dat"),
                scope: AppStorageScope::Documents,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::Export {
                destination, reply, ..
            }) => {
                assert_eq!(destination, PathBuf::from("slot-2.dat"));
                reply
                    .send(Ok(AppDocumentTransfer {
                        bytes_transferred: 84,
                        files_transferred: 2,
                        directories_transferred: 1,
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let export = export.await.unwrap().unwrap().0;
        assert_eq!(export["bytes_written"], 84);
        assert_eq!(export["files_written"], 2);
        assert_eq!(export["directories_written"], 1);
    }

    #[tokio::test]
    async fn device_file_endpoints_dispatch_typed_commands() {
        use devicehub_core::{DeviceFileEntry, DeviceFileKind, DeviceFileList, DeviceFileTransfer};

        let (cancel_state, _) = test_state();
        assert_eq!(
            cancel_device_file_activity(State(cancel_state.clone()), None)
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        let (state, mut input_rx) = test_state();
        assert_eq!(
            device_file_activity(State(state.clone()), None)
                .await
                .0
                .state,
            devicehub_core::DeviceFileActivityState::Idle
        );
        let list = tokio::spawn(device_files(
            State(state.clone()),
            None,
            Query(DeviceFileQuery {
                path: "/DCIM".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceFiles(DeviceFileCommand::List { path, reply }) => {
                assert_eq!(path, "/DCIM");
                reply
                    .send(Ok(DeviceFileList {
                        path,
                        entries: Vec::new(),
                        truncated: false,
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(list.await.unwrap().unwrap().0.path, "/DCIM");

        let export = tokio::spawn(export_device_file(
            State(state.clone()),
            None,
            Json(ExportDeviceFileRequest {
                path: "/DCIM/100APPLE/IMG_0001.HEIC".into(),
                destination: std::env::temp_dir().join("photo.heic"),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceFiles(DeviceFileCommand::Export {
                path,
                destination,
                reply,
            }) => {
                assert_eq!(path, "/DCIM/100APPLE/IMG_0001.HEIC");
                assert_eq!(destination, std::env::temp_dir().join("photo.heic"));
                reply
                    .send(Ok(DeviceFileTransfer {
                        bytes_transferred: 42,
                        files_transferred: 1,
                        directories_transferred: 0,
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(
            export.await.unwrap().unwrap().0,
            json!({ "bytes_written": 42, "files_written": 1, "directories_written": 0 })
        );

        let import = tokio::spawn(import_device_file(
            State(state.clone()),
            None,
            Json(ImportDeviceFileRequest {
                directory: "/Downloads".into(),
                source: PathBuf::from("archive.zip"),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceFiles(DeviceFileCommand::Import {
                directory,
                source,
                reply,
            }) => {
                assert_eq!(directory, "/Downloads");
                assert_eq!(source, PathBuf::from("archive.zip"));
                reply
                    .send(Ok(DeviceFileEntry {
                        name: "archive.zip".into(),
                        path: "/Downloads/archive.zip".into(),
                        kind: DeviceFileKind::File,
                        size_bytes: 42,
                        modified: "2026-07-24T00:00:00Z".into(),
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(import.await.unwrap().unwrap().0.size_bytes, 42);

        let create = tokio::spawn(create_device_file_directory(
            State(state.clone()),
            None,
            Json(CreateDeviceFileDirectoryRequest {
                directory: "/".into(),
                name: "Shared".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceFiles(DeviceFileCommand::CreateDirectory {
                directory,
                name,
                reply,
            }) => {
                assert_eq!(directory, "/");
                assert_eq!(name, "Shared");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(create.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let rename = tokio::spawn(rename_device_file(
            State(state.clone()),
            None,
            Json(RenameDeviceFileRequest {
                path: "/Downloads/archive.zip".into(),
                name: "backup.zip".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceFiles(DeviceFileCommand::Rename { path, name, reply }) => {
                assert_eq!(path, "/Downloads/archive.zip");
                assert_eq!(name, "backup.zip");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(rename.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let delete = tokio::spawn(delete_device_file(
            State(state),
            None,
            Query(DeviceFileQuery {
                path: "/Downloads/backup.zip".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceFiles(DeviceFileCommand::Delete { path, reply }) => {
                assert_eq!(path, "/Downloads/backup.zip");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(delete.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn app_document_conflicts_are_reported_as_http_conflicts() {
        for error in [
            "an application document with this name already exists",
            "directory export destination already exists",
            "application entry changed during recursive deletion",
            devicehub_core::APP_DOCUMENT_TRANSFER_CANCELLED,
        ] {
            let (reply, response) = oneshot::channel::<Result<(), String>>();
            reply.send(Err(error.into())).unwrap();
            assert!(matches!(
                await_app_document_response(response, "transfer").await,
                Err((StatusCode::CONFLICT, _))
            ));
        }
    }

    #[tokio::test]
    async fn app_document_recursive_limits_are_reported_as_payload_too_large() {
        for error in [
            "application directory deletion contains too many entries",
            "application directory deletion exceeds the maximum nesting depth",
        ] {
            let (reply, response) = oneshot::channel::<Result<(), String>>();
            reply.send(Err(error.into())).unwrap();
            assert!(matches!(
                await_app_document_response(response, "recursive delete").await,
                Err((StatusCode::PAYLOAD_TOO_LARGE, _))
            ));
        }
    }
}
