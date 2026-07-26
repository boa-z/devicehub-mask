//! Active-session command execution after management routing.

use std::future::Future;
use std::pin::Pin;

use devicehub_core::{KeyMods, ascii_key_usage};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{DeviceSessionCommand, DeviceSessionRouter};
use crate::{DeviceInputCommand, DeviceInputDispatcher};

pub type ClipboardWriteFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Minimal device-clipboard capability required by the paste command.
///
/// The host adapter retains ownership of pasteboard reconnection and optional
/// host clipboard synchronization. Runtime orchestration only prepares text
/// before sending the HID paste chord.
pub trait DeviceClipboard: Send + Sync {
    fn set_text(&self, text: String) -> ClipboardWriteFuture<'_>;
}

/// Runs commands for a session with authenticated HID and clipboard services.
pub async fn run_device_command_loop<HostPath>(
    mut device_input: DeviceInputDispatcher,
    mut router: DeviceSessionRouter<HostPath>,
    commands: &mut UnboundedReceiver<DeviceSessionCommand<HostPath>>,
    clipboard: &dyn DeviceClipboard,
) where
    HostPath: Send + 'static,
{
    while let Some(command) = commands.recv().await {
        if matches!(command, DeviceSessionCommand::Shutdown) {
            break;
        }
        let Some(command) = router.handle(command).await else {
            continue;
        };
        match command {
            DeviceSessionCommand::PasteText { text, reply } => {
                let result = paste_text(&mut device_input, clipboard, text).await;
                let _ = reply.send(result);
            }
            DeviceSessionCommand::DeviceInput(command) => {
                if let Err(error) = device_input.dispatch(command).await {
                    tracing::warn!(?error, "input dispatch failed");
                }
            }
            DeviceSessionCommand::Shutdown => break,
            _ => tracing::warn!("session router returned an unhandled command"),
        }
    }
}

/// Runs commands when screen control and HID setup failed but management
/// services remain available.
pub async fn run_management_command_loop<HostPath>(
    mut router: DeviceSessionRouter<HostPath>,
    commands: &mut UnboundedReceiver<DeviceSessionCommand<HostPath>>,
) where
    HostPath: Send + 'static,
{
    while let Some(command) = commands.recv().await {
        if matches!(command, DeviceSessionCommand::Shutdown) {
            break;
        }
        let Some(command) = router.handle(command).await else {
            continue;
        };
        match command {
            DeviceSessionCommand::PasteText { reply, .. } => {
                let _ = reply.send(Err("device control is unavailable".into()));
            }
            DeviceSessionCommand::DeviceInput(_) => {
                tracing::debug!("ignoring device input while screen control is unavailable");
            }
            DeviceSessionCommand::Shutdown => break,
            _ => tracing::warn!("session router returned an unhandled command"),
        }
    }
}

async fn paste_text(
    device_input: &mut DeviceInputDispatcher,
    clipboard: &dyn DeviceClipboard,
    text: String,
) -> Result<(), String> {
    clipboard.set_text(text).await?;
    let (paste_usage, _) = ascii_key_usage('v').expect("ASCII v must have a keyboard usage");
    device_input
        .dispatch(DeviceInputCommand::KeyCombo {
            usage: paste_usage,
            mods: KeyMods {
                cmd: true,
                ..KeyMods::default()
            },
        })
        .await
        .map_err(|error| format!("unable to send paste shortcut: {error:?}"))
}
