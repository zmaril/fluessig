//! Node-backend feature demo (this PR): **synchronous / infallible unary ops**
//! and **op export-name pins**.
//!
//! atilla's napi surface is deliberately synchronous and infallible — e.g.
//! `atillaNativeVersion(): string`, NOT `Promise<string>` — under exact JS names
//! (`#[napi(js_name = "…")]`). Before this PR fluessig's node backend emitted
//! every unary op as an `AsyncTask` → `Promise<T>` over a `Result` seam and
//! applied name pins only to DTO fields, so that shape could not be generated.
//!
//! This tiny schema exercises the two new authoring paths against the node
//! backend (see `tests/api_gate.rs::node_emits_sync_and_pinned_shapes`):
//!
//! * `#[fluessig(sync)]` — a synchronous unary op (no `AsyncTask`/`Promise`);
//!   infallible when the Rust return is a bare `T` (no `Result` seam at all),
//!   throwing when it is a `Result<T>`;
//! * `#[fluessig(name = "…")]` — an explicit export-name pin, emitted as
//!   `#[napi(js_name = "…")]` on the function/method.
//!
//! Both are OPT-IN: the untagged `slow_count` op below stays the historical
//! async `Promise<i64>`, proving the default projection is unchanged.

use fluessig_derive::{catalog, export, Entity};

/// A marker table so `catalog!` has an entity root; the ops reference nothing.
#[derive(Entity)]
#[fluessig(name = "markers")]
pub struct Marker {
    /// The marker id.
    #[key]
    pub id: i64,
}

/// Process-local native helpers — a stateless op group (a unit struct whose
/// `#[export] impl` carries only associated, no-`self` ops → free functions).
pub struct Native;

/// Native helpers (no handle). All unary.
#[export]
impl Native {
    /// The native core version — synchronous, infallible, and exported under the
    /// exact JS name atilla's hand-written binding uses
    /// (`atillaNativeVersion(): string`). Reproduces
    /// `crates/atilla-napi/src/lib.rs`'s `atillaNativeVersion` shape.
    #[fluessig(sync, name = "atillaNativeVersion")]
    pub fn native_version() -> String {
        String::new()
    }

    /// A synchronous but FALLIBLE op — a `Result<T>` return keeps the error seam,
    /// so the node backend emits `-> napi::Result<String>` (Err → JS throw), still
    /// with no `AsyncTask`/`Promise`.
    #[fluessig(sync)]
    pub fn checked_root(path: &str) -> Result<String, String> {
        let _ = path;
        Ok(String::new())
    }

    /// An untagged unary op — NOT `sync`, so it keeps the default async
    /// `AsyncTask` → `Promise<i64>` projection (the opt-in / no-regression proof).
    pub fn slow_count(path: &str) -> i64 {
        let _ = path;
        0
    }
}

// The exporter half: the marker entity + the `api:` op root. `api_to_json()`
// prints this schema's `api.json` (the op surface the node gate reads).
catalog! {
    name: "native_demo",
    version: "0.1.0",
    entities: [Marker],
    api: [Native],
}
