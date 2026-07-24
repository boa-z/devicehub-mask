//! Bounded, on-demand WebDriverAgent automation over the active device provider.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use idevice::provider::IdeviceProvider;
use idevice::services::wda::WdaClient;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use crate::supervisor::ServiceReporter;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_SOURCE_CHARS: usize = 128 * 1024;
pub const MAX_SOURCE_CHARS: usize = 1024 * 1024;
pub const MAX_SELECTOR_BYTES: usize = 1024;
pub const MAX_ELEMENTS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WdaStatus {
    pub reachable: bool,
    pub ready: Option<bool>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WdaUiTree {
    pub xml: String,
    pub total_characters: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WdaElement {
    pub index: usize,
    pub rect: Option<WdaRect>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WdaRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug)]
pub enum WdaAutomationCommand {
    Status {
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<WdaStatus, String>>,
    },
    Source {
        max_characters: usize,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<WdaUiTree, String>>,
    },
    Find {
        using: String,
        value: String,
        limit: usize,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<Vec<WdaElement>, String>>,
    },
    Click {
        using: String,
        value: String,
        index: usize,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<WdaElement, String>>,
    },
}

impl WdaAutomationCommand {
    fn expires_at(&self) -> tokio::time::Instant {
        match self {
            Self::Status { expires_at, .. }
            | Self::Source { expires_at, .. }
            | Self::Find { expires_at, .. }
            | Self::Click { expires_at, .. } => *expires_at,
        }
    }

    fn reject(self, reason: impl Into<String>) {
        let reason = reason.into();
        match self {
            Self::Status { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::Source { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::Find { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::Click { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
        }
    }
}

pub fn validate_selector(using: &str, value: &str) -> Result<(), &'static str> {
    if !matches!(
        using,
        "accessibility id"
            | "name"
            | "class name"
            | "xpath"
            | "-ios predicate string"
            | "-ios class chain"
    ) {
        return Err("unsupported WDA selector strategy");
    }
    if value.is_empty() || value.len() > MAX_SELECTOR_BYTES {
        return Err("WDA selector value must contain 1..1024 UTF-8 bytes");
    }
    if value.chars().any(char::is_control) {
        return Err("WDA selector value cannot contain control characters");
    }
    Ok(())
}

pub async fn serve(
    provider: Arc<dyn IdeviceProvider>,
    mut commands: mpsc::Receiver<WdaAutomationCommand>,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    reporter.stopped(0);
    let mut client = WdaClient::new(provider.as_ref()).with_timeout(REQUEST_TIMEOUT);
    let mut attempt = 0;

    loop {
        let command = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { None } else { continue }
            }
            command = commands.recv() => command,
        };
        let Some(command) = command else { break };
        if command.expires_at() <= tokio::time::Instant::now() {
            command.reject("WDA automation request expired");
            continue;
        }

        attempt += 1;
        reporter.connecting(attempt);
        let result = handle_command(&mut client, command).await;
        match result {
            Ok(()) => reporter.ready(attempt),
            Err(error) => {
                reporter.unavailable(attempt, error.clone());
                tracing::warn!(
                    component = "wda_automation",
                    operation = "request",
                    %error,
                    "WebDriverAgent request failed"
                );
                close_session(&client).await;
                client = WdaClient::new(provider.as_ref()).with_timeout(REQUEST_TIMEOUT);
            }
        }
    }

    close_session(&client).await;
    reporter.stopped(attempt);
}

async fn handle_command(
    client: &mut WdaClient<'_>,
    command: WdaAutomationCommand,
) -> Result<(), String> {
    match command {
        WdaAutomationCommand::Status { expires_at, reply } => {
            let result = within(expires_at, client.status(), "WDA status")
                .await
                .map(|value| normalize_status(&value));
            finish(reply, result)
        }
        WdaAutomationCommand::Source {
            max_characters,
            expires_at,
            reply,
        } => {
            let result = async {
                ensure_session(client, expires_at).await?;
                let xml = within(expires_at, client.source(None), "WDA UI tree").await?;
                Ok(bound_source(xml, max_characters))
            }
            .await;
            finish(reply, result)
        }
        WdaAutomationCommand::Find {
            using,
            value,
            limit,
            expires_at,
            reply,
        } => {
            let result = find_elements(client, &using, &value, limit, expires_at).await;
            finish(reply, result)
        }
        WdaAutomationCommand::Click {
            using,
            value,
            index,
            expires_at,
            reply,
        } => {
            let elements = match find_element_ids(client, &using, &value, expires_at).await {
                Ok(elements) => elements,
                Err(error) => return finish(reply, Err(error)),
            };
            let Some(element_id) = elements.get(index) else {
                let _ = reply.send(Err(format!(
                    "WDA selector matched only {} elements",
                    elements.len()
                )));
                return Ok(());
            };
            let result = async {
                let rect = element_rect(client, element_id, expires_at).await;
                within(
                    expires_at,
                    client.click(element_id, None),
                    "WDA element click",
                )
                .await?;
                tracing::info!(
                    component = "wda_automation",
                    operation = "click",
                    strategy = using,
                    match_index = index,
                    "clicked WebDriverAgent element"
                );
                Ok(WdaElement { index, rect })
            }
            .await;
            finish(reply, result)
        }
    }
}

async fn ensure_session(
    client: &mut WdaClient<'_>,
    expires_at: tokio::time::Instant,
) -> Result<(), String> {
    if client.session_id().is_none() {
        within(expires_at, client.start_session(None), "WDA session start").await?;
    }
    Ok(())
}

async fn find_elements(
    client: &mut WdaClient<'_>,
    using: &str,
    value: &str,
    limit: usize,
    expires_at: tokio::time::Instant,
) -> Result<Vec<WdaElement>, String> {
    let elements = find_element_ids(client, using, value, expires_at).await?;
    let mut output = Vec::with_capacity(elements.len().min(limit));
    for (index, element_id) in elements.iter().take(limit).enumerate() {
        output.push(WdaElement {
            index,
            rect: element_rect(client, element_id, expires_at).await,
        });
    }
    Ok(output)
}

async fn find_element_ids(
    client: &mut WdaClient<'_>,
    using: &str,
    value: &str,
    expires_at: tokio::time::Instant,
) -> Result<Vec<String>, String> {
    validate_selector(using, value).map_err(str::to_string)?;
    ensure_session(client, expires_at).await?;
    let mut elements = within(
        expires_at,
        client.find_elements(using, value, None),
        "WDA element search",
    )
    .await?;
    elements.truncate(MAX_ELEMENTS);
    Ok(elements)
}

async fn element_rect(
    client: &WdaClient<'_>,
    element_id: &str,
    expires_at: tokio::time::Instant,
) -> Option<WdaRect> {
    within(
        expires_at,
        client.element_rect(element_id, None),
        "WDA element rectangle",
    )
    .await
    .ok()
    .and_then(|value| rect_from_value(&value))
}

async fn within<T>(
    expires_at: tokio::time::Instant,
    future: impl Future<Output = Result<T, idevice::IdeviceError>>,
    operation: &str,
) -> Result<T, String> {
    tokio::time::timeout_at(expires_at, future)
        .await
        .map_err(|_| format!("{operation} timed out"))?
        .map_err(|error| format!("{operation} failed: {error:?}"))
}

fn finish<T>(
    reply: oneshot::Sender<Result<T, String>>,
    result: Result<T, String>,
) -> Result<(), String> {
    let error = result.as_ref().err().cloned();
    let _ = reply.send(result);
    error.map_or(Ok(()), Err)
}

async fn close_session(client: &WdaClient<'_>) {
    if let Some(session_id) = client.session_id() {
        let _ =
            tokio::time::timeout(Duration::from_secs(2), client.delete_session(session_id)).await;
    }
}

fn normalize_status(value: &Value) -> WdaStatus {
    let body = value.get("value").unwrap_or(value);
    let ready = body.get("ready").and_then(Value::as_bool).or_else(|| {
        body.get("state")
            .and_then(Value::as_str)
            .map(|state| state.eq_ignore_ascii_case("success"))
    });
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .map(|value| value.chars().take(256).collect());
    WdaStatus {
        reachable: true,
        ready,
        message,
    }
}

fn bound_source(xml: String, max_characters: usize) -> WdaUiTree {
    let max_characters = max_characters.clamp(1, MAX_SOURCE_CHARS);
    let total_characters = xml.chars().count();
    let truncated = total_characters > max_characters;
    let xml = if truncated {
        xml.chars().take(max_characters).collect()
    } else {
        xml
    };
    WdaUiTree {
        xml,
        total_characters,
        truncated,
    }
}

fn rect_from_value(value: &Value) -> Option<WdaRect> {
    let number = |name| value.get(name)?.as_f64();
    let rect = WdaRect {
        x: number("x")?,
        y: number("y")?,
        width: number("width")?,
        height: number("height")?,
    };
    if [rect.x, rect.y, rect.width, rect.height]
        .into_iter()
        .all(f64::is_finite)
        && rect.width >= 0.0
        && rect.height >= 0.0
    {
        Some(rect)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selectors_are_allowlisted_and_bounded() {
        assert!(validate_selector("accessibility id", "Continue").is_ok());
        assert!(validate_selector("-ios predicate string", "label == 'Play'").is_ok());
        assert!(validate_selector("css selector", "button").is_err());
        assert!(validate_selector("name", "").is_err());
        assert!(validate_selector("xpath", "bad\nvalue").is_err());
        assert!(validate_selector("xpath", &"x".repeat(MAX_SELECTOR_BYTES + 1)).is_err());
    }

    #[test]
    fn source_is_truncated_on_character_boundaries() {
        let source = bound_source("a你b好c".into(), 4);
        assert_eq!(source.xml, "a你b好");
        assert_eq!(source.total_characters, 5);
        assert!(source.truncated);
    }

    #[test]
    fn status_and_rect_are_normalized_without_raw_payloads() {
        let status = normalize_status(&json!({
            "value": { "ready": true, "message": "ready", "private": "secret" }
        }));
        assert_eq!(status.ready, Some(true));
        assert_eq!(status.message.as_deref(), Some("ready"));
        assert_eq!(
            rect_from_value(&json!({ "x": 1, "y": 2.5, "width": 30, "height": 40 })),
            Some(WdaRect {
                x: 1.0,
                y: 2.5,
                width: 30.0,
                height: 40.0
            })
        );
        assert!(rect_from_value(&json!({ "x": 1, "y": 2, "width": -1, "height": 4 })).is_none());
    }
}
