//! The wasm-bindgen backend's callback + subscription lowering (follow-up to the
//! merged callback IR #78, `Shape::Subscription` #85, and the cpp #87 / java #88 /
//! ruby #89 backends). A forward-only sync-void `ApiType::Callback` param crosses
//! the wasm-bindgen seam as a `js_sys::Function`: the generated glue wraps it in a
//! `WasmCb` newtype (`unsafe impl Send/Sync`, sound because `wasm32-unknown-unknown`
//! is single-threaded, so the closure is only ever invoked on the one JS thread)
//! and marshals it into the ONE uniform core shape `Box<dyn Fn(..) + Send + Sync>`
//! — on invoke it calls the JS `Function` (`call1`), discarding any result. A
//! `Shape::Subscription` op returns a `#[wasm_bindgen]`-exported `Subscription`
//! handle owning the core's unsubscribe closure, with an `unsubscribe` method
//! (take-and-call, idempotent).
//!
//! This suite pins the load-bearing facts of the generated wasm-bindgen Rust glue,
//! plus the uniform core-trait method. The real compile proof lives in
//! `crates/callback-demo-wasm` (a `Ticker` binding + hand-written core that
//! COMPILES to `wasm32-unknown-unknown` — the way the wasm backend is validated
//! without a runnable browser/node harness).
//!
//! straitjacket-allow-file:duplication — the assertion blocks are DELIBERATELY
//! parallel to the sibling cross-backend test files
//! (`tests/ruby_callback_lowering.rs`, `tests/java_callback_lowering.rs`,
//! `tests/cpp_callback_lowering.rs`, `tests/callback_lowering.rs`).

use fluessig::api::load_api;
use fluessig::bindgen::wasm_binding;

/// A stateful `Ticker` (it carries a `Ctor`, so a `&self` Subscription method is
/// legal): a ctor `new`, an INFALLIBLE `Shape::Subscription` op `on_tick` taking
/// one `listener: Callback<(int32)>` and returning a `Subscription` handle, and a
/// plain infallible `Unary` op `tick`. Matches the `crates/callback-demo-wasm`
/// fixture.
const API: &str = r#"{
  "fluessig": {"format": 1},
  "models": [],
  "unions": [],
  "interfaces": [
    {"name": "Ticker", "ops": [
      {"name": "new", "shape": "ctor", "params": [], "returns": "void"},
      {"name": "on_tick", "shape": "subscription", "infallible": true, "params": [
        {"name": "listener", "type": {"callback": {"params": ["int32"]}}}
      ], "returns": "void"},
      {"name": "tick", "shape": "unary", "infallible": true, "params": [], "returns": "void"}
    ]}
  ]
}"#;

/// A callback/subscription-free surface: proves the gates keep such a schema's
/// output byte-identical (no `WasmCb`, no `Subscription` handle).
const PLAIN_API: &str = r#"{
  "fluessig": {"format": 1},
  "models": [],
  "unions": [],
  "interfaces": [
    {"name": "Store", "ops": [
      {"name": "open", "shape": "ctor", "params": [], "returns": "void"},
      {"name": "ping", "shape": "unary", "infallible": true, "params": [], "returns": "void"}
    ]}
  ]
}"#;

/// The uniform core-side shape the `<Iface>Core` trait sees (identical to what
/// node/python/cpp/java/ruby emit for the infallible register→unsubscribe method).
const CORE_TRAIT_METHOD: &str =
    "fn on_tick(&self, listener: Box<dyn Fn(i32) + Send + Sync>) -> Box<dyn Fn() + Send + Sync>";

/// Strip ALL whitespace and the trailing comma rustfmt inserts before a closing
/// paren/brace when it wraps a long param list or struct literal, so a wrapped
/// form matches its one-line canonical form.
fn canon(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace(",)", ")")
        .replace(",}", "}")
}

#[test]
fn wasm_glue_lowers_callback_and_subscription() {
    let api = load_api(API).unwrap();
    let out = wasm_binding(&api, &[], None);

    // the callback param crosses in as a JS `js_sys::Function`.
    assert!(
        out.contains("pub fn on_tick(&self, listener: js_sys::Function)"),
        "wasm callback param is a js_sys::Function:\n{out}"
    );

    // the WasmCb newtype: a JS `Function` asserted Send + Sync (sound under the
    // single-threaded wasm invariant), mirroring cpp's CbCtx / ruby's RubyCb.
    assert!(
        out.contains("struct WasmCb(js_sys::Function);")
            && out.contains("unsafe impl Send for WasmCb {}")
            && out.contains("unsafe impl Sync for WasmCb {}"),
        "wasm emits the unsafe-Send WasmCb wrapper over a js_sys::Function:\n{out}"
    );

    // it is wrapped into the uniform core boxed Fn and, on invoke, calls the JS
    // `Function` (`call1`) with the marshalled arg, discarding the result.
    assert!(
        canon(&out).contains(&canon("let listener: Box<dyn Fn(i32) + Send + Sync> =")),
        "wasm wraps the js_sys::Function into the core boxed Fn:\n{out}"
    );
    assert!(
        canon(&out).contains(&canon("let __cb = WasmCb(listener);"))
            && canon(&out).contains(&canon(
                "let _ = __cb.func().call1(&JsValue::NULL, &JsValue::from(v));"
            )),
        "wasm keeps the Function in a WasmCb and invokes it via call1:\n{out}"
    );

    // the #[wasm_bindgen]-exported Subscription handle + its idempotent unsubscribe.
    assert!(
        out.contains("#[wasm_bindgen]")
            && out.contains("pub struct Subscription {")
            && canon(&out).contains(&canon(
                "unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>"
            ))
            && out.contains("pub fn unsubscribe(&self) {"),
        "wasm emits the #[wasm_bindgen] Subscription handle with unsubscribe:\n{out}"
    );

    // the subscription op REGISTERS the listener + returns an owning Subscription.
    assert!(
        out.contains("pub fn on_tick(&self, listener: js_sys::Function) -> Subscription")
            && out.contains("let unsub = self.inner.on_tick(listener);")
            && canon(&out).contains(&canon(
                "Subscription { unsub: std::sync::Mutex::new(Some(unsub)) }"
            )),
        "wasm on_tick registers the listener + returns a Subscription handle:\n{out}"
    );

    // the JS-facing method name honours the wasm-bindgen js_name rename lever.
    assert!(
        out.contains("#[wasm_bindgen(js_name = \"onTick\")]"),
        "wasm on_tick is exported under its lowerCamel js_name:\n{out}"
    );

    // the uniform core-trait method (register-in, unsubscribe-out).
    assert!(
        canon(&out).contains(&canon(CORE_TRAIT_METHOD)),
        "wasm core trait sees the register→unsubscribe method:\n{out}"
    );
}

#[test]
fn callback_free_surface_stays_byte_identical() {
    let api = load_api(PLAIN_API).unwrap();
    let out = wasm_binding(&api, &[], None);
    // The WasmCb wrapper + Subscription handle are strictly gated: a schema with
    // no subscription op / callback param emits ZERO of them.
    assert!(
        !out.contains("struct WasmCb(")
            && !out.contains("pub struct Subscription {")
            && !out.contains("js_sys::Function"),
        "a callback/subscription-free surface emits no WasmCb/Subscription glue:\n{out}"
    );
}
