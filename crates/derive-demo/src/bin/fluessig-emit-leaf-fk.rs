//! The Slice 8a Gap-1 exporter bin: print the direct-`Id<Leaf>`-FK catalog's
//! `catalog.json` to stdout. `cargo fluessig emit --bin fluessig-emit-leaf-fk -o
//! leaf_fk.json` runs it and writes the output to a file.

fn main() {
    print!("{}", derive_demo::leaf_fk::fluessig_catalog::to_json());
}
