//! The Slice 4 exporter bin: print the polymorphic-families catalog's
//! `catalog.json` to stdout. `cargo fluessig emit --bin fluessig-emit-poly -o
//! poly.json` runs it and writes the output to a file.

fn main() {
    print!("{}", derive_demo::poly::fluessig_catalog::to_json());
}
