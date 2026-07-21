//! The callback + subscription slice of the JNI (Java) backend, factored out of
//! [`super::java`] so that module stays under the file-size gate.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.
//!
//! Two pieces live here, both reachable only from a schema that carries a callback
//! param or a `Shape::Subscription` op (so a callback-free surface's output is
//! byte-identical):
//!   * the callback IN-param wrapper ([`rust_callback_conv`]) — a Java
//!     `Consumer` `JObject` becomes a GLOBAL ref + a captured [`jni::JavaVM`],
//!     wrapped into the uniform core `Box<dyn Fn(..) + Send + Sync>`; on invoke it
//!     `attach_current_thread()`s, boxes the arg, and `call_method`s `accept`;
//!   * the opaque `Subscription` handle: its Rust prelude
//!     ([`subscription_rust_prelude`], the `nativeUnsubscribe`/`nativeFree` JNI
//!     fns), the subscription op JNI emitter ([`emit_subscription_jni`]), and the
//!     Java `Subscription` class ([`subscription_java_class`]).
//!
//! The uniform core-side shape is the same `Box<dyn Fn(..) + Send + Sync>` that
//! node/python/cpp target; the `<Iface>Core` trait (emitted by the shared
//! [`super::emit_core_traits`]) already spells the register→unsubscribe method.

use crate::api::{ApiDoc, ApiOp, CallbackSig};

use super::java::{java_boxed, jni_symbol, marshal_param, to_jobject, LIB, PACKAGE};
use super::*;

/// The Java-visible type a callback param is spelled with: the host-supplied
/// functional interface. A one-arg forward-only callback (the only shape pi has,
/// and what this slice targets) is a `java.util.function.Consumer<Boxed>`; a
/// zero-arg callback is a `Runnable`. Fully-qualified so no import bookkeeping is
/// needed in the interface class.
pub(super) fn callback_java_type(api: &ApiDoc, sig: &CallbackSig) -> String {
    match sig.params.len() {
        0 => "Runnable".to_string(),
        _ => format!(
            "java.util.function.Consumer<{}>",
            java_boxed(api, &sig.params[0])
        ),
    }
}

/// Render the Rust conversion that wraps a callback IN param (`{name}_j:
/// JObject`) into the uniform core `Box<dyn Fn(Args) + Send + Sync>` (`{name}`).
///
/// The incoming `Consumer` is pinned as a GLOBAL ref (so it outlives this call
/// and can be invoked from any thread the core runs on) and the process
/// [`jni::JavaVM`] is captured; both are `Send + Sync`, so the boxed closure is
/// too. On invoke it `attach_current_thread()`s (a no-op / reused env on an
/// already-attached JVM thread, a fresh attach+detach off-thread), boxes each arg
/// into its wrapper object, and `call_method`s `accept` (one-arg `Consumer`) /
/// `run` (zero-arg `Runnable`). A host exception is left pending on the env and
/// not propagated into the core (the forward-only-infallible contract).
pub(super) fn rust_callback_conv(api: &ApiDoc, sig: &CallbackSig, name: &str) -> String {
    let arg_tys = &sig.params;
    let vars = callback_arg_vars(arg_tys.len());
    let box_ty = format!(
        "Box<dyn Fn({}) + Send + Sync>",
        arg_tys
            .iter()
            .map(|t| ty(api, t).0)
            .collect::<Vec<_>>()
            .join(", ")
    );
    let closure_params = vars
        .iter()
        .zip(arg_tys)
        .map(|(v, t)| format!("{v}: {}", ty(api, t).0))
        .collect::<Vec<_>>()
        .join(", ");
    let mut box_lines = String::new();
    let mut jvalue_args: Vec<String> = Vec::new();
    for (i, (v, t)) in vars.iter().zip(arg_tys).enumerate() {
        box_lines.push_str(&format!("let __a{i} = {};\n", to_jobject(t, v)));
        jvalue_args.push(format!("JValue::Object(&__a{i})"));
    }
    // A one-arg callback is a `Consumer.accept(Object)`; a zero-arg one a
    // `Runnable.run()`. (pi's callback surface is uniformly one typed arg.)
    let (jmethod, jsig) = if arg_tys.is_empty() {
        ("run", "()V")
    } else {
        ("accept", "(Ljava/lang/Object;)V")
    };
    let jargs = jvalue_args.join(", ");
    format!(
        "let {name}: {box_ty} = {{\n\
         let __global = env.new_global_ref(&{name}_j).expect(\"callback `{name}`: new_global_ref\");\n\
         let __vm = env.get_java_vm().expect(\"callback `{name}`: get_java_vm\");\n\
         Box::new(move |{closure_params}| {{\n\
         let mut __guard = match __vm.attach_current_thread() {{ Ok(g) => g, Err(_) => return }};\n\
         let env = &mut *__guard;\n\
         {box_lines}\
         let _ = env.call_method(__global.as_obj(), \"{jmethod}\", \"{jsig}\", &[{jargs}]);\n\
         }})\n\
         }};"
    )
}

/// Emit a subscription op's JNI glue: `native<Op>(handle, <cb-in>) -> jlong`.
/// It wraps the callback param into the uniform `Box<dyn Fn>`, REGISTERS it via
/// the core (`core.<op>(listener)`), and returns an opaque `long` Subscription
/// handle owning the core's returned unsubscribe closure. Infallible ⇒ the handle
/// straight through; fallible ⇒ throw on `Err` and return the `0` handle (the
/// same throw seam the unary arms use). A subscription op is always stateful
/// (`&self`), enforced by the loader, so `has_ctor` is expected true.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_subscription_jni(
    api: &ApiDoc,
    iface: &str,
    op: &ApiOp,
    impl_path: &str,
    trait_name: &str,
    has_ctor: bool,
    out: &mut String,
) {
    let native = format!("native{}", pascal(&op.name));
    let sym = jni_symbol(iface, &native);
    let mut params = String::from("mut env: JNIEnv<'local>, _class: JClass<'local>");
    if has_ctor {
        params.push_str(", handle: jlong");
    }
    let mut convs = String::new();
    let mut names: Vec<String> = Vec::new();
    for p in &op.params {
        let (decl, conv) = marshal_param(api, p);
        params.push_str(&format!(", {decl}"));
        convs.push_str(&format!("    {conv}\n"));
        names.push(snake(&p.name));
    }
    let call = if has_ctor {
        format!(
            "{{ let core = unsafe {{ &*(handle as *const Arc<{impl_path}>) }}; core.{}({}) }}",
            snake(&op.name),
            names.join(", ")
        )
    } else {
        format!(
            "<{impl_path} as {trait_name}>::{}({})",
            snake(&op.name),
            names.join(", ")
        )
    };
    let handle_expr =
        "Box::into_raw(Box::new(Subscription { unsub: std::sync::Mutex::new(Some(__unsub)) })) as jlong";
    let body = if op.infallible {
        format!("    let __unsub = {call};\n    {handle_expr}\n")
    } else {
        format!(
            "    match {call} {{\n        Ok(__unsub) => {handle_expr},\n        Err(__e) => {{ throw(env, __e); 0 }}\n    }}\n"
        )
    };
    out.push_str(&format!(
        "/// Register a listener on `{iface}.{op}`; returns an opaque `long` handle\n\
         /// (a leaked `Box<Subscription>`) the `Subscription` class owns.\n\
         #[no_mangle]\npub extern \"system\" fn {sym}<'local>({params}) -> jlong {{\n    let env = &mut env;\n{convs}{body}}}\n\n",
        op = op.name,
    ));
}

/// The opaque `Subscription` handle + its `nativeUnsubscribe`/`nativeFree` JNI
/// fns (gated on subscription usage). Mirrors the C/C++ backend's `Subscription`:
/// it owns the core's returned unsubscribe closure; `nativeUnsubscribe` runs it
/// early and `nativeFree` drops the handle (also unsubscribing if still live).
/// Both take-and-call, so they are idempotent.
pub(super) fn subscription_rust_prelude() -> String {
    let unsub_sym = jni_symbol("Subscription", "nativeUnsubscribe");
    let free_sym = jni_symbol("Subscription", "nativeFree");
    format!(
        "/// An opaque subscription handle owning the core's returned unsubscribe closure.\n\
         struct Subscription {{\n    \
         unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,\n}}\n\n\
         /// Run the unsubscribe closure early (idempotent — a second call is a no-op).\n\
         #[no_mangle]\npub extern \"system\" fn {unsub_sym}<'local>(_env: JNIEnv<'local>, _class: JClass<'local>, handle: jlong) {{\n    \
         if handle == 0 {{ return; }}\n    \
         let s = unsafe {{ &*(handle as *const Subscription) }};\n    \
         let __taken = s.unsub.lock().unwrap().take();\n    \
         if let Some(__f) = __taken {{ __f(); }}\n}}\n\n\
         /// Free the subscription handle, unsubscribing if still live (idempotent).\n\
         #[no_mangle]\npub extern \"system\" fn {free_sym}<'local>(_env: JNIEnv<'local>, _class: JClass<'local>, handle: jlong) {{\n    \
         if handle == 0 {{ return; }}\n    \
         let s = unsafe {{ Box::from_raw(handle as *mut Subscription) }};\n    \
         let __taken = s.unsub.lock().unwrap().take();\n    \
         if let Some(__f) = __taken {{ __f(); }}\n}}\n\n"
    )
}

/// The Java `Subscription` class: an opaque handle whose `unsubscribe()` removes
/// the listener early (idempotent) and whose `close()` frees the native handle
/// (also unsubscribing if still live). Mirrors the poll-cursor / handle classes.
pub(super) fn subscription_java_class() -> String {
    format!(
        "package {PACKAGE};\n\n\
         /** An opaque subscription handle. `unsubscribe()` removes the listener early\n \
         * (idempotent); `close()` frees the native handle (also unsubscribing if still\n \
         * live). Returned by a `Shape::Subscription` op such as `Ticker.onTick`. */\n\
         public final class Subscription {{\n    \
         static {{ System.loadLibrary(\"{LIB}\"); }}\n\n    \
         private long handle;\n\n    \
         Subscription(long handle) {{ this.handle = handle; }}\n\n    \
         private static native void nativeUnsubscribe(long handle);\n    \
         private static native void nativeFree(long handle);\n\n    \
         /** Remove the listener early (idempotent); the handle is still freed by close(). */\n    \
         public void unsubscribe() {{ if (this.handle != 0) {{ nativeUnsubscribe(this.handle); }} }}\n\n    \
         /** Free the native handle (also unsubscribing if still live); idempotent. */\n    \
         public void close() {{ if (this.handle != 0) {{ nativeFree(this.handle); this.handle = 0; }} }}\n}}\n"
    )
}
