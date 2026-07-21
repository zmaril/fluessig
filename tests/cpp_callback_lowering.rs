//! The C/C++ backend's callback + subscription lowering (follow-up to the merged
//! callback IR #78 + `Shape::Subscription` #85, which landed node/python). A
//! forward-only sync-void `ApiType::Callback` param crosses the C ABI as a
//! function pointer + `void* ctx` pair, wrapped into the ONE uniform core shape
//! `Box<dyn Fn(..) + Send + Sync>`; a `Shape::Subscription` op returns an opaque
//! `Subscription*` handle owning the core's unsubscribe closure.
//!
//! This suite pins the load-bearing facts across all three cpp artifacts (the
//! Rust `extern "C"` export layer, the C header, the C++ RAII wrapper), plus the
//! uniform core-trait method. The real compile+run proof lives in
//! `crates/cpp-demo` (consumer.c / consumer.cpp fire a host callback from Rust).
//!
//! straitjacket-allow-file:duplication — the three per-artifact assertion blocks
//! are DELIBERATELY parallel (one per C ABI projection), mirroring the sibling
//! cross-backend test files (`tests/callback_lowering.rs`, `tests/cpp_catalog.rs`).

use fluessig::api::load_api;
use fluessig::bindgen::{cpp_binding, cpp_header, cpp_hpp};

/// A stateful `Ticker` (it carries a `Ctor`, so a `&self` Subscription method is
/// legal): a ctor `new`, a `Shape::Subscription` op `on_tick` taking one
/// `listener: Callback<(int32)>` and returning a `Subscription` handle, and a plain
/// `Unary` op `tick`. Same fixture the node/python subscription suite uses.
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

/// The uniform core-side shape the `<Iface>Core` trait sees (identical to what
/// node/python emit): register the bridged listener, return the unsubscribe
/// closure. rustfmt wraps it across lines, so match on a whitespace-stripped form.
const CORE_TRAIT_METHOD: &str = "fn on_tick(&self, listener: Box<dyn Fn(i32) + Send + Sync>) -> anyhow::Result<Box<dyn Fn() + Send + Sync>>";

/// Strip ALL whitespace and the trailing comma rustfmt inserts before a closing
/// paren when it wraps a long param list, so a wrapped signature matches its
/// one-line canonical form.
fn canon(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace(",)", ")")
}

#[test]
fn cpp_binding_lowers_callback_and_subscription() {
    let api = load_api(API).unwrap();
    let out = cpp_binding(&api, &[], None);

    // the callback-context newtype (asserted `Send`/`Sync` because the C caller
    // owns thread-safety), read through a method so the closure captures the whole
    // newtype (RFC 2229 disjoint capture would otherwise grab the raw pointer).
    assert!(
        out.contains("struct CbCtx(*mut c_void);")
            && out.contains("unsafe impl Send for CbCtx {}")
            && out.contains("unsafe impl Sync for CbCtx {}"),
        "cpp binding emits the CbCtx newtype:\n{out}"
    );
    // the callback param crosses in as a fn-ptr + ctx pair.
    assert!(
        out.contains("listener: extern \"C\" fn(*mut c_void, i32)")
            && out.contains("listener_ctx: *mut c_void"),
        "cpp callback param is a C fn-ptr + void* ctx:\n{out}"
    );
    // the fn-ptr + ctx are wrapped into the uniform core boxed Fn.
    assert!(
        out.contains("let listener_cb: Box<dyn Fn(i32) + Send + Sync> ="),
        "cpp wraps the fn-ptr + ctx into the core boxed Fn:\n{out}"
    );
    assert!(
        out.contains("let ctx = CbCtx(listener_ctx);") && out.contains("(listener)(ctx.ptr(), v)"),
        "cpp forwards to the C fn-ptr through the CbCtx newtype:\n{out}"
    );
    // the opaque Subscription handle + its lifecycle fns, emitted once.
    assert!(
        out.contains("pub struct Subscription {")
            && out.contains(
                "pub unsafe extern \"C\" fn Subscription_unsubscribe(s: *mut Subscription)"
            )
            && out.contains("pub unsafe extern \"C\" fn Subscription_free(s: *mut Subscription)"),
        "cpp emits the Subscription handle + unsubscribe/free:\n{out}"
    );
    // the subscription op returns the opaque handle (fallible ⇒ out + err_out).
    assert!(
        out.contains("out: *mut *mut Subscription")
            && out.contains("*out = Box::into_raw(Box::new(Subscription {")
            && out.contains("this.on_tick(listener_cb)"),
        "cpp on_tick registers the listener + returns a Subscription*:\n{out}"
    );
    // the uniform core-trait method (register-in, unsubscribe-out).
    assert!(
        canon(&out).contains(&canon(CORE_TRAIT_METHOD)),
        "cpp core trait sees the register→unsubscribe method:\n{out}"
    );
}

#[test]
fn cpp_header_declares_callback_and_subscription() {
    let api = load_api(API).unwrap();
    let h = cpp_header(&api, &[], None);

    // the opaque subscription handle typedef.
    assert!(
        h.contains("typedef struct Subscription Subscription;"),
        "cpp header declares the opaque Subscription handle:\n{h}"
    );
    // the callback param prototype: a fn pointer + void* ctx.
    assert!(
        h.contains("void (*listener)(void* ctx, int32_t v), void* listener_ctx"),
        "cpp header spells the callback param as a fn-ptr + ctx:\n{h}"
    );
    // the subscription op returns the handle (fallible ⇒ int + out + err_out).
    assert!(
        h.contains("int Ticker_on_tick(Ticker* self, void (*listener)(void* ctx, int32_t v), void* listener_ctx, Subscription** out, char** err_out);"),
        "cpp header declares Ticker_on_tick returning a Subscription*:\n{h}"
    );
    // the subscription lifecycle free fns.
    assert!(
        h.contains("void Subscription_unsubscribe(Subscription* s);")
            && h.contains("void Subscription_free(Subscription* s);"),
        "cpp header declares the Subscription lifecycle fns:\n{h}"
    );
}

#[test]
fn cpp_hpp_wraps_subscription_in_raii() {
    let api = load_api(API).unwrap();
    let hpp = cpp_hpp(&api, &[], None);

    // the RAII Subscription class + its unsubscribe method.
    assert!(
        hpp.contains("class Subscription {") && hpp.contains("void unsubscribe()"),
        "cpp hpp emits the RAII Subscription class:\n{hpp}"
    );
    // the destructor frees the handle (unsubscribing) + deletes the listener.
    assert!(
        hpp.contains("::Subscription_free(h_)") && hpp.contains("deleter_(ctx_)"),
        "cpp hpp Subscription destructor frees the handle + listener:\n{hpp}"
    );
    // the method takes a std::function listener and returns the RAII Subscription.
    assert!(
        hpp.contains("Subscription on_tick(std::function<void(int32_t)> listener)"),
        "cpp hpp on_tick takes a std::function + returns a Subscription:\n{hpp}"
    );
    // it thunks the std::function into the C fn-ptr + ctx and calls the C op.
    assert!(
        hpp.contains("auto thunk = [](void* c, int32_t v)")
            && hpp.contains("::Ticker_on_tick(h_, thunk, ctx, &out, &err)"),
        "cpp hpp thunks the std::function into the C fn-ptr + ctx:\n{hpp}"
    );
    // the std::function include is pulled in (gated on subscription usage).
    assert!(
        hpp.contains("#include <functional>"),
        "cpp hpp includes <functional> for the std::function listener:\n{hpp}"
    );
}
