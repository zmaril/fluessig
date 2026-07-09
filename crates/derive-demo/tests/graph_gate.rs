//! Slice 2 gate: the derive-emitted **foreign-key graph** loads clean through
//! fluessig's existing loader/validator (the one gate every front end passes),
//! the resolved FK relations carry the right target columns, and the physical
//! projection (`fluessig-gen`'s lowering) materialises those FK columns into the
//! DDL — single, nullable, and composite.
//!
//! This is the semantic-equivalence checkpoint from
//! `notes/derive-front-end-decisions.md` (Slice 2, "an entity graph with single +
//! composite FKs matches"): not a byte-for-byte JSON diff, but "the derived
//! catalog loads clean through the Rust validator" and drives `fluessig-gen` to
//! equivalent output. The byte-level comparison against the TypeSpec emitter is
//! demonstrated out-of-band in the PR (`graph.tsp`); here we guard the load,
//! the FK resolution, and the physical FK columns in CI.

use fluessig::ir::{Cardinality, RelKind, TypeRef};
use fluessig::load_catalog;
use fluessig::sql::{tables, Dialect};

/// The graph loads clean, and every `Id<T>` field resolved to a foreign-key
/// relation with the target columns the design specifies.
#[test]
fn fk_graph_loads_and_resolves() {
    let json = derive_demo::graph::fluessig_catalog::to_json();
    let catalog = load_catalog(&json).expect("derive-emitted FK graph must load clean");

    assert_eq!(catalog.entities.len(), 4);
    let entity = |n: &str| catalog.entities.iter().find(|e| e.name == n).unwrap();
    let field = |ent: &fluessig::ir::Entity, n: &str| {
        ent.fields.iter().find(|f| f.name == n).unwrap().clone()
    };

    // ── single-column FK, also a key member (FK-in-PK) ──
    let pr = entity("PullRequest");
    assert_eq!(catalog.flattened_key(pr), vec!["repo_id", "number"]);
    let repo_id = field(pr, "repo_id");
    assert!(repo_id.key);
    assert!(!repo_id.nullable);
    assert_eq!(
        repo_id.ty,
        TypeRef::Ref {
            name: "Repo".into(),
            entity: true
        }
    );
    let rel = repo_id.relation.expect("repo_id is a relation");
    assert_eq!(rel.to, "Repo");
    assert_eq!(rel.cardinality, Cardinality::One);
    assert_eq!(rel.kind, RelKind::Association);
    // single-column target ⇒ the referencing field name spells the FK column
    assert_eq!(
        rel.fk_columns.as_deref(),
        Some(&["repo_id".to_string()][..])
    );
    // concrete target ⇒ no discriminator column (that is Slice 4)
    assert!(rel.type_column.is_none());

    // ── nullable FK: Option<Id<GhUser>> ──
    let author = field(pr, "author_id");
    assert!(author.nullable);
    let author_rel = author.relation.expect("author_id is a relation");
    assert_eq!(author_rel.to, "GhUser");
    assert_eq!(
        author_rel.fk_columns.as_deref(),
        Some(&["author_id".to_string()][..])
    );

    // ── composite-key target referenced via ref_cols on the target ──
    let review = entity("Review");
    let pr_ref = field(review, "pr");
    assert!(!pr_ref.nullable);
    let pr_rel = pr_ref.relation.expect("pr is a relation");
    assert_eq!(pr_rel.to, "PullRequest");
    // the two FK columns come from PullRequest's key order, spelled by its
    // ref_cols (number ⇒ pr_number), NOT from the `pr` field name
    assert_eq!(
        pr_rel.fk_columns.as_deref(),
        Some(&["repo_id".to_string(), "pr_number".to_string()][..])
    );

    // ── another nullable FK on the review ──
    let reviewer = field(review, "reviewer_id");
    assert!(reviewer.nullable);
    assert_eq!(
        reviewer.relation.unwrap().fk_columns.as_deref(),
        Some(&["reviewer_id".to_string()][..])
    );
}

/// `fluessig-gen`'s physical lowering materialises the resolved FK columns into
/// the DDL — the composite FK expands to two columns with the target-declared
/// spelling and the target key's types.
#[test]
fn fk_columns_land_in_the_physical_tables() {
    let json = derive_demo::graph::fluessig_catalog::to_json();
    let catalog = load_catalog(&json).expect("loads");
    let tables = tables(&catalog, Dialect::Postgres);

    // pull_requests: composite PK on the FK-in-PK + number
    let prs = &tables["pull_requests"];
    let col = |t: &fluessig::sql::TableDef, n: &str| {
        t.columns.iter().find(|c| c.name == n).unwrap().clone()
    };
    assert_eq!(prs.pk, vec!["repo_id", "number"]);
    assert_eq!(col(prs, "repo_id").ty, "text"); // Repo.id is String
    assert!(col(prs, "repo_id").not_null);
    assert_eq!(col(prs, "number").ty, "integer");
    assert!(!col(prs, "author_id").not_null); // nullable FK

    // reviews: the composite FK `pr` materialised as (repo_id, pr_number) with
    // the target key's types (text, integer); id stays the sole PK
    let reviews = &tables["reviews"];
    assert_eq!(reviews.pk, vec!["id"]);
    assert_eq!(col(reviews, "repo_id").ty, "text");
    assert!(col(reviews, "repo_id").not_null);
    assert_eq!(col(reviews, "pr_number").ty, "integer");
    assert!(col(reviews, "pr_number").not_null);
    assert!(!col(reviews, "reviewer_id").not_null); // nullable FK
                                                    // the raw `pr` field name is not a physical column — it expanded to the FKs
    assert!(reviews.columns.iter().all(|c| c.name != "pr"));
}

/// A dangling `Id<T>` target is catchable by the loader. `Id<PullRequest>` where
/// `PullRequest` is a real type is a compile-time-checked reference; here we
/// prove the *catalog* gate also rejects a target that is absent from the
/// catalog (e.g. a type that isn't listed as an entity), by lowering `Review`
/// without its `PullRequest` target and asserting the validator flags it.
#[test]
fn dangling_reference_target_is_rejected() {
    use fluessig_derive::{to_catalog_json, Entity};

    // Review references PullRequest, but PullRequest is left out of the catalog.
    let json = to_catalog_json(
        "dangling",
        "0.0.0",
        &[
            <derive_demo::graph::Review as Entity>::DESCRIPTOR,
            <derive_demo::graph::GhUser as Entity>::DESCRIPTOR,
            <derive_demo::graph::Repo as Entity>::DESCRIPTOR,
        ],
    );
    let err = load_catalog(&json).expect_err("a dangling FK target must be rejected");
    assert!(
        err.contains("unknown entity PullRequest"),
        "expected an unknown-target diagnostic, got:\n{err}"
    );
}
