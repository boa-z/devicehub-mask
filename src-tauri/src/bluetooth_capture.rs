//! Bounded, user-initiated Bluetooth HCI capture through BTPacketLogger.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{Stream, StreamExt};
use idevice::RsdService;
use idevice::bt_packet_logger::{BtFrame, BtPacketKind, BtPacketLoggerClient};
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, watch};

use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
pub const MIN_DURATION_SECONDS: u64 = 1;
pub const MAX_DURATION_SECONDS: u64 = 300;
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BluetoothCaptureState {
    #[default]
    Idle,
    Starting,
    Capturing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BluetoothCaptureStopReason {
    UserRequested,
    DurationLimit,
    SizeLimit,
    SessionEnded,
    StreamEnded,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BluetoothCaptureStatus {
    pub state: BluetoothCaptureState,
    pub packet_count: u64,
    pub bytes_written: u64,
    pub elapsed_ms: u64,
    pub duration_seconds: Option<u64>,
    pub stop_reason: Option<BluetoothCaptureStopReason>,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct BluetoothCaptureSlot(Arc<Mutex<BluetoothCaptureStatus>>);

impl BluetoothCaptureSlot {
    pub fn set(&self, status: BluetoothCaptureStatus) {
        *self.0.lock().unwrap() = status;
    }

    pub fn get(&self) -> BluetoothCaptureStatus {
        self.0.lock().unwrap().clone()
    }

    pub fn reset(&self) {
        self.set(BluetoothCaptureStatus::default());
    }
}

#[derive(Debug)]
pub enum BluetoothCaptureCommand {
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
            .map_err(|error| format!("unable to create Bluetooth capture file: {error}"))?;
        if let Err(error) = file.write_all(&PCAP_HEADER).await {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(format!("unable to write Bluetooth capture header: {error}"));
        }
        Ok(Self {
            file,
            temporary,
            destination,
            bytes_written: PCAP_HEADER.len() as u64,
        })
    }

    fn can_write(&self, frame: &BtFrame) -> Result<bool, String> {
        if frame.h4.len() > MAX_PACKET_BYTES {
            return Err("Bluetooth packet exceeds the 64 KiB snapshot limit".into());
        }
        Ok(self
            .bytes_written
            .saturating_add(20)
            .saturating_add(frame.h4.len() as u64)
            <= MAX_CAPTURE_BYTES)
    }

    async fn write_frame(&mut self, frame: &BtFrame) -> Result<(), String> {
        let record = encode_record(frame)?;
        self.file
            .write_all(&record)
            .await
            .map_err(|error| format!("unable to write Bluetooth capture data: {error}"))?;
        self.bytes_written = self.bytes_written.saturating_add(record.len() as u64);
        Ok(())
    }

    async fn finish(mut self) -> Result<u64, String> {
        let result = async {
            self.file
                .flush()
                .await
                .map_err(|error| format!("unable to flush Bluetooth capture: {error}"))?;
            self.file
                .sync_data()
                .await
                .map_err(|error| format!("unable to synchronize Bluetooth capture: {error}"))?;
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
    stream: Pin<Box<dyn Stream<Item = Result<BtFrame, idevice::IdeviceError>> + Send>>,
    writer: CaptureWriter,
    duration_seconds: u64,
    started: Instant,
    packet_count: u64,
}

pub async fn serve(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    mut commands: mpsc::Receiver<BluetoothCaptureCommand>,
    status: BluetoothCaptureSlot,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
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
                attempt += 1;
                status.set(BluetoothCaptureStatus {
                    state: BluetoothCaptureState::Starting,
                    duration_seconds: Some(duration_seconds),
                    ..BluetoothCaptureStatus::default()
                });
                reporter.connecting(attempt);
                let active =
                    begin_capture(&mut adapter, &mut handshake, destination, duration_seconds)
                        .await;
                let active = match active {
                    Ok(active) => active,
                    Err(error) => {
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
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    destination: PathBuf,
    duration_seconds: u64,
) -> Result<ActiveCapture, String> {
    validate_request(&destination, duration_seconds).await?;
    let client = tokio::time::timeout(
        CONNECT_TIMEOUT,
        BtPacketLoggerClient::connect_rsd(adapter, handshake),
    )
    .await
    .map_err(|_| "Bluetooth packet logger connection timed out".to_string())?
    .map_err(|error| format!("Bluetooth packet logger unavailable: {error:?}"))?;
    let writer = CaptureWriter::create(destination).await?;
    Ok(ActiveCapture {
        stream: client.into_stream(),
        writer,
        duration_seconds,
        started: Instant::now(),
        packet_count: 0,
    })
}

async fn capture(
    mut active: ActiveCapture,
    commands: &mut mpsc::Receiver<BluetoothCaptureCommand>,
    status: &BluetoothCaptureSlot,
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
                    break BluetoothCaptureStopReason::SessionEnded;
                }
            }
            _ = &mut deadline => break BluetoothCaptureStopReason::DurationLimit,
            _ = status_tick.tick() => {
                status.set(capture_status(&active, BluetoothCaptureState::Capturing));
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
                Some(Ok(frame)) => match active.writer.can_write(&frame) {
                    Ok(true) => {
                        if let Err(error) = active.writer.write_frame(&frame).await {
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

fn capture_status(active: &ActiveCapture, state: BluetoothCaptureState) -> BluetoothCaptureStatus {
    BluetoothCaptureStatus {
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
            "Bluetooth capture duration must be between {MIN_DURATION_SECONDS} and {MAX_DURATION_SECONDS} seconds"
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
        return Err("Bluetooth capture destination must be an absolute .pcap path".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Bluetooth capture destination has no parent directory".to_string())?;
    let metadata = tokio::fs::metadata(parent)
        .await
        .map_err(|error| format!("unable to access Bluetooth capture directory: {error}"))?;
    if !metadata.is_dir() {
        return Err("Bluetooth capture parent is not a directory".into());
    }
    match tokio::fs::metadata(path).await {
        Ok(metadata) if !metadata.is_file() => {
            Err("Bluetooth capture destination is not a regular file".into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "unable to inspect Bluetooth capture destination: {error}"
        )),
    }
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

fn temporary_sibling(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "Bluetooth capture destination has no parent directory".to_string())?;
    Ok(parent.join(format!(
        ".devicehub-bluetooth-{}-{}.part",
        std::process::id(),
        uuid::Uuid::new_v4()
    )))
}

async fn replace_local_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    let backup = destination.with_file_name(format!(
        ".devicehub-bluetooth-backup-{}-{}.part",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let had_destination = match tokio::fs::metadata(destination).await {
        Ok(metadata) if metadata.is_file() => {
            tokio::fs::rename(destination, &backup)
                .await
                .map_err(|error| {
                    format!("unable to preserve existing Bluetooth capture: {error}")
                })?;
            true
        }
        Ok(_) => return Err("Bluetooth capture destination is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "unable to inspect Bluetooth capture destination: {error}"
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
            Err(format!("unable to finish Bluetooth capture: {error}"))
        }
    }
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
        let destination = std::env::temp_dir().join(format!(
            "devicehub-mask-bluetooth-{}.pcap",
            uuid::Uuid::new_v4()
        ));
        assert!(
            validate_destination(&destination.with_extension("txt"))
                .await
                .is_err()
        );
        assert!(validate_destination(&destination).await.is_ok());
    }

    #[tokio::test]
    async fn writer_creates_a_complete_bluetooth_pcap() {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-bluetooth-test-{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir(&directory).await.unwrap();
        let destination = directory.join("bluetooth.pcap");
        let mut writer = CaptureWriter::create(destination.clone()).await.unwrap();
        writer
            .write_frame(&frame(BtPacketKind::HciEvt, vec![0x04, 0x0e]))
            .await
            .unwrap();
        let bytes_written = writer.finish().await.unwrap();
        let contents = tokio::fs::read(&destination).await.unwrap();
        assert_eq!(bytes_written, (PCAP_HEADER.len() + 16 + 4 + 2) as u64);
        assert_eq!(&contents[..PCAP_HEADER.len()], &PCAP_HEADER);
        tokio::fs::remove_file(destination).await.unwrap();
        tokio::fs::remove_dir(directory).await.unwrap();
    }
}
