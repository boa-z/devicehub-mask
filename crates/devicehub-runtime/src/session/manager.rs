//! Device discovery, selection, and outer connected-session lifecycle.
//!
//! This manager is the single owner of reconnect and handoff policy. Hosts
//! inject platform capabilities, but cannot create overlapping media sessions
//! or implement a divergent USB/Wi-Fi retry loop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devicehub_core::{
    ActiveSlot, DeviceListSlot, DevicePairingState, ForgetDeviceResult, LocationStatus,
    PairDeviceOutcome, PairDeviceResult,
};
use tokio::sync::{mpsc::UnboundedReceiver, oneshot};

use super::{
    ConnectedSessionHost, ConnectedSessionMedia, ConnectedSessionViews, SessionFailureAction,
    SessionRetry, SessionRetryPolicy, forget_device, pair_device, run_connected_session,
};
use crate::clipboard::HostClipboardFactory;
use crate::runtime::{CoreRuntimeFuture, CoreRuntimeState};
use crate::transport::{CoreTunnelConfig, DeviceDiscovery};
use crate::{
    CaptureFileIo, CoreRuntime, DeveloperImageAssetLoader, DeviceAudioPipelineFactory,
    DeviceBackupDestination, DeviceSessionCommand, DiagnosticDumpSinkFactory,
    HostClipboardProvider, HostFileIo, MuxSidecar, PairingStore, ProvisioningProfileLoader,
    RuntimeClient, RuntimePreferences, RuntimeSessionHostAdapters, SessionCommandSlot,
    SessionControlCommand, SessionDiagnostics, SessionEndpoint, SystemUsbmuxdConfig,
    resolve_device_selection,
};

const IDLE_RESCAN: Duration = Duration::from_secs(2);
const ACTIVE_RESCAN: Duration = Duration::from_secs(8);
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

    /// Run discovery and exactly one selected device session until shutdown.
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

/// Start the sole device runtime from lazily constructed host capabilities.
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
    pub(crate) commands: SessionCommandSlot<HostPath>,
}

enum Next {
    Switch(String),
    RetryWifi {
        selection_id: String,
        retry: SessionRetry,
    },
    Pair {
        selection_id: String,
        reply: oneshot::Sender<PairDeviceResult>,
    },
    Forget {
        selection_id: String,
        reply: oneshot::Sender<ForgetDeviceResult>,
    },
    Idle,
    Quit,
}

fn interrupts_active_session(next: &Next) -> bool {
    matches!(
        next,
        Next::Switch(_) | Next::Pair { .. } | Next::Forget { .. } | Next::Quit
    )
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

fn reset_idle_views<HostPath>(views: &SessionManagerViews<HostPath>) {
    views.active.set(None);
    views
        .connected
        .runtime_services
        .location
        .set(LocationStatus::default());
    views.connected.runtime_services.performance.reset();
    views.connected.runtime_services.device_logs.reset();
    views.connected.runtime_services.services.clear();
}

/// Run discovery and exactly one selected device session until the host quits.
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
    // Auto-pick only before the first connection. Returning to idle after a
    // session ends prevents a persistent hardware failure from hot-looping.
    let mut auto_pick = initial_selection.is_none();
    let mut target = initial_selection;
    let mut retry_policy = SessionRetryPolicy::default();

    loop {
        let (devices, endpoints) = host.discovery.refresh().await;
        views.devices.set(devices);
        let wifi_setup_required = host.discovery.requires_pairing();

        if let Some(requested) = target.as_deref()
            && let Some(resolved) = resolve_device_selection(requested, &views.devices.get())
        {
            target = Some(resolved);
        }

        if target.is_none()
            && auto_pick
            && let Some(first) = views
                .devices
                .get()
                .into_iter()
                .find(|device| device.pairing != DevicePairingState::Unpaired)
        {
            target = Some(first.id);
            auto_pick = false;
        }

        let Some(selection_id) = target.clone() else {
            reset_idle_views(&views);
            views.connected.status.set(if wifi_setup_required {
                "Wi-Fi device found - connect it by USB once to authorize this app"
            } else {
                "no device - pick one from the menu"
            });
            tokio::select! {
                command = control_rx.recv() => match command {
                    Some(SessionControlCommand::Connect(id) | SessionControlCommand::Reconnect(id)) => target = Some(id),
                    Some(SessionControlCommand::Refresh) => host.discovery.invalidate(),
                    Some(SessionControlCommand::Pair { selection_id, reply }) => {
                        let requested = selection_id.clone();
                        if pair_request(selection_id, reply, &endpoints, &views.connected).await {
                            target = Some(requested);
                        }
                        host.discovery.invalidate();
                    }
                    Some(SessionControlCommand::Forget { selection_id, reply }) => {
                        forget_request(selection_id, reply, &endpoints, &views.connected, &mut host.discovery).await;
                        host.discovery.invalidate();
                    }
                    Some(SessionControlCommand::Quit) | None => return,
                },
                _ = tokio::time::sleep(IDLE_RESCAN) => {}
            }
            continue;
        };

        let Some(endpoint) = endpoints.get(&selection_id).cloned() else {
            tracing::debug!(transport = %selection_id, "requested device transport not discovered yet");
            views.active.set(None);
            views
                .connected
                .status
                .set("waiting for selected device transport...");
            tokio::select! {
                command = control_rx.recv() => match command {
                    Some(SessionControlCommand::Connect(id) | SessionControlCommand::Reconnect(id)) => target = Some(id),
                    Some(SessionControlCommand::Refresh) => host.discovery.invalidate(),
                    Some(SessionControlCommand::Pair { selection_id, reply }) => {
                        let requested = selection_id.clone();
                        if pair_request(selection_id, reply, &endpoints, &views.connected).await {
                            target = Some(requested);
                        }
                        host.discovery.invalidate();
                    }
                    Some(SessionControlCommand::Forget { selection_id, reply }) => {
                        forget_request(selection_id, reply, &endpoints, &views.connected, &mut host.discovery).await;
                        target = None;
                        host.discovery.invalidate();
                    }
                    Some(SessionControlCommand::Quit) | None => return,
                },
                _ = tokio::time::sleep(IDLE_RESCAN) => {}
            }
            continue;
        };

        let udid = endpoint.udid().to_owned();
        let connection = endpoint.connection();
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        views.commands.set(Some(command_tx.clone()));
        views
            .active
            .set_selected(udid.clone(), selection_id.clone());
        views.connected.error.set(None);

        let clipboard_sync_enabled = preferences.clipboard_sync_enabled();
        let clipboard_provider = host.clipboard.clone();
        let session = run_connected_session(
            endpoint,
            host.tunnel.clone(),
            ConnectedSessionMedia {
                clipboard_sync_enabled,
                diagnostics: diagnostics.clone(),
            },
            ConnectedSessionHost {
                audio: host.audio.create(preferences.audio_enabled()),
                diagnostic_sinks: host.diagnostic_sinks.clone(),
                clipboard: clipboard_sync_enabled.then(|| {
                    Box::new(move || clipboard_provider.connect()) as HostClipboardFactory
                }),
                services: host.services.clone(),
            },
            views.connected.clone(),
            &mut command_rx,
        );
        tokio::pin!(session);
        let session_started = Instant::now();
        let mut active_rescan = tokio::time::interval(ACTIVE_RESCAN);
        active_rescan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        active_rescan.tick().await;

        let outcome = loop {
            tokio::select! {
                result = &mut session => match result {
                    Ok(()) => break Next::Idle,
                    Err(error_message) => {
                        tracing::error!(connection = connection.label(), "session ended: {error_message}");
                        let next = match retry_policy.after_failure(connection, &error_message, session_started.elapsed()) {
                            SessionFailureAction::Stop => Next::Idle,
                            SessionFailureAction::Retry(retry) => Next::RetryWifi {
                                selection_id: selection_id.clone(),
                                retry,
                            },
                        };
                        views.connected.error.set(Some(error_message));
                        break next;
                    }
                },
                command = control_rx.recv() => match command {
                    Some(SessionControlCommand::Connect(id)) if id != selection_id && id != udid => break Next::Switch(id),
                    Some(SessionControlCommand::Connect(_)) => {}
                    Some(SessionControlCommand::Reconnect(id)) => break Next::Switch(id),
                    Some(SessionControlCommand::Refresh) => {
                        host.discovery.invalidate();
                        let (devices, _) = host.discovery.refresh().await;
                        views.devices.set(devices);
                    }
                    Some(SessionControlCommand::Pair { selection_id, reply }) => break Next::Pair { selection_id, reply },
                    Some(SessionControlCommand::Forget { selection_id, reply }) => break Next::Forget { selection_id, reply },
                    Some(SessionControlCommand::Quit) | None => break Next::Quit,
                },
                _ = active_rescan.tick() => {
                    let (devices, _) = host.discovery.refresh().await;
                    views.devices.set(devices);
                }
            }
        };

        // A user transition waits for teardown so two media/HID owners can
        // never overlap on one physical device.
        if interrupts_active_session(&outcome) {
            let _ = command_tx.send(DeviceSessionCommand::Shutdown);
            let _ = tokio::time::timeout(SWITCH_GRACE, &mut session).await;
        }
        views.commands.set(None);
        views.active.set(None);
        views
            .connected
            .runtime_services
            .location
            .set(LocationStatus::default());

        match outcome {
            Next::Switch(id) => {
                retry_policy.reset();
                target = Some(id);
            }
            Next::RetryWifi {
                selection_id,
                retry,
            } => {
                views
                    .connected
                    .status
                    .set("Wi-Fi control interrupted - retrying connection...");
                tracing::info!(
                    attempt = retry.attempt,
                    retry_ms = retry.delay.as_millis(),
                    "Wi-Fi session transport dropped; rebuilding the complete tunnel"
                );
                target = Some(selection_id);
                tokio::time::sleep(retry.delay).await;
            }
            Next::Pair {
                selection_id,
                reply,
            } => {
                retry_policy.reset();
                let requested = selection_id.clone();
                target = pair_request(selection_id, reply, &endpoints, &views.connected)
                    .await
                    .then_some(requested);
                host.discovery.invalidate();
            }
            Next::Forget {
                selection_id,
                reply,
            } => {
                retry_policy.reset();
                forget_request(
                    selection_id,
                    reply,
                    &endpoints,
                    &views.connected,
                    &mut host.discovery,
                )
                .await;
                target = None;
                host.discovery.invalidate();
            }
            Next::Idle => {
                retry_policy.reset();
                target = None;
            }
            Next::Quit => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Next, SessionRetry, interrupts_active_session};

    #[test]
    fn user_transitions_stop_the_active_session_before_handoff() {
        let (pair_reply, _) = tokio::sync::oneshot::channel();
        let (forget_reply, _) = tokio::sync::oneshot::channel();
        assert!(interrupts_active_session(&Next::Switch("other".into())));
        assert!(interrupts_active_session(&Next::Pair {
            selection_id: "usb:device".into(),
            reply: pair_reply,
        }));
        assert!(interrupts_active_session(&Next::Forget {
            selection_id: "usb:device".into(),
            reply: forget_reply,
        }));
        assert!(interrupts_active_session(&Next::Quit));
        assert!(!interrupts_active_session(&Next::Idle));
        assert!(!interrupts_active_session(&Next::RetryWifi {
            selection_id: "wifi:device".into(),
            retry: SessionRetry {
                attempt: 1,
                delay: Duration::from_secs(1),
            },
        }));
    }
}
