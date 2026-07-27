//! Device discovery, selection, and outer connected-session lifecycle.
//!
//! This manager is the single owner of reconnect and handoff policy. Hosts
//! inject platform capabilities, but cannot create overlapping media sessions
//! or implement a divergent USB/Wi-Fi retry loop.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devicehub_core::{
    ActiveSlot, DeviceListSlot, DevicePairingState, ForgetDeviceResult, LocationStatus,
    PairDeviceOutcome, PairDeviceResult,
};
use tokio::sync::{mpsc::UnboundedReceiver, mpsc::UnboundedSender, oneshot};

use super::{
    ConnectedSessionHost, ConnectedSessionMedia, ConnectedSessionViews, SessionFailureAction,
    SessionRetryPolicy, forget_device, pair_device, run_connected_session,
};
use crate::clipboard::HostClipboardFactory;
use crate::runtime::{CoreRuntimeFuture, CoreRuntimeState, DeviceSessionState};
use crate::transport::{CoreTunnelConfig, DeviceDiscovery};
use crate::{
    CaptureFileIo, CoreRuntime, DeveloperImageAssetLoader, DeviceAudioPipelineFactory,
    DeviceBackupDestination, DeviceSessionCommand, DeviceSessionRegistry,
    DiagnosticDumpSinkFactory, HostClipboardProvider, HostFileIo, MuxSidecar, PairingStore,
    ProvisioningProfileLoader, RuntimeClient, RuntimePreferences, RuntimeSessionHostAdapters,
    SessionCommandSlot, SessionControlCommand, SessionDiagnostics, SessionEndpoint,
    SystemUsbmuxdConfig, resolve_device_selection,
};

const IDLE_RESCAN: Duration = Duration::from_secs(2);
const SWITCH_GRACE: Duration = Duration::from_secs(3);

/// Platform capabilities injected once when starting the device runtime.
///
/// The host resolves operating-system resources, while the runtime retains
/// discovery, trust, reconnect, and connected-session lifecycle policy.
pub struct RuntimeHostAdapters<
    Sidecar,
    Store,
    AudioFactory,
    DiagnosticSinks,
    Clipboard,
    Files,
    CaptureFiles,
    Backup,
    DeveloperImages,
    Profiles,
> {
    pub sidecar: Sidecar,
    pub pairing_store: Option<Store>,
    pub system_usbmuxd: SystemUsbmuxdConfig,
    pub audio: AudioFactory,
    pub diagnostic_sinks: DiagnosticSinks,
    pub clipboard: Clipboard,
    pub services:
        RuntimeSessionHostAdapters<Files, CaptureFiles, Backup, DeveloperImages, Profiles>,
}

/// Runtime-owned outer session manager assembled from host capability ports.
///
/// Device discovery, trust transitions, reconnect policy, and connected-session
/// ownership remain private implementation details.
struct SessionManager<
    Sidecar,
    Store,
    AudioFactory,
    DiagnosticSinks,
    Clipboard,
    Files,
    CaptureFiles,
    Backup,
    DeveloperImages,
    Profiles,
> {
    discovery: DeviceDiscovery<Sidecar, Arc<Store>>,
    tunnel: CoreTunnelConfig,
    audio: AudioFactory,
    diagnostic_sinks: DiagnosticSinks,
    clipboard: Clipboard,
    services: RuntimeSessionHostAdapters<Files, CaptureFiles, Backup, DeveloperImages, Profiles>,
}

/// Running runtime owner and its cloneable host-facing client.
pub struct StartedRuntime<HostPath> {
    runtime: CoreRuntime,
    client: RuntimeClient<HostPath>,
}

impl<HostPath> StartedRuntime<HostPath> {
    pub fn into_parts(self) -> (CoreRuntime, RuntimeClient<HostPath>) {
        (self.runtime, self.client)
    }
}

impl<
    Sidecar,
    Store,
    AudioFactory,
    DiagnosticSinks,
    Clipboard,
    Files,
    CaptureFiles,
    Backup,
    DeveloperImages,
    Profiles,
>
    SessionManager<
        Sidecar,
        Store,
        AudioFactory,
        DiagnosticSinks,
        Clipboard,
        Files,
        CaptureFiles,
        Backup,
        DeveloperImages,
        Profiles,
    >
where
    Sidecar: MuxSidecar,
    Store: PairingStore,
    AudioFactory: DeviceAudioPipelineFactory,
    DiagnosticSinks: DiagnosticDumpSinkFactory,
    Clipboard: HostClipboardProvider,
    Files: HostFileIo,
    CaptureFiles: CaptureFileIo<Destination = Files::Path>,
    Backup: DeviceBackupDestination<Destination = Files::Path>,
    DeveloperImages: DeveloperImageAssetLoader<Source = Files::Path>,
    Profiles: ProvisioningProfileLoader<Source = Files::Path>,
{
    fn new(
        adapters: RuntimeHostAdapters<
            Sidecar,
            Store,
            AudioFactory,
            DiagnosticSinks,
            Clipboard,
            Files,
            CaptureFiles,
            Backup,
            DeveloperImages,
            Profiles,
        >,
    ) -> Self {
        let RuntimeHostAdapters {
            sidecar,
            pairing_store,
            system_usbmuxd,
            audio,
            diagnostic_sinks,
            clipboard,
            services,
        } = adapters;
        let pairing_store = pairing_store.map(Arc::new);
        let tunnel = CoreTunnelConfig::new(pairing_store.clone(), system_usbmuxd);
        Self {
            discovery: DeviceDiscovery::new(sidecar, pairing_store, tunnel.clone()),
            tunnel,
            audio,
            diagnostic_sinks,
            clipboard,
            services,
        }
    }

    /// Run discovery and independently supervised device sessions until shutdown.
    pub(crate) async fn run(
        self,
        initial_selection: Option<String>,
        preferences: RuntimePreferences,
        diagnostics: SessionDiagnostics<DiagnosticSinks::Source>,
        state: CoreRuntimeState<Files::Path>,
        control_rx: UnboundedReceiver<SessionControlCommand>,
    ) {
        run_session_manager(
            initial_selection,
            preferences,
            diagnostics,
            self,
            state,
            control_rx,
        )
        .await;
    }
}

/// Start the device runtime from lazily constructed host capabilities.
///
/// Capability construction happens on the dedicated owner thread. Hosts cannot
/// construct the session manager, its state graph, or a competing reconnect
/// loop; they receive only the runtime owner and its cloneable client.
pub fn start_runtime<
    Build,
    Sidecar,
    Store,
    AudioFactory,
    DiagnosticSinks,
    Clipboard,
    Files,
    CaptureFiles,
    Backup,
    DeveloperImages,
    Profiles,
>(
    build: Build,
    initial_selection: Option<String>,
    preferences: RuntimePreferences,
    diagnostics: SessionDiagnostics<DiagnosticSinks::Source>,
) -> Result<StartedRuntime<Files::Path>, String>
where
    Build: FnOnce() -> RuntimeHostAdapters<
            Sidecar,
            Store,
            AudioFactory,
            DiagnosticSinks,
            Clipboard,
            Files,
            CaptureFiles,
            Backup,
            DeveloperImages,
            Profiles,
        > + Send
        + 'static,
    Sidecar: MuxSidecar,
    Store: PairingStore,
    AudioFactory: DeviceAudioPipelineFactory,
    DiagnosticSinks: DiagnosticDumpSinkFactory,
    Clipboard: HostClipboardProvider,
    Files: HostFileIo,
    CaptureFiles: CaptureFileIo<Destination = Files::Path>,
    Backup: DeviceBackupDestination<Destination = Files::Path>,
    DeveloperImages: DeveloperImageAssetLoader<Source = Files::Path>,
    Profiles: ProvisioningProfileLoader<Source = Files::Path>,
{
    let (runtime, client) = CoreRuntime::start(move |control, control_rx| {
        let state = CoreRuntimeState::<Files::Path>::default();
        let client = state.client(control);
        let task = move || -> CoreRuntimeFuture {
            Box::pin(SessionManager::new(build()).run(
                initial_selection,
                preferences,
                diagnostics,
                state,
                control_rx,
            ))
        };
        (client, task)
    })?;
    Ok(StartedRuntime { runtime, client })
}

/// State surfaces shared with host adapters without exposing session ownership.
#[derive(Clone)]
pub(crate) struct SessionManagerViews<HostPath> {
    pub(crate) connected: ConnectedSessionViews,
    pub(crate) devices: DeviceListSlot,
    pub(crate) active: ActiveSlot,
    pub(crate) sessions: DeviceSessionRegistry<HostPath>,
}

struct ManagedSessionViews<HostPath> {
    connected: ConnectedSessionViews,
    commands: SessionCommandSlot<HostPath>,
    supervisor: SessionSupervisorSlot,
}

impl<HostPath> Clone for ManagedSessionViews<HostPath> {
    fn clone(&self) -> Self {
        Self {
            connected: self.connected.clone(),
            commands: self.commands.clone(),
            supervisor: self.supervisor.clone(),
        }
    }
}

#[derive(Clone, Default)]
struct SessionSupervisorSlot(Arc<Mutex<Option<oneshot::Sender<()>>>>);

impl SessionSupervisorSlot {
    fn replace(&self, sender: oneshot::Sender<()>) {
        *self.0.lock().unwrap() = Some(sender);
    }

    fn stop(&self) {
        if let Some(sender) = self.0.lock().unwrap().take() {
            let _ = sender.send(());
        }
    }

    fn clear(&self) {
        self.0.lock().unwrap().take();
    }
}

enum PendingManagementAction {
    Pair(oneshot::Sender<PairDeviceResult>),
    Forget(oneshot::Sender<ForgetDeviceResult>),
}

enum ManagementOutcome {
    None,
    Connect(String),
    Remove(String),
}

async fn pair_request(
    selection_id: String,
    reply: oneshot::Sender<PairDeviceResult>,
    endpoints: &HashMap<String, SessionEndpoint>,
    views: &ConnectedSessionViews,
) -> bool {
    let result = pair_device(&selection_id, endpoints, &views.status).await;
    let paired = result.outcome == PairDeviceOutcome::Paired;
    let _ = reply.send(result);
    paired
}

async fn forget_request<Sidecar, Store>(
    selection_id: String,
    reply: oneshot::Sender<ForgetDeviceResult>,
    endpoints: &HashMap<String, SessionEndpoint>,
    views: &ConnectedSessionViews,
    discovery: &mut DeviceDiscovery<Sidecar, Store>,
) where
    Sidecar: MuxSidecar,
    Store: PairingStore,
{
    let result = forget_device(&selection_id, endpoints, &views.status, discovery).await;
    let _ = reply.send(result);
}

fn ensure_session<HostPath>(
    selection_id: &str,
    views: &SessionManagerViews<HostPath>,
    sessions: &mut HashMap<String, ManagedSessionViews<HostPath>>,
) -> ManagedSessionViews<HostPath> {
    sessions
        .entry(selection_id.to_string())
        .or_insert_with(|| {
            let state = DeviceSessionState::<HostPath>::default();
            let client = state.client();
            let session_views = ManagedSessionViews {
                connected: state.connected_views(),
                commands: client.commands.clone(),
                supervisor: SessionSupervisorSlot::default(),
            };
            views.sessions.insert(selection_id.to_string(), client);
            session_views
        })
        .clone()
}

async fn perform_management_action<Sidecar, Store, HostPath>(
    selection_id: String,
    action: PendingManagementAction,
    endpoints: &HashMap<String, SessionEndpoint>,
    views: &ManagedSessionViews<HostPath>,
    discovery: &mut DeviceDiscovery<Sidecar, Store>,
) -> ManagementOutcome
where
    Sidecar: MuxSidecar,
    Store: PairingStore,
{
    match action {
        PendingManagementAction::Pair(reply) => {
            let requested = selection_id.clone();
            if pair_request(selection_id, reply, endpoints, &views.connected).await {
                discovery.invalidate();
                ManagementOutcome::Connect(requested)
            } else {
                discovery.invalidate();
                ManagementOutcome::None
            }
        }
        PendingManagementAction::Forget(reply) => {
            forget_request(
                selection_id.clone(),
                reply,
                endpoints,
                &views.connected,
                discovery,
            )
            .await;
            discovery.invalidate();
            ManagementOutcome::Remove(selection_id)
        }
    }
}

fn apply_management_outcome<HostPath>(
    outcome: ManagementOutcome,
    views: &SessionManagerViews<HostPath>,
    sessions: &mut HashMap<String, ManagedSessionViews<HostPath>>,
    pending_connect: &mut HashSet<String>,
) {
    match outcome {
        ManagementOutcome::None => {}
        ManagementOutcome::Connect(selection_id) => {
            pending_connect.insert(selection_id);
        }
        ManagementOutcome::Remove(selection_id) => {
            sessions.remove(&selection_id);
            views.sessions.remove(&selection_id);
            if views.active.selection_id().as_deref() == Some(selection_id.as_str()) {
                views.active.set(None);
            }
        }
    }
}

fn running_selection_for_udid<'a>(
    running: &'a HashSet<String>,
    endpoints: &HashMap<String, SessionEndpoint>,
    udid: &str,
) -> Option<&'a str> {
    running.iter().find_map(|selection_id| {
        endpoints
            .get(selection_id)
            .filter(|endpoint| endpoint.udid() == udid)
            .map(|_| selection_id.as_str())
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_connected_session<
    Sidecar,
    Store,
    AudioFactory,
    DiagnosticSinks,
    Clipboard,
    Files,
    CaptureFiles,
    Backup,
    DeveloperImages,
    Profiles,
>(
    selection_id: String,
    endpoint: SessionEndpoint,
    preferences: RuntimePreferences,
    active: ActiveSlot,
    diagnostics: SessionDiagnostics<DiagnosticSinks::Source>,
    host: &SessionManager<
        Sidecar,
        Store,
        AudioFactory,
        DiagnosticSinks,
        Clipboard,
        Files,
        CaptureFiles,
        Backup,
        DeveloperImages,
        Profiles,
    >,
    views: ManagedSessionViews<Files::Path>,
    ended: UnboundedSender<String>,
) where
    Sidecar: MuxSidecar,
    Store: PairingStore,
    AudioFactory: DeviceAudioPipelineFactory,
    DiagnosticSinks: DiagnosticDumpSinkFactory,
    Clipboard: HostClipboardProvider,
    Files: HostFileIo,
    CaptureFiles: CaptureFileIo<Destination = Files::Path>,
    Backup: DeviceBackupDestination<Destination = Files::Path>,
    DeveloperImages: DeveloperImageAssetLoader<Source = Files::Path>,
    Profiles: ProvisioningProfileLoader<Source = Files::Path>,
{
    let tunnel = host.tunnel.clone();
    let audio = host.audio.clone();
    let diagnostic_sinks = host.diagnostic_sinks.clone();
    let clipboard = host.clipboard.clone();
    let services = host.services.clone();
    let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (supervisor_tx, mut supervisor_rx) = oneshot::channel();
    views.supervisor.replace(supervisor_tx);
    views.commands.set(Some(command_tx.clone()));
    views.connected.error.set(None);

    tokio::task::spawn_local(async move {
        let connection = endpoint.connection();
        let mut retry_policy = SessionRetryPolicy::default();
        loop {
            views.connected.browser_frames.begin_stream();
            let clipboard_sync_enabled = preferences.clipboard_sync_enabled();
            let clipboard_provider = clipboard.clone();
            let session_started = Instant::now();
            let session = run_connected_session(
                endpoint.clone(),
                tunnel.clone(),
                ConnectedSessionMedia {
                    clipboard_sync_enabled,
                    diagnostics: diagnostics.clone(),
                },
                ConnectedSessionHost {
                    audio: audio.create(preferences.audio_enabled(), &selection_id, active.clone()),
                    diagnostic_sinks: diagnostic_sinks.clone(),
                    clipboard: clipboard_sync_enabled.then(|| {
                        Box::new(move || clipboard_provider.connect()) as HostClipboardFactory
                    }),
                    services: services.clone(),
                },
                views.connected.clone(),
                &mut command_rx,
            );
            tokio::pin!(session);
            let (result, stop_requested) = tokio::select! {
                result = &mut session => (result, false),
                _ = &mut supervisor_rx => {
                    let _ = command_tx.send(DeviceSessionCommand::Shutdown);
                    (session.await, true)
                }
            };
            if stop_requested {
                break;
            }

            match result {
                Ok(()) => break,
                Err(error_message) => {
                    tracing::error!(
                        device_id = %selection_id,
                        connection = connection.label(),
                        "session ended: {error_message}"
                    );
                    views.connected.error.set(Some(error_message.clone()));
                    match retry_policy.after_failure(
                        connection,
                        &error_message,
                        session_started.elapsed(),
                    ) {
                        SessionFailureAction::Stop => break,
                        SessionFailureAction::Retry(retry) => {
                            views
                                .connected
                                .status
                                .set("Wi-Fi control interrupted - retrying connection...");
                            tracing::info!(
                                device_id = %selection_id,
                                attempt = retry.attempt,
                                retry_ms = retry.delay.as_millis(),
                                "Wi-Fi session transport dropped; rebuilding the device tunnel"
                            );
                            tokio::select! {
                                _ = tokio::time::sleep(retry.delay) => {}
                                _ = &mut supervisor_rx => break,
                            }
                        }
                    }
                }
            }
        }

        views.commands.set(None);
        views.supervisor.clear();
        views
            .connected
            .runtime_services
            .location
            .set(LocationStatus::default());
        views.connected.runtime_services.performance.reset();
        views.connected.runtime_services.device_logs.reset();
        views.connected.runtime_services.services.clear();
        views.connected.status.set("disconnected");
        let _ = ended.send(selection_id);
    });
}

async fn stop_all_sessions<HostPath>(
    sessions: &HashMap<String, ManagedSessionViews<HostPath>>,
    running: &mut HashSet<String>,
    ended: &mut UnboundedReceiver<String>,
) {
    for selection_id in running.iter() {
        if let Some(session) = sessions.get(selection_id) {
            session.supervisor.stop();
            session.commands.send(DeviceSessionCommand::Shutdown);
        }
    }
    let deadline = tokio::time::sleep(SWITCH_GRACE);
    tokio::pin!(deadline);
    while !running.is_empty() {
        tokio::select! {
            ended_id = ended.recv() => {
                let Some(ended_id) = ended_id else { break };
                running.remove(&ended_id);
            }
            _ = &mut deadline => break,
        }
    }
}

/// Run discovery and any number of independently supervised device sessions.
async fn run_session_manager<
    Sidecar,
    Store,
    AudioFactory,
    DiagnosticSinks,
    Clipboard,
    Files,
    CaptureFiles,
    Backup,
    DeveloperImages,
    Profiles,
>(
    initial_selection: Option<String>,
    preferences: RuntimePreferences,
    diagnostics: SessionDiagnostics<DiagnosticSinks::Source>,
    mut host: SessionManager<
        Sidecar,
        Store,
        AudioFactory,
        DiagnosticSinks,
        Clipboard,
        Files,
        CaptureFiles,
        Backup,
        DeveloperImages,
        Profiles,
    >,
    state: CoreRuntimeState<Files::Path>,
    mut control_rx: UnboundedReceiver<SessionControlCommand>,
) where
    Sidecar: MuxSidecar,
    Store: PairingStore,
    AudioFactory: DeviceAudioPipelineFactory,
    DiagnosticSinks: DiagnosticDumpSinkFactory,
    Clipboard: HostClipboardProvider,
    Files: HostFileIo,
    CaptureFiles: CaptureFileIo<Destination = Files::Path>,
    Backup: DeviceBackupDestination<Destination = Files::Path>,
    DeveloperImages: DeveloperImageAssetLoader<Source = Files::Path>,
    Profiles: ProvisioningProfileLoader<Source = Files::Path>,
{
    let views = state.manager_views();
    let mut sessions = HashMap::<String, ManagedSessionViews<Files::Path>>::new();
    let mut running = HashSet::<String>::new();
    let mut pending_connect = HashSet::<String>::new();
    let mut pending_reconnect = HashSet::<String>::new();
    let mut pending_management = HashMap::<String, PendingManagementAction>::new();
    let (ended_tx, mut ended_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut endpoints = HashMap::<String, SessionEndpoint>::new();
    let mut auto_pick = initial_selection.is_none();
    if let Some(selection_id) = initial_selection {
        pending_connect.insert(selection_id);
    }
    let mut discovery_tick = tokio::time::interval(IDLE_RESCAN);
    discovery_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            command = control_rx.recv() => match command {
                Some(SessionControlCommand::Connect(requested)) => {
                    let selection_id = resolve_device_selection(&requested, &views.devices.get())
                        .unwrap_or(requested);
                    let session = ensure_session(&selection_id, &views, &mut sessions);
                    if let Some(endpoint) = endpoints.get(&selection_id) {
                        views.active.set_selected(endpoint.udid().to_owned(), selection_id.clone());
                    }
                    if !running.contains(&selection_id) {
                        session.connected.status.set("waiting for selected device transport...");
                        pending_connect.insert(selection_id);
                    }
                }
                Some(SessionControlCommand::Disconnect(requested)) => {
                    let selection_id = resolve_device_selection(&requested, &views.devices.get())
                        .unwrap_or(requested);
                    pending_connect.remove(&selection_id);
                    pending_reconnect.remove(&selection_id);
                    if let Some(session) = sessions.get(&selection_id) {
                        session.supervisor.stop();
                        session.commands.send(DeviceSessionCommand::Shutdown);
                        if !running.contains(&selection_id) {
                            session.connected.status.set("disconnected");
                        }
                    }
                    if views.active.selection_id().as_deref() == Some(selection_id.as_str()) {
                        if let Some(fallback) = running.iter()
                            .filter(|candidate| candidate.as_str() != selection_id)
                            .find_map(|candidate| endpoints.get(candidate)
                                .map(|endpoint| (candidate.clone(), endpoint.udid().to_owned())))
                        {
                            views.active.set_selected(fallback.1, fallback.0);
                        } else {
                            views.active.set(None);
                        }
                    }
                }
                Some(SessionControlCommand::Reconnect(requested)) => {
                    let selection_id = resolve_device_selection(&requested, &views.devices.get())
                        .unwrap_or(requested);
                    let session = ensure_session(&selection_id, &views, &mut sessions);
                    pending_reconnect.insert(selection_id.clone());
                    if running.contains(&selection_id) {
                        session.supervisor.stop();
                        session.commands.send(DeviceSessionCommand::Shutdown);
                    } else {
                        pending_connect.insert(selection_id);
                    }
                }
                Some(SessionControlCommand::Refresh) => host.discovery.invalidate(),
                Some(SessionControlCommand::Pair { selection_id, reply }) => {
                    let session = ensure_session(&selection_id, &views, &mut sessions);
                    let action = PendingManagementAction::Pair(reply);
                    if running.contains(&selection_id) {
                        pending_management.insert(selection_id.clone(), action);
                        session.supervisor.stop();
                        session.commands.send(DeviceSessionCommand::Shutdown);
                    } else {
                        let outcome = perform_management_action(
                            selection_id,
                            action,
                            &endpoints,
                            &session,
                            &mut host.discovery,
                        ).await;
                        apply_management_outcome(outcome, &views, &mut sessions, &mut pending_connect);
                    }
                }
                Some(SessionControlCommand::Forget { selection_id, reply }) => {
                    let session = ensure_session(&selection_id, &views, &mut sessions);
                    let action = PendingManagementAction::Forget(reply);
                    if running.contains(&selection_id) {
                        pending_management.insert(selection_id.clone(), action);
                        session.supervisor.stop();
                        session.commands.send(DeviceSessionCommand::Shutdown);
                    } else {
                        let outcome = perform_management_action(
                            selection_id,
                            action,
                            &endpoints,
                            &session,
                            &mut host.discovery,
                        ).await;
                        apply_management_outcome(outcome, &views, &mut sessions, &mut pending_connect);
                    }
                }
                Some(SessionControlCommand::Quit) | None => {
                    stop_all_sessions(&sessions, &mut running, &mut ended_rx).await;
                    return;
                }
            },
            ended_id = ended_rx.recv() => {
                let Some(ended_id) = ended_id else { return };
                running.remove(&ended_id);
                if let Some(action) = pending_management.remove(&ended_id)
                    && let Some(session) = sessions.get(&ended_id).cloned()
                {
                    let outcome = perform_management_action(
                        ended_id.clone(),
                        action,
                        &endpoints,
                        &session,
                        &mut host.discovery,
                    ).await;
                    apply_management_outcome(outcome, &views, &mut sessions, &mut pending_connect);
                } else if pending_reconnect.remove(&ended_id) {
                    pending_connect.insert(ended_id);
                }
            }
            _ = discovery_tick.tick() => {
                let (devices, discovered_endpoints) = host.discovery.refresh().await;
                views.devices.set(devices);
                endpoints = discovered_endpoints;
                if auto_pick
                    && pending_connect.is_empty()
                    && let Some(first) = views.devices.get().into_iter()
                        .find(|device| device.pairing != DevicePairingState::Unpaired)
                {
                    pending_connect.insert(first.id);
                    auto_pick = false;
                }

                let ready = pending_connect.iter()
                    .filter_map(|selection_id| endpoints.get(selection_id).cloned()
                        .map(|endpoint| (selection_id.clone(), endpoint)))
                    .collect::<Vec<_>>();
                for (selection_id, endpoint) in ready {
                    pending_connect.remove(&selection_id);
                    if let Some(existing) = running_selection_for_udid(
                        &running,
                        &endpoints,
                        endpoint.udid(),
                    ) && existing != selection_id
                    {
                        let session = ensure_session(&selection_id, &views, &mut sessions);
                        session.connected.error.set(Some(format!(
                            "device is already connected through {existing}"
                        )));
                        session.connected.status.set("disconnected");
                        continue;
                    }
                    if !running.insert(selection_id.clone()) {
                        continue;
                    }
                    let session = ensure_session(&selection_id, &views, &mut sessions);
                    views.active.set_selected(endpoint.udid().to_owned(), selection_id.clone());
                    spawn_connected_session(
                        selection_id,
                        endpoint,
                        preferences.clone(),
                        views.active.clone(),
                        diagnostics.clone(),
                        &host,
                        session,
                        ended_tx.clone(),
                    );
                }

                if sessions.is_empty() {
                    views.connected.status.set(if host.discovery.requires_pairing() {
                        "Wi-Fi device found - connect it by USB once to authorize this app"
                    } else {
                        "no device - pick one from the menu"
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionManagerViews, ensure_session};
    use crate::runtime::CoreRuntimeState;

    #[test]
    fn creating_another_session_preserves_existing_state() {
        let state = CoreRuntimeState::<String>::default();
        let views: SessionManagerViews<String> = state.manager_views();
        let mut sessions = std::collections::HashMap::new();
        let phone = ensure_session("phone::usb", &views, &mut sessions);
        phone.connected.status.set("connected");

        let tablet = ensure_session("tablet::usb", &views, &mut sessions);
        tablet.connected.status.set("connecting");

        assert_eq!(sessions.len(), 2);
        assert_eq!(phone.connected.status.get(), "connected");
        assert_eq!(tablet.connected.status.get(), "connecting");
        assert_eq!(
            views.sessions.selection_ids(),
            vec!["phone::usb".to_string(), "tablet::usb".to_string()]
        );
    }
}
