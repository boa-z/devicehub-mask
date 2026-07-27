//! Supervision for the optional netmuxd Wi-Fi transport sidecar.

use std::ffi::OsString;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
const RESTART_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct NetmuxdConfig {
    binary: Option<PathBuf>,
    forced: bool,
    log_filter: String,
    upstream_usbmuxd: String,
}

impl NetmuxdConfig {
    /// Resolve process-level overrides at the host boundary. The runtime owns
    /// sidecar supervision, but never reads the host process environment.
    pub fn from_host(
        configured: Option<OsString>,
        log_filter: Option<String>,
        upstream_usbmuxd: Option<String>,
        search_path: Option<OsString>,
        resource_dir: Option<&Path>,
        current_exe: Option<&Path>,
    ) -> Self {
        let forced = configured
            .as_ref()
            .is_some_and(|value| !value.is_empty() && value != "off");
        Self {
            binary: find_binary(configured, search_path, resource_dir, current_exe),
            forced,
            log_filter: log_filter.unwrap_or_else(|| "warn".into()),
            upstream_usbmuxd: upstream_usbmuxd
                .unwrap_or_else(|| default_system_usbmuxd_address().into()),
        }
    }
}

pub struct NetmuxdSupervisor {
    binary: Option<PathBuf>,
    forced: bool,
    pairing_dir: PathBuf,
    child: Option<Child>,
    address: Option<SocketAddr>,
    retry_after: Option<Instant>,
    log_filter: String,
    upstream_usbmuxd: String,
}

impl NetmuxdSupervisor {
    pub fn new(pairing_dir: PathBuf, config: NetmuxdConfig) -> Self {
        Self {
            binary: config.binary,
            forced: config.forced,
            pairing_dir,
            child: None,
            address: None,
            retry_after: None,
            log_filter: config.log_filter,
            upstream_usbmuxd: config.upstream_usbmuxd,
        }
    }

    pub fn is_forced(&self) -> bool {
        self.forced && self.binary.is_some()
    }

    /// Return the private shim address, starting or restarting our child when needed.
    pub async fn ensure_ready(&mut self) -> Option<SocketAddr> {
        let binary = self.binary.clone()?;
        let had_child = self.child.is_some();
        if self.child_is_running() {
            return self.address;
        }
        if had_child {
            self.retry_after = Some(Instant::now() + RESTART_BACKOFF);
            return None;
        }
        if self
            .retry_after
            .is_some_and(|retry_after| Instant::now() < retry_after)
        {
            return None;
        }

        match self.start(&binary).await {
            Ok(address) => {
                self.retry_after = None;
                Some(address)
            }
            Err(error) => {
                tracing::warn!(%error, "netmuxd sidecar unavailable; using direct Wi-Fi fallback");
                self.stop_child();
                self.retry_after = Some(Instant::now() + RESTART_BACKOFF);
                None
            }
        }
    }

    fn child_is_running(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => self.address.is_some(),
            Ok(Some(status)) => {
                tracing::warn!(%status, "netmuxd sidecar exited; scheduling restart");
                self.child = None;
                self.address = None;
                false
            }
            Err(error) => {
                tracing::warn!(%error, "cannot inspect netmuxd sidecar; scheduling restart");
                self.stop_child();
                false
            }
        }
    }

    async fn start(&mut self, binary: &Path) -> Result<SocketAddr, String> {
        std::fs::create_dir_all(&self.pairing_dir)
            .map_err(|error| format!("cannot create pairing directory: {error}"))?;
        #[cfg(unix)]
        std::fs::set_permissions(&self.pairing_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot secure pairing directory: {error}"))?;
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| format!("cannot reserve netmuxd port: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("cannot read reserved netmuxd port: {error}"))?;
        drop(listener);

        let mut command = Command::new(binary);
        hide_windows_console(&mut command);
        command
            .arg("--disable-unix")
            .arg("--host")
            .arg(Ipv4Addr::LOCALHOST.to_string())
            .arg("--port")
            .arg(address.port().to_string())
            // The application already owns a supervised heartbeat. netmuxd's
            // heartbeat is racy when one Bonjour service resolves on multiple
            // interfaces and can open duplicate TLS sessions for one device.
            .arg("--disable-heartbeat")
            .arg("--upstream-usbmuxd")
            .arg(&self.upstream_usbmuxd)
            .arg("--plist-storage")
            .arg(&self.pairing_dir)
            .env("RUST_LOG", &self.log_filter)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", binary.display()))?;
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(forward_output(stdout, false));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(forward_output(stderr, true));
        }
        let child_id = child.id();
        self.child = Some(child);
        self.address = Some(address);

        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if tokio::net::TcpStream::connect(address).await.is_ok() {
                tracing::info!(
                    path = %binary.display(),
                    ?child_id,
                    %address,
                    "netmuxd Wi-Fi transport ready"
                );
                return Ok(address);
            }
            if !self.child_is_running() {
                return Err("netmuxd exited before its listener became ready".into());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "netmuxd listener at {address} did not become ready"
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn stop_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        self.address = None;
    }
}

impl devicehub_runtime::MuxSidecar for NetmuxdSupervisor {
    fn is_forced(&self) -> bool {
        NetmuxdSupervisor::is_forced(self)
    }

    fn ensure_ready(&mut self) -> devicehub_runtime::MuxSidecarFuture<'_> {
        Box::pin(NetmuxdSupervisor::ensure_ready(self))
    }
}

#[cfg(windows)]
fn hide_windows_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.as_std_mut().creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_windows_console(_command: &mut Command) {}

impl Drop for NetmuxdSupervisor {
    fn drop(&mut self) {
        self.stop_child();
    }
}

fn find_binary(
    configured: Option<OsString>,
    search_path: Option<OsString>,
    resource_dir: Option<&Path>,
    current_exe: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(value) = configured {
        if value.is_empty() || value == "off" {
            tracing::info!("netmuxd sidecar disabled by DEVICEHUB_NETMUXD");
            return None;
        }
        return Some(PathBuf::from(value));
    }

    let name = if cfg!(windows) {
        "netmuxd.exe"
    } else {
        "netmuxd"
    };
    if let Some(path) = resource_dir.map(|directory| directory.join(name))
        && path.is_file()
    {
        return Some(path);
    }
    if let Some(parent) = current_exe.and_then(Path::parent) {
        let path = parent.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    path_binary(search_path, name).or_else(|| {
        tracing::info!(
            "netmuxd sidecar not installed; set DEVICEHUB_NETMUXD or use a packaged build"
        );
        None
    })
}

fn path_binary(paths: Option<OsString>, name: &str) -> Option<PathBuf> {
    let paths = paths?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

fn default_system_usbmuxd_address() -> &'static str {
    if cfg!(unix) {
        "/var/run/usbmuxd"
    } else {
        "127.0.0.1:27015"
    }
}

async fn forward_output<R>(reader: R, stderr: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if stderr {
            tracing::debug!(target: "devicehub_mask::netmuxd_sidecar", %line);
        } else {
            tracing::trace!(target: "devicehub_mask::netmuxd_sidecar", %line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_disable_has_no_binary() {
        let config = NetmuxdConfig::from_host(Some("off".into()), None, None, None, None, None);
        assert!(config.binary.is_none());
        assert!(!config.forced);
    }

    #[test]
    fn explicit_binary_is_forced_without_environment_reads() {
        let binary = PathBuf::from("/configured/netmuxd");
        let config = NetmuxdConfig::from_host(
            Some(binary.clone().into_os_string()),
            Some("debug".into()),
            Some("configured-usbmuxd".into()),
            None,
            None,
            None,
        );
        assert_eq!(config.binary, Some(binary));
        assert!(config.forced);
        assert_eq!(config.log_filter, "debug");
        assert_eq!(config.upstream_usbmuxd, "configured-usbmuxd");
    }

    #[test]
    fn private_address_is_loopback() {
        let address = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 1234);
        assert!(address.ip().is_loopback());
    }
}
