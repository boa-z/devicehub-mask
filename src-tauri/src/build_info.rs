//! Product/build identity and update-channel routing.
//!
//! The product version remains independent from Tauri's updater-compatible
//! package version. Update endpoints are selected from a closed enum so the
//! frontend cannot turn the updater into an arbitrary network client.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

const STABLE_ENDPOINT: &str =
    "https://github.com/boa-z/devicehub-mask/releases/latest/download/latest.json";
const NIGHTLY_ENDPOINT: &str =
    "https://github.com/boa-z/devicehub-mask/releases/download/nightly/latest.json";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UpdateChannel {
    Stable,
    Nightly,
}

impl UpdateChannel {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Stable => STABLE_ENDPOINT,
            Self::Nightly => NIGHTLY_ENDPOINT,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuildInfo {
    version: &'static str,
    build: &'static str,
    commit: &'static str,
    updater_version: String,
    update_channel: UpdateChannel,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateMetadata {
    rid: tauri::ResourceId,
    current_version: String,
    version: String,
    date: Option<String>,
    body: Option<String>,
    raw_json: serde_json::Value,
}

fn compiled_channel() -> UpdateChannel {
    match env!("DEVICEHUB_UPDATE_CHANNEL") {
        "stable" => UpdateChannel::Stable,
        _ => UpdateChannel::Nightly,
    }
}

#[tauri::command]
pub(crate) fn build_info(app: tauri::AppHandle) -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION"),
        build: env!("DEVICEHUB_BUILD_NUMBER"),
        commit: env!("DEVICEHUB_COMMIT"),
        updater_version: app.package_info().version.to_string(),
        update_channel: compiled_channel(),
    }
}

#[tauri::command]
pub(crate) async fn check_for_update(
    webview: tauri::Webview,
    channel: UpdateChannel,
) -> Result<Option<UpdateMetadata>, String> {
    let endpoint = tauri::Url::parse(channel.endpoint())
        .map_err(|error| format!("invalid update endpoint: {error}"))?;
    let updater = webview
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|error| format!("unable to select update channel: {error}"))?
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("unable to initialize updater: {error}"))?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|error| format!("unable to check for updates: {error}"))?
    else {
        return Ok(None);
    };
    let metadata = UpdateMetadata {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        date: None,
        body: update.body.clone(),
        raw_json: update.raw_json.clone(),
        rid: webview.resources_table().add(update),
    };
    Ok(Some(metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_route_only_to_fixed_https_manifests() {
        assert_eq!(UpdateChannel::Stable.endpoint(), STABLE_ENDPOINT);
        assert_eq!(UpdateChannel::Nightly.endpoint(), NIGHTLY_ENDPOINT);
        assert!(STABLE_ENDPOINT.starts_with("https://"));
        assert!(NIGHTLY_ENDPOINT.starts_with("https://"));
    }
}
