//! Slice 8b — THE ACID TEST parity gate. The derive-authored entl catalog
//! (`src/lib.rs`) and entl's committed TypeSpec-emitted `catalog.json` / `api.json`
//! (at the repo root) must be **semantically equal**: the SAME physical tables
//! (columns + types + nullability + DDL defaults + primary keys), the SAME enums
//! and semantic scalars, and the SAME op surface (ops + materialised models).
//!
//! The comparison normalises exactly what the earlier equivalence gates already
//! normalise:
//!   * the emitter STAMP (`fluessig.emitter` / `compiler`) and `source` — a
//!     front-end signature, not schema content;
//!   * catalog FIELD NAMES (a TypeSpec relation field `repo` vs the derive's
//!     `repo_id`) — the PHYSICAL projection is the contract (per `sql.rs`), so the
//!     tables are compared column-by-column, not the raw catalog JSON;
//!   * DOC comments — the derive carries Rust `///`, the `.tsp` its own prose.
//!
//! COLUMN + PK ORDER **is** part of the contract (position-sensitive). An earlier
//! version of this gate compared columns as a name-keyed set and PKs as a set, on
//! the theory that "`sql.rs` states order is not part of the parity contract."
//! That was wrong for a consumer whose ingest is POSITIONAL (entl's DuckDB
//! appenders follow the generated column order) and whose drift gate is byte-exact:
//! a column REORDER is a real regression the set comparison could not see. It let
//! exactly such a bug through — a polymorphic family SOURCE reference
//! (`gh_labeled` / `gh_assignees`) emitted its `subject_type` tag LAST
//! (`repo_id, subject_number, subject_type`) instead of at its declared middle
//! position (`repo_id, subject_type, subject_number`) — and still reported "full
//! parity." So this gate now compares physical column order and PK order as
//! ORDERED sequences, and additionally asserts each relation's raw
//! `sourceColumns` / `fkColumns` / discriminator columns match position-for-
//! position, catching the divergence at its catalog-level root.

use std::collections::{BTreeMap, BTreeSet};

use fluessig::ir::Catalog;
use fluessig::sql::{tables, Dialect};
use fluessig::{load_catalog, FORMAT_VERSION};

use entl_schema_derive::fluessig_catalog;

/// The repo-root committed artifact path (two levels up from this crate).
fn root(file: &str) -> String {
    format!("{}/../../{file}", env!("CARGO_MANIFEST_DIR"))
}

/// One physical table reduced to its comparable shape: a name-keyed column map
/// `{col → (type, not_null, default)}` (for per-column type/null/default diffs), the
/// ORDERED column-name sequence, and the ORDERED PK column sequence. Column and PK
/// ORDER are position-sensitive — a reorder is a real regression for a positional
/// consumer (entl's DuckDB appenders + byte-exact drift gate).
type TableShape = (
    BTreeMap<String, (String, bool, Option<String>)>,
    Vec<String>,
    Vec<String>,
);
type Physical = BTreeMap<String, TableShape>;

fn physical(catalog: &Catalog) -> Physical {
    tables(catalog, Dialect::Postgres)
        .into_iter()
        .map(|(name, t)| {
            let cols = t
                .columns
                .iter()
                .map(|c| {
                    (
                        c.name.clone(),
                        (c.ty.clone(), c.not_null, c.default.clone()),
                    )
                })
                .collect();
            let col_order = t.columns.iter().map(|c| c.name.clone()).collect();
            let pk = t.pk.clone();
            (name, (cols, col_order, pk))
        })
        .collect()
}

/// Each enum reduced to its `name → [(variant, value)]` shape, order-independent.
fn enum_shapes(c: &Catalog) -> BTreeMap<String, BTreeSet<(String, String)>> {
    c.enums
        .iter()
        .map(|e| {
            let variants = e
                .variants
                .iter()
                .map(|v| {
                    (
                        v.name.clone(),
                        v.value.as_ref().map(|x| x.to_string()).unwrap_or_default(),
                    )
                })
                .collect();
            (e.name.clone(), variants)
        })
        .collect()
}

/// Each declared scalar reduced to `name → base`.
fn scalar_shapes(c: &Catalog) -> BTreeMap<String, Option<String>> {
    c.scalars
        .iter()
        .map(|s| (s.name.clone(), s.base.clone()))
        .collect()
}

fn load_derive_catalog() -> Catalog {
    load_catalog(&fluessig_catalog::to_json()).expect("derive-emitted catalog.json loads clean")
}

fn load_committed_catalog() -> Catalog {
    let json = std::fs::read_to_string(root("catalog.json")).expect("read committed catalog.json");
    load_catalog(&json).expect("committed catalog.json loads")
}

#[test]
fn derive_catalog_loads_and_validates() {
    let catalog = load_derive_catalog();
    let diags = fluessig::catalog::validate(&catalog);
    assert!(
        diags.0.is_empty(),
        "derive-emitted entl catalog must validate clean, got: {}",
        diags
    );
    assert_eq!(catalog.fluessig.format, FORMAT_VERSION);
}

#[test]
fn derive_and_typespec_project_to_the_same_physical_tables() {
    let derive = physical(&load_derive_catalog());
    let tsp = physical(&load_committed_catalog());

    // table-by-table diff, so a mismatch names the exact table/column.
    let d_names: BTreeSet<&String> = derive.keys().collect();
    let t_names: BTreeSet<&String> = tsp.keys().collect();
    assert_eq!(
        d_names,
        t_names,
        "table SET differs\n  only in derive: {:?}\n  only in tsp: {:?}",
        d_names.difference(&t_names).collect::<Vec<_>>(),
        t_names.difference(&d_names).collect::<Vec<_>>(),
    );

    let mut matched = 0;
    for (name, tsp_shape) in &tsp {
        let derive_shape = &derive[name];
        // per-column properties: type / not-null / default (name-keyed).
        assert_eq!(
            derive_shape.0, tsp_shape.0,
            "table `{name}` columns differ\n  derive: {:#?}\n  tsp: {:#?}",
            derive_shape.0, tsp_shape.0
        );
        // ORDERED column sequence — position-sensitive (positional-ingest contract).
        assert_eq!(
            derive_shape.1, tsp_shape.1,
            "table `{name}` COLUMN ORDER differs\n  derive: {:?}\n  tsp:    {:?}",
            derive_shape.1, tsp_shape.1
        );
        // ORDERED PK sequence — position-sensitive.
        assert_eq!(
            derive_shape.2, tsp_shape.2,
            "table `{name}` PK ORDER differs: derive {:?} vs tsp {:?}",
            derive_shape.2, tsp_shape.2
        );
        matched += 1;
    }
    println!(
        "PARITY: {matched}/{} physical tables match (columns + order + PK order)",
        tsp.len()
    );
}

/// Every to-many relation's column spelling, keyed by the (stable) association
/// TABLE name — the one relation identity that survives the field-name difference
/// between the two front ends (derive `subject` vs `.tsp` `subject`, derive
/// `repo_id` vs `.tsp` `repo`). Each entry carries the ORDERED `sourceColumns` and
/// `fkColumns`, plus the source/target discriminator columns.
type RelSpelling = (Vec<String>, Vec<String>, Option<String>, Option<String>);

fn relation_spellings(c: &Catalog) -> BTreeMap<String, RelSpelling> {
    let mut out = BTreeMap::new();
    for e in &c.entities {
        for f in &e.fields {
            if let Some(r) = &f.relation {
                if let Some(table) = &r.table {
                    out.insert(
                        table.clone(),
                        (
                            r.source_columns.clone().unwrap_or_default(),
                            r.fk_columns.clone().unwrap_or_default(),
                            r.source_type_column.clone(),
                            r.type_column.clone(),
                        ),
                    );
                }
            }
        }
    }
    out
}

/// The catalog-level ROOT-CAUSE guard: a polymorphic family SOURCE reference must
/// spell its discriminator INTO `sourceColumns` at the SAME position the TypeSpec
/// front end declares (`@fkSource(#[…tag…], tag)`), not leave it out (which sends
/// `sql.rs`'s `position()` lookup to the fallback that appends the tag LAST). The
/// physical-table gate above catches the downstream reorder; this catches it at the
/// catalog level, position-for-position, so the failure names the exact relation.
#[test]
fn derive_and_typespec_spell_relation_columns_identically() {
    let derive = relation_spellings(&load_derive_catalog());
    let tsp = relation_spellings(&load_committed_catalog());

    let d_tables: Vec<&String> = derive.keys().collect();
    let t_tables: Vec<&String> = tsp.keys().collect();
    assert_eq!(d_tables, t_tables, "association-table SET differs");

    for (table, tsp_spell) in &tsp {
        let derive_spell = &derive[table];
        assert_eq!(
            derive_spell, tsp_spell,
            "relation on `{table}` spells its columns differently (ORDER-sensitive)\n  \
             derive (sourceColumns, fkColumns, sourceTypeColumn, typeColumn): {derive_spell:?}\n  \
             tsp:    (sourceColumns, fkColumns, sourceTypeColumn, typeColumn): {tsp_spell:?}"
        );
    }
    println!(
        "PARITY: {} association-table relation spellings match (sourceColumns/fkColumns ORDER)",
        tsp.len()
    );
}

#[test]
fn derive_and_typespec_declare_the_same_enums_and_scalars() {
    let derive = load_derive_catalog();
    let tsp = load_committed_catalog();
    assert_eq!(
        enum_shapes(&derive),
        enum_shapes(&tsp),
        "declared enums differ"
    );
    assert_eq!(
        scalar_shapes(&derive),
        scalar_shapes(&tsp),
        "declared scalars differ"
    );
    println!(
        "PARITY: {} enums, {} scalars match",
        derive.enums.len(),
        derive.scalars.len()
    );
}

// ── the op surface ──────────────────────────────────────────────────────────

use fluessig::api::{load_api, ApiDoc};

/// Flatten one `api.json` to a pair of sorted, self-describing line sets — the ops
/// and the models — so equality is a set comparison independent of interface /
/// op / model / field declaration order. Each op line reads
/// `Interface.op [shape](p:ty:opt, …) -> ret`; each model line
/// `Model{ field:ty:nullable, … }`. A single pass (not the keyed-map reduction the
/// derive-demo equivalence gate uses) keeps this gate's shape its own.
fn api_lines(api: &ApiDoc) -> (Vec<String>, Vec<String>) {
    let ty = |t: &fluessig::api::ApiType| format!("{t:?}");
    let mut ops: Vec<String> = api
        .interfaces
        .iter()
        .flat_map(|i| {
            i.ops.iter().map(move |op| {
                let ps = op
                    .params
                    .iter()
                    .map(|p| format!("{}:{}:{}", p.name, ty(&p.ty), p.optional.unwrap_or(false)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{}.{} [{:?}]({ps}) -> {}",
                    i.name,
                    op.name,
                    op.shape,
                    ty(&op.returns)
                )
            })
        })
        .collect();
    ops.sort();
    let mut models: Vec<String> = api
        .models
        .iter()
        .map(|m| {
            let mut fs = m
                .fields
                .iter()
                .map(|f| format!("{}:{}:{}", f.name, ty(&f.ty), f.nullable))
                .collect::<Vec<_>>();
            fs.sort();
            format!("{}{{ {} }}", m.name, fs.join(", "))
        })
        .collect();
    models.sort();
    (ops, models)
}

#[test]
fn derive_and_typespec_lower_to_the_same_ops_and_models() {
    let derive = load_api(&fluessig_catalog::api_to_json()).expect("derive api.json loads clean");
    let tsp_json = std::fs::read_to_string(root("api.json")).expect("read committed api.json");
    let tsp = load_api(&tsp_json).expect("committed api.json loads");

    let (d_ops, d_models) = api_lines(&derive);
    let (t_ops, t_models) = api_lines(&tsp);
    assert_eq!(d_ops, t_ops, "derive and typespec ops differ");
    assert_eq!(d_models, t_models, "derive and typespec models differ");

    println!(
        "PARITY: {} ops, {} models match",
        d_ops.len(),
        d_models.len()
    );
}
