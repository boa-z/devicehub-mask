//! Bounded, on-demand native screenshots with CoreDevice, DVT, and screenshotr backends.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use idevice::core_device::{ImageFormat, ScreenCaptureServiceClient};
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::dvt::screenshot::ScreenshotClient;
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::screenshotr::ScreenshotService;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceService, ReadWrite, RsdService};
use tokio::sync::{mpsc, oneshot, watch};

use crate::protocol::ConnKind;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_SCREENSHOT_BYTES: usize = 32 * 1024 * 1024;
const MAX_SCREENSHOT_DIMENSION: u32 = 16_384;
const MAX_DECODE_ALLOC_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug)]
pub struct ScreenCaptureCommand {
    pub reply: oneshot::Sender<Result<Vec<u8>, String>>,
}

pub struct ScreenCaptureTransport {
    provider: Arc<dyn IdeviceProvider>,
    connection: ConnKind,
    adapter: AdapterHandle,
    handshake: RsdHandshake,
}

impl ScreenCaptureTransport {
    pub fn new(
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

    async fn connect_core(&mut self) -> Result<NativeScreenCaptureClient, String> {
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            ScreenCaptureServiceClient::connect_rsd(&mut self.adapter, &mut self.handshake),
        )
        .await
        .map_err(|_| "CoreDevice screen capture connection timed out".to_string())?
        .map(NativeScreenCaptureClient::CoreDevice)
        .map_err(|error| format!("CoreDevice screen capture unavailable: {error:?}"))
    }

    async fn connect_screenshotr(&mut self) -> Result<NativeScreenCaptureClient, String> {
        let mut failures = Vec::new();
        if self.connection == ConnKind::Usb {
            match tokio::time::timeout(
                CONNECT_TIMEOUT,
                ScreenshotService::connect(self.provider.as_ref()),
            )
            .await
            {
                Ok(Ok(client)) => return Ok(NativeScreenCaptureClient::Screenshotr(client)),
                Ok(Err(error)) => failures.push(format!("USB lockdown screenshotr: {error:?}")),
                Err(_) => failures.push("USB lockdown screenshotr: connection timed out".into()),
            }
        }
        match tokio::time::timeout(
            CONNECT_TIMEOUT,
            ScreenshotService::connect_rsd(&mut self.adapter, &mut self.handshake),
        )
        .await
        {
            Ok(Ok(client)) => Ok(NativeScreenCaptureClient::Screenshotr(client)),
            Ok(Err(error)) => {
                failures.push(format!("CoreDevice RSD screenshotr: {error:?}"));
                Err(format!(
                    "screenshotr service unavailable: {}",
                    failures.join("; ")
                ))
            }
            Err(_) => {
                failures.push("CoreDevice RSD screenshotr: connection timed out".into());
                Err(format!(
                    "screenshotr service unavailable: {}",
                    failures.join("; ")
                ))
            }
        }
    }

    async fn capture_dvt(&mut self) -> Result<Vec<u8>, String> {
        let mut remote = tokio::time::timeout(
            CONNECT_TIMEOUT,
            RemoteServerClient::<Box<dyn ReadWrite>>::connect_rsd(
                &mut self.adapter,
                &mut self.handshake,
            ),
        )
        .await
        .map_err(|_| "DVT screenshot connection timed out".to_string())?
        .map_err(|error| format!("DVT screenshot service unavailable: {error:?}"))?;
        let screenshot = tokio::time::timeout(REQUEST_TIMEOUT, async {
            let mut client = ScreenshotClient::new(&mut remote)
                .await
                .map_err(|error| format!("DVT screenshot channel unavailable: {error:?}"))?;
            client
                .take_screenshot()
                .await
                .map_err(|error| format!("DVT screenshot request failed: {error:?}"))
        })
        .await
        .map_err(|_| "dvt screenshot request timed out".to_string())??;
        normalize_and_log("dvt", screenshot)
    }
}

enum NativeScreenCaptureClient {
    CoreDevice(ScreenCaptureServiceClient<Box<dyn idevice::ReadWrite>>),
    Screenshotr(ScreenshotService),
}

impl NativeScreenCaptureClient {
    fn backend(&self) -> &'static str {
        match self {
            Self::CoreDevice(_) => "coredevice",
            Self::Screenshotr(_) => "screenshotr",
        }
    }

    async fn take_screenshot(&mut self) -> Result<Vec<u8>, String> {
        match self {
            Self::CoreDevice(client) => client
                .take_screenshot(None, ImageFormat::Png)
                .await
                .map_err(|error| format!("CoreDevice screenshot failed: {error:?}")),
            Self::Screenshotr(client) => client
                .take_screenshot()
                .await
                .map_err(|error| format!("screenshotr request failed: {error:?}")),
        }
    }
}

pub async fn serve(
    mut transport: ScreenCaptureTransport,
    mut commands: mpsc::Receiver<ScreenCaptureCommand>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut client = None;
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                let result = capture(&mut client, &mut transport).await;
                let _ = command.reply.send(result);
            }
        }
    }
}

async fn capture(
    client: &mut Option<NativeScreenCaptureClient>,
    transport: &mut ScreenCaptureTransport,
) -> Result<Vec<u8>, String> {
    let mut failures = Vec::new();
    if let Some(active) = client.as_mut() {
        match capture_from(active).await {
            Ok(png) => return Ok(png),
            Err(error) => {
                tracing::warn!(backend = active.backend(), %error, "native screenshot backend failed");
                failures.push(error);
                client.take();
            }
        }
    }

    match transport.connect_core().await {
        Ok(mut connected) => match capture_from(&mut connected).await {
            Ok(png) => {
                *client = Some(connected);
                return Ok(png);
            }
            Err(error) => {
                tracing::warn!(backend = connected.backend(), %error, "native screenshot backend failed; trying fallback");
                failures.push(error);
            }
        },
        Err(error) => {
            tracing::debug!(%error, "CoreDevice screenshot backend unavailable; trying fallback");
            failures.push(error);
        }
    }

    match transport.connect_screenshotr().await {
        Ok(mut connected) => match capture_from(&mut connected).await {
            Ok(png) => {
                *client = Some(connected);
                return Ok(png);
            }
            Err(error) => {
                failures.push(error);
            }
        },
        Err(error) => {
            failures.push(error);
        }
    }

    match transport.capture_dvt().await {
        Ok(png) => Ok(png),
        Err(error) => {
            tracing::debug!(%error, "DVT screenshot fallback unavailable");
            failures.push(error);
            Err(unavailable(failures))
        }
    }
}

async fn capture_from(client: &mut NativeScreenCaptureClient) -> Result<Vec<u8>, String> {
    let backend = client.backend();
    let screenshot = tokio::time::timeout(REQUEST_TIMEOUT, client.take_screenshot())
        .await
        .map_err(|_| format!("{backend} screenshot request timed out"))??;
    normalize_and_log(backend, screenshot)
}

fn normalize_and_log(backend: &'static str, screenshot: Vec<u8>) -> Result<Vec<u8>, String> {
    let png = normalize_screenshot(screenshot)?;
    let (width, height) = validate_png(&png)?;
    tracing::info!(
        backend,
        bytes = png.len(),
        width,
        height,
        "native device screenshot captured"
    );
    Ok(png)
}

fn normalize_screenshot(screenshot: Vec<u8>) -> Result<Vec<u8>, String> {
    if validate_png(&screenshot).is_ok() {
        return Ok(screenshot);
    }
    if screenshot.len() > MAX_SCREENSHOT_BYTES {
        return Err("device screenshot exceeds the 32 MiB limit".into());
    }
    let format = image::guess_format(&screenshot)
        .map_err(|_| "device returned an unsupported screenshot format".to_string())?;
    if format != image::ImageFormat::Tiff {
        return Err("device returned an unsupported screenshot format".into());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SCREENSHOT_DIMENSION);
    limits.max_image_height = Some(MAX_SCREENSHOT_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    let mut reader = image::ImageReader::with_format(Cursor::new(screenshot), format);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("unable to decode device TIFF screenshot: {error}"))?;
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|error| format!("unable to encode device PNG screenshot: {error}"))?;
    let png = output.into_inner();
    validate_png(&png)?;
    Ok(png)
}

fn unavailable(failures: Vec<String>) -> String {
    if failures.is_empty() {
        "native screenshot service unavailable".into()
    } else {
        format!(
            "native screenshot service unavailable: {}",
            failures.join("; ")
        )
    }
}

fn validate_png(png: &[u8]) -> Result<(u32, u32), String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if png.len() > MAX_SCREENSHOT_BYTES {
        return Err("device screenshot exceeds the 32 MiB limit".into());
    }
    if png.len() < 24
        || &png[..8] != PNG_SIGNATURE
        || &png[12..16] != b"IHDR"
        || u32::from_be_bytes(png[8..12].try_into().unwrap()) != 13
    {
        return Err("device returned an invalid PNG screenshot".into());
    }
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    if width == 0
        || height == 0
        || width > MAX_SCREENSHOT_DIMENSION
        || height > MAX_SCREENSHOT_DIMENSION
    {
        return Err("device returned unsupported screenshot dimensions".into());
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::IdeviceService;
    use idevice::core_device_proxy::CoreDeviceProxy;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png
    }

    #[test]
    fn validates_screenshot_png_header_and_dimensions() {
        assert_eq!(validate_png(&png_header(2160, 1620)), Ok((2160, 1620)));
        assert!(validate_png(&[0; 24]).is_err());
        assert!(validate_png(&png_header(0, 100)).is_err());
        assert!(validate_png(&png_header(20_000, 100)).is_err());
    }

    #[test]
    fn normalizes_legacy_tiff_to_bounded_png() {
        let source = image::DynamicImage::new_rgb8(3, 2);
        let mut tiff = Cursor::new(Vec::new());
        source
            .write_to(&mut tiff, image::ImageFormat::Tiff)
            .unwrap();
        let png = normalize_screenshot(tiff.into_inner()).unwrap();
        assert_eq!(validate_png(&png), Ok((3, 2)));
        assert!(normalize_screenshot(b"not an image".to_vec()).is_err());
    }

    #[test]
    fn preserves_backend_failures_in_unavailable_error() {
        let error = unavailable(vec![
            "core failed".into(),
            "fallback failed".into(),
            "dvt failed".into(),
        ]);
        assert_eq!(
            error,
            "native screenshot service unavailable: core failed; fallback failed; dvt failed"
        );
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn captures_native_screenshot_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider =
            Arc::new(device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-screenshot-test"));
        let proxy = CoreDeviceProxy::connect(provider.as_ref()).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let handshake = RsdHandshake::new(stream).await.unwrap();
        let mut transport =
            ScreenCaptureTransport::new(provider, ConnKind::Usb, adapter, handshake);
        let mut client = None;
        let png = capture(&mut client, &mut transport).await.unwrap();
        let dimensions = validate_png(&png).unwrap();
        eprintln!("native screenshot: {dimensions:?}, {} bytes", png.len());
    }
}
