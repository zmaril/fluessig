//! The callback + subscription slice of the ext-php-rs (PHP) backend, factored
//! out of [`super::php`] so that module stays under the file-size gate.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.
//!
//! Two pieces live here, both reachable only from a schema that carries a callback
//! param or a `Shape::Subscription` op (so a callback-free surface's output is
//! byte-identical):
//!   * the `PhpCb` newtype prelude ([`PHP_CALLBACK_PRELUDE`]) + the callback
//!     IN-param renderer ([`rust_callback_conv`]) — an incoming PHP `callable`
//!     (a `Closure`/callable `Zval`) is `shallow_clone`d (bumping its refcount so
//!     it outlives the call), pinned as an owned `ZendCallable<'static>`, wrapped
//!     into the ONE uniform core shape `Box<dyn Fn(..) + Send + Sync>`; on invoke
//!     it calls the callable via ext-php-rs `try_call`;
//!   * the opaque `#[php_class]` `Subscription` handle ([`PHP_SUBSCRIPTION_PRELUDE`]),
//!     owning the core's returned unsubscribe closure, with an `unsubscribe`
//!     method (take-and-call, idempotent).
//!
//! ## THE SYNC-ONLY CONTRACT (coordinator ruling — see notes/callback-function-types.md)
//!
//! PHP's single-thread request model CANNOT invoke a callback from a background
//! thread. This backend therefore lowers callbacks as **documented sync-only,
//! NOT a hard generation error**: pidgin's goal is that every language can drop
//! in callbacks, so PHP stays usable at synchronous invocation points. The
//! generated `PhpCb` newtype asserts `Send`/`Sync` over a `!Send` PHP callable —
//! this is sound ONLY when the boxed closure is invoked synchronously on the
//! same PHP request thread that supplied the callable (e.g. a `Shape::Subscription`
//! op whose core fires its listeners synchronously from a PHP method call).
//! Off-thread invocation is UNSUPPORTED and undefined behaviour. The restriction
//! is surfaced as a LOUD doc comment on the generated `PhpCb` (the compile-time-
//! visible marker) rather than a runtime surprise.
//!
//! The uniform core-side shape is the same `Box<dyn Fn(..) + Send + Sync>` that
//! node/python/cpp/java/ruby/wasm target; the `<Iface>Core` trait (emitted by the
//! shared [`super::emit_core_traits`]) already spells the register→unsubscribe
//! method.

use crate::api::{ApiDoc, ApiOp, ApiType, CallbackSig};

use super::*;

/// The `PhpCb` newtype prelude (gated on callback usage). A PHP callable arrives
/// as a callable `Zval` (a `Closure`/`callable`); it is `!Send`/`!Sync` — bound
/// to the PHP request thread — so, exactly as the C/C++ backend wraps a raw
/// context pointer in `CbCtx`, ruby wraps a `Proc` in `RubyCb`, and wasm wraps a
/// `js_sys::Function` in `WasmCb`, we wrap the owned `ZendCallable` in `PhpCb` and
/// assert `Send + Sync`.
///
/// SYNC-ONLY (the coordinator ruling): this assertion is sound ONLY under
/// synchronous same-request-thread invocation — the boxed closure it wraps must
/// be invoked on the PHP request thread that supplied the callable (PHP cannot
/// call back into a request from a background thread; off-thread invocation is
/// UNSUPPORTED and undefined behaviour). The LOUD doc comment below is the
/// compile-time-visible marker of that restriction.
pub(super) const PHP_CALLBACK_PRELUDE: &str = r#"
/// A PHP callback (a `callable`/`Closure` `Zval`), marshalled into the uniform
/// core `Box<dyn Fn>`.
///
/// # SYNC-ONLY — off-thread invocation is undefined behaviour
///
/// A PHP callable is `!Send`/`!Sync`: it is bound to the PHP request thread, and
/// PHP's single-thread request model CANNOT invoke it from a background thread.
/// `PhpCb` asserts `Send`/`Sync` so the callable can ride the uniform
/// `Box<dyn Fn(..) + Send + Sync>` core shape, but that assertion is SOUND ONLY
/// when the boxed closure is invoked SYNCHRONOUSLY on the SAME PHP request thread
/// that supplied the callable (e.g. a `Shape::Subscription` op whose core fires
/// its listeners synchronously from a PHP method call). Invoking it from any other
/// thread is UNSUPPORTED and undefined behaviour. This is pidgin's one backend
/// where the forward-only callback contract genuinely fights the runtime; the
/// restriction is surfaced here explicitly (see notes/callback-function-types.md)
/// rather than silently mis-lowered.
///
/// The owned `ZendCallable<'static>` keeps the underlying callable alive for the
/// subscription's whole lifetime (the callable `Zval` was `shallow_clone`d, which
/// bumps its refcount, before being handed to `new_owned`).
struct PhpCb(ext_php_rs::types::ZendCallable<'static>);
// SAFETY: sound ONLY under the sync-only contract above — the wrapped callable is
// only ever invoked on the PHP request thread that supplied it. Off-thread
// invocation is undefined behaviour.
unsafe impl Send for PhpCb {}
unsafe impl Sync for PhpCb {}
impl PhpCb {
    /// The owned callable, read through a method so a `move` closure captures the
    /// WHOLE `PhpCb` (which is `Send + Sync`). Reaching `self.0` directly would,
    /// under RFC 2229 disjoint capture, capture only the non-`Send` `ZendCallable`.
    fn callable(&self) -> &ext_php_rs::types::ZendCallable<'static> {
        &self.0
    }
}
"#;

/// The opaque `#[php_class]` `Subscription` handle prelude (gated on subscription
/// usage). It owns the core's returned unsubscribe closure in a `Mutex<Option<…>>`;
/// `unsubscribe()` runs it (take-and-call, so a second call is a no-op). Mirrors
/// the wasm `#[wasm_bindgen]` / node `#[napi]` `Subscription`.
pub(super) const PHP_SUBSCRIPTION_PRELUDE: &str = r#"
/// An opaque subscription handle owning the core's returned unsubscribe closure.
/// `unsubscribe()` removes the registered listener early (idempotent — a second
/// call is a no-op). Returned by a `Shape::Subscription` op. The wrapped PHP
/// callable stays alive via the core's listener registration until the
/// subscription is removed (dropping the handle releases it on the PHP request
/// thread — see the sync-only contract on `PhpCb`).
#[php_class]
pub struct Subscription {
    unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}
#[php_impl]
impl Subscription {
    /// Run the unsubscribe closure early (idempotent — a second call is a no-op).
    pub fn unsubscribe(&self) {
        let taken = self.unsub.lock().unwrap().take();
        if let Some(f) = taken {
            f();
        }
    }
}
"#;

/// Does this op take an [`ApiType::Callback`] param? A callback-carrying op is
/// forced fallible (its method returns `PhpResult`) because the callback-conv
/// prelude marshals the callable via `?` (a non-callable `Zval` raises). Used by
/// [`super::php`] to select the throwing arm even for an otherwise-infallible op.
pub(super) fn op_uses_callback(op: &ApiOp) -> bool {
    op.params
        .iter()
        .any(|p| matches!(&p.ty, ApiType::Callback { .. }))
}

/// The PHP method's `(name, decl_type)` param list, spelling a `Callback` param as
/// `&ext_php_rs::types::Zval` (the raw callable zval — the conv prelude wraps it)
/// rather than the uniform core box; every other param keeps its shared [`ty`]
/// spelling (via [`param_sig`]). Mirrors ruby's `magnus::block::Proc` param swap.
pub(super) fn php_param_sig(api: &ApiDoc, op: &ApiOp) -> Vec<(String, String)> {
    op.params
        .iter()
        .map(|p| {
            if matches!(&p.ty, ApiType::Callback { .. }) {
                (snake(&p.name), "&ext_php_rs::types::Zval".to_string())
            } else {
                let (r, _) = ty(api, &p.ty);
                let r = if p.optional == Some(true) {
                    format!("Option<{r}>")
                } else {
                    r
                };
                (snake(&p.name), r)
            }
        })
        .collect()
}

/// The callback-conv prelude lines for an op: one `rust_callback_conv` per
/// `Callback` param, each shadowing `{name}` with the uniform core `Box<dyn Fn>`
/// so it flows straight into the core call. Empty when the op has no callback
/// param (so a callback-free op's body is byte-identical).
pub(super) fn callback_conv_lines(api: &ApiDoc, op: &ApiOp) -> Vec<String> {
    op.params
        .iter()
        .filter_map(|p| match &p.ty {
            ApiType::Callback { callback } => {
                Some(rust_callback_conv(api, callback, &snake(&p.name)))
            }
            _ => None,
        })
        .collect()
}

/// Render the conversion that wraps a callback IN param (`{name}: &Zval`) into the
/// uniform core `Box<dyn Fn(Args) + Send + Sync>` (`{name}`, so the shadowed
/// binding flows straight into the core call). The callable `Zval` is
/// `shallow_clone`d (bumping its refcount so it outlives the call), pinned as an
/// owned `ZendCallable<'static>` inside a `PhpCb` (asserted `Send`/`Sync` — sound
/// ONLY under the sync-only contract); on invoke the closure calls it via
/// `try_call` with the marshalled args, discarding any result — a host exception
/// surfaces as the `Err` we drop (the forward-only-infallible contract). A
/// non-callable `Zval` raises through the `err` seam (`?`), so a callback-carrying
/// op is always fallible ([`op_uses_callback`]).
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
    // `try_call` takes a `Vec<&dyn IntoZvalDyn>`; a one-arg callback passes
    // `vec![&v]`, N args `vec![&v0, &v1, …]`. A zero-arg callback needs the
    // explicit element type (an empty `vec![]` cannot infer `&dyn IntoZvalDyn`).
    let call = if vars.is_empty() {
        "__cb.callable().try_call(std::vec::Vec::<&dyn ext_php_rs::convert::IntoZvalDyn>::new())"
            .to_string()
    } else {
        let refs = vars
            .iter()
            .map(|v| format!("&{v}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("__cb.callable().try_call(vec![{refs}])")
    };
    format!(
        "let {name}: {box_ty} = {{\n\
         // The callable `Zval` is `shallow_clone`d (bumping its refcount so it\n\
         // outlives this call) and pinned as an owned `ZendCallable<'static>`.\n\
         let __cb = PhpCb(ext_php_rs::types::ZendCallable::new_owned({name}.shallow_clone()).map_err(err)?);\n\
         Box::new(move |{closure_params}| {{\n\
         // SYNC-ONLY: invoked synchronously on the PHP request thread (the core\n\
         // fires listeners from a PHP method call). Off-thread invocation is UB —\n\
         // see the `PhpCb` doc marker / notes/callback-function-types.md.\n\
         let _ = {call};\n\
         }})\n\
         }};"
    )
}
