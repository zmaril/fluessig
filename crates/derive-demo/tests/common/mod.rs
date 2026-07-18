//! Shared helpers for the derive-demo gate tests — physical-projection
//! inspection (`col`) and the shape reduction the equivalence gates compare
//! (`Shape` / `shape_of` / `dump`). Kept in one place so each per-slice gate
//! doesn't re-spell the same lookups (which the copy-paste gate would flag).
//!
//! Each test binary `mod common;`-includes this and uses a subset, so
//! `dead_code` is expected here — allowed module-wide rather than per item.
#![allow(dead_code)]

use fluessig::load_catalog;
use fluessig::sql::{tables, ColumnDef, Dialect, TableDef};
use std::collections::BTreeMap;

/// A named column of a table, or a panic naming what's missing.
pub fn col<'a>(t: &'a TableDef, n: &str) -> &'a ColumnDef {
    t.columns
        .iter()
        .find(|c| c.name == n)
        .unwrap_or_else(|| panic!("column {n} missing from {}", t.name))
}

/// (table name) → ordered (column name, type, not_null) + pk, for set comparison.
pub type Shape = BTreeMap<String, (Vec<(String, String, bool)>, Vec<String>)>;

/// The Postgres physical projection of a catalog, reduced to a comparable shape.
pub fn shape_of(catalog: &fluessig::ir::Catalog) -> Shape {
    tables(catalog, Dialect::Postgres)
        .into_iter()
        .map(|(name, t): (String, TableDef)| {
            let cols = t
                .columns
                .iter()
                .map(|c| (c.name.clone(), c.ty.clone(), c.not_null))
                .collect();
            (name, (cols, t.pk.clone()))
        })
        .collect()
}

/// Print a shape — a debug aid for the equivalence gates run with `--nocapture`.
pub fn dump(label: &str, shape: &Shape) {
    println!("── {label} ──");
    for (table, (cols, pk)) in shape {
        println!("  {table}  PK({})", pk.join(", "));
        for (n, ty, nn) in cols {
            println!("    {n}: {ty}{}", if *nn { " NOT NULL" } else { "" });
        }
    }
}

/// The TypeSpec semantic-equivalence gate body, shared by the Slice-3
/// (`advanced`) and Slice-4 (`poly`) checks: load the derive catalog and, when
/// the TypeSpec catalog path is provided via `tsp_env`, assert both front ends
/// project to identical physical tables. Without the env var it prints a skip
/// note and returns, so CI (which has no Node toolchain on the derive-crate job)
/// stays green while the equivalence stays reproducible on demand.
pub fn assert_typespec_equivalent(
    derive_json: &str,
    tsp_env: &str,
    derive_label: &str,
    tsp_label: &str,
) {
    let derive = load_catalog(derive_json).expect("derive catalog loads");
    let derive_shape = shape_of(&derive);
    dump(derive_label, &derive_shape);

    let Ok(tsp_path) = std::env::var(tsp_env) else {
        println!(
            "\n{tsp_env} not set — skipping the TypeSpec side.\n\
             Emit it with `node emitter/emit.mjs <the .tsp> --out <dir>` and re-run\n\
             with {tsp_env}=<dir>/catalog.json to compare."
        );
        return;
    };

    let tsp_json = std::fs::read_to_string(&tsp_path).expect("read TypeSpec catalog");
    let tsp = load_catalog(&tsp_json).expect("TypeSpec catalog loads");
    let tsp_shape = shape_of(&tsp);
    dump(tsp_label, &tsp_shape);

    assert_eq!(
        derive_shape, tsp_shape,
        "derive and TypeSpec front ends must project to identical physical tables"
    );
    println!("\n✓ identical physical projection across both front ends");
}
