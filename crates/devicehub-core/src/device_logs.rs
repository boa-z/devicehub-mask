//! Normalized, bounded device log observations shared by every host adapter.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Maximum number of entries returned by one device-log observation.
pub const MAX_DEVICE_LOG_BATCH_ENTRIES: usize = 500;
const MAX_DEVICE_LOG_ENTRIES: usize = 2_000;
const MAX_DEVICE_LOG_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_DEVICE_LOG_METADATA_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceLogSource {
    Unified,
    Syslog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceLogLevel {
    Notice,
    Info,
    Debug,
    Error,
    Fault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceLogEntry {
    pub sequence: u64,
    pub received_at_ms: u64,
    pub message: String,
    pub level: Option<DeviceLogLevel>,
    pub process: Option<String>,
    pub pid: Option<u32>,
    pub subsystem: Option<String>,
    pub category: Option<String>,
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceLogBatch {
    pub entries: Vec<DeviceLogEntry>,
    pub oldest_sequence: Option<u64>,
    pub latest_sequence: Option<u64>,
    pub cursor_lagged: bool,
    pub has_more: bool,
    pub streaming: bool,
    pub source: Option<DeviceLogSource>,
}

/// Normalized metadata accepted by the device-log observation port.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceLogMetadata {
    pub level: Option<DeviceLogLevel>,
    pub process: Option<String>,
    pub pid: Option<u32>,
    pub subsystem: Option<String>,
    pub category: Option<String>,
    pub filename: Option<String>,
}

#[derive(Default)]
struct DeviceLogBuffer {
    entries: VecDeque<DeviceLogEntry>,
    next_sequence: u64,
    source: Option<DeviceLogSource>,
}

/// Bounded, cloneable device-log observation port shared by all host adapters.
#[derive(Clone, Default)]
pub struct DeviceLogSlot(Arc<Mutex<DeviceLogBuffer>>);

impl DeviceLogSlot {
    pub fn publish(&self, message: String) {
        self.publish_structured(message, DeviceLogMetadata::default());
    }

    pub fn publish_structured(&self, message: String, metadata: DeviceLogMetadata) {
        let message = sanitize_message(&message);
        if message.is_empty() {
            return;
        }
        let mut buffer = self.0.lock().expect("device log buffer lock poisoned");
        buffer.next_sequence = buffer.next_sequence.saturating_add(1);
        let sequence = buffer.next_sequence;
        buffer.entries.push_back(DeviceLogEntry {
            sequence,
            received_at_ms: unix_millis(),
            message,
            level: metadata.level,
            process: sanitize_optional_metadata(metadata.process),
            pid: metadata.pid,
            subsystem: sanitize_optional_metadata(metadata.subsystem),
            category: sanitize_optional_metadata(metadata.category),
            filename: sanitize_optional_metadata(metadata.filename),
        });
        while buffer.entries.len() > MAX_DEVICE_LOG_ENTRIES {
            buffer.entries.pop_front();
        }
    }

    pub fn snapshot(&self, after: Option<u64>, limit: usize, streaming: bool) -> DeviceLogBatch {
        let buffer = self.0.lock().expect("device log buffer lock poisoned");
        let limit = limit.clamp(1, MAX_DEVICE_LOG_BATCH_ENTRIES);
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
        let cursor_lagged = after
            .zip(oldest_sequence)
            .is_some_and(|(after, oldest)| after.saturating_add(1) < oldest);
        DeviceLogBatch {
            entries,
            oldest_sequence,
            latest_sequence,
            cursor_lagged,
            has_more: available > limit,
            streaming,
            source: buffer.source,
        }
    }

    pub fn set_source(&self, source: Option<DeviceLogSource>) {
        self.0
            .lock()
            .expect("device log buffer lock poisoned")
            .source = source;
    }

    pub fn clear(&self) {
        self.0
            .lock()
            .expect("device log buffer lock poisoned")
            .entries
            .clear();
    }

    pub fn reset(&self) {
        let mut buffer = self.0.lock().expect("device log buffer lock poisoned");
        buffer.entries.clear();
        buffer.source = None;
    }
}

fn sanitize_message(message: &str) -> String {
    let mut sanitized = String::with_capacity(message.len().min(MAX_DEVICE_LOG_MESSAGE_BYTES));
    for character in message.chars() {
        if sanitized.len() + character.len_utf8() > MAX_DEVICE_LOG_MESSAGE_BYTES {
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

fn sanitize_optional_metadata(value: Option<String>) -> Option<String> {
    value
        .map(|value| sanitize_metadata(&value))
        .filter(|value| !value.is_empty())
}

fn sanitize_metadata(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len().min(MAX_DEVICE_LOG_METADATA_BYTES));
    for character in value.chars() {
        if sanitized.len() + character.len_utf8() > MAX_DEVICE_LOG_METADATA_BYTES {
            break;
        }
        if character.is_control() {
            if !sanitized.ends_with(' ') {
                sanitized.push(' ');
            }
        } else {
            sanitized.push(character);
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

    #[test]
    fn log_buffer_is_bounded_and_detects_lagging_cursors() {
        let slot = DeviceLogSlot::default();
        for index in 0..=MAX_DEVICE_LOG_ENTRIES {
            slot.publish(format!("line {index}"));
        }
        let batch = slot.snapshot(Some(0), MAX_DEVICE_LOG_BATCH_ENTRIES, true);
        assert_eq!(batch.entries.len(), MAX_DEVICE_LOG_BATCH_ENTRIES);
        assert_eq!(batch.oldest_sequence, Some(2));
        assert_eq!(
            batch.latest_sequence,
            Some((MAX_DEVICE_LOG_ENTRIES + 1) as u64)
        );
        assert!(batch.cursor_lagged);
        assert!(batch.has_more);
        assert!(batch.streaming);
    }

    #[test]
    fn log_snapshot_returns_latest_entries_without_a_cursor() {
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
    fn messages_and_structured_metadata_are_sanitized_and_bounded() {
        let slot = DeviceLogSlot::default();
        slot.set_source(Some(DeviceLogSource::Unified));
        slot.publish_structured(
            "hello\r\nworld\0".into(),
            DeviceLogMetadata {
                level: Some(DeviceLogLevel::Notice),
                process: Some(format!(
                    "Game\n{}",
                    "x".repeat(MAX_DEVICE_LOG_METADATA_BYTES * 2)
                )),
                pid: Some(42),
                subsystem: Some("com.example.network".into()),
                category: Some("connection".into()),
                filename: Some("Network.swift".into()),
            },
        );
        let batch = slot.snapshot(None, 10, true);
        let entry = &batch.entries[0];
        assert_eq!(entry.message, "hello world");
        assert_eq!(entry.level, Some(DeviceLogLevel::Notice));
        assert_eq!(entry.pid, Some(42));
        assert!(entry.process.as_ref().unwrap().len() <= MAX_DEVICE_LOG_METADATA_BYTES);
        assert!(
            !entry
                .process
                .as_ref()
                .unwrap()
                .chars()
                .any(char::is_control)
        );
    }
}
