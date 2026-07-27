//! Shared native-host composition for the complete private API state graph.

use std::path::PathBuf;

/// Bind host filesystem policies to an existing runtime client.
pub fn state(
    client: devicehub_runtime::RuntimeClient<PathBuf>,
    profile_dir: PathBuf,
    websocket_config: devicehub_server::websocket::WebSocketConfig,
) -> devicehub_server::private_api::PrivateApiState {
    let commands = client.device.commands.clone();
    devicehub_server::private_api::PrivateApiState {
        application: client.clone(),
        device_manager_http: devicehub_server::http::DeviceManagerHttpState::new(
            client.manager.clone(),
        ),
        device_http: devicehub_server::http::DeviceHttpState::new(
            commands.clone(),
            client.device.location.clone(),
            client.device.device_control.clone(),
        ),
        wda_http: devicehub_server::http::WdaHttpState::new(commands.clone()),
        developer_image_http: devicehub_server::http::DeveloperImageHttpState::new(
            commands.clone(),
            client.device.developer_image.clone(),
        ),
        provisioning_http: devicehub_server::http::ProvisioningHttpState::new(commands.clone()),
        performance_http: devicehub_server::http::PerformanceHttpState::new(
            client.device.performance.clone(),
            client.device.performance_demand.clone(),
            client.device.device_logs.clone(),
            client.device.device_log_demand.clone(),
            client.device.device_conditions.clone(),
            client.device.network_capture.clone(),
            client.device.bluetooth_capture.clone(),
            client.device.service_registry.clone(),
            commands.clone(),
            devicehub_server::http::CaptureDestinationValidator::new(
                crate::capture_files::validate_http_destination,
            ),
        ),
        profiles_http: devicehub_server::http::ProfileHttpState::new(
            crate::profile_files::TokioProfileRepository::new(profile_dir),
        ),
        storage_http: devicehub_server::http::StorageHttpState::new(
            commands.clone(),
            client.device.app_documents.clone(),
            client.device.device_files.clone(),
        ),
        diagnostics_http: devicehub_server::http::DiagnosticsHttpState::new(
            commands.clone(),
            client.device.device_backup.clone(),
            client.device.sysdiagnose.clone(),
            client.device.log_archive.clone(),
            devicehub_server::http::DiagnosticDestinationPreparer::new(
                crate::diagnostic_files::prepare_http_destination,
            ),
        ),
        apps_http: devicehub_server::http::AppHttpState::new(
            commands.clone(),
            client.device.app_operation.clone(),
        ),
        crash_reports_http: devicehub_server::http::CrashReportHttpState::new(commands),
        host_http: devicehub_server::http::HostHttpState::unavailable(),
        websocket_config,
    }
}
