//! Host output ports shared by capture services.

use std::future::Future;
use std::pin::Pin;

pub type CaptureFileFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFileKind {
    Network,
    Bluetooth,
}

/// One in-progress host-owned capture file.
pub trait CaptureFileWriter: Send + 'static {
    fn bytes_written(&self) -> u64;

    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> CaptureFileFuture<'a, ()>;

    /// Flushes, synchronizes, and atomically publishes the capture.
    fn finish(self) -> CaptureFileFuture<'static, u64>;
}

/// Host capability for validating and opening capture destinations.
pub trait CaptureFileIo: Clone + Send + Sync + 'static {
    type Destination: Send + Sync + 'static;
    type Writer: CaptureFileWriter;

    fn validate<'a>(
        &'a self,
        destination: &'a Self::Destination,
        kind: CaptureFileKind,
    ) -> CaptureFileFuture<'a, ()>;

    fn create<'a>(
        &'a self,
        destination: Self::Destination,
        kind: CaptureFileKind,
        header: &'static [u8],
    ) -> CaptureFileFuture<'a, Self::Writer>;
}
