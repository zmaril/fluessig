//! Print the Java-demo schema's `api.json` (the op surface: the `Store`
//! interface with one op of every shape) to stdout — the file the Java backend
//! projects into Rust JNI glue + `.java` classes.

fn main() {
    print!("{}", java_demo_schema::fluessig_catalog::api_to_json());
}
