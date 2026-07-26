//! Stable WebDriverAgent values and bounded request policy.

use serde::Serialize;

pub const WDA_DEFAULT_SOURCE_CHARS: usize = 128 * 1024;
pub const WDA_MAX_SOURCE_CHARS: usize = 1024 * 1024;
pub const WDA_MAX_SELECTOR_BYTES: usize = 1024;
pub const WDA_MAX_ELEMENTS: usize = 20;
pub const WDA_MAX_TEXT_CHARACTERS: usize = 1024;
pub const WDA_MAX_TEXT_BYTES: usize = 4096;
pub const WDA_MAX_ATTRIBUTE_CHARACTERS: usize = 1024;
pub const WDA_MAX_ATTRIBUTE_BYTES: usize = 4096;
pub const WDA_MIN_HOLD_DURATION_MS: u64 = 100;
pub const WDA_MAX_HOLD_DURATION_MS: u64 = 10_000;
pub const WDA_MAX_WAIT_TIMEOUT_MS: u64 = 10_000;
pub const WDA_MIN_BACKGROUND_DURATION_MS: u64 = 100;
pub const WDA_MAX_BACKGROUND_DURATION_MS: u64 = 5_000;

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
pub struct WdaUnlockResult {
    pub was_locked: bool,
    pub unlocked: bool,
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
    /// Compatibility value emitted only for presence-based wait conditions.
    pub fn expected_present(self) -> Option<bool> {
        match self {
            Self::Present => Some(true),
            Self::Absent => Some(false),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WdaRunnerPhase {
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WdaRunnerStatus {
    pub phase: WdaRunnerPhase,
    pub managed: bool,
    pub runner_bundle_id: Option<String>,
    pub last_error: Option<String>,
}

impl Default for WdaRunnerStatus {
    fn default() -> Self {
        Self {
            phase: WdaRunnerPhase::Stopped,
            managed: false,
            runner_bundle_id: None,
            last_error: None,
        }
    }
}

pub fn validate_wda_selector(using: &str, value: &str) -> Result<(), &'static str> {
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
    if value.is_empty() || value.len() > WDA_MAX_SELECTOR_BYTES {
        return Err("WDA selector value must contain 1..1024 UTF-8 bytes");
    }
    if value.chars().any(char::is_control) {
        return Err("WDA selector value cannot contain control characters");
    }
    Ok(())
}

pub fn validate_wda_text(text: &str) -> Result<usize, &'static str> {
    let characters = text.chars().count();
    if characters == 0 || characters > WDA_MAX_TEXT_CHARACTERS || text.len() > WDA_MAX_TEXT_BYTES {
        return Err("WDA text must contain 1..1024 characters and at most 4096 UTF-8 bytes");
    }
    if text.contains('\0') {
        return Err("WDA text cannot contain NUL characters");
    }
    Ok(characters)
}

pub fn validate_wda_hold_duration(duration_ms: u64) -> Result<(), &'static str> {
    if !(WDA_MIN_HOLD_DURATION_MS..=WDA_MAX_HOLD_DURATION_MS).contains(&duration_ms) {
        return Err("WDA hold duration must be between 100 and 10000 milliseconds");
    }
    Ok(())
}

pub fn validate_wda_scroll_direction(direction: &str) -> Result<(), &'static str> {
    if matches!(direction, "up" | "down" | "left" | "right") {
        Ok(())
    } else {
        Err("WDA scroll direction must be up, down, left, or right")
    }
}

pub fn validate_wda_wait_timeout(timeout_ms: u64) -> Result<(), &'static str> {
    if timeout_ms <= WDA_MAX_WAIT_TIMEOUT_MS {
        Ok(())
    } else {
        Err("WDA element wait timeout must be between 0 and 10000 milliseconds")
    }
}

pub fn validate_wda_background_duration(duration_ms: Option<u64>) -> Result<(), &'static str> {
    if duration_ms.is_none_or(|duration_ms| {
        (WDA_MIN_BACKGROUND_DURATION_MS..=WDA_MAX_BACKGROUND_DURATION_MS).contains(&duration_ms)
    }) {
        Ok(())
    } else {
        Err("WDA background duration must be between 100 and 5000 milliseconds")
    }
}

pub fn parse_wda_wait_state(state: &str) -> Result<WdaElementWaitState, &'static str> {
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

pub fn validate_wda_runner_bundle_id(bundle_id: &str) -> Result<(), &'static str> {
    if bundle_id.is_empty() || bundle_id.len() > 255 || !bundle_id.ends_with(".xctrunner") {
        return Err("WDA runner bundle ID must end with .xctrunner");
    }
    if bundle_id.starts_with('.')
        || bundle_id.contains("..")
        || bundle_id.split('.').any(|segment| segment.len() > 63)
        || bundle_id
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
    {
        return Err("invalid WDA runner bundle ID");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_are_allowlisted_and_bounded() {
        assert!(validate_wda_selector("accessibility id", "Continue").is_ok());
        assert!(validate_wda_selector("-ios predicate string", "label == 'Play'").is_ok());
        assert!(validate_wda_selector("css selector", "button").is_err());
        assert!(validate_wda_selector("name", "").is_err());
        assert!(validate_wda_selector("xpath", "bad\nvalue").is_err());
        assert!(validate_wda_selector("xpath", &"x".repeat(WDA_MAX_SELECTOR_BYTES + 1)).is_err());
    }

    #[test]
    fn semantic_action_parameters_are_bounded() {
        assert_eq!(validate_wda_text("你好").unwrap(), 2);
        assert!(validate_wda_text("").is_err());
        assert!(validate_wda_text("bad\0text").is_err());
        assert!(validate_wda_text(&"x".repeat(WDA_MAX_TEXT_CHARACTERS + 1)).is_err());
        assert!(validate_wda_text(&"你".repeat(WDA_MAX_TEXT_BYTES / 3 + 1)).is_err());

        assert!(validate_wda_hold_duration(WDA_MIN_HOLD_DURATION_MS).is_ok());
        assert!(validate_wda_hold_duration(WDA_MAX_HOLD_DURATION_MS).is_ok());
        assert!(validate_wda_hold_duration(WDA_MIN_HOLD_DURATION_MS - 1).is_err());
        assert!(validate_wda_hold_duration(WDA_MAX_HOLD_DURATION_MS + 1).is_err());

        for direction in ["up", "down", "left", "right"] {
            assert!(validate_wda_scroll_direction(direction).is_ok());
        }
        assert!(validate_wda_scroll_direction("forward").is_err());
        assert!(validate_wda_scroll_direction("UP").is_err());
        assert!(validate_wda_wait_timeout(0).is_ok());
        assert!(validate_wda_wait_timeout(WDA_MAX_WAIT_TIMEOUT_MS).is_ok());
        assert!(validate_wda_wait_timeout(WDA_MAX_WAIT_TIMEOUT_MS + 1).is_err());
        assert!(validate_wda_background_duration(None).is_ok());
        assert!(validate_wda_background_duration(Some(WDA_MIN_BACKGROUND_DURATION_MS)).is_ok());
        assert!(validate_wda_background_duration(Some(WDA_MAX_BACKGROUND_DURATION_MS)).is_ok());
        assert!(
            validate_wda_background_duration(Some(WDA_MIN_BACKGROUND_DURATION_MS - 1)).is_err()
        );
        assert!(
            validate_wda_background_duration(Some(WDA_MAX_BACKGROUND_DURATION_MS + 1)).is_err()
        );
    }

    #[test]
    fn wait_states_are_parsed_and_preserve_presence_compatibility() {
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
            assert!(parse_wda_wait_state(state).is_ok());
        }
        assert!(parse_wda_wait_state("visible").is_err());
        assert_eq!(WdaElementWaitState::Present.expected_present(), Some(true));
        assert_eq!(WdaElementWaitState::Absent.expected_present(), Some(false));
        assert_eq!(WdaElementWaitState::Enabled.expected_present(), None);
    }

    #[test]
    fn runner_bundle_ids_are_suffix_and_character_bounded() {
        assert!(
            validate_wda_runner_bundle_id("com.example.WebDriverAgentRunner.xctrunner").is_ok()
        );
        assert!(validate_wda_runner_bundle_id("com.example.Runner").is_err());
        assert!(validate_wda_runner_bundle_id("../bad.xctrunner").is_err());
        assert!(validate_wda_runner_bundle_id("com.example.bad_name.xctrunner").is_err());
        assert!(
            validate_wda_runner_bundle_id(&format!("com.{}.xctrunner", "a".repeat(64))).is_err()
        );
        assert!(validate_wda_runner_bundle_id(&format!("{}.xctrunner", "a".repeat(256))).is_err());
    }
}
