//! Slice 5 gate: the derive-emitted `api.json` (the op surface) loads clean
//! through fluessig's existing op-layer loader/validator, its op kinds / params /
//! returns resolve, the entity references in op signatures resolve against the
//! sibling catalog, and the unchanged bindgen back end projects it into sane
//! per-language glue (streams → an async iterator, `manual` recorded-but-not-
//! auto-bound).
//!
//! This is the semantic-equivalence checkpoint from
//! `notes/derive-front-end-decisions.md` (Slice 5): not a byte diff, but "the
//! derived `api.json` loads clean through the Rust validator and drives bindgen
//! to sane output."

use std::collections::BTreeSet;

use fluessig::api::{load_api, ApiType, Shape};
use fluessig::bindgen::{
    node_binding, php_binding, python_binding, ruby_binding, wasm_binding, EnumDesc,
};
use fluessig::load_catalog;

/// The Rust type name a (possibly nullable/list) op type ultimately references,
/// if it is a `{ model }` — for the entity-resolution cross-check.
fn model_ref(t: &ApiType) -> Option<&str> {
    match t {
        ApiType::Model { model } => Some(model),
        ApiType::List { list } => model_ref(list),
        ApiType::Nullable { nullable } => model_ref(nullable),
        _ => None,
    }
}

#[test]
fn emitted_api_loads_and_validates() {
    let json = derive_demo::api::fluessig_catalog::api_to_json();
    let api = load_api(&json).expect("derive-emitted api.json must load clean");

    assert_eq!(api.source.as_deref(), Some("api_demo"));
    assert_eq!(api.fluessig.format, fluessig::FORMAT_VERSION);
    assert_eq!(api.interfaces.len(), 2);

    // ── the Db interface: all four op kinds captured from the impl ────────────
    let db = api.interfaces.iter().find(|i| i.name == "Db").unwrap();
    assert_eq!(
        db.doc.as_deref(),
        Some("An open demo database. Heavy ops are unary; the change feed is a stream.")
    );
    let op = |n: &str| db.ops.iter().find(|o| o.name == n).unwrap();

    // ctor: void return, params camelCased, doc captured
    let open = op("open");
    assert_eq!(open.shape, Shape::Ctor);
    assert!(matches!(&open.returns, ApiType::Scalar(s) if s == "void"));
    assert_eq!(open.params.len(), 1);
    assert_eq!(open.params[0].name, "path");
    assert!(matches!(&open.params[0].ty, ApiType::Scalar(s) if s == "string"));

    // plain unary returning Option<entity> → nullable model
    let repo = op("repo");
    assert_eq!(repo.shape, Shape::Unary);
    match &repo.returns {
        ApiType::Nullable { nullable } => {
            assert!(matches!(&**nullable, ApiType::Model { model } if model == "Repo"))
        }
        other => panic!("repo should return nullable Repo, got {other:?}"),
    }

    // plain unary returning a scalar; snake_case name camelCased
    let count = op("pullRequestCount");
    assert_eq!(count.shape, Shape::Unary);
    assert!(matches!(&count.returns, ApiType::Scalar(s) if s == "int64"));
    assert_eq!(count.params[0].name, "repoId");

    // Option<T> param → optional:true carrying the UNWRAPPED T; List return
    let repos = op("repos");
    assert_eq!(repos.params[0].name, "limit");
    assert_eq!(repos.params[0].optional, Some(true));
    assert!(matches!(&repos.params[0].ty, ApiType::Scalar(s) if s == "int32"));
    assert!(
        matches!(&repos.returns, ApiType::List { list } if matches!(&**list, ApiType::Scalar(s) if s == "string"))
    );

    // stream: shape stream, returns the iterator Item (an entity model)
    let stream = op("pullRequests");
    assert_eq!(stream.shape, Shape::Stream);
    assert!(matches!(&stream.returns, ApiType::Model { model } if model == "PullRequest"));

    // manual: recorded, void return, params captured
    let watch = op("watch");
    assert_eq!(watch.shape, Shape::Manual);
    assert_eq!(watch.params[0].name, "intervalSecs");
    assert!(matches!(&watch.returns, ApiType::Scalar(s) if s == "void"));

    // ── the stateless GitHelpers interface: no-self associated unary ops ──────
    let git = api
        .interfaces
        .iter()
        .find(|i| i.name == "GitHelpers")
        .unwrap();
    assert!(git.ops.iter().all(|o| o.shape == Shape::Unary));
    let be = git.ops.iter().find(|o| o.name == "branchExists").unwrap();
    assert_eq!(be.params.len(), 2);
    assert!(matches!(&be.returns, ApiType::Scalar(s) if s == "boolean"));
}

#[test]
fn op_entity_references_resolve_against_the_catalog() {
    // Gate (c): every `{ model }` an op names must be defined in the sibling
    // catalog — either an entity or (Slice 8a Gap 2) a value struct / DTO. The
    // impl can't reference a type the catalog doesn't define.
    let api = load_api(&derive_demo::api::fluessig_catalog::api_to_json()).unwrap();
    let catalog =
        load_catalog(&derive_demo::api::fluessig_catalog::to_json()).expect("catalog loads");
    let defined: BTreeSet<&str> = catalog
        .entities
        .iter()
        .map(|e| e.name.as_str())
        .chain(catalog.value_structs.iter().map(|s| s.name.as_str()))
        .collect();

    let mut referenced = BTreeSet::new();
    for i in &api.interfaces {
        for op in &i.ops {
            for p in &op.params {
                if let Some(m) = model_ref(&p.ty) {
                    referenced.insert(m.to_string());
                }
            }
            if let Some(m) = model_ref(&op.returns) {
                referenced.insert(m.to_string());
            }
        }
    }
    // the demo references entities (Repo, PullRequest) and DTOs (LoadStats,
    // SinkOptions); every referenced model must resolve in the catalog.
    for want in ["Repo", "PullRequest", "LoadStats", "SinkOptions"] {
        assert!(referenced.contains(want), "op should reference {want}");
    }
    for m in &referenced {
        assert!(
            defined.contains(m.as_str()),
            "op references `{m}`, which is not an entity or value struct in the catalog"
        );
    }
}

#[test]
fn models_are_materialized_with_flattening_and_closure() {
    // Slice 8a Gap 2 gate: the ops reference entities (Repo, PullRequest) and DTOs
    // (LoadStats, SinkOptions); api.json's `models` array materialises those —
    // flattened (a to-one relation → its FK field(s)), plus TableRename pulled in
    // transitively through SinkOptions.renames. This is the always-on check (the
    // TypeSpec byte-equivalence is the env-gated api_typespec_equivalence test).
    let api = load_api(&derive_demo::api::fluessig_catalog::api_to_json()).unwrap();
    let model = |n: &str| {
        api.models
            .iter()
            .find(|m| m.name == n)
            .unwrap_or_else(|| panic!("model {n} missing from api.json"))
    };
    let field_names = |n: &str| {
        model(n)
            .fields
            .iter()
            .map(|f| f.name.clone())
            .collect::<Vec<_>>()
    };

    // the referenced closure: direct op refs + the transitive DTO ref.
    let names: BTreeSet<&str> = api.models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "LoadStats",
            "SinkOptions",
            "TableRename",
            "Repo",
            "PullRequest"
        ]),
        "GhUser/Review (only relation targets, never op-referenced) must NOT join"
    );

    // an entity flattens: PullRequest's `repo_id`/`author_id` relations become FK
    // fields (camelCased), and the FK-in-PK `repoId` lands — no nested entity.
    assert_eq!(
        field_names("PullRequest"),
        vec!["repoId", "number", "title", "authorId"]
    );
    // a plain-scalar entity: the nullable scalar stays nullable, name camelCased.
    let repo = model("Repo");
    let remote = repo.fields.iter().find(|f| f.name == "remoteUrl").unwrap();
    assert!(remote.nullable, "Repo.remoteUrl is a nullable scalar");

    // the DTO layer: a scalar-only Record, and one with a list-of-Record field.
    assert_eq!(field_names("LoadStats"), vec!["commits", "refs"]);
    let sink = model("SinkOptions");
    let renames = sink.fields.iter().find(|f| f.name == "renames").unwrap();
    match &renames.ty {
        ApiType::List { list } => {
            assert!(matches!(&**list, ApiType::Model { model } if model == "TableRename"))
        }
        other => panic!("SinkOptions.renames should be a list of TableRename, got {other:?}"),
    }
}

#[test]
fn bindgen_projects_the_op_surface() {
    // Gate (d): the unchanged bindgen back end produces sane binding surface —
    // in particular the `stream` op becomes an async-iterable and the `manual` op
    // is recorded but NOT auto-bound.
    let api = load_api(&derive_demo::api::fluessig_catalog::api_to_json()).unwrap();
    let enums: Vec<EnumDesc> = Vec::new();

    let node = node_binding(&api, &enums, None);
    // the stream op projects to an async-iterable stream class
    assert!(
        node.contains("pub struct PullRequests")
            && node.contains("impl AsyncGenerator for PullRequests"),
        "node stream class missing"
    );
    assert!(
        node.contains("Symbol.asyncIterator"),
        "node stream should expose an async iterator"
    );
    // the manual op is recorded but hand-written, not auto-bound
    assert!(
        node.contains("// @manual: watch — hand-written in lib.rs."),
        "node should record `watch` as @manual, not auto-bind it"
    );
    // a plain unary op is bound as a real off-thread method
    assert!(
        node.contains("pub fn pull_request_count"),
        "unary op should bind"
    );

    let py = python_binding(&api, &enums, None);
    assert!(
        py.contains("@manual: watch"),
        "python should record `watch` as @manual"
    );
    assert!(
        py.contains("fn pull_requests"),
        "python should bind the stream op"
    );

    // ── wasm: the `Db` interface (ctor) → a handle struct; streams are skipped
    // honestly (no broken code); manual ops are recorded but not auto-bound; a
    // plain unary op binds under a wasm-bindgen `js_name`. ──
    let wasm = wasm_binding(&api, &enums, None);
    // the wasm-bindgen surface exists
    assert!(
        wasm.contains("#[wasm_bindgen]"),
        "wasm should emit a #[wasm_bindgen] surface"
    );
    // the ctor interface projects to a handle struct with a constructor
    assert!(
        wasm.contains("pub struct Db {") && wasm.contains("#[wasm_bindgen(constructor)]"),
        "wasm ctor interface should become a handle struct with a constructor"
    );
    // a plain unary op binds under its lowerCamel js_name
    assert!(
        wasm.contains("js_name = \"pullRequestCount\"")
            && wasm.contains("pub fn pull_request_count"),
        "wasm should bind the unary op under a js_name"
    );
    // the stream op is skipped honestly, not emitted as broken code
    assert!(
        wasm.contains("// stream op `pullRequests` is not yet supported by the wasm backend"),
        "wasm should skip the stream op with an honest note"
    );
    // the manual op is recorded but hand-written, not auto-bound
    assert!(
        wasm.contains("// @manual: watch"),
        "wasm should record `watch` as @manual, not auto-bind it"
    );
}

/// The sync-by-default authoring surface this PR lands, against the `native`
/// demo schema (kept apart from the four-kind `Db`/`GitHelpers` demo so the
/// sync/async/pin concept doesn't leak into that pedagogy):
///
///   * a DEFAULT op is SYNCHRONOUS — no `async` field in `api.json` (synchronous
///     is the GLOBAL default; there is no catalog-level lever). Infallible when
///     the Rust return is a bare `T` (no `Result` seam; the shared core-trait
///     method is `fn … -> T`), fallible when it is a `Result<T>`.
///   * `#[fluessig(async)]` is the OPT-IN: `is_async == true` → the async
///     projection. It is the ONE place async-ness is decided, everywhere.
///   * `#[fluessig(name = "…")]` pins the export name across every backend.
#[test]
fn native_api_carries_sync_default_and_pin_flags() {
    let api = load_api(&derive_demo::native::fluessig_catalog::api_to_json())
        .expect("native api.json must load clean");
    let iface = api.interfaces.iter().find(|i| i.name == "Native").unwrap();
    let op = |n: &str| iface.ops.iter().find(|o| o.name == n).unwrap();

    // DEFAULT sync (no `async` marker) + infallible + name-pinned: the atilla
    // `atillaNativeVersion` shape.
    let nv = op("nativeVersion");
    assert_eq!(nv.shape, Shape::Unary);
    assert!(
        !nv.is_async,
        "a default op is synchronous (no `async` marker)"
    );
    assert!(nv.infallible, "a bare-String return is infallible");
    assert!(matches!(&nv.returns, ApiType::Scalar(s) if s == "string"));
    for lang in ["node", "python", "php", "ruby"] {
        assert_eq!(
            nv.bindings.get(lang).and_then(|b| b.name.as_deref()),
            Some("atillaNativeVersion"),
            "the op export-name pin lands under the {lang} binding"
        );
    }

    // DEFAULT sync but FALLIBLE (Result<T> return): no marker, infallible NOT set.
    let cr = op("checkedRoot");
    assert!(!cr.is_async, "checkedRoot is a default (sync) op");
    assert!(!cr.infallible, "a Result<T> return keeps the error seam");

    // opt-IN: `#[fluessig(async)]` marks async; never infallible; unpinned.
    let sc = op("slowCount");
    assert!(sc.is_async, "slowCount is #[fluessig(async)]");
    assert!(
        !sc.infallible,
        "slowCount resolves async (never infallible)"
    );
    assert!(sc.bindings.is_empty(), "an unpinned op has no bindings");
}

/// The node backend emits the DEFAULT synchronous/infallible + `js_name` shape,
/// and the `#[fluessig(async)]` op keeps the async projection — the concrete
/// generated-code proof. Compare `native_version`'s emission to atilla's
/// hand-written `crates/atilla-napi/src/lib.rs` export (byte-comparable modulo
/// the core-seam body): `#[napi(js_name = "atillaNativeVersion")] pub fn …() -> String`.
#[test]
fn node_emits_sync_default_and_pinned_shapes() {
    let api = load_api(&derive_demo::native::fluessig_catalog::api_to_json()).unwrap();
    let enums: Vec<EnumDesc> = Vec::new();
    let node = node_binding(&api, &enums, None);

    // DEFAULT sync + infallible + pinned → a plain `#[napi] fn` returning
    // `String`, under the pinned js_name, with no Promise/AsyncTask/Result.
    assert!(
        node.contains(
            "#[napi(js_name = \"atillaNativeVersion\")]\npub fn native_version() -> String {"
        ),
        "sync+infallible+pinned op must emit a plain `#[napi(js_name=…)] fn -> String`\n{node}"
    );
    // the infallible core seam is a direct value passthrough — no `.map_err`, no Result.
    assert!(
        node.contains("pub fn native_version() -> String {\n    <crate::core_impl::NativeImpl as NativeCore>::native_version()\n}"),
        "infallible sync op must call the core directly with no Result seam"
    );
    // the shared core trait drops the Result wrapper for the infallible op.
    assert!(
        node.contains("fn native_version() -> String;"),
        "the infallible op's core-trait method must be `fn … -> String`"
    );

    // Default sync, fallible variant: `Result<T>` → `napi::Result`, still no AsyncTask.
    assert!(
        node.contains("#[napi]\npub fn checked_root(path: String) -> Result<String> {"),
        "sync+fallible op must emit `-> Result<String>` (throws), no AsyncTask"
    );

    // Opt-OUT / no regression: the `#[fluessig(async)]` op keeps the async projection.
    assert!(
        node.contains("#[napi(ts_return_type = \"Promise<number>\")]\npub fn slow_count(path: String) -> AsyncTask<SlowCountTask> {"),
        "the #[fluessig(async)] op must stay the async `Promise<number>`"
    );

    // A sync op produces NO per-op Task struct (only the async `slow_count` does).
    assert!(
        !node.contains("struct NativeVersionTask") && !node.contains("struct CheckedRootTask"),
        "a sync op must not generate an off-thread Task"
    );
    assert!(
        node.contains("pub struct SlowCountTask"),
        "the async op still generates its Task"
    );
}

/// The SAME sync-default + infallible + name-pin shape in python, php, and ruby —
/// proving the projection is not node-only. python/php/ruby unary ops are already
/// synchronous, so the observable default-inversion effect is INFALLIBILITY (drop
/// the raise/throw seam) plus the export-name pin.
#[test]
fn python_php_ruby_emit_sync_infallible_and_pinned_shapes() {
    let api = load_api(&derive_demo::native::fluessig_catalog::api_to_json()).unwrap();
    let enums: Vec<EnumDesc> = Vec::new();

    // ── python: `#[pyo3(name = "…")]` + a plain `-> String` (no PyResult) ──
    let py = python_binding(&api, &enums, None);
    assert!(
        py.contains("#[pyo3(name = \"atillaNativeVersion\")]"),
        "python pins the op name via #[pyo3(name = …)]\n{py}"
    );
    assert!(
        py.contains("fn native_version(py: Python<'_>) -> String {"),
        "python infallible op drops PyResult → `-> String`\n{py}"
    );
    // fallible default op keeps PyResult; the async op stays a plain method too.
    assert!(
        py.contains("fn checked_root(py: Python<'_>, path: String) -> PyResult<String> {"),
        "python fallible op keeps the PyResult seam\n{py}"
    );

    // ── php: ext-php-rs `#[rename("…")]` + a plain `-> String` (no PhpResult) ──
    let php = php_binding(&api, &enums, None);
    assert!(
        php.contains("#[rename(\"atillaNativeVersion\")]"),
        "php pins the op name via #[rename(…)]\n{php}"
    );
    assert!(
        php.contains("pub fn native_version() -> String {"),
        "php infallible op drops PhpResult → `-> String`\n{php}"
    );
    assert!(
        php.contains("pub fn checked_root(path: String) -> PhpResult<String> {"),
        "php fallible op keeps the PhpResult seam\n{php}"
    );

    // ── ruby: the pinned singleton-method name + a plain `-> String` (no Result) ──
    let ruby = ruby_binding(&api, &enums, None);
    assert!(
        ruby.contains(
            "define_singleton_method(\"atillaNativeVersion\", function!(native_version, 0))"
        ),
        "ruby exposes the pinned method name; the Rust fn stays snake\n{ruby}"
    );
    assert!(
        ruby.contains("fn native_version() -> String {"),
        "ruby zero-arg infallible op drops the Result seam → `-> String`\n{ruby}"
    );
    assert!(
        ruby.contains("fn checked_root(path: String) -> Result<String, Error> {"),
        "ruby fallible op keeps the Result seam\n{ruby}"
    );
}
