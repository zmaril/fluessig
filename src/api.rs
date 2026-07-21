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
