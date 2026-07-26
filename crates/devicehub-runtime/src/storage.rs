//! Host filesystem port used by streaming device storage services.

use std::future::Future;
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};

mod application;
mod public;

pub use application::{
    AppDocumentActivityKind, AppDocumentActivitySlot, AppDocumentActivityState,
    AppDocumentActivityView, AppDocumentCommand, AppDocumentEntry, AppDocumentKind,
    AppDocumentList, AppDocumentTransfer, AppStorageScope, AppStorageTransport,
    TRANSFER_CANCELLED as APP_DOCUMENT_TRANSFER_CANCELLED,
    is_transfer_cancelled as is_app_document_transfer_cancelled, serve as serve_app_documents,
};
pub use public::{
    DeviceFileActivityKind, DeviceFileActivitySlot, DeviceFileActivityState,
    DeviceFileActivityView, DeviceFileCommand, DeviceFileEntry, DeviceFileKind, DeviceFileList,
    DeviceFileTransfer, DeviceFileTransport, TRANSFER_CANCELLED, is_transfer_cancelled,
    serve as serve_device_files,
};

pub type HostFileFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;
pub type HostFileReader = Box<dyn AsyncRead + Unpin + Send>;
pub type HostFileWriter = Box<dyn HostFileWrite>;

/// Host file writer that can durably synchronize and close itself before an
/// atomic rename. Consuming `self` prevents Windows hosts from publishing a
/// temporary file while an open handle still exists.
pub trait HostFileWrite: AsyncWrite + Unpin + Send {
    fn sync(self: Box<Self>) -> HostFileFuture<'static, ()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFileMetadata {
    pub kind: HostFileKind,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostDirectoryEntry<Path> {
    pub name: String,
    pub path: Path,
    pub metadata: HostFileMetadata,
}

/// Filesystem operations required by device transfers.
///
/// Hosts retain path interpretation and local persistence policy. Runtime
/// services only consume opaque paths and asynchronous streams.
pub trait HostFileIo: Clone + Send + Sync + 'static {
    type Path: Clone + Send + Sync + 'static;

    fn validate_export_file<'a>(&'a self, destination: &'a Self::Path) -> HostFileFuture<'a, ()>;

    fn validate_new_export_directory<'a>(
        &'a self,
        destination: &'a Self::Path,
    ) -> HostFileFuture<'a, ()>;

    fn temporary_sibling(
        &self,
        destination: &Self::Path,
        operation: &str,
    ) -> Result<Self::Path, String>;

    fn child(&self, directory: &Self::Path, name: &str) -> Result<Self::Path, String>;

    fn file_name(&self, path: &Self::Path) -> Result<String, String>;

    fn metadata<'a>(&'a self, path: &'a Self::Path) -> HostFileFuture<'a, HostFileMetadata>;

    fn canonicalize<'a>(&'a self, path: &'a Self::Path) -> HostFileFuture<'a, Self::Path>;

    fn read_directory<'a>(
        &'a self,
        path: &'a Self::Path,
    ) -> HostFileFuture<'a, Vec<HostDirectoryEntry<Self::Path>>>;

    fn open_reader<'a>(&'a self, path: &'a Self::Path) -> HostFileFuture<'a, HostFileReader>;

    fn create_writer<'a>(&'a self, path: &'a Self::Path) -> HostFileFuture<'a, HostFileWriter>;

    fn create_new_writer<'a>(&'a self, path: &'a Self::Path) -> HostFileFuture<'a, HostFileWriter>;

    fn create_directory<'a>(&'a self, path: &'a Self::Path) -> HostFileFuture<'a, ()>;

    fn replace_file<'a>(
        &'a self,
        temporary: &'a Self::Path,
        destination: &'a Self::Path,
    ) -> HostFileFuture<'a, ()>;

    fn rename<'a>(
        &'a self,
        source: &'a Self::Path,
        destination: &'a Self::Path,
    ) -> HostFileFuture<'a, ()>;

    fn remove_file<'a>(&'a self, path: &'a Self::Path) -> HostFileFuture<'a, ()>;

    fn remove_tree<'a>(&'a self, path: &'a Self::Path) -> HostFileFuture<'a, ()>;
}
