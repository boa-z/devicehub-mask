//! Device-scoped ownership for browser input transports.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

#[derive(Clone, Default)]
pub struct BrowserControlLeases {
    inner: Arc<ControlLeaseInner>,
}

struct ControlLeaseInner {
    owners: Mutex<HashMap<String, u64>>,
    next_owner: AtomicU64,
    released: broadcast::Sender<String>,
}

impl Default for ControlLeaseInner {
    fn default() -> Self {
        Self {
            owners: Mutex::new(HashMap::new()),
            next_owner: AtomicU64::new(0),
            released: broadcast::channel(32).0,
        }
    }
}

impl BrowserControlLeases {
    pub fn try_acquire(&self, selection_id: &str) -> Option<BrowserControlLease> {
        let owner = self
            .inner
            .next_owner
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let mut owners = self.inner.owners.lock().unwrap();
        if owners.contains_key(selection_id) {
            return None;
        }
        owners.insert(selection_id.to_string(), owner);
        Some(BrowserControlLease {
            inner: self.inner.clone(),
            selection_id: selection_id.to_string(),
            owner,
        })
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<String> {
        self.inner.released.subscribe()
    }
}

pub struct BrowserControlLease {
    inner: Arc<ControlLeaseInner>,
    selection_id: String,
    owner: u64,
}

impl Drop for BrowserControlLease {
    fn drop(&mut self) {
        let mut owners = self.inner.owners.lock().unwrap();
        if owners.get(&self.selection_id) == Some(&self.owner) {
            owners.remove(&self.selection_id);
            let _ = self.inner.released.send(self.selection_id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_client_controls_each_device_while_other_devices_remain_independent() {
        let leases = BrowserControlLeases::default();
        let phone = leases.try_acquire("phone::usb").expect("phone lease");
        assert!(leases.try_acquire("phone::usb").is_none());
        let tablet = leases.try_acquire("tablet::wifi").expect("tablet lease");

        drop(phone);
        assert!(leases.try_acquire("phone::usb").is_some());
        assert!(leases.try_acquire("tablet::wifi").is_none());
        drop(tablet);
    }

    #[test]
    fn dropping_an_old_guard_cannot_release_a_new_owner() {
        let leases = BrowserControlLeases::default();
        let old = leases.try_acquire("phone::usb").expect("old lease");
        {
            let mut owners = leases.inner.owners.lock().unwrap();
            owners.insert("phone::usb".into(), old.owner.wrapping_add(1));
        }

        drop(old);
        assert!(leases.try_acquire("phone::usb").is_none());
    }

    #[tokio::test]
    async fn release_notifies_waiters_for_the_exact_device() {
        let leases = BrowserControlLeases::default();
        let mut released = leases.subscribe();
        let phone = leases.try_acquire("phone::usb").expect("phone lease");

        drop(phone);

        assert_eq!(released.recv().await.unwrap(), "phone::usb");
        assert!(leases.try_acquire("phone::usb").is_some());
    }
}
