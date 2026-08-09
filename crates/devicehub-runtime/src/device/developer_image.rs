//! Read-only Developer Disk Image status queries used by device runtimes.

use devicehub_core::developer_image_type_for_version;
use idevice::services::lockdown::LockdownClient;
use idevice::services::mobile_image_mounter::ImageMounter;
use idevice::{IdeviceError, IdeviceService, provider::IdeviceProvider};

mod mount;

pub(crate) use mount::serve as serve_developer_image_mount;
pub(crate) use mount::unmount_image;
pub use mount::{
    DeveloperImageAssetFuture, DeveloperImageAssetLoader, DeveloperImageAutomaticRequestFuture,
    DeveloperImageMountCommand, DeveloperImageMountRequest, DeveloperImageVariant,
};

/// Reads the device OS version required to choose the image protocol.
pub(crate) async fn read_device_product_version(
    provider: &dyn IdeviceProvider,
) -> Result<String, String> {
    let mut lockdown = LockdownClient::connect(provider)
        .await
        .map_err(|error| format!("cannot connect Lockdown: {error:?}"))?;
    lockdown
        .get_value(Some("ProductVersion"), None)
        .await
        .map_err(|error| format!("cannot read iOS version: {error:?}"))?
        .into_string()
        .ok_or_else(|| "device returned an invalid iOS version".to_string())
}

/// Queries whether the compatible Developer Disk Image is mounted.
pub(crate) async fn is_developer_image_mounted(
    provider: &dyn IdeviceProvider,
    product_version: &str,
) -> Result<bool, String> {
    let image_type = developer_image_type_for_version(product_version)?;
    let mut mounter = ImageMounter::connect(provider)
        .await
        .map_err(|error| format!("cannot connect mobile image mounter: {error:?}"))?;
    match mounter.lookup_image(image_type).await {
        Ok(_) => Ok(true),
        Err(IdeviceError::NotFound) => mounter
            .copy_devices()
            .await
            .map(|entries| {
                entries.iter().any(|entry| {
                    entry
                        .as_dictionary()
                        .is_some_and(|entry| mounted_entry_matches(entry, image_type))
                })
            })
            .map_err(|error| format!("cannot list mounted developer images: {error:?}")),
        Err(error) => Err(format!("cannot query developer image: {error:?}")),
    }
}

fn mounted_entry_matches(entry: &plist::Dictionary, image_type: &str) -> bool {
    entry
        .get("DiskImageType")
        .and_then(plist::Value::as_string)
        .is_some_and(|value| value == image_type)
        && entry
            .get("IsMounted")
            .and_then(plist::Value::as_boolean)
            .unwrap_or(true)
}

/// Reads the OS version and queries Developer Disk Image readiness in one call.
pub(crate) async fn is_developer_image_mounted_for_device(
    provider: &dyn IdeviceProvider,
) -> Result<bool, String> {
    let product_version = read_device_product_version(provider).await?;
    is_developer_image_mounted(provider, &product_version).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_devices_recognizes_mounted_personalized_cryptex_entries() {
        let entry = plist::Dictionary::from_iter([
            (
                String::from("DiskImageType"),
                plist::Value::String("Personalized".into()),
            ),
            (String::from("IsMounted"), plist::Value::Boolean(true)),
            (
                String::from("PersonalizedImageType"),
                plist::Value::String("DeveloperDiskImage".into()),
            ),
        ]);
        assert!(mounted_entry_matches(&entry, "Personalized"));
        assert!(!mounted_entry_matches(&entry, "Developer"));

        let mut unmounted = entry;
        unmounted.insert(String::from("IsMounted"), plist::Value::Boolean(false));
        assert!(!mounted_entry_matches(&unmounted, "Personalized"));
    }
}
