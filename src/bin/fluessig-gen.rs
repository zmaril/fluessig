//! `fluessig-gen <catalog.json> <out.rs> [--docs <p>] [--py-models <p>] [--ts-tables <p>] [--ts-drizzle <p>]
//! [--api <api.json> [--node <p>] [--python <p>] [--ruby <p>] [--php <p>] [--mcp <p>]]`
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
    let php = flag("--php");
    let mcp = flag("--mcp");
    // Opt-in package/module fan-out: a patterned path like
    // `out/{package}/{module}.rs`. When the api schema carries `(package,
    // module)` group pins for the language, symbols partition into one file per
    // distinct group, `{package}`/`{module}` substituted VERBATIM. No grouping
    // pins ⇒ nothing written here; the single-file `--<lang>` path is untouched.
    let node_out = flag("--node-out");
    let python_out = flag("--python-out");
    let ruby_out = flag("--ruby-out");
    let php_out = flag("--php-out");
    let mcp_out = flag("--mcp-out");
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
    let want_bindings = node.is_some()
        || python.is_some()
        || ruby.is_some()
        || php.is_some()
        || mcp.is_some()
        || node_out.is_some()
        || python_out.is_some()
        || ruby_out.is_some()
        || php_out.is_some()
        || mcp_out.is_some();
    if want_bindings {
        let Some(ap) = api_path.as_deref() else {
            eprintln!("--node/--python/--ruby/--php/--mcp (and their --*-out fan-out) require --api <api.json>");
            std::process::exit(2);
        };
        let api = fluessig::api::load_api_file(ap).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });
        // The shared per-variant enum form every backend consumes: the catalog
        // member name, the neutral `Variant.value` wire override (when a string),
        // and the per-language export-name pins. Each backend resolves its own
        // token / rename through `bindgen::variant_token` / `pinned_name`.
        let enums: Vec<fluessig::bindgen::EnumDesc> = catalog
            .enums
            .iter()
            .map(|e| {
                (
                    e.name.clone(),
                    e.variants
                        .iter()
                        .map(|v| fluessig::bindgen::EnumVariant {
                            name: v.name.clone(),
                            value: v
                                .value
                                .as_ref()
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            bindings: v.bindings.clone(),
                        })
                        .collect(),
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
        if let Some(p) = php {
            write(&p, fluessig::bindgen::php_binding(&api, &enums, note));
        }
        if let Some(m) = mcp {
            write(&m, fluessig::bindgen::mcp_module(&api, &enums, note));
        }

        // ── opt-in package/module fan-out ──
        // One file per distinct `(package, module)` group the schema pins for
        // the language; `{package}`/`{module}` substituted verbatim. A language
        // with no group pins produces nothing. See `bindgen::fan_out`'s KNOWN
        // LIMITATION: group files are the DTO surface and cross-group references
        // are not yet import-resolved, so this stays strictly opt-in.
        let fan = |lang: &str,
                   pattern: Option<String>,
                   render: &dyn Fn(&fluessig::api::ApiDoc) -> String| {
            if let Some(pat) = pattern {
                let groups = fluessig::bindgen::fan_out(&api, lang, &pat);
                if groups.is_empty() {
                    eprintln!("note: --{lang}-out given but the schema carries no {lang} package/module pins; nothing fanned out");
                }
                for (path, sub) in groups {
                    if let Some(dir) = std::path::Path::new(&path).parent() {
                        std::fs::create_dir_all(dir).unwrap_or_else(|e| {
                            eprintln!("mkdir {}: {e}", dir.display());
                            std::process::exit(1);
                        });
                    }
                    write(&path, render(&sub));
                }
            }
        };
        fan("node", node_out, &|a| {
            fluessig::bindgen::node_binding(a, &enums, note)
        });
        fan("python", python_out, &|a| {
            fluessig::bindgen::python_binding(a, &enums, note)
        });
        fan("ruby", ruby_out, &|a| {
            fluessig::bindgen::ruby_binding(a, &enums, note)
        });
        fan("php", php_out, &|a| {
            fluessig::bindgen::php_binding(a, &enums, note)
        });
        fan("mcp", mcp_out, &|a| {
            fluessig::bindgen::mcp_module(a, &enums, note)
        });
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
