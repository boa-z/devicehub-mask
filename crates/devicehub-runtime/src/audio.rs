use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use idevice::tcp::handle::UdpSocketHandle;

pub type DeviceAudioFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Runtime-owned negotiated audio source exposed through transport-neutral
/// operations. Hosts can decode or discard audio without receiving an Apple
/// transport handle.
pub struct DeviceAudioSource {
    udp: UdpSocketHandle,
}

impl DeviceAudioSource {
    pub(crate) fn new(udp: UdpSocketHandle) -> Self {
        Self { udp }
    }

    /// Drain and validate audio RTP until the device transport ends.
    pub async fn drain(&self) {
        crate::media::receive_audio_rtp(&self.udp, None).await;
    }

    /// Forward validated, RFC 3640-packetized audio RTP to a local host decoder.
    pub async fn forward_rtp_to_local_port(&self, port: u16) {
        let sender = match tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await {
            Ok(sender) => sender,
            Err(error) => {
                tracing::warn!(%error, "cannot bind audio RTP forwarding socket");
                self.drain().await;
                return;
            }
        };
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        crate::media::receive_audio_rtp(&self.udp, Some((&sender, target))).await;
    }

    /// Keep the negotiated stream drained during a bounded decoder restart.
    /// Returns false when the device transport ends before the delay expires.
    pub async fn drain_for(&self, delay: Duration) -> bool {
        let retry = tokio::time::sleep(delay);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                _ = &mut retry => return true,
                packet = self.udp.recv() => {
                    if let Err(error) = packet {
                        tracing::warn!(?error, "audio UDP receive failed while restarting decoder");
                        return false;
                    }
                }
            }
        }
    }
}

/// Host-selected audio pipeline for a negotiated device RTP stream.
pub trait DeviceAudioPipeline: Clone + Send + Sync + 'static {
    fn run(&self, source: DeviceAudioSource) -> DeviceAudioFuture;
}

/// Creates one audio pipeline from the latest runtime preference snapshot.
pub trait DeviceAudioPipelineFactory: Clone + Send + Sync + 'static {
    type Pipeline: DeviceAudioPipeline;

    fn create(&self, enabled: bool) -> Self::Pipeline;
}

/// Host-provided sink for decoded interleaved PCM bytes.
pub trait PcmAudioConsumer: Send + Sync + 'static {
    fn publish(&self, pcm: bytes::Bytes);
}

/// Cloneable runtime handle that hides the concrete host audio implementation.
#[derive(Clone)]
pub struct AudioPublisher(Arc<dyn PcmAudioConsumer>);

impl AudioPublisher {
    pub fn new(consumer: impl PcmAudioConsumer) -> Self {
        Self(Arc::new(consumer))
    }

    pub fn publish(&self, pcm: bytes::Bytes) {
        self.0.publish(pcm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingConsumer(Arc<AtomicUsize>);

    impl PcmAudioConsumer for CountingConsumer {
        fn publish(&self, pcm: bytes::Bytes) {
            self.0.fetch_add(pcm.len(), Ordering::Relaxed);
        }
    }

    #[test]
    fn cloned_publishers_share_the_injected_consumer() {
        let published = Arc::new(AtomicUsize::new(0));
        let publisher = AudioPublisher::new(CountingConsumer(published.clone()));

        publisher.clone().publish(bytes::Bytes::from_static(b"pcm"));
        publisher.publish(bytes::Bytes::from_static(b"data"));

        assert_eq!(published.load(Ordering::Relaxed), 7);
    }
}
