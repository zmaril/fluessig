//! The callback + subscription slice of the Magnus (Ruby) backend, factored out
//! of [`super::ruby`] so that module stays under the file-size gate.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.
//!
//! Two pieces live here, both reachable only from a schema that carries a callback
//! param or a `Shape::Subscription` op (so a callback-free surface's output is
//! byte-identical):
//!   * the `RubyCb` newtype prelude ([`RUBY_CALLBACK_PRELUDE`]) + the callback
//!     IN-param renderer ([`rust_callback_conv`]) — an incoming Ruby `Proc` is
//!     boxed for GC (`BoxValue`), wrapped into the ONE uniform core shape
//!     `Box<dyn Fn(..) + Send + Sync>`; on invoke it re-acquires the `Ruby` handle
//!     (proving the GVL) and calls the `Proc`;
//!   * the opaque `Subscription` wrapped class ([`subscription_ruby_prelude`]),
//!     owning the core's returned unsubscribe closure, with an `unsubscribe`
//!     method (take-and-call, idempotent) and a `Drop` that unsubscribes if live.
//!
//! The uniform core-side shape is the same `Box<dyn Fn(..) + Send + Sync>` that
//! node/python/cpp/java target; the `<Iface>Core` trait (emitted by the shared
//! [`super::emit_core_traits`]) already spells the register→unsubscribe method.

use crate::api::{ApiDoc, CallbackSig};

use super::*;

/// The `RubyCb` newtype prelude (gated on callback usage). A `magnus::Value` (a
/// `Proc`) is `!Send`/`!Sync` — it is bound to the Ruby VM thread under the GVL —
/// so, exactly as the C/C++ backend wraps a raw context pointer in `CbCtx`, we
/// wrap the `Proc` in `RubyCb` and assert `Send + Sync`. This is SOUND because the
/// boxed closure below is ONLY ever invoked on the Ruby thread under the GVL (for
/// a `Shape::Subscription` op the core fires its listeners synchronously from a
/// Ruby method call), and the closure re-acquires the `Ruby` handle (via
/// `Ruby::get()`, which succeeds only under the GVL) before touching the `Proc`.
/// The inner `BoxValue` keeps the `Proc` alive across Ruby GC for the
/// subscription's whole lifetime (a bare `Opaque`/`Value` would be collectable).
pub(super) const RUBY_CALLBACK_PRELUDE: &str = r#"/// A Ruby callback (`Proc`), marshalled into the uniform core `Box<dyn Fn>`. A
/// `Value` is `!Send`/`!Sync` (bound to the Ruby VM thread under the GVL); it is
/// asserted `Send`/`Sync` here because the boxed closure this wraps is only ever
/// invoked on the Ruby thread under the GVL, and the `BoxValue` pins the `Proc`
/// against GC for the subscription's lifetime.
struct RubyCb(magnus::value::BoxValue<magnus::block::Proc>);
unsafe impl Send for RubyCb {}
unsafe impl Sync for RubyCb {}
impl RubyCb {
    /// The pinned `Proc`, read through a method so a `move` closure captures the
    /// WHOLE `RubyCb` (which is `Send + Sync`). Reaching `self.0` directly would,
    /// under RFC 2229 disjoint capture, capture only the non-`Send` `BoxValue`.
    fn proc(&self) -> magnus::block::Proc {
        *self.0
    }
}
"#;

/// Render the conversion that wraps a callback IN param (`{name}: magnus::block::
/// Proc`) into the uniform core `Box<dyn Fn(Args) + Send + Sync>` (`{name}`, so
/// the shadowed binding flows straight into the core call). The `Proc` is pinned
/// as a `RubyCb` (GC-boxed + `Send`/`Sync`); on invoke it re-acquires the `Ruby`
/// handle (proving the GVL) and calls the `Proc`, discarding any result — a host
/// exception surfaces as the `Err` we drop (the forward-only-infallible contract).
pub(super) fn rust_callback_conv(api: &ApiDoc, sig: &CallbackSig, name: &str) -> String {
    let arg_tys: Vec<String> = sig.params.iter().map(|p| ty(api, p).0).collect();
    let vars = callback_arg_vars(arg_tys.len());
    let box_ty = format!("Box<dyn Fn({}) + Send + Sync>", arg_tys.join(", "));
    let closure_params = vars
        .iter()
        .zip(&arg_tys)
        .map(|(v, t)| format!("{v}: {t}"))
        .collect::<Vec<_>>()
        .join(", ");
    // A one-arg callback calls `proc.call((v,))`; a zero-arg one `proc.call(())`.
    let call_args = if vars.is_empty() {
        String::new()
    } else {
        format!("{},", vars.join(", "))
    };
    format!(
        "let {name}: {box_ty} = {{\n\
         let __cb = RubyCb(magnus::value::BoxValue::new({name}));\n\
         Box::new(move |{closure_params}| {{\n\
         // Invoked on the Ruby thread under the GVL (the core fires listeners\n\
         // synchronously from a Ruby method call); re-acquiring the `Ruby` handle\n\
         // proves the GVL, so calling the `Proc` here is sound.\n\
         let __p = __cb.proc();\n\
         let _: Result<magnus::Value, magnus::Error> = __p.call(({call_args}));\n\
         }})\n\
         }};"
    )
}

/// The opaque `Subscription` wrapped class + its lifecycle (gated on subscription
/// usage). Mirrors the cpp/java `Subscription`: it owns the core's returned
/// unsubscribe closure; `unsubscribe` runs it early and `Drop` runs it on GC
/// (also unsubscribing if still live). Both take-and-call, so they are idempotent.
/// `module` is the root class the wrapped class nests under (e.g. `Ticker`).
pub(super) fn subscription_ruby_prelude(module: &str) -> String {
    format!(
        "/// An opaque subscription handle owning the core's returned unsubscribe closure.\n\
         /// `unsubscribe` removes the listener early (idempotent); `Drop` removes it on GC\n\
         /// (also unsubscribing if still live). Returned by a `Shape::Subscription` op.\n\
         #[magnus::wrap(class = {class:?}, free_immediately, size)]\n\
         struct Subscription {{\n    \
         unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,\n}}\n\
         impl Subscription {{\n    \
         /// Run the unsubscribe closure early (idempotent — a second call is a no-op).\n    \
         fn unsubscribe(&self) {{\n        \
         let taken = self.unsub.lock().unwrap().take();\n        \
         if let Some(f) = taken {{\n            f();\n        }}\n    }}\n}}\n\
         /// Backstop: dropping the handle (Ruby GC) unsubscribes if still live.\n\
         impl Drop for Subscription {{\n    \
         fn drop(&mut self) {{\n        \
         let taken = self.unsub.lock().unwrap().take();\n        \
         if let Some(f) = taken {{\n            f();\n        }}\n    }}\n}}\n",
        class = format!("{module}::Subscription"),
    )
}
