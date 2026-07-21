//! The PHP (ext-php-rs) backend's callback + subscription lowering (the LAST of
//! the per-backend series, after the merged callback IR #78, `Shape::Subscription`
//! #85, and the cpp #87 / java #88 / ruby #89 / wasm #90 backends). A forward-only
//! sync-void `ApiType::Callback` param crosses the ext-php-rs seam as a PHP
//! `callable` (`&Zval`): the generated glue `shallow_clone`s it (bumping the
//! closure's refcount so it outlives the call), pins it as an owned
//! `ZendCallable<'static>` inside a `PhpCb` newtype (`unsafe impl Send/Sync`), and
//! wraps it into the ONE uniform core shape `Box<dyn Fn(..) + Send + Sync>` — on
//! invoke it calls the callable via `try_call`. A `Shape::Subscription` op returns
//! an opaque `#[php_class]` `Subscription` handle owning the core's unsubscribe
//! closure, with an `unsubscribe` method (take-and-call, idempotent).
//!
//! PHP is DOCUMENTED SYNC-ONLY (the coordinator ruling): the `PhpCb` newtype
//! asserts `Send`/`Sync` over a `!Send` PHP callable, sound only under synchronous
//! same-request-thread invocation (off-thread is UB). This suite pins that the
//! compile-time-visible sync-only marker is emitted, plus the load-bearing facts
//! of the generated ext-php-rs Rust glue and the uniform core-trait method. The
//! real compile+run proof lives in `crates/callback-demo-php` (consumer.php
//! subscribes a PHP `Closure`, ticks twice → sees `[0, 1]`, then silence after
//! unsubscribe — a real ext-php-rs extension built and loaded against PHP 8.4).
//!
//! straitjacket-allow-file:duplication — the assertion blocks are DELIBERATELY
//! parallel to the sibling cross-backend test files
//! (`tests/ruby_callback_lowering.rs`, `tests/wasm_callback_lowering.rs`,
//! `tests/java_callback_lowering.rs`, `tests/cpp_callback_lowering.rs`,
//! `tests/callback_lowering.rs`).

use fluessig::api::load_api;
use fluessig::bindgen::php_binding;

/// A stateful `Ticker` (it carries a `Ctor`, so a `&self` Subscription method is
/// legal): a ctor `new`, an INFALLIBLE `Shape::Subscription` op `on_tick` taking
/// one `listener: Callback<(int32)>` and returning a `Subscription` handle, and a
/// plain infallible `Unary` op `tick`. Matches the `crates/callback-demo-php`
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
/// output byte-identical (no `PhpCb`, no `Subscription` handle).
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
/// node/python/cpp/java/ruby/wasm emit for the infallible register→unsubscribe
/// method).
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
fn php_glue_lowers_callback_and_subscription() {
    let api = load_api(API).unwrap();
    let out = php_binding(&api, &[], None);

    // the callback param crosses in as a raw callable `&Zval` (not the uniform
    // core box — the conv prelude wraps it).
    assert!(
        out.contains("fn on_tick(&self, listener: &ext_php_rs::types::Zval)"),
        "php callback param is a callable &Zval:\n{out}"
    );

    // the PhpCb newtype: an owned ZendCallable asserted Send + Sync, mirroring
    // cpp's CbCtx / ruby's RubyCb / wasm's WasmCb.
    assert!(
        out.contains("struct PhpCb(ext_php_rs::types::ZendCallable<'static>);")
            && out.contains("unsafe impl Send for PhpCb {}")
            && out.contains("unsafe impl Sync for PhpCb {}"),
        "php emits the unsafe-Send PhpCb wrapper over an owned ZendCallable:\n{out}"
    );

    // THE COMPILE-TIME-VISIBLE SYNC-ONLY MARKER (the coordinator ruling): a LOUD
    // doc comment stating the callable is only valid to invoke synchronously on
    // the PHP request thread, and that off-thread invocation is undefined behaviour.
    assert!(
        out.contains("# SYNC-ONLY — off-thread invocation is undefined behaviour"),
        "php emits the sync-only doc marker heading on PhpCb:\n{out}"
    );
    assert!(
        out.contains("SAFETY: sound ONLY under the sync-only contract"),
        "php emits the sync-only SAFETY note on the unsafe Send/Sync impls:\n{out}"
    );

    // it is wrapped into the uniform core boxed Fn and, on invoke, calls the
    // callable via try_call. The callable Zval is shallow_clone'd (bumping the
    // closure's refcount) and pinned as an owned ZendCallable.
    assert!(
        canon(&out).contains(&canon("let listener: Box<dyn Fn(i32) + Send + Sync> =")),
        "php wraps the callable into the core boxed Fn:\n{out}"
    );
    assert!(
        canon(&out).contains(&canon(
            "let __cb = PhpCb(ext_php_rs::types::ZendCallable::new_owned(listener.shallow_clone()).map_err(err)?);"
        )) && canon(&out).contains(&canon("let _ = __cb.callable().try_call(vec![&v]);")),
        "php pins the callable as an owned ZendCallable and invokes it via try_call:\n{out}"
    );

    // the opaque #[php_class] Subscription handle + its idempotent unsubscribe.
    assert!(
        out.contains("#[php_class]")
            && out.contains("pub struct Subscription {")
            && canon(&out).contains(&canon(
                "unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>"
            ))
            && out.contains("pub fn unsubscribe(&self) {"),
        "php emits the #[php_class] Subscription handle with unsubscribe:\n{out}"
    );

    // the subscription op REGISTERS the listener + returns an owning Subscription.
    assert!(
        out.contains(
            "pub fn on_tick(&self, listener: &ext_php_rs::types::Zval) -> PhpResult<Subscription>"
        ) && out.contains("let unsub = self.core.on_tick(listener);")
            && canon(&out).contains(&canon(
                "Ok(Subscription { unsub: std::sync::Mutex::new(Some(unsub)) })"
            )),
        "php on_tick registers the listener + returns a Subscription handle:\n{out}"
    );

    // the uniform core-trait method (register-in, unsubscribe-out).
    assert!(
        canon(&out).contains(&canon(CORE_TRAIT_METHOD)),
        "php core trait sees the register→unsubscribe method:\n{out}"
    );
}

#[test]
fn callback_free_surface_stays_byte_identical() {
    let api = load_api(PLAIN_API).unwrap();
    let out = php_binding(&api, &[], None);
    // The PhpCb wrapper + Subscription handle are strictly gated: a schema with
    // no subscription op / callback param emits ZERO of them.
    assert!(
        !out.contains("struct PhpCb(")
            && !out.contains("pub struct Subscription {")
            && !out.contains("ZendCallable")
            && !out.contains("SYNC-ONLY"),
        "a callback/subscription-free surface emits no PhpCb/Subscription glue:\n{out}"
    );
}
