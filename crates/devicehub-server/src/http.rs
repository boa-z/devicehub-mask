//! Device-scoped HTTP adapters shared by desktop and headless hosts.

mod apps;
mod crash_reports;
mod diagnostics;
mod performance;
mod storage;

pub use apps::{AppHttpState, router as apps_router};
pub use crash_reports::{CrashReportHttpState, router as crash_reports_router};
pub use diagnostics::{
    DiagnosticDestinationKind, DiagnosticDestinationPreparer, DiagnosticsHttpState,
    router as diagnostics_router,
};
pub use performance::{
    CaptureDestinationValidator, PerformanceHttpState, router as performance_router,
};
pub use storage::{StorageHttpState, router as storage_router};
