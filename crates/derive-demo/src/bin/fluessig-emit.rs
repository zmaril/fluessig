//! The exporter bin: print this crate's `catalog.json` to stdout.
//!
//! `derive-front-end.md` §2.8 describes the exporter as "a bin target (or
//! `cargo fluessig emit`) that writes `catalog.json`". This is that bin; the
//! `cargo-fluessig` crate's `cargo fluessig emit` subcommand runs it and writes
//! the output to a file.

fn main() {
    print!("{}", derive_demo::fluessig_catalog::to_json());
}
