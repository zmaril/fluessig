//! Slice 8a Gap 1 gate: a **direct `Id<Leaf>` FK into a composite-keyed family
//! leaf** resolves its inherited composite key through `extends` and lands both
//! FK columns — the shape Slice 4's poly demo avoided.
//!
//! It proves (a) the derive catalog loads clean through fluessig's loader; (b)
//! the IR relation on `Watch.bug` names both FK columns `(project_id,
//! ticket_seq)` — not the under-spelled single column the pre-fix resolver
//! produced; (c) `fluessig-gen`'s physical lowering materialises the `watches`
//! table with the composite FK columns; and (d) the physical projection is
//! identical to the equivalent TypeSpec path (`leaf_fk.tsp`), run out-of-band via
//! the Node emitter and compared when `FLUESSIG_TSP_LEAF_FK` points at its
//! `catalog.json`.

mod common;
use common::{assert_typespec_equivalent, col};
use fluessig::load_catalog;
use fluessig::sql::{tables, Dialect};

fn leaf_fk() -> fluessig::ir::Catalog {
    let json = derive_demo::leaf_fk::fluessig_catalog::to_json();
    load_catalog(&json).expect("Gap-1 derive catalog must load clean")
}

/// The IR relation on the direct leaf FK names BOTH inherited key columns. Before
/// the Gap-1 fix the resolver stopped at the leaf's own (empty) key and
/// under-spelled the FK to a single column; following `extends` to `Ticket`
/// yields the composite `(project_id, ticket_seq)` with no discriminator.
#[test]
fn direct_leaf_fk_spells_the_inherited_composite_key() {
    let c = leaf_fk();
    let watch = c.entity("Watch").expect("Watch present");
    let field = watch
        .fields
        .iter()
        .find(|f| f.name == "bug")
        .expect("bug field present");
    let rel = field.relation.as_ref().expect("a relation");
    assert_eq!(rel.to, "Bug");
    assert_eq!(
        rel.fk_columns.as_deref(),
        Some(&["project_id".to_string(), "ticket_seq".to_string()][..]),
        "an Id<Leaf> FK follows extends to the family's inherited composite key"
    );
    // a direct leaf FK is NOT polymorphic: no type-tag column.
    assert!(
        rel.type_column.is_none(),
        "a direct leaf FK knows its concrete type — no discriminator"
    );
}

/// `fluessig-gen`'s physical projection lands the composite FK columns on the
/// `watches` table with the right types (project_id text from Project.id, and the
/// int32 seq spelled ticket_seq).
#[test]
fn watches_projects_the_composite_fk_columns() {
    let c = leaf_fk();
    let t = tables(&c, Dialect::Postgres);

    let watches = &t["watches"];
    let names: Vec<&str> = watches.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "project_id", "ticket_seq"]);
    assert_eq!(watches.pk, vec!["id"]);
    assert_eq!(col(watches, "project_id").ty, "text"); // Project.id is String
    assert_eq!(col(watches, "ticket_seq").ty, "integer"); // seq is i32

    // the family leaves still share the composite key (sanity: the fix didn't
    // disturb the extends machinery).
    for leaf in ["bugs", "features"] {
        assert_eq!(t[leaf].pk, vec!["project_id", "seq"]);
    }
}

/// The composite-FK-into-a-leaf projects identically across both front ends.
#[test]
fn leaf_fk_matches_the_typespec_projection() {
    assert_typespec_equivalent(
        &derive_demo::leaf_fk::fluessig_catalog::to_json(),
        "FLUESSIG_TSP_LEAF_FK",
        "derive (src/leaf_fk.rs)",
        "typespec (leaf_fk.tsp)",
    );
}
