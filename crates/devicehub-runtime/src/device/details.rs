//! Device identity and hardware metadata queried through lockdown services.

use std::time::Duration;

use devicehub_core::{
    DeviceActivationState, DeviceBattery, DeviceDetails, DeviceRegionalSettings, DeviceStorage,
    validate_device_name,
};
use idevice::IdeviceService;
use idevice::diagnostics_relay::DiagnosticsRelayClient;
use idevice::lockdown::LockdownClient;
use idevice::mobile_image_mounter::ImageMounter;
use idevice::mobileactivationd::MobileActivationdClient;
use idevice::provider::IdeviceProvider;

pub(crate) async fn read_device_details(
    provider: &dyn IdeviceProvider,
    requested_udid: String,
) -> Option<DeviceDetails> {
    let mut lockdown = LockdownClient::connect(provider).await.ok()?;
    let values = lockdown.get_value(None, None).await.ok()?;
    let values = values.as_dictionary()?;
    let integer = |key: &str| values.get(key).and_then(plist::Value::as_unsigned_integer);
    let disk_usage = lockdown
        .get_value(None, Some("com.apple.disk_usage"))
        .await
        .ok()
        .and_then(plist::Value::into_dictionary);
    let storage = disk_usage.as_ref().and_then(storage_from_disk_usage);
    let mut total_disk_capacity = disk_usage
        .as_ref()
        .and_then(|values| values.get("TotalDiskCapacity"))
        .and_then(plist::Value::as_unsigned_integer)
        .or_else(|| integer("TotalDiskCapacity"));
    if total_disk_capacity.is_none() {
        total_disk_capacity = lockdown
            .get_value(Some("TotalDiskCapacity"), Some("com.apple.disk_usage"))
            .await
            .ok()
            .and_then(|value| value.as_unsigned_integer());
    }
    Some(DeviceDetails {
        udid: identity_token(values, "UniqueDeviceID", 128).unwrap_or(requested_udid),
        name: display_name(values).unwrap_or_else(|| "iOS Device".to_string()),
        product_type: identity_token(values, "ProductType", 32)
            .unwrap_or_else(|| "Unknown".to_string()),
        product_version: identity_token(values, "ProductVersion", 32)
            .unwrap_or_else(|| "Unknown".to_string()),
        build_version: identity_token(values, "BuildVersion", 32),
        device_class: identity_token(values, "DeviceClass", 32),
        cpu_architecture: identity_token(values, "CPUArchitecture", 32),
        model_number: identity_token(values, "ModelNumber", 32),
        hardware_model: identity_token(values, "HardwareModel", 32),
        device_color: identity_token(values, "DeviceColor", 32),
        enclosure_color: identity_token(values, "EnclosureColor", 32),
        serial_number: identity_token(values, "SerialNumber", 64),
        ecid: integer("UniqueChipID").map(|value| value.to_string()),
        total_disk_capacity,
        storage,
        activation_state: None,
        developer_mode_enabled: None,
        developer_image_mounted: None,
        regional_settings: regional_settings(values),
        battery: None,
    })
}

pub(crate) async fn rename_device(
    provider: &dyn IdeviceProvider,
    requested_name: &str,
) -> Result<String, String> {
    let name = validate_device_name(requested_name).map_err(str::to_string)?;
    let mut lockdown = LockdownClient::connect(provider)
        .await
        .map_err(|error| format!("cannot connect Lockdown for device rename: {error}"))?;
    let pairing_file = provider
        .get_pairing_file()
        .await
        .map_err(|error| format!("cannot load pairing record for device rename: {error}"))?;
    lockdown
        .start_session(&pairing_file)
        .await
        .map_err(|error| format!("cannot start Lockdown session for device rename: {error}"))?;
    let rename_result: Result<(), String> = async {
        lockdown
            .set_value("DeviceName", plist::Value::String(name.clone()), None)
            .await
            .map_err(|error| format!("device rejected the new name: {error}"))?;
        let verified = lockdown
            .get_value(Some("DeviceName"), None)
            .await
            .map_err(|error| format!("cannot verify the new device name: {error}"))?
            .into_string()
            .ok_or_else(|| "device returned an invalid name after rename".to_string())?;
        if verified != name {
            return Err("device did not retain the requested name".into());
        }
        Ok(())
    }
    .await;
    match tokio::time::timeout(Duration::from_secs(1), lockdown.stop_session()).await {
        Ok(Ok(())) => tracing::debug!("device rename Lockdown session stopped"),
        Ok(Err(error)) => {
            tracing::warn!(%error, "unable to stop device rename Lockdown session")
        }
        Err(_) => tracing::warn!("stopping device rename Lockdown session timed out"),
    }
    rename_result?;
    tracing::info!(
        name_chars = name.chars().count(),
        "device name changed through Lockdown"
    );
    Ok(name)
}

pub(crate) async fn read_activation_state(
    provider: &dyn IdeviceProvider,
) -> Result<DeviceActivationState, String> {
    let raw = MobileActivationdClient::new(provider)
        .state()
        .await
        .map_err(|error| format!("cannot read activation state: {error:?}"))?;
    Ok(normalize_activation_state(&raw))
}

/// Uses AMFI first and preserves the legacy MobileImageMounter fallback for
/// devices whose AMFI service does not expose the status operation.
pub(crate) async fn read_device_developer_mode_status(
    provider: &dyn IdeviceProvider,
) -> Result<bool, String> {
    match tokio::time::timeout(
        Duration::from_millis(1_500),
        super::developer_mode::read_developer_mode_status(provider),
    )
    .await
    {
        Ok(Ok(enabled)) => return Ok(enabled),
        Ok(Err(error)) => {
            tracing::debug!(%error, "AMFI developer mode status unavailable; falling back to MobileImageMounter");
        }
        Err(_) => tracing::debug!(
            "AMFI developer mode status timed out; falling back to MobileImageMounter"
        ),
    }
    let mut mounter = ImageMounter::connect(provider)
        .await
        .map_err(|error| format!("cannot connect mobile image mounter: {error:?}"))?;
    mounter
        .query_developer_mode_status()
        .await
        .map_err(|error| format!("cannot query developer mode: {error:?}"))
}

pub(crate) async fn read_device_battery(
    provider: &dyn IdeviceProvider,
) -> Result<DeviceBattery, String> {
    let mut diagnostics = DiagnosticsRelayClient::connect(provider)
        .await
        .map_err(|error| format!("cannot connect diagnostics relay: {error:?}"))?;
    let values = diagnostics
        .ioregistry(None, Some("AppleSmartBattery"), None)
        .await
        .map_err(|error| format!("cannot query AppleSmartBattery: {error:?}"))?
        .ok_or_else(|| "AppleSmartBattery returned no data".to_string())?;
    Ok(battery_from_ioregistry(&values))
}

fn display_name(values: &plist::Dictionary) -> Option<String> {
    let value = values.get("DeviceName")?.as_string()?.trim();
    let characters = value.chars().count();
    (!value.is_empty()
        && value.len() <= 255
        && characters <= 64
        && !value.chars().any(char::is_control))
    .then(|| value.to_string())
}

fn identity_token(values: &plist::Dictionary, key: &str, max_characters: usize) -> Option<String> {
    let value = values.get(key)?.as_string()?.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value.chars().count() <= max_characters
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.' | '#' | '/' | ',')
        }))
    .then(|| value.to_string())
}

fn regional_settings(values: &plist::Dictionary) -> Option<DeviceRegionalSettings> {
    let token = |key: &str, max_chars: usize, allowed: fn(char) -> bool| {
        values
            .get(key)
            .and_then(plist::Value::as_string)
            .map(str::trim)
            .filter(|value| {
                !value.is_empty()
                    && value.chars().count() <= max_chars
                    && value.chars().all(allowed)
            })
            .map(ToOwned::to_owned)
    };
    let regional = DeviceRegionalSettings {
        language: token("Language", 35, |character| {
            character.is_ascii_alphanumeric() || character == '-'
        }),
        locale: token("Locale", 64, |character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
        }),
        time_zone: token("TimeZone", 64, |character| {
            character.is_ascii_alphanumeric() || matches!(character, '/' | '_' | '-' | '+' | '.')
        }),
        uses_24_hour_clock: values
            .get("Uses24HourClock")
            .and_then(plist::Value::as_boolean),
    };
    (regional.language.is_some()
        || regional.locale.is_some()
        || regional.time_zone.is_some()
        || regional.uses_24_hour_clock.is_some())
    .then_some(regional)
}

fn normalize_activation_state(value: &str) -> DeviceActivationState {
    match value.trim().to_ascii_lowercase().as_str() {
        "activated" => DeviceActivationState::Activated,
        "unactivated" => DeviceActivationState::Unactivated,
        "factoryactivated" | "factory_activated" => DeviceActivationState::FactoryActivated,
        "softactivated" | "soft_activated" => DeviceActivationState::SoftActivated,
        _ => DeviceActivationState::Unknown,
    }
}

fn storage_from_disk_usage(values: &plist::Dictionary) -> Option<DeviceStorage> {
    let unsigned = |key: &str| values.get(key).and_then(plist::Value::as_unsigned_integer);
    let storage = DeviceStorage {
        data_capacity_bytes: unsigned("TotalDataCapacity"),
        data_available_bytes: unsigned("TotalDataAvailable"),
        system_capacity_bytes: unsigned("TotalSystemCapacity"),
        system_available_bytes: unsigned("TotalSystemAvailable"),
    };
    (storage.data_capacity_bytes.is_some()
        || storage.data_available_bytes.is_some()
        || storage.system_capacity_bytes.is_some()
        || storage.system_available_bytes.is_some())
    .then_some(storage)
}

fn battery_from_ioregistry(values: &plist::Dictionary) -> DeviceBattery {
    let unsigned = |dictionary: &plist::Dictionary, key: &str, maximum: u64| {
        dictionary
            .get(key)
            .and_then(plist::Value::as_unsigned_integer)
            .filter(|value| *value <= maximum)
    };
    let signed = |dictionary: &plist::Dictionary, key: &str, absolute_maximum: i64| {
        dictionary
            .get(key)
            .and_then(plist::Value::as_signed_integer)
            .filter(|value| value.unsigned_abs() <= absolute_maximum as u64)
    };
    let boolean = |dictionary: &plist::Dictionary, key: &str| {
        dictionary.get(key).and_then(|value| {
            value
                .as_boolean()
                .or_else(|| value.as_unsigned_integer().map(|value| value != 0))
        })
    };
    let battery_data = values
        .get("BatteryData")
        .and_then(plist::Value::as_dictionary);
    let adapter = values
        .get("AdapterDetails")
        .and_then(plist::Value::as_dictionary);
    let charger_data = values
        .get("ChargerData")
        .and_then(plist::Value::as_dictionary);
    let design_capacity_mah =
        battery_data.and_then(|data| unsigned(data, "DesignCapacity", 100_000));
    let full_charge_capacity_mah =
        battery_data.and_then(|data| unsigned(data, "FullChargeCapacity", 100_000));
    let health_percent = unsigned(values, "MaximumCapacityPercent", 100)
        .or_else(|| battery_data.and_then(|data| unsigned(data, "MaximumCapacityPercent", 100)))
        .map(|value| value as f64)
        .or_else(|| {
            design_capacity_mah
                .filter(|capacity| *capacity > 0)
                .zip(full_charge_capacity_mah)
                .map(|(design, full)| (full as f64 * 100.0 / design as f64).clamp(0.0, 100.0))
        });
    let temperature_celsius = signed(values, "Temperature", 8_000)
        .or_else(|| signed(values, "BatteryTemperature", 8_000))
        .or_else(|| battery_data.and_then(|data| signed(data, "Temperature", 8_000)))
        .map(|value| value as f64 / 100.0)
        .filter(|value| (-20.0..=80.0).contains(value));

    DeviceBattery {
        level_percent: unsigned(values, "CurrentCapacity", 100)
            .or_else(|| battery_data.and_then(|data| unsigned(data, "CurrentCapacity", 100)))
            .map(|value| value as u8),
        temperature_celsius,
        is_charging: boolean(values, "IsCharging")
            .or_else(|| charger_data.and_then(|data| boolean(data, "IsCharging"))),
        external_connected: boolean(values, "ExternalConnected")
            .or_else(|| boolean(values, "AppleRawExternalConnected")),
        fully_charged: boolean(values, "FullyCharged")
            .or_else(|| battery_data.and_then(|data| boolean(data, "FullyCharged"))),
        cycle_count: unsigned(values, "CycleCount", 100_000),
        voltage_mv: unsigned(values, "Voltage", 30_000)
            .or_else(|| unsigned(values, "AppleRawBatteryVoltage", 30_000)),
        instant_amperage_ma: signed(values, "InstantAmperage", 100_000)
            .or_else(|| signed(values, "Amperage", 100_000)),
        design_capacity_mah,
        full_charge_capacity_mah,
        health_percent,
        time_remaining_minutes: unsigned(values, "TimeRemaining", 7 * 24 * 60)
            .or_else(|| unsigned(values, "AvgTimeToEmpty", 7 * 24 * 60)),
        adapter_watts: adapter.and_then(|details| unsigned(details, "Watts", 1_000)),
        adapter_name: adapter
            .and_then(|details| details.get("Name"))
            .and_then(plist::Value::as_string)
            .and_then(normalized_diagnostic_label),
    }
}

fn normalized_diagnostic_label(value: &str) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty()
        && value.chars().count() <= 64
        && value
            .chars()
            .all(|character| !character.is_control() && !matches!(character, '/' | '\\')))
    .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn reads_developer_mode_status_from_hardware() {
        use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection};

        let mut usbmuxd = UsbmuxdConnection::default().await.expect("connect usbmuxd");
        let device = usbmuxd
            .get_devices()
            .await
            .expect("list devices")
            .into_iter()
            .find(|device| matches!(device.connection_type, Connection::Usb))
            .expect("connected USB device");
        let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-details-test");
        let enabled = read_device_developer_mode_status(&provider)
            .await
            .expect("query developer mode");
        eprintln!("developer mode enabled: {enabled}");
    }

    fn dictionary<const N: usize>(entries: [(&str, plist::Value); N]) -> plist::Dictionary {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }

    #[test]
    fn activation_states_are_reduced_to_a_stable_public_enum() {
        assert_eq!(
            normalize_activation_state("Activated"),
            DeviceActivationState::Activated
        );
        assert_eq!(
            normalize_activation_state(" Unactivated "),
            DeviceActivationState::Unactivated
        );
        assert_eq!(
            normalize_activation_state("FactoryActivated"),
            DeviceActivationState::FactoryActivated
        );
        assert_eq!(
            normalize_activation_state("soft_activated"),
            DeviceActivationState::SoftActivated
        );
        assert_eq!(
            normalize_activation_state("future-state\nprivate-data"),
            DeviceActivationState::Unknown
        );
    }

    #[test]
    fn normalizes_and_bounds_lockdown_metadata() {
        let values = dictionary([
            ("DeviceName", plist::Value::String(" Boa 的 iPhone ".into())),
            ("ProductType", plist::Value::String("iPhone14,3".into())),
            ("Language", plist::Value::String(" zh-Hant ".into())),
            ("Locale", plist::Value::String("zh_TW".into())),
            ("TimeZone", plist::Value::String("Asia/Taipei".into())),
            ("Uses24HourClock", plist::Value::Boolean(true)),
        ]);
        assert_eq!(display_name(&values).as_deref(), Some("Boa 的 iPhone"));
        assert_eq!(
            identity_token(&values, "ProductType", 32).as_deref(),
            Some("iPhone14,3")
        );
        let regional = regional_settings(&values).unwrap();
        assert_eq!(regional.language.as_deref(), Some("zh-Hant"));
        assert_eq!(regional.locale.as_deref(), Some("zh_TW"));
        assert_eq!(regional.time_zone.as_deref(), Some("Asia/Taipei"));
        assert_eq!(regional.uses_24_hour_clock, Some(true));

        let invalid = dictionary([
            ("DeviceName", plist::Value::String("phone\nprivate".into())),
            ("Token", plist::Value::String("iPhone Pro".into())),
            ("Language", plist::Value::String("x".repeat(36))),
        ]);
        assert!(display_name(&invalid).is_none());
        assert!(identity_token(&invalid, "Token", 32).is_none());
        assert!(regional_settings(&invalid).is_none());
    }

    #[test]
    fn normalizes_disk_usage_without_inventing_missing_values() {
        let values = dictionary([
            (
                "TotalDataCapacity",
                plist::Value::Integer(120_000_000_000_u64.into()),
            ),
            (
                "TotalDataAvailable",
                plist::Value::Integer(45_000_000_000_u64.into()),
            ),
            (
                "TotalSystemCapacity",
                plist::Value::Integer(8_000_000_000_u64.into()),
            ),
        ]);
        let storage = storage_from_disk_usage(&values).unwrap();
        assert_eq!(storage.data_capacity_bytes, Some(120_000_000_000));
        assert_eq!(storage.data_available_bytes, Some(45_000_000_000));
        assert_eq!(storage.system_capacity_bytes, Some(8_000_000_000));
        assert_eq!(storage.system_available_bytes, None);
        assert!(storage_from_disk_usage(&plist::Dictionary::new()).is_none());
    }

    #[test]
    fn normalizes_battery_diagnostics_without_exposing_private_fields() {
        let battery_data = dictionary([
            ("DesignCapacity", plist::Value::Integer(4325.into())),
            ("FullChargeCapacity", plist::Value::Integer(3482.into())),
        ]);
        let adapter = dictionary([
            (
                "Name",
                plist::Value::String("20W USB-C Power Adapter".into()),
            ),
            ("Watts", plist::Value::Integer(20.into())),
            ("SerialString", plist::Value::String("must-not-leak".into())),
        ]);
        let values = dictionary([
            ("CurrentCapacity", plist::Value::Integer(52.into())),
            ("IsCharging", plist::Value::Boolean(true)),
            ("CycleCount", plist::Value::Integer(1554.into())),
            ("Voltage", plist::Value::Integer(4009.into())),
            ("Temperature", plist::Value::Integer(3150.into())),
            ("InstantAmperage", plist::Value::Integer(2153.into())),
            ("BatteryData", plist::Value::Dictionary(battery_data)),
            ("AdapterDetails", plist::Value::Dictionary(adapter)),
        ]);
        let battery = battery_from_ioregistry(&values);
        assert_eq!(battery.level_percent, Some(52));
        assert_eq!(battery.temperature_celsius, Some(31.5));
        assert_eq!(battery.cycle_count, Some(1554));
        assert_eq!(
            battery.adapter_name.as_deref(),
            Some("20W USB-C Power Adapter")
        );
        assert!((battery.health_percent.unwrap() - 80.508_670_52).abs() < 1e-6);
        assert!(!format!("{battery:?}").contains("must-not-leak"));
    }

    #[test]
    fn bounds_untrusted_battery_diagnostics() {
        let adapter = dictionary([
            ("Name", plist::Value::String("private/path\0adapter".into())),
            ("Watts", plist::Value::Integer(50_000.into())),
        ]);
        let values = dictionary([
            ("CurrentCapacity", plist::Value::Integer(101.into())),
            ("Temperature", plist::Value::Integer(12_000.into())),
            ("CycleCount", plist::Value::Integer(1_000_000.into())),
            ("Voltage", plist::Value::Integer(100_000.into())),
            ("InstantAmperage", plist::Value::Integer(1_000_000.into())),
            ("MaximumCapacityPercent", plist::Value::Integer(96.into())),
            ("AdapterDetails", plist::Value::Dictionary(adapter)),
        ]);
        let battery = battery_from_ioregistry(&values);
        assert_eq!(battery.health_percent, Some(96.0));
        assert!(battery.level_percent.is_none());
        assert!(battery.temperature_celsius.is_none());
        assert!(battery.cycle_count.is_none());
        assert!(battery.voltage_mv.is_none());
        assert!(battery.instant_amperage_ma.is_none());
        assert!(battery.adapter_watts.is_none());
        assert!(battery.adapter_name.is_none());
    }
}
