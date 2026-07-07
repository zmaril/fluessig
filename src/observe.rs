//! The observer pool (issue #7) — the generic runtime for "N slow sources →
//! one poll-shaped stream", fluessig's concurrency sibling to the data-plane
//! runtime in [`crate::data`].
//!
//! One supervised thread per **subject** (a session, a repo, …) runs a
//! caller-supplied poll closure on its own interval and funnels typed items
//! into ONE bounded channel; [`ObserverPool::drain`] hands back whatever is
//! ready without blocking — exactly the poll-based `@stream` shape the
//! bindings generate, so an engine's stream op is a thin wrapper over a pool.
//!
//! The contract, by design:
//! - **Isolation**: a slow source stalls only its own observer; a panicking or
//!   erroring one ends as [`Event::Failed`] — the pool survives.
//! - **Backpressure**: the channel is bounded; when the consumer falls behind,
//!   observers wait (in stop-aware slices), memory never grows unbounded.
//! - **Reapability**: [`ObserverPool::reap`] stops one subject and joins its
//!   thread; dropping or [`ObserverPool::shutdown`] does so for all.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// One poll's outcome: items to emit, and whether this subject is finished.
pub enum Poll<T> {
    /// Emit these (possibly none) and keep observing.
    Items(Vec<T>),
    /// Emit these, then end the subject cleanly ([`Event::Ended`]).
    Done(Vec<T>),
}

/// What [`ObserverPool::drain`] yields.
#[derive(Debug, PartialEq)]
pub enum Event<T> {
    /// One observed item from a live subject.
    Item { subject: String, item: T },
    /// The subject's observer finished cleanly (its closure returned [`Poll::Done`]).
    Ended { subject: String },
    /// The observer errored or panicked; the pool survives, the subject is over.
    Failed { subject: String, error: String },
}

struct Handle {
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

/// A pool of per-subject observer threads feeding one bounded channel.
pub struct ObserverPool<T> {
    tx: SyncSender<Event<T>>,
    rx: Mutex<Receiver<Event<T>>>,
    handles: Mutex<HashMap<String, Handle>>,
}

/// How often a waiting observer re-checks its stop flag (while sleeping out
/// its interval or blocked on a full channel).
const STOP_SLICE: Duration = Duration::from_millis(5);

impl<T: Send + 'static> ObserverPool<T> {
    /// A pool whose funnel holds at most `capacity` undrained events.
    pub fn new(capacity: usize) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel(capacity);
        ObserverPool {
            tx,
            rx: Mutex::new(rx),
            handles: Mutex::new(HashMap::new()),
        }
    }

    /// Start observing `subject`: `poll` runs every `interval` on its own
    /// thread until it returns [`Poll::Done`], errors, panics, or is reaped.
    /// Returns `false` (and does nothing) if the subject is already live.
    pub fn spawn<F>(&self, subject: impl Into<String>, interval: Duration, mut poll: F) -> bool
    where
        F: FnMut() -> Result<Poll<T>, String> + Send + 'static,
    {
        let subject = subject.into();
        let mut handles = self.handles.lock().unwrap();
        // prune finished threads so an ended subject can be re-adopted
        handles.retain(|_, h| !h.join.is_finished());
        if handles.contains_key(&subject) {
            return false;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let tx = self.tx.clone();
        let flag = stop.clone();
        let name = subject.clone();
        let join = std::thread::spawn(move || {
            // send with backpressure, abandoning (not blocking forever) on stop
            let send = |ev: Event<T>| -> bool {
                let mut ev = ev;
                loop {
                    match tx.try_send(ev) {
                        Ok(()) => return true,
                        Err(TrySendError::Disconnected(_)) => return false,
                        Err(TrySendError::Full(back)) => {
                            if flag.load(Ordering::Relaxed) {
                                return false;
                            }
                            ev = back;
                            std::thread::sleep(STOP_SLICE);
                        }
                    }
                }
            };

            loop {
                if flag.load(Ordering::Relaxed) {
                    return; // reaped: no event — the reaper knows
                }
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(&mut poll));
                match outcome {
                    Ok(Ok(Poll::Items(items))) => {
                        for item in items {
                            if !send(Event::Item {
                                subject: name.clone(),
                                item,
                            }) {
                                return;
                            }
                        }
                    }
                    Ok(Ok(Poll::Done(items))) => {
                        for item in items {
                            if !send(Event::Item {
                                subject: name.clone(),
                                item,
                            }) {
                                return;
                            }
                        }
                        send(Event::Ended { subject: name });
                        return;
                    }
                    Ok(Err(error)) => {
                        send(Event::Failed {
                            subject: name,
                            error,
                        });
                        return;
                    }
                    Err(panic) => {
                        let error = panic
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "observer panicked".into());
                        send(Event::Failed {
                            subject: name,
                            error,
                        });
                        return;
                    }
                }
                // sleep out the interval in stop-aware slices
                let mut left = interval;
                while !left.is_zero() {
                    if flag.load(Ordering::Relaxed) {
                        return;
                    }
                    let step = left.min(STOP_SLICE);
                    std::thread::sleep(step);
                    left -= step;
                }
            }
        });

        handles.insert(subject, Handle { stop, join });
        true
    }
}

// only spawn() moves T across threads; the drain/reap surface is unbounded
impl<T> ObserverPool<T> {
    /// Everything that's ready, without blocking — the `@stream` drain.
    pub fn drain(&self) -> Vec<Event<T>> {
        self.rx.lock().unwrap().try_iter().collect()
    }

    /// The live subject ids (finished threads are pruned).
    pub fn subjects(&self) -> Vec<String> {
        let mut handles = self.handles.lock().unwrap();
        handles.retain(|_, h| !h.join.is_finished());
        let mut ids: Vec<String> = handles.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Stop one subject's observer and join its thread. `true` if it was live.
    /// (No [`Event::Ended`] is emitted — the reaper already knows.)
    pub fn reap(&self, subject: &str) -> bool {
        let handle = self.handles.lock().unwrap().remove(subject);
        match handle {
            Some(h) => {
                h.stop.store(true, Ordering::Relaxed);
                let _ = h.join.join();
                true
            }
            None => false,
        }
    }

    /// Stop everything and join every thread.
    pub fn shutdown(&self) {
        let drained: Vec<(String, Handle)> = self.handles.lock().unwrap().drain().collect();
        for (_, h) in drained {
            h.stop.store(true, Ordering::Relaxed);
            let _ = h.join.join();
        }
    }
}

impl<T> Drop for ObserverPool<T> {
    fn drop(&mut self) {
        let drained: Vec<(String, Handle)> = self.handles.lock().unwrap().drain().collect();
        for (_, h) in drained {
            h.stop.store(true, Ordering::Relaxed);
            let _ = h.join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    const TICK: Duration = Duration::from_millis(1);

    /// Drain until `pred` or the deadline — tests never sleep blind.
    fn drain_until<T>(
        pool: &ObserverPool<T>,
        mut pred: impl FnMut(&[Event<T>]) -> bool,
    ) -> Vec<Event<T>> {
        let mut got = Vec::new();
        for _ in 0..1000 {
            got.extend(pool.drain());
            if pred(&got) {
                return got;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("condition not reached; got {} events", got.len());
    }

    #[test]
    fn items_flow_and_done_ends_the_subject() {
        let pool: ObserverPool<u32> = ObserverPool::new(64);
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        pool.spawn("s1", TICK, move || {
            match c.fetch_add(1, Ordering::Relaxed) {
                0 => Ok(Poll::Items(vec![1, 2])),
                1 => Ok(Poll::Items(vec![])), // a quiet tick emits nothing
                _ => Ok(Poll::Done(vec![3])),
            }
        });
        let got = drain_until(&pool, |g| {
            g.iter().any(|e| matches!(e, Event::Ended { .. }))
        });
        let items: Vec<u32> = got
            .iter()
            .filter_map(|e| match e {
                Event::Item { item, .. } => Some(*item),
                _ => None,
            })
            .collect();
        assert_eq!(items, [1, 2, 3], "per-subject order is emission order");
        assert!(pool.subjects().is_empty(), "ended subjects are pruned");
    }

    #[test]
    fn a_panicking_observer_fails_alone() {
        let pool: ObserverPool<u32> = ObserverPool::new(64);
        pool.spawn("bad", TICK, || panic!("kaboom"));
        let steady = Arc::new(AtomicUsize::new(0));
        let c = steady.clone();
        pool.spawn("good", TICK, move || {
            Ok(Poll::Items(vec![c.fetch_add(1, Ordering::Relaxed) as u32]))
        });

        let got = drain_until(&pool, |g| {
            let failed = g
                .iter()
                .any(|e| matches!(e, Event::Failed { subject, error } if subject == "bad" && error.contains("kaboom")));
            let living = g
                .iter()
                .filter(|e| matches!(e, Event::Item { subject, .. } if subject == "good"))
                .count();
            failed && living >= 3
        });
        // the pool survived: the good subject kept emitting after the panic
        assert_eq!(pool.subjects(), ["good"]);
        drop(got);
        pool.shutdown();
    }

    #[test]
    fn an_erroring_observer_reports_and_ends() {
        let pool: ObserverPool<u32> = ObserverPool::new(8);
        pool.spawn("e", TICK, || Err("ssh: connection refused".to_string()));
        let got = drain_until(&pool, |g| !g.is_empty());
        assert!(
            matches!(&got[0], Event::Failed { subject, error } if subject == "e" && error.contains("refused"))
        );
    }

    #[test]
    fn reap_stops_a_live_observer_and_duplicates_are_rejected() {
        let pool: ObserverPool<u32> = ObserverPool::new(8);
        assert!(pool.spawn("s", TICK, || Ok(Poll::Items(vec![7]))));
        assert!(
            !pool.spawn("s", TICK, || Ok(Poll::Items(vec![8]))),
            "live id is taken"
        );
        drain_until(&pool, |g| !g.is_empty());
        assert!(pool.reap("s"));
        assert!(!pool.reap("s"), "already reaped");
        assert!(pool.subjects().is_empty());
        // a reaped id can be re-adopted
        assert!(pool.spawn("s", TICK, || Ok(Poll::Done(vec![9]))));
        let got = drain_until(&pool, |g| {
            g.iter().any(|e| matches!(e, Event::Ended { .. }))
        });
        assert!(got.iter().any(|e| matches!(e, Event::Item { item: 9, .. })));
    }

    #[test]
    fn backpressure_bounds_memory_and_reap_frees_a_blocked_observer() {
        // capacity 2, an observer that emits every tick: it must block on the
        // full channel without deadlocking reap()
        let pool: ObserverPool<u32> = ObserverPool::new(2);
        pool.spawn("noisy", TICK, || Ok(Poll::Items(vec![0, 0, 0, 0])));
        std::thread::sleep(Duration::from_millis(30));
        // never more than capacity waiting
        assert!(pool.drain().len() <= 2);
        // reap unblocks the sender path (send loop is stop-aware)
        assert!(pool.reap("noisy"));
    }
}
