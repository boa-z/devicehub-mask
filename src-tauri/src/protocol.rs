//! Compatibility facade for code not yet migrated to the core/runtime boundaries.
//!
//! New code should import pure models from `domain` and runtime ports from
//! `device_runtime` directly. Keeping this facade temporarily lets boundary
//! extraction proceed without coupling it to a repository-wide path rewrite.

pub(crate) use crate::device_runtime::commands::{ControlCmd, InputCmd};
pub(crate) use crate::device_runtime::state::{
    ActiveSlot, AppOperationSlot, ClipboardSlot, DeviceListSlot, ErrorSlot, InputSink,
    LocationStatusSlot, OrientationSlot, StatusSlot, VideoCounters,
};
pub(crate) use crate::domain::{
    AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AppOperationView, ConnKind, DeviceApp,
    DeviceCrashReportList, DeviceCrashReportSummary, DeviceDetails, DeviceInfo, DevicePairingState,
    ForgetDeviceOutcome, ForgetDeviceResult, HARDWARE_BUTTON_NAMES, LocationStatus, Orientation,
    PairDeviceOutcome, PairDeviceResult, ProvisioningProfile, RotateDir, device_selector, norm,
    unrotate_norm, validate_device_name, validate_paste_text,
};
#[cfg(test)]
pub(crate) use crate::domain::{
    CrashReportFormat, CrashReportKind, DeviceActivationState, DeviceBattery, DeviceCrashReport,
    DeviceCrashReportContent, DeviceRegionalSettings,
};
