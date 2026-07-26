//! Bounded device packet capture through pcapd.

use std::sync::Arc;
use std::time::{Duration, Instant};

use devicehub_core::{
    ConnKind, NetworkCaptureSlot, NetworkCaptureState, NetworkCaptureStatus,
    NetworkCaptureStopReason,
};
use idevice::pcapd::{DevicePacket, PcapdClient};
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceService, RsdService};
use tokio::sync::{mpsc, oneshot, watch};

use super::{CaptureFileIo, CaptureFileKind, CaptureFileWriter};
use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
pub const MIN_NETWORK_CAPTURE_DURATION_SECONDS: u64 = 1;
pub const MAX_NETWORK_CAPTURE_DURATION_SECONDS: u64 = 300;
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

#[derive(Debug)]
pub enum NetworkCaptureCommand<Destination> {
    Start {
        destination: Destination,
        duration_seconds: u64,
        process_id: Option<u32>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

struct ActiveCapture<Writer> {
    client: PcapdClient,
    writer: Writer,
    duration_seconds: u64,
    process_id: Option<u32>,
    started: Instant,
    packet_count: u64,
    filtered_packet_count: u64,
}

pub(crate) struct NetworkCaptureTransport {
    provider: Arc<dyn IdeviceProvider>,
    connection: ConnKind,
    adapter: AdapterHandle,
    handshake: RsdHandshake,
}

impl NetworkCaptureTransport {
    pub(crate) fn new(
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

pub(crate) async fn serve<Files>(
    mut transport: NetworkCaptureTransport,
    mut commands: mpsc::Receiver<NetworkCaptureCommand<Files::Destination>>,
    status: NetworkCaptureSlot,
    files: Files,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) where
    Files: CaptureFileIo,
{
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
                process_id,
                reply,
            } => {
                attempt += 1;
                status.set(NetworkCaptureStatus {
                    state: NetworkCaptureState::Starting,
                    process_id,
                    duration_seconds: Some(duration_seconds),
                    ..NetworkCaptureStatus::default()
                });
                reporter.connecting(attempt);
                let active = begin_capture(
                    &mut transport,
                    destination,
                    duration_seconds,
                    process_id,
                    &files,
                )
                .await;
                let active = match active {
                    Ok(active) => active,
                    Err(error) => {
                        status.set(NetworkCaptureStatus {
                            state: NetworkCaptureState::Failed,
                            process_id,
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

async fn begin_capture<Files>(
    transport: &mut NetworkCaptureTransport,
    destination: Files::Destination,
    duration_seconds: u64,
    process_id: Option<u32>,
    files: &Files,
) -> Result<ActiveCapture<Files::Writer>, String>
where
    Files: CaptureFileIo,
{
    validate_duration(duration_seconds)?;
    files
        .validate(&destination, CaptureFileKind::Network)
        .await?;
    let client = connect_client(
        transport.provider.as_ref(),
        transport.connection,
        &mut transport.adapter,
        &mut transport.handshake,
    )
    .await?;
    let writer = files
        .create(destination, CaptureFileKind::Network, &PCAP_HEADER)
        .await?;
    Ok(ActiveCapture {
        client,
        writer,
        duration_seconds,
        process_id,
        started: Instant::now(),
        packet_count: 0,
        filtered_packet_count: 0,
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

async fn capture<Writer, Destination>(
    mut active: ActiveCapture<Writer>,
    commands: &mut mpsc::Receiver<NetworkCaptureCommand<Destination>>,
    status: &NetworkCaptureSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    shutdown: &mut watch::Receiver<bool>,
) -> bool
where
    Writer: CaptureFileWriter,
{
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
                Ok(packet) if !packet_matches_process(&packet, active.process_id) => {
                    active.filtered_packet_count = active.filtered_packet_count.saturating_add(1);
                }
                Ok(packet) => match can_write(&active.writer, &packet) {
                    Ok(true) => {
                        let record = match encode_record(&packet) {
                            Ok(record) => record,
                            Err(error) => {
                                failure = Some(error);
                                break NetworkCaptureStopReason::StreamEnded;
                            }
                        };
                        if let Err(error) = active.writer.write(&record).await {
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
    let filtered_packet_count = active.filtered_packet_count;
    let elapsed_ms = active.started.elapsed().as_millis() as u64;
    let duration_seconds = active.duration_seconds;
    let process_id = active.process_id;
    let attempted_bytes = active.writer.bytes_written();
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
                process_id,
                packet_count,
                filtered_packet_count,
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
                process_id,
                packet_count,
                filtered_packet_count,
                bytes_written,
                elapsed_ms,
                duration_seconds: Some(duration_seconds),
                stop_reason: Some(reason),
                error: None,
            });
            reporter.stopped(attempt);
            tracing::info!(
                packet_count,
                filtered_packet_count,
                process_id,
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

fn capture_status<Writer>(
    active: &ActiveCapture<Writer>,
    state: NetworkCaptureState,
) -> NetworkCaptureStatus
where
    Writer: CaptureFileWriter,
{
    NetworkCaptureStatus {
        state,
        process_id: active.process_id,
        packet_count: active.packet_count,
        filtered_packet_count: active.filtered_packet_count,
        bytes_written: active.writer.bytes_written(),
        elapsed_ms: active.started.elapsed().as_millis() as u64,
        duration_seconds: Some(active.duration_seconds),
        stop_reason: None,
        error: None,
    }
}

fn packet_matches_process(packet: &DevicePacket, process_id: Option<u32>) -> bool {
    process_id.is_none_or(|process_id| packet.pid == process_id || packet.epid == process_id)
}

pub fn validate_duration(duration_seconds: u64) -> Result<(), String> {
    if (MIN_NETWORK_CAPTURE_DURATION_SECONDS..=MAX_NETWORK_CAPTURE_DURATION_SECONDS)
        .contains(&duration_seconds)
    {
        Ok(())
    } else {
        Err(format!(
            "packet capture duration must be between {MIN_NETWORK_CAPTURE_DURATION_SECONDS} and {MAX_NETWORK_CAPTURE_DURATION_SECONDS} seconds"
        ))
    }
}

fn can_write<Writer>(writer: &Writer, packet: &DevicePacket) -> Result<bool, String>
where
    Writer: CaptureFileWriter,
{
    if packet.data.len() > MAX_PACKET_BYTES {
        return Err("device packet exceeds the 256 KiB snapshot limit".into());
    }
    Ok(writer
        .bytes_written()
        .saturating_add(16)
        .saturating_add(packet.data.len() as u64)
        <= MAX_CAPTURE_BYTES)
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

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::IdeviceService;
    use idevice::core_device_proxy::CoreDeviceProxy;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    #[derive(Clone, Copy, Default)]
    struct MemoryCaptureFiles;

    struct MemoryCaptureWriter(Vec<u8>);

    impl CaptureFileIo for MemoryCaptureFiles {
        type Destination = ();
        type Writer = MemoryCaptureWriter;

        fn validate<'a>(
            &'a self,
            _destination: &'a (),
            _kind: CaptureFileKind,
        ) -> super::super::CaptureFileFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }

        fn create<'a>(
            &'a self,
            _destination: (),
            _kind: CaptureFileKind,
            header: &'static [u8],
        ) -> super::super::CaptureFileFuture<'a, Self::Writer> {
            Box::pin(async move { Ok(MemoryCaptureWriter(header.to_vec())) })
        }
    }

    impl CaptureFileWriter for MemoryCaptureWriter {
        fn bytes_written(&self) -> u64 {
            self.0.len() as u64
        }

        fn write<'a>(&'a mut self, bytes: &'a [u8]) -> super::super::CaptureFileFuture<'a, ()> {
            Box::pin(async move {
                self.0.extend_from_slice(bytes);
                Ok(())
            })
        }

        fn finish(self) -> super::super::CaptureFileFuture<'static, u64> {
            Box::pin(async move { Ok(self.0.len() as u64) })
        }
    }

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
        assert!(validate_duration(MIN_NETWORK_CAPTURE_DURATION_SECONDS).is_ok());
        assert!(validate_duration(MAX_NETWORK_CAPTURE_DURATION_SECONDS).is_ok());
        assert!(validate_duration(0).is_err());
        assert!(validate_duration(MAX_NETWORK_CAPTURE_DURATION_SECONDS + 1).is_err());
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

    #[test]
    fn process_filter_matches_primary_or_effective_pid() {
        let mut packet = packet(vec![0xaa]);
        packet.pid = 42;
        packet.epid = 84;
        assert!(packet_matches_process(&packet, None));
        assert!(packet_matches_process(&packet, Some(42)));
        assert!(packet_matches_process(&packet, Some(84)));
        assert!(!packet_matches_process(&packet, Some(7)));
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
        let provider: Arc<dyn IdeviceProvider> =
            Arc::new(device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-pcap-test"));
        let proxy = CoreDeviceProxy::connect(provider.as_ref()).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let handshake = RsdHandshake::new(stream).await.unwrap();
        let mut transport =
            NetworkCaptureTransport::new(provider, ConnKind::Usb, adapter, handshake);
        let mut active = begin_capture(&mut transport, (), 10, None, &MemoryCaptureFiles)
            .await
            .unwrap();
        let packet = tokio::time::timeout(Duration::from_secs(10), active.client.next_packet())
            .await
            .expect("timed out waiting for device traffic")
            .unwrap();
        assert!(can_write(&active.writer, &packet).unwrap());
        active
            .writer
            .write(&encode_record(&packet).unwrap())
            .await
            .unwrap();
        assert!(active.writer.finish().await.unwrap() > PCAP_HEADER.len() as u64);
    }
}
