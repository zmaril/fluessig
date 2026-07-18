//! Slice 7 gate: the drift guard.
//!
//! `notes/derive-front-end-decisions.md` (Slice 7) — "The regenerate-validate-
//! diff `#[test]`, wired into CI the way the `node` drift job is today." And
//! `derive-front-end.md` §2.8: a ts-rs-style `#[test]` that "regenerates in
//! memory, runs the **full loader validation** … with file:line spans, and diffs
//! against the checked-in catalog. Schema errors fail `cargo test`; a stale
//! catalog fails with 're-run the exporter.'"
//!
//! This is the derive-front-end analogue of the existing TypeSpec drift job
//! (`emitter/test.mjs`: emit in memory, `JSON.stringify`-compare against the
//! committed fixture). The mechanism is an **in-test byte diff** rather than a
//! separate `cargo fluessig emit` + `git diff --exit-code` CI job, because:
//!
//!   * it is self-contained — it rides the existing `rust` CI job (`cargo test`,
//!     which already covers `crates/**`), needing no new workflow wiring, which
//!     is exactly the ts-rs pattern §2.8 cites; and
//!   * the `to_json()` exporter half is byte-deterministic (`serde_json`
//!     `to_string_pretty` over descriptor-ordered structs + a trailing newline),
//!     so a byte diff is exact — the committed fixtures were written straight
//!     from the `fluessig-emit*` bins and match those bins byte-for-byte.
//!
//! For every committed derive fixture it (1) REGENERATES the JSON in memory from
//! the derive source, (2) runs the FULL loader validation — the Slice-6 spanned
//! `validate_with_spans` for the catalog fixtures, `load_api` + a sibling-catalog
//! `load_catalog` for the api fixture — and (3) DIFFS the fresh output against
//! the checked-in file, FAILING with a message that names the drifted fixture
//! and says how to refresh it.

use std::fs;
use std::path::Path;

use fluessig::api::load_api;
use fluessig::load_catalog;
use fluessig_derive::{validate_with_spans, EdgeDescriptor, EntityDescriptor};

/// How a fixture's freshly-regenerated JSON is put through the full loader
/// validation before it is diffed against the checked-in file.
enum Validate {
    /// A `catalog.json` fixture: run the Slice-6 spanned loader validation
    /// (family rules, key arity, `shares()` compatibility, `extends`/root-list
    /// agreement) straight over the descriptors, so a schema error fails with a
    /// `file:line` diagnostic pointing at the offending `.rs` declaration.
    Catalog {
        name: &'static str,
        version: &'static str,
        entities: &'static [&'static EntityDescriptor],
        edges: &'static [&'static EdgeDescriptor],
    },
    /// The `api.json` op-surface fixture: load it through the op-layer loader,
    /// and load its sibling catalog too, so both halves the exporter writes from
    /// one `catalog!` root list are validated.
    Api { sibling_catalog: fn() -> String },
}

/// One committed derive fixture: where it lives, how to regenerate it, and how
/// to validate the regenerated form.
struct Fixture {
    /// Human label for the diagnostic when this fixture drifts.
    label: &'static str,
    /// The committed JSON, relative to the crate root (`CARGO_MANIFEST_DIR`).
    path: &'static str,
    /// Regenerate the JSON in memory from the derive source (the exporter half).
    regen: fn() -> String,
    /// The full-loader-validation strategy for the regenerated JSON.
    validate: Validate,
}

/// Every committed derive fixture the earlier slices produced. One table drives
/// one parametrized loop — a per-fixture copy of the body would trip
/// straitjacket's duplication gate (and rot independently).
fn fixtures() -> Vec<Fixture> {
    use derive_demo::{advanced, api, fluessig_catalog as user, graph, leaf_fk, poly};
    vec![
        Fixture {
            label: "catalog.json (Slice 1 — scalar entity)",
            path: "catalog.json",
            regen: user::to_json,
            validate: Validate::Catalog {
                name: user::NAME,
                version: user::VERSION,
                entities: user::ENTITIES,
                edges: user::EDGES,
            },
        },
        Fixture {
            label: "graph.json (Slice 2 — FK graph)",
            path: "graph.json",
            regen: graph::fluessig_catalog::to_json,
            validate: Validate::Catalog {
                name: graph::fluessig_catalog::NAME,
                version: graph::fluessig_catalog::VERSION,
                entities: graph::fluessig_catalog::ENTITIES,
                edges: graph::fluessig_catalog::EDGES,
            },
        },
        Fixture {
            label: "advanced.json (Slice 3 — flatten + edges + shares)",
            path: "advanced.json",
            regen: advanced::fluessig_catalog::to_json,
            validate: Validate::Catalog {
                name: advanced::fluessig_catalog::NAME,
                version: advanced::fluessig_catalog::VERSION,
                entities: advanced::fluessig_catalog::ENTITIES,
                edges: advanced::fluessig_catalog::EDGES,
            },
        },
        Fixture {
            label: "poly.json (Slice 4 — polymorphic families)",
            path: "poly.json",
            regen: poly::fluessig_catalog::to_json,
            validate: Validate::Catalog {
                name: poly::fluessig_catalog::NAME,
                version: poly::fluessig_catalog::VERSION,
                entities: poly::fluessig_catalog::ENTITIES,
                edges: poly::fluessig_catalog::EDGES,
            },
        },
        Fixture {
            label: "leaf_fk.json (Slice 8a Gap 1 — direct Id<Leaf> composite FK)",
            path: "leaf_fk.json",
            regen: leaf_fk::fluessig_catalog::to_json,
            validate: Validate::Catalog {
                name: leaf_fk::fluessig_catalog::NAME,
                version: leaf_fk::fluessig_catalog::VERSION,
                entities: leaf_fk::fluessig_catalog::ENTITIES,
                edges: leaf_fk::fluessig_catalog::EDGES,
            },
        },
        Fixture {
            label: "api.json (Slice 5 — op surface)",
            path: "api.json",
            regen: api::fluessig_catalog::api_to_json,
            validate: Validate::Api {
                sibling_catalog: api::fluessig_catalog::to_json,
            },
        },
    ]
}

/// Run the full loader validation for one fixture's freshly-regenerated JSON,
/// returning a rendered error (never a bare `Debug`) on failure.
fn run_validation(fresh: &str, v: &Validate) -> Result<(), String> {
    match v {
        Validate::Catalog {
            name,
            version,
            entities,
            edges,
        } => validate_with_spans(name, version, entities, edges)
            .map(|_| ())
            .map_err(|diags| {
                diags
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        Validate::Api { sibling_catalog } => {
            load_catalog(&sibling_catalog())
                .map_err(|e| format!("sibling catalog failed to load: {e}"))?;
            load_api(fresh).map(|_| ()).map_err(|e| e.to_string())
        }
    }
}

/// The drift guard. Regenerate every committed fixture, validate it through the
/// full loader, and diff it against the checked-in file.
#[test]
fn committed_fixtures_are_fresh_and_valid() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut drifted: Vec<&str> = Vec::new();

    for fx in fixtures() {
        // (1) regenerate the artifact in memory from the derive source.
        let fresh = (fx.regen)();

        // (2) run the FULL loader validation on the fresh artifact — a schema
        //     error fails `cargo test` here, with a `file:line` span for the
        //     catalog fixtures (Slice 6).
        run_validation(&fresh, &fx.validate)
            .unwrap_or_else(|e| panic!("[{}] failed loader validation:\n{e}", fx.label));

        // (3) diff the fresh artifact against the checked-in file.
        let path = crate_root.join(fx.path);
        let committed = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "[{}] cannot read committed fixture {}: {e}",
                fx.label,
                path.display()
            )
        });
        if committed != fresh {
            drifted.push(fx.path);
        }
    }

    assert!(
        drifted.is_empty(),
        "these committed derive fixtures have drifted from the derive source: {drifted:?}\n\
         Re-run the exporter to refresh them and commit the result, e.g.\n    \
         cargo run -p derive-demo --bin fluessig-emit          > crates/derive-demo/catalog.json\n    \
         cargo run -p derive-demo --bin fluessig-emit-graph    > crates/derive-demo/graph.json\n    \
         cargo run -p derive-demo --bin fluessig-emit-advanced > crates/derive-demo/advanced.json\n    \
         cargo run -p derive-demo --bin fluessig-emit-poly     > crates/derive-demo/poly.json\n    \
         cargo run -p derive-demo --bin fluessig-emit-leaf-fk  > crates/derive-demo/leaf_fk.json\n    \
         cargo run -p derive-demo --bin fluessig-emit-api      > crates/derive-demo/api.json",
    );
}
