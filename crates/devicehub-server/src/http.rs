//! Device-scoped HTTP adapters shared by desktop and headless hosts.

mod apps;
mod browser_transfers;
mod crash_reports;
mod developer_image;
mod device;
mod devices;
mod diagnostics;
mod host;
mod performance;
mod profiles;
mod provisioning;
mod storage;
mod wda;

pub use apps::{AppHttpState, router as apps_router};
pub use browser_transfers::{BrowserTransferFuture, BrowserTransferStore};
pub use crash_reports::{CrashReportHttpState, router as crash_reports_router};
pub use developer_image::{DeveloperImageHttpState, router as developer_image_router};
pub use device::{DeviceHttpState, router as device_router};
pub use devices::{DeviceManagerHttpState, router as devices_router};
pub use diagnostics::{
    DiagnosticDestinationKind, DiagnosticDestinationPreparer, DiagnosticsHttpState,
    router as diagnostics_router,
};
pub use host::{
    HostBuildInfo, HostCapabilities, HostControl, HostDiagnosticsStatus, HostHttpState,
    HostSettingsPatch, HostSettingsStatus, router as host_router,
};
pub use performance::{
    CaptureDestinationValidator, PerformanceHttpState, router as performance_router,
};
pub use profiles::{
    ProfileHttpState, ProfileRepository, ProfileRepositoryError, ProfileRepositoryFuture,
    ProfileRepositorySnapshot, StoredProfile, router as profiles_router,
};
pub use provisioning::{ProvisioningHttpState, router as provisioning_router};
pub use storage::{StorageHttpState, router as storage_router};
pub use wda::{WdaHttpState, router as wda_router};
