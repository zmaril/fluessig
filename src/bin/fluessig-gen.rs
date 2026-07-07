//! `fluessig-gen <catalog.json> <out.rs> [--docs <p>] [--py-models <p>] [--ts-tables <p>] [--ts-drizzle <p>]`
//! — generate the committed artifacts: the Rust schema module, the docs
//! projection, and the ORM read planes (see [`fluessig::sql`] / [`fluessig::codegen`]).
//!
//! `--banner-note <text>` appends one extra comment line to every generated
//! file's banner — for consumers who want a marker in their generated code
//! (e.g. a lint-suppression line). Off by default: fluessig doesn't bake any
//! tool-specific markers into its output.
//!
//! straitjacket-allow-file:duplication — the catalog→enums extraction here mirrors
//! the one in fluessig's regen test (tests/entl_catalog.rs); both feed the bindgen.

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut flag = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).map(|i| {
            args.remove(i);
            args.remove(i)
        })
    };
    let docs = flag("--docs");
    let api_path = flag("--api");
    let node = flag("--node");
    let python = flag("--python");
    let ruby = flag("--ruby");
    let py_models = flag("--py-models");
    let ts_tables = flag("--ts-tables");
    let ts_drizzle = flag("--ts-drizzle");
    let banner_note = flag("--banner-note");
    let note = banner_note.as_deref();
    let [catalog_path, out_path] = args.as_slice() else {
        eprintln!(
            "usage: fluessig-gen <catalog.json> <out.rs> [--docs <p>] [--py-models <p>] [--ts-tables <p>] [--ts-drizzle <p>] [--banner-note <text>]"
        );
        std::process::exit(2);
    };
    let catalog = fluessig::load_catalog_file(catalog_path).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    let write = |path: &str, content: String| {
        std::fs::write(path, &content).unwrap_or_else(|e| {
            eprintln!("write {path}: {e}");
            std::process::exit(1);
        });
        println!("wrote {path} ({} bytes)", content.len());
    };

    write(out_path, fluessig::sql::rust_schema_module(&catalog, note));
    if let Some(p) = docs {
        let json = fluessig::sql::schema_docs_json(&catalog, fluessig::sql::Dialect::Duckdb);
        write(&p, format!("{json}\n"));
    }
    if let Some(p) = py_models {
        write(&p, fluessig::codegen::python_models(&catalog, note));
    }
    if let Some(p) = ts_tables {
        write(&p, fluessig::codegen::ts_tables(&catalog, note));
    }
    if let Some(p) = ts_drizzle {
        write(&p, fluessig::codegen::ts_drizzle(&catalog, note));
    }
    if let Some(p) = node {
        let Some(ap) = api_path.as_deref() else {
            eprintln!("--node requires --api <api.json>");
            std::process::exit(2);
        };
        let api = fluessig::api::load_api_file(ap).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });
        // name-only enums from the catalog become napi enums; wire-valued ones are strings
        let enums: Vec<(String, Vec<String>)> = catalog
            .enums
            .iter()
            .map(|e| {
                (
                    e.name.clone(),
                    e.variants.iter().map(|v| v.name.clone()).collect(),
                )
            })
            .collect();
        write(&p, fluessig::bindgen::node_binding(&api, &enums, note));
        if let Some(py) = python {
            write(&py, fluessig::bindgen::python_binding(&api, &enums, note));
        }
        if let Some(rb) = ruby {
            write(&rb, fluessig::bindgen::ruby_binding(&api, &enums, note));
        }
    }
}
