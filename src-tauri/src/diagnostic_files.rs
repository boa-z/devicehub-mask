//! Desktop path policy for long-running diagnostic exports.

use std::path::{Path, PathBuf};

const MAX_PATH_BYTES: usize = 4_096;

pub(crate) async fn prepare_destination(
    destination: &Path,
    label: &str,
) -> Result<PathBuf, String> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err(format!("{label} destination must be an absolute file path"));
    }
    if destination.to_string_lossy().len() > MAX_PATH_BYTES {
        return Err(format!("{label} destination path is too long"));
    }
    match tokio::fs::symlink_metadata(destination).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!("{label} destination cannot be a symbolic link"));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("{label} destination must be a regular file"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("unable to inspect {label} destination: {error}"));
        }
    }
    let parent = destination
        .parent()
        .ok_or_else(|| format!("{label} destination has no parent directory"))?;
    let parent = tokio::fs::canonicalize(parent)
        .await
        .map_err(|error| format!("{label} destination is unavailable: {error}"))?;
    if !tokio::fs::metadata(&parent)
        .await
        .map_err(|error| format!("{label} destination is unavailable: {error}"))?
        .is_dir()
    {
        return Err(format!("{label} destination parent is not a directory"));
    }
    Ok(parent.join(destination.file_name().expect("file name checked above")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn diagnostic_destination_is_an_absolute_regular_file() {
        assert!(
            prepare_destination(Path::new("relative.tar"), "log archive")
                .await
                .is_err()
        );
        assert!(
            prepare_destination(&std::env::temp_dir(), "sysdiagnose")
                .await
                .is_err()
        );
        let destination = std::env::temp_dir().join(format!(
            "devicehub-mask-diagnostic-{}.tar",
            uuid::Uuid::new_v4()
        ));
        let expected = tokio::fs::canonicalize(std::env::temp_dir())
            .await
            .unwrap()
            .join(destination.file_name().unwrap());
        assert_eq!(
            prepare_destination(&destination, "log archive")
                .await
                .unwrap(),
            expected
        );
    }
}
