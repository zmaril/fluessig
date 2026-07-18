//! Slice 5 semantic-equivalence check: the derive-authored op surface
//! (`src/api.rs`, captured from the `#[fluessig::export] impl` blocks) and the
//! TypeSpec-authored one (`api.tsp`, a separate `interface`) lower to the same
//! OPS in `api.json` — same op names, kinds (ctor/unary/stream/manual), params
//! (name + type + optional), and returns — even though one captures the surface
//! from the impl that runs and the other declares it separately.
//!
//! Scoped to the OP surface per `notes/derive-front-end-decisions.md` (Slice 5):
//! the TypeSpec path additionally materialises the entities the ops reference as
//! flattened DTO `models`; the derive path emits the op surface only. The
//! comparison therefore normalises away `models` (and the front-end stamp +
//! `source`), comparing ops only — the surface Slice 5 owns.
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
/// only (kinds/params/returns), dropping `models`, the stamp, and `source`.
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

#[test]
fn derive_and_typespec_lower_to_the_same_ops() {
    let derive = load_api(&derive_demo::api::fluessig_catalog::api_to_json())
        .expect("derive api.json loads");

    let Ok(path) = std::env::var("FLUESSIG_TSP_API") else {
        println!(
            "skip: set FLUESSIG_TSP_API=<api.json> (emit crates/derive-demo/api.tsp \
             via `node emitter/emit.mjs`) to check op-surface equivalence."
        );
        return;
    };
    let tsp = load_api(&std::fs::read_to_string(&path).expect("read tsp api.json"))
        .expect("tsp api.json loads");

    let d = op_shapes(&derive);
    let t = op_shapes(&tsp);
    assert_eq!(
        d, t,
        "derive (src/api.rs) and typespec (api.tsp) must lower to the same ops"
    );
    println!("op-surface equivalence: {} ops match", d.len());
}
