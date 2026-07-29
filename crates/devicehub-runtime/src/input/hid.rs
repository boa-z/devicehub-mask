//! Adapters between DeviceHub input values and idevice's typed HID service.

use devicehub_core::TouchContact;
use idevice::ReadWrite;
use idevice::core_device::hid::{HidSurface, TouchscreenContact, UniversalHidServiceClient};
use tokio::sync::mpsc;

pub(crate) fn touchscreen_contacts(contacts: &[TouchContact]) -> Vec<TouchscreenContact> {
    contacts
        .iter()
        .map(|contact| TouchscreenContact {
            identity: contact.identity,
            touching: contact.touching,
            x: contact.x,
            y: contact.y,
        })
        .collect()
}

/// Capture a normalized HID surface diagnostic without exposing idevice
/// client types to the host. Failure is diagnostic-only and never prevents
/// input setup from completing.
pub(crate) async fn capture_connected_services(
    client: &mut UniversalHidServiceClient<Box<dyn ReadWrite>>,
    sink: Option<mpsc::Sender<Vec<u8>>>,
) {
    let Some(sink) = sink else {
        return;
    };
    let services = match client.list_connected_services().await {
        Ok(services) => services,
        Err(error) => {
            tracing::warn!(?error, "failed to query HID surfaces");
            return;
        }
    };
    let bytes = match encode_connected_services(&services) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(%error, "failed to encode HID surfaces");
            return;
        }
    };
    if let Err(error) = sink.try_send(bytes) {
        tracing::warn!(%error, "failed to publish HID surfaces");
    }
}

fn encode_connected_services(services: &[HidSurface]) -> Result<Vec<u8>, plist::Error> {
    let services = services
        .iter()
        .map(|surface| {
            let mut value = plist::Dictionary::new();
            value.insert("serviceId".into(), surface.service_id.into());
            if let Some(product) = &surface.product {
                value.insert("product".into(), product.clone().into());
            }
            if let Some(usage) = surface.primary_usage {
                value.insert("primaryUsage".into(), usage.into());
            }
            if let Some(usage_page) = surface.primary_usage_page {
                value.insert("primaryUsagePage".into(), usage_page.into());
            }
            plist::Value::Dictionary(value)
        })
        .collect();
    let mut root = plist::Dictionary::new();
    root.insert("connectedServices".into(), plist::Value::Array(services));
    let mut bytes = Vec::new();
    plist::to_writer_xml(&mut bytes, &root)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::core_device::hid::build_multitouch_report;

    #[test]
    fn contacts_are_adapted_without_changing_identity_or_state() {
        let contacts = touchscreen_contacts(&[
            TouchContact {
                identity: 2,
                touching: true,
                x: 0x1234,
                y: 0x5678,
            },
            TouchContact {
                identity: 3,
                touching: false,
                x: 0x9abc,
                y: 0xdef0,
            },
        ]);

        assert_eq!(contacts.len(), 2);
        assert_eq!(contacts[0].identity, 2);
        assert!(contacts[0].touching);
        assert_eq!((contacts[0].x, contacts[0].y), (0x1234, 0x5678));
        assert_eq!(contacts[1].identity, 3);
        assert!(!contacts[1].touching);
        assert_eq!((contacts[1].x, contacts[1].y), (0x9abc, 0xdef0));
        let report = build_multitouch_report(&contacts, Some(1))
            .expect("adapted contacts must use idevice's multi-touch report");
        assert_eq!(&report[..3], &[0x09, 0x02, 0x05]);
    }

    #[test]
    fn connected_service_diagnostics_are_normalized_xml() {
        let services = [HidSurface {
            service_id: 257,
            product: Some("CoreDevice touchscreen(nil)".into()),
            primary_usage: Some(4),
            primary_usage_page: Some(13),
        }];
        let bytes = encode_connected_services(&services).expect("encode HID diagnostics");
        let xml = String::from_utf8(bytes).expect("XML is UTF-8");
        assert!(xml.contains("<key>serviceId</key>"));
        assert!(xml.contains("<integer>257</integer>"));
        assert!(xml.contains("CoreDevice touchscreen(nil)"));
    }
}
