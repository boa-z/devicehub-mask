//! Native headless host for the shared DeviceHub runtime and browser UI.

mod discovery;
mod host;

use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use devicehub_runtime::{HostClipboard, RuntimeHostAdapters};

#[derive(Debug)]
struct Config {
    listen: SocketAddr,
    allow_lan: bool,
    data_dir: PathBuf,
    frontend_dir: PathBuf,
    token_file: Option<PathBuf>,
    initial_device: Option<String>,
    ffmpeg: Option<OsString>,
    netmuxd: Option<OsString>,
    system_usbmuxd: Option<String>,
    mcp_listen: Option<SocketAddr>,
}

impl Config {
    fn parse() -> Result<Self, String> {
        let current_dir = std::env::current_dir()
            .map_err(|error| format!("cannot resolve current directory: {error}"))?;
        let mut config = Self {
            listen: "127.0.0.1:8080".parse().unwrap(),
            allow_lan: false,
            data_dir: current_dir.join(".devicehub-mask"),
            frontend_dir: current_dir.join("dist"),
            token_file: None,
            initial_device: None,
            ffmpeg: None,
            netmuxd: None,
            system_usbmuxd: None,
            mcp_listen: None,
        };
        let mut args = std::env::args_os().skip(1);
        while let Some(argument) = args.next() {
            let argument = argument
                .to_str()
                .ok_or_else(|| "arguments must be valid UTF-8".to_string())?;
            match argument {
                "--allow-lan" => config.allow_lan = true,
                "--help" | "-h" => return Err(usage().into()),
                "--listen" => {
                    config.listen = value(&mut args, argument)?
                        .parse()
                        .map_err(|error| format!("invalid --listen address: {error}"))?
                }
                "--data-dir" => config.data_dir = PathBuf::from(value_os(&mut args, argument)?),
                "--frontend-dir" => {
                    config.frontend_dir = PathBuf::from(value_os(&mut args, argument)?)
                }
                "--token-file" => {
                    config.token_file = Some(PathBuf::from(value_os(&mut args, argument)?))
                }
                "--device" => config.initial_device = Some(value(&mut args, argument)?),
                "--ffmpeg" => config.ffmpeg = Some(value_os(&mut args, argument)?),
                "--netmuxd" => config.netmuxd = Some(value_os(&mut args, argument)?),
                "--usbmuxd" => config.system_usbmuxd = Some(value(&mut args, argument)?),
                "--mcp-listen" => {
                    config.mcp_listen = Some(
                        value(&mut args, argument)?
                            .parse()
                            .map_err(|error| format!("invalid --mcp-listen address: {error}"))?,
                    )
                }
                unknown => return Err(format!("unknown argument {unknown}\n\n{}", usage())),
            }
        }
        validate_network_policy(config.listen, config.allow_lan, config.mcp_listen)?;
        if !config.frontend_dir.join("index.html").is_file() {
            return Err(format!(
                "frontend build is missing at {}; run `npm run build` or pass --frontend-dir",
                config.frontend_dir.display()
            ));
        }
        Ok(config)
    }
}

fn validate_network_policy(
    listen: SocketAddr,
    allow_lan: bool,
    mcp_listen: Option<SocketAddr>,
) -> Result<(), String> {
    if !listen.ip().is_loopback() && !allow_lan {
        return Err("non-loopback --listen requires the explicit --allow-lan flag".into());
    }
    if mcp_listen.is_some_and(|address| !address.ip().is_loopback()) {
        return Err("MCP has no authentication and may only listen on loopback".into());
    }
    Ok(())
}

fn value(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<String, String> {
    value_os(args, flag)?
        .into_string()
        .map_err(|_| format!("{flag} must be valid UTF-8"))
}

fn value_os(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<OsString, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn usage() -> &'static str {
    "Usage: devicehub-headless [options]\n\
     \n\
     --listen <IP:PORT>       HTTP listener (default: 127.0.0.1:8080)\n\
     --allow-lan              Required for a non-loopback HTTP listener\n\
     --data-dir <PATH>        Pairings and profiles (default: ./.devicehub-mask)\n\
     --frontend-dir <PATH>    Built Vite UI (default: ./dist)\n\
     --token-file <PATH>      Read a persistent API token from a protected file\n\
     --device <IDENTIFIER>    Initially selected device\n\
     --ffmpeg <PATH>          FFmpeg executable override\n\
     --netmuxd <PATH|off>     netmuxd executable override\n\
     --usbmuxd <ADDRESS>      System usbmuxd address override\n\
     --mcp-listen <IP:PORT>   Optional loopback-only MCP listener"
}

#[derive(Clone, Copy)]
struct UnavailableClipboard;

impl devicehub_runtime::HostClipboardProvider for UnavailableClipboard {
    fn connect(&self) -> Result<Box<dyn HostClipboard>, String> {
        Err("host clipboard integration is unavailable in headless mode".into())
    }
}

fn read_token(path: Option<&Path>) -> Result<String, String> {
    let Some(path) = path else {
        return Ok(uuid::Uuid::new_v4().simple().to_string());
    };
    #[cfg(unix)]
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("cannot inspect token file {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "token file {} must not be accessible by group or other users (use chmod 600)",
                path.display()
            ));
        }
    }
    let token = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read token file {}: {error}", path.display()))?;
    let token = token.trim();
    if token.len() < 24
        || !token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("token file must contain one URL-safe token of at least 24 characters".into());
    }
    Ok(token.to_owned())
}

fn display_host(ip: IpAddr) -> String {
    if ip.is_unspecified() {
        "127.0.0.1".into()
    } else if ip.is_ipv6() {
        format!("[{ip}]")
    } else {
        ip.to_string()
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    if std::env::args_os()
        .skip(1)
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("{}", usage());
        return;
    }
    let config = match Config::parse() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    if let Err(error) = run(config).await {
        tracing::error!(%error, "headless host stopped");
        std::process::exit(1);
    }
}

async fn run(config: Config) -> Result<(), String> {
    std::fs::create_dir_all(&config.data_dir)
        .map_err(|error| format!("cannot create data directory: {error}"))?;
    let transfer_dir = config.data_dir.join("transfers");
    match tokio::fs::remove_dir_all(&transfer_dir).await {
        Ok(()) => {
            tracing::info!(path = %transfer_dir.display(), "removed stale browser transfer staging")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot clean browser transfer staging: {error}")),
    }
    let token = read_token(config.token_file.as_deref())?;
    let pairing_dir = config.data_dir.join("pairings");
    let profile_dir = config.data_dir.join("profiles");
    let current_exe = std::env::current_exe().ok();
    let search_path = std::env::var_os("PATH");
    let audio_decoder = devicehub_host::decode::AudioDecoderConfig::from_host(
        config.ffmpeg,
        search_path.clone(),
        None,
        current_exe.as_deref(),
    );
    let netmuxd = devicehub_host::netmuxd::NetmuxdConfig::from_host(
        config.netmuxd,
        None,
        config.system_usbmuxd.clone(),
        search_path,
        None,
        current_exe.as_deref(),
    );
    let system_usbmuxd = devicehub_runtime::SystemUsbmuxdConfig::from_host(config.system_usbmuxd);
    let host_control = host::HeadlessHostControl::load(config.data_dir.join("settings.json"));
    let preferences = host_control.runtime_preferences();
    let browser_audio = devicehub_server::websocket::BrowserAudioSlot::default();
    let runtime_browser_audio = browser_audio.clone();
    let diagnostics = devicehub_runtime::SessionDiagnostics {
        send_frame_ack: false,
        rtcp: devicehub_runtime::RtcpOptions::default(),
        hevc_dump: None,
        hid_dump: None,
    };
    let started = devicehub_runtime::start_runtime(
        move || {
            let pairing_store =
                devicehub_host::wifi_devices::HostPairingStore::new(pairing_dir.clone())
                    .map_err(|error| tracing::warn!(%error, "Wi-Fi pairing storage unavailable"))
                    .ok();
            RuntimeHostAdapters {
                sidecar: devicehub_host::netmuxd::NetmuxdSupervisor::new(pairing_dir, netmuxd),
                pairing_store,
                system_usbmuxd,
                audio: devicehub_host::decode::FfmpegAudioPipelineFactory::new(
                    devicehub_runtime::AudioPublisher::new(host::BrowserPcmConsumer(
                        runtime_browser_audio.clone(),
                    )),
                    audio_decoder,
                )
                .all_sessions(),
                diagnostic_sinks: devicehub_host::diagnostic_sinks::TokioDiagnosticDumpSinks,
                clipboard: UnavailableClipboard,
                services: devicehub_host::session_adapters(),
            }
        },
        config.initial_device,
        preferences,
        diagnostics,
    )?;
    let (runtime, client) = started.into_parts();
    let mut api_state = devicehub_host::private_api::state(
        client.clone(),
        profile_dir,
        devicehub_server::websocket::WebSocketConfig::default(),
    );
    api_state.host_http = devicehub_server::http::HostHttpState::new(
        host::capabilities(),
        host::build_info(),
        host_control,
    );
    let browser_transfers =
        devicehub_host::browser_transfers::TokioBrowserTransferStore::new(transfer_dir);
    api_state.storage_http = api_state
        .storage_http
        .with_browser_transfers(browser_transfers.clone());
    api_state.crash_reports_http = api_state
        .crash_reports_http
        .with_browser_transfers(browser_transfers);
    api_state.browser_audio = Some(browser_audio);
    let api = devicehub_server::private_api::router(api_state, token.clone());
    let app = devicehub_server::spa::router(api, config.frontend_dir);
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|error| format!("cannot bind {}: {error}", config.listen))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("cannot read listener address: {error}"))?;
    let url = format!(
        "http://{}:{}/#access_token={token}",
        display_host(address.ip()),
        address.port()
    );
    tracing::info!(listen = %address, lan = config.allow_lan, "headless server listening");
    let _service_advertiser = if config.allow_lan && !address.ip().is_loopback() {
        match discovery::ServiceAdvertiser::start(address.port()) {
            Ok(advertiser) => Some(advertiser),
            Err(error) => {
                tracing::warn!(%error, "LAN discovery advertisement unavailable");
                None
            }
        }
    } else {
        None
    };
    println!("Open {url}");

    let mcp = config.mcp_listen.map(|address| {
        let router = devicehub_server::mcp::router(client);
        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(address).await {
                Ok(listener) => {
                    tracing::info!(%address, "MCP server listening");
                    if let Err(error) = axum::serve(listener, router).await {
                        tracing::error!(%error, "MCP server stopped");
                    }
                }
                Err(error) => tracing::error!(%address, %error, "MCP server failed to bind"),
            }
        })
    });
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(%error, "cannot install shutdown signal handler");
            }
        })
        .await
        .map_err(|error| format!("HTTP server failed: {error}"));
    if let Some(mcp) = mcp {
        mcp.abort();
    }
    runtime.stop();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_exposure_requires_explicit_lan_opt_in() {
        assert!(validate_network_policy("127.0.0.1:8080".parse().unwrap(), false, None).is_ok());
        assert!(validate_network_policy("0.0.0.0:8080".parse().unwrap(), false, None).is_err());
        assert!(validate_network_policy("0.0.0.0:8080".parse().unwrap(), true, None).is_ok());
    }

    #[test]
    fn unauthenticated_mcp_is_always_loopback_only() {
        assert!(
            validate_network_policy(
                "127.0.0.1:8080".parse().unwrap(),
                false,
                Some("127.0.0.1:8009".parse().unwrap()),
            )
            .is_ok()
        );
        assert!(
            validate_network_policy(
                "127.0.0.1:8080".parse().unwrap(),
                false,
                Some("0.0.0.0:8009".parse().unwrap()),
            )
            .is_err()
        );
    }
}
