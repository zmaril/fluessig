//! The dogfood gate: the committed `catalog.json` (lowered from `entl.tsp` by
//! @fluessig/emitter) must load + validate, and the IR must answer the questions
//! the codecs will ask. `entl.tsp` + `catalog.json` + `api.json` are kept here as
//! a fixture — a copy of entl's real catalog. Regenerate with:
//!   cd emitter && node emit.mjs ../entl.tsp

use fluessig::{load_catalog, Cardinality, RelKind, TypeRef};

const CATALOG: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog.json"));

#[test]
fn entl_catalog_loads_and_validates() {
    let c = load_catalog(CATALOG).expect("entl catalog must validate");

    // 29-table accounting: 23 concrete entity tables + 6 relation tables.
    let concrete: Vec<_> = c.entities.iter().filter(|e| !e.is_abstract).collect();
    assert_eq!(concrete.len(), 23);
    let mut tables: std::collections::BTreeSet<String> =
        concrete.iter().map(|e| c.table_name(e)).collect();
    for e in &c.entities {
        for f in &e.fields {
            if let Some(t) = f.relation.as_ref().and_then(|r| r.table.clone()) {
                tables.insert(t);
            }
        }
    }
    assert_eq!(tables.len(), 29, "tables: {tables:?}");
}

#[test]
fn polymorphic_families_flatten() {
    let c = load_catalog(CATALOG).unwrap();

    // GitObject family: key is inherited by every leaf (oid alone — content
    // hashes are globally unique; repo is an association, not a key member,
    // matching the real PKs on commits/trees/blobs).
    let commit = c.entity("Commit").unwrap();
    assert_eq!(c.flattened_key(commit), ["oid"]);
    assert_eq!(commit.extends.as_deref(), Some("GitObject"));

    // The leaf's own fields come after the inherited ones (column-order rule).
    let fields = c.flattened_fields(commit);
    assert_eq!(fields[0].name, "oid");
    assert_eq!(fields[1].name, "repo");
    assert!(fields.iter().any(|f| f.name == "parents"));

    // GhSubject family.
    let pr = c.entity("GhPullRequest").unwrap();
    assert_eq!(c.flattened_key(pr), ["repo", "number"]);
}

#[test]
fn the_three_relation_shapes() {
    let c = load_catalog(CATALOG).unwrap();

    // 1. Edge with properties + local key: Commit.parents over commit_parents(commit_oid, idx).
    let commit = c.entity("Commit").unwrap();
    let parents = commit.fields.iter().find(|f| f.name == "parents").unwrap();
    let rel = parents.relation.as_ref().unwrap();
    assert_eq!(rel.cardinality, Cardinality::Many);
    assert_eq!(rel.table.as_deref(), Some("commit_parents"));
    assert_eq!(rel.properties.as_deref(), Some("CommitParent"));
    let edge = c.edge_struct("CommitParent").unwrap();
    let local_key: Vec<_> = edge
        .fields
        .iter()
        .filter(|f| f.key)
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(local_key, ["idx"]);

    // 2. Polymorphic edge: Tree.entries → GitObject with (entry_type, child_oid).
    let tree = c.entity("Tree").unwrap();
    let entries = tree.fields.iter().find(|f| f.name == "entries").unwrap();
    let rel = entries.relation.as_ref().unwrap();
    assert_eq!(rel.to, "GitObject");
    assert!(c.entity("GitObject").unwrap().is_abstract);
    assert_eq!(rel.type_column.as_deref(), Some("entry_type"));
    assert_eq!(
        rel.fk_columns.as_deref(),
        Some(["child_oid".to_string()].as_slice())
    );

    // 3. Polymorphic to-one: GhComment.subject → the GhSubject family.
    let comment = c.entity("GhComment").unwrap();
    let subject = comment.fields.iter().find(|f| f.name == "subject").unwrap();
    let rel = subject.relation.as_ref().unwrap();
    assert_eq!(rel.cardinality, Cardinality::One);
    assert_eq!(rel.kind, RelKind::Association);
    assert_eq!(rel.type_column.as_deref(), Some("subject_type"));
    assert_eq!(
        rel.fk_columns.as_deref(),
        Some(["repo_id".to_string(), "subject_number".to_string()].as_slice())
    );
}

#[test]
fn layer_a_details_survive() {
    let c = load_catalog(CATALOG).unwrap();

    // Semantic scalar with physical carrier.
    let oid = c.scalars.iter().find(|s| s.name == "Oid").unwrap();
    assert_eq!(oid.base.as_deref(), Some("bytes"));

    // Enum wire values.
    let fs = c.enums.iter().find(|e| e.name == "FileStatus").unwrap();
    let added = fs.variants.iter().find(|v| v.name == "added").unwrap();
    assert_eq!(added.value.as_ref().unwrap(), "A");

    // The faithful catalog is FLAT — the real schema has no nested structs
    // (author identity is flattened into author_* columns). Value structs here
    // are all API DTOs (GitStats, …); Layer-A nesting is exercised by the demo
    // catalog (tests/fixtures/entl.tsp).
    let commit = c.entity("Commit").unwrap();
    let author_name = commit
        .fields
        .iter()
        .find(|f| f.name == "authorName")
        .unwrap();
    assert!(author_name.relation.is_none());
    assert_eq!(
        author_name.ty,
        TypeRef::Scalar {
            name: "string".into(),
            base: None
        }
    );
    assert!(c.value_struct("GitStats").is_some());

    // Defaults for byte parity.
    let is_merge = commit.fields.iter().find(|f| f.name == "isMerge").unwrap();
    assert_eq!(is_merge.default.as_ref().unwrap(), false);

    // The §9.3 carve-out: isMerge carries a real @derived spec.
    let der = is_merge.derived.as_ref().unwrap();
    assert_eq!((der.agg.as_str(), der.of.as_str()), ("exists", "parents"));
    assert_eq!(der.filter.as_ref().unwrap().get("idx").unwrap(), 1);
}

// NB: the `the_committed_schema_gen_module_regenerates_identically` drift guard
// that used to live here moved to entl (crates/entl-core/tests/schema_drift.rs)
// when fluessig was extracted — it validates *entl's* committed generated files,
// so it belongs with entl, not with the tool. The tests below exercise fluessig
// itself against a committed copy of entl's catalog kept as a fixture.

#[test]
fn ddl_carries_derived_views_meta_and_extras() {
    use fluessig::sql::{ddl, derived_views, fingerprint, Dialect};
    let c = load_catalog(CATALOG).unwrap();

    // The derived view: commits_derived computes is_merge from commit_parents.
    let views = derived_views(&c, Dialect::Postgres);
    assert_eq!(views.len(), 1, "exactly one entity has @derived fields");
    let v = &views[0];
    assert!(
        v.contains("CREATE OR REPLACE VIEW \"commits_derived\""),
        "{v}"
    );
    assert!(
        v.contains("EXISTS (SELECT 1 FROM \"commit_parents\" x WHERE x.\"commit_oid\" = t.\"oid\" AND x.\"idx\" = 1) AS \"is_merge\""),
        "{v}"
    );
    // SQLite has no OR REPLACE.
    assert!(derived_views(&c, Dialect::Sqlite)[0].starts_with("CREATE VIEW IF NOT EXISTS"));

    // Full DDL: tables + view + meta + extras, and the fingerprint reacts to extras.
    let extras = "CREATE INDEX IF NOT EXISTS commits_repo_idx ON \"commits\" (\"repo_id\");";
    let sql = ddl(&c, Dialect::Postgres, Some(extras));
    assert!(sql.contains("\"_fluessig_meta\""));
    assert!(sql.trim_end().ends_with(extras), "extras append last");
    let fp = fingerprint(&c, Dialect::Postgres, Some(extras));
    assert!(
        sql.contains(&fp),
        "the emitted meta INSERT carries the fingerprint"
    );
    assert_ne!(
        fp,
        fingerprint(&c, Dialect::Postgres, None),
        "editing extras trips drift"
    );
    assert_ne!(
        fp,
        fingerprint(&c, Dialect::Sqlite, Some(extras)),
        "fingerprint is per-dialect"
    );
}
