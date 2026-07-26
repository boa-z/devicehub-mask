use std::sync::Arc;

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
