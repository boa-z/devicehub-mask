//! Tokio filesystem adapter for optional runtime diagnostic byte sinks.

use std::path::PathBuf;

use devicehub_runtime::{DiagnosticDumpSinkFactory, DiagnosticDumpSinkFuture};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TokioDiagnosticDumpSinks;

impl DiagnosticDumpSinkFactory for TokioDiagnosticDumpSinks {
    type Source = PathBuf;

    fn open<'a>(
        &'a self,
        source: Option<PathBuf>,
        capacity: usize,
        label: &'static str,
    ) -> DiagnosticDumpSinkFuture<'a> {
        Box::pin(open(source, capacity, label))
    }
}

async fn open(
    source: Option<PathBuf>,
    capacity: usize,
    label: &'static str,
) -> Option<mpsc::Sender<Vec<u8>>> {
    let path = source?;
    let mut file = match tokio::fs::File::create(&path).await {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, %label, "could not open diagnostic dump");
            return None;
        }
    };
    tracing::info!(path = %path.display(), %label, "opened diagnostic dump");
    let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(capacity);
    tokio::spawn(async move {
        while let Some(bytes) = receiver.recv().await {
            if let Err(error) = file.write_all(&bytes).await {
                tracing::warn!(path = %path.display(), %error, %label, "diagnostic dump stopped");
                break;
            }
        }
    });
    Some(sender)
}
