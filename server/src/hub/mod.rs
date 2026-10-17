//! Registry of connected devices (spec 006).

pub mod dispatcher;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    Text(String),
    Close(u16, String),
}

struct Handle {
    tx: mpsc::UnboundedSender<Outgoing>,
    notify: Arc<Notify>,
    generation: u64,
    dispatcher: Option<JoinHandle<()>>,
}

impl Handle {
    fn shut(self, code: u16, reason: &str) {
        if let Some(d) = self.dispatcher {
            d.abort();
        }
        let _ = self.tx.send(Outgoing::Close(code, reason.to_string()));
    }
}

#[derive(Default)]
pub struct Hub {
    devices: Mutex<HashMap<i64, Handle>>,
    next_gen: AtomicU64,
}

pub struct Registration {
    pub generation: u64,
    pub rx: mpsc::UnboundedReceiver<Outgoing>,
    pub tx: mpsc::UnboundedSender<Outgoing>,
    pub notify: Arc<Notify>,
}

pub const CLOSE_REPLACED: u16 = 4000;
pub const CLOSE_REVOKED: u16 = 4003;
/// The app closed the socket because the user switched the gateway off (spec 002).
pub const CLOSE_GATEWAY_OFF: u16 = 4010;

impl Hub {
    /// Register a new socket. An existing one for the same device is closed with 4000.
    pub fn register(&self, device_id: i64) -> Registration {
        let generation = self.next_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::unbounded_channel();
        let notify = Arc::new(Notify::new());
        let old = self.devices.lock().unwrap().insert(
            device_id,
            Handle {
                tx: tx.clone(),
                notify: notify.clone(),
                generation,
                dispatcher: None,
            },
        );
        if let Some(old) = old {
            old.shut(CLOSE_REPLACED, "replaced");
        }
        Registration {
            generation,
            rx,
            tx,
            notify,
        }
    }

    /// Attach the dispatcher task; it is aborted if the generation is stale.
    pub fn set_dispatcher(&self, device_id: i64, generation: u64, task: JoinHandle<()>) {
        let mut map = self.devices.lock().unwrap();
        match map.get_mut(&device_id) {
            Some(h) if h.generation == generation => {
                if let Some(old) = h.dispatcher.replace(task) {
                    old.abort();
                }
            }
            _ => task.abort(),
        }
    }

    pub fn has_dispatcher(&self, device_id: i64, generation: u64) -> bool {
        self.devices
            .lock()
            .unwrap()
            .get(&device_id)
            .is_some_and(|h| h.generation == generation && h.dispatcher.is_some())
    }

    /// Remove the handle only if it still belongs to `generation`.
    pub fn unregister(&self, device_id: i64, generation: u64) -> bool {
        let mut map = self.devices.lock().unwrap();
        if map
            .get(&device_id)
            .is_some_and(|h| h.generation == generation)
        {
            if let Some(h) = map.remove(&device_id)
                && let Some(d) = h.dispatcher
            {
                d.abort();
            }
            true
        } else {
            false
        }
    }

    pub fn notify(&self, device_id: i64) {
        if let Some(h) = self.devices.lock().unwrap().get(&device_id) {
            h.notify.notify_one();
        }
    }

    pub fn send(&self, device_id: i64, frame: Outgoing) -> bool {
        self.devices
            .lock()
            .unwrap()
            .get(&device_id)
            .is_some_and(|h| h.tx.send(frame).is_ok())
    }

    pub fn is_connected(&self, device_id: i64) -> bool {
        self.devices.lock().unwrap().contains_key(&device_id)
    }

    /// Close and forget a device's socket (e.g. 4003 on removal).
    pub fn close(&self, device_id: i64, code: u16, reason: &str) {
        let h = self.devices.lock().unwrap().remove(&device_id);
        if let Some(h) = h {
            h.shut(code, reason);
        }
    }

    pub fn close_all(&self, code: u16, reason: &str) {
        let all: Vec<Handle> = self
            .devices
            .lock()
            .unwrap()
            .drain()
            .map(|(_, h)| h)
            .collect();
        for h in all {
            h.shut(code, reason);
        }
    }
}
