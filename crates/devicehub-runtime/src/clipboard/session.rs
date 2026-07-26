//! Device Pasteboard lifecycle with a minimal injected host clipboard.

use std::time::Duration;

use devicehub_core::{ClipboardContentKind, ClipboardEvent, clipboard_preview};
use idevice::{
    IdeviceError, ReadWrite, RsdService,
    core_device::{
        DataInclusionPolicy, GENERAL_PASTEBOARD, PasteboardServiceClient, PasteboardSnapshot,
        UTI_PNG,
    },
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
};
use tokio::sync::mpsc::{Receiver, Sender};

use super::ClipboardSlot;
use crate::{ClipboardWriteFuture, DeviceClipboard};

const CLIPBOARD_POLL: Duration = Duration::from_millis(600);
const CLIPBOARD_PREVIEW_LEN: usize = 48;
const CLIPBOARD_COMMAND_TIMEOUT: Duration = Duration::from_secs(8);
const CLIPBOARD_COMMAND_CAPACITY: usize = 4;

/// Owned RGBA pixels exchanged with a host clipboard implementation.
pub struct ClipboardImage {
    pub width: usize,
    pub height: usize,
    pub bytes: Vec<u8>,
}

/// Small synchronous port implemented by desktop or future headless hosts.
/// Runtime code owns device Pasteboard protocol and synchronization policy.
pub trait HostClipboard {
    fn get_text(&mut self) -> Result<String, String>;
    fn set_text(&mut self, text: String) -> Result<(), String>;
    fn get_image(&mut self) -> Result<ClipboardImage, String>;
    fn set_image(&mut self, image: ClipboardImage) -> Result<(), String>;
}

/// Lazy host capability construction avoids touching the system clipboard when
/// synchronization is disabled or the device Pasteboard service is absent.
pub type HostClipboardFactory =
    Box<dyn FnOnce() -> Result<Box<dyn HostClipboard>, String> + 'static>;

enum ClipboardCommand {
    SetText {
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

enum ClipboardWake {
    Push(Result<PasteboardSnapshot, IdeviceError>),
    Tick,
    Command(Option<ClipboardCommand>),
}

struct ClipboardCommands(Receiver<ClipboardCommand>);

/// Command capability shared with the device input dispatcher for paste-text.
#[derive(Clone)]
pub struct ClipboardBridge(Sender<ClipboardCommand>);

impl ClipboardBridge {
    fn channel() -> (Self, ClipboardCommands) {
        let (sender, receiver) = tokio::sync::mpsc::channel(CLIPBOARD_COMMAND_CAPACITY);
        (Self(sender), ClipboardCommands(receiver))
    }

    async fn set_text(&self, text: String) -> Result<(), String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.0
            .try_send(ClipboardCommand::SetText { text, reply })
            .map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => {
                    "device clipboard is busy".to_string()
                }
                tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                    "device clipboard is unavailable".to_string()
                }
            })?;
        tokio::time::timeout(CLIPBOARD_COMMAND_TIMEOUT, response)
            .await
            .map_err(|_| "device clipboard request timed out".to_string())?
            .map_err(|_| "device clipboard session ended".to_string())?
    }
}

impl DeviceClipboard for ClipboardBridge {
    fn set_text(&self, text: String) -> ClipboardWriteFuture<'_> {
        Box::pin(ClipboardBridge::set_text(self, text))
    }
}

/// Owns the one Pasteboard client allowed for a connected device session.
pub struct DeviceClipboardSession {
    pasteboard: Option<PasteboardServiceClient<Box<dyn ReadWrite>>>,
    host: Option<Box<dyn HostClipboard>>,
    sync_enabled: bool,
    commands: ClipboardCommands,
}

/// Establish the optional Pasteboard service and its command bridge. Explicit
/// paste remains available through lazy reconnect even when automatic sync is
/// disabled or the first Pasteboard connection fails.
pub async fn connect_device_clipboard(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    sync_enabled: bool,
    host_factory: Option<HostClipboardFactory>,
) -> (ClipboardBridge, DeviceClipboardSession) {
    let (bridge, commands) = ClipboardBridge::channel();
    let pasteboard = if sync_enabled {
        match PasteboardServiceClient::connect_rsd(adapter, handshake).await {
            Ok(client) => {
                tracing::info!("clipboard sync enabled for this device session");
                Some(client)
            }
            Err(error) => {
                tracing::warn!(?error, "no pasteboardservice; clipboard sync unavailable");
                None
            }
        }
    } else {
        tracing::info!("clipboard sync disabled for this device session");
        None
    };
    let host = if sync_enabled && pasteboard.is_some() {
        match host_factory {
            Some(factory) => match factory() {
                Ok(host) => Some(host),
                Err(error) => {
                    tracing::warn!(%error, "no host clipboard; clipboard sync disabled");
                    None
                }
            },
            None => {
                tracing::info!("host clipboard not configured; clipboard sync disabled");
                None
            }
        }
    } else {
        None
    };
    (
        bridge,
        DeviceClipboardSession {
            pasteboard,
            host,
            sync_enabled,
            commands,
        },
    )
}

struct ClipState {
    last_text: Option<String>,
    last_image: Option<u64>,
    last_change_count: Option<i64>,
}

impl DeviceClipboardSession {
    pub async fn run(
        self,
        activity: ClipboardSlot,
        adapter: &mut AdapterHandle,
        handshake: &mut RsdHandshake,
    ) {
        let Self {
            pasteboard,
            host,
            sync_enabled,
            commands: ClipboardCommands(mut commands),
        } = self;
        let Some(mut pasteboard) = pasteboard else {
            command_loop(None, &activity, &mut commands, adapter, handshake).await;
            return;
        };
        if !sync_enabled {
            command_loop(
                Some(pasteboard),
                &activity,
                &mut commands,
                adapter,
                handshake,
            )
            .await;
            return;
        }
        let Some(mut host) = host else {
            command_loop(
                Some(pasteboard),
                &activity,
                &mut commands,
                adapter,
                handshake,
            )
            .await;
            return;
        };

        // Seed state without copying, so connecting never overwrites either side.
        let mut state = ClipState {
            last_text: host.get_text().ok(),
            last_image: host.get_image().ok().map(|image| image_hash(&image.bytes)),
            last_change_count: pasteboard
                .get(GENERAL_PASTEBOARD)
                .await
                .ok()
                .and_then(|snapshot| snapshot.change_count),
        };

        subscribe(&mut pasteboard).await;
        let mut tick = tokio::time::interval(CLIPBOARD_POLL);
        let mut commands_open = true;
        loop {
            let wake = tokio::select! {
                result = pasteboard.recv_push() => ClipboardWake::Push(result),
                _ = tick.tick() => ClipboardWake::Tick,
                command = commands.recv(), if commands_open => ClipboardWake::Command(command),
            };

            match wake {
                ClipboardWake::Push(Ok(snapshot)) => {
                    apply_device_snapshot(&snapshot, host.as_mut(), &activity, &mut state)
                }
                ClipboardWake::Push(Err(error)) => {
                    tracing::warn!(?error, "clipboard PUSH failed");
                    if let Some(client) = reconnect(adapter, handshake).await {
                        pasteboard = client;
                        subscribe(&mut pasteboard).await;
                        state.last_change_count = pasteboard
                            .get(GENERAL_PASTEBOARD)
                            .await
                            .ok()
                            .and_then(|snapshot| snapshot.change_count);
                    }
                }
                ClipboardWake::Tick => {
                    match pasteboard.get(GENERAL_PASTEBOARD).await {
                        Ok(snapshot) => {
                            apply_device_snapshot(&snapshot, host.as_mut(), &activity, &mut state)
                        }
                        Err(error) => {
                            tracing::warn!(?error, "clipboard PULL failed");
                            if let Some(client) = reconnect(adapter, handshake).await {
                                pasteboard = client;
                                subscribe(&mut pasteboard).await;
                            }
                            continue;
                        }
                    }
                    if let Err(error) =
                        push_host_clipboard(&mut pasteboard, host.as_mut(), &activity, &mut state)
                            .await
                    {
                        tracing::warn!(?error, "clipboard host -> device failed");
                        if let Some(client) = reconnect(adapter, handshake).await {
                            pasteboard = client;
                            subscribe(&mut pasteboard).await;
                        }
                    }
                }
                ClipboardWake::Command(Some(command)) => {
                    let prepared_text = match &command {
                        ClipboardCommand::SetText { text, .. } => text.clone(),
                    };
                    if execute_command(&mut pasteboard, &activity, command).await {
                        state.last_text = Some(prepared_text);
                        state.last_image = None;
                        state.last_change_count = pasteboard
                            .get(GENERAL_PASTEBOARD)
                            .await
                            .ok()
                            .and_then(|snapshot| snapshot.change_count);
                    } else if let Some(client) = reconnect(adapter, handshake).await {
                        pasteboard = client;
                        subscribe(&mut pasteboard).await;
                    }
                }
                ClipboardWake::Command(None) => commands_open = false,
            }
        }
    }
}

async fn command_loop(
    mut pasteboard: Option<PasteboardServiceClient<Box<dyn ReadWrite>>>,
    activity: &ClipboardSlot,
    commands: &mut Receiver<ClipboardCommand>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) {
    loop {
        let Some(command) = commands.recv().await else {
            std::future::pending::<()>().await;
            return;
        };
        if pasteboard.is_none() {
            pasteboard = reconnect(adapter, handshake).await;
        }
        let Some(client) = pasteboard.as_mut() else {
            reject_command(command, "device pasteboard service is unavailable");
            continue;
        };
        if !execute_command(client, activity, command).await {
            pasteboard = None;
        }
    }
}

async fn execute_command(
    pasteboard: &mut PasteboardServiceClient<Box<dyn ReadWrite>>,
    activity: &ClipboardSlot,
    command: ClipboardCommand,
) -> bool {
    match command {
        ClipboardCommand::SetText { text, reply } => {
            let result = pasteboard
                .set_text(&text, GENERAL_PASTEBOARD)
                .await
                .map_err(|error| format!("unable to set device clipboard: {error:?}"));
            let succeeded = result.is_ok();
            if succeeded {
                tracing::info!(
                    bytes = text.len(),
                    "clipboard: text prepared for device paste"
                );
                activity.set(ClipboardEvent {
                    from_device: false,
                    kind: ClipboardContentKind::Text,
                    preview: clipboard_preview(&text, CLIPBOARD_PREVIEW_LEN),
                });
            }
            let _ = reply.send(result);
            succeeded
        }
    }
}

fn reject_command(command: ClipboardCommand, reason: &str) {
    match command {
        ClipboardCommand::SetText { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

async fn subscribe(pasteboard: &mut PasteboardServiceClient<Box<dyn ReadWrite>>) {
    if let Err(error) = pasteboard
        .set_change_notifications(
            true,
            GENERAL_PASTEBOARD,
            Some(DataInclusionPolicy::AllResolved),
        )
        .await
    {
        tracing::warn!(
            ?error,
            "clipboard: failed to subscribe to change notifications"
        );
    }
}

fn apply_device_snapshot(
    snapshot: &PasteboardSnapshot,
    host: &mut dyn HostClipboard,
    activity: &ClipboardSlot,
    state: &mut ClipState,
) {
    if snapshot.change_count == state.last_change_count {
        return;
    }
    state.last_change_count = snapshot.change_count;

    if let Some(text) = snapshot.text() {
        if Some(&text) != state.last_text.as_ref() {
            match host.set_text(text.clone()) {
                Ok(()) => {
                    tracing::info!(bytes = text.len(), "clipboard: device -> host text");
                    activity.set(ClipboardEvent {
                        from_device: true,
                        kind: ClipboardContentKind::Text,
                        preview: clipboard_preview(&text, CLIPBOARD_PREVIEW_LEN),
                    });
                    state.last_text = Some(text);
                    state.last_image = None;
                }
                Err(error) => tracing::warn!(%error, "failed to set host text"),
            }
        }
    } else if let Some((_uti, bytes)) = snapshot.image() {
        match decode_image(&bytes) {
            Some(image) => {
                let (width, height) = (image.width, image.height);
                let hash = image_hash(&image.bytes);
                if Some(hash) != state.last_image {
                    match host.set_image(image) {
                        Ok(()) => {
                            tracing::info!(width, height, "clipboard: device -> host image");
                            activity.set(ClipboardEvent {
                                from_device: true,
                                kind: ClipboardContentKind::Image,
                                preview: format!("{width} x {height}"),
                            });
                            state.last_image = Some(hash);
                            state.last_text = None;
                        }
                        Err(error) => tracing::warn!(%error, "failed to set host image"),
                    }
                }
            }
            None => tracing::warn!("clipboard: undecodable device image, skipping"),
        }
    }
}

async fn push_host_clipboard(
    pasteboard: &mut PasteboardServiceClient<Box<dyn ReadWrite>>,
    host: &mut dyn HostClipboard,
    activity: &ClipboardSlot,
    state: &mut ClipState,
) -> Result<(), IdeviceError> {
    if let Ok(text) = host.get_text()
        && !text.is_empty()
    {
        if Some(&text) != state.last_text.as_ref() {
            pasteboard.set_text(&text, GENERAL_PASTEBOARD).await?;
            tracing::info!(bytes = text.len(), "clipboard: host -> device text");
            activity.set(ClipboardEvent {
                from_device: false,
                kind: ClipboardContentKind::Text,
                preview: clipboard_preview(&text, CLIPBOARD_PREVIEW_LEN),
            });
            state.last_text = Some(text);
            state.last_image = None;
            state.last_change_count = pasteboard
                .get(GENERAL_PASTEBOARD)
                .await
                .ok()
                .and_then(|snapshot| snapshot.change_count);
        }
        return Ok(());
    }

    if let Ok(image) = host.get_image() {
        let hash = image_hash(&image.bytes);
        if Some(hash) != state.last_image {
            let (width, height) = (image.width, image.height);
            match encode_png(&image) {
                Some(png) => {
                    pasteboard
                        .set_image(&png, UTI_PNG, GENERAL_PASTEBOARD)
                        .await?;
                    tracing::info!(
                        width,
                        height,
                        bytes = png.len(),
                        "clipboard: host -> device image"
                    );
                    activity.set(ClipboardEvent {
                        from_device: false,
                        kind: ClipboardContentKind::Image,
                        preview: format!("{width} x {height}"),
                    });
                    state.last_image = Some(hash);
                    state.last_text = None;
                    state.last_change_count = pasteboard
                        .get(GENERAL_PASTEBOARD)
                        .await
                        .ok()
                        .and_then(|snapshot| snapshot.change_count);
                }
                None => tracing::warn!("clipboard: failed to encode host image to PNG"),
            }
        }
    }
    Ok(())
}

fn image_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn decode_image(bytes: &[u8]) -> Option<ClipboardImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some(ClipboardImage {
        width: image.width() as usize,
        height: image.height() as usize,
        bytes: image.into_raw(),
    })
}

fn encode_png(image: &ClipboardImage) -> Option<Vec<u8>> {
    let buffer =
        image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.bytes.clone())?;
    let mut output = std::io::Cursor::new(Vec::new());
    buffer.write_to(&mut output, image::ImageFormat::Png).ok()?;
    Some(output.into_inner())
}

async fn reconnect(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) -> Option<PasteboardServiceClient<Box<dyn ReadWrite>>> {
    match tokio::time::timeout(
        Duration::from_secs(5),
        PasteboardServiceClient::connect_rsd(adapter, handshake),
    )
    .await
    {
        Ok(Ok(client)) => {
            tracing::info!("clipboard: reconnected pasteboard service");
            Some(client)
        }
        Ok(Err(error)) => {
            tracing::warn!(?error, "clipboard reconnect failed");
            None
        }
        Err(_) => {
            tracing::warn!("clipboard reconnect timed out");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClipboardImage, decode_image, encode_png, image_hash};

    #[test]
    fn rgba_png_round_trip_preserves_pixels() {
        let pixels = vec![255, 0, 0, 255, 0, 128, 255, 64];
        let image = ClipboardImage {
            width: 2,
            height: 1,
            bytes: pixels.clone(),
        };

        let png = encode_png(&image).expect("valid RGBA should encode");
        let decoded = decode_image(&png).expect("encoded PNG should decode");

        assert_eq!((decoded.width, decoded.height), (2, 1));
        assert_eq!(decoded.bytes, pixels);
    }

    #[test]
    fn malformed_rgba_buffer_is_rejected() {
        let image = ClipboardImage {
            width: 2,
            height: 2,
            bytes: vec![0; 15],
        };
        assert!(encode_png(&image).is_none());
    }

    #[test]
    fn image_hash_is_stable_and_content_sensitive() {
        assert_eq!(image_hash(&[1, 2, 3]), image_hash(&[1, 2, 3]));
        assert_ne!(image_hash(&[1, 2, 3]), image_hash(&[1, 2, 4]));
    }
}
