//! `fluessig-gen <catalog.json> <out.rs> [--docs <p>] [--py-models <p>] [--ts-tables <p>] [--ts-drizzle <p>]
//! [--api <api.json> [--node <p>] [--python <p>] [--ruby <p>] [--mcp <p>]]`
//! — generate the committed artifacts: the Rust schema module, the docs
//! projection, the ORM read planes (see [`fluessig::sql`] / [`fluessig::codegen`]),
//! the binding surfaces, and the MCP module (see [`fluessig::bindgen`]).
//!
//! `--banner-note <text>` appends one extra comment line to every generated
//! file's banner — for consumers who want a marker in their generated code
//! (e.g. a lint-suppression line). Off by default: fluessig doesn't bake any
//! tool-specific markers into its output.
//!
//! `--readme <template.md> --readme-out <pattern>` renders one Markdown template
//! per target language (see [`fluessig::readme`]). A `{lang}` in the pattern
//! fans out over `--readme-langs <slugs>` (default: all four); without it,
//! `--readme-lang <slug>` names the single target. `--readme-pkg <name>` sets
//! the package name in install lines (default: the catalog name, else `yourpkg`).
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
    let mcp = flag("--mcp");
    let py_models = flag("--py-models");
    let ts_tables = flag("--ts-tables");
    let ts_drizzle = flag("--ts-drizzle");
    let readme = flag("--readme");
    let readme_out = flag("--readme-out");
    let readme_lang = flag("--readme-lang");
    let readme_langs = flag("--readme-langs");
    let readme_pkg = flag("--readme-pkg");
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
    if node.is_some() || python.is_some() || ruby.is_some() || mcp.is_some() {
        let Some(ap) = api_path.as_deref() else {
            eprintln!("--node/--python/--ruby/--mcp require --api <api.json>");
            std::process::exit(2);
        };
        let api = fluessig::api::load_api_file(ap).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });
        // name-only enums from the catalog become napi string enums (snake_case
        // wire tokens); wire-valued ones are plain strings
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
        if let Some(p) = node {
            write(&p, fluessig::bindgen::node_binding(&api, &enums, note));
        }
        if let Some(py) = python {
            write(&py, fluessig::bindgen::python_binding(&api, &enums, note));
        }
        if let Some(rb) = ruby {
            write(&rb, fluessig::bindgen::ruby_binding(&api, &enums, note));
        }
        if let Some(m) = mcp {
            write(&m, fluessig::bindgen::mcp_module(&api, &enums, note));
        }
    }

    if let Some(tpl_path) = readme {
        render_readme(
            &catalog,
            &tpl_path,
            readme_out.as_deref(),
            readme_lang.as_deref(),
            readme_langs.as_deref(),
            readme_pkg.as_deref(),
            &write,
        );
    }
}

/// Render a README template per target language and write each output. Errors
/// (bad flags, template parse/render) print a message and exit non-zero.
#[allow(clippy::too_many_arguments)]
fn render_readme(
    catalog: &fluessig::Catalog,
    tpl_path: &str,
    out: Option<&str>,
    lang: Option<&str>,
    langs: Option<&str>,
    pkg: Option<&str>,
    write: &impl Fn(&str, String),
) {
    use fluessig::readme;

    let fail = |msg: String| -> ! {
        eprintln!("{msg}");
        std::process::exit(2);
    };

    let Some(out_pattern) = out else {
        fail("--readme requires --readme-out <pattern>".into());
    };
    let template =
        std::fs::read_to_string(tpl_path).unwrap_or_else(|e| fail(format!("read {tpl_path}: {e}")));

    let catalog_name = catalog
        .source
        .as_deref()
        .map(readme::catalog_name_from_source);
    let pkg = pkg
        .map(str::to_string)
        .or_else(|| catalog_name.clone())
        .unwrap_or_else(|| "yourpkg".to_string());
    let ctx = readme::RenderCtx { pkg, catalog_name };

    // Resolve a slug list to registry languages, exiting on an unknown slug.
    let resolve = |csv: &str| -> Vec<&'static readme::Language> {
        csv.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                readme::by_slug(s)
                    .unwrap_or_else(|| fail(format!("unknown --readme language slug: {s}")))
            })
            .collect()
    };

    let targets: Vec<&readme::Language> = if out_pattern.contains("{lang}") {
        if lang.is_some() {
            fail("--readme-lang is for a single-file --readme-out; use --readme-langs with a {lang} pattern".into());
        }
        match langs {
            Some(csv) => resolve(csv),
            None => readme::languages().iter().collect(),
        }
    } else {
        if langs.is_some() {
            fail("--readme-langs needs a {lang} pattern in --readme-out; use --readme-lang for a single file".into());
        }
        let slug = lang.unwrap_or_else(|| {
            fail("--readme-out without {lang} needs --readme-lang <slug>".into())
        });
        vec![readme::by_slug(slug)
            .unwrap_or_else(|| fail(format!("unknown --readme-lang slug: {slug}")))]
    };

    let rendered = readme::render_files(&template, out_pattern, &targets, &ctx)
        .unwrap_or_else(|e| fail(format!("render {tpl_path}: {e}")));
    for (path, content) in rendered {
        write(&path, content);
    }
}
