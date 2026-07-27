//! Native-host asset binding for the runtime-owned Developer Disk Image service.

use std::path::PathBuf;

use tokio::io::AsyncReadExt;

const MAX_PATH_BYTES: usize = 4_096;

#[derive(Debug, Clone, Copy, Default)]
pub struct TokioDeveloperImageAssets;

impl devicehub_runtime::DeveloperImageAssetLoader for TokioDeveloperImageAssets {
    type Source = PathBuf;

    fn file_name(&self, source: &PathBuf, label: &str) -> Result<String, String> {
        source
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("{label} has an invalid file name"))
    }

    fn load<'a>(
        &'a self,
        source: &'a PathBuf,
        label: &'a str,
        max_bytes: u64,
    ) -> devicehub_runtime::DeveloperImageAssetFuture<'a> {
        Box::pin(async move {
            if !source.is_absolute() || source.as_os_str().len() > MAX_PATH_BYTES {
                return Err(format!("{label} must be an absolute local file path"));
            }
            let metadata = tokio::fs::symlink_metadata(source)
                .await
                .map_err(|error| format!("{label} is unavailable: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "{label} must be a regular file, not a symbolic link"
                ));
            }
            if metadata.len() == 0 || metadata.len() > max_bytes {
                return Err(format!("{label} size is outside the supported range"));
            }

            let file = tokio::fs::File::open(source)
                .await
                .map_err(|error| format!("cannot open {label}: {error}"))?;
            let mut bytes = Vec::with_capacity(
                usize::try_from(metadata.len().min(max_bytes)).unwrap_or(usize::MAX),
            );
            file.take(max_bytes + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| format!("cannot read {label}: {error}"))?;
            if bytes.is_empty() || bytes.len() as u64 > max_bytes {
                return Err(format!("{label} changed while it was being read"));
            }

            Ok(bytes)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_runtime::DeveloperImageAssetLoader;

    #[tokio::test]
    async fn selected_files_are_absolute_regular_and_size_bounded() {
        let loader = TokioDeveloperImageAssets;
        assert!(
            loader
                .load(&PathBuf::from("relative.dmg"), "image", 10)
                .await
                .is_err()
        );
        let path = std::env::temp_dir().join(format!(
            "devicehub-mask-developer-image-{}",
            uuid::Uuid::new_v4().simple()
        ));
        tokio::fs::write(&path, b"image").await.unwrap();
        assert_eq!(loader.load(&path, "image", 5).await.unwrap(), b"image");
        assert!(loader.load(&path, "image", 4).await.is_err());
        tokio::fs::remove_file(path).await.unwrap();
    }
}
