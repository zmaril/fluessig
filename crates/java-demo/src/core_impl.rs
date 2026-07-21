//! The hand-written core the generated JNI glue routes into — the seam
//! `<crate::core_impl::StoreImpl as StoreCore>::op(..)` calls resolve to.
//!
//! In a real consumer this would sit over the actual engine; here it returns
//! deterministic values so the round-trip's assertions are exact. Every op shape
//! the Java backend projects is implemented once:
//!
//! * `open` — the fallible ctor (stashes the numeric seed);
//! * `version` — synchronous + infallible (bare `String`);
//! * `checked` — synchronous + fallible (`Result<i64>`; `key == "boom"` errors,
//!   exercising the JNI `RuntimeException` throw seam);
//! * `count` — async + fallible (`Result<i64>`; the Java side wraps it in a
//!   `CompletableFuture`);
//! * `items` — the stream, a `PollStream<Item>` draining a fixed script.
//!
//! `TickerImpl` implements the callback + subscription slice: `on_tick` registers
//! a host callback (the uniform `Box<dyn Fn(i32) + Send + Sync>` the JNI glue
//! builds from a Java `Consumer<Integer>`) and returns an unsubscribe closure that
//! removes it; `tick` fires every live listener with an incrementing counter.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fluessig_runtime::{Poll, PollStream};

use crate::generated::{Item, StoreCore, TickerCore};

/// The demo engine: just the seed handed to the ctor.
pub struct StoreImpl {
    seed: i64,
}

impl StoreCore for StoreImpl {
    fn open(seed: i64) -> anyhow::Result<Self> {
        Ok(StoreImpl { seed })
    }

    fn version(&self) -> String {
        // Infallible: a bare value, no error channel.
        "store-1.0".to_string()
    }

    fn checked(&self, key: String) -> anyhow::Result<i64> {
        // Fallible: the "boom" key drives the Err → thrown RuntimeException path.
        if key == "boom" {
            anyhow::bail!("boom requested for key {key}");
        }
        Ok(self.seed + key.len() as i64)
    }

    fn count(&self, prefix: String) -> anyhow::Result<i64> {
        // Async at the Java seam (CompletableFuture); a plain blocking call here.
        Ok(prefix.len() as i64)
    }

    fn items(&self) -> anyhow::Result<Box<dyn PollStream<Item>>> {
        // A fixed, ordered script so the drained sequence is exact.
        Ok(Box::new(ScriptStream::new(vec![
            Item {
                id: 1,
                label: "alpha".to_string(),
            },
            Item {
                id: 2,
                label: "beta".to_string(),
            },
            Item {
                id: 3,
                label: "gamma".to_string(),
            },
        ])))
    }
}

/// A `PollStream` that yields a fixed queue of items in order, then closes.
struct ScriptStream {
    queue: Mutex<VecDeque<Item>>,
}

impl ScriptStream {
    fn new(items: Vec<Item>) -> Self {
        ScriptStream {
            queue: Mutex::new(items.into_iter().collect()),
        }
    }
}

impl PollStream<Item> for ScriptStream {
    fn poll(&self, _timeout: Duration) -> Poll<Item> {
        match self.queue.lock().unwrap().pop_front() {
            Some(item) => Poll::Item(item),
            None => Poll::Closed,
        }
    }
}

/// A registered listener: its stable id (so an unsubscribe can find and remove
/// it) and the uniform boxed callback the JNI glue built from a Java `Consumer`.
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
        // Snapshot nothing — the listener closures are not `Clone`; fire them under
        // the lock. The demo's Java `Consumer` only appends to a list, so it never
        // re-enters the ticker (which would otherwise deadlock this `Mutex`).
        for (_, listener) in self.listeners.lock().unwrap().iter() {
            listener(value);
        }
    }
}
