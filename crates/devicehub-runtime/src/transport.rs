//! Explicit device transport selection and CoreDevice tunnel construction.
//!
//! This module owns the picker endpoint through provider and RSD tunnel setup.
//! It deliberately never switches USB/Wi-Fi implicitly: discovery and the
//! session manager decide when fallback or reconnect is allowed. Device trust
//! management and session task supervision remain separate concerns.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use idevice::{
    IdeviceError, IdeviceService, RemoteXpcClient,
    core_device_proxy::CoreDeviceProxy,
    provider::{IdeviceProvider, TcpProvider},
    remote_pairing::{
        RemotePairingClient, RpPairingFile, RpPairingSocket, connect_tls_psk_tunnel_native,
        errors::RemotePairingError,
    },
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdDevice},
};

use devicehub_core::{ConnKind, DeviceInfo, StatusSlot, device_id_fingerprint};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const INITIAL_WIFI_PAIRING_TIMEOUT: Duration = Duration::from_secs(120);
const REMOTE_PAIRING_VERIFY_TIMEOUT: Duration = Duration::from_secs(8);
const REMOTE_PAIRING_VERIFY_ATTEMPTS: usize = 3;
const REMOTE_PAIRING_RETRY_DELAY: Duration = Duration::from_millis(300);
pub const WIFI_REAUTHORIZE_REQUIRED: &str = "Wi-Fi control authorization is no longer accepted by the device. Connect it by USB, remove computer trust, then trust it again to create new Wi-Fi credentials.";

type WifiPairingClient = RemotePairingClient<RpPairingSocket<tokio::net::TcpStream>>;

#[derive(Clone, Debug)]
pub struct SystemUsbmuxdConfig {
    address: Result<UsbmuxdAddr, String>,
}

impl SystemUsbmuxdConfig {
    pub fn from_host(configured: Option<String>) -> Self {
        Self {
            address: parse_system_usbmuxd_address(configured),
        }
    }

    pub fn address(&self) -> Result<UsbmuxdAddr, String> {
        self.address.clone()
    }
}

fn parse_system_usbmuxd_address(configured: Option<String>) -> Result<UsbmuxdAddr, String> {
    let Some(configured) = configured else {
        return Ok(UsbmuxdAddr::default());
    };
    #[cfg(unix)]
    if !configured.contains(':') {
        return Ok(UsbmuxdAddr::UnixSocket(configured));
    }
    std::net::SocketAddr::from_str(&configured)
        .map(UsbmuxdAddr::TcpSocket)
        .map_err(|error| format!("invalid USBMUXD_SOCKET_ADDRESS {configured:?}: {error}"))
}

#[derive(Debug)]
enum WifiPairingVerificationError {
    ConnectTimeout,
    Connect(std::io::Error),
    VerifyTimeout,
    Verify(IdeviceError),
}

impl WifiPairingVerificationError {
    fn is_transient(&self) -> bool {
        match self {
            Self::ConnectTimeout | Self::VerifyTimeout => true,
            Self::Connect(error) => !matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::PermissionDenied
            ),
            Self::Verify(IdeviceError::Socket(error)) => matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
            ),
            Self::Verify(IdeviceError::Timeout) => true,
            _ => false,
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::Verify(IdeviceError::RemotePairing(
                RemotePairingError::PairVerifyFailed
                | RemotePairingError::PairingRejected(_)
                | RemotePairingError::SrpAuthFailed,
            )) => WIFI_REAUTHORIZE_REQUIRED.into(),
            Self::Verify(IdeviceError::DeviceLocked | IdeviceError::PasswordProtected) => {
                "The device must remain unlocked while Wi-Fi control authorization is verified."
                    .into()
            }
            error if error.is_transient() => "The device closed its Wi-Fi authorization service before verification completed. Existing credentials were preserved; keep the device awake on the same network and reconnect.".into(),
            _ => "The device returned an invalid response while verifying Wi-Fi control authorization. Existing credentials were preserved; reconnect by USB and authorize Wi-Fi control again if the error persists.".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WifiEndpoint {
    pub udid: String,
    pub address: IpAddr,
    pub scope_id: Option<u32>,
    pub remote_pairing_address: IpAddr,
    pub remote_pairing_scope_id: Option<u32>,
    pub remote_pairing_port: u16,
    pub pairing_file: idevice::pairing_file::PairingFile,
}

#[derive(Clone, Debug)]
pub struct UsbmuxdEndpoint {
    pub device: UsbmuxdDevice,
    pub address: UsbmuxdAddr,
}

#[derive(Clone, Debug)]
pub enum SessionEndpoint {
    Usbmuxd(Box<UsbmuxdEndpoint>),
    Wifi(Box<WifiEndpoint>),
}

impl SessionEndpoint {
    pub fn udid(&self) -> &str {
        match self {
            Self::Usbmuxd(endpoint) => &endpoint.device.udid,
            Self::Wifi(endpoint) => &endpoint.udid,
        }
    }

    pub fn connection(&self) -> ConnKind {
        match self {
            Self::Usbmuxd(endpoint) => connection_kind(&endpoint.device.connection_type),
            Self::Wifi(_) => ConnKind::Network,
        }
    }
}

/// Build exactly the provider represented by the picker selection. Transport
/// fallback belongs to discovery/reconnection and must not happen implicitly.
pub async fn connect_provider(
    endpoint: SessionEndpoint,
) -> Result<(Arc<dyn IdeviceProvider>, ConnKind), String> {
    let udid = endpoint.udid().to_owned();
    let connection = endpoint.connection();
    let provider: Arc<dyn IdeviceProvider> = match endpoint {
        SessionEndpoint::Usbmuxd(endpoint) => Arc::new(
            endpoint
                .device
                .to_provider(endpoint.address, "devicehub_rs"),
        ),
        SessionEndpoint::Wifi(endpoint) => Arc::new(wifi_provider(&endpoint)),
    };
    tracing::info!(
        device_id = %device_id_fingerprint(&udid),
        connection = connection.label(),
        "selected CoreDevice transport"
    );
    Ok((provider, connection))
}

pub fn connection_priority(connection: &Connection) -> u8 {
    match connection {
        Connection::Usb => 0,
        Connection::Network(_) => 1,
        Connection::Unknown(_) => 2,
    }
}

pub fn uses_usbmuxd_core_proxy(connection: &Connection) -> bool {
    matches!(connection, Connection::Usb)
}

pub fn connection_kind(connection: &Connection) -> ConnKind {
    match connection {
        Connection::Network(_) => ConnKind::Network,
        Connection::Usb => ConnKind::Usb,
        Connection::Unknown(_) => ConnKind::Other,
    }
}

pub fn connection_kind_priority(connection: ConnKind) -> u8 {
    match connection {
        ConnKind::Usb => 0,
        ConnKind::Network => 1,
        ConnKind::Other => 2,
    }
}

pub fn resolve_device_selection(requested: &str, devices: &[DeviceInfo]) -> Option<String> {
    devices
        .iter()
        .find(|device| device.id == requested)
        .or_else(|| {
            devices
                .iter()
                .filter(|device| device.udid == requested)
                .min_by_key(|device| connection_kind_priority(device.connection))
        })
        .map(|device| device.id.clone())
}

pub fn select_preferred_usbmuxd_device(
    devices: Vec<UsbmuxdDevice>,
    udid: Option<&str>,
) -> Option<UsbmuxdDevice> {
    devices
        .into_iter()
        .filter(|device| udid.is_none_or(|wanted| device.udid == wanted))
        .min_by_key(|device| {
            (
                connection_priority(&device.connection_type),
                device.device_id,
            )
        })
}

pub fn wifi_provider(endpoint: &WifiEndpoint) -> TcpProvider {
    TcpProvider {
        addr: endpoint.address,
        scope_id: endpoint.scope_id,
        pairing_file: endpoint.pairing_file.clone(),
        label: "devicehub_rs_wifi".into(),
    }
}

/// Build the RSD tunnel matching the already-selected endpoint. The returned
/// adapter and handshake are cloneable capabilities consumed by device services;
/// their lifetime is still supervised by the parent session.
pub async fn connect_core_tunnel(
    endpoint: &SessionEndpoint,
    provider: &dyn IdeviceProvider,
    pairing_dir: &Path,
    status: &StatusSlot,
    system_usbmuxd: &SystemUsbmuxdConfig,
) -> Result<(AdapterHandle, RsdHandshake), String> {
    match endpoint {
        SessionEndpoint::Usbmuxd(_) => connect_usb_core_tunnel(provider).await,
        SessionEndpoint::Wifi(endpoint) => {
            connect_wifi_core_tunnel(endpoint, pairing_dir, status, system_usbmuxd).await
        }
    }
}

async fn connect_usb_core_tunnel(
    provider: &dyn IdeviceProvider,
) -> Result<(AdapterHandle, RsdHandshake), String> {
    let proxy = CoreDeviceProxy::connect(provider)
        .await
        .map_err(|error| format!("no core device proxy: {error:?}"))?;
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy
        .create_software_tunnel()
        .map_err(|error| format!("no software tunnel: {error:?}"))?;
    let mut adapter = adapter.to_async_handle();
    let stream = adapter
        .connect(rsd_port)
        .await
        .map_err(|error| format!("RSD connect failed: {error:?}"))?;
    let handshake = RsdHandshake::new(stream)
        .await
        .map_err(|error| format!("RSD handshake failed: {error:?}"))?;
    Ok((adapter, handshake))
}

async fn connect_wifi_core_tunnel(
    endpoint: &WifiEndpoint,
    pairing_dir: &Path,
    status: &StatusSlot,
    system_usbmuxd: &SystemUsbmuxdConfig,
) -> Result<(AdapterHandle, RsdHandshake), String> {
    // RemotePairing credentials are created once over USB, then reused for
    // subsequent network sessions. Missing credentials are the only condition
    // that starts interactive authorization; transport errors must not erase or
    // silently replace an existing identity.
    let pairing_path = remote_pairing_path(pairing_dir, &endpoint.udid)?;
    let mut pairing_file = match RpPairingFile::read_from_file(&pairing_path).await {
        Ok(pairing_file) => pairing_file,
        Err(_) => {
            status.set("unlock the device and approve Wi-Fi control...");
            tracing::info!(
                device_id = %device_id_fingerprint(&endpoint.udid),
                "remote pairing credentials missing; authorizing over USB"
            );
            tokio::time::timeout(
                INITIAL_WIFI_PAIRING_TIMEOUT,
                pair_remote_via_usb(&endpoint.udid, &pairing_path, system_usbmuxd),
            )
            .await
            .map_err(|_| {
                "initial Wi-Fi authorization timed out; unlock the device and accept its trust prompt"
                    .to_string()
            })??
        }
    };
    status.set("verifying Wi-Fi control authorization...");
    let address = scoped_socket_addr(
        endpoint.remote_pairing_address,
        endpoint.remote_pairing_scope_id,
        endpoint.remote_pairing_port,
    );
    let mut client = None;
    for attempt in 1..=REMOTE_PAIRING_VERIFY_ATTEMPTS {
        match verify_remote_pairing_once(address, &mut pairing_file).await {
            Ok(verified) => {
                client = Some(verified);
                break;
            }
            Err(error) if error.is_transient() && attempt < REMOTE_PAIRING_VERIFY_ATTEMPTS => {
                tracing::warn!(
                    device_id = %device_id_fingerprint(&endpoint.udid),
                    attempt,
                    max_attempts = REMOTE_PAIRING_VERIFY_ATTEMPTS,
                    ?error,
                    "remote pairing verification interrupted; retrying with a fresh connection"
                );
                tokio::time::sleep(REMOTE_PAIRING_RETRY_DELAY * attempt as u32).await;
            }
            Err(error) => {
                tracing::warn!(
                    device_id = %device_id_fingerprint(&endpoint.udid),
                    attempt,
                    ?error,
                    transient = error.is_transient(),
                    "remote pairing verification failed"
                );
                return Err(error.user_message());
            }
        }
    }
    let mut client = client.ok_or_else(|| {
        "Wi-Fi control authorization verification ended without a result; reconnect the device."
            .to_string()
    })?;

    let tunnel_port = client
        .create_tcp_listener()
        .await
        .map_err(|error| format!("remote tunnel listener failed: {error:?}"))?;
    status.set("establishing secure Wi-Fi tunnel...");
    let tunnel_address = scoped_socket_addr(
        endpoint.remote_pairing_address,
        endpoint.remote_pairing_scope_id,
        tunnel_port,
    );
    let tunnel_stream = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect(tunnel_address),
    )
    .await
    .map_err(|_| "remote tunnel connection timed out".to_string())?
    .map_err(|error| format!("remote tunnel connection failed: {error}"))?;
    let tunnel = connect_tls_psk_tunnel_native(tunnel_stream, client.encryption_key())
        .await
        .map_err(|error| format!("remote TLS-PSK tunnel failed: {error:?}"))?;
    let client_ip = tunnel
        .info
        .client_address
        .parse()
        .map_err(|error| format!("invalid remote tunnel client address: {error}"))?;
    let server_ip = tunnel
        .info
        .server_address
        .parse()
        .map_err(|error| format!("invalid remote tunnel server address: {error}"))?;
    let rsd_port = tunnel.info.server_rsd_port;
    let mtu = tunnel.info.mtu as usize;
    let mut adapter =
        idevice::tcp::adapter::Adapter::new(Box::new(tunnel.into_inner()), client_ip, server_ip);
    // Leave room for the tunnel's IP/TCP framing. Sending at the advertised MTU
    // without this allowance causes fragmentation and unstable Wi-Fi sessions.
    adapter.set_mss(mtu.saturating_sub(60));
    let mut adapter = adapter.to_async_handle();
    let rsd_stream = adapter
        .connect(rsd_port)
        .await
        .map_err(|error| format!("remote RSD connect failed: {error:?}"))?;
    let handshake = RsdHandshake::new(rsd_stream)
        .await
        .map_err(|error| format!("remote RSD handshake failed: {error:?}"))?;
    tracing::info!(
        device_id = %device_id_fingerprint(&endpoint.udid),
        "remote pairing CoreDevice tunnel established"
    );
    Ok((adapter, handshake))
}

async fn verify_remote_pairing_once(
    address: std::net::SocketAddr,
    pairing_file: &mut RpPairingFile,
) -> Result<WifiPairingClient, WifiPairingVerificationError> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(address))
        .await
        .map_err(|_| WifiPairingVerificationError::ConnectTimeout)?
        .map_err(WifiPairingVerificationError::Connect)?;
    let socket = RpPairingSocket::new(stream);
    let mut client = RemotePairingClient::new(socket, "devicehub-mask");
    tokio::time::timeout(REMOTE_PAIRING_VERIFY_TIMEOUT, async {
        // Existing credentials are verified only. Any failure must remain a
        // failure: RemotePairingClient::connect would turn all verification
        // errors, including an EOF, into an implicit new pairing attempt.
        client.attempt_pair_verify().await?;
        client.validate_pairing(pairing_file).await
    })
    .await
    .map_err(|_| WifiPairingVerificationError::VerifyTimeout)?
    .map_err(WifiPairingVerificationError::Verify)?;
    Ok(client)
}

async fn pair_remote_via_usb(
    udid: &str,
    path: &Path,
    system_usbmuxd: &SystemUsbmuxdConfig,
) -> Result<RpPairingFile, String> {
    // This is authorization for future Wi-Fi control, not normal USB session
    // setup. It connects to the untrusted tunnel service exposed by CoreDevice
    // and persists only the resulting RemotePairing identity.
    let address = system_usbmuxd
        .address()
        .map_err(|error| format!("USB transport unavailable for remote pairing: {error}"))?;
    let mut mux = address
        .connect(0)
        .await
        .map_err(|error| format!("USB connection required for initial Wi-Fi pairing: {error:?}"))?;
    let device = mux
        .get_devices()
        .await
        .map_err(|error| format!("cannot list USB devices for remote pairing: {error:?}"))?
        .into_iter()
        .find(|device| device.udid == udid && matches!(device.connection_type, Connection::Usb))
        .ok_or_else(|| "connect this device by USB once to authorize Wi-Fi control".to_string())?;
    let provider = device.to_provider(address, "devicehub-mask-remote-pairing");
    tracing::debug!(
        device_id = %device_id_fingerprint(udid),
        "opening USB CoreDevice tunnel for remote pairing"
    );
    let (mut adapter, handshake) = connect_usb_core_tunnel(&provider).await?;
    let service = handshake
        .services
        .get("com.apple.internal.dt.coredevice.untrusted.tunnelservice")
        .ok_or_else(|| "device does not expose the remote pairing service".to_string())?;
    let stream = adapter
        .connect(service.port)
        .await
        .map_err(|error| format!("remote pairing service connect failed: {error:?}"))?;
    let mut connection = RemoteXpcClient::new(stream)
        .await
        .map_err(|error| format!("remote pairing XPC connection failed: {error:?}"))?;
    connection
        .do_handshake()
        .await
        .map_err(|error| format!("remote pairing XPC handshake failed: {error:?}"))?;
    connection
        .recv_root()
        .await
        .map_err(|error| format!("remote pairing XPC root failed: {error:?}"))?;
    tracing::info!(
        device_id = %device_id_fingerprint(udid),
        "waiting for device to authorize remote pairing"
    );
    let mut pairing_file = RpPairingFile::generate("devicehub-mask");
    let mut client = RemotePairingClient::new(connection, "devicehub-mask");
    client
        .connect(&mut pairing_file, async || "000000".to_string())
        .await
        .map_err(|error| format!("USB remote pairing failed: {error:?}"))?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("cannot create remote pairing directory: {error}"))?;
    }
    pairing_file
        .write_to_file(path)
        .await
        .map_err(|error| format!("cannot save remote pairing credentials: {error:?}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("cannot secure remote pairing credentials: {error}"))?;
    }
    tracing::info!(
        device_id = %device_id_fingerprint(udid),
        "created remote pairing credentials over USB"
    );
    Ok(pairing_file)
}

fn remote_pairing_path(pairing_dir: &Path, udid: &str) -> Result<PathBuf, String> {
    if udid.is_empty()
        || !udid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("device UDID contains unsupported characters".into());
    }
    let base = pairing_dir.parent().unwrap_or(pairing_dir);
    Ok(base.join("remote-pairings").join(format!("{udid}.plist")))
}

pub fn remove_remote_pairing_credentials(pairing_dir: &Path, udid: &str) -> Result<(), String> {
    let path = remote_pairing_path(pairing_dir, udid)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove remote pairing credentials: {error}")),
    }
}

fn scoped_socket_addr(
    address: std::net::IpAddr,
    scope_id: Option<u32>,
    port: u16,
) -> std::net::SocketAddr {
    match address {
        std::net::IpAddr::V4(_) => std::net::SocketAddr::new(address, port),
        std::net::IpAddr::V6(address) => std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
            address,
            port,
            0,
            scope_id.unwrap_or(0),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use idevice::usbmuxd::Connection;

    use super::{
        WIFI_REAUTHORIZE_REQUIRED, WifiPairingVerificationError, parse_system_usbmuxd_address,
        remote_pairing_path, remove_remote_pairing_credentials, resolve_device_selection,
        uses_usbmuxd_core_proxy,
    };
    use devicehub_core::{ConnKind, DeviceInfo, DevicePairingState, device_selector};
    use idevice::IdeviceError;

    #[test]
    fn host_tcp_usbmuxd_address_is_parsed_without_environment_reads() {
        let address = parse_system_usbmuxd_address(Some("127.0.0.1:27016".into())).unwrap();
        assert!(matches!(
            address,
            idevice::usbmuxd::UsbmuxdAddr::TcpSocket(address)
                if address == "127.0.0.1:27016".parse().unwrap()
        ));
        assert!(parse_system_usbmuxd_address(Some("not:a:socket".into())).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn host_unix_usbmuxd_path_is_preserved() {
        let address = parse_system_usbmuxd_address(Some("/custom/usbmuxd".into())).unwrap();
        assert!(matches!(
            address,
            idevice::usbmuxd::UsbmuxdAddr::UnixSocket(path) if path == "/custom/usbmuxd"
        ));
    }

    #[test]
    fn explicit_selection_and_legacy_udids_prefer_usb() {
        let devices = vec![
            DeviceInfo {
                id: device_selector("phone", ConnKind::Usb),
                udid: "phone".into(),
                name: "iPhone".into(),
                connection: ConnKind::Usb,
                pairing: DevicePairingState::Paired,
            },
            DeviceInfo {
                id: device_selector("phone", ConnKind::Network),
                udid: "phone".into(),
                name: "iPhone".into(),
                connection: ConnKind::Network,
                pairing: DevicePairingState::NotApplicable,
            },
        ];

        assert_eq!(
            resolve_device_selection("phone", &devices).as_deref(),
            Some("phone::usb")
        );
        assert_eq!(
            resolve_device_selection("phone::wifi", &devices).as_deref(),
            Some("phone::wifi")
        );
    }

    #[test]
    fn only_usb_devices_use_the_usbmuxd_core_proxy() {
        assert!(uses_usbmuxd_core_proxy(&Connection::Usb));
        assert!(!uses_usbmuxd_core_proxy(&Connection::Network(
            [192, 0, 2, 1].into()
        )));
        assert!(!uses_usbmuxd_core_proxy(&Connection::Unknown(
            "Network".into()
        )));
    }

    #[test]
    fn remote_pairing_credentials_stay_inside_application_data() {
        let pairing_dir = Path::new("app-data").join("pairings");
        assert_eq!(
            remote_pairing_path(&pairing_dir, "00008030-001905C02106402E").unwrap(),
            Path::new("app-data")
                .join("remote-pairings")
                .join("00008030-001905C02106402E.plist")
        );
        assert!(remote_pairing_path(&pairing_dir, "../outside").is_err());
        assert!(remote_pairing_path(&pairing_dir, "phone/plist").is_err());
        assert!(remote_pairing_path(&pairing_dir, "").is_err());
    }

    #[test]
    fn transient_remote_pairing_disconnects_are_retryable() {
        for kind in [
            std::io::ErrorKind::UnexpectedEof,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::TimedOut,
        ] {
            let error = WifiPairingVerificationError::Verify(IdeviceError::Socket(
                std::io::Error::new(kind, "test disconnect"),
            ));
            assert!(error.is_transient(), "{kind:?} should be retried");
            assert!(!error.user_message().contains("Socket("));
        }
        assert!(WifiPairingVerificationError::ConnectTimeout.is_transient());
        assert!(WifiPairingVerificationError::VerifyTimeout.is_transient());
    }

    #[test]
    fn rejected_remote_pairing_requires_explicit_reauthorization() {
        let error = WifiPairingVerificationError::Verify(IdeviceError::RemotePairing(
            idevice::remote_pairing::errors::RemotePairingError::PairVerifyFailed,
        ));
        assert!(!error.is_transient());
        assert_eq!(error.user_message(), WIFI_REAUTHORIZE_REQUIRED);
    }

    #[test]
    fn removing_remote_pairing_credentials_is_idempotent_and_confined() {
        let base = std::env::temp_dir().join(format!(
            "devicehub-mask-remote-pairing-removal-{}",
            uuid::Uuid::new_v4()
        ));
        let pairing_dir = base.join("pairings");
        let remote_dir = base.join("remote-pairings");
        std::fs::create_dir_all(&remote_dir).unwrap();
        let udid = "00008110-0011223344556677";
        let path = remote_dir.join(format!("{udid}.plist"));
        std::fs::write(&path, b"private credentials").unwrap();

        remove_remote_pairing_credentials(&pairing_dir, udid).unwrap();
        assert!(!path.exists());
        remove_remote_pairing_credentials(&pairing_dir, udid).unwrap();
        assert!(remove_remote_pairing_credentials(&pairing_dir, "../outside").is_err());

        std::fs::remove_dir_all(base).unwrap();
    }
}
