//! Bounded Bluetooth HCI capture through BTPacketLogger.

use std::pin::Pin;
use std::time::{Duration, Instant};

use devicehub_core::{
    BluetoothCaptureSlot, BluetoothCaptureState, BluetoothCaptureStatus,
    BluetoothCaptureStopReason, ManagedOperationError, ManagedOperationKind,
    ManagedOperationRegistry, OperationErrorCode,
};
use futures_util::{Stream, StreamExt};
use idevice::RsdService;
use idevice::bt_packet_logger::{BtFrame, BtPacketKind, BtPacketLoggerClient};
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use tokio::sync::{mpsc, oneshot, watch};

use super::{CaptureFileIo, CaptureFileKind, CaptureFileWriter};
use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
pub const MIN_BLUETOOTH_CAPTURE_DURATION_SECONDS: u64 = 1;
pub const MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS: u64 = 300;
const MAX_PACKET_BYTES: usize = 64 * 1024;
const MAX_CAPTURE_BYTES: u64 = 64 * 1024 * 1024;
const PCAP_HEADER: [u8; 24] = [
    0xa1, 0xb2, 0xc3, 0xd4, // big-endian magic
    0x00, 0x02, 0x00, 0x04, // PCAP 2.4
    0x00, 0x00, 0x00, 0x00, // GMT offset
    0x00, 0x00, 0x00, 0x00, // timestamp accuracy
    0x00, 0x00, 0xff, 0xff, // 65535-byte snapshot length
    0x00, 0x00, 0x00, 0xc9, // DLT_BLUETOOTH_HCI_H4_WITH_PHDR (201)
];

#[derive(Debug)]
pub enum BluetoothCaptureCommand<Destination> {
    Start {
        destination: Destination,
        duration_seconds: u64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

struct ActiveCapture<Writer> {
    stream: Pin<Box<dyn Stream<Item = Result<BtFrame, idevice::IdeviceError>> + Send>>,
    writer: Writer,
    duration_seconds: u64,
    started: Instant,
    packet_count: u64,
}

pub(crate) struct BluetoothCaptureTransport {
    adapter: AdapterHandle,
    handshake: RsdHandshake,
}

impl BluetoothCaptureTransport {
    pub(crate) fn new(adapter: AdapterHandle, handshake: RsdHandshake) -> Self {
        Self { adapter, handshake }
    }
}

pub(crate) async fn serve<Files>(
    mut transport: BluetoothCaptureTransport,
    mut commands: mpsc::Receiver<BluetoothCaptureCommand<Files::Destination>>,
    status: BluetoothCaptureSlot,
    operations: ManagedOperationRegistry,
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
                if changed.is_err() || *shutdown.borrow() { return; }
                continue;
            }
            command = commands.recv() => command,
        };
        let Some(command) = command else { return };
        match command {
            BluetoothCaptureCommand::Stop { reply } => {
                let _ = reply.send(Err("no Bluetooth capture is running".into()));
            }
            BluetoothCaptureCommand::Start {
                destination,
                duration_seconds,
                reply,
            } => {
                let managed_id =
                    match operations.begin(ManagedOperationKind::BluetoothCapture, None, true) {
                        Ok(id) => id,
                        Err(error) => {
                            let _ = reply.send(Err(error.message));
                            continue;
                        }
                    };
                attempt += 1;
                status.set(BluetoothCaptureStatus {
                    state: BluetoothCaptureState::Starting,
                    duration_seconds: Some(duration_seconds),
                    ..BluetoothCaptureStatus::default()
                });
                reporter.connecting(attempt);
                let active = begin_capture(
                    &mut transport.adapter,
                    &mut transport.handshake,
                    destination,
                    duration_seconds,
                    &files,
                )
                .await;
                let active = match active {
                    Ok(active) => active,
                    Err(error) => {
                        operations.fail(
                            managed_id,
                            ManagedOperationError::new(
                                OperationErrorCode::Unavailable,
                                error.clone(),
                            ),
                        );
                        status.set(BluetoothCaptureStatus {
                            state: BluetoothCaptureState::Failed,
                            duration_seconds: Some(duration_seconds),
                            error: Some(error.clone()),
                            ..BluetoothCaptureStatus::default()
                        });
                        reporter.unavailable(attempt, error.clone());
                        let _ = reply.send(Err(error));
                        continue;
                    }
                };
                reporter.ready(attempt);
                status.set(capture_status(&active, BluetoothCaptureState::Capturing));
                let _ = reply.send(Ok(()));
                if capture(
                    active,
                    &mut commands,
                    &status,
                    &reporter,
                    &operations,
                    managed_id,
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
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    destination: Files::Destination,
    duration_seconds: u64,
    files: &Files,
) -> Result<ActiveCapture<Files::Writer>, String>
where
    Files: CaptureFileIo,
{
    validate_duration(duration_seconds)?;
    files
        .validate(&destination, CaptureFileKind::Bluetooth)
        .await?;
    let client = tokio::time::timeout(
        CONNECT_TIMEOUT,
        BtPacketLoggerClient::connect_rsd(adapter, handshake),
    )
    .await
    .map_err(|_| "Bluetooth packet logger connection timed out".to_string())?
    .map_err(|error| format!("Bluetooth packet logger unavailable: {error:?}"))?;
    let writer = files
        .create(destination, CaptureFileKind::Bluetooth, &PCAP_HEADER)
        .await?;
    Ok(ActiveCapture {
        stream: client.into_stream(),
        writer,
        duration_seconds,
        started: Instant::now(),
        packet_count: 0,
    })
}

#[allow(clippy::too_many_arguments)]
async fn capture<Writer, Destination>(
    mut active: ActiveCapture<Writer>,
    commands: &mut mpsc::Receiver<BluetoothCaptureCommand<Destination>>,
    status: &BluetoothCaptureSlot,
    reporter: &ServiceReporter,
    operations: &ManagedOperationRegistry,
    managed_id: u64,
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
                    break BluetoothCaptureStopReason::SessionEnded;
                }
            }
            _ = &mut deadline => break BluetoothCaptureStopReason::DurationLimit,
            _ = status_tick.tick() => {
                status.set(capture_status(&active, BluetoothCaptureState::Capturing));
                operations.update(
                    managed_id,
                    Some("capturing".into()),
                    Some((active.started.elapsed().as_secs_f64()
                        / active.duration_seconds as f64 * 100.0).min(99.0)),
                );
            }
            command = commands.recv() => match command {
                Some(BluetoothCaptureCommand::Stop { reply }) => {
                    stop_reply = Some(reply);
                    break BluetoothCaptureStopReason::UserRequested;
                }
                Some(BluetoothCaptureCommand::Start { reply, .. }) => {
                    let _ = reply.send(Err("a Bluetooth capture is already running".into()));
                }
                None => {
                    stopped_for_shutdown = true;
                    break BluetoothCaptureStopReason::SessionEnded;
                }
            },
            frame = active.stream.next() => match frame {
                Some(Ok(frame)) => match can_write(&active.writer, &frame) {
                    Ok(true) => {
                        let record = match encode_record(&frame) {
                            Ok(record) => record,
                            Err(error) => {
                                failure = Some(error);
                                break BluetoothCaptureStopReason::StreamEnded;
                            }
                        };
                        if let Err(error) = active.writer.write(&record).await {
                            failure = Some(error);
                            break BluetoothCaptureStopReason::StreamEnded;
                        }
                        active.packet_count = active.packet_count.saturating_add(1);
                    }
                    Ok(false) => break BluetoothCaptureStopReason::SizeLimit,
                    Err(error) => {
                        failure = Some(error);
                        break BluetoothCaptureStopReason::StreamEnded;
                    }
                },
                Some(Err(error)) => {
                    failure = Some(format!("Bluetooth capture stream ended: {error:?}"));
                    break BluetoothCaptureStopReason::StreamEnded;
                }
                None => break BluetoothCaptureStopReason::StreamEnded,
            }
        }
    };

    let packet_count = active.packet_count;
    let elapsed_ms = active.started.elapsed().as_millis() as u64;
    let duration_seconds = active.duration_seconds;
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
            operations.fail(
                managed_id,
                ManagedOperationError::new(OperationErrorCode::Internal, error.clone()),
            );
            status.set(BluetoothCaptureStatus {
                state: BluetoothCaptureState::Failed,
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
            if stopped_for_shutdown || reason == BluetoothCaptureStopReason::UserRequested {
                operations.cancel(managed_id, "Bluetooth capture stopped");
            } else {
                operations.succeed(managed_id);
            }
            status.set(BluetoothCaptureStatus {
                state: BluetoothCaptureState::Completed,
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
                "Bluetooth HCI capture completed"
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
    state: BluetoothCaptureState,
) -> BluetoothCaptureStatus
where
    Writer: CaptureFileWriter,
{
    BluetoothCaptureStatus {
        state,
        packet_count: active.packet_count,
        bytes_written: active.writer.bytes_written(),
        elapsed_ms: active.started.elapsed().as_millis() as u64,
        duration_seconds: Some(active.duration_seconds),
        stop_reason: None,
        error: None,
    }
}

pub fn validate_duration(duration_seconds: u64) -> Result<(), String> {
    if (MIN_BLUETOOTH_CAPTURE_DURATION_SECONDS..=MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS)
        .contains(&duration_seconds)
    {
        Ok(())
    } else {
        Err(format!(
            "Bluetooth capture duration must be between {MIN_BLUETOOTH_CAPTURE_DURATION_SECONDS} and {MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS} seconds"
        ))
    }
}

fn can_write<Writer>(writer: &Writer, frame: &BtFrame) -> Result<bool, String>
where
    Writer: CaptureFileWriter,
{
    if frame.h4.len() > MAX_PACKET_BYTES {
        return Err("Bluetooth packet exceeds the 64 KiB snapshot limit".into());
    }
    Ok(writer
        .bytes_written()
        .saturating_add(20)
        .saturating_add(frame.h4.len() as u64)
        <= MAX_CAPTURE_BYTES)
}

fn packet_direction(kind: BtPacketKind) -> Result<u32, String> {
    match kind {
        BtPacketKind::HciCmd | BtPacketKind::AclSent | BtPacketKind::ScoSent => Ok(0),
        BtPacketKind::HciEvt | BtPacketKind::AclRecv | BtPacketKind::ScoRecv => Ok(1),
        BtPacketKind::Other(value) => Err(format!("unsupported Bluetooth packet kind: {value}")),
    }
}

fn encode_record(frame: &BtFrame) -> Result<Vec<u8>, String> {
    if frame.h4.len() > MAX_PACKET_BYTES {
        return Err("Bluetooth packet exceeds the 64 KiB snapshot limit".into());
    }
    let body_length = frame.h4.len().saturating_add(4);
    let length = u32::try_from(body_length)
        .map_err(|_| "Bluetooth packet length cannot be represented in PCAP".to_string())?;
    let mut record = Vec::with_capacity(16 + body_length);
    record.extend_from_slice(&frame.hdr.ts_secs.to_be_bytes());
    record.extend_from_slice(&frame.hdr.ts_usecs.to_be_bytes());
    record.extend_from_slice(&length.to_be_bytes());
    record.extend_from_slice(&length.to_be_bytes());
    record.extend_from_slice(&packet_direction(frame.kind)?.to_be_bytes());
    record.extend_from_slice(&frame.h4);
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::bt_packet_logger::{BtHeader, BtPacketKind};

    fn frame(kind: BtPacketKind, h4: Vec<u8>) -> BtFrame {
        BtFrame {
            hdr: BtHeader {
                length: h4.len() as u32,
                ts_secs: 0x0102_0304,
                ts_usecs: 0x0506_0708,
            },
            kind,
            h4,
        }
    }

    #[test]
    fn pcap_record_contains_direction_and_h4_payload() {
        let record = encode_record(&frame(BtPacketKind::AclRecv, vec![0x02, 0xaa, 0xbb])).unwrap();
        assert_eq!(&record[0..4], &[1, 2, 3, 4]);
        assert_eq!(&record[4..8], &[5, 6, 7, 8]);
        assert_eq!(&record[8..12], &[0, 0, 0, 7]);
        assert_eq!(&record[16..20], &[0, 0, 0, 1]);
        assert_eq!(&record[20..], &[0x02, 0xaa, 0xbb]);
    }

    #[test]
    fn sent_and_received_packets_use_distinct_direction_flags() {
        assert_eq!(packet_direction(BtPacketKind::HciCmd).unwrap(), 0);
        assert_eq!(packet_direction(BtPacketKind::HciEvt).unwrap(), 1);
        assert!(packet_direction(BtPacketKind::Other(99)).is_err());
    }

    #[test]
    fn capture_duration_is_bounded() {
        assert!(validate_duration(MIN_BLUETOOTH_CAPTURE_DURATION_SECONDS).is_ok());
        assert!(validate_duration(MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS).is_ok());
        assert!(validate_duration(0).is_err());
        assert!(validate_duration(MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS + 1).is_err());
    }
}
