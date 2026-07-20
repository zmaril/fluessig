//! Sync-by-default gate demo (this PR): **synchronous ops are the GLOBAL
//! DEFAULT** across every backend, `#[fluessig(async)]` is the opt-out, and op
//! **export-name pins** apply in node/python/php/ruby.
//!
//! atilla's napi surface is deliberately synchronous and infallible — e.g.
//! `atillaNativeVersion(): string`, NOT `Promise<string>` — under exact JS names
//! (`#[napi(js_name = "…")]`). That is now simply the DEFAULT projection: a plain
//! unary op with no `#[fluessig(async)]` marker generates a synchronous binding
//! in every backend (node `#[napi] fn`, python plain method, php sync method,
//! ruby sync method), infallible when its Rust return is a bare `T`.
//!
//! This tiny schema exercises the three authoring paths against every backend
//! (see `tests/api_gate.rs`):
//!
//! * `native_version` — the DEFAULT: synchronous + **infallible** (bare `String`
//!   return, no `Result` seam) + **name-pinned** (`#[fluessig(name = "…")]`),
//!   reproducing atilla's `atillaNativeVersion(): string` shape verbatim;
//! * `checked_root` — the DEFAULT projection but **fallible** (`Result<T>`
//!   return), so it keeps the throw/raise seam, still with no async;
//! * `slow_count` — `#[fluessig(async)]`, the OPT-OUT: the historical async
//!   projection (node `AsyncTask` → `Promise<i64>`), proving async is reachable.

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
    /// The native core version — the DEFAULT synchronous + infallible projection,
    /// exported under the exact JS name atilla's hand-written binding uses
    /// (`atillaNativeVersion(): string`). Reproduces
    /// `crates/atilla-napi/src/lib.rs`'s `atillaNativeVersion` shape.
    #[fluessig(name = "atillaNativeVersion")]
    pub fn native_version() -> String {
        String::new()
    }

    /// A synchronous but FALLIBLE op — a `Result<T>` return keeps the error seam,
    /// so every backend emits its throwing/raising form (node `-> napi::Result`,
    /// python `PyResult`, php `PhpResult`, ruby `Result<_, Error>`), still with no
    /// async projection.
    pub fn checked_root(path: &str) -> Result<String, String> {
        let _ = path;
        Ok(String::new())
    }

    /// `#[fluessig(async)]` — the OPT-OUT: this op keeps the historical async
    /// `AsyncTask` → `Promise<i64>` projection on the node backend (the async
    /// default is now a per-op opt-in, the no-regression proof).
    #[fluessig(async)]
    pub fn slow_count(path: &str) -> i64 {
        let _ = path;
        0
    }
}

// The exporter half: the marker entity + the `api:` op root. `api_to_json()`
// prints this schema's `api.json` (the op surface every backend gate reads).
// Synchronous is the GLOBAL default; only `slow_count` opts in with
// `#[fluessig(async)]`.
catalog! {
    name: "native_demo",
    version: "0.1.0",
    entities: [Marker],
    api: [Native],
}
