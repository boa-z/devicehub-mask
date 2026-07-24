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
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(250);
pub const DEFAULT_SOURCE_CHARS: usize = 128 * 1024;
pub const MAX_SOURCE_CHARS: usize = 1024 * 1024;
pub const MAX_SELECTOR_BYTES: usize = 1024;
pub const MAX_ELEMENTS: usize = 20;
pub const MAX_TEXT_CHARACTERS: usize = 1024;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_ATTRIBUTE_CHARACTERS: usize = 1024;
pub const MAX_ATTRIBUTE_BYTES: usize = 4096;
pub const MIN_HOLD_DURATION_MS: u64 = 100;
pub const MAX_HOLD_DURATION_MS: u64 = 10_000;
pub const MAX_WAIT_TIMEOUT_MS: u64 = 10_000;
const MAX_LOGICAL_DIMENSION: f64 = 100_000.0;

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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WdaSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WdaOrientation {
    Portrait,
    PortraitUpsideDown,
    Landscape,
    LandscapeLeft,
    LandscapeRight,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WdaDeviceState {
    pub locked: bool,
    pub orientation: WdaOrientation,
    pub window: WdaSize,
    pub viewport: Option<WdaRect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WdaBoundedText {
    pub text: String,
    pub total_characters: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WdaElementDetails {
    pub element: WdaElement,
    pub element_type: Option<WdaBoundedText>,
    pub name: Option<WdaBoundedText>,
    pub label: Option<WdaBoundedText>,
    pub value: Option<WdaBoundedText>,
    pub displayed: bool,
    pub enabled: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WdaElementWaitResult {
    pub condition_met: bool,
    pub expected_state: WdaElementWaitState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_present: Option<bool>,
    pub index: usize,
    pub returned_matches: usize,
    pub element: Option<WdaElement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WdaElementWaitState {
    Present,
    Absent,
    Displayed,
    Hidden,
    Enabled,
    Disabled,
    Selected,
    Unselected,
}

impl WdaElementWaitState {
    fn expected_present(self) -> Option<bool> {
        match self {
            Self::Present => Some(true),
            Self::Absent => Some(false),
            _ => None,
        }
    }
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
    DeviceState {
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<WdaDeviceState, String>>,
    },
    Find {
        using: String,
        value: String,
        limit: usize,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<Vec<WdaElement>, String>>,
    },
    Inspect {
        using: String,
        value: String,
        index: usize,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<WdaElementDetails, String>>,
    },
    WaitForElement {
        using: String,
        value: String,
        index: usize,
        expected_state: WdaElementWaitState,
        timeout_ms: u64,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<WdaElementWaitResult, String>>,
    },
    Click {
        using: String,
        value: String,
        index: usize,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<WdaElement, String>>,
    },
    TypeText {
        text: String,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<usize, String>>,
    },
    DoubleTap {
        using: String,
        value: String,
        index: usize,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<WdaElement, String>>,
    },
    TouchAndHold {
        using: String,
        value: String,
        index: usize,
        duration_ms: u64,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<WdaElement, String>>,
    },
    Scroll {
        direction: String,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

impl WdaAutomationCommand {
    fn expires_at(&self) -> tokio::time::Instant {
        match self {
            Self::Status { expires_at, .. }
            | Self::Source { expires_at, .. }
            | Self::DeviceState { expires_at, .. }
            | Self::Find { expires_at, .. }
            | Self::Inspect { expires_at, .. }
            | Self::WaitForElement { expires_at, .. }
            | Self::Click { expires_at, .. }
            | Self::TypeText { expires_at, .. }
            | Self::DoubleTap { expires_at, .. }
            | Self::TouchAndHold { expires_at, .. }
            | Self::Scroll { expires_at, .. } => *expires_at,
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
            Self::DeviceState { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::Find { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::Inspect { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::WaitForElement { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::Click { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::TypeText { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::DoubleTap { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::TouchAndHold { reply, .. } => {
                let _ = reply.send(Err(reason));
            }
            Self::Scroll { reply, .. } => {
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

pub fn validate_text(text: &str) -> Result<usize, &'static str> {
    let characters = text.chars().count();
    if characters == 0 || characters > MAX_TEXT_CHARACTERS || text.len() > MAX_TEXT_BYTES {
        return Err("WDA text must contain 1..1024 characters and at most 4096 UTF-8 bytes");
    }
    if text.contains('\0') {
        return Err("WDA text cannot contain NUL characters");
    }
    Ok(characters)
}

pub fn validate_hold_duration(duration_ms: u64) -> Result<(), &'static str> {
    if !(MIN_HOLD_DURATION_MS..=MAX_HOLD_DURATION_MS).contains(&duration_ms) {
        return Err("WDA hold duration must be between 100 and 10000 milliseconds");
    }
    Ok(())
}

pub fn validate_scroll_direction(direction: &str) -> Result<(), &'static str> {
    if matches!(direction, "up" | "down" | "left" | "right") {
        Ok(())
    } else {
        Err("WDA scroll direction must be up, down, left, or right")
    }
}

pub fn validate_wait_timeout(timeout_ms: u64) -> Result<(), &'static str> {
    if timeout_ms <= MAX_WAIT_TIMEOUT_MS {
        Ok(())
    } else {
        Err("WDA element wait timeout must be between 0 and 10000 milliseconds")
    }
}

pub fn parse_wait_state(state: &str) -> Result<WdaElementWaitState, &'static str> {
    match state {
        "present" => Ok(WdaElementWaitState::Present),
        "absent" => Ok(WdaElementWaitState::Absent),
        "displayed" => Ok(WdaElementWaitState::Displayed),
        "hidden" => Ok(WdaElementWaitState::Hidden),
        "enabled" => Ok(WdaElementWaitState::Enabled),
        "disabled" => Ok(WdaElementWaitState::Disabled),
        "selected" => Ok(WdaElementWaitState::Selected),
        "unselected" => Ok(WdaElementWaitState::Unselected),
        _ => Err(
            "WDA element wait state must be present, absent, displayed, hidden, enabled, disabled, selected, or unselected",
        ),
    }
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
        WdaAutomationCommand::DeviceState { expires_at, reply } => {
            let result = read_device_state(client, expires_at).await;
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
        WdaAutomationCommand::Inspect {
            using,
            value,
            index,
            expires_at,
            reply,
        } => {
            let result = inspect_element(client, &using, &value, index, expires_at).await;
            finish(reply, result)
        }
        WdaAutomationCommand::WaitForElement {
            using,
            value,
            index,
            expected_state,
            timeout_ms,
            expires_at,
            reply,
        } => {
            let result = wait_for_element(
                client,
                &using,
                &value,
                index,
                expected_state,
                timeout_ms,
                expires_at,
            )
            .await;
            finish(reply, result)
        }
        WdaAutomationCommand::Click {
            using,
            value,
            index,
            expires_at,
            reply,
        } => {
            let result = async {
                let (element_id, element) =
                    resolve_element(client, &using, &value, index, expires_at).await?;
                within(
                    expires_at,
                    client.click(&element_id, None),
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
                Ok(element)
            }
            .await;
            finish(reply, result)
        }
        WdaAutomationCommand::TypeText {
            text,
            expires_at,
            reply,
        } => {
            let result = async {
                let characters = validate_text(&text).map_err(str::to_string)?;
                ensure_session(client, expires_at).await?;
                within(expires_at, client.send_keys(&text, None), "WDA text input").await?;
                tracing::info!(
                    component = "wda_automation",
                    operation = "type_text",
                    characters,
                    "typed text through WebDriverAgent"
                );
                Ok(characters)
            }
            .await;
            finish(reply, result)
        }
        WdaAutomationCommand::DoubleTap {
            using,
            value,
            index,
            expires_at,
            reply,
        } => {
            let result = async {
                let (element_id, element) =
                    resolve_element(client, &using, &value, index, expires_at).await?;
                within(
                    expires_at,
                    client.double_tap(None, None, Some(&element_id), None),
                    "WDA element double tap",
                )
                .await?;
                tracing::info!(
                    component = "wda_automation",
                    operation = "double_tap",
                    strategy = using,
                    match_index = index,
                    "double-tapped WebDriverAgent element"
                );
                Ok(element)
            }
            .await;
            finish(reply, result)
        }
        WdaAutomationCommand::TouchAndHold {
            using,
            value,
            index,
            duration_ms,
            expires_at,
            reply,
        } => {
            let result = async {
                validate_hold_duration(duration_ms).map_err(str::to_string)?;
                let (element_id, element) =
                    resolve_element(client, &using, &value, index, expires_at).await?;
                within(
                    expires_at,
                    client.touch_and_hold(
                        duration_ms as f64 / 1000.0,
                        None,
                        None,
                        Some(&element_id),
                        None,
                    ),
                    "WDA element hold",
                )
                .await?;
                tracing::info!(
                    component = "wda_automation",
                    operation = "touch_and_hold",
                    strategy = using,
                    match_index = index,
                    duration_ms,
                    "held WebDriverAgent element"
                );
                Ok(element)
            }
            .await;
            finish(reply, result)
        }
        WdaAutomationCommand::Scroll {
            direction,
            expires_at,
            reply,
        } => {
            let result = async {
                validate_scroll_direction(&direction).map_err(str::to_string)?;
                ensure_session(client, expires_at).await?;
                within(
                    expires_at,
                    client.scroll(Some(&direction), None, None, None, None, None),
                    "WDA scroll",
                )
                .await?;
                tracing::info!(
                    component = "wda_automation",
                    operation = "scroll",
                    %direction,
                    "scrolled through WebDriverAgent"
                );
                Ok(())
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

async fn read_device_state(
    client: &mut WdaClient<'_>,
    expires_at: tokio::time::Instant,
) -> Result<WdaDeviceState, String> {
    ensure_session(client, expires_at).await?;
    let locked = within(expires_at, client.is_locked(None), "WDA device lock state").await?;
    let orientation = within(
        expires_at,
        client.orientation(None),
        "WDA device orientation",
    )
    .await
    .and_then(|value| {
        orientation_from_value(&value)
            .ok_or_else(|| "WDA device orientation was unsupported".to_string())
    })?;
    let window = within(expires_at, client.window_size(None), "WDA window size")
        .await
        .and_then(|value| {
            size_from_value(&value).ok_or_else(|| "WDA window size was invalid".to_string())
        })?;
    let viewport = match within(expires_at, client.viewport_rect(None), "WDA viewport").await {
        Ok(value) => rect_from_value(&value),
        Err(error) => {
            tracing::debug!(
                component = "wda_automation",
                operation = "device_state",
                %error,
                "WebDriverAgent viewport unavailable"
            );
            None
        }
    };
    tracing::info!(
        component = "wda_automation",
        operation = "device_state",
        locked,
        ?orientation,
        has_viewport = viewport.is_some(),
        "read WebDriverAgent device state"
    );
    Ok(WdaDeviceState {
        locked,
        orientation,
        window,
        viewport,
    })
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

async fn resolve_element(
    client: &mut WdaClient<'_>,
    using: &str,
    value: &str,
    index: usize,
    expires_at: tokio::time::Instant,
) -> Result<(String, WdaElement), String> {
    let elements = find_element_ids(client, using, value, expires_at).await?;
    let Some(element_id) = elements.get(index) else {
        return Err(format!(
            "WDA selector matched only {} elements",
            elements.len()
        ));
    };
    Ok((
        element_id.clone(),
        WdaElement {
            index,
            rect: element_rect(client, element_id, expires_at).await,
        },
    ))
}

async fn inspect_element(
    client: &mut WdaClient<'_>,
    using: &str,
    value: &str,
    index: usize,
    expires_at: tokio::time::Instant,
) -> Result<WdaElementDetails, String> {
    let (element_id, element) = resolve_element(client, using, value, index, expires_at).await?;
    let element_type = within(
        expires_at,
        client.element_attribute(&element_id, "type", None),
        "WDA element type",
    )
    .await?;
    let name = within(
        expires_at,
        client.element_attribute(&element_id, "name", None),
        "WDA element name",
    )
    .await?;
    let label = within(
        expires_at,
        client.element_attribute(&element_id, "label", None),
        "WDA element label",
    )
    .await?;
    let value = within(
        expires_at,
        client.element_attribute(&element_id, "value", None),
        "WDA element value",
    )
    .await?;
    let displayed = within(
        expires_at,
        client.element_displayed(&element_id, None),
        "WDA element displayed state",
    )
    .await?;
    let enabled = within(
        expires_at,
        client.element_enabled(&element_id, None),
        "WDA element enabled state",
    )
    .await?;
    let selected = within(
        expires_at,
        client.element_selected(&element_id, None),
        "WDA element selected state",
    )
    .await?;
    tracing::info!(
        component = "wda_automation",
        operation = "inspect",
        strategy = using,
        match_index = index,
        displayed,
        enabled,
        selected,
        "inspected WebDriverAgent element"
    );
    Ok(WdaElementDetails {
        element,
        element_type: bounded_attribute(element_type),
        name: bounded_attribute(name),
        label: bounded_attribute(label),
        value: bounded_attribute(value),
        displayed,
        enabled,
        selected,
    })
}

async fn wait_for_element(
    client: &mut WdaClient<'_>,
    using: &str,
    value: &str,
    index: usize,
    expected_state: WdaElementWaitState,
    timeout_ms: u64,
    expires_at: tokio::time::Instant,
) -> Result<WdaElementWaitResult, String> {
    validate_selector(using, value).map_err(str::to_string)?;
    validate_wait_timeout(timeout_ms).map_err(str::to_string)?;
    if index >= MAX_ELEMENTS {
        return Err(format!(
            "WDA element index must be less than {MAX_ELEMENTS}"
        ));
    }
    let wait_deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let elements = find_element_ids(client, using, value, expires_at).await?;
        let condition_met = element_wait_condition(
            client,
            elements.get(index).map(String::as_str),
            expected_state,
            expires_at,
        )
        .await?;
        if condition_met
            || timeout_ms == 0
            || tokio::time::Instant::now() >= wait_deadline
            || tokio::time::Instant::now() >= expires_at
        {
            let element = if let Some(element_id) = elements.get(index) {
                Some(WdaElement {
                    index,
                    rect: element_rect(client, element_id, expires_at).await,
                })
            } else {
                None
            };
            tracing::info!(
                component = "wda_automation",
                operation = "wait_for_element",
                strategy = using,
                match_index = index,
                expected_state = ?expected_state,
                condition_met,
                returned_matches = elements.len(),
                timeout_ms,
                "finished WebDriverAgent element wait"
            );
            return Ok(WdaElementWaitResult {
                condition_met,
                expected_state,
                expected_present: expected_state.expected_present(),
                index,
                returned_matches: elements.len(),
                element,
            });
        }
        let next_poll = tokio::time::Instant::now() + WAIT_POLL_INTERVAL;
        tokio::time::sleep_until(next_poll.min(wait_deadline).min(expires_at)).await;
    }
}

async fn element_wait_condition(
    client: &WdaClient<'_>,
    element_id: Option<&str>,
    expected_state: WdaElementWaitState,
    expires_at: tokio::time::Instant,
) -> Result<bool, String> {
    let Some(element_id) = element_id else {
        return Ok(missing_element_satisfies(expected_state));
    };
    match expected_state {
        WdaElementWaitState::Present => Ok(true),
        WdaElementWaitState::Absent => Ok(false),
        WdaElementWaitState::Displayed | WdaElementWaitState::Hidden => within(
            expires_at,
            client.element_displayed(element_id, None),
            "WDA element displayed state",
        )
        .await
        .map(|displayed| displayed == matches!(expected_state, WdaElementWaitState::Displayed)),
        WdaElementWaitState::Enabled | WdaElementWaitState::Disabled => within(
            expires_at,
            client.element_enabled(element_id, None),
            "WDA element enabled state",
        )
        .await
        .map(|enabled| enabled == matches!(expected_state, WdaElementWaitState::Enabled)),
        WdaElementWaitState::Selected | WdaElementWaitState::Unselected => within(
            expires_at,
            client.element_selected(element_id, None),
            "WDA element selected state",
        )
        .await
        .map(|selected| selected == matches!(expected_state, WdaElementWaitState::Selected)),
    }
}

fn missing_element_satisfies(expected_state: WdaElementWaitState) -> bool {
    matches!(
        expected_state,
        WdaElementWaitState::Absent | WdaElementWaitState::Hidden
    )
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

fn size_from_value(value: &Value) -> Option<WdaSize> {
    let width = value.get("width")?.as_f64()?;
    let height = value.get("height")?.as_f64()?;
    if [width, height]
        .into_iter()
        .all(|value| value.is_finite() && value > 0.0 && value <= MAX_LOGICAL_DIMENSION)
    {
        Some(WdaSize { width, height })
    } else {
        None
    }
}

fn orientation_from_value(value: &str) -> Option<WdaOrientation> {
    match value.trim().to_ascii_uppercase().as_str() {
        "PORTRAIT" => Some(WdaOrientation::Portrait),
        "PORTRAIT_UPSIDEDOWN" | "PORTRAIT_UPSIDE_DOWN" => Some(WdaOrientation::PortraitUpsideDown),
        "LANDSCAPE" => Some(WdaOrientation::Landscape),
        "LANDSCAPELEFT" | "LANDSCAPE_LEFT" => Some(WdaOrientation::LandscapeLeft),
        "LANDSCAPERIGHT" | "LANDSCAPE_RIGHT" => Some(WdaOrientation::LandscapeRight),
        _ => None,
    }
}

fn bounded_attribute(value: Value) -> Option<WdaBoundedText> {
    let value = match value {
        Value::String(value) => value,
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) => return None,
    };
    let total_characters = value.chars().count();
    let mut text = String::with_capacity(value.len().min(MAX_ATTRIBUTE_BYTES));
    for character in value.chars().take(MAX_ATTRIBUTE_CHARACTERS) {
        if text.len() + character.len_utf8() > MAX_ATTRIBUTE_BYTES {
            break;
        }
        text.push(character);
    }
    let truncated = text.len() < value.len();
    Some(WdaBoundedText {
        text,
        total_characters,
        truncated,
    })
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
    fn semantic_action_parameters_are_bounded() {
        assert_eq!(validate_text("你好").unwrap(), 2);
        assert!(validate_text("").is_err());
        assert!(validate_text("bad\0text").is_err());
        assert!(validate_text(&"x".repeat(MAX_TEXT_CHARACTERS + 1)).is_err());
        assert!(validate_text(&"你".repeat(MAX_TEXT_BYTES / 3 + 1)).is_err());

        assert!(validate_hold_duration(MIN_HOLD_DURATION_MS).is_ok());
        assert!(validate_hold_duration(MAX_HOLD_DURATION_MS).is_ok());
        assert!(validate_hold_duration(MIN_HOLD_DURATION_MS - 1).is_err());
        assert!(validate_hold_duration(MAX_HOLD_DURATION_MS + 1).is_err());

        for direction in ["up", "down", "left", "right"] {
            assert!(validate_scroll_direction(direction).is_ok());
        }
        assert!(validate_scroll_direction("forward").is_err());
        assert!(validate_scroll_direction("UP").is_err());
        assert!(validate_wait_timeout(0).is_ok());
        assert!(validate_wait_timeout(MAX_WAIT_TIMEOUT_MS).is_ok());
        assert!(validate_wait_timeout(MAX_WAIT_TIMEOUT_MS + 1).is_err());
        for state in [
            "present",
            "absent",
            "displayed",
            "hidden",
            "enabled",
            "disabled",
            "selected",
            "unselected",
        ] {
            assert!(parse_wait_state(state).is_ok());
        }
        assert!(parse_wait_state("visible").is_err());
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

    #[test]
    fn device_geometry_and_orientation_are_normalized() {
        assert_eq!(
            size_from_value(&json!({ "width": 430, "height": 932, "private": "ignored" })),
            Some(WdaSize {
                width: 430.0,
                height: 932.0,
            })
        );
        assert!(size_from_value(&json!({ "width": 0, "height": 932 })).is_none());
        assert!(size_from_value(&json!({ "width": 430, "height": 1e200 })).is_none());
        assert_eq!(
            orientation_from_value("PORTRAIT_UPSIDE_DOWN"),
            Some(WdaOrientation::PortraitUpsideDown)
        );
        assert_eq!(
            orientation_from_value("landscapeRight"),
            Some(WdaOrientation::LandscapeRight)
        );
        assert!(orientation_from_value("diagonal").is_none());
    }

    #[test]
    fn element_attributes_are_primitive_and_bounded() {
        assert_eq!(
            bounded_attribute(json!("Button")),
            Some(WdaBoundedText {
                text: "Button".into(),
                total_characters: 6,
                truncated: false,
            })
        );
        assert_eq!(bounded_attribute(json!(true)).unwrap().text, "true");
        assert!(bounded_attribute(json!(null)).is_none());
        assert!(bounded_attribute(json!({ "private": "payload" })).is_none());

        let bounded = bounded_attribute(json!("你".repeat(MAX_ATTRIBUTE_CHARACTERS + 1))).unwrap();
        assert_eq!(bounded.text.chars().count(), MAX_ATTRIBUTE_CHARACTERS);
        assert_eq!(bounded.total_characters, MAX_ATTRIBUTE_CHARACTERS + 1);
        assert!(bounded.truncated);
    }

    #[test]
    fn missing_element_satisfies_only_absent_and_hidden_states() {
        for state in [WdaElementWaitState::Absent, WdaElementWaitState::Hidden] {
            assert!(missing_element_satisfies(state));
        }
        for state in [
            WdaElementWaitState::Present,
            WdaElementWaitState::Displayed,
            WdaElementWaitState::Enabled,
            WdaElementWaitState::Disabled,
            WdaElementWaitState::Selected,
            WdaElementWaitState::Unselected,
        ] {
            assert!(!missing_element_satisfies(state));
        }
    }

    #[test]
    fn wait_results_preserve_presence_compatibility_without_inventing_it_for_other_states() {
        let result = |expected_state| WdaElementWaitResult {
            condition_met: true,
            expected_state,
            expected_present: expected_state.expected_present(),
            index: 0,
            returned_matches: 0,
            element: None,
        };
        let absent = serde_json::to_value(result(WdaElementWaitState::Absent)).unwrap();
        assert_eq!(absent["expected_state"], "absent");
        assert_eq!(absent["expected_present"], false);

        let enabled = serde_json::to_value(result(WdaElementWaitState::Enabled)).unwrap();
        assert_eq!(enabled["expected_state"], "enabled");
        assert!(enabled.get("expected_present").is_none());
    }
}
