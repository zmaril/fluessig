//! Exporter bin for the `#[fluessig(single_threaded)]` demo: print its op-surface
//! `api.json` (the `Tui` interface carries `"single_threaded": true`) to stdout.

fn main() {
    print!(
        "{}",
        derive_demo::single_threaded::fluessig_catalog::api_to_json()
    );
}
