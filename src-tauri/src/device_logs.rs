//! On-demand, bounded device syslog collection.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idevice::IdeviceService;
use idevice::provider::IdeviceProvider;
use idevice::syslog_relay::SyslogRelayClient;
use serde::Serialize;
use tokio::sync::watch;

use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_ENTRIES: usize = 2_000;
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_BATCH_ENTRIES: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceLogEntry {
    pub sequence: u64,
    pub received_at_ms: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceLogBatch {
    pub entries: Vec<DeviceLogEntry>,
    pub oldest_sequence: Option<u64>,
    pub latest_sequence: Option<u64>,
    pub cursor_lagged: bool,
    pub has_more: bool,
    pub streaming: bool,
}

#[derive(Default)]
struct DeviceLogBuffer {
    entries: VecDeque<DeviceLogEntry>,
    next_sequence: u64,
}

#[derive(Clone, Default)]
pub struct DeviceLogSlot(Arc<Mutex<DeviceLogBuffer>>);

impl DeviceLogSlot {
    pub fn publish(&self, message: String) {
        let message = sanitize_message(&message);
        if message.is_empty() {
            return;
        }
        let mut buffer = self.0.lock().unwrap();
        buffer.next_sequence = buffer.next_sequence.saturating_add(1);
        let sequence = buffer.next_sequence;
        buffer.entries.push_back(DeviceLogEntry {
            sequence,
            received_at_ms: unix_millis(),
            message,
        });
        while buffer.entries.len() > MAX_ENTRIES {
            buffer.entries.pop_front();
        }
    }

    pub fn snapshot(&self, after: Option<u64>, limit: usize, streaming: bool) -> DeviceLogBatch {
        let buffer = self.0.lock().unwrap();
        let limit = limit.clamp(1, MAX_BATCH_ENTRIES);
        let oldest_sequence = buffer.entries.front().map(|entry| entry.sequence);
        let latest_sequence = buffer.entries.back().map(|entry| entry.sequence);
        let start = match after {
            Some(after) => buffer
                .entries
                .iter()
                .position(|entry| entry.sequence > after)
                .unwrap_or(buffer.entries.len()),
            None => buffer.entries.len().saturating_sub(limit),
        };
        let available = buffer.entries.len().saturating_sub(start);
        let entries = buffer
            .entries
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .collect();
        let cursor_fell_behind = after
            .zip(oldest_sequence)
            .is_some_and(|(after, oldest)| after.saturating_add(1) < oldest);
        DeviceLogBatch {
            entries,
            oldest_sequence,
            latest_sequence,
            cursor_lagged: cursor_fell_behind,
            has_more: available > limit,
            streaming,
        }
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().entries.clear();
    }
}

#[derive(Clone)]
pub struct DeviceLogDemand(watch::Sender<bool>);

impl Default for DeviceLogDemand {
    fn default() -> Self {
        let (sender, _) = watch::channel(false);
        Self(sender)
    }
}

impl DeviceLogDemand {
    pub fn set(&self, enabled: bool) {
        self.0.send_replace(enabled);
    }

    pub fn enabled(&self) -> bool {
        *self.0.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }
}

pub async fn supervise(
    provider: Arc<dyn IdeviceProvider>,
    slot: DeviceLogSlot,
    reporter: ServiceReporter,
    mut enabled: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        if !wait_until_enabled(&mut enabled, &mut shutdown, &reporter, attempt).await {
            break;
        }
        attempt += 1;
        reporter.connecting(attempt);
        let result = run_once(
            provider.clone(),
            slot.clone(),
            &reporter,
            attempt,
            &mut enabled,
            &mut shutdown,
        )
        .await;
        if *shutdown.borrow() {
            break;
        }
        let Some(error) = result.err() else {
            continue;
        };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

async fn run_once(
    provider: Arc<dyn IdeviceProvider>,
    slot: DeviceLogSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    enabled: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let mut client = tokio::time::timeout(
        CONNECT_TIMEOUT,
        SyslogRelayClient::connect(provider.as_ref()),
    )
    .await
    .map_err(|_| "device syslog connection timed out".to_string())?
    .map_err(|error| format!("device syslog connection failed: {error:?}"))?;
    reporter.ready(attempt);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    return Ok(());
                }
            }
            line = client.next() => match line {
                Ok(line) => slot.publish(line),
                Err(error) => return Err(format!("device syslog stream failed: {error:?}")),
            }
        }
    }
}

async fn wait_until_enabled(
    enabled: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> bool {
    while !*enabled.borrow() {
        reporter.stopped(attempt);
        tokio::select! {
            changed = enabled.changed() => {
                if changed.is_err() {
                    return false;
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return false;
                }
            }
        }
    }
    true
}

fn sanitize_message(message: &str) -> String {
    let mut sanitized = String::with_capacity(message.len().min(MAX_MESSAGE_BYTES));
    for character in message.chars() {
        if sanitized.len() + character.len_utf8() > MAX_MESSAGE_BYTES {
            break;
        }
        if character == '\t' || !character.is_control() {
            sanitized.push(character);
        } else if !sanitized.ends_with(' ') {
            sanitized.push(' ');
        }
    }
    sanitized.trim().to_owned()
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    #[test]
    fn device_log_buffer_is_bounded_and_detects_lagging_cursors() {
        let slot = DeviceLogSlot::default();
        for index in 0..=MAX_ENTRIES {
            slot.publish(format!("line {index}"));
        }
        let batch = slot.snapshot(Some(0), MAX_BATCH_ENTRIES, true);
        assert_eq!(batch.entries.len(), MAX_BATCH_ENTRIES);
        assert_eq!(batch.oldest_sequence, Some(2));
        assert_eq!(batch.latest_sequence, Some((MAX_ENTRIES + 1) as u64));
        assert!(batch.cursor_lagged);
        assert!(batch.has_more);
        assert!(batch.streaming);
    }

    #[test]
    fn device_log_snapshot_returns_latest_entries_without_a_cursor() {
        let slot = DeviceLogSlot::default();
        for index in 0..10 {
            slot.publish(format!("line {index}"));
        }
        let batch = slot.snapshot(None, 3, false);
        assert_eq!(
            batch
                .entries
                .iter()
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>(),
            vec![8, 9, 10]
        );
        assert!(!batch.cursor_lagged);
        assert!(!batch.has_more);
    }

    #[test]
    fn device_log_messages_are_sanitized_and_utf8_safe() {
        assert_eq!(sanitize_message("hello\r\nworld\0"), "hello world");
        let oversized = "界".repeat(MAX_MESSAGE_BYTES);
        let sanitized = sanitize_message(&oversized);
        assert!(sanitized.len() <= MAX_MESSAGE_BYTES);
        assert!(sanitized.is_char_boundary(sanitized.len()));
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn reads_syslog_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-device-log-test");
        let mut client = SyslogRelayClient::connect(&provider).await.unwrap();
        let line = tokio::time::timeout(Duration::from_secs(10), client.next())
            .await
            .expect("timed out waiting for syslog")
            .unwrap();
        assert!(!sanitize_message(&line).is_empty());
    }
}
