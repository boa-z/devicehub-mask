//! Bounded, user-initiated device packet capture through pcapd.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use idevice::pcapd::{DevicePacket, PcapdClient};
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceService, RsdService};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, watch};

use crate::protocol::ConnKind;
use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
pub const MIN_DURATION_SECONDS: u64 = 1;
pub const MAX_DURATION_SECONDS: u64 = 300;
const MAX_PACKET_BYTES: usize = 256 * 1024;
const MAX_CAPTURE_BYTES: u64 = 256 * 1024 * 1024;
const PCAP_HEADER: [u8; 24] = [
    0xa1, 0xb2, 0xc3, 0xd4, // big-endian magic
    0x00, 0x02, 0x00, 0x04, // PCAP 2.4
    0x00, 0x00, 0x00, 0x00, // GMT offset
    0x00, 0x00, 0x00, 0x00, // timestamp accuracy
    0x00, 0x04, 0x00, 0x00, // 256 KiB snapshot length
    0x00, 0x00, 0x00, 0x01, // LINKTYPE_ETHERNET
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkCaptureState {
    #[default]
    Idle,
    Starting,
    Capturing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkCaptureStopReason {
    UserRequested,
    DurationLimit,
    SizeLimit,
    SessionEnded,
    StreamEnded,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NetworkCaptureStatus {
    pub state: NetworkCaptureState,
    pub packet_count: u64,
    pub bytes_written: u64,
    pub elapsed_ms: u64,
    pub duration_seconds: Option<u64>,
    pub stop_reason: Option<NetworkCaptureStopReason>,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct NetworkCaptureSlot(Arc<Mutex<NetworkCaptureStatus>>);

impl NetworkCaptureSlot {
    pub fn set(&self, status: NetworkCaptureStatus) {
        *self.0.lock().unwrap() = status;
    }

    pub fn get(&self) -> NetworkCaptureStatus {
        self.0.lock().unwrap().clone()
    }

    pub fn reset(&self) {
        self.set(NetworkCaptureStatus::default());
    }
}

#[derive(Debug)]
pub enum NetworkCaptureCommand {
    Start {
        destination: PathBuf,
        duration_seconds: u64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

struct CaptureWriter {
    file: tokio::fs::File,
    temporary: PathBuf,
    destination: PathBuf,
    bytes_written: u64,
}

impl CaptureWriter {
    async fn create(destination: PathBuf) -> Result<Self, String> {
        validate_destination(&destination).await?;
        let temporary = temporary_sibling(&destination)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await
            .map_err(|error| format!("unable to create packet capture file: {error}"))?;
        if let Err(error) = file.write_all(&PCAP_HEADER).await {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(format!("unable to write packet capture header: {error}"));
        }
        Ok(Self {
            file,
            temporary,
            destination,
            bytes_written: PCAP_HEADER.len() as u64,
        })
    }

    fn can_write(&self, packet: &DevicePacket) -> Result<bool, String> {
        if packet.data.len() > MAX_PACKET_BYTES {
            return Err("device packet exceeds the 256 KiB snapshot limit".into());
        }
        Ok(self
            .bytes_written
            .saturating_add(16)
            .saturating_add(packet.data.len() as u64)
            <= MAX_CAPTURE_BYTES)
    }

    async fn write_packet(&mut self, packet: &DevicePacket) -> Result<(), String> {
        let record = encode_record(packet)?;
        self.file
            .write_all(&record)
            .await
            .map_err(|error| format!("unable to write packet capture data: {error}"))?;
        self.bytes_written = self.bytes_written.saturating_add(record.len() as u64);
        Ok(())
    }

    async fn finish(mut self) -> Result<u64, String> {
        let result = async {
            self.file
                .flush()
                .await
                .map_err(|error| format!("unable to flush packet capture: {error}"))?;
            self.file
                .sync_data()
                .await
                .map_err(|error| format!("unable to synchronize packet capture: {error}"))?;
            drop(self.file);
            replace_local_file(&self.temporary, &self.destination).await?;
            Ok(self.bytes_written)
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&self.temporary).await;
        }
        result
    }
}

struct ActiveCapture {
    client: PcapdClient,
    writer: CaptureWriter,
    duration_seconds: u64,
    started: Instant,
    packet_count: u64,
}

pub struct NetworkCaptureTransport {
    provider: Arc<dyn IdeviceProvider>,
    connection: ConnKind,
    adapter: AdapterHandle,
    handshake: RsdHandshake,
}

impl NetworkCaptureTransport {
    pub fn new(
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
    ) -> Self {
        Self {
            provider,
            connection,
            adapter,
            handshake,
        }
    }
}

pub async fn serve(
    mut transport: NetworkCaptureTransport,
    mut commands: mpsc::Receiver<NetworkCaptureCommand>,
    status: NetworkCaptureSlot,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt = 0;
    status.reset();
    reporter.stopped(attempt);
    loop {
        let command = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
            command = commands.recv() => command,
        };
        let Some(command) = command else { return };
        match command {
            NetworkCaptureCommand::Stop { reply } => {
                let _ = reply.send(Err("no packet capture is running".into()));
            }
            NetworkCaptureCommand::Start {
                destination,
                duration_seconds,
                reply,
            } => {
                attempt += 1;
                status.set(NetworkCaptureStatus {
                    state: NetworkCaptureState::Starting,
                    duration_seconds: Some(duration_seconds),
                    ..NetworkCaptureStatus::default()
                });
                reporter.connecting(attempt);
                let active = begin_capture(
                    transport.provider.as_ref(),
                    transport.connection,
                    &mut transport.adapter,
                    &mut transport.handshake,
                    destination,
                    duration_seconds,
                )
                .await;
                let active = match active {
                    Ok(active) => active,
                    Err(error) => {
                        status.set(NetworkCaptureStatus {
                            state: NetworkCaptureState::Failed,
                            duration_seconds: Some(duration_seconds),
                            error: Some(error.clone()),
                            ..NetworkCaptureStatus::default()
                        });
                        reporter.unavailable(attempt, error.clone());
                        let _ = reply.send(Err(error));
                        continue;
                    }
                };
                reporter.ready(attempt);
                status.set(capture_status(&active, NetworkCaptureState::Capturing));
                let _ = reply.send(Ok(()));
                if capture(
                    active,
                    &mut commands,
                    &status,
                    &reporter,
                    attempt,
                    &mut shutdown,
                )
                .await
                {
                    return;
                }
            }
        }
    }
}

async fn begin_capture(
    provider: &dyn IdeviceProvider,
    connection: ConnKind,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    destination: PathBuf,
    duration_seconds: u64,
) -> Result<ActiveCapture, String> {
    validate_request(&destination, duration_seconds).await?;
    let client = connect_client(provider, connection, adapter, handshake).await?;
    let writer = CaptureWriter::create(destination).await?;
    Ok(ActiveCapture {
        client,
        writer,
        duration_seconds,
        started: Instant::now(),
        packet_count: 0,
    })
}

async fn connect_client(
    provider: &dyn IdeviceProvider,
    connection: ConnKind,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) -> Result<PcapdClient, String> {
    let mut failures = Vec::new();
    if connection == ConnKind::Usb {
        match tokio::time::timeout(CONNECT_TIMEOUT, PcapdClient::connect(provider)).await {
            Ok(Ok(client)) => {
                tracing::info!(
                    transport = "lockdown-usb",
                    "packet capture service connected"
                );
                return Ok(client);
            }
            Ok(Err(error)) => failures.push(format!(
                "USB lockdown pcapd: {}",
                describe_service_error(&error)
            )),
            Err(_) => failures.push("USB lockdown pcapd: connection timed out".into()),
        }
    }

    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        PcapdClient::connect_rsd(adapter, handshake),
    )
    .await
    {
        Ok(Ok(client)) => {
            tracing::info!(
                transport = "coredevice-rsd",
                "packet capture service connected"
            );
            Ok(client)
        }
        Ok(Err(error)) => {
            failures.push(format!(
                "CoreDevice RSD pcapd: {}",
                describe_service_error(&error)
            ));
            Err(format!(
                "packet capture service unavailable: {}",
                failures.join("; ")
            ))
        }
        Err(_) => {
            failures.push("CoreDevice RSD pcapd: connection timed out".into());
            Err(format!(
                "packet capture service unavailable: {}",
                failures.join("; ")
            ))
        }
    }
}

fn describe_service_error(error: &idevice::IdeviceError) -> String {
    match error {
        idevice::IdeviceError::UnknownErrorType(message)
            if message.eq_ignore_ascii_case("ServiceProhibited") =>
        {
            "the device prohibited this capture service".into()
        }
        _ => format!("{error:?}"),
    }
}

async fn capture(
    mut active: ActiveCapture,
    commands: &mut mpsc::Receiver<NetworkCaptureCommand>,
    status: &NetworkCaptureSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    let deadline = tokio::time::sleep(Duration::from_secs(active.duration_seconds));
    tokio::pin!(deadline);
    let mut status_tick = tokio::time::interval(STATUS_INTERVAL);
    status_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut stop_reply = None;
    let mut stopped_for_shutdown = false;
    let mut failure = None;
    let reason = loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    stopped_for_shutdown = true;
                    break NetworkCaptureStopReason::SessionEnded;
                }
            }
            _ = &mut deadline => break NetworkCaptureStopReason::DurationLimit,
            _ = status_tick.tick() => {
                status.set(capture_status(&active, NetworkCaptureState::Capturing));
            }
            command = commands.recv() => match command {
                Some(NetworkCaptureCommand::Stop { reply }) => {
                    stop_reply = Some(reply);
                    break NetworkCaptureStopReason::UserRequested;
                }
                Some(NetworkCaptureCommand::Start { reply, .. }) => {
                    let _ = reply.send(Err("a packet capture is already running".into()));
                }
                None => {
                    stopped_for_shutdown = true;
                    break NetworkCaptureStopReason::SessionEnded;
                }
            },
            packet = active.client.next_packet() => match packet {
                Ok(packet) => match active.writer.can_write(&packet) {
                    Ok(true) => {
                        if let Err(error) = active.writer.write_packet(&packet).await {
                            failure = Some(error);
                            break NetworkCaptureStopReason::StreamEnded;
                        }
                        active.packet_count = active.packet_count.saturating_add(1);
                    }
                    Ok(false) => break NetworkCaptureStopReason::SizeLimit,
                    Err(error) => {
                        failure = Some(error);
                        break NetworkCaptureStopReason::StreamEnded;
                    }
                },
                Err(error) => {
                    failure = Some(format!("packet capture stream ended: {error:?}"));
                    break NetworkCaptureStopReason::StreamEnded;
                }
            }
        }
    };

    let packet_count = active.packet_count;
    let elapsed_ms = active.started.elapsed().as_millis() as u64;
    let duration_seconds = active.duration_seconds;
    let attempted_bytes = active.writer.bytes_written;
    let finish_result = active.writer.finish().await;
    let bytes_written = finish_result.as_ref().copied().unwrap_or(attempted_bytes);
    if let Err(error) = finish_result {
        failure = Some(match failure {
            Some(previous) => format!("{previous}; {error}"),
            None => error,
        });
    }
    let result = match failure {
        Some(error) => {
            status.set(NetworkCaptureStatus {
                state: NetworkCaptureState::Failed,
                packet_count,
                bytes_written,
                elapsed_ms,
                duration_seconds: Some(duration_seconds),
                stop_reason: Some(reason),
                error: Some(error.clone()),
            });
            reporter.unavailable(attempt, error.clone());
            Err(error)
        }
        None => {
            status.set(NetworkCaptureStatus {
                state: NetworkCaptureState::Completed,
                packet_count,
                bytes_written,
                elapsed_ms,
                duration_seconds: Some(duration_seconds),
                stop_reason: Some(reason),
                error: None,
            });
            reporter.stopped(attempt);
            tracing::info!(
                packet_count,
                bytes_written,
                elapsed_ms,
                ?reason,
                "device packet capture completed"
            );
            Ok(())
        }
    };
    if let Some(reply) = stop_reply {
        let _ = reply.send(result);
    }
    stopped_for_shutdown
}

fn capture_status(active: &ActiveCapture, state: NetworkCaptureState) -> NetworkCaptureStatus {
    NetworkCaptureStatus {
        state,
        packet_count: active.packet_count,
        bytes_written: active.writer.bytes_written,
        elapsed_ms: active.started.elapsed().as_millis() as u64,
        duration_seconds: Some(active.duration_seconds),
        stop_reason: None,
        error: None,
    }
}

fn validate_duration(duration_seconds: u64) -> Result<(), String> {
    if (MIN_DURATION_SECONDS..=MAX_DURATION_SECONDS).contains(&duration_seconds) {
        Ok(())
    } else {
        Err(format!(
            "packet capture duration must be between {MIN_DURATION_SECONDS} and {MAX_DURATION_SECONDS} seconds"
        ))
    }
}

pub async fn validate_request(path: &Path, duration_seconds: u64) -> Result<(), String> {
    validate_duration(duration_seconds)?;
    validate_destination(path).await
}

async fn validate_destination(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pcap"))
    {
        return Err("packet capture destination must be an absolute .pcap path".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "packet capture destination has no parent directory".to_string())?;
    let metadata = tokio::fs::metadata(parent)
        .await
        .map_err(|error| format!("unable to access packet capture directory: {error}"))?;
    if !metadata.is_dir() {
        return Err("packet capture parent is not a directory".into());
    }
    match tokio::fs::metadata(path).await {
        Ok(metadata) if !metadata.is_file() => {
            Err("packet capture destination is not a regular file".into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "unable to inspect packet capture destination: {error}"
        )),
    }
}

fn encode_record(packet: &DevicePacket) -> Result<Vec<u8>, String> {
    if packet.data.len() > MAX_PACKET_BYTES {
        return Err("device packet exceeds the 256 KiB snapshot limit".into());
    }
    let length = u32::try_from(packet.data.len())
        .map_err(|_| "device packet length cannot be represented in PCAP".to_string())?;
    let mut record = Vec::with_capacity(16 + packet.data.len());
    record.extend_from_slice(&packet.seconds.to_be_bytes());
    record.extend_from_slice(&packet.microseconds.to_be_bytes());
    record.extend_from_slice(&length.to_be_bytes());
    record.extend_from_slice(&length.to_be_bytes());
    record.extend_from_slice(&packet.data);
    Ok(record)
}

fn temporary_sibling(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "packet capture destination has no parent directory".to_string())?;
    Ok(parent.join(format!(
        ".devicehub-capture-{}-{}.part",
        std::process::id(),
        uuid::Uuid::new_v4()
    )))
}

async fn replace_local_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    let backup = destination.with_file_name(format!(
        ".devicehub-capture-backup-{}-{}.part",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let had_destination = match tokio::fs::metadata(destination).await {
        Ok(metadata) if metadata.is_file() => {
            tokio::fs::rename(destination, &backup)
                .await
                .map_err(|error| format!("unable to preserve existing capture file: {error}"))?;
            true
        }
        Ok(_) => return Err("packet capture destination is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "unable to inspect packet capture destination: {error}"
            ));
        }
    };
    match tokio::fs::rename(temporary, destination).await {
        Ok(()) => {
            if had_destination {
                let _ = tokio::fs::remove_file(backup).await;
            }
            Ok(())
        }
        Err(error) => {
            if had_destination {
                let _ = tokio::fs::rename(&backup, destination).await;
            }
            Err(format!("unable to finish packet capture: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::IdeviceService;
    use idevice::core_device_proxy::CoreDeviceProxy;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    fn packet(data: Vec<u8>) -> DevicePacket {
        DevicePacket {
            header_length: 0,
            header_version: 2,
            packet_length: data.len() as u32,
            interface_type: 0,
            unit: 0,
            io: 0,
            protocol_family: 2,
            frame_pre_length: 0,
            frame_post_length: 0,
            interface_name: "en0".into(),
            pid: 1,
            comm: "test".into(),
            svc: 0,
            epid: 1,
            ecomm: "test".into(),
            seconds: 0x0102_0304,
            microseconds: 0x0506_0708,
            data,
        }
    }

    #[test]
    fn pcap_record_uses_big_endian_timestamps_and_lengths() {
        let record = encode_record(&packet(vec![0xaa, 0xbb, 0xcc])).unwrap();
        assert_eq!(&record[0..4], &[1, 2, 3, 4]);
        assert_eq!(&record[4..8], &[5, 6, 7, 8]);
        assert_eq!(&record[8..12], &[0, 0, 0, 3]);
        assert_eq!(&record[12..16], &[0, 0, 0, 3]);
        assert_eq!(&record[16..], &[0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn capture_duration_is_bounded() {
        assert!(validate_duration(MIN_DURATION_SECONDS).is_ok());
        assert!(validate_duration(MAX_DURATION_SECONDS).is_ok());
        assert!(validate_duration(0).is_err());
        assert!(validate_duration(MAX_DURATION_SECONDS + 1).is_err());
    }

    #[tokio::test]
    async fn destination_must_be_absolute_pcap_in_an_existing_directory() {
        assert!(
            validate_destination(Path::new("relative.pcap"))
                .await
                .is_err()
        );
        let destination =
            std::env::temp_dir().join(format!("devicehub-mask-{}.pcap", uuid::Uuid::new_v4()));
        assert!(
            validate_destination(&destination.with_extension("txt"))
                .await
                .is_err()
        );
        assert!(validate_destination(&destination).await.is_ok());
    }

    #[tokio::test]
    async fn request_validation_checks_duration_before_touching_the_path() {
        let error = validate_request(Path::new("relative.txt"), 0)
            .await
            .unwrap_err();
        assert!(error.contains("duration"));
    }

    #[tokio::test]
    async fn writer_creates_a_complete_pcap_and_replaces_existing_file() {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-capture-test-{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir(&directory).await.unwrap();
        let destination = directory.join("capture.pcap");
        tokio::fs::write(&destination, b"old").await.unwrap();

        let mut writer = CaptureWriter::create(destination.clone()).await.unwrap();
        writer
            .write_packet(&packet(vec![0xaa, 0xbb, 0xcc]))
            .await
            .unwrap();
        let bytes_written = writer.finish().await.unwrap();
        let contents = tokio::fs::read(&destination).await.unwrap();
        assert_eq!(bytes_written, 43);
        assert_eq!(&contents[..PCAP_HEADER.len()], &PCAP_HEADER);
        assert_eq!(&contents[PCAP_HEADER.len() + 16..], &[0xaa, 0xbb, 0xcc]);

        tokio::fs::remove_file(destination).await.unwrap();
        tokio::fs::remove_dir(directory).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device with network traffic"]
    async fn captures_a_pcap_packet_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-pcap-test");
        let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let mut handshake = RsdHandshake::new(stream).await.unwrap();
        let destination = std::env::temp_dir().join(format!(
            "devicehub-mask-hardware-{}.pcap",
            uuid::Uuid::new_v4()
        ));
        let mut active = begin_capture(
            &provider,
            ConnKind::Usb,
            &mut adapter,
            &mut handshake,
            destination.clone(),
            10,
        )
        .await
        .unwrap();
        let packet = tokio::time::timeout(Duration::from_secs(10), active.client.next_packet())
            .await
            .expect("timed out waiting for device traffic")
            .unwrap();
        assert!(active.writer.can_write(&packet).unwrap());
        active.writer.write_packet(&packet).await.unwrap();
        assert!(active.writer.finish().await.unwrap() > PCAP_HEADER.len() as u64);
        tokio::fs::remove_file(destination).await.unwrap();
    }

    #[test]
    fn prohibited_service_errors_are_actionable() {
        assert_eq!(
            describe_service_error(&idevice::IdeviceError::UnknownErrorType(
                "ServiceProhibited".into()
            )),
            "the device prohibited this capture service"
        );
        assert!(
            describe_service_error(&idevice::IdeviceError::ServiceNotFound)
                .contains("ServiceNotFound")
        );
    }
}
