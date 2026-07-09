//! The Slice 2 exporter bin: print the foreign-key graph's `catalog.json` to
//! stdout. `cargo fluessig emit --bin fluessig-emit-graph -o graph.json` runs it
//! and writes the output to a file, exactly like the Slice-1 exporter.

fn main() {
    print!("{}", derive_demo::graph::fluessig_catalog::to_json());
}
