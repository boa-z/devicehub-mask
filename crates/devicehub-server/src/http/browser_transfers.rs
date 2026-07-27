//! Host-injected staging used when a browser cannot supply native file paths.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use axum::body::Bytes;
use axum::http::header;
use axum::response::{IntoResponse, Response};

pub type BrowserTransferFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

pub trait BrowserTransferStore: Send + Sync + 'static {
    fn stage_upload(&self, name: String, bytes: Bytes) -> BrowserTransferFuture<PathBuf>;
    fn prepare_download(&self, name: String) -> BrowserTransferFuture<PathBuf>;
    fn read_and_remove(&self, path: PathBuf) -> BrowserTransferFuture<Bytes>;
    fn remove(&self, path: PathBuf) -> BrowserTransferFuture<()>;
}

pub(super) fn validate_file_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
    {
        return Err("invalid browser file name".into());
    }
    Ok(())
}

pub(super) fn binary_download(bytes: Bytes) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, "attachment"),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_file_names_cannot_introduce_paths() {
        assert!(validate_file_name("save.dat").is_ok());
        assert!(validate_file_name("../save.dat").is_err());
        assert!(validate_file_name("folder/save.dat").is_err());
        assert!(validate_file_name("").is_err());
    }
}
