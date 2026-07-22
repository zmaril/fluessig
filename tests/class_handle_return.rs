//! Feature 2 — CLASS-HANDLE-RETURN IR lowering: an op whose `returns` names a
//! DECLARED interface is a FACTORY that MINTS a live handle. fluessig reuses
//! `ApiType::Model` for the reference (no new IR variant) and distinguishes "a
//! `Model` naming a declared interface" from "a `Model` naming a DTO" by
//! interface-set membership (Feature 1's `constructible_interfaces`).
//!
//! The fixture mirrors pi's factory-born `RpcProcessInstance`: a free-function
//! factory (`createRpcProcessInstance`) hands back a ctor-less interface whose
//! methods span a subscription (`onEvent`), an ASYNC unary returning a typed value
//! (`send`), an ASYNC unary void (`dispose`), and a SYNC void (`handleUiResponse`).
//! node + python FULLY lower it (a ctor-less handle class, the mint wrap, the real
//! subscription, the async methods); cpp/java/ruby/php/wasm emit honest skip-notes
//! (the follow-up backends, mirroring the Subscription rollout).
//!
//! straitjacket-allow-file:duplication — the per-backend assertion blocks are
//! DELIBERATELY parallel (one per backend), mirroring the sibling cross-backend
//! test files (`tests/subscription_lowering.rs`, `tests/callback_lowering.rs`).

use fluessig::api::load_api;
use fluessig::bindgen::{
    cpp_binding, java_binding, node_binding, php_binding, python_binding, ruby_binding,
    wasm_binding,
};

/// A factory-born (ctor-less) `RpcProcessInstance` handed back by the free-function
/// factory `RpcFactory.create_rpc_process_instance`. The handle carries the full
/// method spread this interface needs: a subscription, an async unary returning a
/// value, an async unary void, and a sync void. (`Json` stands in for the
/// cross-package `RpcCommand`/`RpcResponse` a no-context build degrades to.)
const API: &str = r#"{
  "fluessig": {"format": 1},
  "models": [], "unions": [],
  "interfaces": [
    {"name": "RpcFactory", "ops": [
      {"name": "create_rpc_process_instance", "shape": "unary", "params": [
        {"name": "instance_id", "type": "string"}
      ], "returns": {"model": "RpcProcessInstance"}}
    ]},
    {"name": "RpcProcessInstance", "ops": [
      {"name": "on_event", "shape": "subscription", "params": [
        {"name": "listener", "type": {"callback": {"params": ["Json"]}}}
      ], "returns": "void"},
      {"name": "send", "shape": "unary", "async": true, "params": [
        {"name": "cmd", "type": "Json"}
      ], "returns": "Json"},
      {"name": "dispose", "shape": "unary", "async": true, "params": [], "returns": "void"},
      {"name": "handle_ui_response", "shape": "unary", "params": [
        {"name": "resp", "type": "Json"}
      ], "returns": "void"}
    ]}
  ]
}"#;

/// Collapse every run of ASCII whitespace to a single space so a rustfmt-wrapped
/// multi-line signature matches its one-line canonical form.
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip ALL whitespace and the trailing comma rustfmt inserts before a closing
/// paren when it wraps a long param list, so a wrapped signature matches its
/// one-line canonical form.
fn canon(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace(",)", ")")
}

/// The fixture LOADS: an op returning `{"model": "RpcProcessInstance"}` (a declared
/// ctor-less interface) is accepted, and that interface's subscription op — legal
/// only on a constructible interface — is accepted because the factory op makes it
/// constructible (Feature 1).
#[test]
fn factory_return_and_subscription_load() {
    assert!(
        load_api(API).is_ok(),
        "a factory op returning a declared ctor-less interface, plus its subscription, loads"
    );
}

/// The CORE-trait return spelling for the factory op is the CORE object
/// `Arc<crate::core_impl::RpcProcessInstanceImpl>` (not the handle class the pure
/// core cannot name), shared by node and python; the binding wraps it into the
/// handle class. The factory-born interface's own methods are all `&self`.
#[test]
fn core_trait_factory_return_is_arc_impl() {
    for out in [
        node_binding(&load_api(API).unwrap(), &[], None),
        python_binding(&load_api(API).unwrap(), &[], None),
    ] {
        let f = flat(&out);
        assert!(
            f.contains(
                "fn create_rpc_process_instance( instance_id: String, ) -> anyhow::Result<Arc<crate::core_impl::RpcProcessInstanceImpl>>"
            ) || f.contains(
                "fn create_rpc_process_instance(instance_id: String) -> anyhow::Result<Arc<crate::core_impl::RpcProcessInstanceImpl>>"
            ),
            "factory core method returns Arc<Impl>, got: {f}"
        );
        assert!(
            f.contains("fn send(&self, cmd: String) -> anyhow::Result<String>")
                && f.contains("fn dispose(&self) -> anyhow::Result<()>"),
            "factory-born interface methods are `&self`"
        );
    }
}

/// node FULLY lowers the factory-born interface: a ctor-less handle class holding
/// `Arc<Impl>` (NO `#[napi(constructor)]`), the mint wrap on the factory free
/// function, the real subscription registration (not #92's skip-note), and the
/// async unary methods (`send`/`dispose`) as `AsyncTask`s.
#[test]
fn node_full_lowering() {
    let out = node_binding(&load_api(API).unwrap(), &[], None);
    let f = flat(&out);
    // ctor-less handle class holding the core.
    assert!(
        f.contains("pub struct RpcProcessInstance")
            && f.contains("pub(crate) core: Arc<crate::core_impl::RpcProcessInstanceImpl>"),
        "node emits the RpcProcessInstance handle class holding Arc<Impl>"
    );
    assert!(
        !f.contains("#[napi(constructor)] pub fn new"),
        "the factory-born handle class has NO public constructor"
    );
    // the mint wrap on the factory free function.
    assert!(
        f.contains(
            "pub fn create_rpc_process_instance(instance_id: String) -> Result<RpcProcessInstance>"
        ) && f.contains("Ok(RpcProcessInstance { core:"),
        "node wraps the core-returned Arc<Impl> into the handle class"
    );
    // real subscription lowering — NOT the deferred skip-note.
    assert!(
        f.contains("pub fn on_event( &self, listener: ThreadsafeFunction")
            || f.contains("pub fn on_event(&self, listener: ThreadsafeFunction"),
        "node lowers the subscription as a real &self registration"
    );
    assert!(
        f.contains("let unsub = self.core.on_event(listener_cb)")
            && f.contains("Ok(Subscription {"),
        "node registers the listener and returns a Subscription handle"
    );
    assert!(
        !out.contains("subscription `RpcProcessInstance.on_event`: factory-born"),
        "node no longer emits #92's subscription skip-note for this interface"
    );
    // async unary methods on the handle.
    assert!(
        f.contains("pub fn send(&self, cmd: String) -> AsyncTask<SendTask>")
            && f.contains("pub fn dispose(&self) -> AsyncTask<DisposeTask>"),
        "node lowers the async unary methods as AsyncTasks"
    );
    // sync void method.
    assert!(
        f.contains("pub fn handle_ui_response(&self, resp: String) -> Result<()>"),
        "node lowers the sync void method"
    );
    // the handle class's Arc import is present (prelude gate widened).
    assert!(
        out.contains("use std::sync::Arc;"),
        "node imports Arc for the handle class"
    );
}

/// python FULLY lowers the factory-born interface: a ctor-less `#[pyclass]` holding
/// `Arc<Impl>` (NO `#[new]`), the mint wrap on the factory free function, and its
/// methods (subscription + async-as-sync unary + void) as `#[pymethods]`.
#[test]
fn python_full_lowering() {
    let out = python_binding(&load_api(API).unwrap(), &[], None);
    let f = flat(&out);
    let c = canon(&out);
    assert!(
        f.contains("pub struct RpcProcessInstance")
            && f.contains("pub(crate) core: Arc<crate::core_impl::RpcProcessInstanceImpl>"),
        "python emits the RpcProcessInstance pyclass holding Arc<Impl>"
    );
    assert!(
        !f.contains("#[new]"),
        "the factory-born pyclass has NO #[new] constructor"
    );
    assert!(
        c.contains("fncreate_rpc_process_instance(py:Python<'_>,instance_id:String)->PyResult<RpcProcessInstance>")
            && f.contains("Ok(RpcProcessInstance { core:"),
        "python wraps the core-returned Arc<Impl> into the pyclass"
    );
    assert!(
        c.contains("fnon_event(&self,py:Python<'_>,listener:PyObject)->PyResult<Subscription>"),
        "python lowers the subscription as a real &self registration"
    );
    assert!(
        c.contains("fnsend(&self,py:Python<'_>,cmd:String)->PyResult<String>")
            && c.contains("fndispose(&self,py:Python<'_>)->PyResult<()>"),
        "python lowers the (async→sync) unary methods on the handle"
    );
    assert!(
        f.contains("m.add_class::<RpcProcessInstance>()")
            && f.contains("m.add_function(wrap_pyfunction!(create_rpc_process_instance"),
        "python registers the handle class + the factory free function"
    );
}

/// Each FOLLOW-UP backend emits honest skip-notes rather than broken glue: an
/// interface-return note for the factory op (it cannot marshal the core's
/// `Arc<Impl>` as a value) and a factory-born-interface note for the ctor-less
/// handle it cannot yet mint.
#[test]
fn follow_up_backends_skip_note() {
    let api = load_api(API).unwrap();
    let backends = [
        ("cpp", cpp_binding(&api, &[], None)),
        ("java", java_binding(&api, &[], None)),
        ("ruby", ruby_binding(&api, &[], None)),
        ("php", php_binding(&api, &[], None)),
        ("wasm", wasm_binding(&api, &[], None)),
    ];
    for (lang, out) in backends {
        assert!(
            out.contains("returns handle `RpcProcessInstance`")
                && out.contains("not lowered for this backend yet; deferred."),
            "{lang} emits the factory-op interface-return skip-note"
        );
        assert!(
            out.contains("interface `RpcProcessInstance`: factory-born (ctor-less) handle"),
            "{lang} emits the factory-born-interface skip-note"
        );
        // the follow-up backend does NOT mint the handle (only node/python do); the
        // core trait still DECLARES the methods, but no binding wraps the mint.
        assert!(
            !out.contains("Ok(RpcProcessInstance {") && !out.contains("Ok(RpcProcessInstance{"),
            "{lang} does not emit the handle-mint wrap"
        );
    }
}
