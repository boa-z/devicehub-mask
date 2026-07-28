use std::sync::{Arc, Mutex};

use tokio::sync::watch;

#[derive(Default)]
struct DemandState {
    explicit: bool,
    leases: usize,
}

struct DemandInner {
    state: Mutex<DemandState>,
    sender: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct Demand(Arc<DemandInner>);

impl Default for Demand {
    fn default() -> Self {
        let (sender, _) = watch::channel(false);
        Self(Arc::new(DemandInner {
            state: Mutex::new(DemandState::default()),
            sender,
        }))
    }
}

impl Demand {
    pub fn set(&self, enabled: bool) {
        let mut state = self.0.state.lock().unwrap();
        state.explicit = enabled;
        self.0
            .sender
            .send_replace(state.explicit || state.leases > 0);
    }

    pub fn enabled(&self) -> bool {
        *self.0.sender.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.sender.subscribe()
    }

    pub fn acquire(&self) -> DemandLease {
        let mut state = self.0.state.lock().unwrap();
        state.leases = state.leases.saturating_add(1);
        self.0.sender.send_replace(true);
        DemandLease(Some(self.0.clone()))
    }
}

pub struct DemandLease(Option<Arc<DemandInner>>);

impl Drop for DemandLease {
    fn drop(&mut self) {
        let Some(inner) = self.0.take() else {
            return;
        };
        let mut state = inner.state.lock().unwrap();
        state.leases = state.leases.saturating_sub(1);
        inner
            .sender
            .send_replace(state.explicit || state.leases > 0);
    }
}

/// Consumer-driven media resources owned by one device session.
///
/// Video and audio are independent because a status/control client may request
/// neither, either, or both. Leases compose across browser clients and release
/// automatically when their transport closes.
#[derive(Clone, Default)]
pub struct SessionMediaDemand {
    pub video: Demand,
    pub audio: Demand,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_compose_with_explicit_demand() {
        let demand = Demand::default();
        let first = demand.acquire();
        let second = demand.acquire();
        assert!(demand.enabled());
        drop(first);
        assert!(demand.enabled());
        demand.set(true);
        drop(second);
        assert!(demand.enabled());
        demand.set(false);
        assert!(!demand.enabled());
    }

    #[test]
    fn session_media_demand_keeps_resources_and_devices_independent() {
        let phone = SessionMediaDemand::default();
        let tablet = SessionMediaDemand::default();
        let first_video = phone.video.acquire();
        let second_video = phone.video.acquire();
        let audio = phone.audio.acquire();

        assert!(phone.video.enabled());
        assert!(phone.audio.enabled());
        assert!(!tablet.video.enabled());
        assert!(!tablet.audio.enabled());

        drop(first_video);
        assert!(phone.video.enabled());
        drop(second_video);
        drop(audio);
        assert!(!phone.video.enabled());
        assert!(!phone.audio.enabled());
    }
}
