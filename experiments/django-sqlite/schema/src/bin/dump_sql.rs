//! Render the genuinely **fluessig-owned** SQLite DDL for the issue-tracker
//! schema — the CREATE TABLE statements fluessig owns in mode (a) (managed=False).
//! Reads the committed `catalog.json` and calls `fluessig::sql::ddl` for
//! `Dialect::Sqlite`; the DDL is not hand-written.
//!
//! `cargo run --bin dump_sql > ../project/fluessig_schema.sql`

use fluessig::sql::{ddl, Dialect};

fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/catalog.json");
    let json = std::fs::read_to_string(path).expect("catalog.json present");
    let catalog = fluessig::catalog::load_catalog(&json).expect("catalog validates");
    print!("{}", ddl(&catalog, Dialect::Sqlite, None));
}
