//! The shared streaming runtime contract for fluessig.
//!
//! Every language backend (`src/bindgen/{node,python,ruby,php}.rs`) and the
//! hand-written core speak the same streaming/terminal protocol: a stream is a
//! blocking, timeout-bounded [`PollStream`] whose [`poll`](PollStream::poll)
//! returns a [`Poll`] result — an item, an idle tick, a clean close, or a
//! terminal failure. Historically each backend emitted its own copy of these
//! two shapes into its generated prelude; the copies drifted. This crate holds
//! the ONE canonical definition so the protocol is described — and unit-tested —
//! in exactly one place, and every emitter and the core can share it verbatim.

use std::time::Duration;

/// One poll result from a core stream (the sync primitive every stream shape dresses).
/// `Failed(msg)` is the SECOND error model. Once a stream has started, pi's
/// contract flips: a request/model/runtime failure is no longer thrown — it is
/// ENCODED IN THE STREAM as a terminal error EVENT and the stream then completes
/// (packages/ai/src/types.ts: after `stream()` returns, failures ride the stream,
/// never reject the promise). `Failed` is the generic path for a core that surfaces
/// a mid-stream failure as a Rust `Result`/error; a core that instead emits its
/// terminal error as a normal union VARIANT of the element type flows through
/// `Item` unchanged — both satisfy "never throw after stream start". The message
/// is owned (`String`) so the enum stays trivially `Send` and dependency-free.
pub enum Poll<T> {
    Item(T),
    Idle,
    Closed,
    Failed(String),
}

/// The one sync primitive: a blocking, timeout-bounded poll.
pub trait PollStream<T>: Send + Sync {
    fn poll(&self, timeout: Duration) -> Poll<T>;
    /// Release core-side resources. Called on async-iterator cancellation
    /// (`return()`), on completion, and on drop. Must be idempotent; the
    /// default is a no-op so poll-only cores need no change.
    fn close(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// A mock stream backed by a queue of scripted poll results. Each `poll`
    /// pops the next scripted result; an exhausted queue reports `Closed`.
    /// `close()` bumps a counter so we can assert idempotency and custom
    /// close behaviour.
    struct MockStream<T> {
        script: Mutex<VecDeque<Poll<T>>>,
        closes: AtomicUsize,
    }

    impl<T> MockStream<T> {
        fn new(script: impl IntoIterator<Item = Poll<T>>) -> Self {
            MockStream {
                script: Mutex::new(script.into_iter().collect()),
                closes: AtomicUsize::new(0),
            }
        }

        fn close_count(&self) -> usize {
            self.closes.load(Ordering::SeqCst)
        }
    }

    impl<T: Send + Sync> PollStream<T> for MockStream<T> {
        fn poll(&self, _timeout: Duration) -> Poll<T> {
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Poll::Closed)
        }

        fn close(&self) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// The canonical driver loop every binding wraps: skip `Idle`, collect
    /// `Item`s in order, stop on `Closed`, and surface `Failed(msg)` as a
    /// terminal error. Returns the items drained before termination together
    /// with the terminal outcome (`Ok(())` on clean close, `Err(msg)` on
    /// failure).
    fn drive<T>(stream: &dyn PollStream<T>) -> (Vec<T>, Result<(), String>) {
        let mut items = Vec::new();
        loop {
            match stream.poll(Duration::from_millis(0)) {
                Poll::Item(v) => items.push(v),
                Poll::Idle => continue,
                Poll::Closed => return (items, Ok(())),
                Poll::Failed(msg) => return (items, Err(msg)),
            }
        }
    }

    #[test]
    fn driver_skips_idle_and_yields_items_in_order_until_closed() {
        let stream = MockStream::new([
            Poll::Idle,
            Poll::Item(1),
            Poll::Idle,
            Poll::Idle,
            Poll::Item(2),
            Poll::Item(3),
            Poll::Idle,
            Poll::Closed,
        ]);

        let (items, outcome) = drive(&stream);

        assert_eq!(items, vec![1, 2, 3], "items must arrive in emission order");
        assert_eq!(outcome, Ok(()), "a Closed stream terminates cleanly");
    }

    #[test]
    fn failed_is_a_terminal_error_carrying_the_message() {
        let stream = MockStream::new([
            Poll::Item(10),
            Poll::Idle,
            Poll::Failed("model overloaded".to_string()),
            // Anything after the terminal event must never be observed.
            Poll::Item(999),
        ]);

        let (items, outcome) = drive(&stream);

        assert_eq!(items, vec![10], "items before the failure still arrive");
        assert_eq!(
            outcome,
            Err("model overloaded".to_string()),
            "Failed(msg) is observed as a terminal error carrying its message"
        );
    }

    #[test]
    fn default_close_is_a_no_op_and_idempotent() {
        /// A poll-only core that never overrides `close()`, exercising the
        /// default no-op body.
        struct PollOnly;
        impl PollStream<i32> for PollOnly {
            fn poll(&self, _timeout: Duration) -> Poll<i32> {
                Poll::Closed
            }
        }

        let stream = PollOnly;
        // The default no-op must be callable repeatedly without effect.
        stream.close();
        stream.close();
        stream.close();
        // And the stream still behaves after being "closed".
        assert!(matches!(
            stream.poll(Duration::from_millis(0)),
            Poll::Closed
        ));
    }

    #[test]
    fn custom_close_runs_and_is_idempotently_callable() {
        let stream = MockStream::<i32>::new([Poll::Closed]);
        assert_eq!(stream.close_count(), 0);

        stream.close();
        stream.close();

        assert_eq!(
            stream.close_count(),
            2,
            "a custom close() impl runs on every call the runtime makes"
        );
    }

    #[test]
    fn pollstream_is_object_safe_through_box_and_arc() {
        // Guards the `Box<dyn PollStream<T>>` / `Arc<dyn PollStream<T>>` usage
        // the generated bindings and the core signatures rely on.
        let boxed: Box<dyn PollStream<i32>> =
            Box::new(MockStream::new([Poll::Item(7), Poll::Closed]));
        let (items, outcome) = drive(boxed.as_ref());
        assert_eq!(items, vec![7]);
        assert_eq!(outcome, Ok(()));
        boxed.close();

        let shared: Arc<dyn PollStream<i32>> = Arc::new(MockStream::new([
            Poll::Item(1),
            Poll::Item(2),
            Poll::Closed,
        ]));
        let clone = Arc::clone(&shared);
        let (items, outcome) = drive(clone.as_ref());
        assert_eq!(items, vec![1, 2]);
        assert_eq!(outcome, Ok(()));

        // `Send + Sync` supertraits let the trait object cross threads.
        let moved = Arc::clone(&shared);
        let handle = std::thread::spawn(move || moved.poll(Duration::from_millis(0)));
        assert!(matches!(handle.join().unwrap(), Poll::Closed));
    }
}
