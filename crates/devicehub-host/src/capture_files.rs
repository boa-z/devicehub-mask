//! Atomic local-file output for runtime-owned capture services.

use std::path::{Path, PathBuf};

use devicehub_runtime::{CaptureFileFuture, CaptureFileIo, CaptureFileKind, CaptureFileWriter};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, Default)]
pub struct TokioCaptureFileIo;

/// Adapts native-host filesystem policy to server-side request validation without
/// exposing filesystem APIs to `devicehub-server`.
pub async fn validate_http_destination(
    destination: PathBuf,
    kind: CaptureFileKind,
) -> Result<(), String> {
    TokioCaptureFileIo.validate(&destination, kind).await
}

pub struct TokioCaptureFileWriter {
    file: tokio::fs::File,
    temporary: PathBuf,
    destination: PathBuf,
    kind: CaptureFileKind,
    bytes_written: u64,
}

impl CaptureFileIo for TokioCaptureFileIo {
    type Destination = PathBuf;
    type Writer = TokioCaptureFileWriter;

    fn validate<'a>(
        &'a self,
        destination: &'a PathBuf,
        kind: CaptureFileKind,
    ) -> CaptureFileFuture<'a, ()> {
        Box::pin(validate_destination(destination, kind))
    }

    fn create<'a>(
        &'a self,
        destination: PathBuf,
        kind: CaptureFileKind,
        header: &'static [u8],
    ) -> CaptureFileFuture<'a, Self::Writer> {
        Box::pin(async move {
            validate_destination(&destination, kind).await?;
            let temporary = temporary_sibling(&destination, kind)?;
            let mut file = tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .await
                .map_err(|error| format!("{}: {error}", kind.create_error()))?;
            if let Err(error) = file.write_all(header).await {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(format!("{}: {error}", kind.header_error()));
            }
            Ok(TokioCaptureFileWriter {
                file,
                temporary,
                destination,
                kind,
                bytes_written: header.len() as u64,
            })
        })
    }
}

impl CaptureFileWriter for TokioCaptureFileWriter {
    fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> CaptureFileFuture<'a, ()> {
        Box::pin(async move {
            self.file
                .write_all(bytes)
                .await
                .map_err(|error| format!("{}: {error}", self.kind.write_error()))?;
            self.bytes_written = self.bytes_written.saturating_add(bytes.len() as u64);
            Ok(())
        })
    }

    fn finish(mut self) -> CaptureFileFuture<'static, u64> {
        Box::pin(async move {
            let result = async {
                self.file
                    .flush()
                    .await
                    .map_err(|error| format!("{}: {error}", self.kind.flush_error()))?;
                self.file
                    .sync_data()
                    .await
                    .map_err(|error| format!("{}: {error}", self.kind.sync_error()))?;
                drop(self.file);
                replace_local_file(&self.temporary, &self.destination, self.kind).await?;
                Ok(self.bytes_written)
            }
            .await;
            if result.is_err() {
                let _ = tokio::fs::remove_file(&self.temporary).await;
            }
            result
        })
    }
}

async fn validate_destination(path: &Path, kind: CaptureFileKind) -> Result<(), String> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pcap"))
    {
        return Err(format!(
            "{} destination must be an absolute .pcap path",
            kind.label()
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} destination has no parent directory", kind.label()))?;
    let metadata = tokio::fs::metadata(parent)
        .await
        .map_err(|error| format!("unable to access {} directory: {error}", kind.label()))?;
    if !metadata.is_dir() {
        return Err(format!("{} parent is not a directory", kind.label()));
    }
    match tokio::fs::metadata(path).await {
        Ok(metadata) if !metadata.is_file() => Err(format!(
            "{} destination is not a regular file",
            kind.label()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "unable to inspect {} destination: {error}",
            kind.label()
        )),
    }
}

fn temporary_sibling(destination: &Path, kind: CaptureFileKind) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("{} destination has no parent directory", kind.label()))?;
    Ok(parent.join(format!(
        ".{}-{}-{}.part",
        kind.temporary_prefix(),
        std::process::id(),
        uuid::Uuid::new_v4()
    )))
}

async fn replace_local_file(
    temporary: &Path,
    destination: &Path,
    kind: CaptureFileKind,
) -> Result<(), String> {
    let backup = destination.with_file_name(format!(
        ".{}-{}-{}.part",
        kind.backup_prefix(),
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let had_destination = match tokio::fs::metadata(destination).await {
        Ok(metadata) if metadata.is_file() => {
            tokio::fs::rename(destination, &backup)
                .await
                .map_err(|error| format!("{}: {error}", kind.preserve_error()))?;
            true
        }
        Ok(_) => {
            return Err(format!(
                "{} destination is not a regular file",
                kind.label()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "unable to inspect {} destination: {error}",
                kind.label()
            ));
        }
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
            Err(format!("{}: {error}", kind.finish_error()))
        }
    }
}

trait CaptureFileKindLabels {
    fn label(self) -> &'static str;
    fn temporary_prefix(self) -> &'static str;
    fn backup_prefix(self) -> &'static str;
    fn create_error(self) -> &'static str;
    fn header_error(self) -> &'static str;
    fn write_error(self) -> &'static str;
    fn flush_error(self) -> &'static str;
    fn sync_error(self) -> &'static str;
    fn preserve_error(self) -> &'static str;
    fn finish_error(self) -> &'static str;
}

impl CaptureFileKindLabels for CaptureFileKind {
    fn label(self) -> &'static str {
        match self {
            Self::Network => "packet capture",
            Self::Bluetooth => "Bluetooth capture",
        }
    }

    fn temporary_prefix(self) -> &'static str {
        match self {
            Self::Network => "devicehub-capture",
            Self::Bluetooth => "devicehub-bluetooth",
        }
    }

    fn backup_prefix(self) -> &'static str {
        match self {
            Self::Network => "devicehub-capture-backup",
            Self::Bluetooth => "devicehub-bluetooth-backup",
        }
    }

    fn create_error(self) -> &'static str {
        match self {
            Self::Network => "unable to create packet capture file",
            Self::Bluetooth => "unable to create Bluetooth capture file",
        }
    }

    fn header_error(self) -> &'static str {
        match self {
            Self::Network => "unable to write packet capture header",
            Self::Bluetooth => "unable to write Bluetooth capture header",
        }
    }

    fn write_error(self) -> &'static str {
        match self {
            Self::Network => "unable to write packet capture data",
            Self::Bluetooth => "unable to write Bluetooth capture data",
        }
    }

    fn flush_error(self) -> &'static str {
        match self {
            Self::Network => "unable to flush packet capture",
            Self::Bluetooth => "unable to flush Bluetooth capture",
        }
    }

    fn sync_error(self) -> &'static str {
        match self {
            Self::Network => "unable to synchronize packet capture",
            Self::Bluetooth => "unable to synchronize Bluetooth capture",
        }
    }

    fn preserve_error(self) -> &'static str {
        match self {
            Self::Network => "unable to preserve existing capture file",
            Self::Bluetooth => "unable to preserve existing Bluetooth capture",
        }
    }

    fn finish_error(self) -> &'static str {
        match self {
            Self::Network => "unable to finish packet capture",
            Self::Bluetooth => "unable to finish Bluetooth capture",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn destinations_are_absolute_pcap_files_in_existing_directories() {
        let files = TokioCaptureFileIo;
        assert!(
            files
                .validate(&PathBuf::from("relative.pcap"), CaptureFileKind::Network)
                .await
                .is_err()
        );
        let destination =
            std::env::temp_dir().join(format!("devicehub-mask-{}.pcap", uuid::Uuid::new_v4()));
        assert!(
            files
                .validate(
                    &destination.with_extension("txt"),
                    CaptureFileKind::Bluetooth
                )
                .await
                .is_err()
        );
        assert!(
            files
                .validate(&destination, CaptureFileKind::Network)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn writer_replaces_existing_file_with_complete_capture() {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-capture-test-{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir(&directory).await.unwrap();
        let destination = directory.join("capture.pcap");
        tokio::fs::write(&destination, b"old").await.unwrap();
        let mut writer = TokioCaptureFileIo
            .create(destination.clone(), CaptureFileKind::Network, b"header")
            .await
            .unwrap();
        writer.write(b"record").await.unwrap();
        assert_eq!(writer.finish().await.unwrap(), 12);
        assert_eq!(
            tokio::fs::read(&destination).await.unwrap(),
            b"headerrecord"
        );
        tokio::fs::remove_file(destination).await.unwrap();
        tokio::fs::remove_dir(directory).await.unwrap();
    }
}
