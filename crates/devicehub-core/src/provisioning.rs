//! Provisioning profile metadata used by developer-service policy.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ProvisioningProfile {
    pub name: String,
    pub uuid: String,
    pub team_identifiers: Vec<String>,
    pub application_identifier: Option<String>,
    pub creation_date: Option<String>,
    pub expiration_date: Option<String>,
    pub provisioned_devices: usize,
    pub is_expired: bool,
    pub get_task_allow: bool,
    pub removal_supported: bool,
    pub parse_error: Option<String>,
}
