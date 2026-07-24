//! On-demand, bounded application icons from CoreDevice with a SpringBoard fallback.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use idevice::RsdService;
use idevice::core_device::{AppServiceClient, IconData};
use idevice::rsd::RsdHandshake;
use idevice::springboardservices::SpringBoardServicesClient;
use idevice::tcp::handle::AdapterHandle;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use tokio::sync::{mpsc, oneshot, watch};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const APP_SERVICE_ICON_SIZE: f32 = 64.0;
const APP_SERVICE_ICON_HEADER_BYTES: usize = 0x30;
const MAX_ICON_BYTES: usize = 4 * 1024 * 1024;
const MAX_ICON_DIMENSION: u32 = 2_048;
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 256;

#[derive(Debug)]
pub struct AppIconCommand {
    pub bundle_id: String,
    pub reply: oneshot::Sender<Result<Vec<u8>, String>>,
}

#[derive(Default)]
struct IconCache {
    entries: HashMap<String, Vec<u8>>,
    order: VecDeque<String>,
    bytes: usize,
}

#[derive(Default)]
struct CoreDeviceIconSource {
    client: Option<AppServiceClient<Box<dyn idevice::ReadWrite>>>,
    consecutive_failures: u8,
    disabled: bool,
}

impl CoreDeviceIconSource {
    fn succeeded(&mut self) {
        self.consecutive_failures = 0;
    }

    fn failed(&mut self, permanent: bool) {
        self.client.take();
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.disabled = permanent || self.consecutive_failures >= 2;
    }
}

impl IconCache {
    fn get(&self, bundle_id: &str) -> Option<Vec<u8>> {
        self.entries.get(bundle_id).cloned()
    }

    fn insert(&mut self, bundle_id: String, icon: Vec<u8>) {
        while self.entries.len() >= MAX_CACHE_ENTRIES
            || self.bytes.saturating_add(icon.len()) > MAX_CACHE_BYTES
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(removed.len());
            }
        }
        self.bytes += icon.len();
        self.order.push_back(bundle_id.clone());
        self.entries.insert(bundle_id, icon);
    }
}

pub async fn serve(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    mut commands: mpsc::Receiver<AppIconCommand>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut app_service = CoreDeviceIconSource::default();
    let mut springboard = None;
    let mut cache = IconCache::default();
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                if let Some(icon) = cache.get(&command.bundle_id) {
                    let _ = command.reply.send(Ok(icon));
                    continue;
                }
                let result = fetch_icon(
                    &mut app_service,
                    &mut springboard,
                    &mut adapter,
                    &mut handshake,
                    &command.bundle_id,
                ).await;
                if let Ok(icon) = &result {
                    tracing::debug!(
                        bundle_id = %command.bundle_id,
                        bytes = icon.len(),
                        "app icon loaded"
                    );
                    cache.insert(command.bundle_id.clone(), icon.clone());
                } else if let Err(error) = &result {
                    tracing::debug!(
                        bundle_id = %command.bundle_id,
                        %error,
                        "app icon unavailable"
                    );
                }
                let _ = command.reply.send(result);
            }
        }
    }
}

async fn fetch_icon(
    app_service: &mut CoreDeviceIconSource,
    springboard: &mut Option<SpringBoardServicesClient>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
) -> Result<Vec<u8>, String> {
    let app_service_error =
        match fetch_core_device_icon(app_service, adapter, handshake, bundle_id).await {
            Ok(icon) => return Ok(icon),
            Err(error) => error,
        };
    tracing::debug!(%app_service_error, "CoreDevice app icon unavailable; using SpringBoard fallback");
    fetch_springboard_icon(springboard, adapter, handshake, bundle_id)
        .await
        .map_err(|springboard_error| {
            format!(
                "CoreDevice app icon unavailable ({app_service_error}); SpringBoard fallback unavailable ({springboard_error})"
            )
        })
}

async fn fetch_core_device_icon(
    source: &mut CoreDeviceIconSource,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
) -> Result<Vec<u8>, String> {
    if source.disabled {
        return Err("CoreDevice app icon source disabled for this device session".into());
    }
    if source.client.is_none() {
        let connection = tokio::time::timeout(
            CONNECT_TIMEOUT,
            AppServiceClient::connect_rsd(adapter, handshake),
        )
        .await;
        source.client = match connection {
            Ok(Ok(client)) => Some(client),
            Ok(Err(error)) => {
                source.failed(true);
                return Err(format!(
                    "CoreDevice app icon service unavailable: {error:?}"
                ));
            }
            Err(_) => {
                source.failed(true);
                return Err("CoreDevice app icon service connection timed out".into());
            }
        };
    }
    let result = tokio::time::timeout(
        REQUEST_TIMEOUT,
        source
            .client
            .as_mut()
            .expect("CoreDevice app icon client initialized")
            .fetch_app_icon(
                bundle_id,
                APP_SERVICE_ICON_SIZE,
                APP_SERVICE_ICON_SIZE,
                1.0,
                false,
            ),
    )
    .await;
    match result {
        Ok(Ok(icon)) => match app_service_icon_to_png(icon) {
            Ok(png) => {
                source.succeeded();
                Ok(png)
            }
            Err(error) => {
                source.failed(false);
                Err(error)
            }
        },
        Ok(Err(error)) => {
            source.failed(false);
            Err(format!("unable to read CoreDevice app icon: {error:?}"))
        }
        Err(_) => {
            source.failed(true);
            Err("CoreDevice app icon request timed out".into())
        }
    }
}

async fn fetch_springboard_icon(
    client: &mut Option<SpringBoardServicesClient>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
) -> Result<Vec<u8>, String> {
    if client.is_none() {
        *client = Some(
            tokio::time::timeout(
                CONNECT_TIMEOUT,
                SpringBoardServicesClient::connect_rsd(adapter, handshake),
            )
            .await
            .map_err(|_| "SpringBoard icon service connection timed out".to_string())?
            .map_err(|error| format!("SpringBoard icon service unavailable: {error:?}"))?,
        );
    }
    let result = tokio::time::timeout(
        REQUEST_TIMEOUT,
        client
            .as_mut()
            .expect("SpringBoard client initialized")
            .get_icon_pngdata(bundle_id.to_owned()),
    )
    .await;
    match result {
        Ok(Ok(icon)) => validate_png_icon(icon),
        Ok(Err(error)) => {
            client.take();
            Err(format!("unable to read app icon: {error:?}"))
        }
        Err(_) => {
            client.take();
            Err("app icon request timed out".into())
        }
    }
}

fn app_service_icon_to_png(icon: IconData) -> Result<Vec<u8>, String> {
    encode_app_service_rgba(icon.data.as_ref(), icon.icon_width, icon.icon_height)
}

fn encode_app_service_rgba(data: &[u8], width: f64, height: f64) -> Result<Vec<u8>, String> {
    if data.len() > MAX_ICON_BYTES {
        return Err("CoreDevice app icon exceeds the 4 MiB limit".into());
    }
    let width = validated_icon_dimension(width)?;
    let height = validated_icon_dimension(height)?;
    let pixel_bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "CoreDevice app icon dimensions overflow".to_string())?;
    let end = APP_SERVICE_ICON_HEADER_BYTES
        .checked_add(pixel_bytes)
        .ok_or_else(|| "CoreDevice app icon size overflow".to_string())?;
    let rgba = data
        .get(APP_SERVICE_ICON_HEADER_BYTES..end)
        .ok_or_else(|| "CoreDevice app icon RGBA payload is truncated".to_string())?;
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|error| format!("unable to encode CoreDevice app icon: {error}"))?;
    validate_png_icon(png)
}

fn validated_icon_dimension(value: f64) -> Result<u32, String> {
    if !value.is_finite() || value < 1.0 || value > MAX_ICON_DIMENSION as f64 {
        return Err("device returned unsupported CoreDevice app icon dimensions".into());
    }
    let rounded = value.round();
    if (value - rounded).abs() > f64::EPSILON {
        return Err("device returned fractional CoreDevice app icon dimensions".into());
    }
    Ok(rounded as u32)
}

fn validate_png_icon(icon: Vec<u8>) -> Result<Vec<u8>, String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if icon.len() > MAX_ICON_BYTES {
        return Err("app icon exceeds the 4 MiB limit".into());
    }
    if icon.len() < 24
        || &icon[..8] != PNG_SIGNATURE
        || &icon[12..16] != b"IHDR"
        || u32::from_be_bytes(icon[8..12].try_into().unwrap()) != 13
    {
        return Err("device returned an invalid PNG app icon".into());
    }
    let width = u32::from_be_bytes(icon[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(icon[20..24].try_into().unwrap());
    if width == 0 || height == 0 || width > MAX_ICON_DIMENSION || height > MAX_ICON_DIMENSION {
        return Err("device returned unsupported app icon dimensions".into());
    }
    Ok(icon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::IdeviceService;
    use idevice::core_device_proxy::CoreDeviceProxy;
    use idevice::installation_proxy::InstallationProxyClient;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut icon = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        icon.extend_from_slice(&width.to_be_bytes());
        icon.extend_from_slice(&height.to_be_bytes());
        icon
    }

    #[test]
    fn validates_png_icon_header_and_dimensions() {
        assert!(validate_png_icon(png_header(120, 120)).is_ok());
        assert!(validate_png_icon(vec![0; 24]).is_err());
        assert!(validate_png_icon(png_header(0, 120)).is_err());
        assert!(validate_png_icon(png_header(4_096, 4_096)).is_err());
    }

    #[test]
    fn icon_cache_evicts_oldest_entries_at_the_entry_limit() {
        let mut cache = IconCache::default();
        for index in 0..=MAX_CACHE_ENTRIES {
            cache.insert(index.to_string(), vec![index as u8]);
        }
        assert!(cache.get("0").is_none());
        assert_eq!(cache.get(&MAX_CACHE_ENTRIES.to_string()), Some(vec![0]));
        assert_eq!(cache.entries.len(), MAX_CACHE_ENTRIES);
    }

    #[test]
    fn core_device_icon_source_stops_retrying_repeated_failures() {
        let mut source = CoreDeviceIconSource::default();
        source.failed(false);
        assert!(!source.disabled);
        source.failed(false);
        assert!(source.disabled);

        let mut permanent = CoreDeviceIconSource::default();
        permanent.failed(true);
        assert!(permanent.disabled);
    }

    #[test]
    fn converts_bounded_core_device_rgba_icon_to_png() {
        let mut data = vec![0; APP_SERVICE_ICON_HEADER_BYTES];
        data.extend_from_slice(&[255, 0, 0, 255, 0, 255, 0, 255]);
        let png = encode_app_service_rgba(&data, 2.0, 1.0).unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.as_raw(), &data[APP_SERVICE_ICON_HEADER_BYTES..]);
    }

    #[test]
    fn rejects_malformed_core_device_icon_payloads() {
        assert!(encode_app_service_rgba(&[0; APP_SERVICE_ICON_HEADER_BYTES], 1.0, 1.0).is_err());
        assert!(
            encode_app_service_rgba(&[0; APP_SERVICE_ICON_HEADER_BYTES + 4], 1.5, 1.0).is_err()
        );
        assert!(
            encode_app_service_rgba(&[0; APP_SERVICE_ICON_HEADER_BYTES + 4], 0.0, 1.0).is_err()
        );
        assert!(encode_app_service_rgba(&vec![0; MAX_ICON_BYTES + 1], 1.0, 1.0).is_err());
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device with an installed user app"]
    async fn reads_app_icon_sources_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-icon-test");
        let bundle_id = InstallationProxyClient::connect(&provider)
            .await
            .unwrap()
            .get_apps(Some("User"), None)
            .await
            .unwrap()
            .into_keys()
            .next()
            .expect("device has no installed user apps");
        let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let mut handshake = RsdHandshake::new(stream).await.unwrap();
        let mut app_service = AppServiceClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .unwrap();
        let core_device_icon = app_service
            .fetch_app_icon(
                bundle_id.clone(),
                APP_SERVICE_ICON_SIZE,
                APP_SERVICE_ICON_SIZE,
                1.0,
                false,
            )
            .await
            .unwrap();
        let core_device_png = app_service_icon_to_png(core_device_icon).unwrap();
        println!(
            "read {bundle_id} CoreDevice icon: {} bytes",
            core_device_png.len()
        );
        let mut client = SpringBoardServicesClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .unwrap();
        let icon =
            validate_png_icon(client.get_icon_pngdata(bundle_id.clone()).await.unwrap()).unwrap();
        println!("read {bundle_id} icon: {} bytes", icon.len());
    }
}
