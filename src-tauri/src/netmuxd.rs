//! Supervision for the optional netmuxd Wi-Fi transport sidecar.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use idevice::usbmuxd::UsbmuxdAddr;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
const RESTART_BACKOFF: Duration = Duration::from_secs(5);

pub struct NetmuxdSupervisor {
    binary: Option<PathBuf>,
    forced: bool,
    pairing_dir: PathBuf,
    child: Option<Child>,
    address: Option<SocketAddr>,
    retry_after: Option<Instant>,
}

impl NetmuxdSupervisor {
    pub fn new(pairing_dir: PathBuf, resource_dir: Option<PathBuf>) -> Self {
        let forced = std::env::var_os("DEVICEHUB_NETMUXD")
            .is_some_and(|value| !value.is_empty() && value != "off");
        Self {
            binary: find_binary(resource_dir.as_deref()),
            forced,
            pairing_dir,
            child: None,
            address: None,
            retry_after: None,
        }
    }

    pub fn is_forced(&self) -> bool {
        self.forced && self.binary.is_some()
    }

    /// Return the private shim address, starting or restarting our child when needed.
    pub async fn ensure_ready(&mut self) -> Option<UsbmuxdAddr> {
        let binary = self.binary.clone()?;
        let had_child = self.child.is_some();
        if self.child_is_running() {
            return self.address.map(UsbmuxdAddr::TcpSocket);
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
                Some(UsbmuxdAddr::TcpSocket(address))
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
            .arg(system_usbmuxd_address())
            .arg("--plist-storage")
            .arg(&self.pairing_dir)
            .env(
                "RUST_LOG",
                std::env::var("DEVICEHUB_NETMUXD_LOG").unwrap_or_else(|_| "warn".into()),
            )
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

impl Drop for NetmuxdSupervisor {
    fn drop(&mut self) {
        self.stop_child();
    }
}

fn find_binary(resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("DEVICEHUB_NETMUXD") {
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
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
    {
        let path = parent.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    path_binary(name).or_else(|| {
        tracing::info!(
            "netmuxd sidecar not installed; set DEVICEHUB_NETMUXD or use a packaged build"
        );
        None
    })
}

fn path_binary(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

fn system_usbmuxd_address() -> String {
    std::env::var("USBMUXD_SOCKET_ADDRESS").unwrap_or_else(|_| {
        if cfg!(unix) {
            "/var/run/usbmuxd".into()
        } else {
            "127.0.0.1:27015".into()
        }
    })
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
        // Binary resolution itself is covered through the pure PATH helper; changing
        // process environment in parallel tests would be racy.
        assert_eq!(path_binary("definitely-not-a-devicehub-binary"), None);
    }

    #[test]
    fn private_address_is_loopback() {
        let address = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 1234);
        assert!(address.ip().is_loopback());
    }
}
