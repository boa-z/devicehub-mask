use std::io::Cursor;
use std::time::SystemTime;

use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use der::Decode;
use der::asn1::{ObjectIdentifier, OctetString};
use plist::{Dictionary, Value};

use crate::protocol::ProvisioningProfile;

const SIGNED_DATA_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const DATA_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");

pub fn parse_profile(raw: &[u8], now: SystemTime) -> Result<ProvisioningProfile, String> {
    let content_info =
        ContentInfo::from_der(raw).map_err(|error| format!("invalid CMS content info: {error}"))?;
    if content_info.content_type != SIGNED_DATA_OID {
        return Err(format!(
            "unsupported CMS content type: {}",
            content_info.content_type
        ));
    }
    let signed_data = content_info
        .content
        .decode_as::<SignedData>()
        .map_err(|error| format!("invalid CMS signed data: {error}"))?;
    if signed_data.encap_content_info.econtent_type != DATA_OID {
        return Err(format!(
            "unsupported profile payload type: {}",
            signed_data.encap_content_info.econtent_type
        ));
    }
    let content = signed_data
        .encap_content_info
        .econtent
        .ok_or_else(|| "CMS profile has no encapsulated content".to_string())?
        .decode_as::<OctetString>()
        .map_err(|error| format!("invalid CMS profile payload: {error}"))?;
    let value = Value::from_reader(Cursor::new(content.as_bytes()))
        .map_err(|error| format!("invalid profile plist: {error}"))?;
    parse_profile_plist(&value, now)
}

fn parse_profile_plist(value: &Value, now: SystemTime) -> Result<ProvisioningProfile, String> {
    let fields = value
        .as_dictionary()
        .ok_or_else(|| "profile plist root is not a dictionary".to_string())?;
    let name = required_string(fields, "Name")?;
    let uuid = required_string(fields, "UUID")?;
    let creation_date = date(fields, "CreationDate");
    let expiration = fields.get("ExpirationDate").and_then(Value::as_date);
    let expiration_date = expiration.map(|date| date.to_xml_format());
    let is_expired = expiration
        .map(|date| SystemTime::from(date) <= now)
        .unwrap_or(false);
    let entitlements = fields.get("Entitlements").and_then(Value::as_dictionary);
    let mut team_identifiers = string_array(fields.get("TeamIdentifier"));
    if team_identifiers.is_empty()
        && let Some(team) = entitlements
            .and_then(|items| items.get("com.apple.developer.team-identifier"))
            .and_then(Value::as_string)
    {
        team_identifiers.push(team.to_owned());
    }
    let provisioned_devices = fields
        .get("ProvisionedDevices")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let application_identifier = entitlements
        .and_then(|items| items.get("application-identifier"))
        .and_then(Value::as_string)
        .map(ToOwned::to_owned);
    let get_task_allow = entitlements
        .and_then(|items| items.get("get-task-allow"))
        .and_then(Value::as_boolean)
        .unwrap_or(false);

    Ok(ProvisioningProfile {
        name,
        uuid,
        team_identifiers,
        application_identifier,
        creation_date,
        expiration_date,
        provisioned_devices,
        is_expired,
        get_task_allow,
        parse_error: None,
    })
}

fn required_string(fields: &Dictionary, key: &str) -> Result<String, String> {
    fields
        .get(key)
        .and_then(Value::as_string)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("profile is missing {key}"))
}

fn date(fields: &Dictionary, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(Value::as_date)
        .map(|date| date.to_xml_format())
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_string)
        .map(ToOwned::to_owned)
        .collect()
}

pub fn unreadable_profile(index: usize, error: String) -> ProvisioningProfile {
    ProvisioningProfile {
        name: format!("Unreadable profile {}", index + 1),
        uuid: format!("invalid-{}", index + 1),
        team_identifiers: Vec::new(),
        application_identifier: None,
        creation_date: None,
        expiration_date: None,
        provisioned_devices: 0,
        is_expired: false,
        get_task_allow: false,
        parse_error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cms::content_info::CmsVersion;
    use cms::signed_data::{EncapsulatedContentInfo, SignerInfos};
    use der::Encode;
    use der::asn1::{Any, OctetString, SetOfVec};
    use std::time::Duration;

    fn profile_value(expiration: SystemTime) -> Value {
        let mut entitlements = Dictionary::new();
        entitlements.insert(
            "application-identifier".into(),
            "TEAM123.com.example.game".into(),
        );
        entitlements.insert("get-task-allow".into(), true.into());

        let mut profile = Dictionary::new();
        profile.insert("Name".into(), "Example Development".into());
        profile.insert("UUID".into(), "00000000-1111-2222-3333-444444444444".into());
        profile.insert(
            "TeamIdentifier".into(),
            Value::Array(vec!["TEAM123".into()]),
        );
        profile.insert(
            "ProvisionedDevices".into(),
            Value::Array(vec!["device-a".into(), "device-b".into()]),
        );
        profile.insert(
            "CreationDate".into(),
            plist::Date::from(SystemTime::UNIX_EPOCH).into(),
        );
        profile.insert(
            "ExpirationDate".into(),
            plist::Date::from(expiration).into(),
        );
        profile.insert("Entitlements".into(), entitlements.into());
        profile.into()
    }

    #[test]
    fn plist_fields_are_normalized_for_the_frontend() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let parsed =
            parse_profile_plist(&profile_value(now + Duration::from_secs(10)), now).unwrap();

        assert_eq!(parsed.name, "Example Development");
        assert_eq!(parsed.team_identifiers, ["TEAM123"]);
        assert_eq!(
            parsed.application_identifier.as_deref(),
            Some("TEAM123.com.example.game")
        );
        assert_eq!(parsed.provisioned_devices, 2);
        assert!(parsed.get_task_allow);
        assert!(!parsed.is_expired);
    }

    #[test]
    fn expired_profiles_are_detected_against_the_supplied_clock() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let parsed =
            parse_profile_plist(&profile_value(now - Duration::from_secs(1)), now).unwrap();
        assert!(parsed.is_expired);
    }

    #[test]
    fn malformed_profiles_return_a_specific_error() {
        let error = parse_profile_plist(&Value::Dictionary(Dictionary::new()), SystemTime::now())
            .unwrap_err();
        assert_eq!(error, "profile is missing Name");
    }

    #[test]
    fn cms_signed_data_payload_is_decoded_before_plist_parsing() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let mut plist_bytes = Vec::new();
        profile_value(now + Duration::from_secs(10))
            .to_writer_xml(&mut plist_bytes)
            .unwrap();
        let payload = OctetString::new(plist_bytes).unwrap();
        let signed_data = SignedData {
            version: CmsVersion::V1,
            digest_algorithms: SetOfVec::default(),
            encap_content_info: EncapsulatedContentInfo {
                econtent_type: DATA_OID,
                econtent: Some(Any::encode_from(&payload).unwrap()),
            },
            certificates: None,
            crls: None,
            signer_infos: SignerInfos(SetOfVec::default()),
        };
        let content_info = ContentInfo {
            content_type: SIGNED_DATA_OID,
            content: Any::encode_from(&signed_data).unwrap(),
        };

        let parsed = parse_profile(&content_info.to_der().unwrap(), now).unwrap();
        assert_eq!(parsed.name, "Example Development");
        assert_eq!(
            parsed.application_identifier.as_deref(),
            Some("TEAM123.com.example.game")
        );
    }

    #[test]
    fn rejects_non_signed_cms_content() {
        let payload = OctetString::new(Vec::<u8>::new()).unwrap();
        let content_info = ContentInfo {
            content_type: DATA_OID,
            content: Any::encode_from(&payload).unwrap(),
        };

        let error = parse_profile(&content_info.to_der().unwrap(), SystemTime::now()).unwrap_err();
        assert!(error.starts_with("unsupported CMS content type:"));
    }

    #[test]
    #[ignore = "requires profiles copied from a physical device"]
    fn parses_profiles_from_hardware_fixture_directory() {
        let directory = std::env::var_os("DEVICEHUB_TEST_PROFILE_DIR")
            .map(std::path::PathBuf::from)
            .expect("set DEVICEHUB_TEST_PROFILE_DIR to a temporary profile directory");
        let mut count = 0;
        for entry in std::fs::read_dir(directory).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            parse_profile(&bytes, SystemTime::now()).unwrap();
            count += 1;
        }
        assert!(count > 0, "the device returned no provisioning profiles");
    }
}
