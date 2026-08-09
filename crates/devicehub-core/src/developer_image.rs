//! Developer Disk Image domain state and version policy.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperImageKind {
    Legacy,
    Personalized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperImageSourceKind {
    Xcode,
    Custom,
    Managed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperImageMountPolicy {
    Manual,
    #[default]
    Ask,
    Automatic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeveloperImageSetDescriptor {
    pub id: String,
    pub kind: DeveloperImageKind,
    pub source: DeveloperImageSourceKind,
    pub display_name: String,
    pub platform: String,
    pub product_version: Option<String>,
    pub product_build_version: Option<String>,
    pub image_name: String,
    pub auxiliary_name: String,
    pub manifest_name: Option<String>,
    pub size_bytes: u64,
    pub removable: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperImageMountState {
    #[default]
    Idle,
    Validating,
    Personalizing,
    Uploading,
    Mounting,
    Unmounting,
    Mounted,
    Unmounted,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperImageOperation {
    Mount,
    Unmount,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DeveloperImageMountStatus {
    pub state: DeveloperImageMountState,
    pub operation: Option<DeveloperImageOperation>,
    pub progress_percent: Option<f64>,
    pub product_version: Option<String>,
    pub image_type: Option<String>,
    pub error: Option<String>,
}

/// Shared observation port for one session's Developer Disk Image operation.
#[derive(Clone, Debug, Default)]
pub struct DeveloperImageMountSlot(Arc<Mutex<DeveloperImageMountStatus>>);

impl DeveloperImageMountSlot {
    pub fn set(&self, status: DeveloperImageMountStatus) {
        *self.0.lock().expect("developer image status lock poisoned") = status;
    }

    pub fn update(&self, update: impl FnOnce(&mut DeveloperImageMountStatus)) {
        update(&mut self.0.lock().expect("developer image status lock poisoned"));
    }

    pub fn get(&self) -> DeveloperImageMountStatus {
        self.0
            .lock()
            .expect("developer image status lock poisoned")
            .clone()
    }

    pub fn reset(&self) {
        self.set(DeveloperImageMountStatus::default());
    }
}

/// Resolves the image type expected by an iOS major version.
pub fn developer_image_type_for_version(product_version: &str) -> Result<&'static str, String> {
    let major = product_version
        .split('.')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| format!("invalid iOS version {product_version:?}"))?;
    Ok(if major < 17 {
        "Developer"
    } else {
        "Personalized"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_type_tracks_the_personalized_image_transition() {
        assert_eq!(
            developer_image_type_for_version("16.7.12").unwrap(),
            "Developer"
        );
        assert_eq!(
            developer_image_type_for_version("17.0").unwrap(),
            "Personalized"
        );
        assert_eq!(
            developer_image_type_for_version("27.0").unwrap(),
            "Personalized"
        );
        assert!(developer_image_type_for_version("").is_err());
        assert!(developer_image_type_for_version("future").is_err());
    }

    #[test]
    fn cloned_mount_slot_shares_updates_and_reset() {
        let slot = DeveloperImageMountSlot::default();
        let reader = slot.clone();
        slot.update(|status| {
            status.state = DeveloperImageMountState::Uploading;
            status.operation = Some(DeveloperImageOperation::Mount);
            status.progress_percent = Some(50.0);
        });
        assert_eq!(reader.get().state, DeveloperImageMountState::Uploading);
        assert_eq!(reader.get().operation, Some(DeveloperImageOperation::Mount));
        assert_eq!(reader.get().progress_percent, Some(50.0));
        slot.reset();
        assert_eq!(reader.get(), DeveloperImageMountStatus::default());
    }
}
