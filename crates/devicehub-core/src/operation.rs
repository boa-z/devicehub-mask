//! Shared lifecycle metadata for bounded, device-scoped operations.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const MAX_RETAINED_OPERATIONS: usize = 32;
const MAX_OPERATION_TEXT_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedOperationKind {
    AppUninstall,
    AppDocumentExport,
    AppDocumentImport,
    DeviceFileExport,
    DeviceFileImport,
    DeviceBackup,
    Sysdiagnose,
    LogArchive,
    NetworkCapture,
    BluetoothCapture,
    DeveloperImageMount,
    DeveloperImageUnmount,
    WdaRunner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedOperationPhase {
    Running,
    Cancelling,
    Succeeded,
    Cancelled,
    Failed,
}

impl ManagedOperationPhase {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Cancelling)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationErrorCode {
    InvalidRequest,
    Busy,
    Unavailable,
    Timeout,
    Cancelled,
    DeviceLocked,
    PermissionDenied,
    Transport,
    Unsupported,
    Internal,
}

impl OperationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Busy => "busy",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::DeviceLocked => "device_locked",
            Self::PermissionDenied => "permission_denied",
            Self::Transport => "transport",
            Self::Unsupported => "unsupported",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationSuggestedAction {
    Retry,
    UnlockDevice,
    ReconnectDevice,
    CloseDeveloperTools,
    CheckPermissions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManagedOperationError {
    pub code: OperationErrorCode,
    pub message: String,
    pub retryable: bool,
    pub suggested_action: Option<OperationSuggestedAction>,
}

impl ManagedOperationError {
    pub fn new(code: OperationErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: bounded_text(message),
            retryable: false,
            suggested_action: None,
        }
    }

    pub fn retryable(mut self, action: OperationSuggestedAction) -> Self {
        self.retryable = true;
        self.suggested_action = Some(action);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManagedOperation {
    pub id: u64,
    pub kind: ManagedOperationKind,
    pub phase: ManagedOperationPhase,
    pub stage: Option<String>,
    pub label: Option<String>,
    pub progress_percent: Option<f64>,
    pub cancellable: bool,
    pub started_at_ms: u64,
    pub updated_at_ms: u64,
    pub error: Option<ManagedOperationError>,
}

/// Lightweight projection suitable for inventories that include many devices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ManagedOperationSummary {
    pub active_count: usize,
    pub failed_count: usize,
    pub latest_updated_at_ms: Option<u64>,
}

#[derive(Debug, Default)]
struct ManagedOperationRegistryInner {
    next_id: u64,
    operations: VecDeque<ManagedOperation>,
}

/// Per-device observation and coordination port for long-running operations.
#[derive(Clone, Debug, Default)]
pub struct ManagedOperationRegistry(Arc<Mutex<ManagedOperationRegistryInner>>);

impl ManagedOperationRegistry {
    pub fn begin(
        &self,
        kind: ManagedOperationKind,
        label: Option<String>,
        cancellable: bool,
    ) -> Result<u64, ManagedOperationError> {
        let mut inner = self.0.lock().expect("managed operation lock poisoned");
        if inner
            .operations
            .iter()
            .any(|operation| operation.kind == kind && operation.phase.is_active())
        {
            return Err(ManagedOperationError::new(
                OperationErrorCode::Busy,
                "an operation of this kind is already running",
            ));
        }
        while inner.operations.len() >= MAX_RETAINED_OPERATIONS {
            let Some(index) = inner
                .operations
                .iter()
                .position(|operation| !operation.phase.is_active())
            else {
                return Err(ManagedOperationError::new(
                    OperationErrorCode::Busy,
                    "too many device operations are running",
                ));
            };
            inner.operations.remove(index);
        }
        inner.next_id = inner.next_id.wrapping_add(1).max(1);
        let id = inner.next_id;
        let now = unix_millis();
        inner.operations.push_back(ManagedOperation {
            id,
            kind,
            phase: ManagedOperationPhase::Running,
            stage: None,
            label: label.map(bounded_text),
            progress_percent: None,
            cancellable,
            started_at_ms: now,
            updated_at_ms: now,
            error: None,
        });
        Ok(id)
    }

    pub fn update(&self, id: u64, stage: Option<String>, progress_percent: Option<f64>) {
        self.update_active(id, |operation| {
            operation.stage = stage.map(bounded_text);
            operation.progress_percent =
                progress_percent.map(|progress| progress.clamp(0.0, 100.0));
        });
    }

    pub fn request_cancel(&self, id: u64) -> bool {
        let mut changed = false;
        self.update_active(id, |operation| {
            if operation.cancellable {
                operation.phase = ManagedOperationPhase::Cancelling;
                changed = true;
            }
        });
        changed
    }

    pub fn succeed(&self, id: u64) {
        self.finish(id, ManagedOperationPhase::Succeeded, None);
    }

    pub fn cancel(&self, id: u64, message: impl Into<String>) {
        self.finish(
            id,
            ManagedOperationPhase::Cancelled,
            Some(ManagedOperationError::new(
                OperationErrorCode::Cancelled,
                message,
            )),
        );
    }

    pub fn fail(&self, id: u64, error: ManagedOperationError) {
        self.finish(id, ManagedOperationPhase::Failed, Some(error));
    }

    pub fn cancel_all(&self, message: impl Into<String>) {
        let message = bounded_text(message);
        let mut inner = self.0.lock().expect("managed operation lock poisoned");
        let now = unix_millis();
        for operation in &mut inner.operations {
            if operation.phase.is_active() {
                operation.phase = ManagedOperationPhase::Cancelled;
                operation.updated_at_ms = now;
                operation.error = Some(ManagedOperationError::new(
                    OperationErrorCode::Cancelled,
                    message.clone(),
                ));
            }
        }
    }

    pub fn snapshot(&self) -> Vec<ManagedOperation> {
        self.0
            .lock()
            .expect("managed operation lock poisoned")
            .operations
            .iter()
            .rev()
            .cloned()
            .collect()
    }

    pub fn summary(&self) -> ManagedOperationSummary {
        let inner = self.0.lock().expect("managed operation lock poisoned");
        ManagedOperationSummary {
            active_count: inner
                .operations
                .iter()
                .filter(|operation| operation.phase.is_active())
                .count(),
            failed_count: inner
                .operations
                .iter()
                .filter(|operation| operation.phase == ManagedOperationPhase::Failed)
                .count(),
            latest_updated_at_ms: inner
                .operations
                .iter()
                .map(|operation| operation.updated_at_ms)
                .max(),
        }
    }

    fn update_active(&self, id: u64, update: impl FnOnce(&mut ManagedOperation)) {
        let mut inner = self.0.lock().expect("managed operation lock poisoned");
        if let Some(operation) = inner
            .operations
            .iter_mut()
            .find(|operation| operation.id == id && operation.phase.is_active())
        {
            update(operation);
            operation.updated_at_ms = unix_millis();
        }
    }

    fn finish(&self, id: u64, phase: ManagedOperationPhase, error: Option<ManagedOperationError>) {
        self.update_active(id, |operation| {
            operation.phase = phase;
            operation.stage = None;
            if phase == ManagedOperationPhase::Succeeded {
                operation.progress_percent = Some(100.0);
            }
            operation.error = error;
        });
    }
}

fn bounded_text(value: impl Into<String>) -> String {
    value
        .into()
        .chars()
        .take(MAX_OPERATION_TEXT_CHARS)
        .collect()
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
    fn registry_tracks_lifecycle_and_rejects_same_kind_concurrency() {
        let registry = ManagedOperationRegistry::default();
        let id = registry
            .begin(ManagedOperationKind::DeviceBackup, None, true)
            .unwrap();
        assert!(
            registry
                .begin(ManagedOperationKind::DeviceBackup, None, true)
                .is_err()
        );
        registry.update(id, Some("copying".into()), Some(120.0));
        assert!(registry.request_cancel(id));
        registry.cancel(id, "session ended");

        let operation = registry.snapshot().remove(0);
        assert_eq!(operation.phase, ManagedOperationPhase::Cancelled);
        assert_eq!(operation.progress_percent, Some(100.0));
        assert_eq!(registry.summary().active_count, 0);
        assert_eq!(registry.summary().failed_count, 0);
        assert_eq!(
            registry.summary().latest_updated_at_ms,
            Some(operation.updated_at_ms)
        );
        assert_eq!(operation.error.unwrap().code, OperationErrorCode::Cancelled);
    }
}
