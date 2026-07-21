//! `#[fluessig(single_threaded)]` demo (this PR): a THREAD-CONFINED handle over a
//! GENUINELY `!Send` core.
//!
//! Some native cores are inherently thread-local — pidgin's `TuiCore` is a
//! `Tui<LoggingTerminal>` holding `Rc<RefCell<dyn Component>>` + boxed non-Send
//! closures, so it is `!Send`. napi CLASS instances are thread-confined (they
//! never cross threads), so a hand-written `#[napi]` class can hold such a core
//! fine — but fluessig's ordinary generated handle holds the core as
//! `Arc<crate::core_impl::<Iface>Impl>`, which forces `Impl: Send + Sync` (needed
//! only for the ASYNC projection, where the `Arc` clones onto a threadpool
//! worker). That is a hard `!Send` wall a `Mutex` cannot fix.
//!
//! `#[fluessig(single_threaded)]` on the exported `impl` lowers the interface to a
//! thread-confined handle: the node backend holds the core in a `RefCell` WITHOUT
//! `Arc`/`Send`/`Sync`, so a `!Send` core can be GENERATED instead of hand-written
//! (see `tests/single_threaded.rs` for the byte-golden node handle). The trade:
//! such an interface may carry ONLY synchronous ops — an async/stream op needs a
//! `Send` core for the threadpool, so the derive macro rejects it with a spanned
//! compile error.
//!
//! [`TuiCore`] below is deliberately `!Send` (a `PhantomData<*const ()>` raw
//! pointer + an `Rc<RefCell<…>>` interior state), standing in for pidgin's real
//! renderer, so this fixture is honest: an ordinary (`Arc<Impl>`) handle could not
//! wrap it, but the single_threaded handle can.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

use fluessig_derive::{catalog, export, Entity};

/// A marker table so `catalog!` has an entity root; the ops reference nothing.
#[derive(Entity)]
#[fluessig(name = "tui_markers")]
pub struct TuiMarker {
    /// The marker id.
    #[key]
    pub id: i64,
}

/// A GENUINELY `!Send` renderer core — a stand-in for pidgin's `Tui<LoggingTerminal>`.
/// The `PhantomData<*const ()>` makes it `!Send`/`!Sync` (a raw pointer is neither),
/// and the `Rc<RefCell<…>>` interior state models the real renderer's shared-mutable
/// frame buffer. An ordinary `Arc<Impl>` handle (which needs `Impl: Send + Sync`)
/// could NOT hold this; the single_threaded handle (`RefCell<Impl>`, no bound) can.
pub struct Tui {
    _not_send: PhantomData<*const ()>,
    /// Bytes written across frames — shared-mutable interior state.
    frame_bytes: Rc<RefCell<i64>>,
    /// The current terminal width the next render reads.
    cols: i64,
}

/// A thread-confined TUI renderer over a `!Send` core. Sync ops only — the whole
/// point of `single_threaded` is that no op crosses to a threadpool worker.
#[export]
#[fluessig(single_threaded)]
impl Tui {
    /// Build a renderer over an in-memory terminal of `cols` x `rows`.
    #[fluessig(ctor)]
    pub fn open(cols: i64, rows: i64) -> Self {
        let _ = rows;
        Tui {
            _not_send: PhantomData,
            frame_bytes: Rc::new(RefCell::new(0)),
            cols,
        }
    }

    /// Render one frame; return the running total of bytes written. Mutates the
    /// shared frame buffer via the core's own interior mutability, so on the node
    /// handle this is a `&self` method reaching `&mut` through `borrow_mut()`.
    pub fn tick(&mut self) -> i64 {
        *self.frame_bytes.borrow_mut() += self.cols;
        *self.frame_bytes.borrow()
    }

    /// Resize the terminal the next render reads — a fallible sync op (a bad size
    /// is rejected), so it keeps the throwing seam without any async.
    pub fn set_size(&mut self, cols: i64, rows: i64) -> Result<bool, String> {
        if cols <= 0 || rows <= 0 {
            return Err("terminal dimensions must be positive".into());
        }
        self.cols = cols;
        Ok(true)
    }
}

// The exporter half: the marker entity + the single_threaded `api:` op root.
// `api_to_json()` prints this schema's `api.json` — the op surface carries
// `"single_threaded": true` on the `Tui` interface (skip-if-false, so every other
// interface's `api.json` stays byte-identical).
catalog! {
    name: "single_threaded_demo",
    version: "0.1.0",
    entities: [TuiMarker],
    api: [Tui],
}
