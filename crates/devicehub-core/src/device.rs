//! Device identity, transport selection, pairing, and hardware metadata.

use serde::Serialize;

/// How a device is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnKind {
    Usb,
    Network,
    Other,
}

impl ConnKind {
    /// A short label for the picker.
    pub fn label(self) -> &'static str {
        match self {
            ConnKind::Usb => "USB",
            ConnKind::Network => "Wi-Fi",
            ConnKind::Other => "?",
        }
    }

    pub fn selector_suffix(self) -> &'static str {
        match self {
            ConnKind::Usb => "usb",
            ConnKind::Network => "wifi",
            ConnKind::Other => "other",
        }
    }
}

pub fn device_selector(udid: &str, connection: ConnKind) -> String {
    format!("{udid}::{}", connection.selector_suffix())
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePairingState {
    #[default]
    NotApplicable,
    Paired,
    Unpaired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairDeviceOutcome {
    Paired,
    Denied,
    Locked,
    TimedOut,
    Failed,
}

#[derive(Debug, Serialize)]
pub struct PairDeviceResult {
    pub outcome: PairDeviceOutcome,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgetDeviceOutcome {
    Forgotten,
    HostRecordRemoved,
    DeviceForgottenHostCleanupFailed,
    Failed,
}

#[derive(Debug, Serialize)]
pub struct ForgetDeviceResult {
    pub outcome: ForgetDeviceOutcome,
    pub error: Option<String>,
}

/// One selectable device transport exposed to clients.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Stable picker value. Unlike the UDID, this distinguishes USB and Wi-Fi.
    pub id: String,
    pub udid: String,
    /// The device's `DeviceName` (best-effort; falls back to the UDID).
    pub name: String,
    pub connection: ConnKind,
    pub pairing: DevicePairingState,
}

/// A paired companion exposed by the iPhone's CompanionProxy registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompanionDevice {
    pub identifier: String,
    pub name: Option<String>,
    pub product_type: Option<String>,
    pub product_version: Option<String>,
    pub build_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceDetails {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub product_version: String,
    pub build_version: Option<String>,
    pub device_class: Option<String>,
    pub cpu_architecture: Option<String>,
    pub model_number: Option<String>,
    pub hardware_model: Option<String>,
    pub device_color: Option<String>,
    pub enclosure_color: Option<String>,
    pub serial_number: Option<String>,
    /// Decimal text avoids losing 64-bit ECID precision in JavaScript clients.
    pub ecid: Option<String>,
    pub total_disk_capacity: Option<u64>,
    pub storage: Option<DeviceStorage>,
    pub activation_state: Option<DeviceActivationState>,
    pub developer_mode_enabled: Option<bool>,
    pub developer_image_mounted: Option<bool>,
    pub regional_settings: Option<DeviceRegionalSettings>,
    pub battery: Option<DeviceBattery>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceRegionalSettings {
    pub language: Option<String>,
    pub locale: Option<String>,
    pub time_zone: Option<String>,
    pub uses_24_hour_clock: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceActivationState {
    Activated,
    Unactivated,
    FactoryActivated,
    SoftActivated,
    Unknown,
}

pub fn validate_device_name(name: &str) -> Result<String, &'static str> {
    let normalized = name.trim();
    let characters = normalized.chars().count();
    if characters == 0 {
        return Err("device name cannot be empty");
    }
    if characters > 64 || normalized.len() > 255 {
        return Err("device name is too long");
    }
    if normalized.chars().any(char::is_control) {
        return Err("device name cannot contain control characters");
    }
    Ok(normalized.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceStorage {
    pub data_capacity_bytes: Option<u64>,
    pub data_available_bytes: Option<u64>,
    pub system_capacity_bytes: Option<u64>,
    pub system_available_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceBattery {
    pub level_percent: Option<u8>,
    pub temperature_celsius: Option<f64>,
    pub is_charging: Option<bool>,
    pub external_connected: Option<bool>,
    pub fully_charged: Option<bool>,
    pub cycle_count: Option<u64>,
    pub voltage_mv: Option<u64>,
    pub instant_amperage_ma: Option<i64>,
    pub design_capacity_mah: Option<u64>,
    pub full_charge_capacity_mah: Option<u64>,
    pub health_percent: Option<f64>,
    pub time_remaining_minutes: Option<u64>,
    pub adapter_watts: Option<u64>,
    pub adapter_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_selectors_are_stable_across_transports() {
        assert_eq!(device_selector("phone", ConnKind::Usb), "phone::usb");
        assert_eq!(device_selector("phone", ConnKind::Network), "phone::wifi");
        assert_eq!(device_selector("phone", ConnKind::Other), "phone::other");
    }

    #[test]
    fn device_name_validation_preserves_unicode_and_rejects_unsafe_values() {
        assert_eq!(
            validate_device_name("  Boa 的 iPhone  ").unwrap(),
            "Boa 的 iPhone"
        );
        assert!(validate_device_name("").is_err());
        assert!(validate_device_name("name\nwith control").is_err());
        assert!(validate_device_name(&"界".repeat(64)).is_ok());
        assert!(validate_device_name(&"界".repeat(65)).is_err());
        assert!(validate_device_name(&"😀".repeat(64)).is_err());
    }
}
