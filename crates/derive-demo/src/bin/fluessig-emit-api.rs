//! The Slice 5 exporter bin: print the op-surface `api.json` to stdout.
//! `cargo fluessig emit --bin fluessig-emit-api --api-bin fluessig-emit-api`
//! (or directly `-o api.json`) runs it. Pairs with `fluessig-emit-api-catalog`,
//! which prints the `catalog.json` for the same demo.

fn main() {
    print!("{}", derive_demo::api::fluessig_catalog::api_to_json());
}
