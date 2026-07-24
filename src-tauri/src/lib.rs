mod app_documents;
mod app_icons;
mod audio_output;
mod browser_video;
mod companion_devices;
mod crash_reports;
mod decode;
mod demand;
mod developer_mode;
mod device_conditions;
mod device_events;
mod device_logs;
mod diagnostics;
mod heartbeat;
mod hid;
mod home_screen;
mod location;
mod mcp;
mod netmuxd;
mod network_capture;
mod performance;
mod protocol;
mod provisioning;
mod screen_capture;
mod session;
mod settings;
mod supervisor;
mod web;
mod wifi_devices;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use protocol::{
    ActiveSlot, AppOperationSlot, ClipboardSlot, ControlCmd, DeviceListSlot, ErrorSlot, FrameSlot,
    InputSink, LocationStatusSlot, OrientationSlot, StatusSlot, VideoCounters,
};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

// RSD handshakes decode nested XPC dictionaries recursively. The device owner
// also hosts a LocalSet for non-Send DVT channels, so the platform thread
// default (2 MiB on macOS) is not enough for larger iOS 27 service catalogs.
const COREDEVICE_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;

struct BackendHandle {
    control: mpsc::UnboundedSender<ControlCmd>,
    origin: String,
    token: String,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Serialize)]
struct BackendConnection {
    origin: String,
    token: String,
}

#[tauri::command]
fn backend_connection(state: tauri::State<'_, BackendHandle>) -> BackendConnection {
    BackendConnection {
        origin: state.origin.clone(),
        token: state.token.clone(),
    }
}

#[tauri::command]
fn diagnostics_status(
    state: tauri::State<'_, diagnostics::Diagnostics>,
) -> diagnostics::DiagnosticsStatus {
    state.status()
}

#[tauri::command]
fn set_debug_logging(
    enabled: bool,
    state: tauri::State<'_, diagnostics::Diagnostics>,
) -> Result<diagnostics::DiagnosticsStatus, String> {
    state.set_debug_enabled(enabled)
}

#[tauri::command]
fn open_log_directory(state: tauri::State<'_, diagnostics::Diagnostics>) -> Result<(), String> {
    state.open_log_directory()
}

#[tauri::command]
fn frontend_log(event: diagnostics::FrontendLogEvent) -> Result<(), String> {
    diagnostics::record_frontend_event(event)
}

#[tauri::command]
fn video_settings_status(
    state: tauri::State<'_, Arc<settings::AppSettings>>,
) -> settings::VideoSettingsStatus {
    state.status()
}

#[tauri::command]
fn set_video_pixel_format(
    video_pixel_format: protocol::FrameFormat,
    state: tauri::State<'_, Arc<settings::AppSettings>>,
) -> Result<settings::VideoSettingsStatus, String> {
    state.set_video_pixel_format(video_pixel_format)
}

#[tauri::command]
fn set_video_decoder_backend(
    video_decoder_backend: settings::VideoDecoderBackend,
    state: tauri::State<'_, Arc<settings::AppSettings>>,
) -> Result<settings::VideoSettingsStatus, String> {
    state.set_video_decoder_backend(video_decoder_backend)
}

#[tauri::command]
fn set_audio_enabled(
    enabled: bool,
    state: tauri::State<'_, Arc<settings::AppSettings>>,
) -> Result<settings::VideoSettingsStatus, String> {
    state.set_audio_enabled(enabled)
}

#[tauri::command]
fn set_audio_playback(
    muted: bool,
    volume: f32,
    settings: tauri::State<'_, Arc<settings::AppSettings>>,
    output: tauri::State<'_, audio_output::AudioOutput>,
) -> Result<settings::VideoSettingsStatus, String> {
    let status = settings.set_audio_playback(muted, volume)?;
    output.set_preferences(status.audio_muted, status.audio_volume)?;
    Ok(status)
}

#[tauri::command]
fn audio_output_status(
    output: tauri::State<'_, audio_output::AudioOutput>,
) -> audio_output::AudioOutputStatus {
    output.status()
}

#[tauri::command]
fn set_clipboard_sync_enabled(
    enabled: bool,
    state: tauri::State<'_, Arc<settings::AppSettings>>,
) -> Result<settings::VideoSettingsStatus, String> {
    state.set_clipboard_sync_enabled(enabled)
}

impl BackendHandle {
    fn stop(&self) {
        let _ = self.control.send(ControlCmd::Quit);
        if let Some(shutdown) = self.shutdown.lock().unwrap().take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }
}

fn spawn_backend(
    initial_udid: Option<String>,
    profile_dir: PathBuf,
    pairing_dir: PathBuf,
    resource_dir: Option<PathBuf>,
    settings: Arc<settings::AppSettings>,
    audio: audio_output::AudioOutput,
) -> Result<BackendHandle, String> {
    let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlCmd>();
    let thread_control = control_tx.clone();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let token = uuid::Uuid::new_v4().simple().to_string();
    let server_token = token.clone();

    let thread = std::thread::Builder::new()
        .name("devicehub-coredevice".into())
        .stack_size(COREDEVICE_THREAD_STACK_BYTES)
        .spawn(move || {
            tracing::info!(
                stack_bytes = COREDEVICE_THREAD_STACK_BYTES,
                "CoreDevice owner thread started"
            );
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build CoreDevice runtime");
            let device_tasks = tokio::task::LocalSet::new();
            runtime.block_on(device_tasks.run_until(async move {
                let frames = FrameSlot::default();
                let browser_frames = browser_video::BrowserVideoSlot::default();
                let video_counters = VideoCounters::default();
                let status = StatusSlot::default();
                let clipboard = ClipboardSlot::default();
                let device_events = device_events::DeviceEventSlot::default();
                let network_capture = network_capture::NetworkCaptureSlot::default();
                let device_conditions = device_conditions::DeviceConditionSlot::default();
                let orientation = OrientationSlot::default();
                let devices = DeviceListSlot::default();
                let active = ActiveSlot::default();
                let error = ErrorSlot::default();
                let input = InputSink::default();
                let app_operation = AppOperationSlot::default();
                let location = LocationStatusSlot::default();
                let performance = performance::PerformanceSlot::default();
                let performance_demand = performance::PerformanceDemand::default();
                let device_logs = device_logs::DeviceLogSlot::default();
                let device_log_demand = device_logs::DeviceLogDemand::default();
                let services = supervisor::ServiceRegistry::default();

                tokio::spawn(mcp::serve(
                    frames.clone(),
                    browser_frames.clone(),
                    input.clone(),
                    orientation.clone(),
                    devices.clone(),
                    active.clone(),
                    error.clone(),
                    status.clone(),
                    location.clone(),
                    device_events.clone(),
                    performance.clone(),
                    performance_demand.clone(),
                    device_logs.clone(),
                    device_log_demand.clone(),
                    thread_control.clone(),
                ));

                let manager = session::manage(
                    initial_udid,
                    pairing_dir,
                    resource_dir,
                    settings.clone(),
                    video_counters.clone(),
                    || {},
                    frames.clone(),
                    browser_frames.clone(),
                    audio.clone(),
                    status.clone(),
                    clipboard.clone(),
                    device_events.clone(),
                    network_capture.clone(),
                    device_conditions.clone(),
                    orientation.clone(),
                    devices.clone(),
                    active.clone(),
                    error.clone(),
                    app_operation.clone(),
                    location.clone(),
                    performance.clone(),
                    performance_demand.clone(),
                    device_logs.clone(),
                    device_log_demand.clone(),
                    services.clone(),
                    input.clone(),
                    control_rx,
                );
                let app = web::router(
                    web::AppState {
                        frames,
                        browser_frames,
                        clipboard,
                        device_events,
                        network_capture,
                        device_conditions,
                        video_counters,
                        status,
                        orientation,
                        devices,
                        active,
                        error,
                        app_operation,
                        location,
                        performance,
                        performance_demand,
                        device_logs,
                        device_log_demand,
                        services,
                        input,
                        control: thread_control.clone(),
                        profile_dir: Arc::new(profile_dir),
                        settings,
                    },
                    server_token,
                );

                let address =
                    std::env::var("DEVICEHUB_ADDR").unwrap_or_else(|_| "127.0.0.1:0".into());
                let listener = match tokio::net::TcpListener::bind(&address).await {
                    Ok(listener) => listener,
                    Err(error) => {
                        let _ = ready_tx.send(Err(format!(
                            "cannot bind CoreDevice API at {address}: {error}"
                        )));
                        return;
                    }
                };
                let local_address = match listener.local_addr() {
                    Ok(address) => address,
                    Err(error) => {
                        let _ = ready_tx.send(Err(format!("cannot read backend address: {error}")));
                        return;
                    }
                };
                let origin = format!("http://{local_address}");
                let _ = ready_tx.send(Ok(origin.clone()));
                tracing::info!("private Tauri backend listening on {origin}");

                let server = axum::serve(listener, app).with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                });
                tokio::select! {
                    result = server => {
                        if let Err(error) = result {
                            tracing::error!("control API stopped: {error}");
                        }
                    }
                    _ = manager => tracing::warn!("device manager stopped"),
                }
                let _ = thread_control.send(ControlCmd::Quit);
            }));
        })
        .map_err(|error| format!("cannot start CoreDevice thread: {error}"))?;

    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(origin)) => Ok(BackendHandle {
            control: control_tx,
            origin,
            token,
            shutdown: Mutex::new(Some(shutdown_tx)),
            thread: Mutex::new(Some(thread)),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(error) => Err(format!("CoreDevice backend did not start: {error}")),
    }
}

pub fn run() {
    use tauri::Manager;

    let initial_udid = std::env::args().nth(1);
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            backend_connection,
            diagnostics_status,
            set_debug_logging,
            open_log_directory,
            frontend_log,
            video_settings_status,
            set_video_pixel_format,
            set_video_decoder_backend,
            set_audio_enabled,
            set_audio_playback,
            audio_output_status,
            set_clipboard_sync_enabled
        ])
        .setup(move |app| {
            let log_directory = app.path().app_log_dir()?;
            let diagnostics =
                diagnostics::Diagnostics::init(log_directory).map_err(std::io::Error::other)?;
            app.manage(diagnostics);
            let settings = Arc::new(settings::AppSettings::load(
                app.path().app_config_dir()?.join("settings.json"),
            ));
            let audio_settings = settings.status();
            let audio_output = audio_output::AudioOutput::spawn(
                audio_settings.audio_muted,
                audio_settings.audio_volume,
            )
            .map_err(std::io::Error::other)?;
            app.manage(audio_output.clone());
            app.manage(settings.clone());
            let app_data_dir = app.path().app_data_dir()?;
            let resource_dir = app.path().resource_dir().ok();
            let profile_dir = std::env::var_os("DEVICEHUB_PROFILE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| app_data_dir.join("profiles"));
            let backend = spawn_backend(
                initial_udid,
                profile_dir,
                app_data_dir.join("pairings"),
                resource_dir,
                settings,
                audio_output,
            )
            .map_err(std::io::Error::other)?;
            app.manage(backend);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("build Tauri application");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            tracing::info!("application exiting");
            app_handle.state::<BackendHandle>().stop();
        }
    });
}
