//! The Java (JNI) backend's callback + subscription lowering (follow-up to the
//! merged callback IR #78, `Shape::Subscription` #85, and the cpp backend #87). A
//! forward-only sync-void `ApiType::Callback` param crosses the JNI seam as a
//! Java `Consumer` `JObject`: the generated glue pins it as a GLOBAL ref, captures
//! the process `JavaVM`, and wraps the pair into the ONE uniform core shape
//! `Box<dyn Fn(..) + Send + Sync>` — on invoke it `attach_current_thread()`s and
//! `call_method`s `accept`. A `Shape::Subscription` op returns an opaque `long`
//! Subscription handle owning the core's unsubscribe closure, with
//! `nativeUnsubscribe` (take-and-call, idempotent) + `nativeFree`.
//!
//! This suite pins the load-bearing facts across both Java artifacts (the Rust
//! JNI glue + the `.java` classes), plus the uniform core-trait method. The real
//! compile+run proof lives in `crates/java-demo` (Main.java fires a Java
//! `Consumer<Integer>` from Rust, sees `[0, 1]`, then silence after unsubscribe).
//!
//! straitjacket-allow-file:duplication — the assertion blocks are DELIBERATELY
//! parallel to the sibling cross-backend test files
//! (`tests/cpp_callback_lowering.rs`, `tests/callback_lowering.rs`).

use fluessig::api::load_api;
use fluessig::bindgen::{java_binding, java_sources};

/// A stateful `Ticker` (it carries a `Ctor`, so a `&self` Subscription method is
/// legal): a ctor `new`, an INFALLIBLE `Shape::Subscription` op `on_tick` taking
/// one `listener: Callback<(int32)>` and returning a `Subscription` handle, and a
/// plain infallible `Unary` op `tick`. Matches the `crates/java-demo` fixture.
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
/// output byte-identical (no `Subscription` handle, no callback preludes).
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
/// node/python/cpp emit for the infallible register→unsubscribe method).
const CORE_TRAIT_METHOD: &str =
    "fn on_tick(&self, listener: Box<dyn Fn(i32) + Send + Sync>) -> Box<dyn Fn() + Send + Sync>";

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
fn java_glue_lowers_callback_and_subscription() {
    let api = load_api(API).unwrap();
    let out = java_binding(&api, &[], None);

    // the callback param crosses in as a JObject (the Java Consumer).
    assert!(
        out.contains("listener_j: JObject<'local>"),
        "java callback param is a JObject:\n{out}"
    );
    // it is pinned as a GLOBAL ref + the JavaVM is captured, wrapped into the
    // uniform core boxed Fn.
    assert!(
        canon(&out).contains(&canon("let listener: Box<dyn Fn(i32) + Send + Sync> =")),
        "java wraps the Consumer into the core boxed Fn:\n{out}"
    );
    assert!(
        canon(&out).contains(&canon("env.new_global_ref(&listener_j)"))
            && canon(&out).contains(&canon("env.get_java_vm()")),
        "java pins a global ref + captures the JavaVM:\n{out}"
    );
    // on invoke it attaches the current thread and calls `accept` with the boxed arg.
    assert!(
        out.contains("__vm.attach_current_thread()")
            && canon(&out).contains(&canon(
                "env.call_method(__global.as_obj(), \"accept\", \"(Ljava/lang/Object;)V\","
            )),
        "java attaches the thread + calls Consumer.accept:\n{out}"
    );
    // the i32 arg is boxed into a java.lang.Integer.
    assert!(
        canon(&out).contains(&canon(
            "env.new_object(\"java/lang/Integer\", \"(I)V\", &[JValue::Int(v)])"
        )),
        "java boxes the i32 into an Integer:\n{out}"
    );

    // the opaque Subscription handle + its lifecycle JNI fns, emitted once.
    assert!(
        out.contains("struct Subscription {")
            && canon(&out).contains(&canon(
                "unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>"
            ))
            && out.contains(
                "pub extern \"system\" fn Java_fluessig_Subscription_nativeUnsubscribe<'local>"
            )
            && out
                .contains("pub extern \"system\" fn Java_fluessig_Subscription_nativeFree<'local>"),
        "java emits the Subscription handle + nativeUnsubscribe/nativeFree:\n{out}"
    );

    // the subscription op REGISTERS the listener + returns the opaque handle.
    assert!(
        out.contains("pub extern \"system\" fn Java_fluessig_Ticker_nativeOnTick<'local>")
            && out.contains("-> jlong")
            && out.contains("core.on_tick(listener)")
            && canon(&out).contains(&canon("Box::into_raw(Box::new(Subscription {")),
        "java nativeOnTick registers the listener + returns a Subscription handle:\n{out}"
    );

    // the uniform core-trait method (register-in, unsubscribe-out).
    assert!(
        canon(&out).contains(&canon(CORE_TRAIT_METHOD)),
        "java core trait sees the register→unsubscribe method:\n{out}"
    );
}

#[test]
fn java_sources_wrap_subscription_in_a_handle_class() {
    let api = load_api(API).unwrap();
    let files = java_sources(&api, &[]);
    let src = |name: &str| {
        files
            .iter()
            .find(|(p, _)| p == &format!("fluessig/{name}.java"))
            .map(|(_, s)| s.as_str())
            .unwrap_or_else(|| panic!("missing generated {name}.java"))
    };

    // the Subscription class: an opaque handle with unsubscribe()/close().
    let subscription = src("Subscription");
    assert!(
        subscription.contains("public final class Subscription {")
            && subscription.contains("public void unsubscribe()")
            && subscription.contains("private static native void nativeUnsubscribe(long handle);")
            && subscription.contains("private static native void nativeFree(long handle);"),
        "Subscription.java is an opaque handle class:\n{subscription}"
    );

    // the Ticker class: onTick takes a Consumer<Integer> + returns a Subscription.
    let ticker = src("Ticker");
    assert!(
        ticker.contains(
            "public Subscription onTick(java.util.function.Consumer<Integer> listener)"
        )
        && ticker.contains(
            "private static native long nativeOnTick(long handle, java.util.function.Consumer<Integer> listener);"
        ),
        "Ticker.onTick takes a Consumer + returns a Subscription:\n{ticker}"
    );
}

#[test]
fn callback_free_surface_stays_byte_identical() {
    let api = load_api(PLAIN_API).unwrap();
    let out = java_binding(&api, &[], None);
    // The Subscription handle + callback preludes are strictly gated: a schema
    // with no subscription op / callback param emits ZERO of them.
    assert!(
        !out.contains("struct Subscription {")
            && !out.contains("Java_fluessig_Subscription_nativeUnsubscribe")
            && !out.contains("attach_current_thread"),
        "a callback/subscription-free surface emits no Subscription/callback glue:\n{out}"
    );
    // and no Subscription.java is generated.
    let files = java_sources(&api, &[]);
    assert!(
        !files.iter().any(|(p, _)| p == "fluessig/Subscription.java"),
        "no Subscription.java for a subscription-free surface"
    );
}
