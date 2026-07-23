//! Bounded, on-demand screenshots from CoreDevice ScreenCaptureService.

use std::time::Duration;

use idevice::RsdService;
use idevice::core_device::{ImageFormat, ScreenCaptureServiceClient};
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use tokio::sync::{mpsc, oneshot, watch};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_SCREENSHOT_BYTES: usize = 32 * 1024 * 1024;
const MAX_SCREENSHOT_DIMENSION: u32 = 16_384;

#[derive(Debug)]
pub struct ScreenCaptureCommand {
    pub reply: oneshot::Sender<Result<Vec<u8>, String>>,
}

pub async fn serve(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
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
                let result = capture(&mut client, &mut adapter, &mut handshake).await;
                let _ = command.reply.send(result);
            }
        }
    }
}

async fn capture(
    client: &mut Option<ScreenCaptureServiceClient<Box<dyn idevice::ReadWrite>>>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) -> Result<Vec<u8>, String> {
    if client.is_none() {
        *client = Some(
            tokio::time::timeout(
                CONNECT_TIMEOUT,
                ScreenCaptureServiceClient::connect_rsd(adapter, handshake),
            )
            .await
            .map_err(|_| "screen capture service connection timed out".to_string())?
            .map_err(|error| format!("screen capture service unavailable: {error:?}"))?,
        );
    }
    let result = tokio::time::timeout(
        REQUEST_TIMEOUT,
        client
            .as_mut()
            .expect("screen capture client initialized")
            .take_screenshot(None, ImageFormat::Png),
    )
    .await;
    match result {
        Ok(Ok(png)) => {
            let (width, height) = match validate_png(&png) {
                Ok(dimensions) => dimensions,
                Err(error) => {
                    client.take();
                    return Err(error);
                }
            };
            tracing::info!(
                bytes = png.len(),
                width,
                height,
                "native device screenshot captured"
            );
            Ok(png)
        }
        Ok(Err(error)) => {
            client.take();
            Err(format!("unable to capture device screenshot: {error:?}"))
        }
        Err(_) => {
            client.take();
            Err("device screenshot request timed out".into())
        }
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
        let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-screenshot-test");
        let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let mut handshake = RsdHandshake::new(stream).await.unwrap();
        let mut client = None;
        let png = capture(&mut client, &mut adapter, &mut handshake)
            .await
            .unwrap();
        let dimensions = validate_png(&png).unwrap();
        eprintln!("native screenshot: {dimensions:?}, {} bytes", png.len());
    }
}
