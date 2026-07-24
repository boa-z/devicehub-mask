//! Developer disk image readiness checks shared by device details and XCTest startup.

use idevice::services::lockdown::LockdownClient;
use idevice::services::mobile_image_mounter::ImageMounter;
use idevice::{IdeviceError, IdeviceService, provider::IdeviceProvider};

pub fn image_type_for_version(product_version: &str) -> Result<&'static str, String> {
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

pub async fn read_product_version(provider: &dyn IdeviceProvider) -> Result<String, String> {
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

pub async fn is_mounted(
    provider: &dyn IdeviceProvider,
    product_version: &str,
) -> Result<bool, String> {
    let image_type = image_type_for_version(product_version)?;
    let mut mounter = ImageMounter::connect(provider)
        .await
        .map_err(|error| format!("cannot connect mobile image mounter: {error:?}"))?;
    match mounter.lookup_image(image_type).await {
        Ok(_) => Ok(true),
        Err(IdeviceError::NotFound) => Ok(false),
        Err(error) => Err(format!("cannot query developer image: {error:?}")),
    }
}

pub async fn is_mounted_for_device(provider: &dyn IdeviceProvider) -> Result<bool, String> {
    let product_version = read_product_version(provider).await?;
    is_mounted(provider, &product_version).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_type_tracks_personalized_ddi_transition() {
        assert_eq!(image_type_for_version("16.7.12").unwrap(), "Developer");
        assert_eq!(image_type_for_version("17.0").unwrap(), "Personalized");
        assert_eq!(image_type_for_version("27.0").unwrap(), "Personalized");
        assert!(image_type_for_version("").is_err());
        assert!(image_type_for_version("future").is_err());
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn reads_developer_image_status_from_hardware() {
        use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider = device.to_provider(
            UsbmuxdAddr::default(),
            "devicehub-mask-developer-image-test",
        );
        let product_version = read_product_version(&provider).await.unwrap();
        let mounted = is_mounted(&provider, &product_version).await.unwrap();
        println!("iOS {product_version} developer image mounted: {mounted}");
    }
}
