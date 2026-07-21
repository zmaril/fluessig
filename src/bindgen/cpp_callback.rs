//! The callback + subscription slice of the C/C++ (extern "C" ABI) backend,
//! factored out of [`super::cpp`] so that module stays under the file-size gate.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.
//!
//! Two pieces live here, both reachable only from a schema that carries a callback
//! param (so a callback-free surface's output is byte-identical):
//!   * the Rust-side preludes ([`RUST_CALLBACK_PRELUDE`] / [`RUST_SUBSCRIPTION_PRELUDE`]),
//!   * the callback IN-param renderer ([`rust_callback_in`]) and the subscription
//!     op emitter ([`emit_subscription`]) the `extern "C"` layer drives.
//!
//! The uniform core-side shape is the same `Box<dyn Fn(..) + Send + Sync>` node and
//! python target: a `Callback{params}` param crosses the C ABI as a fn-ptr +
//! `void* ctx` pair, and this module wraps that pair into the boxed closure.

use crate::api::{ApiDoc, ApiOp, CallbackSig};

use super::cpp::{op_symbol, rust_in_params};
use super::*;

/// The callback-context newtype (gated on callback usage). A raw context pointer
/// is not `Send`/`Sync`, but the C caller owns thread-safety across the ABI (for
/// the demo the callback fires synchronously on the same thread), so we assert it
/// here — letting the boxed closure the binding builds be `Send + Sync` for the
/// core (mirroring the uniform `Box<dyn Fn(..) + Send + Sync>` node/python target).
pub(super) const RUST_CALLBACK_PRELUDE: &str = r#"use std::os::raw::c_void;

/// A C callback context pointer, made `Send`/`Sync` because the C caller owns
/// thread-safety at the ABI boundary (the boxed closure below is what the core sees).
struct CbCtx(*mut c_void);
unsafe impl Send for CbCtx {}
unsafe impl Sync for CbCtx {}
impl CbCtx {
    /// The raw context pointer, read through a method so a `move` closure captures
    /// the WHOLE `CbCtx` (which is `Send + Sync`). Reaching `ctx.0` directly would,
    /// under RFC 2229 disjoint capture, capture only the non-`Send` `*mut c_void`.
    fn ptr(&self) -> *mut c_void {
        self.0
    }
}
"#;

/// The opaque `Subscription` handle + its lifecycle fns (gated on subscription
/// usage). Mirrors the node/python `Subscription` class: it owns the core's
/// returned unsubscribe closure; `Subscription_unsubscribe` runs it early and
/// `Subscription_free` drops the handle (also unsubscribing if still live). Both
/// take-and-call, so they are idempotent.
pub(super) const RUST_SUBSCRIPTION_PRELUDE: &str = r#"/// An opaque subscription handle owning the core's returned unsubscribe closure.
pub struct Subscription {
    unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// Run the unsubscribe closure early (idempotent — a second call is a no-op).
#[no_mangle]
pub unsafe extern "C" fn Subscription_unsubscribe(s: *mut Subscription) {
    if s.is_null() {
        return;
    }
    let s = &*s;
    let taken = s.unsub.lock().unwrap().take();
    if let Some(f) = taken {
        f();
    }
}

/// Free the subscription handle, unsubscribing if still live.
#[no_mangle]
pub unsafe extern "C" fn Subscription_free(s: *mut Subscription) {
    if s.is_null() {
        return;
    }
    let s = Box::from_raw(s);
    let taken = s.unsub.lock().unwrap().take();
    if let Some(f) = taken {
        f();
    }
}
"#;

/// Render a callback IN param: the two Rust extern decls (`{name}: extern "C"
/// fn(*mut c_void, Args)` + `{name}_ctx: *mut c_void`) and the conversion that
/// wraps the fn-ptr + context into the uniform core `Box<dyn Fn(Args) + Send +
/// Sync>` (`{name}_cb`). The C caller owns thread-safety, so the raw context rides
/// the `CbCtx` newtype to become `Send + Sync`.
pub(super) fn rust_callback_in(
    api: &ApiDoc,
    sig: &CallbackSig,
    name: &str,
    decls: &mut Vec<String>,
    conv: &mut Vec<String>,
) {
    let arg_tys: Vec<String> = sig.params.iter().map(|p| ty(api, p).0).collect();
    let vars = callback_arg_vars(arg_tys.len());
    let fn_args = std::iter::once("*mut c_void".to_string())
        .chain(arg_tys.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    decls.push(format!("{name}: extern \"C\" fn({fn_args})"));
    decls.push(format!("{name}_ctx: *mut c_void"));
    let box_ty = format!("Box<dyn Fn({}) + Send + Sync>", arg_tys.join(", "));
    let closure_params = vars
        .iter()
        .zip(&arg_tys)
        .map(|(v, t)| format!("{v}: {t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let call_args = std::iter::once("ctx.ptr()".to_string())
        .chain(vars.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    conv.push(format!(
        "let {name}_cb: {box_ty} = {{ let ctx = CbCtx({name}_ctx); Box::new(move |{closure_params}| {{ ({name})({call_args}); }}) }};"
    ));
}

/// A subscription op: `<Iface>_<op>(self_, <cb-in>, out: **Subscription[, err_out])`.
/// Registers the listener (its callback param, wrapped into the uniform `Box<dyn
/// Fn>`) and hands back an opaque `Subscription*` owning the core's returned
/// unsubscribe closure. Infallible ⇒ `void` + `out`; fallible ⇒ `c_int` status +
/// `err_out` (the same error channel the ctor/unary arms use). Always `&self` (a
/// subscription op requires a stateful interface — enforced by the loader).
pub(super) fn emit_subscription(api: &ApiDoc, iface: &str, op: &ApiOp, impl_path: &str) -> String {
    let sym = op_symbol(iface, op);
    let (decls, conv, names) = rust_in_params(api, op);
    let mut in_sig = decls.join(", ");
    if !in_sig.is_empty() {
        in_sig.push_str(", ");
    }
    let call = format!("this.{}", snake(&op.name));
    let args = names.join(", ");
    let handle = "Subscription { unsub: std::sync::Mutex::new(Some(unsub)) }";
    let mut s = String::new();
    if let Some(doc) = &op.doc {
        for line in doc.lines() {
            s.push_str(&format!("/// {line}\n"));
        }
    }
    s.push_str(&format!(
        "/// Register a listener on `{iface}.{}`; returns an owning Subscription handle.\n",
        op.name
    ));
    if op.infallible {
        s.push_str(&format!(
            "#[no_mangle]\npub unsafe extern \"C\" fn {sym}(self_: *mut {impl_path}, {in_sig}out: *mut *mut Subscription) {{\n"
        ));
        s.push_str("    let this = &*self_;\n");
        for c in &conv {
            s.push_str(&format!("    {c}\n"));
        }
        s.push_str(&format!("    let unsub = {call}({args});\n"));
        s.push_str(&format!(
            "    *out = Box::into_raw(Box::new({handle}));\n}}\n"
        ));
    } else {
        s.push_str(&format!(
            "#[no_mangle]\npub unsafe extern \"C\" fn {sym}(self_: *mut {impl_path}, {in_sig}out: *mut *mut Subscription, err_out: *mut *mut c_char) -> c_int {{\n"
        ));
        s.push_str("    let this = &*self_;\n");
        for c in &conv {
            s.push_str(&format!("    {c}\n"));
        }
        s.push_str(&format!("    match {call}({args}) {{\n"));
        s.push_str(&format!(
            "        Ok(unsub) => {{\n            *out = Box::into_raw(Box::new({handle}));\n            if !err_out.is_null() {{ *err_out = std::ptr::null_mut(); }}\n            0\n        }}\n"
        ));
        s.push_str("        Err(e) => {\n            if !err_out.is_null() { *err_out = fl_str_out(e.to_string()); }\n            1\n        }\n");
        s.push_str("    }\n}\n");
    }
    s
}
