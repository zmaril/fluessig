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

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use fluessig_runtime::{Poll, PollStream};

use crate::generated::{Item, StoreCore};

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
