mod decode;
mod hid;
mod protocol;
mod provisioning;
mod session;
mod web;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use protocol::{
    ActiveSlot, ClipboardSlot, ControlCmd, DeviceListSlot, ErrorSlot, FrameSlot, InputSink,
    OrientationSlot, StatusSlot,
};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

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

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "devicehub_mask=info,devicehub_mask::session=info".into()),
        )
        .try_init();
}

fn spawn_backend(
    initial_udid: Option<String>,
    profile_dir: PathBuf,
) -> Result<BackendHandle, String> {
    let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlCmd>();
    let thread_control = control_tx.clone();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let token = uuid::Uuid::new_v4().simple().to_string();
    let server_token = token.clone();

    let thread = std::thread::Builder::new()
        .name("devicehub-coredevice".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build CoreDevice runtime");
            runtime.block_on(async move {
                let frames = FrameSlot::default();
                let status = StatusSlot::default();
                let clipboard = ClipboardSlot::default();
                let orientation = OrientationSlot::default();
                let devices = DeviceListSlot::default();
                let active = ActiveSlot::default();
                let error = ErrorSlot::default();
                let input = InputSink::default();

                let manager = session::manage(
                    initial_udid,
                    || {},
                    frames.clone(),
                    status.clone(),
                    clipboard,
                    orientation.clone(),
                    devices.clone(),
                    active.clone(),
                    error.clone(),
                    input.clone(),
                    control_rx,
                );
                let app = web::router(
                    web::AppState {
                        frames,
                        status,
                        orientation,
                        devices,
                        active,
                        error,
                        input,
                        control: thread_control.clone(),
                        profile_dir: Arc::new(profile_dir),
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
            });
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

    init_tracing();
    let initial_udid = std::env::args().nth(1);
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![backend_connection])
        .setup(move |app| {
            let profile_dir = std::env::var_os("DEVICEHUB_PROFILE_DIR")
                .map(PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?.join("profiles"));
            let backend =
                spawn_backend(initial_udid, profile_dir).map_err(std::io::Error::other)?;
            app.manage(backend);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("build Tauri application");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app_handle.state::<BackendHandle>().stop();
        }
    });
}
