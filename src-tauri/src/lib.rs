mod audio_output;
mod build_info;
mod device_runtime;
mod diagnostics;
mod mcp;
mod session;
mod settings;
mod web;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::oneshot;

struct BackendHandle {
    origin: String,
    token: String,
    runtime: device_runtime::DeviceRuntime,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

struct ProfileDirectory(PathBuf);

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
async fn open_profile_directory(state: tauri::State<'_, ProfileDirectory>) -> Result<(), String> {
    tokio::fs::create_dir_all(&state.0)
        .await
        .map_err(|error| format!("cannot create key mapping directory: {error}"))?;
    tauri_plugin_opener::open_path(&state.0, None::<&str>)
        .map_err(|error| format!("cannot open key mapping directory: {error}"))
}

#[tauri::command]
fn frontend_log(event: diagnostics::FrontendLogEvent) -> Result<(), String> {
    diagnostics::record_frontend_event(event)
}

#[tauri::command]
fn app_settings_status(
    state: tauri::State<'_, Arc<settings::AppSettings>>,
) -> settings::SettingsStatus {
    state.status()
}

#[tauri::command]
fn set_audio_enabled(
    enabled: bool,
    state: tauri::State<'_, Arc<settings::AppSettings>>,
) -> Result<settings::SettingsStatus, String> {
    state.set_audio_enabled(enabled)
}

#[tauri::command]
fn set_audio_playback(
    muted: bool,
    volume: f32,
    settings: tauri::State<'_, Arc<settings::AppSettings>>,
    output: tauri::State<'_, audio_output::AudioOutput>,
) -> Result<settings::SettingsStatus, String> {
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
) -> Result<settings::SettingsStatus, String> {
    state.set_clipboard_sync_enabled(enabled)
}

impl BackendHandle {
    fn stop(&self) {
        if let Some(shutdown) = self.shutdown.lock().unwrap().take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
        self.runtime.stop();
    }
}

fn spawn_backend(
    profile_dir: PathBuf,
    runtime_config: device_runtime::RuntimeConfig,
) -> Result<BackendHandle, String> {
    let runtime = device_runtime::DeviceRuntime::start(runtime_config)?;
    let client = runtime.client();
    let runtime_control = client.manager.control.clone();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let token = uuid::Uuid::new_v4().simple().to_string();
    let server_token = token.clone();
    let websocket_config = devicehub_server::websocket::WebSocketConfig::new(
        devicehub_runtime::configured_in_flight_frames(
            std::env::var_os("DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES").as_deref(),
        ),
    );

    let thread = std::thread::Builder::new()
        .name("devicehub-private-server".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build private server runtime");
            runtime.block_on(async move {
                tokio::spawn(mcp::serve(client.clone()));
                let app = web::router(
                    devicehub_host::private_api::state(
                        client.clone(),
                        profile_dir,
                        websocket_config,
                    ),
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
                if let Err(error) = server.await {
                    tracing::error!("control API stopped: {error}");
                }
            });
            let _ = runtime_control.send(device_runtime::ControlCmd::Quit);
        })
        .map_err(|error| format!("cannot start private server thread: {error}"))?;

    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(origin)) => Ok(BackendHandle {
            origin,
            token,
            runtime,
            shutdown: Mutex::new(Some(shutdown_tx)),
            thread: Mutex::new(Some(thread)),
        }),
        Ok(Err(error)) => {
            let _ = shutdown_tx.send(());
            let _ = thread.join();
            runtime.stop();
            Err(error)
        }
        Err(error) => {
            let _ = shutdown_tx.send(());
            let _ = thread.join();
            runtime.stop();
            Err(format!("CoreDevice backend did not start: {error}"))
        }
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
            build_info::build_info,
            build_info::check_for_update,
            diagnostics_status,
            set_debug_logging,
            open_log_directory,
            open_profile_directory,
            frontend_log,
            app_settings_status,
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
            let runtime_preferences = settings.runtime_preferences();
            let audio_output = audio_output::AudioOutput::spawn(
                audio_settings.audio_muted,
                audio_settings.audio_volume,
            )
            .map_err(std::io::Error::other)?;
            app.manage(audio_output.clone());
            app.manage(settings.clone());
            let app_data_dir = app.path().app_data_dir()?;
            let resource_dir = app.path().resource_dir().ok();
            let current_exe = std::env::current_exe().ok();
            let audio_decoder = devicehub_host::decode::AudioDecoderConfig::from_host(
                std::env::var_os("DEVICEHUB_FFMPEG"),
                std::env::var_os("PATH"),
                resource_dir.as_deref(),
                current_exe.as_deref(),
            );
            let session_diagnostics = device_runtime::RuntimeSessionDiagnostics {
                send_frame_ack: std::env::var("DEVICEHUB_FRAME_ACK").is_ok(),
                rtcp: devicehub_runtime::RtcpOptions {
                    send_rctl: std::env::var("DEVICEHUB_RCTL").is_ok(),
                },
                hevc_dump: std::env::var("DEVICEHUB_DUMP_HEVC").ok().map(PathBuf::from),
                hid_dump: std::env::var("DEVICEHUB_HID_DUMP").ok().map(PathBuf::from),
            };
            let system_usbmuxd = std::env::var("USBMUXD_SOCKET_ADDRESS").ok();
            let netmuxd = devicehub_host::netmuxd::NetmuxdConfig::from_host(
                std::env::var_os("DEVICEHUB_NETMUXD"),
                std::env::var("DEVICEHUB_NETMUXD_LOG").ok(),
                system_usbmuxd.clone(),
                std::env::var_os("PATH"),
                resource_dir.as_deref(),
                current_exe.as_deref(),
            );
            let transport = session::DeviceTransportConfig::from_host(netmuxd, system_usbmuxd);
            let profile_dir = std::env::var_os("DEVICEHUB_PROFILE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| app_data_dir.join("profiles"));
            app.manage(ProfileDirectory(profile_dir.clone()));
            let backend = spawn_backend(
                profile_dir,
                device_runtime::RuntimeConfig {
                    initial_udid,
                    pairing_dir: app_data_dir.join("pairings"),
                    transport,
                    preferences: runtime_preferences,
                    audio: device_runtime::AudioPublisher::new(audio_output),
                    audio_decoder,
                    session_diagnostics,
                },
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
