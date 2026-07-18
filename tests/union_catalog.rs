//! The tagged-union gate (format 1): the committed union fixture
//! (`tests/fixtures/union.tsp`, emitted beside it) must load + validate, lower
//! to twin columns in every dialect, and cross the op layer as the envelope
//! carrier. Regenerate with:
//!   cd emitter && node emit.mjs ../tests/fixtures/union.tsp
//!
//! straitjacket-allow-file:duplication — the per-language enum-parity assertions
//! here are DELIBERATELY parallel to `tests/php_catalog.rs` (same fixture load,
//! same name-only-enum setup); the cross-language token parity is the point.

use fluessig::sql::{ddl, tables, Dialect};
use fluessig::{load_catalog, TypeRef};

const CATALOG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/catalog.json"
));
const API: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/api.json"
));

#[test]
fn union_catalog_loads_and_validates() {
    let c = load_catalog(CATALOG).expect("union fixture must validate");
    let u = c.union_def("EventPayload").unwrap();
    assert_eq!(
        u.variants
            .iter()
            .map(|v| v.tag.as_str())
            .collect::<Vec<_>>(),
        ["message", "log", "exit"]
    );
    // variant bodies are value structs
    for v in &u.variants {
        let TypeRef::Ref { name, entity } = &v.ty else {
            panic!("variant {} should be a ref", v.tag)
        };
        assert!(!entity);
        assert!(c.value_struct(name).is_some(), "{name} is a value struct");
    }
}

#[test]
fn union_fields_lower_to_twin_columns() {
    let c = load_catalog(CATALOG).unwrap();
    for d in [Dialect::Postgres, Dialect::Duckdb, Dialect::Sqlite] {
        let t = &tables(&c, d)["events"];
        let col = |n: &str| t.columns.iter().find(|c| c.name == n).unwrap();

        // required union → both twins NOT NULL; kind is text; payload is json-typed
        assert!(col("payload_kind").not_null);
        assert_eq!(col("payload_kind").ty, "text");
        assert!(col("payload").not_null);
        let payload_ty = match d {
            Dialect::Postgres => "jsonb",
            Dialect::Duckdb => "json",
            Dialect::Sqlite => "text",
        };
        assert_eq!(col("payload").ty, payload_ty);
        // kind column documents its union
        assert!(col("payload_kind")
            .doc
            .as_deref()
            .unwrap()
            .contains("EventPayload"));

        // nullable union → both twins nullable
        assert!(!col("note_kind").not_null);
        assert!(!col("note").not_null);

        // twins sit adjacent, kind first (canonical column order)
        let names: Vec<_> = t.columns.iter().map(|c| c.name.as_str()).collect();
        let kind_at = names.iter().position(|n| *n == "payload_kind").unwrap();
        assert_eq!(names[kind_at + 1], "payload");
    }

    // the DDL carries the twins
    let sql = ddl(&c, Dialect::Postgres, None);
    assert!(sql.contains("\"payload_kind\" text NOT NULL"), "{sql}");
    assert!(sql.contains("\"payload\" jsonb NOT NULL"), "{sql}");
}

#[test]
fn union_crosses_the_op_layer() {
    let api = fluessig::api::load_api(API).unwrap();

    // the union rides in api.unions with api-typed variants
    let u = api
        .unions
        .iter()
        .find(|u| u.name == "EventPayload")
        .unwrap();
    assert_eq!(u.variants.len(), 3);

    // variant models joined the referenced closure
    for m in ["AgentMessage", "LogLine", "ExitInfo", "Event"] {
        assert!(
            api.models.iter().any(|am| am.name == m),
            "{m} in api models"
        );
    }

    // bindgen: union values cross as the JSON envelope (String carrier), and
    // the generated surface still renders for every language
    let enums: Vec<(String, Vec<String>)> = Vec::new();
    let node = fluessig::bindgen::node_binding(&api, &enums, None);
    assert!(
        node.contains("pub payload: String"),
        "envelope carrier:\n{node}"
    );
    fluessig::bindgen::python_binding(&api, &enums, None);
    fluessig::bindgen::ruby_binding(&api, &enums, None);
}

// ── the dual error model (node backend): pre-start throws, post-start yields ──
// The seam: setup/validation/creation (ctor, unary, stream CONSTRUCTION) keep
// mapping a core `Err` to a THROWN napi error; a failure AFTER a stream has
// started rides out as a terminal error EVENT (a value), never a rejection —
// pi's `stream()` boundary (packages/ai/src/types.ts).

fn node_fixture() -> String {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<(String, Vec<String>)> = Vec::new();
    fluessig::bindgen::node_binding(&api, &enums, None)
}

/// The `impl Task for Next<Class>Task { … }` compute body, sliced out so an
/// assertion can scope to the in-stream path (no `err()` there).
fn task_impl<'a>(node: &'a str, class: &str) -> &'a str {
    let start = node
        .find(&format!("impl Task for Next{class}Task"))
        .expect("stream task impl");
    let end = node[start..].find("fn resolve").expect("resolve") + start;
    &node[start..end]
}

/// The `impl AsyncGenerator for <Class> { fn next … }` body up to `fn complete`,
/// so an assertion can scope to the PRIMARY async-iterable in-stream path.
fn async_gen_next<'a>(node: &'a str, class: &str) -> &'a str {
    let start = node
        .find(&format!("impl AsyncGenerator for {class}"))
        .expect("async generator impl");
    let end = node[start..].find("fn complete").expect("complete") + start;
    &node[start..end]
}

#[test]
fn node_unary_op_still_throws_on_core_err() {
    // A UNARY op is pre-start-ish (a discrete call): its compute funnels the core
    // `Err` through `.map_err(err)` — a thrown/rejected promise, unchanged.
    let node = node_fixture();
    assert!(
        node.contains("self.core.emit(self.payload.clone()).map_err(err)"),
        "unary compute must still throw via map_err(err):\n{node}"
    );
}

#[test]
fn node_default_stream_op_rejects_mid_stream() {
    // DEFAULT (unannotated) stream op = idiomatic native-TS REJECT. A mid-stream
    // `Poll::Failed` maps to `Err(err(e))` on BOTH surfaces — no `Either`, no
    // `<Op>ErrorEvent` struct, `Yield = <item>` unchanged, `next()` ts stays
    // `Promise<item | null>`. `Watch.events` in the fixture carries no `@streamError`,
    // so it is throw-mode.
    let node = node_fixture();
    // retained poll cursor rejects
    let compute = task_impl(&node, "Events");
    assert!(
        compute.contains("Poll::Failed(e) => return Err(err(e))"),
        "throw-mode retained cursor rejects on failure:\n{compute}"
    );
    // primary async-iterable rejects
    let gen_next = async_gen_next(&node, "Events");
    assert!(
        gen_next.contains("Poll::Failed(e) => return Err(err(e))"),
        "throw-mode async-iterable rejects on failure:\n{gen_next}"
    );
    // NO event-mode machinery for an unannotated op
    assert!(
        !node.contains("EventsErrorEvent"),
        "no error-event struct for a default (unannotated) stream op:\n{node}"
    );
    assert!(
        !node.contains("Either::B") && !node.contains("Either<Event"),
        "no Either widening for a default stream op:\n{node}"
    );
    // the sibling async-iterator surface is UNTOUCHED for default ops
    assert!(
        node.contains("type Yield = Event;"),
        "default op keeps the plain item Yield:\n{node}"
    );
    assert!(
        node.contains("#[napi(ts_return_type = \"Promise<Event | null>\")]"),
        "default op keeps the plain nullable Promise ts type:\n{node}"
    );
}

#[test]
fn node_annotated_stream_op_yields_error_event_never_throws() {
    // OPT-IN `@streamError` = error-as-event (mirror a library). A *bare* `@streamError`
    // lowers to `stream_error: {}` = pi's default `{ type: "error", reason, error }`.
    // Assert the full event-mode wiring: the pi-shaped `<Op>ErrorEvent` struct, the
    // `Either<item, ErrorEvent>` Yield, `Either::B` on `Poll::Failed` (NEVER `Err`,
    // never `map_err`), and the `next()` ts type carrying the event — on both surfaces.
    let json = r#"{
      "fluessig": { "format": 1 },
      "source": "t.tsp",
      "models": [ { "name": "Tick", "fields": [
        { "name": "n", "type": "int64", "nullable": false }
      ] } ],
      "unions": [],
      "interfaces": [ { "name": "Feed", "ops": [
        { "name": "open", "shape": "ctor", "params": [], "returns": "void" },
        { "name": "ticks", "shape": "stream",
          "stream_error": {},
          "params": [], "returns": { "model": "Tick" } }
      ] } ]
    }"#;
    let api = fluessig::api::load_api(json).unwrap();
    let enums: Vec<(String, Vec<String>)> = Vec::new();
    let node = fluessig::bindgen::node_binding(&api, &enums, None);
    // pi-shaped error-event struct (bare `@streamError` → defaults)
    let s = node
        .find("pub struct TicksErrorEvent")
        .expect("error-event struct");
    let region = &node[s..s + 400];
    assert!(
        region.contains("#[napi(js_name = \"type\")]"),
        "tag js-name defaults to `type`:\n{region}"
    );
    assert!(region.contains("pub type_: String"), "{region}");
    assert!(region.contains("pub reason: String"), "{region}");
    assert!(region.contains("pub error: String"), "{region}");
    assert!(
        node.contains("type_: \"error\".into()"),
        "tag value defaults to \"error\":\n{node}"
    );
    // Either-widened Yield (event-mode only)
    assert!(
        node.contains("type Yield = napi::bindgen_prelude::Either<Tick, TicksErrorEvent>;"),
        "event-mode widens the Yield to Either:\n{node}"
    );
    // primary async-iterable yields the event, never returns Err
    let gen_next = async_gen_next(&node, "Ticks");
    assert!(
        gen_next.contains("Either::B(TicksErrorEvent"),
        "async-iterable yields the error event:\n{gen_next}"
    );
    assert!(
        !gen_next.contains("return Err("),
        "event-mode async path never returns Err:\n{gen_next}"
    );
    assert!(
        gen_next.contains("error: e,") && !gen_next.contains("err(e)"),
        "the core failure message is yielded raw, not thrown via err():\n{gen_next}"
    );
    // retained cursor yields the event, never throws
    let compute = task_impl(&node, "Ticks");
    assert!(
        compute.contains("Either::B(TicksErrorEvent"),
        "retained cursor yields the error event:\n{compute}"
    );
    assert!(
        !compute.contains("return Err(") && !compute.contains(".map_err("),
        "event-mode retained cursor never throws:\n{compute}"
    );
    assert!(
        node.contains("Promise<Tick | TicksErrorEvent | null>"),
        "next() ts_return_type carries the error event:\n{node}"
    );
}

#[test]
fn node_annotated_stream_error_event_is_configurable() {
    // WITH `stream_error`, the emitted struct/tag adopt the overrides — proving
    // the shape is schema-configurable (pi's shape is merely the default).
    let json = r#"{
      "fluessig": { "format": 1 },
      "source": "t.tsp",
      "models": [ { "name": "Tick", "fields": [
        { "name": "n", "type": "int64", "nullable": false }
      ] } ],
      "unions": [],
      "interfaces": [ { "name": "Feed", "ops": [
        { "name": "open", "shape": "ctor", "params": [], "returns": "void" },
        { "name": "ticks", "shape": "stream",
          "stream_error": { "tag_name": "kind", "tag_value": "failure",
                            "reason_name": "why", "error_name": "message" },
          "params": [], "returns": { "model": "Tick" } }
      ] } ]
    }"#;
    let api = fluessig::api::load_api(json).unwrap();
    let enums: Vec<(String, Vec<String>)> = Vec::new();
    let node = fluessig::bindgen::node_binding(&api, &enums, None);
    assert!(node.contains("pub struct TicksErrorEvent"), "{node}");
    assert!(
        node.contains("#[napi(js_name = \"kind\")]"),
        "tag override:\n{node}"
    );
    assert!(
        node.contains("#[napi(js_name = \"why\")]"),
        "reason override:\n{node}"
    );
    assert!(
        node.contains("#[napi(js_name = \"message\")]"),
        "error override:\n{node}"
    );
    assert!(
        node.contains("type_: \"failure\".into()"),
        "tag value override:\n{node}"
    );
    assert!(
        node.contains("Promise<Tick | TicksErrorEvent | null>"),
        "ts type carries the error event:\n{node}"
    );
}

#[test]
fn node_pre_start_paths_still_throw() {
    // Pre-start boundary: the stream CONSTRUCTION (handle) method and the CTOR
    // both keep `.map_err(err)?` — a core failure there THROWS (setup/creation).
    let node = node_fixture();
    assert!(
        node.contains("self.core.events(after_idx).map_err(err)?"),
        "stream construction throws before start:\n{node}"
    );
    assert!(
        node.contains("as WatchCore>::open(path).map_err(err)?"),
        "ctor throws:\n{node}"
    );
}

#[test]
fn node_stream_error_rejected_off_stream_shape() {
    // The loader validates: `stream_error` is meaningless off a stream op.
    let json = r#"{
      "fluessig": { "format": 1 },
      "source": "t.tsp",
      "models": [], "unions": [],
      "interfaces": [ { "name": "Feed", "ops": [
        { "name": "poke", "shape": "unary",
          "stream_error": { "tag_value": "boom" },
          "params": [], "returns": "void" }
      ] } ]
    }"#;
    let err = fluessig::api::load_api(json).unwrap_err();
    assert!(
        err.contains("stream_error") && err.contains("only valid on a stream op"),
        "{err}"
    );
}

#[test]
fn node_name_only_enums_lower_to_snake_case_string_enums() {
    // A name-only vocabulary (not in the wire-valued allowlist) must lower to a
    // napi *string* enum whose variants carry their snake_case wire token, so JS
    // sees `CapabilityKind.Dispatch === "dispatch"` — the same tokens the ruby
    // emitter hands out via `wire()`, not the magic discriminant number a plain
    // `#[napi]` enum emits.
    let api = fluessig::api::load_api(API).unwrap();
    let enums = vec![(
        "CapabilityKind".to_string(),
        vec!["dispatch".to_string(), "isolation_vm".to_string()],
    )];
    let node = fluessig::bindgen::node_binding(&api, &enums, None);
    assert!(
        node.contains("#[napi(string_enum)]"),
        "string enum, not numeric:\n{node}"
    );
    assert!(
        node.contains("pub enum CapabilityKind"),
        "enum type is still emitted:\n{node}"
    );
    assert!(
        node.contains("#[napi(value = \"dispatch\")]"),
        "explicit wire token:\n{node}"
    );
    assert!(
        node.contains("#[napi(value = \"isolation_vm\")]"),
        "snake_case wire token, underscore preserved:\n{node}"
    );
    // the Rust variant ident is unchanged (a consumer's core_impl keeps
    // constructing `CapabilityKind::IsolationVm`).
    assert!(node.contains("IsolationVm,"), "variant ident:\n{node}");

    // ruby parity: the node string token is exactly the token ruby maps in its
    // enum codec (here the always-emitted `parse()` arm).
    let ruby = fluessig::bindgen::ruby_binding(&api, &enums, None);
    assert!(
        ruby.contains("\"isolation_vm\" => Ok(Self::IsolationVm)"),
        "ruby token parity:\n{ruby}"
    );
}

#[test]
fn union_validation_rejects_the_bad_shapes() {
    let base = |unions: &str, fields: &str| {
        format!(
            r#"{{
              "fluessig": {{ "format": 1 }},
              "scalars": [], "enums": [],
              "unions": {unions},
              "entities": [{{
                "name": "E", "table": "es", "key": ["id"],
                "fields": [
                  {{ "name": "id", "type": {{"k":"scalar","name":"int64"}}, "nullable": false, "key": true }}{fields}
                ]
              }}],
              "relationProperties": [],
              "valueStructs": [{{ "name": "Body", "fields": [
                {{ "name": "x", "type": {{"k":"scalar","name":"string"}}, "nullable": false }}
              ]}}]
            }}"#
        )
    };
    let load = |unions: &str, fields: &str| load_catalog(&base(unions, fields));

    // unknown union
    let err = load(
        "[]",
        r#", { "name": "p", "type": {"k":"union","name":"Nope"}, "nullable": false }"#,
    )
    .unwrap_err();
    assert!(err.contains("unknown union Nope"), "{err}");

    // union as key member
    let err = load(
        r#"[{ "name": "U", "variants": [{ "tag": "b", "type": {"k":"ref","name":"Body","entity":false} }] }]"#,
        r#", { "name": "p", "type": {"k":"union","name":"U"}, "nullable": false, "key": true }"#,
    )
    .unwrap_err();
    assert!(err.contains("cannot be a key member"), "{err}");

    // list of unions
    let err = load(
        r#"[{ "name": "U", "variants": [{ "tag": "b", "type": {"k":"ref","name":"Body","entity":false} }] }]"#,
        r#", { "name": "p", "type": {"k":"list","of":{"k":"union","name":"U"}}, "nullable": false }"#,
    )
    .unwrap_err();
    assert!(err.contains("lists of unions"), "{err}");

    // nested unions
    let err = load(
        r#"[
          { "name": "U", "variants": [{ "tag": "v", "type": {"k":"union","name":"V"} }] },
          { "name": "V", "variants": [{ "tag": "b", "type": {"k":"ref","name":"Body","entity":false} }] }
        ]"#,
        "",
    )
    .unwrap_err();
    assert!(err.contains("cannot nest"), "{err}");

    // entity variant
    let err = load(
        r#"[{ "name": "U", "variants": [{ "tag": "e", "type": {"k":"ref","name":"E","entity":true} }] }]"#,
        "",
    )
    .unwrap_err();
    assert!(err.contains("cannot be entities"), "{err}");

    // duplicate tags
    let err = load(
        r#"[{ "name": "U", "variants": [
          { "tag": "b", "type": {"k":"ref","name":"Body","entity":false} },
          { "tag": "b", "type": {"k":"ref","name":"Body","entity":false} }
        ] }]"#,
        "",
    )
    .unwrap_err();
    assert!(err.contains("duplicate variant tag"), "{err}");
}

#[test]
fn stream_op_projects_async_iterable_and_retains_poll_cursor() {
    // `Watch.events` (shape stream, returns `Event`) must project BOTH surfaces:
    // the primary JS async-iterable (napi 3 `#[napi(async_iterator)]` +
    // `impl AsyncGenerator`) AND the retained `next()` poll cursor. The class is
    // `Events`, the item `Event`. Substrings match the rustfmt'd emission.
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<(String, Vec<String>)> = Vec::new();
    let node = fluessig::bindgen::node_binding(&api, &enums, None);

    // async-iterable surface
    assert!(
        node.contains("#[napi(async_iterator)]"),
        "async-iterator attribute:\n{node}"
    );
    assert!(
        node.contains("impl AsyncGenerator for Events"),
        "AsyncGenerator impl on the stream class:\n{node}"
    );
    assert!(
        node.contains("type Yield = Event;"),
        "yields the item type:\n{node}"
    );
    // blocking poll driven off the runtime so the event loop is never blocked
    assert!(
        node.contains("napi::tokio::task::spawn_blocking"),
        "spawn_blocking drives the blocking poll:\n{node}"
    );

    // cancellation / close(): default no-op on the trait, called on complete + drop
    assert!(
        node.contains("fn close(&self) {}"),
        "default no-op close on the trait:\n{node}"
    );
    assert!(
        node.contains("stream.close();"),
        "cancellation closes the core stream:\n{node}"
    );
    assert!(
        node.contains("impl Drop for Events"),
        "Drop backstop closes the core stream:\n{node}"
    );

    // retained poll cursor still present
    assert!(
        node.contains("AsyncTask<NextEventsTask>"),
        "poll cursor retained:\n{node}"
    );
    assert!(
        node.contains("#[napi(ts_return_type = \"Promise<Event | null>\")]"),
        "poll cursor keeps its nullable Promise ts type:\n{node}"
    );
}
