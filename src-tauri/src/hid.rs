//! Desktop diagnostics for the runtime Universal HID client.

use std::path::Path;

use devicehub_runtime::UniversalHidClient;
use idevice::ReadWrite;

/// If `DEVICEHUB_HID_DUMP` is set, export the raw service response as XML
/// and log every embedded data field with its path and byte length.
pub(crate) async fn dump_services_from_env(client: &mut UniversalHidClient<Box<dyn ReadWrite>>) {
    let Ok(path) = std::env::var("DEVICEHUB_HID_DUMP") else {
        return;
    };

    match client.connected_services_raw().await {
        Ok(value) => {
            log_data_fields(&value, "root");
            match plist::to_file_xml(Path::new(&path), &value) {
                Ok(()) => tracing::info!("wrote raw HID surface data to {path}"),
                Err(error) => tracing::warn!("failed to write HID dump {path}: {error}"),
            }
        }
        Err(error) => tracing::warn!("failed to query raw HID surfaces: {error:?}"),
    }
}

fn log_data_fields(value: &plist::Value, path: &str) {
    match value {
        plist::Value::Dictionary(dict) => {
            for (key, value) in dict {
                log_data_fields(value, &format!("{path}.{key}"));
            }
        }
        plist::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                log_data_fields(value, &format!("{path}[{index}]"));
            }
        }
        plist::Value::Data(data) => {
            let prefix = data
                .iter()
                .take(32)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            tracing::info!("HID data field {path}: {} bytes [{prefix}]", data.len());
        }
        _ => {}
    }
}
