//! Regression demo: the **unsigned + float scalars** (`uint8` / `uint16` /
//! `uint32` / `float32` / `float64`). Before the `ty()` fix these fell through
//! the shared bindgen catchall and were emitted as `String` in every backend —
//! a wrong `.d.ts`/type at the boundary. `tests/numeric_gate.rs` proves each
//! backend now spells them as its native numeric type (node/python `number`,
//! the C/Java primitives, …), NOT `String`.
//!
//! These are exactly the scalar shapes a batch of pidgin modules need:
//! `FuzzyMatchResult.score` (`float64`), `now_ms` op args (`float64`), and
//! `allocateImageId`'s full-range `uint32` (`[1, 0xffffffff]`).

use fluessig_derive::{catalog, export, Entity, Record};

/// A measurement row carrying every previously-mis-emitted numeric scalar as an
/// entity **field** (materialised into `api.json`'s `models`, and into store DDL
/// via `sql.rs`).
#[derive(Entity)]
#[fluessig(name = "measurements")]
pub struct Measurement {
    /// The row id.
    #[key]
    pub id: i64,
    /// A full-range image id — `uint32` (`[1, 0xffffffff]`).
    pub image_id: u32,
    /// A small retry count — `uint8`.
    pub retries: u8,
    /// A TCP-port-sized value — `uint16`.
    pub port: u16,
    /// A single-precision weight — `float32`.
    pub weight: f32,
    /// A double-precision score — `float64`.
    pub score: f64,
}

/// A DTO carrying the same numeric scalars as **fields**, proving they flow
/// through the op-passed `models` layer too. Mirrors pidgin's `FuzzyMatchResult`.
#[derive(Record)]
pub struct FuzzyMatchResult {
    /// The match score — `float64` (the pidgin blocker field).
    pub score: f64,
    /// Confidence — `float32`.
    pub confidence: f32,
    /// The matched image id — `uint32`.
    pub image_id: u32,
}

/// A stateful handle whose `#[export] impl` exercises the numeric scalars as op
/// **params and returns**.
pub struct Metrics {
    _private: (),
}

/// A metrics handle. All ops are synchronous.
#[export]
impl Metrics {
    /// Open the handle.
    #[fluessig(ctor)]
    pub fn open() -> Self {
        Metrics { _private: () }
    }

    /// A `float64` param + a `float32` param, returning `float64` — the
    /// `now_ms`-style op shape (`http-idle-timeout`, `anthropic-sse`, oauth).
    #[fluessig(sync)]
    pub fn scale(&self, now_ms: f64, factor: f32) -> f64 {
        let _ = (now_ms, factor);
        0.0
    }

    /// A full-range `uint32` return — pidgin's `allocateImageId`.
    #[fluessig(sync)]
    pub fn allocate_image_id(&self) -> u32 {
        0
    }

    /// `uint8` + `uint16` params, returning the DTO carrying `float64` /
    /// `float32` / `uint32` fields.
    #[fluessig(sync)]
    pub fn fuzzy_match(&self, retries: u8, port: u16) -> FuzzyMatchResult {
        let _ = (retries, port);
        FuzzyMatchResult {
            score: 0.0,
            confidence: 0.0,
            image_id: 0,
        }
    }
}

// The exporter half — its own `fluessig_catalog` module (`to_json()` /
// `api_to_json()`) the gate loads and drives through every bindgen backend.
catalog! {
    name: "numeric_demo",
    version: "0.1.0",
    entities: [Measurement],
    records: [FuzzyMatchResult],
    api: [Metrics],
}
