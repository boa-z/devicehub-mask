//! Host ports for optional connected-session diagnostic byte streams.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::RtcpOptions;

pub type DiagnosticDumpSinkFuture<'a> =
    Pin<Box<dyn Future<Output = Option<mpsc::Sender<Vec<u8>>>> + Send + 'a>>;

/// Opens bounded diagnostic sinks while keeping path interpretation in hosts.
pub trait DiagnosticDumpSinkFactory: Clone + Send + Sync + 'static {
    type Source: Clone + Send + Sync + 'static;

    fn open<'a>(
        &'a self,
        source: Option<Self::Source>,
        capacity: usize,
        label: &'static str,
    ) -> DiagnosticDumpSinkFuture<'a>;
}

/// Immutable diagnostic choices applied to one connected device session.
#[derive(Clone, Debug, Default)]
pub struct SessionDiagnostics<Source> {
    pub send_frame_ack: bool,
    pub rtcp: RtcpOptions,
    pub hevc_dump: Option<Source>,
    pub hid_dump: Option<Source>,
}
