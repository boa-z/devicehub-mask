//! Desktop filesystem implementation of key-mapping profile persistence.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use devicehub_core::validate_key_mapping_profile_name;
use devicehub_server::http::{
    ProfileRepository, ProfileRepositoryError, ProfileRepositoryFuture, ProfileRepositorySnapshot,
    StoredProfile,
};

#[derive(Clone)]
pub(crate) struct TokioProfileRepository {
    profile_dir: Arc<PathBuf>,
}

impl TokioProfileRepository {
    pub(crate) fn new(profile_dir: PathBuf) -> Self {
        Self {
            profile_dir: Arc::new(profile_dir),
        }
    }

    fn profile_path(profile_dir: &Path, name: &str) -> Result<PathBuf, ProfileRepositoryError> {
        validate_key_mapping_profile_name(name).map_err(|_| ProfileRepositoryError::Unavailable)?;
        Ok(profile_dir.join(format!("{name}.json")))
    }

    fn active_profile_path(profile_dir: &Path) -> PathBuf {
        profile_dir.join(".active-profile")
    }
}

impl ProfileRepository for TokioProfileRepository {
    fn snapshot(&self) -> ProfileRepositoryFuture<ProfileRepositorySnapshot> {
        let profile_dir = self.profile_dir.clone();
        Box::pin(async move {
            tokio::fs::create_dir_all(profile_dir.as_ref())
                .await
                .map_err(|_| ProfileRepositoryError::Unavailable)?;
            let mut entries = tokio::fs::read_dir(profile_dir.as_ref())
                .await
                .map_err(|_| ProfileRepositoryError::Unavailable)?;
            let mut profiles = Vec::new();
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|_| ProfileRepositoryError::Unavailable)?
            {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                    continue;
                }
                let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
                    continue;
                };
                if validate_key_mapping_profile_name(name).is_err() {
                    continue;
                }
                let Ok(bytes) = tokio::fs::read(&path).await else {
                    continue;
                };
                profiles.push(StoredProfile {
                    name: name.to_string(),
                    bytes,
                });
            }
            let active = tokio::fs::read_to_string(Self::active_profile_path(&profile_dir))
                .await
                .ok();
            Ok(ProfileRepositorySnapshot { profiles, active })
        })
    }

    fn read(&self, name: String) -> ProfileRepositoryFuture<Vec<u8>> {
        let profile_dir = self.profile_dir.clone();
        Box::pin(async move {
            let path = Self::profile_path(&profile_dir, &name)?;
            tokio::fs::read(path).await.map_err(map_io_error)
        })
    }

    fn write(&self, name: String, bytes: Vec<u8>) -> ProfileRepositoryFuture<()> {
        let profile_dir = self.profile_dir.clone();
        Box::pin(async move {
            let path = Self::profile_path(&profile_dir, &name)?;
            tokio::fs::create_dir_all(profile_dir.as_ref())
                .await
                .map_err(|_| ProfileRepositoryError::Unavailable)?;
            tokio::fs::write(path, bytes)
                .await
                .map_err(|_| ProfileRepositoryError::Unavailable)
        })
    }

    fn exists(&self, name: String) -> ProfileRepositoryFuture<bool> {
        let profile_dir = self.profile_dir.clone();
        Box::pin(async move {
            let path = Self::profile_path(&profile_dir, &name)?;
            tokio::fs::try_exists(path)
                .await
                .map_err(|_| ProfileRepositoryError::Unavailable)
        })
    }

    fn active(&self) -> ProfileRepositoryFuture<Option<String>> {
        let profile_dir = self.profile_dir.clone();
        Box::pin(async move {
            Ok(
                tokio::fs::read_to_string(Self::active_profile_path(&profile_dir))
                    .await
                    .ok(),
            )
        })
    }

    fn set_active(&self, name: String) -> ProfileRepositoryFuture<()> {
        let profile_dir = self.profile_dir.clone();
        Box::pin(async move {
            Self::profile_path(&profile_dir, &name)?;
            tokio::fs::create_dir_all(profile_dir.as_ref())
                .await
                .map_err(|_| ProfileRepositoryError::Unavailable)?;
            tokio::fs::write(Self::active_profile_path(&profile_dir), name)
                .await
                .map_err(|_| ProfileRepositoryError::Unavailable)
        })
    }

    fn delete(&self, name: String) -> ProfileRepositoryFuture<()> {
        let profile_dir = self.profile_dir.clone();
        Box::pin(async move {
            let path = Self::profile_path(&profile_dir, &name)?;
            tokio::fs::remove_file(path).await.map_err(map_io_error)
        })
    }
}

fn map_io_error(error: std::io::Error) -> ProfileRepositoryError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ProfileRepositoryError::NotFound
    } else {
        ProfileRepositoryError::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_repository() -> (TokioProfileRepository, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-profile-files-test-{}",
            uuid::Uuid::new_v4()
        ));
        (TokioProfileRepository::new(directory.clone()), directory)
    }

    #[tokio::test]
    async fn repository_round_trip_preserves_profiles_and_active_selection() {
        let (repository, directory) = test_repository();
        repository
            .write("default".into(), br#"{"version":1}"#.to_vec())
            .await
            .unwrap();
        repository.set_active("default".into()).await.unwrap();

        let snapshot = repository.snapshot().await.unwrap();
        assert_eq!(snapshot.profiles.len(), 1);
        assert_eq!(snapshot.profiles[0].name, "default");
        assert_eq!(snapshot.active.as_deref(), Some("default"));
        assert!(repository.exists("default".into()).await.unwrap());
        assert_eq!(
            repository.read("default".into()).await.unwrap(),
            br#"{"version":1}"#
        );

        repository.delete("default".into()).await.unwrap();
        assert_eq!(
            repository.read("default".into()).await,
            Err(ProfileRepositoryError::NotFound)
        );
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_ignores_invalid_names_non_json_and_unreadable_entries() {
        let (repository, directory) = test_repository();
        tokio::fs::create_dir_all(&directory).await.unwrap();
        tokio::fs::write(directory.join("valid.json"), b"valid")
            .await
            .unwrap();
        tokio::fs::write(directory.join(".hidden.json"), b"hidden")
            .await
            .unwrap();
        tokio::fs::write(directory.join("ignored.txt"), b"ignored")
            .await
            .unwrap();
        tokio::fs::create_dir(directory.join("directory.json"))
            .await
            .unwrap();

        let snapshot = repository.snapshot().await.unwrap();
        assert_eq!(
            snapshot.profiles,
            vec![StoredProfile {
                name: "valid".into(),
                bytes: b"valid".to_vec(),
            }]
        );
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
