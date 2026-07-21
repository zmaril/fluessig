//! The Ruby (magnus) backend's callback + subscription lowering (follow-up to the
//! merged callback IR #78, `Shape::Subscription` #85, and the cpp #87 / java #88
//! backends). A forward-only sync-void `ApiType::Callback` param crosses the magnus
//! seam as a Ruby `Proc`: the generated glue pins it in a `RubyCb` newtype
//! (`BoxValue` for GC + `unsafe impl Send/Sync`, sound because the closure is only
//! ever invoked on the Ruby thread under the GVL) and wraps it into the ONE uniform
//! core shape `Box<dyn Fn(..) + Send + Sync>` — on invoke it re-acquires the `Ruby`
//! handle and calls the `Proc`. A `Shape::Subscription` op returns an opaque
//! `Subscription` wrapped class owning the core's unsubscribe closure, with an
//! `unsubscribe` method (take-and-call, idempotent) + a `Drop` that unsubscribes.
//!
//! This suite pins the load-bearing facts of the generated magnus Rust glue, plus
//! the uniform core-trait method. The real compile+run proof lives in
//! `crates/callback-demo-ruby` (consumer.rb subscribes a Ruby `Proc`, ticks twice
//! → sees `[0, 1]`, then silence after unsubscribe — a real magnus extension built
//! and loaded against ruby 3.3.6).
//!
//! straitjacket-allow-file:duplication — the assertion blocks are DELIBERATELY
//! parallel to the sibling cross-backend test files
//! (`tests/java_callback_lowering.rs`, `tests/cpp_callback_lowering.rs`,
//! `tests/callback_lowering.rs`).

use fluessig::api::load_api;
use fluessig::bindgen::ruby_binding;

/// A stateful `Ticker` (it carries a `Ctor`, so a `&self` Subscription method is
/// legal): a ctor `new`, an INFALLIBLE `Shape::Subscription` op `on_tick` taking
/// one `listener: Callback<(int32)>` and returning a `Subscription` handle, and a
/// plain infallible `Unary` op `tick`. Matches the `crates/callback-demo-ruby`
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
/// output byte-identical (no `RubyCb`, no `Subscription` handle).
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
/// node/python/cpp/java emit for the infallible register→unsubscribe method).
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
fn ruby_glue_lowers_callback_and_subscription() {
    let api = load_api(API).unwrap();
    let out = ruby_binding(&api, &[], None);

    // the callback param crosses in as a magnus Proc (the Ruby block/lambda).
    assert!(
        out.contains("fn on_tick(&self, listener: magnus::block::Proc)"),
        "ruby callback param is a magnus::block::Proc:\n{out}"
    );

    // the RubyCb newtype: a GC-boxed Proc asserted Send + Sync (sound under the
    // GVL invariant), mirroring cpp's CbCtx / java's global-ref wrapper.
    assert!(
        out.contains("struct RubyCb(magnus::value::BoxValue<magnus::block::Proc>);")
            && out.contains("unsafe impl Send for RubyCb {}")
            && out.contains("unsafe impl Sync for RubyCb {}"),
        "ruby emits the unsafe-Send RubyCb wrapper over a BoxValue<Proc>:\n{out}"
    );

    // it is wrapped into the uniform core boxed Fn and, on invoke, calls the Proc
    // (the GVL is proven by re-acquiring the Ruby handle in the core-side closure).
    assert!(
        canon(&out).contains(&canon("let listener: Box<dyn Fn(i32) + Send + Sync> =")),
        "ruby wraps the Proc into the core boxed Fn:\n{out}"
    );
    assert!(
        canon(&out).contains(&canon(
            "let __cb = RubyCb(magnus::value::BoxValue::new(listener));"
        )) && canon(&out).contains(&canon(
            "let _: Result<magnus::Value, magnus::Error> = __p.call((v,));"
        )),
        "ruby boxes the Proc for GC and invokes it under the GVL:\n{out}"
    );

    // the opaque Subscription wrapped class + its idempotent lifecycle.
    assert!(
        out.contains("#[magnus::wrap(class = \"Ticker::Subscription\", free_immediately, size)]")
            && out.contains("struct Subscription {")
            && canon(&out).contains(&canon(
                "unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>"
            ))
            && out.contains("fn unsubscribe(&self) {")
            && out.contains("impl Drop for Subscription {"),
        "ruby emits the Subscription wrapped class with unsubscribe + Drop:\n{out}"
    );

    // the subscription op REGISTERS the listener + returns an owning Subscription.
    assert!(
        out.contains(
            "fn on_tick(&self, listener: magnus::block::Proc) -> Result<Subscription, Error>"
        ) && out.contains("let unsub = self.core.on_tick(listener);")
            && canon(&out).contains(&canon(
                "Ok(Subscription { unsub: std::sync::Mutex::new(Some(unsub)) })"
            )),
        "ruby on_tick registers the listener + returns a Subscription handle:\n{out}"
    );

    // the class + its Subscription handle are registered for #[magnus::init].
    assert!(
        out.contains(
            "let subscription = class.define_class(\"Subscription\", ruby.class_object())?;"
        ) && out.contains(
            "subscription.define_method(\"unsubscribe\", method!(Subscription::unsubscribe, 0))?;"
        ) && out.contains("class.define_method(\"on_tick\", method!(Ticker::on_tick, 1))?;"),
        "ruby registers the Subscription class + on_tick method:\n{out}"
    );

    // the uniform core-trait method (register-in, unsubscribe-out).
    assert!(
        canon(&out).contains(&canon(CORE_TRAIT_METHOD)),
        "ruby core trait sees the register→unsubscribe method:\n{out}"
    );
}

#[test]
fn callback_free_surface_stays_byte_identical() {
    let api = load_api(PLAIN_API).unwrap();
    let out = ruby_binding(&api, &[], None);
    // The RubyCb wrapper + Subscription handle are strictly gated: a schema with
    // no subscription op / callback param emits ZERO of them.
    assert!(
        !out.contains("struct RubyCb(")
            && !out.contains("struct Subscription {")
            && !out.contains("magnus::block::Proc")
            && !out.contains("BoxValue"),
        "a callback/subscription-free surface emits no RubyCb/Subscription glue:\n{out}"
    );
}
