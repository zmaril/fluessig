//! Print the demo's `catalog.json` (the entity roots the schema declares) to
//! stdout. `regen.sh` redirects it to `catalog.json`.

fn main() {
    print!("{}", cpp_demo::schema::fluessig_catalog::to_json());
}
