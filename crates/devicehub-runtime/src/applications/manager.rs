use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use devicehub_core::{
    AppOperationKind, AppOperationSlot, AppSigningKind, DeviceApp,
    process_executable_belongs_to_app,
};

use super::AppCommand;
use idevice::{
    IdeviceService, ReadWrite, RsdService,
    core_device::AppServiceClient,
    dvt::{process_control::ProcessControlClient, remote_server::RemoteServerClient},
    installation_proxy::InstallationProxyClient,
    provider::IdeviceProvider,
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
};

const APP_SERVICE_LIST_TIMEOUT: Duration = Duration::from_secs(6);
const APP_METADATA_TIMEOUT: Duration = Duration::from_secs(4);
const APP_CLIENT_RECONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const APP_DVT_CHANNEL_TIMEOUT: Duration = Duration::from_secs(2);
const APP_CONTROL_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);
pub const APP_LIST_REQUEST_TIMEOUT: Duration = Duration::from_secs(24);
pub const APP_CONTROL_REQUEST_TIMEOUT: Duration = Duration::from_secs(22);

#[derive(Clone, Default)]
struct AppControlSlot(Arc<AtomicBool>);

impl AppControlSlot {
    fn try_start(&self) -> Result<AppControlLease, String> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| AppControlLease(self.clone()))
            .map_err(|_| "another app control command is already running".into())
    }
}

struct AppControlLease(AppControlSlot);

impl Drop for AppControlLease {
    fn drop(&mut self) {
        self.0.0.store(false, Ordering::Release);
    }
}

pub struct AppServiceTransport {
    adapter: AdapterHandle,
    handshake: RsdHandshake,
}

impl AppServiceTransport {
    pub fn new(adapter: AdapterHandle, handshake: RsdHandshake) -> Self {
        Self { adapter, handshake }
    }
}

pub struct AppClientSet {
    app_service: Option<AppServiceClient<Box<dyn ReadWrite>>>,
    installation_proxy: Option<InstallationProxyClient>,
}

impl AppClientSet {
    pub async fn connect_installation_proxy(provider: &dyn IdeviceProvider) -> Self {
        let installation_proxy = match InstallationProxyClient::connect(provider).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::warn!(
                    "installation proxy unavailable; app list fallback disabled: {error:?}"
                );
                None
            }
        };
        Self {
            app_service: None,
            installation_proxy,
        }
    }

    pub async fn connect_app_service(
        &mut self,
        adapter: &mut AdapterHandle,
        handshake: &mut RsdHandshake,
    ) {
        self.app_service = match AppServiceClient::connect_rsd(adapter, handshake).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::warn!("no CoreDevice AppService; app management disabled: {error:?}");
                None
            }
        };
    }
}

struct ActiveAppOperation {
    id: u64,
    handle: tokio::task::JoinHandle<()>,
}

fn cancel_active_operation(operation: &AppOperationSlot, task: &mut Option<ActiveAppOperation>) {
    if let Some(active) = task.take() {
        if !active.handle.is_finished() {
            active.handle.abort();
        }
        operation.cancel(active.id);
    }
}

/// Owns every long-lived app client and all app-operation task state for one device session.
pub struct AppManagement {
    provider: Arc<dyn IdeviceProvider>,
    control: AppControlSlot,
    operation: AppOperationSlot,
    operation_task: Option<ActiveAppOperation>,
    app_service: Option<AppServiceClient<Box<dyn ReadWrite>>>,
    installation_proxy: Option<InstallationProxyClient>,
    transport: AppServiceTransport,
}

impl Drop for AppManagement {
    fn drop(&mut self) {
        cancel_active_operation(&self.operation, &mut self.operation_task);
    }
}

impl AppManagement {
    pub fn new(
        provider: Arc<dyn IdeviceProvider>,
        operation: AppOperationSlot,
        clients: AppClientSet,
        transport: AppServiceTransport,
    ) -> Self {
        let AppClientSet {
            app_service,
            installation_proxy,
        } = clients;
        Self {
            provider,
            control: AppControlSlot::default(),
            operation,
            operation_task: None,
            app_service,
            installation_proxy,
            transport,
        }
    }

    pub async fn handle(&mut self, command: AppCommand) {
        match command {
            AppCommand::List {
                include_system,
                include_app_clips,
                reply,
            } => {
                self.list_apps(include_system, include_app_clips, reply)
                    .await;
            }
            AppCommand::Launch { bundle_id, reply } => match self.control.try_start() {
                Ok(lease) => {
                    self.app_service.take();
                    let adapter = self.transport.adapter.clone();
                    let handshake = self.transport.handshake.clone();
                    tokio::task::spawn_local(async move {
                        let _lease = lease;
                        let _ = reply.send(launch_device_app(adapter, handshake, bundle_id).await);
                    });
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
            },
            AppCommand::Stop { bundle_id, reply } => match self.control.try_start() {
                Ok(lease) => {
                    self.app_service.take();
                    let adapter = self.transport.adapter.clone();
                    let handshake = self.transport.handshake.clone();
                    tokio::task::spawn_local(async move {
                        let _lease = lease;
                        let _ = reply
                            .send(stop_device_app_isolated(adapter, handshake, bundle_id).await);
                    });
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
            },
            AppCommand::Uninstall { bundle_id, reply } => {
                let result = self.uninstall_app(bundle_id);
                let _ = reply.send(result);
            }
        }
    }

    async fn list_apps(
        &mut self,
        include_system: bool,
        include_app_clips: bool,
        reply: tokio::sync::oneshot::Sender<Result<Vec<DeviceApp>, String>>,
    ) {
        if reply.is_closed() {
            tracing::debug!("discarding cancelled app list request");
            return;
        }
        let started = Instant::now();
        let mut recovered = false;
        let extended_scope = extended_app_scope(include_system, include_app_clips);
        let first = if extended_scope.is_none() {
            list_user_apps_via_installation_proxy(self.installation_proxy.as_mut()).await
        } else {
            list_device_apps(
                self.app_service.as_mut(),
                self.installation_proxy.as_mut(),
                include_system,
                include_app_clips,
                false,
            )
            .await
        };
        let result = match first {
            Ok(apps) => Ok(apps),
            Err(first_error) => {
                if reply.is_closed() {
                    tracing::debug!("app list caller disconnected before recovery");
                    return;
                }
                tracing::warn!(
                    error = %first_error,
                    extended_scope,
                    "app listing failed; reconnecting the required read-only service once"
                );
                let reconnect = if extended_scope.is_none() {
                    self.reconnect_installation_proxy().await
                } else {
                    self.reconnect_app_clients().await
                };
                match reconnect {
                    Ok(()) => {
                        recovered = true;
                        let retry = if extended_scope.is_none() {
                            list_user_apps_via_installation_proxy(self.installation_proxy.as_mut())
                                .await
                        } else {
                            list_device_apps(
                                self.app_service.as_mut(),
                                self.installation_proxy.as_mut(),
                                include_system,
                                include_app_clips,
                                true,
                            )
                            .await
                        };
                        retry.map_err(|retry_error| {
                            format!("{retry_error} (initial app listing failure: {first_error})")
                        })
                    }
                    Err(reconnect_error) => Err(format!("{first_error}; {reconnect_error}")),
                }
            }
        };
        match &result {
            Ok(apps) => tracing::debug!(
                count = apps.len(),
                recovered,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "application list completed"
            ),
            Err(error) => tracing::warn!(
                %error,
                recovered,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "application list failed"
            ),
        }
        let _ = reply.send(result);
    }

    async fn reconnect_app_clients(&mut self) -> Result<(), String> {
        self.app_service.take();
        self.installation_proxy.take();
        let mut adapter = self.transport.adapter.clone();
        let mut handshake = self.transport.handshake.clone();
        let provider = self.provider.clone();
        let (app_service, installation_proxy) = tokio::join!(
            tokio::time::timeout(
                APP_CLIENT_RECONNECT_TIMEOUT,
                AppServiceClient::connect_rsd(&mut adapter, &mut handshake),
            ),
            tokio::time::timeout(
                APP_CLIENT_RECONNECT_TIMEOUT,
                InstallationProxyClient::connect(&*provider),
            ),
        );
        let mut errors = Vec::new();
        match app_service {
            Ok(Ok(client)) => self.app_service = Some(client),
            Ok(Err(error)) => errors.push(format!("CoreDevice AppService: {error:?}")),
            Err(_) => errors.push("CoreDevice AppService connection timed out".into()),
        }
        match installation_proxy {
            Ok(Ok(client)) => self.installation_proxy = Some(client),
            Ok(Err(error)) => errors.push(format!("InstallationProxy: {error:?}")),
            Err(_) => errors.push("InstallationProxy connection timed out".into()),
        }
        if self.app_service.is_some() || self.installation_proxy.is_some() {
            if !errors.is_empty() {
                tracing::debug!(errors = ?errors, "some app listing services remain unavailable after reconnect");
            }
            Ok(())
        } else {
            Err(format!(
                "unable to reconnect app listing services: {}",
                errors.join("; ")
            ))
        }
    }

    async fn reconnect_installation_proxy(&mut self) -> Result<(), String> {
        self.installation_proxy.take();
        let connection = tokio::time::timeout(
            APP_CLIENT_RECONNECT_TIMEOUT,
            InstallationProxyClient::connect(&*self.provider),
        )
        .await;
        self.installation_proxy = match connection {
            Ok(Ok(client)) => Some(client),
            Ok(Err(error)) => {
                return Err(format!("unable to reconnect InstallationProxy: {error:?}"));
            }
            Err(_) => return Err("InstallationProxy connection timed out".into()),
        };
        Ok(())
    }

    fn clear_finished_operation(&mut self) {
        if self
            .operation_task
            .as_ref()
            .is_some_and(|operation| operation.handle.is_finished())
            && let Some(operation) = self.operation_task.take()
        {
            self.operation
                .fail(operation.id, "app operation ended unexpectedly".into());
        }
    }

    fn uninstall_app(&mut self, bundle_id: String) -> Result<(), String> {
        self.clear_finished_operation();
        let id = self
            .operation
            .start(AppOperationKind::Uninstall, bundle_id.clone())?;
        self.operation.update(id, "verifying", None);
        let provider = self.provider.clone();
        let operation = self.operation.clone();
        let task_operation = operation.clone();
        let handle = tokio::spawn(async move {
            let result =
                uninstall_user_app(provider.as_ref(), &bundle_id, task_operation.clone(), id).await;
            match result {
                Ok(()) => operation.succeed(id),
                Err(error) => operation.fail(id, error),
            }
        });
        self.operation_task = Some(ActiveAppOperation { id, handle });
        Ok(())
    }
}

pub(super) async fn uninstall_user_app(
    provider: &dyn IdeviceProvider,
    bundle_id: &str,
    operation: AppOperationSlot,
    operation_id: u64,
) -> Result<(), String> {
    let mut client = InstallationProxyClient::connect(provider)
        .await
        .map_err(|error| format!("installation proxy is unavailable: {error:?}"))?;
    let mut matches = client
        .get_apps(Some("User"), Some(vec![bundle_id.to_string()]))
        .await
        .map_err(|error| format!("unable to verify app: {error:?}"))?;
    let value = matches
        .remove(bundle_id)
        .ok_or_else(|| "app is not installed as a user application".to_string())?;
    let app = device_app_from_installation(bundle_id.to_string(), &value)
        .ok_or_else(|| "device returned invalid app metadata".to_string())?;
    if !app.is_removable || app.is_first_party {
        return Err("the selected app is not a removable third-party application".into());
    }

    operation.update(operation_id, "uninstalling", Some(0));
    client
        .uninstall_with_callback(
            bundle_id,
            None,
            |(progress, (operation, id))| async move {
                operation.update(id, "uninstalling", Some(progress.min(100) as u8));
            },
            (operation, operation_id),
        )
        .await
        .map_err(|error| format!("unable to uninstall app: {error:?}"))
}

pub(super) async fn list_device_apps(
    app_service: Option<&mut AppServiceClient<Box<dyn ReadWrite>>>,
    mut installation_proxy: Option<&mut InstallationProxyClient>,
    include_system: bool,
    include_app_clips: bool,
    allow_fallback_after_app_service_error: bool,
) -> Result<Vec<DeviceApp>, String> {
    if let Some(client) = app_service {
        let app_service_result = tokio::time::timeout(
            APP_SERVICE_LIST_TIMEOUT,
            client.list_apps(include_app_clips, true, false, false, include_system),
        )
        .await
        .map_err(|_| {
            format!(
                "CoreDevice AppService list timed out after {} seconds",
                APP_SERVICE_LIST_TIMEOUT.as_secs()
            )
        })
        .and_then(|result| {
            result.map_err(|error| format!("CoreDevice AppService list failed: {error:?}"))
        });
        match app_service_result {
            Ok(entries) => {
                let application_type = if include_system { "Any" } else { "User" };
                let bundle_identifiers = entries
                    .iter()
                    .map(|entry| entry.bundle_identifier.clone())
                    .collect();
                let metadata = async {
                    if entries.is_empty() {
                        return std::collections::HashMap::new();
                    }
                    match installation_proxy.as_deref_mut() {
                        Some(client) => match tokio::time::timeout(
                            APP_METADATA_TIMEOUT,
                            client.get_apps(Some(application_type), Some(bundle_identifiers)),
                        )
                        .await
                        {
                            Ok(Ok(apps)) => apps,
                            Ok(Err(error)) => {
                                tracing::warn!(
                                    "installation proxy app metadata unavailable: {error:?}"
                                );
                                std::collections::HashMap::new()
                            }
                            Err(_) => {
                                tracing::warn!(
                                    timeout_ms = APP_METADATA_TIMEOUT.as_millis() as u64,
                                    "installation proxy app metadata timed out"
                                );
                                std::collections::HashMap::new()
                            }
                        },
                        None => std::collections::HashMap::new(),
                    }
                };
                let process_list = async {
                    match tokio::time::timeout(APP_METADATA_TIMEOUT, client.list_processes()).await
                    {
                        Ok(Ok(processes)) => Some(processes),
                        Ok(Err(error)) => {
                            tracing::warn!("CoreDevice process list unavailable: {error:?}");
                            None
                        }
                        Err(_) => {
                            tracing::warn!(
                                timeout_ms = APP_METADATA_TIMEOUT.as_millis() as u64,
                                "CoreDevice process list timed out"
                            );
                            None
                        }
                    }
                };
                let (installation_apps, processes) = tokio::join!(metadata, process_list);
                if installation_apps.is_empty() && !entries.is_empty() {
                    tracing::debug!(
                        "application list is returning without InstallationProxy metadata"
                    );
                }
                if processes.is_none() {
                    tracing::debug!("application list is returning without running state");
                }
                return Ok(sort_device_apps(
                    entries
                        .into_iter()
                        .map(|entry| {
                            let metadata = installation_apps.get(&entry.bundle_identifier);
                            let documents_available =
                                metadata.is_some_and(installation_supports_documents);
                            let (
                                static_disk_usage_bytes,
                                dynamic_disk_usage_bytes,
                                total_disk_usage_bytes,
                            ) = metadata.map(app_disk_usage).unwrap_or((None, None, None));
                            let signing_kind = app_signing_kind(
                                metadata,
                                entry.is_first_party,
                                entry.is_developer_app,
                            );
                            let is_developer_app = entry.is_developer_app
                                || signing_kind == AppSigningKind::Development;
                            let minimum_os_version = metadata.and_then(app_minimum_os_version);
                            let debuggable = metadata.and_then(app_debuggable);
                            DeviceApp {
                                is_running: processes.as_ref().map(|processes| {
                                    processes.iter().any(|process| {
                                        process.executable_url.as_ref().is_some_and(|executable| {
                                            process_executable_belongs_to_app(
                                                &entry.path,
                                                &executable.relative,
                                            )
                                        })
                                    })
                                }),
                                bundle_id: entry.bundle_identifier,
                                name: entry.name,
                                version: entry.version,
                                bundle_version: entry.bundle_version,
                                is_removable: entry.is_removable,
                                is_first_party: entry.is_first_party,
                                is_developer_app,
                                is_app_clip: entry.is_app_clip,
                                signing_kind,
                                minimum_os_version,
                                debuggable,
                                documents_available,
                                static_disk_usage_bytes,
                                dynamic_disk_usage_bytes,
                                total_disk_usage_bytes,
                            }
                        })
                        .collect(),
                ));
            }
            Err(error) => {
                if !allow_fallback_after_app_service_error {
                    return Err(error);
                }
                if let Some(scope) = extended_app_scope(include_system, include_app_clips) {
                    return Err(format!(
                        "{scope} listing requires CoreDevice AppService: {error}"
                    ));
                }
                tracing::warn!(
                    "CoreDevice AppService list failed; using installation proxy: {error}"
                );
            }
        }
    }

    if let Some(scope) = extended_app_scope(include_system, include_app_clips) {
        return Err(format!(
            "{scope} listing requires CoreDevice AppService, but it is unavailable"
        ));
    }

    list_user_apps_via_installation_proxy(installation_proxy).await
}

pub(super) async fn list_user_apps_via_installation_proxy(
    installation_proxy: Option<&mut InstallationProxyClient>,
) -> Result<Vec<DeviceApp>, String> {
    let client = installation_proxy
        .ok_or_else(|| "InstallationProxy app listing service is unavailable".to_string())?;
    let entries = tokio::time::timeout(
        APP_SERVICE_LIST_TIMEOUT,
        client.get_apps(Some("User"), None),
    )
    .await
    .map_err(|_| {
        format!(
            "InstallationProxy app list timed out after {} seconds",
            APP_SERVICE_LIST_TIMEOUT.as_secs()
        )
    })?
    .map_err(|error| format!("unable to list apps: {error:?}"))?;
    Ok(sort_device_apps(
        entries
            .into_iter()
            .filter_map(|(bundle_id, value)| device_app_from_installation(bundle_id, &value))
            .collect(),
    ))
}

pub(super) fn extended_app_scope(
    include_system: bool,
    include_app_clips: bool,
) -> Option<&'static str> {
    match (include_system, include_app_clips) {
        (true, true) => Some("system app and App Clip"),
        (true, false) => Some("system app"),
        (false, true) => Some("App Clip"),
        (false, false) => None,
    }
}

pub(super) fn device_app_from_installation(
    bundle_id: String,
    value: &plist::Value,
) -> Option<DeviceApp> {
    let fields = value.as_dictionary()?;
    let string = |key: &str| {
        fields
            .get(key)
            .and_then(plist::Value::as_string)
            .map(ToOwned::to_owned)
    };
    let boolean = |key: &str| fields.get(key).and_then(plist::Value::as_boolean);
    let name = string("CFBundleDisplayName")
        .or_else(|| string("CFBundleName"))
        .unwrap_or_else(|| bundle_id.clone());
    let signer = string("SignerIdentity").unwrap_or_default();
    let is_first_party = boolean("IsFirstParty").unwrap_or(false);
    let is_developer_app = boolean("IsXcodeManaged").unwrap_or(false)
        || signer.contains("Apple Development")
        || signer.contains("iPhone Developer");
    let (static_disk_usage_bytes, dynamic_disk_usage_bytes, total_disk_usage_bytes) =
        app_disk_usage(value);
    Some(DeviceApp {
        bundle_id,
        name,
        version: string("CFBundleShortVersionString"),
        bundle_version: string("CFBundleVersion"),
        is_removable: boolean("IsRemovable").unwrap_or(false),
        is_first_party,
        is_developer_app,
        is_app_clip: false,
        signing_kind: app_signing_kind(Some(value), is_first_party, is_developer_app),
        minimum_os_version: app_minimum_os_version(value),
        debuggable: app_debuggable(value),
        documents_available: installation_supports_documents(value),
        static_disk_usage_bytes,
        dynamic_disk_usage_bytes,
        total_disk_usage_bytes,
        is_running: None,
    })
}

pub(super) fn app_signing_kind(
    value: Option<&plist::Value>,
    is_first_party: bool,
    is_developer_app: bool,
) -> AppSigningKind {
    if is_first_party {
        return AppSigningKind::System;
    }
    let fields = value.and_then(plist::Value::as_dictionary);
    let signer = fields
        .and_then(|fields| fields.get("SignerIdentity"))
        .and_then(plist::Value::as_string)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let xcode_managed = fields
        .and_then(|fields| fields.get("IsXcodeManaged"))
        .and_then(plist::Value::as_boolean)
        .unwrap_or(false);
    if is_developer_app
        || xcode_managed
        || signer.contains("development")
        || signer.contains("developer")
    {
        return AppSigningKind::Development;
    }
    let testflight = fields.is_some_and(|fields| {
        fields.contains_key("BetaExternalVersionIdentifier")
            || fields
                .get("IsBetaApp")
                .and_then(plist::Value::as_boolean)
                .unwrap_or(false)
    });
    if testflight {
        AppSigningKind::TestFlight
    } else if signer.contains("iphone os application signing") {
        AppSigningKind::AppStore
    } else if signer.contains("distribution") {
        AppSigningKind::Distribution
    } else {
        AppSigningKind::Unknown
    }
}

pub(super) fn app_minimum_os_version(value: &plist::Value) -> Option<String> {
    let version = normalized_app_metadata_text(value, "MinimumOSVersion", 32)?;
    let segments = version.split('.').collect::<Vec<_>>();
    (segments.len() <= 4
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment.len() <= 3
                && segment.bytes().all(|byte| byte.is_ascii_digit())
        }))
    .then_some(version)
}

pub(super) fn app_debuggable(value: &plist::Value) -> Option<bool> {
    value
        .as_dictionary()?
        .get("Entitlements")?
        .as_dictionary()?
        .get("get-task-allow")?
        .as_boolean()
}

fn normalized_app_metadata_text(
    value: &plist::Value,
    key: &str,
    max_chars: usize,
) -> Option<String> {
    let raw = value.as_dictionary()?.get(key)?.as_string()?;
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty() && normalized.chars().count() <= max_chars).then_some(normalized)
}

pub(super) const MAX_APP_DISK_USAGE_BYTES: u64 = 16 * 1_000_000_000_000;

pub(super) fn app_disk_usage(value: &plist::Value) -> (Option<u64>, Option<u64>, Option<u64>) {
    let Some(fields) = value.as_dictionary() else {
        return (None, None, None);
    };
    let bounded = |key: &str| {
        fields
            .get(key)
            .and_then(plist::Value::as_unsigned_integer)
            .filter(|bytes| *bytes <= MAX_APP_DISK_USAGE_BYTES)
    };
    let static_bytes = bounded("StaticDiskUsage");
    let dynamic_bytes = bounded("DynamicDiskUsage");
    let total_bytes = match (static_bytes, dynamic_bytes) {
        (Some(static_bytes), Some(dynamic_bytes)) => static_bytes.checked_add(dynamic_bytes),
        (Some(bytes), None) | (None, Some(bytes)) => Some(bytes),
        (None, None) => None,
    }
    .filter(|bytes| *bytes <= MAX_APP_DISK_USAGE_BYTES);
    (static_bytes, dynamic_bytes, total_bytes)
}

fn installation_supports_documents(value: &plist::Value) -> bool {
    value.as_dictionary().is_some_and(|fields| {
        ["UIFileSharingEnabled", "UISupportsDocumentBrowser"]
            .into_iter()
            .any(|key| {
                fields
                    .get(key)
                    .and_then(plist::Value::as_boolean)
                    .unwrap_or(false)
            })
    })
}

async fn stop_device_app(
    client: &mut AppServiceClient<Box<dyn ReadWrite>>,
    bundle_id: &str,
) -> Result<bool, String> {
    let apps = client
        .list_apps(true, true, false, false, false)
        .await
        .map_err(|error| format!("unable to resolve app before stopping it: {error:?}"))?;
    let app = apps
        .into_iter()
        .find(|app| app.bundle_identifier == bundle_id)
        .ok_or_else(|| "app is not installed or is not user-manageable".to_string())?;
    let processes = client
        .list_processes()
        .await
        .map_err(|error| format!("unable to list app processes: {error:?}"))?;
    let process_ids: Vec<_> = processes
        .into_iter()
        .filter(|process| {
            process.executable_url.as_ref().is_some_and(|executable| {
                process_executable_belongs_to_app(&app.path, &executable.relative)
            })
        })
        .map(|process| process.pid)
        .collect();
    for pid in &process_ids {
        client
            .send_signal(*pid, 15)
            .await
            .map_err(|error| format!("unable to stop app: {error:?}"))?;
    }
    Ok(!process_ids.is_empty())
}

async fn connect_app_control(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<AppServiceClient<Box<dyn ReadWrite>>, String> {
    tokio::time::timeout(
        APP_CLIENT_RECONNECT_TIMEOUT,
        AppServiceClient::connect_rsd(&mut adapter, &mut handshake),
    )
    .await
    .map_err(|_| "CoreDevice app control connection timed out".to_string())?
    .map_err(|error| format!("CoreDevice app control service unavailable: {error:?}"))
}

pub(super) async fn launch_device_app(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    bundle_id: String,
) -> Result<(), String> {
    match launch_device_app_via_dvt(adapter.clone(), handshake.clone(), &bundle_id).await {
        DvtLaunchOutcome::Attempted(result) => return result,
        DvtLaunchOutcome::Unavailable(error) => {
            tracing::warn!(%error, %bundle_id, "DVT app launch unavailable; using CoreDevice AppService");
        }
    }
    launch_device_app_via_coredevice(adapter, handshake, bundle_id).await
}

enum DvtLaunchOutcome {
    Unavailable(String),
    Attempted(Result<(), String>),
}

async fn launch_device_app_via_dvt(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    bundle_id: &str,
) -> DvtLaunchOutcome {
    let started = Instant::now();
    let mut remote = match tokio::time::timeout(
        APP_CLIENT_RECONNECT_TIMEOUT,
        RemoteServerClient::<Box<dyn ReadWrite>>::connect_rsd(&mut adapter, &mut handshake),
    )
    .await
    {
        Ok(Ok(remote)) => remote,
        Ok(Err(error)) => {
            return DvtLaunchOutcome::Unavailable(format!(
                "DVT process control connection failed: {error:?}"
            ));
        }
        Err(_) => {
            return DvtLaunchOutcome::Unavailable(
                "DVT process control connection timed out".into(),
            );
        }
    };
    let mut client = match tokio::time::timeout(
        APP_DVT_CHANNEL_TIMEOUT,
        ProcessControlClient::new(&mut remote),
    )
    .await
    {
        Ok(Ok(client)) => client,
        Ok(Err(error)) => {
            return DvtLaunchOutcome::Unavailable(format!(
                "DVT ProcessControl channel unavailable: {error:?}"
            ));
        }
        Err(_) => {
            return DvtLaunchOutcome::Unavailable(
                "DVT ProcessControl channel creation timed out".into(),
            );
        }
    };
    let result = tokio::time::timeout(
        APP_CONTROL_OPERATION_TIMEOUT,
        client.launch_app(bundle_id, None, None, false, true),
    )
    .await
    .map_err(|_| {
        format!(
            "DVT app launch timed out after {} seconds",
            APP_CONTROL_OPERATION_TIMEOUT.as_secs()
        )
    })
    .and_then(|result| {
        result
            .map(|_| ())
            .map_err(|error| format!("unable to launch app through DVT: {error:?}"))
    });
    tracing::info!(
        %bundle_id,
        backend = "dvt-process-control",
        success = result.is_ok(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "app launch completed"
    );
    DvtLaunchOutcome::Attempted(result)
}

async fn launch_device_app_via_coredevice(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    bundle_id: String,
) -> Result<(), String> {
    let started = Instant::now();
    let mut client = connect_app_control(adapter, handshake).await?;
    let result = tokio::time::timeout(
        APP_CONTROL_OPERATION_TIMEOUT,
        client.launch_application(&bundle_id, &[], true, false, None, None, None),
    )
    .await
    .map_err(|_| {
        format!(
            "CoreDevice app launch timed out after {} seconds",
            APP_CONTROL_OPERATION_TIMEOUT.as_secs()
        )
    })?
    .map(|_| ())
    .map_err(|error| format!("unable to launch app: {error:?}"));
    tracing::debug!(
        %bundle_id,
        backend = "coredevice-app-service",
        success = result.is_ok(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "app launch completed"
    );
    result
}

pub(super) async fn stop_device_app_isolated(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    bundle_id: String,
) -> Result<bool, String> {
    let started = Instant::now();
    let mut client = connect_app_control(adapter, handshake).await?;
    let result = tokio::time::timeout(
        APP_CONTROL_OPERATION_TIMEOUT,
        stop_device_app(&mut client, &bundle_id),
    )
    .await
    .map_err(|_| {
        format!(
            "CoreDevice app stop timed out after {} seconds",
            APP_CONTROL_OPERATION_TIMEOUT.as_secs()
        )
    })?;
    tracing::debug!(
        %bundle_id,
        success = result.is_ok(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "isolated app stop completed"
    );
    result
}

fn sort_device_apps(mut apps: Vec<DeviceApp>) -> Vec<DeviceApp> {
    apps.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.bundle_id.cmp(&right.bundle_id))
    });
    apps
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::AppOperationState;

    #[tokio::test]
    async fn cancelling_app_owner_aborts_work_and_publishes_cancelled_state() {
        let operation = AppOperationSlot::default();
        let id = operation
            .start(AppOperationKind::Uninstall, "com.example.app".into())
            .unwrap();
        let handle = tokio::spawn(std::future::pending::<()>());
        let abort = handle.abort_handle();
        let mut task = Some(ActiveAppOperation { id, handle });

        cancel_active_operation(&operation, &mut task);
        tokio::task::yield_now().await;

        assert!(task.is_none());
        assert!(abort.is_finished());
        assert_eq!(operation.get().state, AppOperationState::Cancelled);
    }

    #[test]
    fn app_list_outer_timeout_covers_one_bounded_recovery() {
        let default_worst_case =
            APP_SERVICE_LIST_TIMEOUT + APP_CLIENT_RECONNECT_TIMEOUT + APP_SERVICE_LIST_TIMEOUT;
        let extended_worst_case = APP_SERVICE_LIST_TIMEOUT
            + APP_CLIENT_RECONNECT_TIMEOUT
            + APP_SERVICE_LIST_TIMEOUT
            + APP_METADATA_TIMEOUT;
        assert!(APP_LIST_REQUEST_TIMEOUT > default_worst_case);
        assert!(APP_LIST_REQUEST_TIMEOUT > extended_worst_case);
    }

    #[test]
    fn app_control_outer_timeout_covers_connection_and_operation_deadlines() {
        let dvt_attempt =
            APP_CLIENT_RECONNECT_TIMEOUT + APP_DVT_CHANNEL_TIMEOUT + APP_CONTROL_OPERATION_TIMEOUT;
        let fallback_attempt = APP_CLIENT_RECONNECT_TIMEOUT
            + APP_DVT_CHANNEL_TIMEOUT
            + APP_CLIENT_RECONNECT_TIMEOUT
            + APP_CONTROL_OPERATION_TIMEOUT;
        assert!(APP_CONTROL_REQUEST_TIMEOUT > dvt_attempt);
        assert!(APP_CONTROL_REQUEST_TIMEOUT > fallback_attempt);
    }

    #[test]
    fn app_control_slot_serializes_commands_and_releases_on_drop() {
        let slot = AppControlSlot::default();
        let lease = slot.try_start().unwrap();
        assert!(slot.try_start().is_err());
        drop(lease);
        assert!(slot.try_start().is_ok());
    }

    #[test]
    fn maps_installation_proxy_metadata_without_losing_bundle_identity() {
        let value = plist::Value::Dictionary(plist::Dictionary::from_iter([
            (
                String::from("CFBundleDisplayName"),
                plist::Value::String("Example Game".into()),
            ),
            (
                String::from("CFBundleShortVersionString"),
                plist::Value::String("2.4".into()),
            ),
            (
                String::from("CFBundleVersion"),
                plist::Value::String("42".into()),
            ),
            (String::from("IsXcodeManaged"), plist::Value::Boolean(true)),
            (
                String::from("UIFileSharingEnabled"),
                plist::Value::Boolean(true),
            ),
            (
                String::from("StaticDiskUsage"),
                plist::Value::Integer(1_500_000_u64.into()),
            ),
            (
                String::from("DynamicDiskUsage"),
                plist::Value::Integer(2_500_000_u64.into()),
            ),
        ]));

        let app = device_app_from_installation("com.example.game".into(), &value).unwrap();
        assert_eq!(app.bundle_id, "com.example.game");
        assert_eq!(app.name, "Example Game");
        assert_eq!(app.version.as_deref(), Some("2.4"));
        assert_eq!(app.bundle_version.as_deref(), Some("42"));
        assert!(app.is_developer_app);
        assert!(!app.is_app_clip);
        assert!(app.documents_available);
        assert_eq!(app.static_disk_usage_bytes, Some(1_500_000));
        assert_eq!(app.dynamic_disk_usage_bytes, Some(2_500_000));
        assert_eq!(app.total_disk_usage_bytes, Some(4_000_000));
        assert!(!app.is_removable);
        assert_eq!(app.is_running, None);
    }

    #[tokio::test]
    async fn extended_app_scopes_require_coredevice_app_service() {
        assert_eq!(
            list_device_apps(None, None, false, true, true)
                .await
                .unwrap_err(),
            "App Clip listing requires CoreDevice AppService, but it is unavailable"
        );
        assert_eq!(
            list_device_apps(None, None, true, true, true)
                .await
                .unwrap_err(),
            "system app and App Clip listing requires CoreDevice AppService, but it is unavailable"
        );
    }

    #[test]
    fn bounds_untrusted_installation_proxy_disk_usage() {
        let value = plist::Value::Dictionary(plist::Dictionary::from_iter([
            (
                String::from("StaticDiskUsage"),
                plist::Value::Integer((MAX_APP_DISK_USAGE_BYTES + 1).into()),
            ),
            (
                String::from("DynamicDiskUsage"),
                plist::Value::Integer(750_000_u64.into()),
            ),
        ]));
        assert_eq!(app_disk_usage(&value), (None, Some(750_000), Some(750_000)));
        assert_eq!(
            app_disk_usage(&plist::Value::String("invalid".into())),
            (None, None, None)
        );
    }

    #[test]
    fn normalizes_app_signing_metadata_without_exposing_signer_identity() {
        use devicehub_core::AppSigningKind;

        let metadata = |signer: &str, extra: Vec<(&str, plist::Value)>| {
            let mut fields = plist::Dictionary::new();
            fields.insert("SignerIdentity".into(), signer.into());
            fields.extend(extra.into_iter().map(|(key, value)| (key.into(), value)));
            plist::Value::Dictionary(fields)
        };
        let development = metadata(
            "Apple Development: Private Name (TEAM123)",
            vec![
                ("MinimumOSVersion", " 17.0\n".into()),
                (
                    "Entitlements",
                    plist::Value::Dictionary(plist::Dictionary::from_iter([(
                        String::from("get-task-allow"),
                        plist::Value::Boolean(true),
                    )])),
                ),
            ],
        );
        assert_eq!(
            app_signing_kind(Some(&development), false, false),
            AppSigningKind::Development
        );
        assert_eq!(
            app_minimum_os_version(&development).as_deref(),
            Some("17.0")
        );
        assert_eq!(app_debuggable(&development), Some(true));

        let testflight = metadata(
            "Apple iPhone OS Application Signing",
            vec![("BetaExternalVersionIdentifier", 123_u64.into())],
        );
        assert_eq!(
            app_signing_kind(Some(&testflight), false, false),
            AppSigningKind::TestFlight
        );
        assert_eq!(
            app_signing_kind(
                Some(&metadata("Apple iPhone OS Application Signing", vec![])),
                false,
                false,
            ),
            AppSigningKind::AppStore
        );
        assert_eq!(
            app_signing_kind(
                Some(&metadata("iPhone Distribution: Private Company", vec![])),
                false,
                false,
            ),
            AppSigningKind::Distribution
        );
        assert_eq!(
            app_signing_kind(Some(&testflight), true, false),
            AppSigningKind::System
        );
        assert_eq!(
            app_signing_kind(None, false, false),
            AppSigningKind::Unknown
        );
    }

    #[test]
    fn rejects_unbounded_app_metadata_text() {
        let value = plist::Value::Dictionary(plist::Dictionary::from_iter([(
            String::from("MinimumOSVersion"),
            plist::Value::String("x".repeat(33)),
        )]));
        assert_eq!(app_minimum_os_version(&value), None);
        let invalid = plist::Value::Dictionary(plist::Dictionary::from_iter([(
            String::from("MinimumOSVersion"),
            plist::Value::String("17.0 beta".into()),
        )]));
        assert_eq!(app_minimum_os_version(&invalid), None);
    }
}
