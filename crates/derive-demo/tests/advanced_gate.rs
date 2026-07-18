//! Slice 3 gate: the derive-emitted catalog exercising **flatten** embedding,
//! **edge structs**, and **column sharing** loads clean through fluessig's
//! existing loader/validator, and `fluessig-gen`'s physical lowering
//! materialises the shapes as specified — the flattened columns land in the leaf
//! table, the edge table materialises with its local-key PK, and the shared
//! column appears once.
//!
//! This is the semantic-equivalence checkpoint from
//! `notes/derive-front-end-decisions.md` (Slice 3, "the grammar is a proposal,
//! not a survivor of implementation contact" — first contact with real parsing):
//! not a byte-for-byte JSON diff, but "the derived catalog loads clean through
//! the Rust validator" and drives `fluessig-gen` to the expected output. The
//! byte-level comparison against the TypeSpec emitter is demonstrated out-of-band
//! in the PR (`advanced.tsp`); here we guard the load and the physical
//! projection in CI.

mod common;
use common::col;
use fluessig::load_catalog;
use fluessig::sql::{tables, Dialect};
use fluessig_derive::{
    to_catalog_json_with_edges, EdgeDescriptor, EdgeFieldDescriptor, EdgeRole, Entity,
    FieldDescriptor, FieldKind,
};

fn advanced() -> fluessig::ir::Catalog {
    let json = derive_demo::advanced::fluessig_catalog::to_json();
    load_catalog(&json).expect("Slice 3 derive catalog must load clean")
}

/// `#[fluessig(flatten)]` embeds the root's columns into the leaf: `commits`
/// carries `oid` (the embedded key) + `repo_id` (the embedded association) inline,
/// then its own fields, and the entity keys on the embedded `oid`.
#[test]
fn flatten_embeds_root_columns_in_the_leaf() {
    let c = advanced();

    // GitObject is a flatten *source*, not a table — it must not appear.
    assert!(
        c.entity("GitObject").is_none(),
        "the flatten source is not itself an entity/table"
    );

    let commit = c.entity("Commit").expect("Commit present");
    // the embedded key participates as the leaf's key
    assert_eq!(c.flattened_key(commit), vec!["oid"]);
    let names: Vec<&str> = commit.fields.iter().map(|f| f.name.as_str()).collect();
    // oid + repo_id are embedded (first, for column-order parity), then own fields
    assert_eq!(
        names,
        vec!["oid", "repo_id", "message", "summary", "parents"]
    );

    // the embedded repo_id kept its FK relation (it is not a bare scalar)
    let repo_id = commit.fields.iter().find(|f| f.name == "repo_id").unwrap();
    assert_eq!(repo_id.relation.as_ref().unwrap().to, "Repo");

    // physical projection: the flattened columns land in the commits table
    let t = tables(&c, Dialect::Postgres);
    let commits = &t["commits"];
    assert_eq!(commits.pk, vec!["oid"]);
    assert_eq!(col(commits, "oid").ty, "text");
    assert_eq!(col(commits, "repo_id").ty, "text"); // Repo.id is String
    assert!(col(commits, "message").not_null);
    assert!(col(commits, "summary").not_null);
}

/// The edge struct materialises its own table: `commit_parents` with the source
/// column (`commit_oid`), the local-key property (`idx`), the target column
/// (`parent_oid`), and the FINDINGS #3 PK = source key + local key.
#[test]
fn edge_struct_materialises_its_table() {
    let c = advanced();

    // the edge landed as a relationProperties struct carrying only its local key
    let cp = c
        .edge_struct("CommitParent")
        .expect("CommitParent edge struct");
    let cp_fields: Vec<&str> = cp.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(cp_fields, vec!["idx"]);
    assert!(cp.fields[0].key, "idx is the edge's local key");

    // the relation field on the from-entity carries the edge wiring
    let commit = c.entity("Commit").unwrap();
    let parents = commit.fields.iter().find(|f| f.name == "parents").unwrap();
    let rel = parents.relation.as_ref().unwrap();
    assert_eq!(rel.to, "Commit");
    assert_eq!(rel.cardinality, fluessig::ir::Cardinality::Many);
    assert_eq!(rel.table.as_deref(), Some("commit_parents"));
    assert_eq!(rel.properties.as_deref(), Some("CommitParent"));
    assert_eq!(
        rel.source_columns.as_deref(),
        Some(&["commit_oid".to_string()][..])
    );
    assert_eq!(
        rel.fk_columns.as_deref(),
        Some(&["parent_oid".to_string()][..])
    );

    // physical projection: the edge table, PK = source + local key
    let t = tables(&c, Dialect::Postgres);
    let cp_tbl = &t["commit_parents"];
    let names: Vec<&str> = cp_tbl.columns.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"commit_oid"));
    assert!(names.contains(&"parent_oid"));
    assert!(names.contains(&"idx"));
    assert_eq!(cp_tbl.pk, vec!["commit_oid", "idx"]);
}

/// `#[fluessig(shares(repo_id))]` folds the label's leading FK column into the
/// issue's `repo_id`: `gh_issue_labels` carries `repo_id` **once**, with no
/// separate `label_repo_id`.
#[test]
fn shares_folds_the_shared_column() {
    let c = advanced();

    let t = tables(&c, Dialect::Postgres);
    let edge = &t["gh_issue_labels"];
    let names: Vec<&str> = edge.columns.iter().map(|c| c.name.as_str()).collect();
    // source (repo_id, issue_number) + target (repo_id shared → folded, label_name)
    assert_eq!(names, vec!["repo_id", "issue_number", "label_name"]);
    assert!(
        !names.contains(&"label_repo_id"),
        "shares(repo_id) folds the label repo column into the shared one"
    );
    // the shared column carries the whole PK once
    assert_eq!(edge.pk, vec!["repo_id", "issue_number", "label_name"]);
}

/// Control: the SAME edge lowered **without** `shares(repo_id)` keeps the label's
/// own leading FK column (`label_repo_id`) distinct — four columns, not three.
/// This proves `shares(...)` is load-bearing, not cosmetic.
#[test]
fn without_shares_the_column_is_not_folded() {
    // an edge descriptor identical to GhIssueLabel but with the label reference's
    // `shares` cleared — everything else (targets, ref_cols on the entities) held.
    static NO_SHARES: EdgeDescriptor = EdgeDescriptor {
        name: "GhIssueLabelPlain",
        table: Some("gh_issue_labels_plain"),
        doc: None,
        from: "GhIssue",
        to: "GhLabel",
        expose: Some("labels_plain"),
        fields: &[
            EdgeFieldDescriptor {
                field: FieldDescriptor {
                    name: "issue",
                    kind: FieldKind::Reference("GhIssue"),
                    nullable: false,
                    key: false,
                    doc: None,
                    shares: &[],
                    span: fluessig_derive::SourceSpan::UNKNOWN,
                },
                role: EdgeRole::Source,
            },
            EdgeFieldDescriptor {
                field: FieldDescriptor {
                    name: "label",
                    kind: FieldKind::Reference("GhLabel"),
                    nullable: false,
                    key: false,
                    doc: None,
                    shares: &[], // ← the only difference from the demo's shares(repo_id)
                    span: fluessig_derive::SourceSpan::UNKNOWN,
                },
                role: EdgeRole::Target,
            },
        ],
        span: fluessig_derive::SourceSpan::UNKNOWN,
    };

    let entities = [
        <derive_demo::advanced::Repo as Entity>::DESCRIPTOR,
        <derive_demo::advanced::GhIssue as Entity>::DESCRIPTOR,
        <derive_demo::advanced::GhLabel as Entity>::DESCRIPTOR,
    ];
    let json = to_catalog_json_with_edges("plain", "0.0.0", &entities, &[&NO_SHARES]);
    let c = load_catalog(&json).expect("no-shares variant still loads clean");

    let t = tables(&c, Dialect::Postgres);
    let edge = &t["gh_issue_labels_plain"];
    let names: Vec<&str> = edge.columns.iter().map(|c| c.name.as_str()).collect();
    // without shares, the label's leading key part keeps its own spelling
    assert_eq!(
        names,
        vec!["repo_id", "issue_number", "label_repo_id", "label_name"]
    );
    assert!(names.contains(&"label_repo_id"));
}
