//! Slice 3 semantic-equivalence check: the derive-authored catalog
//! (`src/advanced.rs`) and the TypeSpec-authored one (`advanced.tsp`) project to
//! the **same physical tables** — same table names, columns (name + type +
//! nullability), and primary keys — even though they spell inheritance, edges,
//! and column sharing differently at the front end (flatten vs `extends`, edge
//! struct vs `@edge`, `shares(col)` vs twin `@fk`/`@fkSource` naming).
//!
//! The TypeSpec catalog is produced out-of-band by the Node emitter
//! (`node emitter/emit.mjs crates/derive-demo/advanced.tsp --out <dir>`), so this
//! test only runs when its path is provided:
//!
//! ```sh
//! TMP=$(mktemp -d)
//! node emitter/emit.mjs crates/derive-demo/advanced.tsp --out "$TMP"
//! FLUESSIG_TSP_CATALOG="$TMP/catalog.json" cargo test -p derive-demo --test typespec_equivalence -- --nocapture
//! ```
//!
//! Without the env var it prints a skip note and passes, so CI (which has no Node
//! toolchain on the derive-crate job) stays green while the equivalence stays
//! reproducible on demand.

mod common;

#[test]
fn derive_and_typespec_project_to_the_same_tables() {
    common::assert_typespec_equivalent(
        &derive_demo::advanced::fluessig_catalog::to_json(),
        "FLUESSIG_TSP_CATALOG",
        "derive (src/advanced.rs)",
        "typespec (advanced.tsp)",
    );
}
