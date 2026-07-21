//! The hand-written core the generated ext-php-rs glue routes into — the seam
//! `<crate::core_impl::TickerImpl as TickerCore>::op(..)` calls resolve to.
//!
//! `TickerImpl` implements the callback + subscription slice: `on_tick` registers
//! a host callback (the uniform `Box<dyn Fn(i32) + Send + Sync>` the ext-php-rs
//! glue builds from a PHP `callable`) and returns an unsubscribe closure that
//! removes it; `tick` fires every live listener with an incrementing counter. It
//! knows nothing about PHP — it sees only the uniform boxed `Fn`, exactly as the
//! java/cpp/node/python/ruby/wasm demo cores do. Every listener firing happens
//! synchronously on the PHP request thread (the PHP sync-only contract).
//!
//! straitjacket-allow-file:duplication — deliberately parallel to the sibling
//! `crates/callback-demo-ruby/src/core_impl.rs` `TickerImpl` (the callback +
//! subscription demo core); each demo crate carries its own so the per-language
//! round-trips are independently buildable.

use std::sync::{Arc, Mutex};

use crate::generated::TickerCore;

/// A registered listener: its stable id (so an unsubscribe can find and remove
/// it) and the uniform boxed callback the ext-php-rs glue built from a PHP
/// `callable`.
type Listener = (u64, Box<dyn Fn(i32) + Send + Sync>);

/// The ticker demo engine: a set of live listeners (behind an `Arc<Mutex<…>>` so
/// each `on_tick`'s returned unsubscribe closure can share it), a monotonic id
/// source, and the tick counter fired at each `tick`.
pub struct TickerImpl {
    listeners: Arc<Mutex<Vec<Listener>>>,
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

    fn on_tick(&self, listener: Box<dyn Fn(i32) + Send + Sync>) -> Box<dyn Fn() + Send + Sync> {
        // Assign a stable id, register the listener, and hand back an unsubscribe
        // closure that removes exactly this registration. The closure captures an
        // `Arc` clone of the shared set, so the returned `Subscription` handle can
        // deregister independently of `self`'s lifetime.
        let id = {
            let mut n = self.next_id.lock().unwrap();
            let id = *n;
            *n += 1;
            id
        };
        self.listeners.lock().unwrap().push((id, listener));
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || {
            listeners.lock().unwrap().retain(|(lid, _)| *lid != id);
        })
    }

    fn tick(&self) {
        // Read-then-increment: the first tick fires 0, the next 1, and so on.
        let value = {
            let mut c = self.counter.lock().unwrap();
            let v = *c;
            *c += 1;
            v
        };
        // Fire every live listener under the lock, SYNCHRONOUSLY on the calling
        // (PHP request) thread — the sync-only contract PHP requires. The demo's
        // PHP closure only appends to an array, so it never re-enters the ticker
        // (which would otherwise deadlock this `Mutex`).
        for (_, listener) in self.listeners.lock().unwrap().iter() {
            listener(value);
        }
    }
}
