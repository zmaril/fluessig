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
use fluessig::bindgen::{node_binding, python_binding, EnumDesc};
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
}
