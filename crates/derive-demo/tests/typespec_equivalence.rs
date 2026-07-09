//! Slice 3 semantic-equivalence check: the derive-authored catalog
//! (`src/advanced.rs`) and the TypeSpec-authored one (`advanced.tsp`) project to
//! the **same physical tables** — same table names, columns (name + type +
//! nullability), and primary keys — even though they spell inheritance, edges,
//! and column sharing differently at the front end (flatten vs `extends`, edge
//! struct vs `@edge`, `shares(col)` vs twin `@fk`/`@fkSource` naming).
//!
//! The TypeSpec catalog is produced out-of-band by the Node emitter
//! (`node emitter/emit.mjs crates/derive-demo/advanced.tsp --out <dir>`), so this
//! test only runs when its path is provided:
//!
//! ```sh
//! TMP=$(mktemp -d)
//! node emitter/emit.mjs crates/derive-demo/advanced.tsp --out "$TMP"
//! FLUESSIG_TSP_CATALOG="$TMP/catalog.json" cargo test -p derive-demo --test typespec_equivalence -- --nocapture
//! ```
//!
//! Without the env var it prints a skip note and passes, so CI (which has no Node
//! toolchain on the derive-crate job) stays green while the equivalence stays
//! reproducible on demand.

use fluessig::load_catalog;
use fluessig::sql::{tables, Dialect, TableDef};
use std::collections::BTreeMap;

/// (table name) → ordered (column name, type, not_null) + pk, for set comparison.
type Shape = BTreeMap<String, (Vec<(String, String, bool)>, Vec<String>)>;

fn shape_of(catalog: &fluessig::ir::Catalog) -> Shape {
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

fn dump(label: &str, shape: &Shape) {
    println!("── {label} ──");
    for (table, (cols, pk)) in shape {
        println!("  {table}  PK({})", pk.join(", "));
        for (n, ty, nn) in cols {
            println!("    {n}: {ty}{}", if *nn { " NOT NULL" } else { "" });
        }
    }
}

#[test]
fn derive_and_typespec_project_to_the_same_tables() {
    let derive_json = derive_demo::advanced::fluessig_catalog::to_json();
    let derive = load_catalog(&derive_json).expect("derive catalog loads");
    let derive_shape = shape_of(&derive);
    dump("derive (src/advanced.rs)", &derive_shape);

    let Ok(tsp_path) = std::env::var("FLUESSIG_TSP_CATALOG") else {
        println!(
            "\nFLUESSIG_TSP_CATALOG not set — skipping the TypeSpec side.\n\
             Emit it with `node emitter/emit.mjs crates/derive-demo/advanced.tsp --out <dir>`\n\
             and re-run with FLUESSIG_TSP_CATALOG=<dir>/catalog.json to compare."
        );
        return;
    };

    let tsp_json = std::fs::read_to_string(&tsp_path).expect("read TypeSpec catalog");
    let tsp = load_catalog(&tsp_json).expect("TypeSpec catalog loads");
    let tsp_shape = shape_of(&tsp);
    dump("typespec (advanced.tsp)", &tsp_shape);

    assert_eq!(
        derive_shape, tsp_shape,
        "derive and TypeSpec front ends must project to identical physical tables"
    );
    println!("\n✓ identical physical projection across both front ends");
}
