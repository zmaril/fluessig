//! The napi (Node) template grid — one language's projection of the op shapes.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.

use genco::prelude::*;

use crate::api::{ApiDoc, ApiType, ApiUnion, CallbackSig, Shape};

use super::*;

/// This backend's language slug — the key it reads out of every symbol's
/// `bindings` map through the shared [`pinned_name`] / [`variant_token`]
/// resolver. Node hardcodes no pin; it only knows its own rename syntax
/// (`#[napi(js_name = …)]` on fields, `#[napi(value = …)]` on enum tokens).
const LANG: &str = "node";

/// napi's `Either` helper caps at `Either26` (`Either` = 2, `Either3`..=`Either26`).
/// A union with more variants than this falls back to the JSON envelope carrier.
const MAX_EITHER_ARITY: usize = 26;

/// Backend options for the node (napi) generator. The 3-arg [`node_binding`]
/// threads `NodeOptions::default()`, whose [`UnionProjection::default`] is now
/// structured tagged-object projection (`Either{N}`); pass an explicit
/// [`UnionProjection::Envelope`] to opt back into the historical JSON-string
/// carrier.
#[derive(Default, Clone)]
pub struct NodeOptions {
    /// How union return values and nested union DTO fields are lowered.
    pub union_projection: UnionProjection,
    /// When set, the package's public typings are fronted by this hand-written
    /// `.d.ts` (napi collapses literal tags to `type: string` and can't express
    /// `A | B`). The generated file carries an external-typings banner and omits
    /// the inline `ts_return_type` hints on union-typed symbols so napi's own
    /// `.d.ts` doesn't fight the external one.
    pub external_dts: Option<String>,
}

/// A union eligible for structured projection (2..=26 variants), else `None`.
fn structured_union<'a>(api: &'a ApiDoc, name: &str) -> Option<&'a ApiUnion> {
    api.unions.iter().find(|u| u.name == name).filter(|u| {
        let n = u.variants.len();
        (2..=MAX_EITHER_ARITY).contains(&n)
    })
}

/// napi's `Either` family name for an arity: 2 → `Either`, 3..=26 → `Either{n}`.
fn either_name(n: usize) -> String {
    if n == 2 {
        "Either".to_string()
    } else {
        format!("Either{n}")
    }
}

/// The exact set of `Either{N}` arities the structured projection will emit, so
/// the prelude imports precisely those (napi's 2-arity is `Either`, 3..=26 are
/// `Either3`..`Either26`). Importing an arity that never appears would trip a
/// consumer's `-D warnings` on the unused import, so we walk every type position
/// that goes through [`node_ty`] (DTO fields, op returns / stream items — union
/// params ride the shared envelope `String` and never project) and collect the
/// arities actually produced. Empty in envelope mode.
fn either_arities(api: &ApiDoc, opts: &NodeOptions) -> std::collections::BTreeSet<usize> {
    let mut set = std::collections::BTreeSet::new();
    if !matches!(opts.union_projection, UnionProjection::Structured { .. }) {
        return set;
    }
    fn walk(api: &ApiDoc, t: &ApiType, set: &mut std::collections::BTreeSet<usize>) {
        match t {
            ApiType::Union { union } => {
                if let Some(u) = structured_union(api, union) {
                    set.insert(u.variants.len());
                }
            }
            ApiType::List { list } => walk(api, list, set),
            ApiType::Nullable { nullable } => walk(api, nullable, set),
            _ => {}
        }
    }
    for m in &api.models {
        for f in &m.fields {
            walk(api, &f.ty, &mut set);
        }
    }
    for i in &api.interfaces {
        for op in &i.ops {
            walk(api, &op.returns, &mut set);
        }
    }
    set
}

/// Node's `<Interface>Core` traits: the shared [`emit_core_traits_with`] spine
/// driven with node's structured return mapping ([`node_ty`]) instead of the
/// envelope [`ty`], so a union-returning op's core-trait signature matches its
/// napi `Task::Output` (`Either{N}<…>`) and `compute`'s unwrapped passthrough
/// type-checks. This is the sole node divergence: union *params* still ride the
/// shared envelope `String`, matching the handle methods and tasks. In envelope
/// mode `node_ty` delegates to `ty`, so the output is byte-identical to the
/// language default.
fn emit_core_traits_node(t: &mut rust::Tokens, api: &ApiDoc, opts: &NodeOptions) {
    emit_core_traits_full(
        t,
        api,
        |op| node_ty(api, opts, &op.returns).0,
        node_param_sig,
        |op| op.result_error.clone(),
    );
}

/// Node's param `(name, rust_type)` list — the shared [`param_sig`] with ONE
/// divergence: a `bytes` PARAM is spelled `Uint8Array` (Feature 1 — pi/pidgin's
/// binary-input convention, byte-exact with pi's `.d.ts`), matching the handle
/// methods, free functions, and task fields so the core call type-checks. Every
/// non-bytes param delegates to the shared [`ty`], so it is byte-identical.
fn node_param_sig(api: &ApiDoc, op: &ApiOp) -> Vec<(String, String)> {
    param_sig_with(op, |t| node_param_ty(api, t).0)
}

/// The IN half of node's position-aware `bytes` spelling: a `bytes` param crosses
/// in as `napi::bindgen_prelude::Uint8Array` (a read-only view over the JS bytes),
/// which napi's `.d.ts` generator names `Uint8Array`. Non-bytes types delegate to
/// the shared [`ty`] (params never carry a structured union — they ride the
/// envelope `String`), so the spelling is byte-identical for everything else.
fn node_param_ty(api: &ApiDoc, t: &ApiType) -> (String, String) {
    match t {
        ApiType::Scalar(s) if s == "bytes" => (
            "napi::bindgen_prelude::Uint8Array".into(),
            "Uint8Array".into(),
        ),
        ApiType::List { list } => {
            let (r, s) = node_param_ty(api, list);
            (format!("Vec<{r}>"), format!("{s}[]"))
        }
        ApiType::Nullable { nullable } => {
            let (r, s) = node_param_ty(api, nullable);
            (format!("Option<{r}>"), format!("{s} | null"))
        }
        _ => ty(api, t),
    }
}

/// Does any op in the surface take a callback param? Gates the napi
/// `threadsafe_function` imports in the prelude, so a callback-free file emits
/// ZERO new import lines and its golden stays byte-identical.
fn api_uses_callback(api: &ApiDoc) -> bool {
    api.interfaces.iter().flat_map(|i| &i.ops).any(|op| {
        op.params
            .iter()
            .any(|p| matches!(&p.ty, ApiType::Callback { .. }))
    })
}

/// The napi BINDING-position spelling of a callback param: a
/// `ThreadsafeFunction<Args, ErrorStrategy::Fatal>` (single arg → the arg type
/// directly, N args → a tuple). This is the type JS supplies; the fn body bridges
/// it into the uniform core `Box<dyn Fn(..)>` via [`node_callback_wrapper`]. The
/// CORE-trait side keeps the shared `Box<dyn Fn>` spelling (via [`node_param_ty`]
/// → [`ty`]) so the boxed closure the binding builds type-checks against it.
fn node_callback_tsfn_ty(api: &ApiDoc, sig: &CallbackSig) -> String {
    let args: Vec<String> = sig.params.iter().map(|p| ty(api, p).0).collect();
    let arg = match args.len() {
        1 => args[0].clone(),
        _ => format!("({})", args.join(", ")),
    };
    format!("ThreadsafeFunction<{arg}, ErrorStrategy::Fatal>")
}

/// Node's BINDING param `(name, rust_type)` list: a callback param is spelled its
/// napi `ThreadsafeFunction` (the value JS supplies); every other param delegates
/// to [`node_param_ty`], so it is byte-identical to [`node_param_sig`]. Used for
/// the emitted fn/method signature ONLY — the core-trait method keeps the shared
/// `Box<dyn Fn>` spelling.
fn node_binding_param_sig(api: &ApiDoc, op: &ApiOp) -> Vec<(String, String)> {
    param_sig_with(op, |t| match t {
        ApiType::Callback { callback } => node_callback_tsfn_ty(api, callback),
        _ => node_param_ty(api, t).0,
    })
}

/// The wrapper `let {name}_cb: Box<dyn Fn(..)> = { … };` bridging a napi
/// `ThreadsafeFunction` param into the uniform core closure: the boxed `Fn`
/// forwards each invocation to the TSFN `NonBlocking` (queues to the JS event
/// loop, never blocks the caller thread). Single-arg forwards the value directly;
/// N-arg forwards a tuple.
fn node_callback_wrapper(api: &ApiDoc, name: &str, sig: &CallbackSig) -> String {
    let arg_tys: Vec<String> = sig.params.iter().map(|p| ty(api, p).0).collect();
    let vars = callback_arg_vars(arg_tys.len());
    let box_ty = format!("Box<dyn Fn({}) + Send + Sync>", arg_tys.join(", "));
    let closure_params = vars
        .iter()
        .zip(&arg_tys)
        .map(|(v, ty)| format!("{v}: {ty}"))
        .collect::<Vec<_>>()
        .join(", ");
    // the TSFN takes the sole value directly, or a tuple for 0 / N args.
    let call_arg = match vars.len() {
        1 => vars[0].clone(),
        _ => format!("({})", vars.join(", ")),
    };
    format!(
        "let {name}_cb: {box_ty} = {{ let tsfn = {name}.clone(); Box::new(move |{closure_params}| {{ tsfn.call({call_arg}, ThreadsafeFunctionCallMode::NonBlocking); }}) }};\n"
    )
}

/// The trait-call argument name list and the wrapper prelude for a node fn/method:
/// a callback param is passed as its bridged `{name}_cb` local (whose `let` is in
/// the returned prelude), every other param by its plain name. An op with no
/// callback param yields the historical name list and an empty prelude, so its
/// output is byte-identical.
fn node_call_bridge(api: &ApiDoc, op: &ApiOp) -> (String, String) {
    let mut wrappers = String::new();
    let names = op
        .params
        .iter()
        .map(|p| {
            let n = snake(&p.name);
            if let ApiType::Callback { callback } = &p.ty {
                wrappers.push_str(&node_callback_wrapper(api, &n, callback));
                format!("{n}_cb")
            } else {
                n
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    (names, wrappers)
}

/// The node `(rust, ts)` spelling of a type, applying structured union
/// projection when [`NodeOptions::union_projection`] asks for it. Delegates to
/// the shared [`ty`] for everything else, so envelope mode is byte-identical to
/// the historical output.
fn node_ty(api: &ApiDoc, opts: &NodeOptions, t: &ApiType) -> (String, String) {
    match (t, &opts.union_projection) {
        // Feature 1 — a `bytes` value crosses OUT (a return / a DTO field) as a
        // napi `Buffer` (an owned byte buffer), the JS idiom for binary output and
        // byte-exact with pi/pidgin. Spelled with the fully-qualified napi type so
        // napi's `.d.ts` generator names it `Buffer` directly (no alias to resolve,
        // no `ts_return_type` hint). `bytes` PARAMS are spelled `Uint8Array` by
        // [`node_param_sig`]; this is the OUT half of that position-aware split.
        (ApiType::Scalar(s), _) if s == "bytes" => {
            ("napi::bindgen_prelude::Buffer".into(), "Buffer".into())
        }
        (ApiType::Union { union }, UnionProjection::Structured { .. }) => {
            match structured_union(api, union) {
                Some(u) => {
                    let names: Vec<String> = u
                        .variants
                        .iter()
                        .map(|v| tagged_variant_name(&u.name, &v.tag))
                        .collect();
                    let either = either_name(u.variants.len());
                    (format!("{either}<{}>", names.join(", ")), names.join(" | "))
                }
                // too many (or too few) variants → envelope fallback
                None => ty(api, t),
            }
        }
        (ApiType::List { list }, _) => {
            let (r, s) = node_ty(api, opts, list);
            (format!("Vec<{r}>"), format!("{s}[]"))
        }
        (ApiType::Nullable { nullable }, _) => {
            let (r, s) = node_ty(api, opts, nullable);
            (format!("Option<{r}>"), format!("{s} | null"))
        }
        _ => ty(api, t),
    }
}

/// Emit the per-variant tagged `#[napi(object)]` structs plus the literal-set
/// `From<VariantModel>` conversions for every structurally-projected union.
/// Nothing is emitted in envelope mode, keeping historical output unchanged.
fn emit_union_variants(t: &mut rust::Tokens, api: &ApiDoc, opts: &NodeOptions) {
    let UnionProjection::Structured { tag_field } = &opts.union_projection else {
        return;
    };
    for u in &api.unions {
        let n = u.variants.len();
        if !(2..=MAX_EITHER_ARITY).contains(&n) {
            quote_in! { *t =>
                $['\r']
                $(format!("// note: union {} has {n} variants; napi Either caps at {MAX_EITHER_ARITY} — kept as the JSON envelope carrier.", u.name))
            };
            continue;
        }
        let field = u.tag_field.clone().unwrap_or_else(|| tag_field.clone());
        let ident = tag_ident(&field);
        for v in &u.variants {
            let sname = tagged_variant_name(&u.name, &v.tag);
            let ApiType::Model { model } = &v.ty else {
                quote_in! { *t =>
                    $['\r']
                    $(format!("// note: union {} variant {} is not a model ref — no tagged struct emitted.", u.name, v.tag))
                };
                continue;
            };
            let Some(m) = api.models.iter().find(|m| &m.name == model) else {
                continue;
            };
            // struct fields: the tag first, then the variant model's real fields
            let mut struct_fields: Vec<rust::Tokens> = Vec::new();
            struct_fields.push(quote!($(format!("pub {ident}: String,"))));
            let mut from_fields: Vec<String> = Vec::new();
            from_fields.push(format!("{ident}: {:?}.into(),", v.tag));
            for f in &m.fields {
                let (r, _) = node_ty(api, opts, &f.ty);
                let r = if f.nullable {
                    format!("Option<{r}>")
                } else {
                    r
                };
                let fname = snake(&f.name);
                struct_fields.push(quote!($(format!("pub {fname}: {r},"))));
                from_fields.push(format!("{fname}: v.{fname},"));
            }
            quote_in! { *t =>
                $['\r']
                $(format!("/// `{}` union variant `{}` — the tag `{}` rides as the `{}` literal.", u.name, v.tag, v.tag, field))
                #[napi(object)]
                #[derive(Clone)]
                pub struct $(&sname) {
                    $(for f in &struct_fields join ($['\r']) => $f)
                }
                impl From<$model> for $(&sname) {
                    fn from(v: $model) -> Self {
                        Self {
                            $(for f in &from_fields join ($['\r']) => $f)
                        }
                    }
                }
                $['\n']
            };
        }
    }
}

/// The two `#[napi(object)]` arm names of a `#[fluessig(result)]` op's envelope,
/// keyed off the op name: `readBinaryFile` → (`ReadBinaryFileOk`,
/// `ReadBinaryFileErr`). The struct emission and the method return type read this
/// same helper so they always agree.
fn result_envelope_names(op_name: &str) -> (String, String) {
    let p = pascal(op_name);
    (format!("{p}Ok"), format!("{p}Err"))
}

/// Feature 2 — emit the two `#[napi(object)]` arms of every `#[fluessig(result)]`
/// op's envelope: `<Op>Ok { ok, value: T }` and `<Op>Err { ok, error: E }`. The
/// op's handle method / free function returns `Either<<Op>Ok, <Op>Err>`, which
/// napi renders `<Op>Ok | <Op>Err`; the `ok` bool discriminates the arms. Nothing
/// is emitted when no op is marked, so historical output is byte-identical.
///
/// NOTE: napi collapses a bool field to `ok: boolean` in its generated `.d.ts`
/// (the same limitation the structured-union `type: string` tags hit) — the exact
/// discriminated `ok: true` / `ok: false` literals are an external-`.d.ts`
/// concern, out of scope here. The structural `{ ok, value } | { ok, error }`
/// shape is exact.
fn emit_result_envelopes(t: &mut rust::Tokens, api: &ApiDoc, opts: &NodeOptions) {
    for i in &api.interfaces {
        for op in &i.ops {
            let Some(err_rec) = &op.result_error else {
                continue;
            };
            let (ok_name, err_name) = result_envelope_names(&op.name);
            let (val_ty, _) = node_ty(api, opts, &op.returns);
            quote_in! { *t =>
                $['\r']
                $(format!("/// The `ok` arm of `{}.{}`'s result envelope — `{{ ok: true, value }}`.", i.name, op.name))
                #[napi(object)]
                #[derive(Clone)]
                pub struct $(&ok_name) {
                    $("/// Always `true` — discriminates the envelope's success arm.")
                    pub ok: bool,
                    $(format!("pub value: {val_ty},"))
                }
                $(format!("/// The `error` arm of `{}.{}`'s result envelope — `{{ ok: false, error }}`,", i.name, op.name))
                $(format!("/// the `{err_rec}` record handed back AS A VALUE (never thrown)."))
                #[napi(object)]
                #[derive(Clone)]
                pub struct $(&err_name) {
                    $("/// Always `false` — discriminates the envelope's error arm.")
                    pub ok: bool,
                    $(format!("pub error: {err_rec},"))
                }
                $['\n']
            };
        }
    }
}

/// The `#[napi(ts_return_type = "…")]` attribute line for a return position, or
/// an empty string when the external-`.d.ts` mode suppresses it for a union.
fn ts_return_attr(
    api: &ApiDoc,
    opts: &NodeOptions,
    ret: &ApiType,
    wrap: impl Fn(&str) -> String,
) -> String {
    if opts.external_dts.is_some() && matches!(ret, ApiType::Union { .. }) {
        return String::new();
    }
    let (_, ts) = node_ty(api, opts, ret);
    format!("#[napi(ts_return_type = {:?})]", wrap(&ts))
}

/// Inject an op-level export-name pin (Feature B — `#[fluessig(name = "…")]`,
/// read as `bindings["node"].name`) into a unary op's `#[napi(…)]` attribute as
/// a `js_name`. `pin` `None` ⇒ `attr` is returned VERBATIM, so an unpinned op is
/// byte-identical to before this authoring path existed. When the async
/// `ts_return_type` hint is suppressed (an external-`.d.ts` union return, giving
/// an empty `attr`) a pinned op still gets a lone `#[napi(js_name = "…")]`.
fn with_js_name(attr: String, pin: Option<&str>) -> String {
    let Some(js) = pin else { return attr };
    let js = format!("js_name = {js:?}");
    match attr.strip_prefix("#[napi(") {
        Some(rest) => format!("#[napi({js}, {rest}"),
        None => format!("#[napi({js})]"),
    }
}

/// The `#[napi(…)]` attribute for a SYNCHRONOUS (`#[fluessig(sync)]`) unary op:
/// `#[napi(js_name = "…")]` when name-pinned, else the bare `#[napi]`. A sync op
/// carries no `ts_return_type` — its emitted return type IS the value (no
/// `AsyncTask` wrapper for napi to see through).
fn sync_napi_attr(pin: Option<&str>) -> String {
    match pin {
        Some(js) => format!("#[napi(js_name = {js:?})]"),
        None => "#[napi]".to_string(),
    }
}

/// Which prelude items a surface's ops actually pull in — the gate that keeps a
/// pure sync-infallible binding free of the napi-3-ONLY streaming/async symbols
/// (`AsyncGenerator` / `AsyncTask`) so it compiles against `napi = "2"`. The
/// prior code emitted the whole streaming/async prelude UNCONDITIONALLY, so a
/// stream-less, ctor-less, sync-infallible surface failed with `unresolved
/// import napi::bindgen_prelude::AsyncGenerator` (that symbol does not exist in
/// napi 2). Each import is therefore gated on the op kinds that use it; a full
/// (stream/async/ctor/fallible) surface still emits every item, so its bytes are
/// unchanged.
struct NodePrelude {
    /// Any `stream` op → `AsyncGenerator` + `Future`/`Duration` + the shared
    /// `Poll`/`PollStream` runtime import (all streaming-only, napi-3 for the
    /// generator trait).
    stream: bool,
    /// `AsyncTask` + `napi::{Env, Task}` — an async unary op OR a stream (whose
    /// retained `next()` cursor is itself an `AsyncTask`).
    async_task: bool,
    /// `Arc` — an ORDINARY handle (ctor) surface or a stream (both hold `Arc<…>`).
    /// A `single_threaded` handle does NOT hold `Arc` (it is thread-confined), so a
    /// surface whose only ctor interfaces are single_threaded (and no stream)
    /// leaves this `false` and never imports `Arc`.
    arc: bool,
    /// `std::cell::RefCell` — a `single_threaded` handle holds its core as
    /// `RefCell<…Impl>` (thread-confined interior mutability, no `Send`), so any
    /// single_threaded ctor interface turns this on.
    single_threaded: bool,
    /// `napi::bindgen_prelude::Result` + the `err` fn — any op/getter that throws
    /// on `Err`: an async task, a stream, a ctor `new`, a sync-FALLIBLE op, or an
    /// Arrow-payload IPC getter. `Result` and `err` are always co-present (every
    /// throwing site both spells `Result<…>` and calls `.map_err(err)`), so one
    /// flag gates both.
    result: bool,
    /// Any op with a callback param → the `napi::threadsafe_function::{…}` import
    /// trio (the `ThreadsafeFunction` binding type + `NonBlocking` call mode +
    /// `ErrorStrategy`). A callback-free surface leaves this `false` and emits ZERO
    /// new import lines, keeping its golden byte-identical.
    callback: bool,
}

impl NodePrelude {
    fn of(api: &ApiDoc) -> Self {
        let stream = api
            .interfaces
            .iter()
            .flat_map(|i| &i.ops)
            .any(|o| o.shape == Shape::Stream);
        let async_unary = api
            .interfaces
            .iter()
            .flat_map(|i| &i.ops)
            .any(|o| o.shape == Shape::Unary && o.is_async);
        let ctor = api
            .interfaces
            .iter()
            .flat_map(|i| &i.ops)
            .any(|o| o.shape == Shape::Ctor);
        // an ORDINARY (async-capable) handle holds `Arc<Impl>`; a `single_threaded`
        // handle does NOT (it is thread-confined, `RefCell<Impl>`). So `Arc` is
        // needed only for a NON-single_threaded ctor interface (or a stream).
        let ordinary_ctor = api
            .interfaces
            .iter()
            .filter(|i| !i.single_threaded)
            .flat_map(|i| &i.ops)
            .any(|o| o.shape == Shape::Ctor);
        // any `single_threaded` ctor interface → the handle holds `RefCell<Impl>`.
        let single_threaded = api
            .interfaces
            .iter()
            .filter(|i| i.single_threaded)
            .flat_map(|i| &i.ops)
            .any(|o| o.shape == Shape::Ctor);
        // a DEFAULT sync unary op that is NOT infallible keeps a `Result<T>` seam
        // and throws → `Result` + `err`.
        let sync_fallible = api
            .interfaces
            .iter()
            .flat_map(|i| &i.ops)
            .any(|o| o.shape == Shape::Unary && !o.is_async && !o.infallible);
        // an Arrow-payload DTO's IPC getter returns `Result<Bytes>` + `.map_err(err)`.
        let arrow = api.models.iter().any(|m| arrow_field(m).is_some());
        Self {
            stream,
            async_task: async_unary || stream,
            arc: ordinary_ctor || stream,
            single_threaded,
            // the ctor of a single_threaded handle still throws (`.map_err(err)?`),
            // so it too needs `Result` + `err` — `ctor` (any ctor) already covers it.
            result: async_unary || stream || ctor || sync_fallible || arrow,
            callback: api_uses_callback(api),
        }
    }

    /// True when the surface emits the FULL historical prelude (every streaming/
    /// async/fallible item). Only a REDUCED surface gets the belt-and-suspenders
    /// `#![allow(unused_imports)]` banner line — this keeps a full surface's
    /// committed golden byte-identical (it never carried that line).
    fn is_full(&self) -> bool {
        self.stream && self.async_task && self.arc && self.result
    }
}

/// Generate the napi (Node) binding with default options: structured
/// tagged-object union projection (`Either{N}`, tag field `"type"`) and no
/// external `.d.ts`. A thin wrapper over [`node_binding_with_options`]; pass an
/// explicit [`UnionProjection::Envelope`] to opt into the JSON-string carrier.
pub fn node_binding(api: &ApiDoc, enums: &[EnumDesc], banner_note: Option<&str>) -> String {
    node_binding_with_options(api, enums, banner_note, &NodeOptions::default())
}

/// Generate the napi (Node) binding: DTO structs, enums, core traits, per-op
/// AsyncTasks, stream classes, free functions, and the handle class. `opts`
/// selects union projection (envelope vs. structured `Either{N}`) and the
/// external-`.d.ts` mode.
pub fn node_binding_with_options(
    api: &ApiDoc,
    enums: &[EnumDesc],
    banner_note: Option<&str>,
    opts: &NodeOptions,
) -> String {
    // The CONDITIONAL streaming/async prelude. Each `use` is gated on the op
    // kinds that use it ([`NodePrelude`]); a pure sync-infallible surface reduces
    // to just `use napi_derive::napi;` (napi-2-compatible). A full surface still
    // emits every item, and since rustfmt reorders the imports the emission order
    // here is immaterial — the formatted bytes match the historical prelude.
    let needs = NodePrelude::of(api);
    // Structured projection emits bare `Either{N}<…>` in DTO fields, op returns,
    // and `Task::Output`; napi's prelude glob is NOT imported, so the exact
    // arities used must be named here (a real napi compile fails E0425 otherwise).
    // Envelope mode produces no `Either`, so the set is empty.
    let either = either_arities(api, opts);
    let mut t: rust::Tokens = quote! {
        $("// The fixed prelude — generated code uses fully-qualified paths elsewhere.")
    };
    // `use napi::bindgen_prelude::{…}` — only the items actually used, in the
    // historical order (AsyncGenerator, AsyncTask, Result, then Either arities).
    // Omitted entirely for a pure sync-infallible surface (napi-3-free).
    let mut bp: Vec<String> = Vec::new();
    if needs.stream {
        bp.push("AsyncGenerator".to_string());
    }
    if needs.async_task {
        bp.push("AsyncTask".to_string());
    }
    if needs.result {
        bp.push("Result".to_string());
    }
    bp.extend(either.iter().map(|&n| either_name(n)));
    if !bp.is_empty() {
        let line = format!("use napi::bindgen_prelude::{{{}}};", bp.join(", "));
        quote_in! { t => $['\r'] $(&line) };
    }
    if needs.async_task {
        // `Env`/`Task` back every `AsyncTask` (an async unary op or a stream's
        // retained poll cursor).
        quote_in! { t => $['\r'] use napi::{Env, Task}; };
    }
    // ALWAYS — the one napi-2-compatible import every surface needs.
    quote_in! { t => $['\r'] use napi_derive::napi; };
    if needs.callback {
        // Callback params cross in as a napi `ThreadsafeFunction`, delivered to the
        // JS event loop `NonBlocking` (never blocks the caller thread). Guarded, so
        // a callback-free surface emits none of these lines.
        quote_in! { t => $['\r'] use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode, ErrorStrategy}; };
    }
    if needs.stream {
        quote_in! { t => $['\r'] use std::future::Future; };
    }
    if needs.arc {
        quote_in! { t => $['\r'] use std::sync::Arc; };
    }
    if needs.single_threaded {
        // a `single_threaded` handle holds its `!Send` core as `RefCell<…Impl>` —
        // thread-confined interior mutability, no `Arc`, no `Send`/`Sync`.
        quote_in! { t => $['\r'] use std::cell::RefCell; };
    }
    if needs.stream {
        // the shared streaming-contract import flows through the use-emitter
        // ([`RUNTIME_STREAM_IMPORT`]) rather than a hardcoded string, so every
        // generated `use` line has one emission path; renders byte-identically.
        let runtime_import = RUNTIME_STREAM_IMPORT.render();
        quote_in! { t =>
            $['\r'] use std::time::Duration;
            $['\r'] $("// The shared streaming contract — Poll/PollStream live in the fluessig-runtime crate.")
            $['\r'] $(&runtime_import)
        };
    }
    if needs.result {
        // the shared `Err`→`napi::Error` mapper, called at every throwing site.
        quote_in! { t =>
            $['\n']
            fn err(e: impl std::fmt::Display) -> napi::Error {
                napi::Error::from_reason(e.to_string())
            }
        };
    }
    if api_uses_bytes(api) {
        quote_in! { t =>
            $['\n']
            $("/// Bulk bytes cross into JS as a Buffer (Arrow IPC payloads and friends).")
            pub type Bytes = napi::bindgen_prelude::Buffer;
        };
    }
    t.line();

    // ── enums (name-only variants → napi string enums; wire-valued → strings) ──
    // A name-only enum lowers to a napi *string* enum whose variants carry an
    // explicit snake_case wire token (`#[napi(value = "…")]`), so JS sees
    // `CapabilityKind.Dispatch === "dispatch"` — the same tokens the ruby
    // emitter hands out via `wire()`, not the magic discriminant number a plain
    // `#[napi]` enum would emit. The Rust variant idents are unchanged, so a
    // consumer's core_impl keeps constructing `CapabilityKind::Dispatch`.
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        // each line: `#[napi(value = "<wire token>")] <PascalVariant>,` — the
        // token comes from the shared resolver (a `node` pin wins, then the
        // neutral `Variant.value`, else the catalog member lowercased, identical
        // to ruby's `wire()`). The Rust variant ident is always `pascal(name)`,
        // independent of the wire token.
        let vs: Vec<String> = variants
            .iter()
            .map(|v| {
                format!(
                    "#[napi(value = {:?})] {},",
                    variant_token(v, LANG),
                    pascal(&v.name)
                )
            })
            .collect();
        // napi 3 no longer auto-derives Clone/Copy on #[napi] enums; option
        // structs that carry one derive Clone, so the enum must too.
        quote_in! { t =>
            $['\n']
            #[napi(string_enum)]
            #[derive(Clone, Copy)]
            pub enum $name {
                $(for v in &vs join ($['\r']) => $v)
            }
        };
    }
    t.line();

    // ── DTO structs ──
    for m in &api.models {
        if let Some(doc) = &m.doc {
            for line in doc.lines() {
                quote_in! { t => $['\r']$(format!("/// {line}")) };
            }
        }
        if let Some(af) = arrow_field(m) {
            // Arrow-payload DTO: a class holding the RecordBatch, getters for the
            // scalar envelope, and a lazy IPC getter — no encode until accessed.
            let plain: Vec<&crate::api::ApiField> =
                m.fields.iter().filter(|f| f.name != af.name).collect();
            let storage: Vec<rust::Tokens> = plain
                .iter()
                .map(|f| {
                    let (r, _) = ty(api, &f.ty);
                    let n = snake(&f.name);
                    quote!(pub(crate) $n: $r,)
                })
                .collect();
            let getters: Vec<rust::Tokens> = plain
                .iter()
                .map(|f| {
                    let (r, _) = ty(api, &f.ty);
                    let n = snake(&f.name);
                    quote! {
                        #[napi(getter)]
                        pub fn $(&n)(&self) -> $r {
                            self.$(&n).clone()
                        }
                    }
                })
                .collect();
            let ipc = snake(&af.name);
            quote_in! { t =>
                $['\r']
                #[napi]
                #[derive(Clone)]
                pub struct $(&m.name) {
                    $(for f in &storage join ($['\r']) => $f)
                    $("// the rows, still columnar — encoded only when the getter is hit")
                    pub(crate) batch: entl_core::RecordBatch,
                }
                #[napi]
                impl $(&m.name) {
                    $(for g in &getters join ($['\r']) => $g)
                    $("/// The rows as one Arrow IPC stream — decode with `tableFromIPC` (apache-arrow).")
                    #[napi(getter, ts_return_type = "Buffer")]
                    pub fn $(&ipc)(&self) -> Result<Bytes> {
                        Ok(entl_core::batch_ipc(&self.batch).map_err(err)?.into())
                    }
                }
                $['\n']
            };
            continue;
        }
        let fields: Vec<rust::Tokens> = m
            .fields
            .iter()
            .map(|f| {
                // structured projection reaches nested union-typed DTO fields too
                let (r, _) = node_ty(api, opts, &f.ty);
                let r = if f.nullable {
                    format!("Option<{r}>")
                } else {
                    r
                };
                // The Rust field ident is always `snake(&f.name)` (a valid
                // ident); a `node` pin puts the exact JS spelling ONLY in a
                // `#[napi(js_name = "…")]` attr, overriding napi's default
                // snake→camel casing. Un-pinned ⇒ no attr, byte-identical.
                let n = snake(&f.name);
                match pinned_name(&f.bindings, LANG) {
                    Some(js) => {
                        let attr = format!("#[napi(js_name = {js:?})]");
                        quote!($attr pub $n: $r,)
                    }
                    None => quote!(pub $n: $r,),
                }
            })
            .collect();
        quote_in! { t =>
            $['\r']
            #[napi(object)]
            #[derive(Clone)]
            pub struct $(&m.name) {
                $(for f in &fields join ($['\r']) => $f)
            }
            $['\n']
        };
    }

    // per-variant tagged structs (+ literal-set conversions) for structured unions
    emit_union_variants(&mut t, api, opts);

    // Feature 2 — the `{ ok, value } | { ok, error }` envelope arm structs for
    // every `#[fluessig(result)]` op (nothing emitted when none is marked).
    emit_result_envelopes(&mut t, api, opts);

    emit_core_traits_node(&mut t, api, opts);

    // The generated `Subscription` handle class — emitted ONCE (guarded, so a
    // subscription-free surface is byte-identical). It wraps the core's returned
    // UNSUBSCRIBE closure; `unsubscribe()` (and drop) takes and calls it.
    if api_uses_subscription(api) {
        quote_in! { t =>
            $['\r']
            $("/// A subscription handle wrapping the core's returned unsubscribe closure.")
            $("/// `unsubscribe()` (or dropping the handle) removes the registered listener.")
            #[napi]
            pub struct Subscription {
                unsub: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
            }
            #[napi]
            impl Subscription {
                #[napi]
                pub fn unsubscribe(&self) {
                    if let Some(f) = self.unsub.lock().unwrap().take() {
                        f();
                    }
                }
            }
            $['\n']
        };
    }

    // ── per-interface surface ──
    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        let trait_name = format!("{}Core", i.name);
        let impl_path = format!("crate::core_impl::{}Impl", i.name);

        // stream classes + next-tasks. The error model is chosen per-op by
        // `stream_error`: `None` (unannotated) = the DEFAULT idiomatic native-TS
        // REJECT (a mid-stream `Poll::Failed` maps to `Err(err(e))`, so the awaited
        // pull rejects and `for await` throws — safe by default, no silent-swallow);
        // `Some(shape)` = opt-in error-AS-EVENT (mirror-a-library mode, e.g. pi's
        // `{ type, reason, error }`), where the failure is yielded as a terminal
        // event and the stream then completes (never rejects). `Poll::Failed(String)`
        // is the core→binding channel in BOTH modes; only the mapping differs.
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            let (item, _) = node_ty(api, opts, &op.returns);
            match &op.stream_error {
                // ── DEFAULT throw-mode (unannotated): native-TS reject ──
                None => {
                    let next_attr = ts_return_attr(api, opts, &op.returns, |ts| {
                        format!("Promise<{ts} | null>")
                    });
                    quote_in! { t =>
                        $['\r']
                        $(format!("/// Event stream from `{}.{}`.", i.name, op.name))
                        $("///")
                        $("/// Primary surface: a JS async-iterable — `for await (const ev of stream)`.")
                        $("/// Retained surface: `next()` poll cursor (resolves `null` at end) for")
                        $("/// consumers that cannot use async iteration or napi's `tokio_rt` feature.")
                        $("///")
                        $("/// DEFAULT error model = idiomatic native-TS REJECT: a mid-stream core")
                        $("/// failure (`Poll::Failed`) maps to `Err(err(e))`, so the awaited pull")
                        $("/// REJECTS and the `for await` loop THROWS — safe by default, never a")
                        $("/// silent-swallow. Annotate the op `@streamError` to opt into the")
                        $("/// error-AS-EVENT model instead (mirror a source library like pi).")
                        #[napi(async_iterator)]
                        pub struct $(&class) {
                            stream: Arc<dyn PollStream<$(&item)>>,
                        }

                        $("// Async-iterable surface (Symbol.asyncIterator). napi drives one pull at a")
                        $("// time, so backpressure is one in-flight poll by construction.")
                        #[napi]
                        impl AsyncGenerator for $(&class) {
                            type Yield = $(&item);
                            type Next = ();
                            type Return = ();

                            fn next(
                                &mut self,
                                _value: Option<Self::Next>,
                            ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
                                let stream = self.stream.clone();
                                async move {
                                    loop {
                                        let s = stream.clone();
                                        $("// Drive the blocking poll off the async runtime so the Node")
                                        $("// event loop is never blocked.")
                                        let poll = napi::tokio::task::spawn_blocking(move || {
                                            s.poll(Duration::from_millis(500))
                                        })
                                        .await
                                        .map_err(err)?;
                                        $("// DEFAULT throw-mode: a mid-stream failure REJECTS the pull")
                                        $("// (native TS — the `for await` loop throws). Opt into")
                                        $("// error-as-event with `@streamError`.")
                                        match poll {
                                            Poll::Item(v) => return Ok(Some(v)),
                                            Poll::Idle => continue,
                                            Poll::Closed => return Ok(None),
                                            Poll::Failed(e) => return Err(err(e)),
                                        }
                                    }
                                }
                            }

                            fn complete(
                                &mut self,
                                _value: Option<Self::Return>,
                            ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
                                $("// Cancellation: consumer called `return()` (e.g. `break` in for-await).")
                                let stream = self.stream.clone();
                                async move {
                                    stream.close();
                                    Ok(None)
                                }
                            }
                        }

                        $("// Backstop: guarantee core-side close even if the consumer neither")
                        $("// exhausts nor cancels the iterator.")
                        impl Drop for $(&class) {
                            fn drop(&mut self) {
                                self.stream.close();
                            }
                        }

                        $("// Retained poll cursor: `next(): Promise<Item | null>`.")
                        pub struct Next$(&class)Task {
                            stream: Arc<dyn PollStream<$(&item)>>,
                        }
                        impl Task for Next$(&class)Task {
                            type Output = Option<$(&item)>;
                            type JsValue = Option<$(&item)>;
                            fn compute(&mut self) -> Result<Self::Output> {
                                loop {
                                    match self.stream.poll(Duration::from_millis(500)) {
                                        Poll::Item(v) => return Ok(Some(v)),
                                        Poll::Idle => continue,
                                        Poll::Closed => return Ok(None),
                                        $("// throw-mode: reject the pull (native TS).")
                                        Poll::Failed(e) => return Err(err(e)),
                                    }
                                }
                            }
                            fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> {
                                Ok(o)
                            }
                        }
                        #[napi]
                        impl $(&class) {
                            $next_attr
                            pub fn next(&self) -> AsyncTask<Next$(&class)Task> {
                                AsyncTask::new(Next$(&class)Task { stream: self.stream.clone() })
                            }
                        }
                        $['\n']
                    };
                }
                // ── OPT-IN event-mode (@streamError): error-as-event (mirror a library) ──
                Some(se) => {
                    let err_evt = format!("{class}ErrorEvent");
                    // each field: a js_name attr only when the js-name diverges from the
                    // rust ident (the tag always needs one — `type_` never equals its
                    // js-name), mirroring the `{:?}` string-literal idiom above.
                    let ev_field = |rust: &str, js: &str| {
                        if js == rust {
                            format!("pub {rust}: String,")
                        } else {
                            format!("#[napi(js_name = {js:?})] pub {rust}: String,")
                        }
                    };
                    let ev_fields: Vec<String> = vec![
                        ev_field("type_", &se.tag_name),
                        ev_field("reason", &se.reason_name),
                        ev_field("error", &se.error_name),
                    ];
                    let next_attr = ts_return_attr(api, opts, &op.returns, |ts| {
                        format!("Promise<{ts} | {err_evt} | null>")
                    });
                    quote_in! { t =>
                        $['\r']
                        $(format!("/// The terminal error event yielded (NEVER thrown) when `{}.{}`'s core stream", i.name, op.name))
                        $("/// fails after it has started — the opt-in `@streamError` (error-as-event)")
                        $("/// model, mirroring a source library's contract (pi's post-start boundary as")
                        $("/// a plain value). NOTE: a core that instead surfaces its terminal error as a")
                        $("/// normal union VARIANT of the element type already rides out through")
                        $("/// `Poll::Item`; this struct is only the carrier for a `Result`/error failure.")
                        #[napi(object)]
                        pub struct $(&err_evt) {
                            $(for f in &ev_fields join ($['\r']) => $f)
                        }
                        $(format!("/// Event stream from `{}.{}`.", i.name, op.name))
                        $("///")
                        $("/// Primary surface: a JS async-iterable — `for await (const ev of stream)`.")
                        $("/// Retained surface: `next()` poll cursor (resolves `null` at end) for")
                        $("/// consumers that cannot use async iteration or napi's `tokio_rt` feature.")
                        $("///")
                        $("/// `@streamError` error model = error-AS-EVENT: a mid-stream core failure is")
                        $("/// yielded as a terminal `<Op>ErrorEvent` and the stream then completes —")
                        $("/// it NEVER rejects/throws (mirrors pi's contract, packages/ai/src/types.ts).")
                        #[napi(async_iterator)]
                        pub struct $(&class) {
                            stream: Arc<dyn PollStream<$(&item)>>,
                            $("// latched once the terminal error event is handed out — a started stream")
                            $("// never restarts, so every subsequent next() must resolve null (done).")
                            closed: Arc<std::sync::atomic::AtomicBool>,
                        }

                        $("// Async-iterable surface (Symbol.asyncIterator). napi drives one pull at a")
                        $("// time, so backpressure is one in-flight poll by construction.")
                        #[napi]
                        impl AsyncGenerator for $(&class) {
                            $("// Yield WIDENED to Either<item, error-event> — event-mode only.")
                            $("// WHY the Either: the terminal error event is a distinct TOP-LEVEL shape")
                            $("// `{ type, reason, error }` whose keys differ from the element/union")
                            $("// carrier, so it cannot ride the plain `item` Yield — it must be a second")
                            $("// arm. napi renders `Either<A, B>` as `A | B` in the generated `.d.ts`,")
                            $("// so the async iterator's element type reads `item | <Op>ErrorEvent`.")
                            $("// (Unannotated ops keep `type Yield = <item>` — this surface is untouched.)")
                            type Yield = napi::bindgen_prelude::Either<$(&item), $(&err_evt)>;
                            type Next = ();
                            type Return = ();

                            fn next(
                                &mut self,
                                _value: Option<Self::Next>,
                            ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
                                let stream = self.stream.clone();
                                let closed = self.closed.clone();
                                async move {
                                    use std::sync::atomic::Ordering;
                                    $("// A started stream never restarts: once the terminal error event has")
                                    $("// been handed out the latch is set, so every subsequent pull completes.")
                                    if closed.load(Ordering::SeqCst) {
                                        return Ok(None);
                                    }
                                    loop {
                                        let s = stream.clone();
                                        $("// Drive the blocking poll off the async runtime so the Node")
                                        $("// event loop is never blocked.")
                                        let poll = napi::tokio::task::spawn_blocking(move || {
                                            s.poll(Duration::from_millis(500))
                                        })
                                        .await
                                        .map_err(err)?;
                                        $("// event-mode: a mid-stream failure is ENCODED IN THE STREAM as a")
                                        $("// terminal error EVENT and the stream then completes — it must")
                                        $("// NEVER reject/throw. `Poll::Failed` yields `Either::B(event)` then")
                                        $("// the latch makes the next pull return `Ok(None)`.")
                                        match poll {
                                            Poll::Item(v) => return Ok(Some(napi::bindgen_prelude::Either::A(v))),
                                            Poll::Idle => continue,
                                            Poll::Closed => return Ok(None),
                                            Poll::Failed(e) => {
                                                $("// latch closed so the next pull completes, then hand the failure")
                                                $("// out AS A VALUE — never a thrown/rejected error.")
                                                closed.store(true, Ordering::SeqCst);
                                                return Ok(Some(napi::bindgen_prelude::Either::B($(&err_evt) {
                                                    type_: $(quoted(se.tag_value.clone())).into(),
                                                    reason: "error".into(),
                                                    error: e,
                                                })));
                                            }
                                        }
                                    }
                                }
                            }

                            fn complete(
                                &mut self,
                                _value: Option<Self::Return>,
                            ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
                                $("// Cancellation: consumer called `return()` (e.g. `break` in for-await).")
                                let stream = self.stream.clone();
                                async move {
                                    stream.close();
                                    Ok(None)
                                }
                            }
                        }

                        $("// Backstop: guarantee core-side close even if the consumer neither")
                        $("// exhausts nor cancels the iterator.")
                        impl Drop for $(&class) {
                            fn drop(&mut self) {
                                self.stream.close();
                            }
                        }

                        $("// Retained poll cursor: `next(): Promise<Item | <Op>ErrorEvent | null>`.")
                        pub struct Next$(&class)Task {
                            stream: Arc<dyn PollStream<$(&item)>>,
                            closed: Arc<std::sync::atomic::AtomicBool>,
                        }
                        impl Task for Next$(&class)Task {
                            $("// Either::A = a normal item; Either::B = the terminal error event. The")
                            $("// in-stream failure path is a VALUE, never a rejected promise.")
                            type Output = Option<napi::bindgen_prelude::Either<$(&item), $(&err_evt)>>;
                            type JsValue = Option<napi::bindgen_prelude::Either<$(&item), $(&err_evt)>>;
                            fn compute(&mut self) -> Result<Self::Output> {
                                use std::sync::atomic::Ordering;
                                if self.closed.load(Ordering::SeqCst) {
                                    return Ok(None);
                                }
                                loop {
                                    match self.stream.poll(Duration::from_millis(500)) {
                                        Poll::Item(v) => return Ok(Some(napi::bindgen_prelude::Either::A(v))),
                                        Poll::Idle => continue,
                                        Poll::Closed => return Ok(None),
                                        Poll::Failed(e) => {
                                            $("// latch closed so the next next() resolves null, then hand the")
                                            $("// failure out AS A VALUE — never a thrown/rejected error.")
                                            self.closed.store(true, Ordering::SeqCst);
                                            return Ok(Some(napi::bindgen_prelude::Either::B($(&err_evt) {
                                                type_: $(quoted(se.tag_value.clone())).into(),
                                                reason: "error".into(),
                                                error: e,
                                            })));
                                        }
                                    }
                                }
                            }
                            fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> {
                                Ok(o)
                            }
                        }
                        #[napi]
                        impl $(&class) {
                            $next_attr
                            pub fn next(&self) -> AsyncTask<Next$(&class)Task> {
                                AsyncTask::new(Next$(&class)Task { stream: self.stream.clone(), closed: self.closed.clone() })
                            }
                        }
                        $['\n']
                    };
                }
            }
        }

        // unary op tasks — a SYNCHRONOUS op (the default; `#[fluessig(async)]`
        // opts back into async) needs NO off-thread Task (it is emitted as a plain
        // `#[napi] fn`), so only the async unary ops generate one here.
        for op in i
            .ops
            .iter()
            .filter(|o| o.shape == Shape::Unary && o.is_async)
        {
            let task = format!("{}Task", pascal(&op.name));
            let name = snake(&op.name);
            let (ret, _) = node_ty(api, opts, &op.returns);
            let fields: Vec<String> = node_param_sig(api, op)
                .iter()
                .map(|(n, r)| format!("{n}: {r},"))
                .collect();
            let args = node_param_sig(api, op)
                .iter()
                .map(|(n, _)| format!("self.{n}.clone()"))
                .collect::<Vec<_>>()
                .join(", ");
            let call = if has_ctor {
                format!("self.core.{name}({args})")
            } else {
                format!("<{impl_path} as {trait_name}>::{name}({args})")
            };
            let core_field = if has_ctor {
                format!("core: Arc<{impl_path}>,")
            } else {
                String::new()
            };
            quote_in! { t =>
                $['\r']
                pub struct $(&task) {
                    $core_field
                    $(for f in &fields join ($['\r']) => $f)
                }
                impl Task for $(&task) {
                    type Output = $(&ret);
                    type JsValue = $(&ret);
                    fn compute(&mut self) -> Result<Self::Output> {
                        $call.map_err(err)
                    }
                    fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> {
                        Ok(o)
                    }
                }
                $['\n']
            };
        }

        if has_ctor {
            // the handle class. A `single_threaded` interface diverges from the
            // ordinary (async-capable) handle: it holds its core by plain
            // ownership inside a `RefCell` — NO `Arc`, NO `Send`/`Sync` — so a
            // `!Send` core (pidgin's `TuiCore`: `Rc<RefCell<…>>` + non-Send
            // closures) can be GENERATED as a thread-confined napi class. A napi
            // class instance never crosses threads, so it needs no `Send` bound;
            // `&self` methods reach `&mut` through `RefCell::borrow_mut()` (the
            // core trait's ops are `&mut self` for a single_threaded interface).
            // The loader + derive macro guarantee such an interface carries only
            // synchronous ops, so the async/stream arms below never fire here.
            let st = i.single_threaded;
            // How a handle method reaches the core to CALL an op: an ordinary
            // handle derefs its `Arc<Impl>` directly (`self.core`); a
            // single_threaded handle takes a `&mut` through the `RefCell`.
            let core_recv = if st {
                "self.core.borrow_mut()"
            } else {
                "self.core"
            };
            let mut methods: rust::Tokens = quote!();
            for op in &i.ops {
                let name = snake(&op.name);
                if op.shape != Shape::Manual {
                    if let Some(doc) = &op.doc {
                        for line in doc.lines() {
                            quote_in! { methods => $['\r']$(format!("/// {line}")) };
                        }
                    }
                }
                let params: Vec<String> = node_param_sig(api, op)
                    .iter()
                    .map(|(n, r)| format!("{n}: {r}"))
                    .collect();
                let ps = params.join(", ");
                let names = node_param_sig(api, op)
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                match op.shape {
                    Shape::Ctor => {
                        // ordinary → `Arc::new(…)`; single_threaded → `RefCell::new(…)`.
                        let wrap = if st { "RefCell::new" } else { "Arc::new" };
                        quote_in! { methods =>
                            $['\r']
                            #[napi(constructor)]
                            pub fn new($(&ps)) -> Result<Self> {
                                Ok(Self { core: $(wrap)(<$(&impl_path) as $(&trait_name)>::$(&name)($(&names)).map_err(err)?) })
                            }
                        }
                    }
                    Shape::Unary => {
                        let pin = pinned_name(&op.bindings, LANG);
                        let pin = pin.as_deref();
                        if op.result_error.is_some() {
                            // Feature 2 — the result-envelope method: return
                            // `Either<Ok, Err>` (`{ ok, value } | { ok, error }`),
                            // building the error arm from the core's `Result<T, E>`
                            // VALUE instead of throwing. Always synchronous.
                            let (ok_name, err_name) = result_envelope_names(&op.name);
                            let attr = sync_napi_attr(pin);
                            quote_in! { methods =>
                                $['\r']
                                $attr
                                pub fn $(&name)(&self, $(&ps)) -> napi::bindgen_prelude::Either<$(&ok_name), $(&err_name)> {
                                    match $(core_recv).$(&name)($(&names)) {
                                        Ok(value) => napi::bindgen_prelude::Either::A($(&ok_name) { ok: true, value }),
                                        Err(error) => napi::bindgen_prelude::Either::B($(&err_name) { ok: false, error }),
                                    }
                                }
                            }
                        } else if !op.is_async {
                            // DEFAULT: a synchronous method — no `AsyncTask`, no
                            // `Promise`. Infallible (bare-`T` core) passes the value
                            // straight through; fallible (`Result<T>` core) throws.
                            // A callback param crosses in as its `ThreadsafeFunction`
                            // (via `node_binding_param_sig`) and is bridged into the
                            // core `Box<dyn Fn>` by `wrappers` before the call; a
                            // callback-free op keeps `ps`/`names` byte-identical.
                            let (ret, _) = node_ty(api, opts, &op.returns);
                            let attr = sync_napi_attr(pin);
                            let ps = node_binding_param_sig(api, op)
                                .iter()
                                .map(|(n, r)| format!("{n}: {r}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            let (call_names, wrappers) = node_call_bridge(api, op);
                            if op.infallible {
                                quote_in! { methods =>
                                    $['\r']
                                    $attr
                                    pub fn $(&name)(&self, $(&ps)) -> $(&ret) {
                                        $(&wrappers)$(core_recv).$(&name)($(&call_names))
                                    }
                                }
                            } else {
                                quote_in! { methods =>
                                    $['\r']
                                    $attr
                                    pub fn $(&name)(&self, $(&ps)) -> Result<$(&ret)> {
                                        $(&wrappers)$(core_recv).$(&name)($(&call_names)).map_err(err)
                                    }
                                }
                            }
                        } else {
                            let task = format!("{}Task", pascal(&op.name));
                            let attr = with_js_name(
                                ts_return_attr(api, opts, &op.returns, |ts| {
                                    format!("Promise<{ts}>")
                                }),
                                pin,
                            );
                            quote_in! { methods =>
                                $['\r']
                                $attr
                                pub fn $(&name)(&self, $(&ps)) -> AsyncTask<$(&task)> {
                                    AsyncTask::new($(&task) { core: self.core.clone(), $(&names) })
                                }
                            }
                        }
                    }
                    Shape::Stream => {
                        let class = pascal(&op.name);
                        // The `closed` latch field exists only in event-mode
                        // (`@streamError`); default throw-mode streams have no latch.
                        let closed_init = if op.stream_error.is_some() {
                            "closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),"
                        } else {
                            ""
                        };
                        quote_in! { methods =>
                            $['\r']
                            $("// pre-start boundary: building the stream (setup/validation) always")
                            $("// THROWS on a core Err — independent of the stream's error model.")
                            #[napi]
                            pub fn $(&name)(&self, $(&ps)) -> Result<$(&class)> {
                                Ok($(&class) {
                                    stream: Arc::from(self.core.$(&name)($(&names)).map_err(err)?),
                                    $closed_init
                                })
                            }
                        }
                    }
                    Shape::Subscription => {
                        // Register the listener; return a `Subscription` handle wrapping
                        // the core's UNSUBSCRIBE closure. The callback param crosses in as
                        // its TSFN (bridged by `wrappers`); parts shared with python.
                        let ps = node_binding_param_sig(api, op)
                            .iter()
                            .map(|(n, r)| format!("{n}: {r}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let (call_names, wrappers) = node_call_bridge(api, op);
                        let (ret, call, ok) = subscription_method_parts(
                            op.infallible,
                            "Result",
                            &format!("{core_recv}.{name}({call_names})"),
                        );
                        quote_in! { methods =>
                            $['\r']
                            #[napi]
                            pub fn $(&name)(&self, $(&ps)) -> $(&ret) {
                                $(&wrappers)let unsub = $(&call);
                                $(&ok)
                            }
                        }
                    }
                    Shape::Manual => quote_in! { methods =>
                        $['\r']
                        $(format!("// @manual: {} — hand-written in lib.rs.", op.name))
                    },
                }
            }
            if let Some(doc) = &i.doc {
                for line in doc.lines() {
                    quote_in! { t => $['\r']$(format!("/// {line}")) };
                }
            }
            // The core-holding field: an ordinary handle shares its core across
            // AsyncTask workers via `Arc<Impl>` (⇒ `Impl: Send + Sync`); a
            // single_threaded handle owns it thread-confined in a `RefCell<Impl>`
            // (no `Arc`, no `Send`/`Sync` bound — a `!Send` core compiles).
            let core_field = if st {
                format!("RefCell<{impl_path}>")
            } else {
                format!("Arc<{impl_path}>")
            };
            quote_in! { t =>
                $['\r']
                #[napi]
                pub struct $(&i.name) {
                    $("// pub(crate): the @manual ops in lib.rs extend this class and need the core")
                    pub(crate) core: $(&core_field),
                }

                #[napi]
                impl $(&i.name) {
                    $methods
                }
                $['\n']
            };
        } else {
            // stateless interface → free functions
            for op in &i.ops {
                let name = snake(&op.name);
                if op.shape == Shape::Manual {
                    quote_in! { t => $['\r']$(format!("// @manual: {}.{} — hand-written in lib.rs.", i.name, op.name)) };
                    continue;
                }
                let params: Vec<String> = node_param_sig(api, op)
                    .iter()
                    .map(|(n, r)| format!("{n}: {r}"))
                    .collect();
                let ps = params.join(", ");
                let names = node_param_sig(api, op)
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Some(doc) = &op.doc {
                    for line in doc.lines() {
                        quote_in! { t => $['\r']$(format!("/// {line}")) };
                    }
                }
                let pin = pinned_name(&op.bindings, LANG);
                let pin = pin.as_deref();
                if op.result_error.is_some() {
                    // Feature 2 — the result-envelope free function: return
                    // `Either<Ok, Err>` (`{ ok, value } | { ok, error }`), building
                    // the error arm from the core's `Result<T, E>` VALUE instead of
                    // throwing. Always synchronous.
                    let (ok_name, err_name) = result_envelope_names(&op.name);
                    let attr = sync_napi_attr(pin);
                    let call = format!("<{impl_path} as {trait_name}>::{name}({names})");
                    quote_in! { t =>
                        $['\r']
                        $attr
                        pub fn $(&name)($(&ps)) -> napi::bindgen_prelude::Either<$(&ok_name), $(&err_name)> {
                            match $(&call) {
                                Ok(value) => napi::bindgen_prelude::Either::A($(&ok_name) { ok: true, value }),
                                Err(error) => napi::bindgen_prelude::Either::B($(&err_name) { ok: false, error }),
                            }
                        }
                        $['\n']
                    };
                } else if !op.is_async {
                    // DEFAULT: a synchronous free function — a direct call into
                    // the core trait, no `AsyncTask`/`Promise`. Infallible returns
                    // the value; fallible throws on `Err`. A callback param crosses
                    // in as its `ThreadsafeFunction` (via `node_binding_param_sig`)
                    // and is bridged into the core `Box<dyn Fn>` by `wrappers` before
                    // the call; a callback-free op keeps `ps`/`names` byte-identical.
                    let (ret, _) = node_ty(api, opts, &op.returns);
                    let attr = sync_napi_attr(pin);
                    let ps = node_binding_param_sig(api, op)
                        .iter()
                        .map(|(n, r)| format!("{n}: {r}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let (call_names, wrappers) = node_call_bridge(api, op);
                    let call = format!("<{impl_path} as {trait_name}>::{name}({call_names})");
                    if op.infallible {
                        quote_in! { t =>
                            $['\r']
                            $attr
                            pub fn $(&name)($(&ps)) -> $(&ret) {
                                $(&wrappers)$(&call)
                            }
                            $['\n']
                        };
                    } else {
                        quote_in! { t =>
                            $['\r']
                            $attr
                            pub fn $(&name)($(&ps)) -> Result<$(&ret)> {
                                $(&wrappers)$(&call).map_err(err)
                            }
                            $['\n']
                        };
                    }
                } else {
                    let task = format!("{}Task", pascal(&op.name));
                    let attr = with_js_name(
                        ts_return_attr(api, opts, &op.returns, |ts| format!("Promise<{ts}>")),
                        pin,
                    );
                    quote_in! { t =>
                        $['\r']
                        $attr
                        pub fn $(&name)($(&ps)) -> AsyncTask<$(&task)> {
                            AsyncTask::new($(&task) { $(&names) })
                        }
                        $['\n']
                    };
                }
            }
        }
    }

    let src = api.source.as_deref().unwrap_or("the fluessig catalog");
    let body = t.to_file_string().expect("rust renders");
    // external-.d.ts mode: the package's public typings are fronted by a
    // hand-written file (napi collapses literal tags to `type: string`), so mark
    // the generated file and rely on the suppressed union `ts_return_type` hints.
    let ext_note = match &opts.external_dts {
        Some(p) => format!(
            "//! external-typings: public .d.ts fronted by `{p}` — napi union typings suppressed.\n"
        ),
        None => String::new(),
    };
    // Belt-and-suspenders against any residual unused-import warning on a REDUCED
    // (non-full) surface, mirroring the php backend's banner. Emitted ONLY when
    // the prelude was trimmed: a FULL surface never carried this line, so its
    // committed golden stays byte-identical (gating it here is what keeps the
    // async/stream `node.golden` unchanged). It sits BEFORE `#![allow(clippy::all)]`
    // so that line stays the LAST prologue attribute — the fan-out import splice
    // ([`splice_imports`]) inserts `use crate::…` right after it, which must land
    // after every inner attribute or rustc rejects the module.
    let unused = if needs.is_full() {
        String::new()
    } else {
        "#![allow(unused_imports)]\n".to_string()
    };
    crate::rustfmt::format(format!(
        "//! GENERATED by fluessig bindgen from {src} (api layer). Do not edit.\n{}{}{}#![allow(clippy::all)]\n\n{body}",
        note_line(banner_note),
        ext_note,
        unused
    ))
}
