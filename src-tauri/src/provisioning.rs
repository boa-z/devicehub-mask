use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use der::Decode;
use der::asn1::{ObjectIdentifier, OctetString};
use idevice::IdeviceService;
use idevice::misagent::MisagentClient;
use idevice::provider::IdeviceProvider;
use plist::{Dictionary, Value};
use tokio::sync::{mpsc, oneshot, watch};

use crate::protocol::ProvisioningProfile;
use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const SIGNED_DATA_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const DATA_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_PROFILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PROFILE_COUNT: usize = 512;
const MAX_PROFILE_STRING_CHARS: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProvisioningFailure {
    Invalid(String),
    NotFound(String),
    Conflict(String),
    Unavailable(String),
    Deadline(String),
    Timeout(String),
}

impl ProvisioningFailure {
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::Unavailable(message)
            | Self::Deadline(message)
            | Self::Timeout(message) => message,
        }
    }

    fn reconnect_service(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::Timeout(_))
    }
}

impl std::fmt::Display for ProvisioningFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

#[derive(Debug)]
pub enum ProvisioningCommand {
    List {
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<Vec<ProvisioningProfile>, ProvisioningFailure>>,
    },
    Install {
        path: PathBuf,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<ProvisioningProfile, ProvisioningFailure>>,
    },
    Remove {
        uuid: String,
        expires_at: tokio::time::Instant,
        reply: oneshot::Sender<Result<(), ProvisioningFailure>>,
    },
}

pub async fn supervise(
    provider: Arc<dyn IdeviceProvider>,
    mut commands: mpsc::Receiver<ProvisioningCommand>,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        attempt += 1;
        reporter.connecting(attempt);
        let mut client =
            match tokio::time::timeout(CONNECT_TIMEOUT, MisagentClient::connect(provider.as_ref()))
                .await
            {
                Ok(Ok(client)) => client,
                Ok(Err(error)) => {
                    let error = format!("provisioning profile service unavailable: {error:?}");
                    reporter.retrying(attempt, error);
                    if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                        break;
                    }
                    continue;
                }
                Err(_) => {
                    reporter.retrying(attempt, "provisioning profile service connection timed out");
                    if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                        break;
                    }
                    continue;
                }
            };
        reporter.ready(attempt);

        let failure = loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break None;
                    }
                }
                command = commands.recv() => {
                    let Some(command) = command else { break None };
                    if let Err(error) = handle_command(&mut client, command).await {
                        break Some(error);
                    }
                }
            }
        };
        let Some(error) = failure else { break };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

async fn handle_command(
    client: &mut MisagentClient,
    command: ProvisioningCommand,
) -> Result<(), String> {
    match command {
        ProvisioningCommand::List { expires_at, reply } => {
            let Some(reply) = active_reply(expires_at, reply) else {
                return Ok(());
            };
            match list_profiles_until(client, expires_at).await {
                Ok(profiles) => {
                    let _ = reply.send(Ok(profiles));
                    Ok(())
                }
                Err(error) => {
                    let reconnect = error.reconnect_service();
                    let message = error.to_string();
                    let _ = reply.send(Err(error));
                    if reconnect { Err(message) } else { Ok(()) }
                }
            }
        }
        ProvisioningCommand::Install {
            path,
            expires_at,
            reply,
        } => {
            let Some(reply) = active_reply(expires_at, reply) else {
                return Ok(());
            };
            let (raw, profile) = match load_install_profile(&path, expires_at).await {
                Ok(profile) => profile,
                Err(error) => {
                    let _ = reply.send(Err(error));
                    return Ok(());
                }
            };
            let result: Result<ProvisioningProfile, ProvisioningFailure> = async {
                timeout_at(expires_at, client.install(raw), "profile installation").await?;
                let installed = list_profiles_until(client, expires_at).await?;
                if !installed
                    .iter()
                    .any(|item| same_uuid(&item.uuid, &profile.uuid))
                {
                    return Err(ProvisioningFailure::Conflict(
                        "profile installation was not present during verification".into(),
                    ));
                }
                tracing::info!(
                    component = "provisioning",
                    operation = "install",
                    profile_uuid = %profile.uuid,
                    "provisioning profile installed"
                );
                Ok(profile)
            }
            .await;
            match result {
                Ok(profile) => {
                    let _ = reply.send(Ok(profile));
                    Ok(())
                }
                Err(error) => {
                    let reconnect = error.reconnect_service();
                    let message = error.to_string();
                    let _ = reply.send(Err(error));
                    if reconnect { Err(message) } else { Ok(()) }
                }
            }
        }
        ProvisioningCommand::Remove {
            uuid,
            expires_at,
            reply,
        } => {
            let Some(reply) = active_reply(expires_at, reply) else {
                return Ok(());
            };
            let requested = match uuid::Uuid::parse_str(&uuid) {
                Ok(uuid) => uuid,
                Err(_) => {
                    let _ = reply.send(Err(ProvisioningFailure::Invalid(
                        "invalid provisioning profile UUID".into(),
                    )));
                    return Ok(());
                }
            };
            let result: Result<(), ProvisioningFailure> = async {
                let profiles = list_profiles_until(client, expires_at).await?;
                let installed = profiles
                    .iter()
                    .find(|profile| uuid::Uuid::parse_str(&profile.uuid).ok() == Some(requested))
                    .ok_or_else(|| {
                        ProvisioningFailure::NotFound(
                            "provisioning profile is not installed".into(),
                        )
                    })?;
                let installed_uuid = installed.uuid.clone();
                timeout_at(
                    expires_at,
                    client.remove(&installed_uuid),
                    "profile removal",
                )
                .await?;
                let remaining = list_profiles_until(client, expires_at).await?;
                if remaining
                    .iter()
                    .any(|profile| same_uuid(&profile.uuid, &installed_uuid))
                {
                    return Err(ProvisioningFailure::Conflict(
                        "provisioning profile remained installed after removal".into(),
                    ));
                }
                tracing::info!(
                    component = "provisioning",
                    operation = "remove",
                    profile_uuid = %installed_uuid,
                    "provisioning profile removed"
                );
                Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    let _ = reply.send(Ok(()));
                    Ok(())
                }
                Err(error) => {
                    let reconnect = error.reconnect_service();
                    let message = error.to_string();
                    let _ = reply.send(Err(error));
                    if reconnect { Err(message) } else { Ok(()) }
                }
            }
        }
    }
}

fn active_reply<T>(
    expires_at: tokio::time::Instant,
    reply: oneshot::Sender<Result<T, ProvisioningFailure>>,
) -> Option<oneshot::Sender<Result<T, ProvisioningFailure>>> {
    if reply.is_closed() {
        return None;
    }
    if tokio::time::Instant::now() >= expires_at {
        let _ = reply.send(Err(ProvisioningFailure::Deadline(
            "provisioning request deadline expired before execution".into(),
        )));
        return None;
    }
    Some(reply)
}

async fn timeout_at<T, E>(
    expires_at: tokio::time::Instant,
    operation: impl Future<Output = Result<T, E>>,
    label: &str,
) -> Result<T, ProvisioningFailure>
where
    E: std::fmt::Debug,
{
    tokio::time::timeout_at(
        expires_at.min(tokio::time::Instant::now() + OPERATION_TIMEOUT),
        operation,
    )
    .await
    .map_err(|_| ProvisioningFailure::Timeout(format!("{label} timed out")))?
    .map_err(|error| ProvisioningFailure::Unavailable(format!("{label} failed: {error:?}")))
}

async fn list_profiles_until(
    client: &mut MisagentClient,
    expires_at: tokio::time::Instant,
) -> Result<Vec<ProvisioningProfile>, ProvisioningFailure> {
    let raw = timeout_at(expires_at, client.copy_all(), "profile listing").await?;
    profiles_from_raw(raw, SystemTime::now()).map_err(ProvisioningFailure::Unavailable)
}

pub fn profiles_from_raw(
    raw_profiles: Vec<Vec<u8>>,
    now: SystemTime,
) -> Result<Vec<ProvisioningProfile>, String> {
    if raw_profiles.len() > MAX_PROFILE_COUNT {
        return Err(format!(
            "device returned more than {MAX_PROFILE_COUNT} provisioning profiles"
        ));
    }
    let mut profiles: Vec<_> = raw_profiles
        .into_iter()
        .enumerate()
        .map(|(index, raw)| {
            if raw.len() as u64 > MAX_PROFILE_BYTES {
                return unreadable_profile(index, "profile exceeds the 16 MiB limit".into());
            }
            parse_profile(&raw, now).unwrap_or_else(|error| {
                tracing::warn!(index, "unable to parse provisioning profile: {error}");
                unreadable_profile(index, error)
            })
        })
        .collect();
    profiles.sort_by(|left, right| {
        left.is_expired
            .cmp(&right.is_expired)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.uuid.cmp(&right.uuid))
    });
    Ok(profiles)
}

async fn load_install_profile(
    path: &Path,
    expires_at: tokio::time::Instant,
) -> Result<(Vec<u8>, ProvisioningProfile), ProvisioningFailure> {
    if !path.is_absolute() || !has_mobileprovision_extension(path) {
        return Err(ProvisioningFailure::Invalid(
            "profile path must be an absolute .mobileprovision file".into(),
        ));
    }
    let canonical = tokio::time::timeout_at(expires_at, tokio::fs::canonicalize(path))
        .await
        .map_err(|_| ProvisioningFailure::Deadline("profile file validation timed out".into()))?
        .map_err(|error| {
            ProvisioningFailure::Invalid(format!("unable to resolve profile file: {error}"))
        })?;
    let metadata = tokio::time::timeout_at(expires_at, tokio::fs::metadata(&canonical))
        .await
        .map_err(|_| ProvisioningFailure::Deadline("profile file validation timed out".into()))?
        .map_err(|error| {
            ProvisioningFailure::Invalid(format!("unable to inspect profile file: {error}"))
        })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_PROFILE_BYTES {
        return Err(ProvisioningFailure::Invalid(
            "profile must be a non-empty regular file no larger than 16 MiB".into(),
        ));
    }
    let raw = tokio::time::timeout_at(expires_at, tokio::fs::read(&canonical))
        .await
        .map_err(|_| ProvisioningFailure::Deadline("profile file read timed out".into()))?
        .map_err(|error| {
            ProvisioningFailure::Invalid(format!("unable to read profile file: {error}"))
        })?;
    let profile = parse_profile(&raw, SystemTime::now()).map_err(|error| {
        ProvisioningFailure::Invalid(format!("invalid provisioning profile: {error}"))
    })?;
    uuid::Uuid::parse_str(&profile.uuid).map_err(|_| {
        ProvisioningFailure::Invalid("provisioning profile contains an invalid UUID".into())
    })?;
    if profile.is_expired {
        return Err(ProvisioningFailure::Invalid(
            "provisioning profile is expired".into(),
        ));
    }
    Ok((raw, profile))
}

fn has_mobileprovision_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mobileprovision"))
}

fn same_uuid(left: &str, right: &str) -> bool {
    let left = uuid::Uuid::parse_str(left).ok();
    left.is_some() && left == uuid::Uuid::parse_str(right).ok()
}

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
    let name = bounded_string(required_string(fields, "Name")?);
    let uuid = bounded_string(required_string(fields, "UUID")?);
    let removal_supported = uuid::Uuid::parse_str(&uuid).is_ok();
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
        team_identifiers.push(bounded_string(team.to_owned()));
    }
    let provisioned_devices = fields
        .get("ProvisionedDevices")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let application_identifier = entitlements
        .and_then(|items| items.get("application-identifier"))
        .and_then(Value::as_string)
        .map(|value| bounded_string(value.to_owned()));
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
        removal_supported,
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
        .take(64)
        .map(|value| bounded_string(value.to_owned()))
        .collect()
}

fn bounded_string(value: String) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_PROFILE_STRING_CHARS)
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
        removal_supported: false,
        parse_error: Some(bounded_string(error)),
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

    fn profile_bytes(expiration: SystemTime) -> Vec<u8> {
        let mut plist_bytes = Vec::new();
        profile_value(expiration)
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
        ContentInfo {
            content_type: SIGNED_DATA_OID,
            content: Any::encode_from(&signed_data).unwrap(),
        }
        .to_der()
        .unwrap()
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
        assert!(parsed.removal_supported);
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
        let parsed = parse_profile(&profile_bytes(now + Duration::from_secs(10)), now).unwrap();
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
    fn profile_metadata_is_bounded_and_single_line() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let mut value = profile_value(now + Duration::from_secs(10));
        value
            .as_dictionary_mut()
            .unwrap()
            .insert("Name".into(), format!("Game\n{}", "x".repeat(800)).into());
        let parsed = parse_profile_plist(&value, now).unwrap();
        assert!(!parsed.name.chars().any(char::is_control));
        assert_eq!(parsed.name.chars().count(), MAX_PROFILE_STRING_CHARS);
    }

    #[test]
    fn device_profile_catalog_is_count_bounded() {
        let error = profiles_from_raw(vec![Vec::new(); MAX_PROFILE_COUNT + 1], SystemTime::now())
            .unwrap_err();
        assert!(error.contains("more than 512"));
    }

    #[tokio::test]
    async fn install_file_validation_accepts_valid_unexpired_profile() {
        let path = std::env::temp_dir().join(format!(
            "devicehub-mask-{}.mobileprovision",
            uuid::Uuid::new_v4()
        ));
        let bytes = profile_bytes(SystemTime::now() + Duration::from_secs(3600));
        std::fs::write(&path, &bytes).unwrap();
        let loaded =
            load_install_profile(&path, tokio::time::Instant::now() + Duration::from_secs(2))
                .await
                .unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(loaded.0, bytes);
        assert_eq!(loaded.1.name, "Example Development");
    }

    #[tokio::test]
    async fn install_file_validation_rejects_wrong_path_and_expired_profile() {
        let relative = Path::new("Game.mobileprovision");
        assert!(
            load_install_profile(
                relative,
                tokio::time::Instant::now() + Duration::from_secs(1)
            )
            .await
            .unwrap_err()
            .message()
            .contains("absolute")
        );

        let path = std::env::temp_dir().join(format!(
            "devicehub-mask-{}.mobileprovision",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            profile_bytes(SystemTime::now() - Duration::from_secs(1)),
        )
        .unwrap();
        let error =
            load_install_profile(&path, tokio::time::Instant::now() + Duration::from_secs(2))
                .await
                .unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            error,
            ProvisioningFailure::Invalid("provisioning profile is expired".into())
        );
    }

    #[test]
    fn only_transport_failures_reconnect_the_misagent_service() {
        assert!(!ProvisioningFailure::Invalid("invalid".into()).reconnect_service());
        assert!(!ProvisioningFailure::NotFound("missing".into()).reconnect_service());
        assert!(!ProvisioningFailure::Conflict("conflict".into()).reconnect_service());
        assert!(!ProvisioningFailure::Deadline("expired".into()).reconnect_service());
        assert!(ProvisioningFailure::Unavailable("closed".into()).reconnect_service());
        assert!(ProvisioningFailure::Timeout("slow".into()).reconnect_service());
    }

    #[tokio::test]
    async fn expired_queued_request_receives_a_deadline_failure() {
        let (reply, response) = oneshot::channel::<Result<(), ProvisioningFailure>>();
        assert!(active_reply(tokio::time::Instant::now(), reply).is_none());
        assert!(matches!(
            response.await.unwrap(),
            Err(ProvisioningFailure::Deadline(_))
        ));
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
