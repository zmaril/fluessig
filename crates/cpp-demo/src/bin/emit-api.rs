//! Print the demo's op-surface `api.json` to stdout. `regen.sh` redirects it to
//! `api.json`, then feeds it to `fluessig-gen --api`.

fn main() {
    print!("{}", cpp_demo::schema::fluessig_catalog::api_to_json());
}
