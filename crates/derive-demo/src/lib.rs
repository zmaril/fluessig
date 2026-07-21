//! Slice 1 demo: one scalar-only entity authored entirely in Rust with the new
//! `#[derive(Entity)]`, collected by `catalog!`. Compare against the TypeSpec a
//! `.tsp` author would write for the same table:
//!
//! ```tsp
//! @name("users")
//! model User {
//!   @key id: int64;
//!   login: string;
//!   name?: string;
//!   admin: boolean;
//! }
//! ```
//!
//! Here the struct *is* the schema — no `.tsp`, no Node.

use fluessig_derive::{catalog, Entity};

/// Slice 2: the foreign-key entity graph (`Id<T>` references + composite keys).
pub mod graph;

/// Slice 3: the attribute grammar — flatten embedding, edge structs, and column
/// sharing (`#[fluessig(flatten)]`, `#[derive(Edge)]`, `#[fluessig(shares(…))]`).
pub mod advanced;

/// Slice 4: polymorphic families — `#[derive(AbstractRoot)]` generating the
/// `<Root>Id` key enums + the `AbstractRoot` alias, scalar- and composite-keyed
/// families, and (tag, key) polymorphic reference sites.
pub mod poly;

/// Slice 5: the op surface — `#[fluessig::export]` on an `impl` block captures
/// each method's shape (ctor / plain unary / stream / manual) into `api.json`,
/// wired into `catalog!`'s `api:` root list alongside the entity catalog.
pub mod api;

/// Slice 8a Gap 1: a direct `Id<Leaf>` FK into a composite-keyed family leaf —
/// the resolver follows `extends` to spell the inherited composite FK columns.
pub mod leaf_fk;

/// Node-backend features (this PR): synchronous / infallible unary ops
/// (`#[fluessig(sync)]`) and op export-name pins (`#[fluessig(name = "…")]`),
/// proven against the node backend in `tests/api_gate.rs`.
pub mod native;

/// Node-backend "tail" features (this PR): position-aware binary spelling
/// (`bytes` param → `Uint8Array`, return → `Buffer`) and the `#[fluessig(result)]`
/// `{ ok, value } | { ok, error }` envelope, proven in `tests/api_gate.rs`.
pub mod binary;

/// A minimal user record — the scalar-only end-to-end skeleton for the derive
/// front end.
#[derive(Entity)]
#[fluessig(name = "users")]
pub struct User {
    /// The user's unique id.
    #[key]
    pub id: i64,
    /// The login handle.
    pub login: String,
    /// Display name, if the user set one.
    pub name: Option<String>,
    /// Whether the user is a site admin.
    pub admin: bool,
}

// The exporter half: `catalog!` collects the listed entities and expands to a
// `fluessig_catalog` module that can print the `catalog.json` the loader consumes.
catalog! {
    name: "user_demo",
    version: "0.1.0",
    entities: [User],
}
