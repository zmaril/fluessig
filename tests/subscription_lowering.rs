//! `Shape::Subscription` lowering: an op that REGISTERS a listener (its one
//! `ApiType::Callback` param) and returns a generated `Subscription` HANDLE whose
//! `unsubscribe()`/drop removes the listener. Builds directly on the merged
//! callback slice (#78) — the callback param still crosses in as node's napi
//! `ThreadsafeFunction` / python's PyO3 `PyObject`, bridged into the uniform core
//! shape `Box<dyn Fn(..) + Send + Sync>`; the NEW piece is that the core-trait
//! method RETURNS the unsubscribe closure (`Box<dyn Fn() + Send + Sync>`) and each
//! backend wraps that into its `Subscription` handle class.
//!
//! This suite pins, per backend, the generated handle class + its `unsubscribe`,
//! the op's handle-returning signature, and the reused TSFN/GIL callback wrapper,
//! plus the uniform core-trait signature, the `"shape":"subscription"` serde
//! round-trip, and the loader's exactly-one-callback-param rejection.
//!
//! straitjacket-allow-file:duplication — the node/python assertion pairs here are
//! DELIBERATELY parallel (one per backend), mirroring the sibling cross-backend
//! test files (`tests/callback_lowering.rs`, `tests/python_stream.rs`).

use fluessig::api::load_api;
use fluessig::bindgen::{node_binding, python_binding};

/// A stateful `Ticker` interface (it carries a `Ctor`, so a `&self` Subscription
/// method is legal): a ctor `new`, a `Shape::Subscription` op `on_tick` taking one
/// `listener: Callback<(int32)>` and returning a `Subscription` handle, and a plain
/// `Unary` op `tick`.
const API: &str = r#"{
  "fluessig": {"format": 1},
  "models": [],
  "unions": [],
  "interfaces": [
    {"name": "Ticker", "ops": [
      {"name": "new", "shape": "ctor", "params": [], "returns": "void"},
      {"name": "on_tick", "shape": "subscription", "params": [
        {"name": "listener", "type": {"callback": {"params": ["int32"]}}}
      ], "returns": "void"},
      {"name": "tick", "shape": "unary", "params": [], "returns": "void"}
    ]}
  ]
}"#;

/// The uniform core-side shape every backend's `<Iface>Core` trait sees for the
/// subscription op: it takes the bridged listener closure and RETURNS the
/// unsubscribe closure. rustfmt wraps this across lines, so assertions normalize
/// whitespace to a single line before matching.
const CORE_TRAIT_METHOD: &str = "fn on_tick(&self, listener: Box<dyn Fn(i32) + Send + Sync>) -> anyhow::Result<Box<dyn Fn() + Send + Sync>>";

/// Collapse every run of ASCII whitespace to a single space, so a rustfmt-wrapped
/// multi-line signature can be matched against its one-line canonical form.
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip ALL whitespace and the trailing comma rustfmt inserts before a closing
/// paren when it wraps a long param list (`listener: …,\n)` → `listener: …)`), so a
/// wrapped trait-method signature matches its one-line canonical form.
fn canon(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace(",)", ")")
}

#[test]
fn node_lowers_subscription_to_handle_class() {
    let api = load_api(API).unwrap();
    let out = node_binding(&api, &[], None);
    let flat_out = flat(&out);

    // the generated handle class + its unsubscribe, emitted ONCE.
    assert!(
        out.contains("pub struct Subscription"),
        "node emits the generated Subscription handle class:\n{out}"
    );
    assert!(
        flat_out.contains("unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>"),
        "node Subscription wraps the core's unsubscribe closure in a Mutex<Option<..>>:\n{out}"
    );
    assert!(
        out.contains("pub fn unsubscribe(&self)"),
        "node Subscription has an unsubscribe method:\n{out}"
    );
    assert!(
        flat_out.contains("if let Some(f) = self.unsub.lock().unwrap().take() { f(); }"),
        "node unsubscribe takes and calls the closure once:\n{out}"
    );
    // the op returns the handle class (fallible ⇒ Result<Subscription>).
    assert!(
        out.contains("-> Result<Subscription>"),
        "node subscription op returns Result<Subscription>:\n{out}"
    );
    // the callback param still crosses in as its napi TSFN (reused #78 lowering).
    assert!(
        out.contains("listener: ThreadsafeFunction<i32, ErrorStrategy::Fatal>"),
        "node subscription callback param is a ThreadsafeFunction:\n{out}"
    );
    assert!(
        out.contains("let listener_cb: Box<dyn Fn(i32) + Send + Sync>"),
        "node bridges the TSFN into the core boxed Fn:\n{out}"
    );
    assert!(
        out.contains("tsfn.call(v, ThreadsafeFunctionCallMode::NonBlocking)"),
        "node forwards to the TSFN NonBlocking:\n{out}"
    );
    // the core's returned unsubscribe closure is wrapped into the handle.
    assert!(
        out.contains("let unsub = self.core.on_tick(listener_cb).map_err(err)?;"),
        "node calls the core subscription op and keeps its unsubscribe closure:\n{out}"
    );
    assert!(
        flat_out.contains("Ok(Subscription { unsub: std::sync::Mutex::new(Some(unsub)), })"),
        "node wraps the core unsubscribe closure into a Subscription handle:\n{out}"
    );
    // the uniform core-trait method (register-in, unsubscribe-out).
    assert!(
        canon(&out).contains(&canon(CORE_TRAIT_METHOD)),
        "node core trait sees the register→unsubscribe method:\n{out}"
    );
}

#[test]
fn python_lowers_subscription_to_handle_class() {
    let api = load_api(API).unwrap();
    let out = python_binding(&api, &[], None);
    let flat_out = flat(&out);

    // the generated pyclass handle + its unsubscribe, emitted ONCE.
    assert!(
        out.contains("#[pyclass]"),
        "python emits pyclass attrs:\n{out}"
    );
    assert!(
        out.contains("pub struct Subscription"),
        "python emits the generated Subscription pyclass:\n{out}"
    );
    assert!(
        flat_out.contains("unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>"),
        "python Subscription wraps the core's unsubscribe closure in a Mutex<Option<..>>:\n{out}"
    );
    assert!(
        out.contains("fn unsubscribe(&self)"),
        "python Subscription has an unsubscribe method:\n{out}"
    );
    assert!(
        flat_out.contains("if let Some(f) = self.unsub.lock().unwrap().take() { f(); }"),
        "python unsubscribe takes and calls the closure once:\n{out}"
    );
    // the op returns the handle class (fallible ⇒ PyResult<Subscription>).
    assert!(
        out.contains("-> PyResult<Subscription>"),
        "python subscription op returns PyResult<Subscription>:\n{out}"
    );
    // the callback param still crosses in as a Python callable (reused #78 lowering).
    assert!(
        out.contains("listener: PyObject"),
        "python subscription callback param is a PyObject:\n{out}"
    );
    assert!(
        out.contains("let listener_cb: Box<dyn Fn(i32) + Send + Sync>"),
        "python bridges the callable into the core boxed Fn:\n{out}"
    );
    assert!(
        out.contains("Python::with_gil(|py| {"),
        "python invokes the callable under the GIL:\n{out}"
    );
    assert!(
        out.contains("let unsub = self.core.on_tick(listener_cb).map_err(err)?;"),
        "python calls the core subscription op and keeps its unsubscribe closure:\n{out}"
    );
    assert!(
        flat_out.contains("Ok(Subscription { unsub: std::sync::Mutex::new(Some(unsub)), })"),
        "python wraps the core unsubscribe closure into a Subscription handle:\n{out}"
    );
    // the Subscription class is registered on the pymodule.
    assert!(
        out.contains("m.add_class::<Subscription>()?;"),
        "python registers the Subscription class on the module:\n{out}"
    );
    // the uniform core-trait method (register-in, unsubscribe-out).
    assert!(
        canon(&out).contains(&canon(CORE_TRAIT_METHOD)),
        "python core trait sees the register→unsubscribe method:\n{out}"
    );
}

/// `Shape::Subscription` serializes as the lowercase `"subscription"` shape and
/// round-trips through serde byte-for-byte.
#[test]
fn subscription_shape_serde_round_trips() {
    let api = load_api(API).unwrap();
    let json = serde_json::to_string(&api).unwrap();
    assert!(
        json.contains(r#""shape":"subscription""#),
        "the subscription op serializes with shape=subscription:\n{json}"
    );
    // deserialize the re-serialized doc → still loads (round-trip is lossless).
    let back = load_api(&json).unwrap();
    let json2 = serde_json::to_string(&back).unwrap();
    assert_eq!(json, json2, "subscription doc round-trips byte-for-byte");
}

/// A `Shape::Subscription` op MUST have exactly one callback param; the loader
/// rejects zero or two so the IR never claims a subscription shape no backend can
/// wrap into a single listener registration.
#[test]
fn loader_rejects_wrong_callback_arity() {
    // zero callback params.
    let zero = r#"{
      "fluessig": {"format": 1},
      "models": [], "unions": [],
      "interfaces": [
        {"name": "Ticker", "ops": [
          {"name": "new", "shape": "ctor", "params": [], "returns": "void"},
          {"name": "on_tick", "shape": "subscription", "params": [
            {"name": "count", "type": "int32"}
          ], "returns": "void"}
        ]}
      ]
    }"#;
    let err = load_api(zero).expect_err("subscription op with zero callback params is rejected");
    assert!(
        err.contains("must have exactly one callback param"),
        "clear arity error for zero callbacks, got: {err}"
    );

    // two callback params.
    let two = r#"{
      "fluessig": {"format": 1},
      "models": [], "unions": [],
      "interfaces": [
        {"name": "Ticker", "ops": [
          {"name": "new", "shape": "ctor", "params": [], "returns": "void"},
          {"name": "on_tick", "shape": "subscription", "params": [
            {"name": "a", "type": {"callback": {"params": ["int32"]}}},
            {"name": "b", "type": {"callback": {"params": ["int32"]}}}
          ], "returns": "void"}
        ]}
      ]
    }"#;
    let err = load_api(two).expect_err("subscription op with two callback params is rejected");
    assert!(
        err.contains("must have exactly one callback param"),
        "clear arity error for two callbacks, got: {err}"
    );
}

/// A `Shape::Subscription` op requires a CONSTRUCTIBLE interface (its method is
/// `&self`). The loader rejects a subscription op on an interface that NOTHING
/// constructs — here `Ticker` has no ctor of its own AND no op anywhere returns it,
/// so there is no live instance to hang the `&self` method on. (A ctor-less
/// interface that a FACTORY op DOES return is now accepted — see
/// `subscription_on_factory_born_interface_loads` in `src/api.rs`.)
#[test]
fn loader_rejects_unconstructible_subscription() {
    let stateless = r#"{
      "fluessig": {"format": 1},
      "models": [], "unions": [],
      "interfaces": [
        {"name": "Ticker", "ops": [
          {"name": "on_tick", "shape": "subscription", "params": [
            {"name": "listener", "type": {"callback": {"params": ["int32"]}}}
          ], "returns": "void"}
        ]}
      ]
    }"#;
    let err = load_api(stateless)
        .expect_err("subscription op on an unconstructible interface is rejected");
    assert!(
        err.contains("requires a constructible interface") && err.contains("nothing constructs"),
        "clear unconstructible-interface error, got: {err}"
    );
}
