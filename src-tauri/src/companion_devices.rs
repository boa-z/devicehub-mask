//! On-demand Apple Watch discovery through the iPhone CompanionProxy service.

use std::time::Duration;

use idevice::RsdService;
use idevice::companion_proxy::CompanionProxy;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use plist::Value;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};

use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_COMPANIONS: usize = 16;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_VALUE_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompanionDevice {
    pub identifier: String,
    pub name: Option<String>,
    pub product_type: Option<String>,
    pub product_version: Option<String>,
    pub build_version: Option<String>,
}

#[derive(Debug)]
pub enum CompanionDeviceCommand {
    List {
        reply: oneshot::Sender<Result<Vec<CompanionDevice>, String>>,
    },
}

pub async fn serve(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    mut commands: mpsc::Receiver<CompanionDeviceCommand>,
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
                let result = tokio::time::timeout(
                    REQUEST_TIMEOUT,
                    list_companions(&mut client, &mut adapter, &mut handshake),
                )
                .await
                .map_err(|_| "companion device request timed out".to_string())
                .and_then(|result| result);
                match &result {
                    Ok(devices) => {
                        reporter.ready(attempt);
                        tracing::info!(count = devices.len(), "paired companion devices listed");
                    }
                    Err(error) => {
                        client.take();
                        reporter.unavailable(attempt, error.clone());
                    }
                }
                match command {
                    CompanionDeviceCommand::List { reply } => {
                        let _ = reply.send(result);
                    }
                }
            }
        }
    }
}

async fn list_companions(
    client: &mut Option<CompanionProxy>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) -> Result<Vec<CompanionDevice>, String> {
    if client.is_none() {
        *client = Some(
            tokio::time::timeout(
                CONNECT_TIMEOUT,
                CompanionProxy::connect_rsd(adapter, handshake),
            )
            .await
            .map_err(|_| "companion proxy connection timed out".to_string())?
            .map_err(|error| format!("companion proxy unavailable: {error:?}"))?,
        );
    }
    let client = client.as_mut().expect("companion proxy initialized");
    let identifiers = client
        .get_device_registry()
        .await
        .map_err(|error| format!("unable to list paired companion devices: {error:?}"))?;
    let mut devices = Vec::new();
    for identifier in identifiers.into_iter().take(MAX_COMPANIONS) {
        let Some(identifier) = normalize_identifier(identifier) else {
            continue;
        };
        let name = read_value(client, &identifier, "DeviceName").await;
        let product_type = read_value(client, &identifier, "ProductType").await;
        let product_version = read_value(client, &identifier, "ProductVersion").await;
        let build_version = read_value(client, &identifier, "BuildVersion").await;
        devices.push(CompanionDevice {
            identifier,
            name,
            product_type,
            product_version,
            build_version,
        });
    }
    Ok(devices)
}

async fn read_value(client: &mut CompanionProxy, identifier: &str, key: &str) -> Option<String> {
    let value = client.get_value(identifier, key).await.ok()?;
    normalize_value(value)
}

fn normalize_identifier(value: String) -> Option<String> {
    (value.len() <= MAX_IDENTIFIER_BYTES
        && !value.is_empty()
        && !value.chars().any(char::is_control))
    .then_some(value)
}

fn normalize_value(value: Value) -> Option<String> {
    let value = match value {
        Value::String(value) => value,
        Value::Integer(value) => value.to_string(),
        _ => return None,
    };
    let normalized = value.trim();
    (!normalized.is_empty() && !normalized.chars().any(char::is_control))
        .then(|| normalized.chars().take(MAX_VALUE_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_values_are_bounded_and_sanitized() {
        assert_eq!(
            normalize_value(Value::String("  Apple Watch  ".into())),
            Some("Apple Watch".into())
        );
        assert_eq!(normalize_value(Value::String("bad\nvalue".into())), None);
        assert_eq!(
            normalize_value(Value::String("x".repeat(MAX_VALUE_CHARS + 10))),
            Some("x".repeat(MAX_VALUE_CHARS))
        );
        assert_eq!(normalize_value(Value::Boolean(true)), None);
    }

    #[test]
    fn companion_identifiers_reject_unbounded_or_control_content() {
        assert_eq!(
            normalize_identifier("watch-id".into()),
            Some("watch-id".into())
        );
        assert_eq!(normalize_identifier(String::new()), None);
        assert_eq!(normalize_identifier("watch\nid".into()), None);
        assert_eq!(
            normalize_identifier("x".repeat(MAX_IDENTIFIER_BYTES + 1)),
            None
        );
    }
}
