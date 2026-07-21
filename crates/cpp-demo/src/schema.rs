//! The demo op surface, authored with the fluessig **derive front end** (mirrors
//! `crates/derive-demo`): `#[fluessig::export]` on the `impl Store` block captures
//! each method's shape into `api.json`, and `catalog!` lowers it. The bodies here
//! are stubs — the derive reads only the signatures; the REAL engine lives in
//! `crate::core_impl::StoreImpl`, which implements the generated `StoreCore`
//! trait.
//!
//! The surface is deliberately small but exercises every branch the C/C++
//! backend's compile+run gate needs:
//!
//! * `open` — a **`#[fluessig(ctor)]`** taking an **int**: the opaque-handle
//!   `Store_new` / `Store_free` lifecycle.
//! * `put` — a **fallible** op (`Result<i32>`): the `int` status + `char** err_out`
//!   channel, **string** IN and **int** OUT.
//! * `get` — a **string → string fallible** op whose error path (missing key) is
//!   reachable, so the consumer can assert nonzero status + an owned err message.
//! * `keys` — an **infallible** op returning **`list<string>`** (the C
//!   `FlStringList` OUT marshalling — phase 1 left this a TODO placeholder).
//! * `remove_all` — an **infallible** op taking **`list<string>`** IN and
//!   returning an **int** directly (the list IN marshalling counterpart).
//! * `count` — an **infallible** op returning an **int** directly (the
//!   value-returned-directly axis, no `err_out`).
//! * `contains` — an **infallible** op taking a **string** and returning a
//!   **bool** directly.

use fluessig_derive::{catalog, export, Entity};

/// A marker entity so `catalog!` has an entity root; the ops reference nothing
/// from it (the demo's data plane is the in-memory map in `core_impl`).
#[derive(Entity)]
#[fluessig(name = "markers")]
pub struct Marker {
    /// The marker id.
    #[key]
    pub id: i64,
}

/// A tiny stateful key/value store handle whose `impl` is the op interface. The
/// engine behind it is `crate::core_impl::StoreImpl`; only the method shapes are
/// captured here.
pub struct Store {
    _private: (),
}

/// An in-memory key/value store. `put`/`get` are fallible (the error seam); the
/// observers (`keys`/`count`/`contains`/`remove_all`) are synchronous +
/// infallible (a bare `T` return, no `Result`).
#[export]
impl Store {
    /// Open a store with room for `capacity` entries.
    #[fluessig(ctor)]
    pub fn open(capacity: i32) -> Self {
        let _ = capacity;
        Store { _private: () }
    }

    /// Insert or replace `key` → `value`; returns the store's new size. Fails
    /// when the store is at capacity and `key` is new.
    pub fn put(&self, key: &str, value: &str) -> Result<i32, String> {
        let _ = (key, value);
        Ok(0)
    }

    /// Fetch the value for `key`. Fails (the reachable error path) when `key` is
    /// absent.
    pub fn get(&self, key: &str) -> Result<String, String> {
        let _ = key;
        Ok(String::new())
    }

    /// Every key currently stored, in sorted order (infallible `list<string>`).
    pub fn keys(&self) -> Vec<String> {
        Vec::new()
    }

    /// Remove every key in `keys`; returns how many were actually removed
    /// (infallible, `list<string>` IN + `int` out).
    pub fn remove_all(&self, keys: Vec<String>) -> i32 {
        let _ = keys;
        0
    }

    /// The number of entries (infallible `int`).
    pub fn count(&self) -> i32 {
        0
    }

    /// Whether `key` is present (infallible `bool`).
    pub fn contains(&self, key: &str) -> bool {
        let _ = key;
        false
    }
}

// The exporter half: the marker entity root + the `api:` op root.
// `to_json()` prints `catalog.json`; `api_to_json()` prints `api.json`.
catalog! {
    name: "cpp_demo",
    version: "0.1.0",
    entities: [Marker],
    api: [Store],
}
