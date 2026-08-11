//! Device-scoped control surface for managed long-running operations.

use std::fmt;
use std::time::Duration;

use devicehub_core::{
    AppDocumentActivitySlot, DeviceFileActivitySlot, ManagedOperationKind, ManagedOperationRegistry,
};
use tokio::sync::oneshot;

use crate::{
    BluetoothCaptureCommand, DeveloperImageMountCommand, DeviceBackupCommand, DeviceSessionCommand,
    LogArchiveCommand, NetworkCaptureCommand, SessionCommandSlot, SysdiagnoseCommand,
    WdaRunnerCommand,
};

const CANCEL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedOperationCancelError {
    NotFound,
    NotActive,
    NotCancellable,
    ServiceUnavailable,
    Rejected(String),
    TimedOut,
}

impl fmt::Display for ManagedOperationCancelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("operation was not found"),
            Self::NotActive => formatter.write_str("operation is no longer running"),
            Self::NotCancellable => formatter.write_str("operation cannot be cancelled"),
            Self::ServiceUnavailable => {
                formatter.write_str("device operation service is unavailable")
            }
            Self::Rejected(error) => formatter.write_str(error),
            Self::TimedOut => formatter.write_str("operation cancellation timed out"),
        }
    }
}

impl std::error::Error for ManagedOperationCancelError {}

/// Routes a managed operation ID to the runtime service that owns its
/// cancellation semantics. HTTP, MCP, and host adapters do not reconstruct
/// service-specific stop commands.
pub struct ManagedOperationController<HostPath> {
    registry: ManagedOperationRegistry,
    commands: SessionCommandSlot<HostPath>,
    app_documents: AppDocumentActivitySlot,
    device_files: DeviceFileActivitySlot,
}

impl<HostPath> Clone for ManagedOperationController<HostPath> {
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            commands: self.commands.clone(),
            app_documents: self.app_documents.clone(),
            device_files: self.device_files.clone(),
        }
    }
}

impl<HostPath> ManagedOperationController<HostPath> {
    pub(crate) fn new(
        registry: ManagedOperationRegistry,
        commands: SessionCommandSlot<HostPath>,
        app_documents: AppDocumentActivitySlot,
        device_files: DeviceFileActivitySlot,
    ) -> Self {
        Self {
            registry,
            commands,
            app_documents,
            device_files,
        }
    }
}

impl<HostPath> ManagedOperationController<HostPath>
where
    HostPath: Send + 'static,
{
    pub async fn cancel(&self, id: u64) -> Result<(), ManagedOperationCancelError> {
        let operation = self
            .registry
            .snapshot()
            .into_iter()
            .find(|operation| operation.id == id)
            .ok_or(ManagedOperationCancelError::NotFound)?;
        if !operation.phase.is_active() {
            return Err(ManagedOperationCancelError::NotActive);
        }
        if !operation.cancellable {
            return Err(ManagedOperationCancelError::NotCancellable);
        }
        if !self.registry.request_cancel(id) {
            return Err(ManagedOperationCancelError::NotActive);
        }

        let result = match operation.kind {
            ManagedOperationKind::AppDocumentExport | ManagedOperationKind::AppDocumentImport => {
                operation
                    .label
                    .as_deref()
                    .filter(|bundle_id| self.app_documents.cancel(bundle_id))
                    .map(|_| ())
                    .ok_or(ManagedOperationCancelError::NotActive)
            }
            ManagedOperationKind::DeviceFileExport | ManagedOperationKind::DeviceFileImport => self
                .device_files
                .cancel()
                .then_some(())
                .ok_or(ManagedOperationCancelError::NotActive),
            ManagedOperationKind::DeviceBackup => {
                self.stop(|reply| {
                    DeviceSessionCommand::DeviceBackup(DeviceBackupCommand::Stop { reply })
                })
                .await
            }
            ManagedOperationKind::Sysdiagnose => {
                self.stop(|reply| {
                    DeviceSessionCommand::Sysdiagnose(SysdiagnoseCommand::Stop { reply })
                })
                .await
            }
            ManagedOperationKind::LogArchive => {
                self.stop(|reply| {
                    DeviceSessionCommand::LogArchive(LogArchiveCommand::Stop { reply })
                })
                .await
            }
            ManagedOperationKind::NetworkCapture => {
                self.stop(|reply| {
                    DeviceSessionCommand::NetworkCapture(NetworkCaptureCommand::Stop { reply })
                })
                .await
            }
            ManagedOperationKind::BluetoothCapture => {
                self.stop(|reply| {
                    DeviceSessionCommand::BluetoothCapture(BluetoothCaptureCommand::Stop { reply })
                })
                .await
            }
            ManagedOperationKind::DeveloperImageMount => {
                self.stop(|reply| {
                    DeviceSessionCommand::DeveloperImageMount(DeveloperImageMountCommand::Stop {
                        reply,
                    })
                })
                .await
            }
            ManagedOperationKind::WdaRunner => {
                self.stop(|reply| DeviceSessionCommand::WdaRunner(WdaRunnerCommand::Stop { reply }))
                    .await
            }
            ManagedOperationKind::AppUninstall | ManagedOperationKind::DeveloperImageUnmount => {
                Err(ManagedOperationCancelError::NotCancellable)
            }
        };
        if result.is_err() {
            self.registry.cancel_request_failed(id);
        }
        result
    }

    async fn stop<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, String>>) -> DeviceSessionCommand<HostPath>,
    ) -> Result<(), ManagedOperationCancelError>
    where
        T: Send + 'static,
    {
        let (reply, response) = oneshot::channel();
        if !self.commands.try_send(command(reply)) {
            return Err(ManagedOperationCancelError::ServiceUnavailable);
        }
        let result = tokio::time::timeout(CANCEL_RESPONSE_TIMEOUT, response).await;
        match result {
            Ok(Ok(Ok(_))) => Ok(()),
            Ok(Ok(Err(error))) => Err(ManagedOperationCancelError::Rejected(error)),
            Ok(Err(_)) => Err(ManagedOperationCancelError::ServiceUnavailable),
            Err(_) => Err(ManagedOperationCancelError::TimedOut),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::ManagedOperationPhase;
    use tokio::sync::mpsc::unbounded_channel;

    #[tokio::test]
    async fn cancellation_routes_to_the_owning_service() {
        let registry = ManagedOperationRegistry::default();
        let id = registry
            .begin(ManagedOperationKind::NetworkCapture, None, true)
            .unwrap();
        let commands = SessionCommandSlot::default();
        let (sender, mut receiver) = unbounded_channel();
        commands.set(Some(sender));
        let controller = ManagedOperationController::<std::path::PathBuf>::new(
            registry.clone(),
            commands,
            AppDocumentActivitySlot::default(),
            DeviceFileActivitySlot::default(),
        );

        let cancellation = tokio::spawn(async move { controller.cancel(id).await });
        let DeviceSessionCommand::NetworkCapture(NetworkCaptureCommand::Stop { reply }) =
            receiver.recv().await.unwrap()
        else {
            panic!("unexpected cancellation command");
        };
        reply.send(Ok(())).unwrap();
        cancellation.await.unwrap().unwrap();
        assert_eq!(
            registry.snapshot()[0].phase,
            ManagedOperationPhase::Cancelling
        );
    }
}
