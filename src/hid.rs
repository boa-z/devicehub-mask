//! Project-local Universal HID client.
//!
//! `idevice::UniversalHidServiceClient` intentionally exposes only a small
//! typed view of connected surfaces. Multi-touch exploration needs the complete
//! response, so this client preserves the raw plist while retaining the stable
//! single-touch send path used by the application.

use std::borrow::Cow;
use std::path::Path;

use idevice::core_device::{DIGITIZER_SURFACE_MAIN_TOUCHSCREEN, build_touchscreen_report};
use idevice::xpc::{Dictionary, XPCObject};
use idevice::{IdeviceError, ReadWrite, RemoteXpcClient, RsdService};

const SERVICE_NAME: &str = "com.apple.coredevice.hid.universalhidservice";
const FEATURE_ID: &str = "com.apple.coredevice.feature.remote.universalhidservice";
const TOUCHSCREEN_CONTACT_COUNT_MAXIMUM: u8 = 5;
const TOUCHSCREEN_CONTACTS_OFFSET: usize = 3;
const TOUCHSCREEN_CONTACT_SIZE: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchContact {
    pub identity: u8,
    pub touching: bool,
    pub x: u16,
    pub y: u16,
}

/// Build the five-slot report described by Apple's UniversalHID
/// `DigitizerReport.descriptor`.
pub fn build_multitouch_report(
    contacts: &[TouchContact],
    timestamp: Option<u64>,
) -> Result<Vec<u8>, &'static str> {
    if contacts.len() > TOUCHSCREEN_CONTACT_COUNT_MAXIMUM as usize {
        return Err("touchscreen report supports at most five contacts");
    }
    if contacts
        .iter()
        .any(|contact| contact.identity >= TOUCHSCREEN_CONTACT_COUNT_MAXIMUM)
    {
        return Err("contact identity must be in 0..5");
    }
    for (index, contact) in contacts.iter().enumerate() {
        if contacts[..index]
            .iter()
            .any(|other| other.identity == contact.identity)
        {
            return Err("contact identities must be unique");
        }
    }

    let seed = contacts.first().copied().unwrap_or(TouchContact {
        identity: 0,
        touching: false,
        x: 0,
        y: 0,
    });
    let state = (if seed.touching { 0xC0 } else { 0 }) | seed.identity;
    let mut report = build_touchscreen_report(state, seed.x, seed.y, timestamp);
    report[1] = contacts.len() as u8;
    report[2] = TOUCHSCREEN_CONTACT_COUNT_MAXIMUM;
    report[TOUCHSCREEN_CONTACTS_OFFSET
        ..TOUCHSCREEN_CONTACTS_OFFSET
            + TOUCHSCREEN_CONTACT_COUNT_MAXIMUM as usize * TOUCHSCREEN_CONTACT_SIZE]
        .fill(0);

    for (slot, contact) in contacts.iter().enumerate() {
        let offset = TOUCHSCREEN_CONTACTS_OFFSET + slot * TOUCHSCREEN_CONTACT_SIZE;
        report[offset] = (if contact.touching { 0xC0 } else { 0 }) | contact.identity;
        report[offset + 1..offset + 3].copy_from_slice(&contact.x.to_le_bytes());
        report[offset + 3..offset + 5].copy_from_slice(&contact.y.to_le_bytes());
    }
    Ok(report)
}

#[derive(Debug)]
pub struct UniversalHidClient<R: ReadWrite> {
    inner: RemoteXpcClient<R>,
}

impl RsdService for UniversalHidClient<Box<dyn ReadWrite>> {
    fn rsd_service_name() -> Cow<'static, str> {
        Cow::Borrowed(SERVICE_NAME)
    }

    async fn from_stream(stream: Box<dyn ReadWrite>) -> Result<Self, IdeviceError> {
        let mut inner = RemoteXpcClient::new(stream).await?;
        inner.do_handshake().await?;
        Ok(Self { inner })
    }
}

impl<R: ReadWrite> UniversalHidClient<R> {
    fn request(payload: Dictionary) -> Dictionary {
        let mut message = Dictionary::new();
        message.insert(
            "featureIdentifier".into(),
            XPCObject::String(FEATURE_ID.into()),
        );
        message.insert("messageType".into(), XPCObject::String("Request".into()));
        message.insert("payload".into(), XPCObject::Dictionary(payload));
        message
    }

    /// Return the complete response from `connectedServices`, including fields
    /// the typed upstream `HidSurface` representation does not retain.
    pub async fn connected_services_raw(&mut self) -> Result<plist::Value, IdeviceError> {
        let mut query = Dictionary::new();
        query.insert(
            "connectedServices".into(),
            XPCObject::Dictionary(Dictionary::new()),
        );
        self.inner.send_object(Self::request(query), true).await?;
        self.inner.recv().await
    }

    pub async fn send_report(
        &mut self,
        service_id: u64,
        report: Vec<u8>,
    ) -> Result<(), IdeviceError> {
        let mut send = Dictionary::new();
        send.insert("_0".into(), XPCObject::Data(report));
        send.insert("_1".into(), XPCObject::UInt64(service_id));

        let mut payload = Dictionary::new();
        payload.insert("send".into(), XPCObject::Dictionary(send));
        self.inner.send_object(Self::request(payload), false).await
    }

    pub async fn send_touchscreen(
        &mut self,
        state: u8,
        x: u16,
        y: u16,
        timestamp: Option<u64>,
    ) -> Result<(), IdeviceError> {
        self.send_report(
            DIGITIZER_SURFACE_MAIN_TOUCHSCREEN,
            build_touchscreen_report(state, x, y, timestamp),
        )
        .await
    }

    pub async fn tap(&mut self, x: u16, y: u16) -> Result<(), IdeviceError> {
        use idevice::core_device::{TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE};

        self.send_touchscreen(TOUCHSCREEN_STATE_CONTACT, x, y, None)
            .await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.send_touchscreen(TOUCHSCREEN_STATE_RELEASE, x, y, None)
            .await
    }

    /// If `DEVICEHUB_HID_DUMP` is set, export the raw service response as XML
    /// and log every embedded data field with its path and byte length.
    pub async fn dump_services_from_env(&mut self) {
        let Ok(path) = std::env::var("DEVICEHUB_HID_DUMP") else {
            return;
        };

        match self.connected_services_raw().await {
            Ok(value) => {
                log_data_fields(&value, "root");
                match plist::to_file_xml(Path::new(&path), &value) {
                    Ok(()) => tracing::info!("wrote raw HID surface data to {path}"),
                    Err(error) => tracing::warn!("failed to write HID dump {path}: {error}"),
                }
            }
            Err(error) => tracing::warn!("failed to query raw HID surfaces: {error:?}"),
        }
    }
}

fn log_data_fields(value: &plist::Value, path: &str) {
    match value {
        plist::Value::Dictionary(dict) => {
            for (key, value) in dict {
                log_data_fields(value, &format!("{path}.{key}"));
            }
        }
        plist::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                log_data_fields(value, &format!("{path}[{index}]"));
            }
        }
        plist::Value::Data(data) => {
            let prefix = data
                .iter()
                .take(32)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            tracing::info!("HID data field {path}: {} bytes [{prefix}]", data.len());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_keeps_the_expected_coredevice_envelope() {
        let request = UniversalHidClient::<std::io::Cursor<Vec<u8>>>::request(Dictionary::new());
        assert_eq!(
            request.get("featureIdentifier"),
            Some(&XPCObject::String(FEATURE_ID.into()))
        );
        assert_eq!(
            request.get("messageType"),
            Some(&XPCObject::String("Request".into()))
        );
        assert!(matches!(
            request.get("payload"),
            Some(XPCObject::Dictionary(_))
        ));
    }

    #[test]
    fn multitouch_report_uses_five_fixed_contact_slots() {
        let report = build_multitouch_report(
            &[
                TouchContact {
                    identity: 2,
                    touching: true,
                    x: 0x1234,
                    y: 0x5678,
                },
                TouchContact {
                    identity: 3,
                    touching: true,
                    x: 0x9ABC,
                    y: 0xDEF0,
                },
            ],
            Some(1),
        )
        .unwrap();

        assert_eq!(report.len(), 58);
        assert_eq!(&report[..3], &[0x09, 0x02, 0x05]);
        assert_eq!(&report[3..8], &[0xC2, 0x34, 0x12, 0x78, 0x56]);
        assert_eq!(&report[8..13], &[0xC3, 0xBC, 0x9A, 0xF0, 0xDE]);
        assert_eq!(&report[13..28], &[0; 15]);
    }

    #[test]
    fn multitouch_report_rejects_duplicate_identities() {
        let contact = TouchContact {
            identity: 1,
            touching: true,
            x: 0,
            y: 0,
        };
        assert_eq!(
            build_multitouch_report(&[contact, contact], None),
            Err("contact identities must be unique")
        );
    }
}
