//! First slice of language-agnostic callback types: `ApiType::Callback` lowered
//! to the node napi `ThreadsafeFunction` and the python PyO3 `PyObject`, both
//! bridged into the ONE uniform core-side shape `Box<dyn Fn(..) + Send + Sync>`.
//!
//! The IR carries a forward-only sync-void callback param; each backend's
//! generated binding wraps its native callable into the boxed `Fn` at the FFI
//! boundary. This suite pins the three load-bearing facts per backend (binding
//! param type, the non-blocking/GIL wrapper, the bridged core call) plus the
//! uniform core-trait signature, and checks the loader rejects the not-yet-lowered
//! async/fallible/non-void callback shapes.
//!
//! straitjacket-allow-file:duplication — the node/python assertion pairs here are
//! DELIBERATELY parallel (one per backend), mirroring the sibling cross-backend
//! test files (`tests/union_structured_langs.rs`, `tests/python_stream.rs`).

use fluessig::api::load_api;
use fluessig::bindgen::{node_binding, python_binding};

/// A `Ticker` interface with one stateless (no ctor ⇒ free-function) `Unary` op
/// `each_tick(count: int32, listener: Callback<(int32)>) -> void`. The callback is
/// a plain forward-only sync-void one, so it serializes to just
/// `{"callback":{"params":["int32"]}}`.
const API: &str = r#"{
  "fluessig": {"format": 1},
  "models": [],
  "unions": [],
  "interfaces": [
    {"name": "Ticker", "ops": [
      {"name": "each_tick", "shape": "unary", "params": [
        {"name": "count", "type": "int32"},
        {"name": "listener", "type": {"callback": {"params": ["int32"]}}}
      ], "returns": "void"}
    ]}
  ]
}"#;

/// The uniform core-side shape every backend's `<Iface>Core` trait sees for the
/// callback — the whole point of the design (the core never learns the source
/// language). Both backends must emit exactly this trait method.
const CORE_TRAIT_METHOD: &str =
    "fn each_tick(count: i32, listener: Box<dyn Fn(i32) + Send + Sync>) -> anyhow::Result<()>";

#[test]
fn node_lowers_callback_to_threadsafe_function() {
    let api = load_api(API).unwrap();
    let out = node_binding(&api, &[], None);

    // binding param position: the callback crosses in as a napi TSFN.
    assert!(
        out.contains("listener: ThreadsafeFunction<i32, ErrorStrategy::Fatal>"),
        "node callback param is a ThreadsafeFunction:\n{out}"
    );
    // the non-blocking bridge into the core `Box<dyn Fn>`.
    assert!(
        out.contains("let listener_cb: Box<dyn Fn(i32) + Send + Sync>"),
        "node wraps the TSFN into the core boxed Fn:\n{out}"
    );
    assert!(
        out.contains("tsfn.call(v, ThreadsafeFunctionCallMode::NonBlocking)"),
        "node forwards to the TSFN NonBlocking (never blocks the caller thread):\n{out}"
    );
    // the bridged local — not the raw TSFN — is passed into the core call.
    assert!(
        out.contains("<crate::core_impl::TickerImpl as TickerCore>::each_tick(count, listener_cb)"),
        "node passes the bridged closure into the core trait call:\n{out}"
    );
    // the threadsafe_function imports are pulled into the prelude.
    assert!(
        out.contains(
            "use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};"
        ),
        "node prelude imports the threadsafe_function trio:\n{out}"
    );
    // the uniform core-trait method.
    assert!(
        out.contains(CORE_TRAIT_METHOD),
        "node core trait sees the uniform boxed Fn:\n{out}"
    );
}

#[test]
fn python_lowers_callback_to_pyobject() {
    let api = load_api(API).unwrap();
    let out = python_binding(&api, &[], None);

    // binding param position: the callback crosses in as a Python callable.
    assert!(
        out.contains("listener: PyObject"),
        "python callback param is a PyObject:\n{out}"
    );
    // the GIL-holding bridge into the core `Box<dyn Fn>`.
    assert!(
        out.contains("let listener_cb: Box<dyn Fn(i32) + Send + Sync>"),
        "python wraps the callable into the core boxed Fn:\n{out}"
    );
    assert!(
        out.contains("Python::with_gil(|py| {"),
        "python invokes the callable under the (re-entrant) GIL:\n{out}"
    );
    assert!(
        out.contains("cb.call1(py, (v,))"),
        "python calls the host callable with the forwarded arg:\n{out}"
    );
    // callback-carrying op HOLDS the GIL — no `py.detach` around the core call.
    assert!(
        !out.contains("py.detach"),
        "python does NOT release the GIL for a callback-carrying op:\n{out}"
    );
    assert!(
        out.contains("<crate::core_impl::TickerImpl as TickerCore>::each_tick(count, listener_cb)"),
        "python passes the bridged closure into the core trait call:\n{out}"
    );
    // the uniform core-trait method.
    assert!(
        out.contains(CORE_TRAIT_METHOD),
        "python core trait sees the uniform boxed Fn:\n{out}"
    );
}

/// The plain forward-only sync-void callback round-trips through serde with the
/// reserved fields omitted, so existing goldens stay untouched.
#[test]
fn plain_callback_serializes_without_reserved_fields() {
    let api = load_api(API).unwrap();
    let json = serde_json::to_string(&api).unwrap();
    assert!(
        json.contains(r#"{"callback":{"params":["int32"]}}"#),
        "a sync-void callback omits returns/isAsync/fallible:\n{json}"
    );
}

/// This slice lowers ONLY forward-only sync-void callbacks; the loader rejects an
/// async / fallible / non-void-returning callback param so the IR never claims a
/// shape no backend wraps.
#[test]
fn loader_rejects_not_yet_lowered_callback_shapes() {
    for bad in [
        r#"{"callback": {"params": ["int32"], "isAsync": true}}"#,
        r#"{"callback": {"params": ["int32"], "fallible": true}}"#,
        r#"{"callback": {"params": ["int32"], "returns": "int32"}}"#,
    ] {
        let api = format!(
            r#"{{
              "fluessig": {{"format": 1}},
              "models": [], "unions": [],
              "interfaces": [
                {{"name": "Ticker", "ops": [
                  {{"name": "each_tick", "shape": "unary", "params": [
                    {{"name": "listener", "type": {bad}}}
                  ], "returns": "void"}}
                ]}}
              ]
            }}"#
        );
        let err = load_api(&api).expect_err("not-yet-lowered callback shape is rejected");
        assert!(
            err.contains("only forward-only sync void callbacks are supported"),
            "clear validation error for `{bad}`, got: {err}"
        );
    }
}
