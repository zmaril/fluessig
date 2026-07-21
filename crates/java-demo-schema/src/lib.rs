//! A compact, purpose-built fluessig schema for the Java (JNI) round-trip
//! harness. It is authored with the same derive front end as `derive-demo`, and
//! its whole reason to exist is to exercise **every op shape the Java backend
//! projects** in one small interface with deterministic, easy-to-assert returns:
//!
//! * `#[fluessig(ctor)]` — the stateful handle path (`Store(long) → init/free`);
//! * a plain unary op returning a bare `String` — **synchronous + infallible**
//!   (`String version()`, no `Result` seam, a direct blocking native);
//! * a plain unary op returning `Result<i64>` — **synchronous + fallible**, so
//!   the JNI `RuntimeException` throw seam is exercised (`long checked(String)`);
//! * `#[fluessig(async)]` — the async projection (`CompletableFuture<Long>
//!   count(String)`, a blocking native wrapped in `supplyAsync`);
//! * `#[fluessig(stream)]` — the poll cursor (`Items items()` →
//!   `Optional<Item> next()` over a `PollStream<Item>`).
//!
//! The `Db` fixture in `derive-demo` deliberately has *only* async ops and large
//! flattened entity models (`Repo`/`PullRequest`), which makes a live round-trip
//! noisy and gives no genuinely synchronous native to call. This schema is the
//! minimal shape that lets the harness call one op of every kind and assert its
//! exact output. It is still a real derive-front-end schema, lowered by the same
//! `catalog!` machinery and fed to `fluessig-gen --java`.

use fluessig_derive::{catalog, export, Entity, Record};

/// A marker table so `catalog!` has an entity root; the ops reference nothing.
#[derive(Entity)]
#[fluessig(name = "markers")]
pub struct Marker {
    /// The marker id.
    #[key]
    pub id: i64,
}

/// One streamed record — a flat scalar DTO the `items` stream yields.
#[derive(Record)]
pub struct Item {
    /// A monotonic item id.
    pub id: i64,
    /// A human label.
    pub label: String,
}

/// A tiny stateful demo handle whose `impl` is the op interface. The engine
/// behind it is irrelevant to the schema — only the method shapes are captured.
pub struct Store {
    _private: (),
}

/// An open demo store. Exercises one op of every shape the Java backend projects.
#[export]
impl Store {
    /// Open the store with a numeric seed — the stateful ctor (`init`/`free`).
    #[fluessig(ctor)]
    pub fn open(seed: i64) -> Self {
        let _ = seed;
        Store { _private: () }
    }

    /// The store version — a synchronous, **infallible** op (bare `String`
    /// return, no error seam; a direct blocking native on the Java side).
    pub fn version(&self) -> String {
        String::new()
    }

    /// Look up a key — a synchronous but **fallible** op (`Result<i64>`), so the
    /// JNI `RuntimeException` throw seam is exercised on the `Err` path.
    pub fn checked(&self, key: &str) -> Result<i64, String> {
        let _ = key;
        Ok(0)
    }

    /// Count matches for a prefix — an `#[fluessig(async)]` op, projected as a
    /// `CompletableFuture<Long>` wrapping the blocking native.
    #[fluessig(async)]
    pub fn count(&self, prefix: &str) -> i64 {
        let _ = prefix;
        0
    }

    /// Stream every item — the stream plane; bindgen maps it to a Java poll
    /// cursor (`Items` with `Optional<Item> next()`).
    #[fluessig(stream)]
    pub fn items(&self) -> impl Iterator<Item = Item> {
        std::iter::empty()
    }
}

// The exporter half: the marker entity + the `Item` DTO + the `api:` op root.
// `to_json()` prints catalog.json; `api_to_json()` prints api.json.
catalog! {
    name: "java_demo",
    version: "0.1.0",
    entities: [Marker],
    records: [Item],
    api: [Store],
}
