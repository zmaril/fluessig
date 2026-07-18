//! Slice 5 + Slice 8a Gap 2 semantic-equivalence check: the derive-authored op
//! surface (`src/api.rs`, captured from the `#[fluessig::export] impl` blocks +
//! `#[derive(Record)]` DTOs) and the TypeSpec-authored one (`api.tsp`, a separate
//! `interface` + plain DTO models) lower to the same `api.json` — the same OPS
//! (names, kinds, params, returns) AND the same `models` (the flattened DTO/entity
//! shapes the ops reference, closed transitively).
//!
//! Slice 5 scoped this to the ops only (the derive path left `models` empty).
//! Slice 8a Gap 2 materialises the DTO/model layer, so the comparison now covers
//! BOTH the ops and the models — each model reduced to its `(field name, type,
//! nullable)` list, keyed by name (order- and doc-independent, the same
//! normalisation the op comparison uses; the derive carries Rust doc comments the
//! `.tsp` fixture doesn't, and value-struct declaration order is incidental).
//!
//! The TypeSpec `api.json` is produced out-of-band by the Node emitter:
//!
//! ```sh
//! TMP=$(mktemp -d)
//! node emitter/emit.mjs crates/derive-demo/api.tsp --out "$TMP"
//! FLUESSIG_TSP_API="$TMP/api.json" \
//!   cargo test -p derive-demo --test api_typespec_equivalence -- --nocapture
//! ```
//!
//! Without the env var it prints a skip note and passes, so CI (which has no Node
//! toolchain on the derive-crate job) stays green while the equivalence stays
//! reproducible on demand.

use std::collections::BTreeMap;

use fluessig::api::{load_api, ApiDoc};

/// One op reduced to its comparable surface: `(shape, params, returns)` with all
/// types normalised to their JSON spelling. Keyed by `(interface, op)`.
type OpShape = BTreeMap<(String, String), (String, Vec<(String, String, bool)>, String)>;

/// Reduce an `api.json` to the op surface the equivalence gate compares — ops
/// only (kinds/params/returns), dropping the stamp and `source`.
fn op_shapes(api: &ApiDoc) -> OpShape {
    // Debug is a stable structural spelling of each type / shape — enough to
    // compare the two front ends' ops without pulling in a serializer.
    let ty = |t: &fluessig::api::ApiType| format!("{t:?}");
    let mut out = OpShape::new();
    for i in &api.interfaces {
        for op in &i.ops {
            let params = op
                .params
                .iter()
                .map(|p| (p.name.clone(), ty(&p.ty), p.optional.unwrap_or(false)))
                .collect();
            out.insert(
                (i.name.clone(), op.name.clone()),
                (format!("{:?}", op.shape), params, ty(&op.returns)),
            );
        }
    }
    out
}

/// Each model reduced to its comparable shape: its `(field name, type, nullable)`
/// list, keyed by model name — order- and doc-independent (the derive carries
/// Rust doc comments the `.tsp` doesn't, and value-struct declaration order is
/// incidental), the same normalisation `op_shapes` applies.
type ModelShape = BTreeMap<String, Vec<(String, String, bool)>>;

fn model_shapes(api: &ApiDoc) -> ModelShape {
    let ty = |t: &fluessig::api::ApiType| format!("{t:?}");
    api.models
        .iter()
        .map(|m| {
            let fields = m
                .fields
                .iter()
                .map(|f| (f.name.clone(), ty(&f.ty), f.nullable))
                .collect();
            (m.name.clone(), fields)
        })
        .collect()
}

#[test]
fn derive_and_typespec_lower_to_the_same_ops_and_models() {
    let derive = load_api(&derive_demo::api::fluessig_catalog::api_to_json())
        .expect("derive api.json loads");

    let Ok(path) = std::env::var("FLUESSIG_TSP_API") else {
        println!(
            "skip: set FLUESSIG_TSP_API=<api.json> (emit crates/derive-demo/api.tsp \
             via `node emitter/emit.mjs`) to check op + model equivalence."
        );
        return;
    };
    let tsp = load_api(&std::fs::read_to_string(&path).expect("read tsp api.json"))
        .expect("tsp api.json loads");

    let (d_ops, t_ops) = (op_shapes(&derive), op_shapes(&tsp));
    assert_eq!(
        d_ops, t_ops,
        "derive (src/api.rs) and typespec (api.tsp) must lower to the same ops"
    );

    let (d_models, t_models) = (model_shapes(&derive), model_shapes(&tsp));
    assert_eq!(
        d_models, t_models,
        "derive (src/api.rs) and typespec (api.tsp) must materialise the same models"
    );
    println!(
        "op + model equivalence: {} ops, {} models match",
        d_ops.len(),
        d_models.len()
    );
}
