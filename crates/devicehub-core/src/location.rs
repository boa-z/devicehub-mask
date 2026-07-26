//! Location simulation state exposed by the core service.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationBackend {
    Dvt,
    Legacy,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LocationStatus {
    pub available: bool,
    pub active: bool,
    pub backend: Option<LocationBackend>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub error: Option<String>,
}
