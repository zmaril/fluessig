//! The hand-written core the generated wasm-bindgen glue routes into — the seam
//! `self.inner.op(..)` / `<crate::core_impl::TickerImpl as TickerCore>::op(..)`
//! calls resolve to.
//!
//! `TickerImpl` implements the callback + subscription slice: `on_tick` registers
//! a host callback (the uniform `Box<dyn Fn(i32) + Send + Sync>` the wasm-bindgen
//! glue builds from a JS `Function`) and returns an unsubscribe closure that
//! removes it; `tick` fires every live listener with an incrementing counter. It
//! knows nothing about wasm/JS — it sees only the uniform boxed `Fn`, exactly as
//! the ruby/java/cpp/node/python demo cores do.
//!
//! straitjacket-allow-file:duplication — deliberately parallel to the sibling
//! `crates/callback-demo-ruby/src/core_impl.rs` `TickerImpl` (the callback +
//! subscription demo core); each demo crate carries its own so the per-language
//! round-trips are independently buildable.

use std::sync::{Arc, Mutex};

use crate::generated::TickerCore;

/// A registered listener: its stable id (so an unsubscribe can find and remove
/// it) and the uniform boxed callback the wasm-bindgen glue built from a JS
/// `Function`.
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
        // Fire every live listener under the lock. The demo's JS callback only
        // records the value, so it never re-enters the ticker (which would
        // otherwise deadlock this `Mutex`).
        for (_, listener) in self.listeners.lock().unwrap().iter() {
            listener(value);
        }
    }
}
