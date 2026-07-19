// straitjacket-allow-file:duplication — this gate DELIBERATELY mirrors the
// `entl-schema-derive` parity gate's comparison shape (the `physical` / `enum_shapes`
// / `scalar_shapes` / `api_lines` reducers): it is the PARALLEL acid test for a
// second real schema, and reproducing the exact same order-sensitive comparison is
// the point. Factoring the two into a shared helper crate would couple two
// independent migration proofs; each gate stays self-contained and self-describing.
//! THE DISPONENT ACID TEST parity gate. The derive-authored disponent catalog
//! (`src/lib.rs`) and disponent's committed TypeSpec-emitted `catalog.json` /
//! `api.json` must be **semantically equal**: the SAME physical tables (columns +
//! types + nullability + PK, ORDER-sensitive), the SAME enums / scalars, the SAME
//! tagged unions (nine `EventPayload` variants), and the SAME op surface — ops
//! (with every `@readonly` / `@destructive` flag), models, and referenced
//! api-unions.
//!
//! This is the gate that proves the three front-end features this PR adds (union
//! authoring, `#[fluessig(readonly)]`, `#[fluessig(destructive)]`) reproduce the
//! TypeSpec path against disponent's REAL schema.
//!
//! Normalisation (what the entl parity gate already normalises, and no more):
//!   * the emitter STAMP (`fluessig.emitter` / `compiler`) and `source` — a
//!     front-end signature, not schema content;
//!   * catalog FIELD NAMES — the PHYSICAL projection is the contract, so tables are
//!     compared column-by-column (a `.tsp` relation field `session` vs the derive's
//!     `session_uid`, both column `session_uid`);
//!   * DOC comments — the derive carries Rust `///`, the `.tsp` its own prose;
//!   * relation `sourceColumns` / `fkColumns` on the `env_capabilities` edge — the
//!     `.tsp` authored the edge WITHOUT `@fk`/`@fkSource` (both `null`, so `sql.rs`
//!     derives them from the endpoint keys), the derive spells them explicitly
//!     (`["slug"]` / `["capability"]`); both project to the IDENTICAL physical
//!     `env_capabilities` table, which the column/PK-order gate below asserts. This
//!     is the same field-name/spelling normalisation the entl gate documents.
//!
//! COLUMN + PK ORDER **is** part of the contract (position-sensitive), matching the
//! strengthened entl gate — disponent's positional consumers + byte-exact drift.

use std::collections::{BTreeMap, BTreeSet};

use fluessig::ir::Catalog;
use fluessig::sql::{tables, Dialect};
use fluessig::{load_catalog, FORMAT_VERSION};

use disponent_schema_derive::fluessig_catalog;

/// The committed disponent artifact path (the checked-out sibling repo).
fn disponent(file: &str) -> String {
    format!(
        "{}/../../../disponent/schema/{file}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// One physical table reduced to its comparable shape: a name-keyed column map
/// `{col → (type, not_null, default)}`, the ORDERED column-name sequence, and the
/// ORDERED PK column sequence — column + PK ORDER position-sensitive.
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

/// Each declared scalar reduced to `name → base` (the DECLARATION base — the
/// immediate `extends`, `Cents → int64`; the field-usage base roots deeper).
fn scalar_shapes(c: &Catalog) -> BTreeMap<String, Option<String>> {
    c.scalars
        .iter()
        .map(|s| (s.name.clone(), s.base.clone()))
        .collect()
}

/// Each union reduced to `name → [(tag, body-type-name)]`, order-SENSITIVE (the
/// variant order is the wire contract). The body type name is the innermost ref /
/// scalar / enum name, front-end-independent.
fn union_shapes(c: &Catalog) -> BTreeMap<String, Vec<(String, String)>> {
    use fluessig::ir::TypeRef;
    let type_name = |t: &TypeRef| match t {
        TypeRef::Ref { name, .. } => name.clone(),
        TypeRef::Scalar { name, .. } => name.clone(),
        TypeRef::Enum { name } => name.clone(),
        TypeRef::Union { name } => name.clone(),
        TypeRef::List { .. } => "<list>".to_string(),
    };
    c.unions
        .iter()
        .map(|u| {
            (
                u.name.clone(),
                u.variants
                    .iter()
                    .map(|v| (v.tag.clone(), type_name(&v.ty)))
                    .collect(),
            )
        })
        .collect()
}

fn load_derive_catalog() -> Catalog {
    load_catalog(&fluessig_catalog::to_json()).expect("derive-emitted catalog.json loads clean")
}

fn load_committed_catalog() -> Catalog {
    let json =
        std::fs::read_to_string(disponent("catalog.json")).expect("read committed catalog.json");
    load_catalog(&json).expect("committed catalog.json loads")
}

#[test]
fn derive_catalog_loads_and_validates() {
    let catalog = load_derive_catalog();
    let diags = fluessig::catalog::validate(&catalog);
    assert!(
        diags.0.is_empty(),
        "derive-emitted disponent catalog must validate clean, got: {}",
        diags
    );
    assert_eq!(catalog.fluessig.format, FORMAT_VERSION);
}

#[test]
fn derive_and_typespec_project_to_the_same_physical_tables() {
    let derive = physical(&load_derive_catalog());
    let tsp = physical(&load_committed_catalog());

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
        assert_eq!(
            derive_shape.0, tsp_shape.0,
            "table `{name}` columns differ\n  derive: {:#?}\n  tsp: {:#?}",
            derive_shape.0, tsp_shape.0
        );
        assert_eq!(
            derive_shape.1, tsp_shape.1,
            "table `{name}` COLUMN ORDER differs\n  derive: {:?}\n  tsp:    {:?}",
            derive_shape.1, tsp_shape.1
        );
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

#[test]
fn derive_and_typespec_declare_the_same_enums_scalars_and_unions() {
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
    let d_unions = union_shapes(&derive);
    let t_unions = union_shapes(&tsp);
    assert_eq!(d_unions, t_unions, "declared unions differ (feature A)");
    // the whole point: the nine-variant EventPayload union is populated, not empty.
    assert_eq!(
        d_unions.get("EventPayload").map(Vec::len),
        Some(9),
        "EventPayload must carry all nine variants"
    );
    println!(
        "PARITY: {} enums, {} scalars, {} unions ({} EventPayload variants) match",
        derive.enums.len(),
        derive.scalars.len(),
        derive.unions.len(),
        d_unions["EventPayload"].len(),
    );
}

// ── the op surface ──────────────────────────────────────────────────────────

use fluessig::api::{load_api, ApiDoc};

/// Flatten one `api.json` to sorted, self-describing line sets — ops, models, and
/// unions — so equality is order-independent. Each op line carries its shape AND
/// its `@readonly` / `@destructive` flags, so a missing hint is a failure.
fn api_lines(api: &ApiDoc) -> (Vec<String>, Vec<String>, Vec<String>) {
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
                    "{}.{} [{:?}{}{}]({ps}) -> {}",
                    i.name,
                    op.name,
                    op.shape,
                    if op.readonly { " readonly" } else { "" },
                    if op.destructive { " destructive" } else { "" },
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
    let mut unions: Vec<String> = api
        .unions
        .iter()
        .map(|u| {
            let vs = u
                .variants
                .iter()
                .map(|v| format!("{}:{}", v.tag, ty(&v.ty)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}[ {vs} ]", u.name)
        })
        .collect();
    unions.sort();
    (ops, models, unions)
}

#[test]
fn derive_and_typespec_lower_to_the_same_ops_models_and_unions() {
    let derive = load_api(&fluessig_catalog::api_to_json()).expect("derive api.json loads clean");
    let tsp_json = std::fs::read_to_string(disponent("api.json")).expect("read committed api.json");
    let tsp = load_api(&tsp_json).expect("committed api.json loads");

    let (d_ops, d_models, d_unions) = api_lines(&derive);
    let (t_ops, t_models, t_unions) = api_lines(&tsp);
    assert_eq!(d_ops, t_ops, "derive and typespec ops differ");
    assert_eq!(d_models, t_models, "derive and typespec models differ");
    assert_eq!(
        d_unions, t_unions,
        "derive and typespec api-unions differ (feature A)"
    );

    // features B + C: the nine readonly ops and two destructive ops are present.
    let readonly = derive
        .interfaces
        .iter()
        .flat_map(|i| &i.ops)
        .filter(|o| o.readonly)
        .count();
    let destructive = derive
        .interfaces
        .iter()
        .flat_map(|i| &i.ops)
        .filter(|o| o.destructive)
        .count();
    assert_eq!(readonly, 9, "expected 9 @readonly ops");
    assert_eq!(destructive, 2, "expected 2 @destructive ops");

    println!(
        "PARITY: {} ops ({readonly} readonly, {destructive} destructive), {} models, {} unions match",
        d_ops.len(),
        d_models.len(),
        d_unions.len(),
    );
}
