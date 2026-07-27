//! Bounded, isolated staging for browser-to-device file transfers.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use devicehub_server::http::{BrowserTransferFuture, BrowserTransferStore};

const MAX_BROWSER_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone)]
pub struct TokioBrowserTransferStore {
    root: PathBuf,
}

impl TokioBrowserTransferStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    async fn create_destination(root: PathBuf, name: String) -> Result<PathBuf, String> {
        let directory = root.join(uuid::Uuid::new_v4().simple().to_string());
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| format!("cannot create browser transfer staging: {error}"))?;
        Ok(directory.join(name))
    }

    async fn cleanup(path: &Path) -> Result<(), String> {
        let Some(directory) = path.parent() else {
            return Err("browser transfer path has no staging directory".into());
        };
        match tokio::fs::remove_dir_all(directory).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("cannot clean browser transfer staging: {error}")),
        }
    }
}

impl BrowserTransferStore for TokioBrowserTransferStore {
    fn stage_upload(&self, name: String, bytes: Bytes) -> BrowserTransferFuture<PathBuf> {
        let root = self.root.clone();
        Box::pin(async move {
            let destination = Self::create_destination(root, name).await?;
            if let Err(error) = tokio::fs::write(&destination, bytes).await {
                let _ = Self::cleanup(&destination).await;
                return Err(format!("cannot stage browser upload: {error}"));
            }
            Ok(destination)
        })
    }

    fn prepare_download(&self, name: String) -> BrowserTransferFuture<PathBuf> {
        let root = self.root.clone();
        Box::pin(async move { Self::create_destination(root, name).await })
    }

    fn read_and_remove(&self, path: PathBuf) -> BrowserTransferFuture<Bytes> {
        Box::pin(async move {
            let result = async {
                let metadata = tokio::fs::metadata(&path)
                    .await
                    .map_err(|error| format!("cannot inspect browser download: {error}"))?;
                if !metadata.is_file() || metadata.len() > MAX_BROWSER_DOWNLOAD_BYTES {
                    return Err(
                        "browser download must be one regular file no larger than 256 MiB".into(),
                    );
                }
                tokio::fs::read(&path)
                    .await
                    .map(Bytes::from)
                    .map_err(|error| format!("cannot read browser download: {error}"))
            }
            .await;
            let _ = Self::cleanup(&path).await;
            result
        })
    }

    fn remove(&self, path: PathBuf) -> BrowserTransferFuture<()> {
        Box::pin(async move { Self::cleanup(&path).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn staged_upload_is_isolated_and_removed() {
        let root = std::env::temp_dir().join(format!(
            "devicehub-browser-transfer-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = TokioBrowserTransferStore::new(root.clone());
        let path = store
            .stage_upload("sample.txt".into(), Bytes::from_static(b"sample"))
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"sample");
        store.remove(path).await.unwrap();
        let mut entries = tokio::fs::read_dir(&root).await.unwrap();
        assert!(entries.next_entry().await.unwrap().is_none());
        tokio::fs::remove_dir(root).await.unwrap();
    }
}
