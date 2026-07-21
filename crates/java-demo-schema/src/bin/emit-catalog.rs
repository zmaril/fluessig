//! Print the Java-demo schema's `catalog.json` (entities + the `Item` DTO) to
//! stdout — the entity/enum side `fluessig-gen` reads for DTO/enum extraction.

fn main() {
    print!("{}", java_demo_schema::fluessig_catalog::to_json());
}
