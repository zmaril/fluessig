//! Slice 8a Gap 1 demo: a **direct `Id<Leaf>` foreign key into a composite-keyed
//! family leaf** — the one reference shape Slice 4's polymorphism demo (`poly.rs`)
//! deliberately avoided, and the shape the entl migration (Slice 8b) needs.
//!
//! Slice 4 proved *polymorphic* references into a family — a `(type-tag, key)`
//! column pair spelled from the abstract root. It never referenced a **concrete
//! leaf directly** by `Id<Leaf>`. That case is subtly different: a leaf re-lists
//! none of the family key on itself (it lives on the abstract root, reached
//! through `extends`), so the FK resolver has to walk `extends` to see the whole
//! composite key — otherwise a composite-keyed leaf looks single-keyed and its FK
//! is under-spelled (a single column where two are needed).
//!
//! A distinct family (deliberately not entl's PR/Issue shape, to keep this focused
//! on the Gap-1 mechanic): `Ticket` is a composite-keyed root (`(project_id,
//! seq)`) with `Bug` / `Feature` leaves, and `Watch` holds a **direct**
//! `Id<Bug>`. The resolver follows `extends` to `Ticket`'s inherited key and
//! spells the two FK columns `(project_id, ticket_seq)` — the family's declared
//! reference spelling, with NO discriminator (a direct leaf FK knows the concrete
//! type; only the *polymorphic* family reference carries a type tag).
//!
//! Compare against the TypeSpec an author would write for the same graph
//! (`leaf_fk.tsp`, alongside this file): both project to the same physical
//! `watches` table with the composite FK columns — modulo the front-end stamp and
//! `source` name (`notes/derive-front-end-decisions.md`, semantic-equivalence).

use fluessig_derive::{catalog, AbstractRoot, Entity, Id};

/// A project — the single-column-keyed reference target the composite family keys
/// on.
#[derive(Entity)]
#[fluessig(name = "projects")]
pub struct Project {
    /// The project id (a slug).
    #[key]
    pub id: String,
}

/// The tracker-item family root — a **composite** key `(project_id, seq)`. It
/// shares its reference spelling once (`tag_col` + `ref_cols(seq = "ticket_seq")`)
/// with every site that points at the family, polymorphic or direct.
#[derive(AbstractRoot)]
#[fluessig(
    abstract_root(Bug, Feature),
    tag_col = "ticket_kind",
    ref_cols(seq = "ticket_seq")
)]
pub struct Ticket {
    /// The project the ticket lives in.
    #[key]
    pub project_id: Id<Project>,
    /// The per-project sequence number.
    #[key]
    pub seq: i32,
}

/// A bug — a leaf of the Ticket family, inheriting `(project_id, seq)` into the
/// `bugs` table.
#[derive(Entity)]
#[fluessig(name = "bugs", extends = Ticket)]
pub struct Bug {
    /// How bad it is.
    pub severity: String,
}

/// A feature request — a leaf of the Ticket family, sharing the same composite
/// key.
#[derive(Entity)]
#[fluessig(name = "features", extends = Ticket)]
pub struct Feature {
    /// A one-line summary.
    pub summary: String,
}

/// A watch subscription — the Gap-1 shape. `bug: Id<Bug>` is a **direct** FK into
/// a composite-keyed family leaf: the resolver walks `extends` to `Ticket`'s
/// inherited key and materialises the two FK columns `(project_id, ticket_seq)`.
#[derive(Entity)]
#[fluessig(name = "watches")]
pub struct Watch {
    /// The subscription id.
    #[key]
    pub id: i64,
    /// The watched bug — a composite FK `(project_id, ticket_seq)` followed
    /// through `extends`, with no discriminator (the concrete leaf is known at the
    /// site).
    pub bug: Id<Bug>,
}

// The exporter half. The family root (`Ticket`) IS an (abstract) entity and is
// listed; the leaves point back via `extends`.
catalog! {
    name: "leaf_fk_demo",
    version: "0.1.0",
    entities: [Project, Ticket, Bug, Feature, Watch],
}
