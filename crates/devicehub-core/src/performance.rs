use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessPerformance {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessEnergy {
    pub pid: u32,
    pub name: String,
    pub total_score: f64,
    pub cpu_score: f64,
    pub gpu_score: f64,
    pub networking_score: f64,
    pub display_score: f64,
    pub location_score: f64,
    pub app_state_score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceNetworkInterfaceKind {
    Wifi,
    Cellular,
    Ethernet,
    Loopback,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceNetworkInterface {
    pub name: String,
    pub kind: DeviceNetworkInterfaceKind,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PerformanceSnapshot {
    pub captured_at_ms: u64,
    pub system_cpu_percent: Option<f64>,
    pub process_count: Option<u32>,
    pub logical_cpu_count: Option<u32>,
    pub physical_cpu_count: Option<u32>,
    pub physical_memory_bytes: Option<u64>,
    pub top_processes: Vec<ProcessPerformance>,
    pub energy_processes: Vec<ProcessEnergy>,
    pub graphics_fps: Option<f64>,
    pub gpu_allocated_bytes: Option<u64>,
    pub gpu_in_use_bytes: Option<u64>,
    pub gpu_driver_bytes: Option<u64>,
    pub gpu_recovery_count: Option<u64>,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
    pub network_recent_connections: Option<u32>,
    pub network_interfaces: Vec<DeviceNetworkInterface>,
    pub network_interfaces_available: bool,
    pub network_interfaces_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppActivityEvent {
    pub sequence: u64,
    pub received_at_ms: u64,
    pub notification_type: String,
    pub app_name: Option<String>,
    pub exec_name: Option<String>,
    pub pid: Option<u32>,
    pub state_description: Option<String>,
}
