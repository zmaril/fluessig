//! The Slice 3 exporter bin: print the flatten + edge + shares catalog's
//! `catalog.json` to stdout. `cargo fluessig emit --bin fluessig-emit-advanced -o
//! advanced.json` runs it and writes the output to a file.

fn main() {
    print!("{}", derive_demo::advanced::fluessig_catalog::to_json());
}
