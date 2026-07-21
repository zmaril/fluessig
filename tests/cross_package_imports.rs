//! The cross-package import subsystem for fan-out (PR #41 follow-on): making
//! fanned-out multi-file bindings ACTUALLY COMPILE instead of referencing
//! undefined bare names across group files.
//!
//! Coverage:
//!  * sanitization: `(package, module)` npm/`.d.ts` coordinates → a valid,
//!    deterministic, total Rust module path (incl. the five real pi pins);
//!  * collision detection: two DISTINCT pins onto one Rust path is a hard error;
//!  * enum-home dedup: an enum referenced by DTOs in two groups is defined ONCE
//!    (in `common`) and imported into each group;
//!  * the exhaustive external-ref walk, including #40's structured unions;
//!  * THE acceptance bar — a HERMETIC compile proof: the fanned tree + root
//!    mod-tree is stripped to dep-free Rust and compiled with a local `rustc`
//!    (offline, no napi/pyo3/magnus), asserting exit 0; a NEGATIVE CONTROL
//!    (same tree WITHOUT the generated imports) is asserted to FAIL to compile.

use std::collections::BTreeSet;
use std::process::Command;

use fluessig::api::{load_api, ApiDoc};
use fluessig::bindgen::{
    common_path_for, external_refs, fan_out_crate, group_module_path, group_table, node_binding,
    render_use_block, resolve_module_paths, symbol_groups, EnumDesc, EnumVariant, FanOutSpec,
    GroupKey, SymbolGroup,
};

// ── sanitization: pin → Rust module path ─────────────────────────────────────

fn pinned(package: &str, module: &str) -> GroupKey {
    GroupKey::Pinned {
        package: package.to_string(),
        module: module.to_string(),
    }
}

#[test]
fn common_projects_to_the_reserved_common_module() {
    assert_eq!(group_module_path(&GroupKey::Common), vec!["common"]);
}

#[test]
fn sanitizes_the_five_real_pi_pins_to_expected_module_paths() {
    // The five canonical `@earendil-works/pi-*` packages + their deep `../src/*`
    // modules — the real pi vendored coordinates — each project deterministically.
    let cases: [(&str, &str, &[&str]); 5] = [
        (
            "@earendil-works/pi-agent-core",
            "../src/agent",
            &["earendil_works", "pi_agent_core", "src", "agent"],
        ),
        (
            "@earendil-works/pi-ai",
            "../src/ai",
            &["earendil_works", "pi_ai", "src", "ai"],
        ),
        (
            "@earendil-works/pi-coding-agent",
            "../src/coding-agent",
            &["earendil_works", "pi_coding_agent", "src", "coding_agent"],
        ),
        (
            "@earendil-works/pi-orchestrator",
            "../src/orchestrator",
            &["earendil_works", "pi_orchestrator", "src", "orchestrator"],
        ),
        (
            "@earendil-works/pi-tui",
            "../src/tui",
            &["earendil_works", "pi_tui", "src", "tui"],
        ),
    ];
    for (package, module, expected) in cases {
        assert_eq!(
            group_module_path(&pinned(package, module)),
            expected.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "pin ({package}, {module})"
        );
    }
}

#[test]
fn sanitization_drops_artifacts_and_guards_leading_digits() {
    // `.`/`..`/empty segments dropped; `@scope` sigil stripped; a leading digit
    // gets a `_` prefix; non-alnum runs collapse to a single `_`.
    assert_eq!(
        group_module_path(&pinned("@scope/3d-pkg", "./a//b/../c")),
        vec!["scope", "_3d_pkg", "a", "b", "c"]
    );
    // deterministic + total: same input, same output.
    assert_eq!(
        group_module_path(&pinned("@a/b", "c")),
        group_module_path(&pinned("@a/b", "c"))
    );
}

// ── collision detection ──────────────────────────────────────────────────────

#[test]
fn distinct_pins_that_map_to_one_rust_path_are_a_hard_error() {
    // `a.b` and `a-b` both sanitize to the ident `a_b` → the same Rust module
    // path — distinct npm coordinates must NOT be silently merged.
    let groups = symbol_groups(
        &load_api(
            r#"{"fluessig":{"format":1},"models":[
              {"name":"X","fields":[{"name":"n","type":"int32","nullable":false}],"bindings":{"node":{"package":"a.b","module":""}}},
              {"name":"Y","fields":[{"name":"n","type":"int32","nullable":false}],"bindings":{"node":{"package":"a-b","module":""}}}
            ],"interfaces":[]}"#,
        )
        .unwrap(),
        "node",
    );
    let err = resolve_module_paths(&groups).expect_err("collision must be an error");
    assert!(
        err.contains("COLLISION") && err.contains("a_b"),
        "clear collision message: {err}"
    );
}

#[test]
fn a_pin_of_only_artifacts_is_an_empty_path_error() {
    let groups = symbol_groups(
        &load_api(
            r#"{"fluessig":{"format":1},"models":[
              {"name":"X","fields":[{"name":"n","type":"int32","nullable":false}],"bindings":{"node":{"package":".","module":".."}}}
            ],"interfaces":[]}"#,
        )
        .unwrap(),
        "node",
    );
    let err = resolve_module_paths(&groups).expect_err("empty path must be an error");
    assert!(err.contains("EMPTY"), "clear empty-path message: {err}");
}

#[test]
fn the_five_pi_pins_do_not_collide() {
    // The five canonical `@earendil-works/pi-*` packages + their deep `../src/*`
    // modules resolve to five distinct Rust module paths (no collision).
    let pins = [
        ("@earendil-works/pi-agent-core", "../src/agent"),
        ("@earendil-works/pi-ai", "../src/ai"),
        ("@earendil-works/pi-coding-agent", "../src/coding-agent"),
        ("@earendil-works/pi-orchestrator", "../src/orchestrator"),
        ("@earendil-works/pi-tui", "../src/tui"),
    ];
    let groups: Vec<SymbolGroup> = pins
        .iter()
        .map(|(p, m)| SymbolGroup {
            package: p.to_string(),
            module: m.to_string(),
            symbols: vec!["X".to_string()],
        })
        .collect();
    let resolved = resolve_module_paths(&groups).expect("no collision among the five pi pins");
    assert_eq!(resolved.len(), 5);
}

// ── external refs (incl. #40 structured unions) ──────────────────────────────

#[test]
fn external_refs_are_the_cross_group_models_and_nonstring_enums() {
    let api = load_api(CROSS_GROUP_API).unwrap();
    let enums = flavor_enum();
    let table = group_table(&api, &enums, "node");
    // The `alpha` group holds `Message`, which references `Account` (beta) and
    // the `Flavor` enum (common) — both cross-group.
    let alpha = pinned("@acme/alpha", "../src/alpha");
    let sub = sub_for(&api, &["Message"]);
    let refs = external_refs(&sub, &alpha, &table);
    let got: BTreeSet<(String, String)> = refs
        .iter()
        .map(|r| (r.symbol.clone(), r.module_path.join("::")))
        .collect();
    assert!(
        got.contains(&("Account".into(), "acme::beta::src::beta".into())),
        "cross-group model ref imported: {got:?}"
    );
    assert!(
        got.contains(&("Flavor".into(), "common".into())),
        "shared enum ref imported from common: {got:?}"
    );
}

#[test]
fn external_refs_walk_structured_union_member_types() {
    // #40: a union DEFINITION names its member models as bare Rust idents. A union
    // whose member model is homed in another group must surface that member as a
    // cross-group ref (the exhaustive walk the op layer will also need later).
    let api = load_api(
        r#"{"fluessig":{"format":1},
          "models":[
            {"name":"Msg","fields":[{"name":"n","type":"int32","nullable":false}],"bindings":{"node":{"package":"@x/beta","module":"../src/beta"}}}
          ],
          "unions":[
            {"name":"Evt","variants":[{"tag":"msg","type":{"model":"Msg"}}],"bindings":{"node":{"package":"@x/alpha","module":"../src/alpha"}}}
          ],
          "interfaces":[]}"#,
    )
    .unwrap();
    let table = group_table(&api, &[], "node");
    let alpha = pinned("@x/alpha", "../src/alpha");
    // The sub-doc for the `alpha` group carries the union `Evt` (its home).
    let sub = ApiDoc {
        fluessig: api.fluessig.clone(),
        source: None,
        models: vec![],
        unions: api.unions.clone(),
        consts: Vec::new(),
        interfaces: vec![],
    };
    let refs = external_refs(&sub, &alpha, &table);
    assert!(
        refs.iter()
            .any(|r| r.symbol == "Msg" && r.module_path.join("::") == "x::beta::src::beta"),
        "union member model surfaced as a cross-group ref: {refs:?}"
    );
}

#[test]
fn per_backend_import_line_is_use_crate_path_symbol() {
    // node/python/ruby/php all emit Rust, so the import line is uniform.
    let mut refs = BTreeSet::new();
    refs.insert(fluessig::bindgen::ExternalRef {
        symbol: "Account".into(),
        module_path: vec!["acme".into(), "beta".into(), "src".into(), "beta".into()],
    });
    assert_eq!(
        render_use_block(&refs),
        "use crate::acme::beta::src::beta::Account;\n"
    );
}

// ── enum-home dedup ──────────────────────────────────────────────────────────

#[test]
fn a_shared_enum_is_defined_once_and_imported_by_each_group() {
    let api = load_api(CROSS_GROUP_API).unwrap();
    let crate_out = render_crate(&api, &flavor_enum());

    // `Flavor` is defined exactly ONCE, in `common`.
    let (_, common) = crate_out
        .common
        .as_ref()
        .expect("a common file (holds the shared enum)");
    assert!(
        common.contains("pub enum Flavor"),
        "the shared enum lives in common:\n{common}"
    );
    let group_defs: usize = crate_out
        .groups
        .iter()
        .filter(|(_, c)| c.contains("pub enum Flavor"))
        .count();
    assert_eq!(
        group_defs, 0,
        "no group file re-defines the shared enum (the duplication bug is fixed)"
    );

    // Both groups that reference it import it from common (imported twice total).
    let imports: usize = crate_out
        .groups
        .iter()
        .filter(|(_, c)| c.contains("use crate::common::Flavor;"))
        .count();
    assert_eq!(imports, 2, "each referencing group imports the shared enum");
}

// ── single-file byte-identical control ───────────────────────────────────────

#[test]
fn no_group_pins_means_no_fan_out_single_file_untouched() {
    // An api with no group pins fans out to nothing (Ok(None)): the single-file
    // path is left entirely untouched — the subsystem is strictly opt-in.
    let api = load_api(
        r#"{"fluessig":{"format":1},"models":[{"name":"M","fields":[{"name":"a","type":"string","nullable":false}]}],"interfaces":[]}"#,
    )
    .unwrap();
    let spec = FanOutSpec {
        lang: "node",
        pattern: "out/{package}/{module}.rs",
        mod_out: "out/root.rs",
        common_out: "out/common.rs",
        note: None,
        mcp_models_only: false,
    };
    let out = fan_out_crate(&api, &[], &spec, &|a, e| node_binding(a, e, None)).expect("no error");
    assert!(out.is_none(), "no group pins ⇒ nothing fanned out");
}

#[test]
fn single_file_gains_no_cross_group_imports() {
    // The runtime import now flows through the shared use-emitter module (see the
    // `runtime_import_fold` suite for the byte-identity golden gate), but the
    // cross-group SUBSYSTEM (fan_out_crate / render_use_block) still never runs in
    // single-file mode: single-file output is deterministic and gains no
    // `use crate::…` cross-group import — the subsystem's sole product.
    let api = load_api(CROSS_GROUP_API).unwrap();
    let a = node_binding(&api, &flavor_enum(), None);
    let b = node_binding(&api, &flavor_enum(), None);
    assert_eq!(a, b, "single-file render is deterministic");
    assert!(
        !a.contains("use crate::"),
        "single-file output carries no cross-group imports (subsystem never runs here)"
    );
}

#[test]
fn external_refs_cover_stream_op_return_types() {
    // #46's dual-error stream surfaces (python `__anext__`, ruby each/Enumerator)
    // synthesize a LOCAL `<Op>ErrorEvent` — three `String` fields, not an api
    // model/union, not in the GroupTable, always emitted beside its stream op
    // (ops never fan out) — so it carries no cross-group ref. The real cross-group
    // surface is the stream ITEM type (`op.returns`), which the exhaustive walk
    // covers even though fan_out drops ops today: a stream op returning a model
    // pinned to another group surfaces that model as a cross-group ref.
    let api = load_api(
        r#"{"fluessig":{"format":1},
          "models":[
            {"name":"Chunk","fields":[{"name":"n","type":"int32","nullable":false}],"bindings":{"node":{"package":"@x/beta","module":"../src/beta"}}}
          ],
          "interfaces":[
            {"name":"Svc","ops":[
              {"name":"watch","shape":"stream","stream_error":{},"params":[],"returns":{"model":"Chunk"}}
            ]}
          ]}"#,
    )
    .unwrap();
    let table = group_table(&api, &[], "node");
    // A sub-doc carrying the interface (the op layer); its home is `common`.
    let sub = ApiDoc {
        fluessig: api.fluessig.clone(),
        source: None,
        models: vec![],
        unions: vec![],
        consts: Vec::new(),
        interfaces: api.interfaces.clone(),
    };
    let refs = external_refs(&sub, &GroupKey::Common, &table);
    assert!(
        refs.iter()
            .any(|r| r.symbol == "Chunk" && r.module_path.join("::") == "x::beta::src::beta"),
        "the stream item type (op return) surfaces as a cross-group ref: {refs:?}"
    );
}

// ── THE compile proof (hermetic, offline) + negative control ─────────────────

#[test]
fn fanned_tree_compiles_and_negative_control_fails() {
    let api = load_api(CROSS_GROUP_API).unwrap();
    let crate_out = render_crate(&api, &flavor_enum());
    let root = std::path::Path::new(&crate_out.root.0).to_path_buf();
    let outdir = root.parent().expect("root has a parent dir").to_path_buf();

    // ── positive: write the dep-free-stripped tree + root, compile it ──
    write_tree(&crate_out, /* with_imports = */ true);
    let out = rustc_compile(&root, &outdir.join("out_pos.rlib"));
    assert!(
        out.status.success(),
        "the fanned tree must compile (cross-module name resolution works).\n\
         rustc stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // ── negative control: the SAME tree WITHOUT the generated `use crate::…`
    // imports must FAIL — proving the imports are load-bearing. ──
    write_tree(&crate_out, /* with_imports = */ false);
    let neg = rustc_compile(&root, &outdir.join("out_neg.rlib"));
    assert!(
        !neg.status.success(),
        "without the generated imports the tree must NOT compile"
    );
    let stderr = String::from_utf8_lossy(&neg.stderr);
    assert!(
        stderr.contains("cannot find") || stderr.contains("E0412") || stderr.contains("E0433"),
        "the failure is unresolved cross-group names, as expected:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&outdir);
}

// ── fixtures ─────────────────────────────────────────────────────────────────

/// The one shared cross-group fixture: two groups (`alpha`, `beta`) + a `common`
/// enum + a structured union.
///  * `Account`, `AgentX` → group `beta`; `Message`, `Note` + union `Evt` → `alpha`.
///  * `Flavor` (an enum) is un-pinned ⇒ homed in `common`, referenced from BOTH
///    groups (`AgentX.kind`, `Message.kind`) — the enum-dedup vector.
///  * `Message` (alpha) references `Account` (beta) — the cross-group DTO import.
///  * union `Evt` (alpha) over co-located variant models `Message`/`Note`; its
///    `Message` variant reaches `Account` (beta) through the inlined field, so
///    `alpha`'s tagged struct must import `crate::acme::beta::src::beta::Account`.
///
/// Used by the ref-set, enum-dedup, AND compile-proof tests (one fixture, no
/// near-duplicate siblings).
const CROSS_GROUP_API: &str = r#"{
  "fluessig": {"format": 1},
  "models": [
    {"name": "Account", "fields": [{"name": "id", "type": "int32", "nullable": false}], "bindings": {"node": {"package": "@acme/beta", "module": "../src/beta"}}},
    {"name": "AgentX",  "fields": [{"name": "kind", "type": {"enum": "Flavor"}, "nullable": false}], "bindings": {"node": {"package": "@acme/beta", "module": "../src/beta"}}},
    {"name": "Message", "fields": [
      {"name": "kind", "type": {"enum": "Flavor"}, "nullable": false},
      {"name": "owner", "type": {"model": "Account"}, "nullable": true}
    ], "bindings": {"node": {"package": "@acme/alpha", "module": "../src/alpha"}}},
    {"name": "Note", "fields": [{"name": "body", "type": "string", "nullable": false}], "bindings": {"node": {"package": "@acme/alpha", "module": "../src/alpha"}}}
  ],
  "unions": [
    {"name": "Evt", "variants": [
      {"tag": "msg",  "type": {"model": "Message"}},
      {"tag": "note", "type": {"model": "Note"}}
    ], "bindings": {"node": {"package": "@acme/alpha", "module": "../src/alpha"}}}
  ],
  "interfaces": []
}"#;

fn flavor_enum() -> Vec<EnumDesc> {
    vec![(
        "Flavor".to_string(),
        vec![EnumVariant::plain("a"), EnumVariant::plain("b")],
    )]
}

fn sub_for(api: &ApiDoc, names: &[&str]) -> ApiDoc {
    ApiDoc {
        fluessig: api.fluessig.clone(),
        source: None,
        models: api
            .models
            .iter()
            .filter(|m| names.contains(&m.name.as_str()))
            .cloned()
            .collect(),
        unions: vec![],
        consts: Vec::new(),
        interfaces: vec![],
    }
}

/// The rendered fan-out crate as content strings, ready to strip + write.
struct RenderedCrate {
    groups: Vec<(String, String)>,
    common: Option<(String, String)>,
    root: (String, String),
}

/// Render the whole node fan-out crate into a fixed tmp layout.
fn render_crate(api: &ApiDoc, enums: &[EnumDesc]) -> RenderedCrate {
    let base = unique_dir("render");
    let pattern = format!("{}/out/{{package}}/{{module}}.rs", base.display());
    let mod_out = format!("{}/root.rs", base.display());
    let common_out = common_path_for(&mod_out);
    let spec = FanOutSpec {
        lang: "node",
        pattern: &pattern,
        mod_out: &mod_out,
        common_out: &common_out,
        note: None,
        mcp_models_only: false,
    };
    let fc = fan_out_crate(api, enums, &spec, &|a, e| node_binding(a, e, None))
        .expect("fan-out ok")
        .expect("groups present");
    RenderedCrate {
        groups: fc.group_files,
        common: fc.common_file,
        root: fc.root_file,
    }
}

fn unique_dir(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "fluessig-xpkg-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Write the crate to its (absolute) on-disk paths, stripping FFI so the module
/// graph is plain, dep-free Rust. `with_imports` false drops the generated `use
/// crate::…` lines from the group/common files (the negative control) while
/// keeping the root mod-tree + re-exports.
fn write_tree(c: &RenderedCrate, with_imports: bool) {
    let prep = |content: &str| -> String {
        let s = strip_ffi(content);
        if with_imports {
            s
        } else {
            remove_crate_imports(&s)
        }
    };
    for (path, content) in &c.groups {
        write_file(path, &prep(content));
    }
    if let Some((path, content)) = &c.common {
        write_file(path, &prep(content));
    }
    // The root ties them together (mod-tree + re-exports); it carries no FFI for
    // node, but strip anyway for uniformity — it is a no-op there.
    write_file(&c.root.0, &strip_ffi(&c.root.1));
}

fn write_file(path: &str, content: &str) {
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// Strip the per-language FFI so the DTO+enum+union module graph is plain Rust:
/// drop FFI `use` preludes, the `fn err(…)` helper, and FFI attribute lines
/// (`#[napi…]`/`#[pyclass]`/`#[pyo3]`/…), keeping `#[derive(…)]`, the plain
/// structs/enums/impls, and the `use crate::…` cross-group imports (the thing
/// under test). This isolates and PROVES cross-module name resolution without
/// needing napi/pyo3/magnus (unavailable offline).
fn strip_ffi(src: &str) -> String {
    let mut out = String::new();
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        // FFI / runtime preludes that aren't available to an offline single-file
        // rustc. `fluessig_runtime` is listed proactively: the streams thread is
        // moving Poll/PollStream out of the local prelude into
        // `use fluessig_runtime::…`; a fanned DTO+enum+union file never references
        // those (ops are dropped), so dropping the import keeps the proof offline.
        if t.starts_with("use napi")
            || t.starts_with("use pyo3")
            || t.starts_with("use ext_php_rs")
            || t.starts_with("use magnus")
            || t.starts_with("use fluessig_runtime")
        {
            continue;
        }
        if t.starts_with("fn err(") {
            // skip the helper block up to its closing brace at column 0.
            for l in lines.by_ref() {
                if l == "}" {
                    break;
                }
            }
            continue;
        }
        if t.starts_with("#[napi")
            || t.starts_with("#[pyclass")
            || t.starts_with("#[pyo3")
            || t.starts_with("#[pymethods")
            || t.starts_with("#[pymodule")
            || t.starts_with("#[php")
            || t.starts_with("#[magnus")
            || t.starts_with("#[getter")
            || t.starts_with("#[new")
        {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn remove_crate_imports(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("use crate::"))
        .map(|l| format!("{l}\n"))
        .collect()
}

fn rustc_compile(root: &std::path::Path, out: &std::path::Path) -> std::process::Output {
    Command::new("rustc")
        .args(["--edition", "2021", "--crate-type", "lib", "-A", "warnings"])
        .arg(root)
        .arg("-o")
        .arg(out)
        .output()
        .expect("invoke rustc (must be on PATH, ships with the toolchain)")
}
