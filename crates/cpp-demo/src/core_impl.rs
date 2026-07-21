//! The hand-written engine behind the demo op surface: an in-memory key/value
//! `Store` implementing the generated `StoreCore` trait. This is the ONE module
//! a consumer writes by hand (the house-style "generated surface + hand-written
//! `core_impl`" split); everything else in the crate is generated.
//!
//! The core is synchronous and holds its state behind a `Mutex` so the trait's
//! `&self` methods can mutate it while satisfying `Send + Sync`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use anyhow::{anyhow, bail};

use crate::generated::StoreCore;

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
