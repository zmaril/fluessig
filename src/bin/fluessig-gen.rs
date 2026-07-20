//! `fluessig-gen <catalog.json> <out.rs> [--docs <p>] [--py-models <p>] [--ts-tables <p>] [--ts-drizzle <p>]
//! [--api <api.json> [--node <p>] [--node-dts <p>]
//! [--{node,python,ruby}-union-mode structured|envelope] [--{node,python,ruby}-union-tag <field>]
//! [--python <p>] [--ruby <p>] [--php <p>] [--mcp <p>]]`
//! — generate the committed artifacts: the Rust schema module, the docs
//! projection, the ORM read planes (see [`fluessig::sql`] / [`fluessig::codegen`]),
//! the binding surfaces, and the MCP module (see [`fluessig::bindgen`]).
//!
//! `--banner-note <text>` appends one extra comment line to every generated
//! file's banner — for consumers who want a marker in their generated code
//! (e.g. a lint-suppression line). Off by default: fluessig doesn't bake any
//! tool-specific markers into its output.
//!
//! Cross-package fan-out (opt-in): `--<lang>-out <pattern> --<lang>-mod-out
//! <root.rs>` splits a language's DTO surface into one file per pinned
//! `(package, module)` group (`{package}`/`{module}` substituted verbatim), a
//! shared `common.rs` (enums + un-pinned DTOs, emitted once), and the generated
//! ROOT module `<root.rs>` — a `#[path]` mod-tree + `pub use` re-exports that
//! make the split output compile as ONE crate. No group pins ⇒ nothing written.
//!
//! `--readme <template.md> --readme-out <pattern>` renders one Markdown template
//! per target language (see [`fluessig::readme`]). A `{lang}` in the pattern
//! fans out over `--readme-langs <slugs>` (default: all four); without it,
//! `--readme-lang <slug>` names the single target. `--readme-pkg <name>` sets
//! the package name in install lines (default: the catalog name, else `yourpkg`).
//!
//! straitjacket-allow-file:duplication — the catalog→enums extraction here mirrors
//! the one in fluessig's regen test (tests/entl_catalog.rs); both feed the bindgen.

/// Resolve a `--*-union-mode` flag (+ its `--*-union-tag`) into a
/// [`fluessig::bindgen::UnionProjection`]. The default (no flag) is now
/// `structured` tagged objects across every backend; `envelope` is the explicit
/// opt-out. Exits non-zero on an unknown mode.
fn union_projection(
    mode: Option<&str>,
    tag: Option<&str>,
    flag_name: &str,
) -> fluessig::bindgen::UnionProjection {
    use fluessig::bindgen::UnionProjection;
    match mode {
        None | Some("structured") => UnionProjection::Structured {
            tag_field: tag.unwrap_or("type").to_string(),
        },
        Some("envelope") => UnionProjection::Envelope,
        Some(other) => {
            eprintln!("{flag_name} must be `structured` or `envelope` (got `{other}`)");
            std::process::exit(2);
        }
    }
}

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
    let node_dts = flag("--node-dts");
    // How each backend lowers tagged unions: `structured` (DEFAULT — tagged
    // objects: napi `Either{N}` / PyO3 `#[pyclass]` variants / Magnus wrapped
    // classes) or `envelope` (the historical JSON-string carrier, an opt-out).
    // `--*-union-tag` names the discriminant field for structured mode (default
    // `type`, matching pi).
    let node_union_mode = flag("--node-union-mode");
    let node_union_tag = flag("--node-union-tag");
    let python_union_mode = flag("--python-union-mode");
    let python_union_tag = flag("--python-union-tag");
    let ruby_union_mode = flag("--ruby-union-mode");
    let ruby_union_tag = flag("--ruby-union-tag");
    let wasm_union_mode = flag("--wasm-union-mode");
    let wasm_union_tag = flag("--wasm-union-tag");
    let python = flag("--python");
    let ruby = flag("--ruby");
    let php = flag("--php");
    // The C/C++ backend: three single-file artifacts (no fan-out).
    let cpp = flag("--cpp");
    let cpp_header = flag("--cpp-header");
    let cpp_hpp = flag("--cpp-hpp");
    let wasm = flag("--wasm");
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
    let wasm_out = flag("--wasm-out");
    let mcp_out = flag("--mcp-out");
    // The generated ROOT module that ties a language's fanned group files into
    // one crate (the `#[path]` mod-tree + `pub use` re-exports + Python
    // `register()` re-collection). Required alongside the matching `--<lang>-out`
    // pattern: fan-out now emits a compilable crate, not orphan group files.
    let node_mod_out = flag("--node-mod-out");
    let python_mod_out = flag("--python-mod-out");
    let ruby_mod_out = flag("--ruby-mod-out");
    let php_mod_out = flag("--php-mod-out");
    let wasm_mod_out = flag("--wasm-mod-out");
    let mcp_mod_out = flag("--mcp-mod-out");
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
        || cpp.is_some()
        || cpp_header.is_some()
        || cpp_hpp.is_some()
        || wasm.is_some()
        || mcp.is_some()
        || node_out.is_some()
        || python_out.is_some()
        || ruby_out.is_some()
        || php_out.is_some()
        || wasm_out.is_some()
        || mcp_out.is_some();
    if want_bindings {
        let Some(ap) = api_path.as_deref() else {
            eprintln!("--node/--python/--ruby/--php/--cpp/--cpp-header/--cpp-hpp/--mcp (and their --*-out fan-out) require --api <api.json>");
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
            // `--node-dts <path>` fronts the package's public typings with a
            // hand-written `.d.ts` (napi can't express `A | B`): the generated
            // addon suppresses union `ts_return_type` hints, and the supplied
            // file is reference-copied next to the emitted artifact so the
            // package's `types` resolves to it.
            let opts = fluessig::bindgen::NodeOptions {
                union_projection: union_projection(
                    node_union_mode.as_deref(),
                    node_union_tag.as_deref(),
                    "--node-union-mode",
                ),
                external_dts: node_dts.clone(),
            };
            write(
                &p,
                fluessig::bindgen::node_binding_with_options(&api, &enums, note, &opts),
            );
            if let Some(dts) = &node_dts {
                let dest = std::path::Path::new(&p)
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join(
                        std::path::Path::new(dts)
                            .file_name()
                            .unwrap_or_else(|| std::ffi::OsStr::new("index.d.ts")),
                    );
                std::fs::copy(dts, &dest).unwrap_or_else(|e| {
                    eprintln!("copy {dts} -> {}: {e}", dest.display());
                    std::process::exit(1);
                });
                println!("copied external .d.ts {dts} -> {}", dest.display());
            }
        }
        if let Some(py) = python {
            let opts = fluessig::bindgen::PythonOptions {
                union_projection: union_projection(
                    python_union_mode.as_deref(),
                    python_union_tag.as_deref(),
                    "--python-union-mode",
                ),
            };
            write(
                &py,
                fluessig::bindgen::python_binding_with_options(&api, &enums, note, &opts),
            );
        }
        if let Some(rb) = ruby {
            let opts = fluessig::bindgen::RubyOptions {
                union_projection: union_projection(
                    ruby_union_mode.as_deref(),
                    ruby_union_tag.as_deref(),
                    "--ruby-union-mode",
                ),
            };
            write(
                &rb,
                fluessig::bindgen::ruby_binding_with_options(&api, &enums, note, &opts),
            );
        }
        if let Some(p) = php {
            write(&p, fluessig::bindgen::php_binding(&api, &enums, note));
        }
        // ── C/C++ backend (single-file only — no package/module fan-out) ──
        if let Some(p) = cpp {
            write(&p, fluessig::bindgen::cpp_binding(&api, &enums, note));
        }
        if let Some(p) = cpp_header {
            // Plain C text — not Rust, so it is written as-is (no rustfmt).
            write(&p, fluessig::bindgen::cpp_header(&api, &enums, note));
        }
        if let Some(p) = cpp_hpp {
            // Plain C++ text — not Rust, so it is written as-is (no rustfmt).
            write(&p, fluessig::bindgen::cpp_hpp(&api, &enums, note));
        }
        if let Some(p) = wasm {
            let opts = fluessig::bindgen::WasmOptions {
                union_projection: union_projection(
                    wasm_union_mode.as_deref(),
                    wasm_union_tag.as_deref(),
                    "--wasm-union-mode",
                ),
            };
            write(
                &p,
                fluessig::bindgen::wasm_binding_with_options(&api, &enums, note, opts),
            );
        }
        if let Some(m) = mcp {
            write(&m, fluessig::bindgen::mcp_module(&api, &enums, note));
        }

        // ── opt-in package/module fan-out (cross-package import subsystem) ──
        // One file per distinct `(package, module)` group the schema pins for the
        // language, PLUS a shared `common` file (enums + un-pinned DTOs, emitted
        // once) and a generated ROOT module. Each group file carries `use
        // crate::<sanitized-path>::Symbol;` imports for its cross-group refs; the
        // root binds each verbatim on-disk path to a valid Rust `mod` via
        // `#[path]` and `pub use`-re-exports the flat surface — so the split
        // output COMPILES. A language with no group pins produces nothing.
        let write_p = |path: &str, content: String| {
            if let Some(dir) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(dir).unwrap_or_else(|e| {
                    eprintln!("mkdir {}: {e}", dir.display());
                    std::process::exit(1);
                });
            }
            write(path, content);
        };
        let fan = |lang: &str,
                   pattern: Option<String>,
                   mod_out: Option<String>,
                   render: &dyn Fn(
            &fluessig::api::ApiDoc,
            &[fluessig::bindgen::EnumDesc],
        ) -> String| {
            let Some(pat) = pattern else {
                if mod_out.is_some() {
                    eprintln!("--{lang}-mod-out requires --{lang}-out <pattern>");
                    std::process::exit(2);
                }
                return;
            };
            let Some(mo) = mod_out else {
                eprintln!(
                    "--{lang}-out requires --{lang}-mod-out <root.rs> (the root module that ties the fanned files into one crate)"
                );
                std::process::exit(2);
            };
            let common = fluessig::bindgen::common_path_for(&mo);
            let spec = fluessig::bindgen::FanOutSpec {
                lang,
                pattern: &pat,
                mod_out: &mo,
                common_out: &common,
                note,
                mcp_models_only: lang == "mcp",
            };
            match fluessig::bindgen::fan_out_crate(&api, &enums, &spec, render) {
                Ok(None) => eprintln!(
                    "note: --{lang}-out given but the schema carries no {lang} package/module pins; nothing fanned out"
                ),
                Ok(Some(fc)) => {
                    for (path, content) in fc.group_files {
                        write_p(&path, content);
                    }
                    if let Some((path, content)) = fc.common_file {
                        write_p(&path, content);
                    }
                    let (path, content) = fc.root_file;
                    write_p(&path, content);
                }
                Err(e) => {
                    eprintln!("fan-out error ({lang}): {e}");
                    std::process::exit(1);
                }
            }
        };
        fan("node", node_out, node_mod_out, &|a, e| {
            fluessig::bindgen::node_binding(a, e, note)
        });
        fan("python", python_out, python_mod_out, &|a, e| {
            fluessig::bindgen::python_binding(a, e, note)
        });
        fan("ruby", ruby_out, ruby_mod_out, &|a, e| {
            fluessig::bindgen::ruby_binding(a, e, note)
        });
        fan("php", php_out, php_mod_out, &|a, e| {
            fluessig::bindgen::php_binding(a, e, note)
        });
        fan("wasm", wasm_out, wasm_mod_out, &|a, e| {
            fluessig::bindgen::wasm_binding(a, e, note)
        });
        fan("mcp", mcp_out, mcp_mod_out, &|a, e| {
            fluessig::bindgen::mcp_module(a, e, note)
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
