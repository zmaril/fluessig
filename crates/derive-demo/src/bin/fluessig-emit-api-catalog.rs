//! The Slice 5 companion exporter bin: print the op-surface demo's
//! `catalog.json` (the entities the ops reference) to stdout, so
//! `cargo fluessig emit` can write both `catalog.json` and `api.json` from the
//! one `catalog!` root list.

fn main() {
    print!("{}", derive_demo::api::fluessig_catalog::to_json());
}
