//! Print the issue-tracker `catalog.json` to stdout:
//! `cargo run --bin emit > catalog.json`.

fn main() {
    print!("{}", django_demo_schema::fluessig_catalog::to_json());
}
