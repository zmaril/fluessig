//! The hand-written engine behind the demo op surface: an in-memory key/value
//! `Store` implementing the generated `StoreCore` trait, plus a `Ticker`
//! implementing `TickerCore` (the callback + subscription slice). This is the ONE
//! module a consumer writes by hand (the house-style "generated surface +
//! hand-written `core_impl`" split); everything else in the crate is generated.
//!
//! The core is synchronous and holds its state behind a `Mutex` so the trait's
//! `&self` methods can mutate it while satisfying `Send + Sync`.
//!
//! straitjacket-allow-file:duplication — the per-language demo cores (this
//! `TickerImpl` and `crates/java-demo`'s) are DELIBERATELY parallel: each proves
//! the SAME callback/subscription contract round-trips for its language's binding
//! (C/C++ fn-ptr, JNI global-ref Consumer, …), so the register/tick bodies match
//! by design. This file sorts first in the clone pair, so the marker lives here.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail};

use crate::generated::{StoreCore, TickerCore};

/// An in-memory key/value store with a fixed capacity. A `BTreeMap` keeps `keys`
/// deterministically sorted.
pub struct StoreImpl {
    capacity: usize,
    map: Mutex<BTreeMap<String, String>>,
}

impl StoreCore for StoreImpl {
    fn open(capacity: i32) -> anyhow::Result<Self> {
        if capacity <= 0 {
            bail!("capacity must be positive (got {capacity})");
        }
        Ok(StoreImpl {
            capacity: capacity as usize,
            map: Mutex::new(BTreeMap::new()),
        })
    }

    fn put(&self, key: String, value: String) -> anyhow::Result<i32> {
        let mut map = self.map.lock().unwrap();
        if !map.contains_key(&key) && map.len() >= self.capacity {
            bail!("store is full (capacity {})", self.capacity);
        }
        map.insert(key, value);
        Ok(map.len() as i32)
    }

    fn get(&self, key: String) -> anyhow::Result<String> {
        let map = self.map.lock().unwrap();
        map.get(&key)
            .cloned()
            .ok_or_else(|| anyhow!("no such key: {key}"))
    }

    fn keys(&self) -> Vec<String> {
        let map = self.map.lock().unwrap();
        map.keys().cloned().collect()
    }

    fn remove_all(&self, keys: Vec<String>) -> i32 {
        let mut map = self.map.lock().unwrap();
        let mut removed = 0;
        for k in keys {
            if map.remove(&k).is_some() {
                removed += 1;
            }
        }
        removed
    }

    fn count(&self) -> i32 {
        self.map.lock().unwrap().len() as i32
    }

    fn contains(&self, key: String) -> bool {
        self.map.lock().unwrap().contains_key(&key)
    }
}

/// The registry of live listeners: an id + the uniform boxed closure the binding
/// hands us (it never learns the callback came from C or C++). Shared behind an
/// `Arc` so each `on_tick` unsubscribe closure can capture a clone and remove its
/// own entry.
type Registry = Arc<Mutex<Vec<(u64, Box<dyn Fn(i32) + Send + Sync>)>>>;

/// A stateful ticker: `on_tick` registers a listener and returns an unsubscribe
/// closure; `tick` fires every live listener with an incrementing counter.
pub struct TickerImpl {
    listeners: Registry,
    next_id: Mutex<u64>,
    counter: Mutex<i32>,
}

impl TickerCore for TickerImpl {
    fn new() -> anyhow::Result<Self> {
        Ok(TickerImpl {
            listeners: Arc::new(Mutex::new(Vec::new())),
            next_id: Mutex::new(0),
            counter: Mutex::new(0),
        })
    }

    fn on_tick(
        &self,
        listener: Box<dyn Fn(i32) + Send + Sync>,
    ) -> anyhow::Result<Box<dyn Fn() + Send + Sync>> {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            let id = *n;
            *n += 1;
            id
        };
        self.listeners.lock().unwrap().push((id, listener));
        // The unsubscribe closure captures an Arc clone of the registry + this
        // listener's id, so dropping/unsubscribing removes exactly this entry.
        let registry = Arc::clone(&self.listeners);
        Ok(Box::new(move || {
            registry.lock().unwrap().retain(|(lid, _)| *lid != id);
        }))
    }

    fn tick(&self) {
        let v = {
            let mut c = self.counter.lock().unwrap();
            let v = *c;
            *c += 1;
            v
        };
        let listeners = self.listeners.lock().unwrap();
        for (_, listener) in listeners.iter() {
            listener(v);
        }
    }
}
