//! Tokio filesystem implementation of the runtime streaming storage port.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use devicehub_runtime::{
    HostDirectoryEntry, HostFileFuture, HostFileIo, HostFileKind, HostFileMetadata, HostFileReader,
    HostFileWrite, HostFileWriter,
};
use tokio::io::AsyncWrite;

#[derive(Debug, Clone, Copy, Default)]
pub struct TokioHostFileIo;

struct TokioHostFileWriter(tokio::fs::File);

impl AsyncWrite for TokioHostFileWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(context, bytes)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(context)
    }
}

impl HostFileWrite for TokioHostFileWriter {
    fn sync(self: Box<Self>) -> HostFileFuture<'static, ()> {
        Box::pin(async move {
            self.0
                .sync_data()
                .await
                .map_err(|error| format!("unable to synchronize host file: {error}"))
        })
    }
}

impl HostFileIo for TokioHostFileIo {
    type Path = PathBuf;

    fn validate_export_file<'a>(&'a self, destination: &'a PathBuf) -> HostFileFuture<'a, ()> {
        Box::pin(validate_export_destination(destination))
    }

    fn validate_new_export_directory<'a>(
        &'a self,
        destination: &'a PathBuf,
    ) -> HostFileFuture<'a, ()> {
        Box::pin(validate_new_directory_destination(destination))
    }

    fn temporary_sibling(&self, destination: &PathBuf, operation: &str) -> Result<PathBuf, String> {
        temporary_sibling(destination, operation)
    }

    fn child(&self, directory: &PathBuf, name: &str) -> Result<PathBuf, String> {
        if name.is_empty() || name.contains(['/', '\\', '\0']) {
            return Err("host file name is invalid".into());
        }
        Ok(directory.join(name))
    }

    fn file_name(&self, path: &PathBuf) -> Result<String, String> {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| "host path has an unsupported file name".to_string())
    }

    fn metadata<'a>(&'a self, path: &'a PathBuf) -> HostFileFuture<'a, HostFileMetadata> {
        Box::pin(async move {
            let metadata = tokio::fs::symlink_metadata(path)
                .await
                .map_err(|error| format!("unable to inspect host path: {error}"))?;
            Ok(metadata_from_std(&metadata))
        })
    }

    fn canonicalize<'a>(&'a self, path: &'a PathBuf) -> HostFileFuture<'a, PathBuf> {
        Box::pin(async move {
            tokio::fs::canonicalize(path)
                .await
                .map_err(|error| format!("host path is unavailable: {error}"))
        })
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a PathBuf,
    ) -> HostFileFuture<'a, Vec<HostDirectoryEntry<PathBuf>>> {
        Box::pin(async move {
            let mut directory = tokio::fs::read_dir(path)
                .await
                .map_err(|error| format!("unable to read host directory: {error}"))?;
            let mut entries = Vec::new();
            while let Some(entry) = directory
                .next_entry()
                .await
                .map_err(|error| format!("unable to read host directory: {error}"))?
            {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "host directory contains an unsupported file name".to_string())?;
                let metadata = tokio::fs::symlink_metadata(entry.path())
                    .await
                    .map_err(|error| format!("unable to inspect host directory entry: {error}"))?;
                entries.push(HostDirectoryEntry {
                    name,
                    path: entry.path(),
                    metadata: metadata_from_std(&metadata),
                });
            }
            Ok(entries)
        })
    }

    fn open_reader<'a>(&'a self, path: &'a PathBuf) -> HostFileFuture<'a, HostFileReader> {
        Box::pin(async move {
            let file = tokio::fs::File::open(path)
                .await
                .map_err(|error| format!("unable to open host file: {error}"))?;
            Ok(Box::new(file) as HostFileReader)
        })
    }

    fn create_writer<'a>(&'a self, path: &'a PathBuf) -> HostFileFuture<'a, HostFileWriter> {
        Box::pin(async move {
            let file = tokio::fs::File::create(path)
                .await
                .map_err(|error| format!("unable to create host file: {error}"))?;
            Ok(Box::new(TokioHostFileWriter(file)) as HostFileWriter)
        })
    }

    fn create_new_writer<'a>(&'a self, path: &'a PathBuf) -> HostFileFuture<'a, HostFileWriter> {
        Box::pin(async move {
            let file = tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(path)
                .await
                .map_err(|error| format!("unable to create new host file: {error}"))?;
            Ok(Box::new(TokioHostFileWriter(file)) as HostFileWriter)
        })
    }

    fn create_directory<'a>(&'a self, path: &'a PathBuf) -> HostFileFuture<'a, ()> {
        Box::pin(async move {
            tokio::fs::create_dir(path)
                .await
                .map_err(|error| format!("unable to create host directory: {error}"))
        })
    }

    fn replace_file<'a>(
        &'a self,
        temporary: &'a PathBuf,
        destination: &'a PathBuf,
    ) -> HostFileFuture<'a, ()> {
        Box::pin(replace_local_file(temporary, destination))
    }

    fn rename<'a>(
        &'a self,
        source: &'a PathBuf,
        destination: &'a PathBuf,
    ) -> HostFileFuture<'a, ()> {
        Box::pin(async move {
            tokio::fs::rename(source, destination)
                .await
                .map_err(|error| format!("unable to commit host path: {error}"))
        })
    }

    fn remove_file<'a>(&'a self, path: &'a PathBuf) -> HostFileFuture<'a, ()> {
        Box::pin(async move {
            tokio::fs::remove_file(path)
                .await
                .map_err(|error| format!("unable to remove host file: {error}"))
        })
    }

    fn remove_tree<'a>(&'a self, path: &'a PathBuf) -> HostFileFuture<'a, ()> {
        Box::pin(async move {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|error| format!("unable to remove host directory: {error}"))
        })
    }
}

fn metadata_from_std(metadata: &std::fs::Metadata) -> HostFileMetadata {
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        HostFileKind::Symlink
    } else if metadata.is_file() {
        HostFileKind::File
    } else if metadata.is_dir() {
        HostFileKind::Directory
    } else {
        HostFileKind::Other
    };
    HostFileMetadata {
        kind,
        len: metadata.len(),
    }
}

async fn validate_export_destination(destination: &Path) -> Result<(), String> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err("export destination must be an absolute file path".into());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "export destination has no parent directory".to_string())?;
    let metadata = tokio::fs::metadata(parent)
        .await
        .map_err(|error| format!("export destination is unavailable: {error}"))?;
    if !metadata.is_dir() {
        return Err("export destination parent is not a directory".into());
    }
    Ok(())
}

async fn validate_new_directory_destination(destination: &Path) -> Result<(), String> {
    validate_export_destination(destination).await?;
    match tokio::fs::symlink_metadata(destination).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("directory export destination already exists".into()),
        Err(error) => Err(format!(
            "unable to inspect directory export destination: {error}"
        )),
    }
}

pub fn temporary_sibling(destination: &Path, operation: &str) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "destination has no parent directory".to_string())?;
    Ok(parent.join(format!(
        ".devicehub-{operation}-{}-{}.part",
        std::process::id(),
        uuid::Uuid::new_v4()
    )))
}

pub async fn replace_local_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    let backup = temporary_sibling(destination, "backup")?;
    let had_destination = match tokio::fs::metadata(destination).await {
        Ok(metadata) if metadata.is_file() => {
            tokio::fs::rename(destination, &backup)
                .await
                .map_err(|error| format!("unable to preserve existing export file: {error}"))?;
            true
        }
        Ok(_) => return Err("export destination is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("unable to inspect export destination: {error}")),
    };
    match tokio::fs::rename(temporary, destination).await {
        Ok(()) => {
            if had_destination {
                let _ = tokio::fs::remove_file(backup).await;
            }
            Ok(())
        }
        Err(error) => {
            if had_destination {
                let _ = tokio::fs::rename(&backup, destination).await;
            }
            Err(format!("unable to finish export file: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "devicehub-mask-host-files-{test_name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[tokio::test]
    async fn export_destination_requires_an_absolute_file_path() {
        let file_io = TokioHostFileIo;
        assert!(
            file_io
                .validate_export_file(&PathBuf::from("photo.heic"))
                .await
                .is_err()
        );
        assert!(
            file_io
                .validate_export_file(&PathBuf::from("/"))
                .await
                .is_err()
        );

        let directory = temporary_directory("destination");
        tokio::fs::create_dir(&directory).await.unwrap();
        file_io
            .validate_export_file(&directory.join("photo.heic"))
            .await
            .unwrap();
        tokio::fs::remove_dir(directory).await.unwrap();
    }

    #[tokio::test]
    async fn directory_export_requires_a_new_destination() {
        let file_io = TokioHostFileIo;
        let directory = temporary_directory("new-directory");
        tokio::fs::create_dir(&directory).await.unwrap();
        let destination = directory.join("DCIM");

        file_io
            .validate_new_export_directory(&destination)
            .await
            .unwrap();
        tokio::fs::create_dir(&destination).await.unwrap();
        assert!(
            file_io
                .validate_new_export_directory(&destination)
                .await
                .is_err()
        );
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn replacing_an_export_preserves_the_new_contents() {
        let file_io = TokioHostFileIo;
        let directory = temporary_directory("replace");
        tokio::fs::create_dir(&directory).await.unwrap();
        let destination = directory.join("photo.heic");
        let temporary = file_io
            .temporary_sibling(&destination, "device-export")
            .unwrap();
        tokio::fs::write(&destination, b"old").await.unwrap();
        tokio::fs::write(&temporary, b"new").await.unwrap();

        file_io
            .replace_file(&temporary, &destination)
            .await
            .unwrap();

        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"new");
        assert!(!temporary.exists());
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn new_writer_synchronizes_and_closes_before_replace() {
        use tokio::io::AsyncWriteExt;

        let file_io = TokioHostFileIo;
        let directory = temporary_directory("sync");
        tokio::fs::create_dir(&directory).await.unwrap();
        let destination = directory.join("archive.tar");
        let temporary = file_io.temporary_sibling(&destination, "archive").unwrap();
        let mut writer = file_io.create_new_writer(&temporary).await.unwrap();
        writer.write_all(b"archive").await.unwrap();
        writer.flush().await.unwrap();
        writer.sync().await.unwrap();
        file_io
            .replace_file(&temporary, &destination)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"archive");
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
