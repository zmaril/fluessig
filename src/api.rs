//! The op-layer IR — serde mirror of `api.json` (format 0): interfaces, ops with
//! shapes, params, returns, and the DTO models the ops reference. The input to
//! [`crate::bindgen`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use crate::ir::SymbolBinding;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiDoc {
    pub fluessig: crate::ir::Versions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub models: Vec<ApiModel>,
    /// Named tagged unions the op surface references (format 1). On the FFI a
    /// union value crosses as its JSON envelope `{"kind": tag, "payload": body}`
    /// — the same carrier as the `Json` scalar; typed surfaces come from the
    /// per-language docs and (for MCP) the generated `oneOf` schemas.
    #[serde(default)]
    pub unions: Vec<ApiUnion>,
    pub interfaces: Vec<ApiInterface>,
    /// Top-level EXPORTED CONSTANTS the surface declares (format 1+), e.g. a
    /// module's `export const VERSION: string = "…"`. A const has no home in the
    /// op layer (it is neither an op nor a DTO field), so it rides here as its own
    /// document section. Empty in every pre-const fixture — `#[serde(default)]`
    /// makes an api.json WITHOUT this key parse byte-for-byte as before, and the
    /// empty-Vec skip keeps a re-serialized doc identical too. Backends that don't
    /// model consts simply ignore it; rust-core lowers each to a `pub const`
    /// (const-representable literals) or a "runtime value" doc-comment note.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consts: Vec<ApiConst>,
}

/// A TOP-LEVEL exported constant (see [`ApiDoc::consts`]). Reuses the shared
/// [`ApiType`] for its declared type, so a `string` const carries `type:
/// "string"` exactly as an op param/return/field does — no new type vocabulary.
/// `value` is the STATICALLY-KNOWN literal, when one exists; a const whose source
/// is a runtime expression (`pkg.version || "0.0.0"`) or a non-literal type
/// carries `value: None` and is emitted as a documented non-representable note
/// rather than a broken `pub const`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiConst {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(rename = "type")]
    pub ty: ApiType,
    /// The compile-time literal, or `None` when the const has no statically-known
    /// value (a runtime expression, or a non-literal type). Absent ⇒ `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ConstValue>,
}

/// The literal a [`ApiConst`] carries. UNTAGGED, so it serializes as the bare
/// JSON scalar the value naturally is — a string const's value is `"0.80.10"`, an
/// int's is `42`, a bool's is `true`, a float's is `3.14` — matching how the
/// source literal reads. On deserialize the arms are tried in order (bool, then
/// integer, then float, then string), so an integer JSON number lands as `Int`
/// and a fractional one as `Float`; the const's declared `type` is the authority
/// for lowering, this is only the value carrier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConstValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiUnion {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Per-union discriminant field name for structured projection (format 1+).
    /// Absent in existing fixtures — `None` falls back to the backend-global tag
    /// field, reproducing prior behavior byte-for-byte.
    #[serde(default)]
    pub tag_field: Option<String>,
    pub variants: Vec<ApiUnionVariant>,
    /// Per-language export-name / package / module pins for this union symbol
    /// (see [`SymbolBinding`]). Empty ⇒ every backend's default rule.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, SymbolBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiUnionVariant {
    pub tag: String,
    #[serde(rename = "type")]
    pub ty: ApiType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiModel {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub fields: Vec<ApiField>,
    /// Per-language export-name / package / module pins for this model symbol
    /// (see [`SymbolBinding`]). Empty ⇒ every backend's default rule.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, SymbolBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ApiType,
    pub nullable: bool,
    /// Per-language export-name pins for this field (see [`SymbolBinding`]).
    /// `bindings["node"].name` ⇒ `#[napi(js_name = "…")]`, `bindings["python"]`
    /// ⇒ `#[pyo3(name = "…")]`, `bindings["php"]` ⇒ the ext-php-rs
    /// `#[rename("…")]`, etc. — each backend overrides ONLY its own casing rule.
    /// Empty ⇒ default behaviour, byte-identical to before this slot existed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, SymbolBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiInterface {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// `#[fluessig(single_threaded)]` on the exported `impl` — the interface
    /// lowers to a THREAD-CONFINED handle class (node-only today). Its generated
    /// napi handle holds the core by plain ownership inside a `RefCell` WITHOUT
    /// `Arc`/`Send`/`Sync`, so a `!Send` core (`Rc<RefCell<…>>` + non-Send
    /// closures, e.g. pidgin's `TuiCore`) can be GENERATED — a napi class instance
    /// never crosses threads, so it needs no `Send` bound. The trade: a
    /// single_threaded interface may carry ONLY synchronous ops (an async/stream
    /// op needs a `Send` core for the threadpool), enforced by the derive macro
    /// (a spanned compile error) and re-checked here by [`load_api`]. Non-node
    /// backends cannot express a thread-confined handle, so they emit an honest
    /// skip-note rather than a silently `Send`-assuming handle. Serialized ONLY
    /// when `true`, so an ordinary (async-capable) interface stays byte-identical.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub single_threaded: bool,
    pub ops: Vec<ApiOp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiOp {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub shape: Shape,
    /// The op's ASYNC marker — the ONE place async-ness is decided, meaning the
    /// same thing everywhere. Synchronous is the GLOBAL default across every
    /// backend: an op with no `#[fluessig(async)]` marker (`is_async = false`,
    /// the field ABSENT) generates a plain, value-returning binding.
    /// `#[fluessig(async)]` (⇒ `"async": true`) opts an op INTO the async
    /// projection (the historical `AsyncTask`/`Promise`/coroutine shape).
    /// Serialized ONLY when `true`. Only meaningful on [`Shape::Unary`] (streams
    /// are always async-iterable, ctors are synchronous constructors).
    #[serde(rename = "async", default, skip_serializing_if = "std::ops::Not::not")]
    pub is_async: bool,
    /// The op's Rust return type is a bare `T` (not `Result<T>`), so a SYNCHRONOUS op
    /// carries NO error channel: node emits `-> T` (no `napi::Result`, no `.map_err`),
    /// python drops its `PyResult`/raise, php its `PhpResult`, ruby its `Result<_, Error>`,
    /// and the shared core-trait method is `fn name(..) -> T`. Only ever `true` when the op
    /// resolves synchronous — an async op always crosses the `Result` seam (a rejected
    /// `Promise`) — so it is meaningless on an async op and defaults `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub infallible: bool,
    /// `@readonly` — flows into the MCP `readOnlyHint` annotation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub readonly: bool,
    /// `@destructive` — flows into the MCP `destructiveHint` annotation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub destructive: bool,
    /// `@worker` — flows into the MCP `workerHint` annotation (marks an op safe for
    /// a worker-role MCP surface).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worker: bool,
    /// `@streamError(...)` — opts a stream op INTO the error-as-EVENT model and
    /// (optionally) shapes that event, for the node backend. This field drives the
    /// MODE, not just the shape: `None` (unannotated) → the DEFAULT idiomatic
    /// native-TS model, where a core failure after stream start REJECTS the pull
    /// (the `for await` loop throws — no silent-swallow); `Some(shape)` → the core
    /// failure is yielded as a terminal error EVENT and the stream completes
    /// (mirror-a-library mode, e.g. pi's `{ type: "error", reason, error }`). A bare
    /// `@streamError` lowers to `Some(StreamErrorShape::default())` = pi's shape
    /// verbatim; args override individual js-names / the tag value. Loader-checked
    /// to be legal only on [`Shape::Stream`] (see [`load_api`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_error: Option<StreamErrorShape>,
    /// `#[fluessig(result)]` — opts a SYNCHRONOUS unary op INTO the node
    /// result-envelope projection: instead of throwing on `Err`, the binding
    /// returns a discriminated `{ ok: true, value: T } | { ok: false, error: E }`
    /// object, with the error handed back AS A VALUE. `Some(name)` carries the
    /// explicit error RECORD type `E` (a `#[derive(Record)]`, e.g. `FileError`),
    /// which the op's Rust return type spells as `Result<T, E>`; the core-trait
    /// method then returns `Result<T, E>` (not `anyhow::Result<T>`) so the binding
    /// can construct the error arm. Node-only today; other backends treat the op
    /// as an ordinary fallible op (throw/raise). `None` ⇒ the default (throw).
    /// Serialized only when set, so an unmarked op is byte-identical to before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_error: Option<String>,
    pub params: Vec<ApiParam>,
    pub returns: ApiType,
    /// Per-language export-name / package / module pins for this op symbol (see
    /// [`SymbolBinding`]). Empty ⇒ every backend's default rule.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, SymbolBinding>,
}

/// The JS shape of a stream op's terminal error event (event-mode only, i.e. when
/// `stream_error` is `Some`). Every field defaults to pi's post-start error shape
/// (`{ type: "error", reason, error }`) verbatim, so a bare `@streamError` and an
/// empty `{}` annotation lower identically; a schema author overrides only what
/// they need. Field NAMES are js-names on the emitted `#[napi(object)]` struct;
/// `tag_value` is the value stamped into the tag field.
#[derive(Debug, Clone, Serialize, Deserialize)]
// container `default`: any field the author omits falls back to `Default` (pi's
// shape below), so a partial `{ "tag_value": … }` fills the rest verbatim.
#[serde(deny_unknown_fields, default)]
pub struct StreamErrorShape {
    /// JS field name of the discriminator tag (pi: `type`).
    pub tag_name: String,
    /// Value stamped into the discriminator tag (pi: `error`).
    pub tag_value: String,
    /// JS field name carrying the coarse reason (pi: `reason`).
    pub reason_name: String,
    /// JS field name carrying the core error message (pi renames `message`→`error`).
    pub error_name: String,
}

impl Default for StreamErrorShape {
    fn default() -> Self {
        Self {
            tag_name: "type".into(),
            tag_value: "error".into(),
            reason_name: "reason".into(),
            error_name: "error".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Ctor,
    Unary,
    Stream,
    /// A register/unsubscribe op (a cousin of [`Shape::Stream`]): it takes exactly
    /// one [`ApiType::Callback`] param, REGISTERS that listener, and returns a
    /// generated `Subscription` HANDLE whose `unsubscribe()`/drop removes the
    /// listener. Maps to pi's `onEvent`/`onExit` `(listener) => () => void`. The
    /// core-trait method returns the UNSUBSCRIBE closure (`Box<dyn Fn() + Send +
    /// Sync>`); each backend's generated binding wraps that closure into its
    /// `Subscription` handle class. Because a Subscription method is `&self`, the
    /// interface must be stateful (carry a [`Shape::Ctor`]) — enforced by
    /// [`load_api`]. node + python lower it fully today; the other backends emit a
    /// skip-note (deferred to follow-up PRs).
    Subscription,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiParam {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ApiType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

/// A type in the op surface: a scalar name (or `"void"`), a model/enum
/// reference, or a list thereof.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ApiType {
    Scalar(String),
    Model {
        model: String,
    },
    Enum {
        r#enum: String,
    },
    List {
        list: Box<ApiType>,
    },
    /// `T | null` — nullable returns/params.
    Nullable {
        nullable: Box<ApiType>,
    },
    /// A named tagged union (see [`ApiDoc::unions`]).
    Union {
        union: String,
    },
    /// A TRULY-FOREIGN type — an external/host value the schema references but
    /// fluessig has no model for (Node's `http.Server`, a `ChildProcess`, an OS
    /// file descriptor, …). Rather than silently collapsing it to a `String`/JSON
    /// carrier, it lowers to a generated, per-type OPAQUE HANDLE the boundary can
    /// carry without needing the real external type in scope. `name` is the source
    /// type name (e.g. `http.Server`); `rust_path` is a best-effort Rust path/label
    /// for the handle (used as documentation, not required to resolve). Serializes
    /// as `{"foreign": {"name": "…", "rustPath": "…"}}`, mirroring the single
    /// distinguishing-key convention of the sibling variants (`model`, `enum`,
    /// `list`, `nullable`, `union`).
    Foreign {
        foreign: ForeignType,
    },
    /// A host-supplied callback: `fn(params...) -> returns`. Forward-only sync-void
    /// today (the only shape any backend lowers); `is_async`/`fallible` are
    /// reserved on [`CallbackSig`] for later. Untagged variant keyed on
    /// `"callback"`. The Rust core sees ONE uniform shape regardless of the source
    /// language: `Box<dyn Fn(args...) + Send + Sync>` (see [`crate::bindgen`]'s
    /// shared `ty`); each backend's generated binding wraps its native callable
    /// into that box at the FFI boundary.
    Callback {
        callback: CallbackSig,
    },
}

/// The payload of an [`ApiType::Foreign`]: the source type `name` and a
/// best-effort `rust_path` label for the generated opaque handle. Kept as a
/// dedicated struct so the variant reads as a single `{"foreign": {…}}` key,
/// matching how the other `ApiType` variants each carry exactly one tag word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForeignType {
    /// The source-language type name, e.g. `http.Server`, `ChildProcess`.
    pub name: String,
    /// A best-effort Rust path/label for the opaque handle (documentation only;
    /// the handle type name is derived deterministically from `name`).
    pub rust_path: String,
}

/// The signature of an [`ApiType::Callback`]. Additive optional fields mirror the
/// house style (`skip_serializing_if`), so a plain forward-only sync-void callback
/// serializes to just `{"callback":{"params":[…]}}` and existing goldens are
/// untouched.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallbackSig {
    pub params: Vec<ApiType>,
    #[serde(
        default = "callback_void_return",
        skip_serializing_if = "is_void_return"
    )]
    pub returns: Box<ApiType>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_async: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fallible: bool,
}

/// The default `returns` for a callback: `void` (the only return any backend
/// lowers this slice).
fn callback_void_return() -> Box<ApiType> {
    Box::new(ApiType::Scalar("void".into()))
}

/// Is a callback `returns` the `void` scalar? Drives `skip_serializing_if` so a
/// sync-void callback omits the field. serde requires the exact `&Box<ApiType>`
/// predicate signature, hence the clippy allow.
#[allow(clippy::borrowed_box)]
fn is_void_return(t: &Box<ApiType>) -> bool {
    matches!(t.as_ref(), ApiType::Scalar(s) if s == "void")
}

/// The set of interface NAMES whose instances something in the API can hand back —
/// either the interface carries its own [`Shape::Ctor`] op (a public constructor)
/// or some op ANYWHERE in the API RETURNS that interface type (a FACTORY op). Such
/// an interface is CONSTRUCTIBLE: a caller can obtain a live instance, so a
/// `Shape::Subscription`/method op (whose generated method is `&self`) has a real,
/// stateful receiver to hang itself on — even when the instance can only ever be
/// HANDED back by a factory, never `new`-ed directly (pi's `RpcProcessInstance`,
/// returned by `openRpcStream`, is exactly this case).
///
/// Unwrapping rule for the factory branch: a return type is followed through the
/// TRANSPARENT container types [`ApiType::Nullable`] and [`ApiType::List`] (a `T?`
/// or `T[]` of an interface still MINTS instances of that interface) down to the
/// innermost named type; if that is an [`ApiType::Model`] whose `model` names an
/// interface, that interface is constructible. Interfaces are referenced in type
/// position as `ApiType::Model { model }`, sharing the name namespace with DTO
/// models — a `Model` naming a plain DTO simply is not in the interface set and is
/// ignored. Opaque/union/foreign/scalar/enum/callback returns never name an
/// interface. The factory op may live on ANY interface, including the target's own.
fn constructible_interfaces(api: &ApiDoc) -> std::collections::BTreeSet<String> {
    let iface_names: std::collections::BTreeSet<&str> =
        api.interfaces.iter().map(|i| i.name.as_str()).collect();
    let mut set = std::collections::BTreeSet::new();
    for i in &api.interfaces {
        // 1. interfaces with their own ctor op (public constructor).
        if i.ops.iter().any(|o| o.shape == Shape::Ctor) {
            set.insert(i.name.clone());
        }
        // 2. interfaces returned by SOME op anywhere (factory-born).
        for op in &i.ops {
            if let Some(name) = returned_interface_name(&op.returns, &iface_names) {
                set.insert(name);
            }
        }
    }
    set
}

/// The interface NAME an op-return type ultimately names, if any: unwrap the
/// transparent containers [`ApiType::Nullable`] and [`ApiType::List`], then match
/// the innermost [`ApiType::Model`] against the known interface names. `None` for a
/// scalar/enum/union/foreign/callback return, or a `Model` naming a DTO rather than
/// an interface. See [`constructible_interfaces`] for the rationale.
fn returned_interface_name(
    ty: &ApiType,
    ifaces: &std::collections::BTreeSet<&str>,
) -> Option<String> {
    match ty {
        ApiType::Nullable { nullable } => returned_interface_name(nullable, ifaces),
        ApiType::List { list } => returned_interface_name(list, ifaces),
        ApiType::Model { model } if ifaces.contains(model.as_str()) => Some(model.clone()),
        _ => None,
    }
}

/// Parse `api.json` (with the same format-version gate as the catalog).
pub fn load_api(json: &str) -> Result<ApiDoc, String> {
    let api: ApiDoc =
        serde_json::from_str(json).map_err(|e| format!("api.json parse error: {e}"))?;
    if api.fluessig.format != crate::FORMAT_VERSION {
        return Err(format!(
            "api format {} is not supported (this fluessig reads format {})",
            api.fluessig.format,
            crate::FORMAT_VERSION
        ));
    }
    // The set of interfaces something can hand a live instance of — either they
    // carry their own ctor or a FACTORY op somewhere returns them. A
    // `Shape::Subscription` op's `&self` method needs such a receiver; computed
    // once so the per-op check below is a cheap membership test.
    let constructible = constructible_interfaces(&api);
    // the loader validates: a `@streamError` shape is meaningless off the stream
    // shape (nothing else has a post-start boundary to encode an error into).
    for i in &api.interfaces {
        for op in &i.ops {
            if op.stream_error.is_some() && op.shape != Shape::Stream {
                return Err(format!(
                    "op `{}.{}`: stream_error (@streamError) is only valid on a stream op, but its shape is {:?}",
                    i.name, op.name, op.shape
                ));
            }
            // a `single_threaded` interface lowers to a THREAD-CONFINED handle
            // holding a `!Send` core — which is incompatible with the async
            // projection (an async/stream op clones the core onto a threadpool
            // worker, so the core MUST be `Send`). The derive macro rejects this
            // at authoring time with a spanned compile error; this re-checks the
            // hand-written / lowered `api.json` path so a bad surface can never
            // reach a backend. Keep the message aligned with the macro's.
            if i.single_threaded && (op.is_async || op.shape == Shape::Stream) {
                return Err(format!(
                    "op `{}.{}`: a #[fluessig(single_threaded)] interface may carry only \
                     synchronous ops — an async or stream op needs a `Send` core for the \
                     threadpool, which is incompatible with a thread-confined `!Send` handle",
                    i.name, op.name
                ));
            }
            // A `Shape::Subscription` op REGISTERS a listener and returns a
            // `Subscription` handle whose drop/unsubscribe removes it. It must take
            // exactly ONE callback param (the listener), and — since its method is
            // `&self` — the interface must be CONSTRUCTIBLE: either it carries its
            // own `Shape::Ctor`, or a FACTORY op somewhere returns it (pi's
            // `RpcProcessInstance`, handed back by `openRpcStream`, is the latter).
            // An interface that NOTHING constructs — no ctor and no factory return
            // anywhere — is still rejected: there is no instance to hang `&self` on.
            if op.shape == Shape::Subscription {
                let callback_params = op
                    .params
                    .iter()
                    .filter(|p| matches!(&p.ty, ApiType::Callback { .. }))
                    .count();
                if callback_params != 1 {
                    return Err(format!(
                        "subscription op `{}.{}` must have exactly one callback param",
                        i.name, op.name
                    ));
                }
                if !constructible.contains(&i.name) {
                    return Err(format!(
                        "subscription op `{}.{}` requires a constructible interface (its method \
                         is `&self`), but nothing constructs `{}`: it has no ctor op and no \
                         factory op (an op returning `{}`, optionally wrapped in nullable/list) \
                         anywhere in the API",
                        i.name, op.name, i.name, i.name
                    ));
                }
            }
            // This slice lowers ONLY forward-only sync-void callbacks. Reject any
            // callback param whose `is_async`/`fallible`/non-void `returns` the
            // backends do not yet wrap, so the IR stays honest about what compiles.
            for p in &op.params {
                if let ApiType::Callback { callback } = &p.ty {
                    if callback.is_async || callback.fallible || !is_void_return(&callback.returns)
                    {
                        return Err(format!(
                            "callback param `{}` on op `{}.{}`: only forward-only sync void callbacks are supported (is_async/fallible/non-void returns not yet implemented)",
                            p.name, i.name, op.name
                        ));
                    }
                }
            }
        }
    }
    Ok(api)
}

/// [`load_api`] from a file path.
pub fn load_api_file(path: impl AsRef<std::path::Path>) -> Result<ApiDoc, String> {
    let json = std::fs::read_to_string(path.as_ref())
        .map_err(|e| format!("read {}: {e}", path.as_ref().display()))?;
    load_api(&json)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks the on-wire shape of [`ApiType::Foreign`] so the converter that
    /// emits it (and the sibling variants' single-distinguishing-key convention)
    /// stay in agreement: `{"foreign": {"name": …, "rustPath": …}}`, with
    /// `rust_path` camelCased to `rustPath`. Round-trips byte-for-byte.
    #[test]
    fn foreign_serializes_as_single_foreign_key() {
        let ty = ApiType::Foreign {
            foreign: ForeignType {
                name: "http.Server".into(),
                rust_path: "http::Server".into(),
            },
        };
        let json = serde_json::to_string(&ty).unwrap();
        assert_eq!(
            json,
            r#"{"foreign":{"name":"http.Server","rustPath":"http::Server"}}"#
        );
        // Deserializes back to the same variant (untagged, distinguished by the
        // `foreign` key — no collision with model/enum/list/nullable/union).
        let back: ApiType = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(back, ApiType::Foreign { foreign } if foreign.name == "http.Server"
            && foreign.rust_path == "http::Server")
        );
    }

    /// Locks the on-wire shape of an [`ApiConst`] so the converter that emits it
    /// stays in agreement: a string const is
    /// `{"name":"VERSION","type":"string","value":"0.80.10"}` — the scalar `type`
    /// rides as the bare string (identical to a param/field type), and the
    /// untagged `value` rides as the bare JSON string. Round-trips byte-for-byte.
    #[test]
    fn const_string_serializes_as_bare_scalar_and_value() {
        let c = ApiConst {
            name: "VERSION".into(),
            doc: None,
            ty: ApiType::Scalar("string".into()),
            value: Some(ConstValue::Str("0.80.10".into())),
        };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(
            json,
            r#"{"name":"VERSION","type":"string","value":"0.80.10"}"#
        );
        let back: ApiConst = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "VERSION");
        assert!(matches!(back.value, Some(ConstValue::Str(ref s)) if s == "0.80.10"));
    }

    /// The untagged [`ConstValue`] carries int / float / bool as their bare JSON
    /// forms, and an integer JSON number lands on `Int` (not `Float`).
    #[test]
    fn const_value_untagged_scalar_forms() {
        let cases = [
            (ConstValue::Int(42), "42"),
            (ConstValue::Bool(true), "true"),
            (ConstValue::Float(3.5), "3.5"),
        ];
        for (v, wire) in cases {
            assert_eq!(serde_json::to_string(&v).unwrap(), wire);
        }
        assert!(matches!(
            serde_json::from_str::<ConstValue>("42").unwrap(),
            ConstValue::Int(42)
        ));
        assert!(matches!(
            serde_json::from_str::<ConstValue>("3.5").unwrap(),
            ConstValue::Float(f) if (f - 3.5).abs() < f64::EPSILON
        ));
    }

    /// Build a two-part api.json for the factory-born subscription tests: a
    /// ctor-less `Session` carrying an `on_event` subscription op, optionally
    /// preceded by an `Orchestrator` whose `open` op RETURNS `factory_return` (the
    /// interface-typed return spelling — e.g. `{"model": "Session"}` or
    /// `{"nullable": {"model": "Session"}}`). `None` omits the factory entirely, so
    /// nothing constructs `Session`. Centralizing the doc keeps the three tests from
    /// each spelling out a near-identical literal.
    fn subscription_api(factory_return: Option<&str>) -> String {
        let orchestrator = match factory_return {
            Some(ret) => format!(
                r#"{{"name": "Orchestrator", "ops": [
                  {{"name": "new", "shape": "ctor", "params": [], "returns": "void"}},
                  {{"name": "open", "shape": "unary", "params": [], "returns": {ret}}}
                ]}},"#
            ),
            None => String::new(),
        };
        format!(
            r#"{{
              "fluessig": {{"format": 1}},
              "models": [], "unions": [],
              "interfaces": [
                {orchestrator}
                {{"name": "Session", "ops": [
                  {{"name": "on_event", "shape": "subscription", "params": [
                    {{"name": "listener", "type": {{"callback": {{"params": ["int32"]}}}}}}
                  ], "returns": "void"}}
                ]}}
              ]
            }}"#
        )
    }

    /// A `Shape::Subscription` op on a FACTORY-BORN interface — one with NO ctor of
    /// its own, whose instances are handed back by another op — now loads. Mirrors
    /// pi's `RpcProcessInstance`: `Orchestrator.open` returns it, so `Session`'s
    /// `on_event` subscription has a real `&self` receiver even with no public
    /// constructor.
    #[test]
    fn subscription_on_factory_born_interface_loads() {
        let json = subscription_api(Some(r#"{"model": "Session"}"#));
        assert!(
            load_api(&json).is_ok(),
            "a subscription op on a factory-born (ctor-less but returned) interface loads"
        );
    }

    /// The factory return is followed through the transparent `Nullable`/`List`
    /// containers: pi's `openRpcStream` returns `RpcProcessInstance | undefined`
    /// (nullable), which still MINTS instances, so the target is constructible.
    #[test]
    fn subscription_factory_return_unwraps_nullable() {
        let json = subscription_api(Some(r#"{"nullable": {"model": "Session"}}"#));
        assert!(
            load_api(&json).is_ok(),
            "a factory op returning `Session?` (nullable) makes Session constructible"
        );
    }

    /// A `Shape::Subscription` op on an interface that NOTHING constructs — no ctor
    /// AND no op anywhere returns it — is STILL rejected: there is no live instance
    /// to hang the `&self` method on. Keeps the relaxed check honest.
    #[test]
    fn subscription_on_unconstructible_interface_is_rejected() {
        let json = subscription_api(None);
        let err =
            load_api(&json).expect_err("subscription on an unconstructible interface rejects");
        assert!(
            err.contains("requires a constructible interface")
                && err.contains("nothing constructs"),
            "clear unconstructible-interface error, got: {err}"
        );
    }

    /// A `consts` key absent from api.json parses as an empty vec (backward-compat)
    /// and re-serializes WITHOUT the key (empty-Vec skip) — no drift for any
    /// pre-const fixture.
    #[test]
    fn missing_consts_is_empty_and_skips_on_serialize() {
        let json = r#"{
          "fluessig": {"format": 1, "emitter": "t", "compiler": "t"},
          "models": [], "unions": [],
          "interfaces": [{"name": "Api", "ops": []}]
        }"#;
        let api = load_api(json).unwrap();
        assert!(api.consts.is_empty());
        assert!(!serde_json::to_string(&api).unwrap().contains("consts"));
    }
}
