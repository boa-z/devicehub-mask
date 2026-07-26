//! Host file adapter for the host-independent provisioning runtime.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use devicehub_runtime::ProvisioningFailure;
pub type ProvisioningCommand = devicehub_runtime::ProvisioningCommand<PathBuf>;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TokioProvisioningProfiles;

impl devicehub_runtime::ProvisioningProfileLoader for TokioProvisioningProfiles {
    type Source = PathBuf;

    fn load<'a>(
        &'a self,
        source: PathBuf,
        expires_at: tokio::time::Instant,
    ) -> devicehub_runtime::ProvisioningProfileFuture<'a> {
        Box::pin(load_install_profile(source, expires_at))
    }
}

async fn load_install_profile(
    path: PathBuf,
    expires_at: tokio::time::Instant,
) -> Result<devicehub_runtime::ProvisioningInstall, ProvisioningFailure> {
    if !path.is_absolute() || !has_mobileprovision_extension(&path) {
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
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > devicehub_runtime::MAX_PROVISIONING_PROFILE_BYTES
    {
        return Err(ProvisioningFailure::Invalid(
            "profile must be a non-empty regular file no larger than 16 MiB".into(),
        ));
    }
    let raw = tokio::time::timeout_at(expires_at, tokio::fs::read(canonical))
        .await
        .map_err(|_| ProvisioningFailure::Deadline("profile file read timed out".into()))?
        .map_err(|error| {
            ProvisioningFailure::Invalid(format!("unable to read profile file: {error}"))
        })?;
    devicehub_runtime::prepare_provisioning_install(raw, SystemTime::now())
}

fn has_mobileprovision_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mobileprovision"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cms::content_info::{CmsVersion, ContentInfo};
    use cms::signed_data::{EncapsulatedContentInfo, SignedData, SignerInfos};
    use der::Encode;
    use der::asn1::{Any, ObjectIdentifier, OctetString, SetOfVec};
    use plist::{Dictionary, Value};
    use std::time::Duration;

    const SIGNED_DATA_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
    const DATA_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");

    fn profile_bytes(expiration: SystemTime) -> Vec<u8> {
        let mut entitlements = Dictionary::new();
        entitlements.insert("get-task-allow".into(), true.into());
        let mut profile = Dictionary::new();
        profile.insert("Name".into(), "Example Development".into());
        profile.insert("UUID".into(), "00000000-1111-2222-3333-444444444444".into());
        profile.insert(
            "ExpirationDate".into(),
            plist::Date::from(expiration).into(),
        );
        profile.insert("Entitlements".into(), Value::Dictionary(entitlements));
        let mut plist_bytes = Vec::new();
        Value::Dictionary(profile)
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

    #[tokio::test]
    async fn host_loader_accepts_a_valid_absolute_profile() {
        let path = std::env::temp_dir().join(format!(
            "devicehub-mask-{}.mobileprovision",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            profile_bytes(SystemTime::now() + Duration::from_secs(3600)),
        )
        .unwrap();
        let loaded = load_install_profile(
            path.clone(),
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await;
        std::fs::remove_file(path).unwrap();
        assert!(loaded.is_ok());
    }

    #[tokio::test]
    async fn host_loader_rejects_relative_and_expired_profiles() {
        let error = load_install_profile(
            PathBuf::from("Game.mobileprovision"),
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await
        .err()
        .unwrap();
        assert!(error.message().contains("absolute"));

        let path = std::env::temp_dir().join(format!(
            "devicehub-mask-{}.mobileprovision",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            profile_bytes(SystemTime::now() - Duration::from_secs(1)),
        )
        .unwrap();
        let error = load_install_profile(
            path.clone(),
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await
        .err()
        .unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            error,
            ProvisioningFailure::Invalid("provisioning profile is expired".into())
        );
    }

    #[test]
    #[ignore = "requires profiles copied from a physical device"]
    fn parses_profiles_from_hardware_fixture_directory() {
        let directory = std::env::var_os("DEVICEHUB_TEST_PROFILE_DIR")
            .map(PathBuf::from)
            .expect("set DEVICEHUB_TEST_PROFILE_DIR to a temporary profile directory");
        let mut count = 0;
        for entry in std::fs::read_dir(directory).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            devicehub_runtime::parse_provisioning_profile(&bytes, SystemTime::now()).unwrap();
            count += 1;
        }
        assert!(count > 0, "the device returned no provisioning profiles");
    }
}
