//! Bounded, read-only application locations from SpringBoardServices.

use std::collections::HashSet;
use std::time::Duration;

use idevice::RsdService;
use idevice::rsd::RsdHandshake;
use idevice::springboardservices::SpringBoardServicesClient;
use idevice::tcp::handle::AdapterHandle;
use plist::{Dictionary, Value};
use tokio::sync::{mpsc, oneshot, watch};

use devicehub_core::{
    HomeScreenAppLocation, HomeScreenContainer, HomeScreenFolderStep, HomeScreenIconMetrics,
    HomeScreenLayout, WallpaperKind,
};

use crate::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const METRICS_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_LISTS: usize = 32;
const MAX_ITEMS_PER_LIST: usize = 256;
const MAX_FOLDER_DEPTH: usize = 4;
const MAX_APPS: usize = 1_024;
const MAX_BUNDLE_ID_BYTES: usize = 255;
const MAX_NAME_CHARS: usize = 128;
const MAX_LAYOUT_DIMENSION: u64 = 65_535;
const MAX_GRID_COUNT: u64 = 64;
const MAX_PAGE_COUNT: u64 = 255;
const MAX_WALLPAPER_BYTES: usize = 32 * 1024 * 1024;
const MAX_WALLPAPER_DIMENSION: u32 = 16_384;

#[derive(Debug)]
pub enum HomeScreenCommand {
    Get {
        reply: oneshot::Sender<Result<HomeScreenLayout, String>>,
    },
    Wallpaper {
        kind: WallpaperKind,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
}

impl HomeScreenCommand {
    pub fn reject(self, reason: &str) {
        match self {
            Self::Get { reply } => {
                let _ = reply.send(Err(reason.into()));
            }
            Self::Wallpaper { reply, .. } => {
                let _ = reply.send(Err(reason.into()));
            }
        }
    }
}

pub async fn serve_home_screen(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    mut commands: mpsc::Receiver<HomeScreenCommand>,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut client = None;
    let mut attempt = 0;
    reporter.stopped(attempt);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                attempt += 1;
                reporter.connecting(attempt);
                match command {
                    HomeScreenCommand::Get { reply } => {
                        let result = tokio::time::timeout(
                            REQUEST_TIMEOUT,
                            load_layout(&mut client, &mut adapter, &mut handshake),
                        )
                        .await
                        .map_err(|_| "home screen layout request timed out".to_string())
                        .and_then(|result| result);
                        match &result {
                            Ok(layout) => {
                                reporter.ready(attempt);
                                tracing::info!(
                                    apps = layout.apps.len(),
                                    pages = layout.page_count,
                                    metrics_available = layout.metrics.is_some(),
                                    truncated = layout.truncated,
                                    "home screen application locations listed"
                                );
                            }
                            Err(error) => {
                                client.take();
                                reporter.unavailable(attempt, error.clone());
                            }
                        }
                        let _ = reply.send(result);
                    }
                    HomeScreenCommand::Wallpaper { kind, reply } => {
                        let result = tokio::time::timeout(
                            REQUEST_TIMEOUT,
                            load_wallpaper(kind, &mut client, &mut adapter, &mut handshake),
                        )
                        .await
                        .map_err(|_| format!("{} wallpaper request timed out", kind.label()))
                        .and_then(|result| result);
                        match &result {
                            Ok(image) => {
                                reporter.ready(attempt);
                                tracing::info!(
                                    wallpaper = kind.label(),
                                    bytes = image.len(),
                                    "device wallpaper preview loaded"
                                );
                            }
                            Err(error) => {
                                client.take();
                                reporter.unavailable(attempt, error.clone());
                            }
                        }
                        let _ = reply.send(result);
                    }
                }
            }
        }
    }
}

async fn load_wallpaper(
    kind: WallpaperKind,
    client: &mut Option<SpringBoardServicesClient>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) -> Result<Vec<u8>, String> {
    ensure_client(client, adapter, handshake).await?;
    let client = client
        .as_mut()
        .expect("SpringBoard home screen client initialized");
    let image = match kind {
        WallpaperKind::Home => client.get_home_screen_wallpaper_preview_pngdata().await,
        WallpaperKind::Lock => client.get_lock_screen_wallpaper_preview_pngdata().await,
    }
    .map_err(|error| format!("unable to read {} wallpaper: {error:?}", kind.label()))?;
    validate_wallpaper_png(image)
}

async fn ensure_client(
    client: &mut Option<SpringBoardServicesClient>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) -> Result<(), String> {
    if client.is_none() {
        *client = Some(
            tokio::time::timeout(
                CONNECT_TIMEOUT,
                SpringBoardServicesClient::connect_rsd(adapter, handshake),
            )
            .await
            .map_err(|_| "SpringBoard home screen service connection timed out".to_string())?
            .map_err(|error| format!("SpringBoard home screen service unavailable: {error:?}"))?,
        );
    }
    Ok(())
}

async fn load_layout(
    client: &mut Option<SpringBoardServicesClient>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) -> Result<HomeScreenLayout, String> {
    ensure_client(client, adapter, handshake).await?;
    let value = client
        .as_mut()
        .expect("SpringBoard home screen client initialized")
        .get_icon_state(Some("2"))
        .await
        .map_err(|error| format!("unable to read home screen layout: {error:?}"))?;
    let mut layout = parse_layout(&value)?;
    layout.metrics = load_metrics(adapter, handshake).await;
    Ok(layout)
}

fn validate_wallpaper_png(image: Vec<u8>) -> Result<Vec<u8>, String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if image.len() > MAX_WALLPAPER_BYTES {
        return Err("device wallpaper exceeds the 32 MiB limit".into());
    }
    if image.len() < 24
        || &image[..8] != PNG_SIGNATURE
        || &image[12..16] != b"IHDR"
        || u32::from_be_bytes(image[8..12].try_into().unwrap()) != 13
    {
        return Err("device returned an invalid PNG wallpaper".into());
    }
    let width = u32::from_be_bytes(image[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(image[20..24].try_into().unwrap());
    if width == 0
        || height == 0
        || width > MAX_WALLPAPER_DIMENSION
        || height > MAX_WALLPAPER_DIMENSION
    {
        return Err("device returned unsupported wallpaper dimensions".into());
    }
    Ok(image)
}

async fn load_metrics(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) -> Option<HomeScreenIconMetrics> {
    let result = tokio::time::timeout(METRICS_TIMEOUT, async {
        let mut client = SpringBoardServicesClient::connect_rsd(adapter, handshake).await?;
        client.get_homescreen_icon_metrics().await
    })
    .await;
    match result {
        Ok(Ok(value)) => normalize_metrics(&value),
        Ok(Err(error)) => {
            tracing::debug!(?error, "home screen icon metrics unavailable");
            None
        }
        Err(_) => {
            tracing::debug!("home screen icon metrics timed out");
            None
        }
    }
}

fn parse_layout(value: &Value) -> Result<HomeScreenLayout, String> {
    let Value::Array(lists) = value else {
        return Err("device returned an unsupported home screen layout".into());
    };
    let mut parser = LayoutParser {
        apps: Vec::new(),
        seen: HashSet::new(),
        truncated: lists.len() > MAX_LISTS,
    };
    for (list_index, list) in lists.iter().take(MAX_LISTS).enumerate() {
        let Value::Array(items) = list else { continue };
        if items.len() > MAX_ITEMS_PER_LIST {
            parser.truncated = true;
        }
        let (container, page) = if list_index == 0 {
            (HomeScreenContainer::Dock, None)
        } else {
            (HomeScreenContainer::Page, u16::try_from(list_index).ok())
        };
        for (position, item) in items.iter().take(MAX_ITEMS_PER_LIST).enumerate() {
            parser.visit(
                item,
                container,
                page,
                u16::try_from(position + 1).unwrap_or(u16::MAX),
                &[],
                0,
            );
        }
    }
    Ok(HomeScreenLayout {
        apps: parser.apps,
        page_count: u16::try_from(lists.len().saturating_sub(1).min(u16::MAX as usize))
            .unwrap_or(u16::MAX),
        metrics: None,
        truncated: parser.truncated,
    })
}

fn normalize_metrics(value: &Dictionary) -> Option<HomeScreenIconMetrics> {
    let metrics = HomeScreenIconMetrics {
        screen_width: bounded_u32(value, "homeScreenWidth", MAX_LAYOUT_DIMENSION),
        screen_height: bounded_u32(value, "homeScreenHeight", MAX_LAYOUT_DIMENSION),
        icon_width: bounded_u32(value, "homeScreenIconWidth", MAX_LAYOUT_DIMENSION),
        icon_height: bounded_u32(value, "homeScreenIconHeight", MAX_LAYOUT_DIMENSION),
        columns: bounded_u16(value, "homeScreenIconColumns", MAX_GRID_COUNT),
        rows: bounded_u16(value, "homeScreenIconRows", MAX_GRID_COUNT),
        dock_max_count: bounded_u16(value, "homeScreenIconDockMaxCount", MAX_GRID_COUNT),
        folder_columns: bounded_u16(value, "homeScreenIconFolderColumns", MAX_GRID_COUNT),
        folder_rows: bounded_u16(value, "homeScreenIconFolderRows", MAX_GRID_COUNT),
        max_pages: bounded_u16(value, "homeScreenIconMaxPages", MAX_PAGE_COUNT),
        folder_max_pages: bounded_u16(value, "homeScreenIconFolderMaxPages", MAX_PAGE_COUNT),
    };
    (metrics != HomeScreenIconMetrics::default()).then_some(metrics)
}

fn bounded_u32(value: &Dictionary, key: &str, maximum: u64) -> Option<u32> {
    let value = value.get(key)?.as_unsigned_integer()?;
    (value > 0 && value <= maximum)
        .then(|| u32::try_from(value).ok())
        .flatten()
}

fn bounded_u16(value: &Dictionary, key: &str, maximum: u64) -> Option<u16> {
    bounded_u32(value, key, maximum).and_then(|value| u16::try_from(value).ok())
}

struct LayoutParser {
    apps: Vec<HomeScreenAppLocation>,
    seen: HashSet<String>,
    truncated: bool,
}

impl LayoutParser {
    fn visit(
        &mut self,
        value: &Value,
        container: HomeScreenContainer,
        page: Option<u16>,
        root_position: u16,
        folders: &[HomeScreenFolderStep],
        depth: usize,
    ) {
        let Value::Dictionary(item) = value else {
            return;
        };
        if is_widget(item) {
            return;
        }
        if let Some(bundle_id) = item.get("bundleIdentifier").and_then(normalize_bundle_id) {
            if self.apps.len() >= MAX_APPS {
                self.truncated = true;
                return;
            }
            if self.seen.insert(bundle_id.clone()) {
                self.apps.push(HomeScreenAppLocation {
                    bundle_id,
                    name: item.get("displayName").and_then(normalize_name),
                    container,
                    page,
                    position: root_position,
                    folders: folders.to_vec(),
                });
            }
            return;
        }
        let Some(Value::Array(folder_pages)) = item.get("iconLists") else {
            return;
        };
        if depth >= MAX_FOLDER_DEPTH {
            if !folder_pages.is_empty() {
                self.truncated = true;
            }
            return;
        }
        if folder_pages.len() > MAX_LISTS {
            self.truncated = true;
        }
        let folder_name = item.get("displayName").and_then(normalize_name);
        for (folder_page, children) in folder_pages.iter().take(MAX_LISTS).enumerate() {
            let Value::Array(children) = children else {
                continue;
            };
            if children.len() > MAX_ITEMS_PER_LIST {
                self.truncated = true;
            }
            for (position, child) in children.iter().take(MAX_ITEMS_PER_LIST).enumerate() {
                let mut route = folders.to_vec();
                route.push(HomeScreenFolderStep {
                    name: folder_name.clone(),
                    page: u16::try_from(folder_page + 1).unwrap_or(u16::MAX),
                    position: u16::try_from(position + 1).unwrap_or(u16::MAX),
                });
                self.visit(child, container, page, root_position, &route, depth + 1);
            }
        }
    }
}

fn is_widget(item: &Dictionary) -> bool {
    item.contains_key("widgetIdentifier")
        || item
            .get("elementType")
            .and_then(Value::as_string)
            .is_some_and(|value| value.eq_ignore_ascii_case("widget"))
}

fn normalize_bundle_id(value: &Value) -> Option<String> {
    let value = value.as_string()?;
    (value.len() <= MAX_BUNDLE_ID_BYTES
        && !value.is_empty()
        && value.contains('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')))
    .then(|| value.to_owned())
}

fn normalize_name(value: &Value) -> Option<String> {
    let value = value.as_string()?.trim();
    (!value.is_empty()
        && value.chars().count() <= MAX_NAME_CHARS
        && !value.chars().any(char::is_control))
    .then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Dictionary {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    fn app(bundle_id: &str, name: &str) -> Value {
        Value::Dictionary(dictionary([
            ("bundleIdentifier", Value::String(bundle_id.to_owned())),
            ("displayIdentifier", Value::String(bundle_id.to_owned())),
            ("displayName", Value::String(name.to_owned())),
        ]))
    }

    #[test]
    fn normalizes_dock_pages_and_folder_routes() {
        let folder = Value::Dictionary(dictionary([
            ("displayName", Value::String("Games".into())),
            (
                "iconLists",
                Value::Array(vec![Value::Array(vec![app("com.example.game", "Game")])]),
            ),
        ]));
        let raw = Value::Array(vec![
            Value::Array(vec![app("com.apple.MobileSMS", "Messages")]),
            Value::Array(vec![folder]),
        ]);
        let layout = parse_layout(&raw).unwrap();
        assert_eq!(layout.page_count, 1);
        assert_eq!(layout.apps[0].container, HomeScreenContainer::Dock);
        assert_eq!(layout.apps[0].position, 1);
        assert_eq!(layout.apps[1].page, Some(1));
        assert_eq!(layout.apps[1].position, 1);
        assert_eq!(layout.apps[1].folders[0].name.as_deref(), Some("Games"));
        assert_eq!(layout.apps[1].folders[0].page, 1);
        assert_eq!(layout.apps[1].folders[0].position, 1);
    }

    #[test]
    fn omits_widgets_web_clips_private_data_and_duplicate_apps() {
        let widget = Value::Dictionary(dictionary([
            ("elementType", Value::String("widget".into())),
            (
                "bundleIdentifier",
                Value::String("com.example.widget".into()),
            ),
            ("widgetIdentifier", Value::String("PRIVATE-UUID".into())),
        ]));
        let web_clip = Value::Dictionary(dictionary([
            ("displayIdentifier", Value::String("webclip".into())),
            (
                "webClipURL",
                Value::String("https://private.example".into()),
            ),
        ]));
        let duplicate = app("com.example.game", "Game");
        let raw = Value::Array(vec![Value::Array(vec![
            widget,
            web_clip,
            duplicate.clone(),
            duplicate,
        ])]);
        let layout = parse_layout(&raw).unwrap();
        assert_eq!(layout.apps.len(), 1);
        assert_eq!(layout.apps[0].bundle_id, "com.example.game");
    }

    #[test]
    fn rejects_unsupported_shapes_and_bounds_lists() {
        assert!(parse_layout(&Value::Dictionary(Dictionary::new())).is_err());
        let raw = Value::Array((0..=MAX_LISTS).map(|_| Value::Array(Vec::new())).collect());
        let layout = parse_layout(&raw).unwrap();
        assert!(layout.truncated);
        assert_eq!(layout.page_count as usize, MAX_LISTS);
    }

    #[test]
    fn icon_metrics_are_numeric_bounded_and_ignore_private_fields() {
        let raw = dictionary([
            ("homeScreenWidth", Value::from(810_u64)),
            ("homeScreenHeight", Value::from(1080_u64)),
            ("homeScreenIconWidth", Value::from(68_u64)),
            ("homeScreenIconHeight", Value::from(68_u64)),
            ("homeScreenIconColumns", Value::from(5_u64)),
            ("homeScreenIconRows", Value::from(6_u64)),
            ("homeScreenIconDockMaxCount", Value::from(20_u64)),
            ("homeScreenIconFolderColumns", Value::from(4_u64)),
            ("homeScreenIconFolderRows", Value::from(4_u64)),
            ("homeScreenIconMaxPages", Value::from(15_u64)),
            ("homeScreenIconFolderMaxPages", Value::from(15_u64)),
            ("privateIdentifier", Value::String("must-not-leak".into())),
        ]);
        let metrics = normalize_metrics(&raw).unwrap();
        assert_eq!(metrics.screen_width, Some(810));
        assert_eq!(metrics.columns, Some(5));
        assert_eq!(metrics.dock_max_count, Some(20));
        assert_eq!(metrics.folder_columns, Some(4));
        assert!(!format!("{metrics:?}").contains("must-not-leak"));

        let invalid = dictionary([
            ("homeScreenWidth", Value::from(0_u64)),
            ("homeScreenIconRows", Value::from(65_u64)),
            ("homeScreenIconMaxPages", Value::from(256_u64)),
        ]);
        assert_eq!(normalize_metrics(&invalid), None);
    }

    #[test]
    fn wallpaper_previews_are_png_and_dimension_bounded() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&430_u32.to_be_bytes());
        png.extend_from_slice(&932_u32.to_be_bytes());
        assert_eq!(validate_wallpaper_png(png.clone()).unwrap(), png);

        let mut oversized = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        oversized.extend_from_slice(&(MAX_WALLPAPER_DIMENSION + 1).to_be_bytes());
        oversized.extend_from_slice(&932_u32.to_be_bytes());
        assert!(validate_wallpaper_png(oversized).is_err());
        assert!(validate_wallpaper_png(vec![0; 24]).is_err());
    }
}
