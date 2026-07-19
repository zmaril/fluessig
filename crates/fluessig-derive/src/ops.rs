//! The op surface (`api.json`) — Slice 5's `#[fluessig::export]` descriptor
//! vocabulary + the api-layer lowering, split out of the crate root to keep it
//! under the file-size budget. Re-exported at the root so macro-generated code
//! names these as `::fluessig_derive::InterfaceDescriptor` etc.

use super::*;

// ═════════════════════════════════════════════════════════════════════════════
// Slice 5 — the op surface (`api.json`)
//
// `derive-front-end.md` §2.7: **the impl that actually runs IS the interface**.
// `#[fluessig::export]` on an `impl` block captures each method's shape (name,
// params, return, op kind) into an [`InterfaceDescriptor`] — pure `&'static`
// data, exactly like [`EntityDescriptor`] — and `catalog!`'s `api:` root list
// lowers those descriptors into the same `api.json` the loader + bindgen already
// consume, so declaration/implementation drift is impossible.
// ═════════════════════════════════════════════════════════════════════════════

/// A type (or unit-struct "namespace") whose `#[fluessig::export] impl` block
/// was captured into an op interface. `#[fluessig::export]` expands to an
/// `impl ApiExport for Self` carrying the `&'static InterfaceDescriptor`, so
/// `catalog!`'s `api:` list can reach it as `<T as ApiExport>::DESCRIPTOR` —
/// the op-surface twin of [`Entity`]/[`Edge`].
pub trait ApiExport {
    /// The descriptor the `#[fluessig::export]` macro expands to.
    const DESCRIPTOR: &'static InterfaceDescriptor;
}

/// One op interface — the `#[fluessig::export] impl <Name>` block: its name (the
/// `Self` type), the impl block's `///` doc, and the ops it exposes, in
/// declaration order. Lowers to one `api.json` `ApiInterface`.
#[derive(Debug, Clone, Copy)]
pub struct InterfaceDescriptor {
    /// The interface name — the `Self` type of the exported impl (`"Entl"`).
    pub name: &'static str,
    /// The impl block's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The captured ops, in declaration order.
    pub ops: &'static [OpDescriptor],
    /// The `.rs` source location of the exported `impl` block (Slice 6).
    pub span: SourceSpan,
}

/// One captured method: its Rust (snake_case) name, doc, op kind, params, and
/// return type. The name/param names are camelCased at lowering to match the
/// `api.json` op-surface convention (the TypeSpec `interface` path spells them
/// lowerCamel too).
#[derive(Debug, Clone, Copy)]
pub struct OpDescriptor {
    /// The Rust method name (snake_case); camelCased at lowering.
    pub name: &'static str,
    /// The method's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The op kind — `ctor` / plain unary / `stream` / `manual`.
    pub kind: OpKind,
    /// `#[fluessig(sync)]` — a synchronous unary op (the node backend emits a
    /// plain `#[napi] fn -> T` instead of an `AsyncTask` → `Promise<T>`). A FLAG
    /// composing with `Unary` only; the macro rejects it on any other kind.
    pub sync: bool,
    /// Whether the method's Rust return type is `Result<T>` (fallible) vs a bare
    /// `T` (infallible). Composed with `sync` at lowering into the op's
    /// `infallible` bit: a `sync` op returning a bare `T` gets an infallible
    /// node seam (`-> T`), one returning `Result<T>` keeps `-> napi::Result<T>`.
    /// For an async op this is unused (every async op crosses the `Result` seam).
    pub fallible: bool,
    /// `#[fluessig(name = "…")]` — an explicit export-name pin for this op,
    /// lowered onto `ApiOp.bindings` so the node backend emits
    /// `#[napi(js_name = "…")]`. `None` ⇒ each backend's default casing.
    pub name_pin: Option<&'static str>,
    /// `#[fluessig(readonly)]` — an observe-only op; lowers to `api.json`
    /// `"readonly": true` and the MCP `readOnlyHint`. A FLAG composing with `kind`
    /// (a readonly op is still unary/stream), not a kind of its own.
    pub readonly: bool,
    /// `#[fluessig(destructive)]` — an irreversible op (`cancel` / `reap`); lowers
    /// to `api.json` `"destructive": true` and the MCP `destructiveHint`. Also a
    /// flag composing with `kind`.
    pub destructive: bool,
    /// The method params (receiver excluded), in declaration order.
    pub params: &'static [ParamDescriptor],
    /// The return type as an op-surface type. A `ctor` is always `void`; a
    /// `stream` carries its iterator's `Item` type (the per-batch type); a
    /// `Result<T>` wrapper is transparent (unwrapped to `T`).
    pub returns: ApiTypeDesc,
    /// The `.rs` source location of this method's declaration (Slice 6).
    pub span: SourceSpan,
}

/// One op param: its Rust (snake_case) name (camelCased at lowering), its
/// op-surface type, and whether it is optional (an `Option<T>` param lowers to
/// `optional: true` carrying the *unwrapped* `T` — params use `optional`,
/// returns use `nullable`, mirroring the TypeSpec op path).
#[derive(Debug, Clone, Copy)]
pub struct ParamDescriptor {
    /// The Rust param name (snake_case); camelCased at lowering.
    pub name: &'static str,
    /// The param's op-surface type.
    pub ty: ApiTypeDesc,
    /// `Option<T>` param ⇒ `true`.
    pub optional: bool,
    /// The `.rs` source location of this param's declaration (Slice 6).
    pub span: SourceSpan,
}

/// The four op kinds (`derive-front-end.md` §2.7). Mirrors [`fluessig::api::Shape`];
/// kept as its own front-end enum so the descriptor layer doesn't depend on the
/// loader's serde types at the capture site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// `#[fluessig(ctor)]` — a constructor. Returns `void` on the op surface.
    Ctor,
    /// An untagged method — a plain unary op (the default).
    Unary,
    /// `#[fluessig(stream)]` — returns an iterator/stream; bindgen maps it to a
    /// JS async iterator / Python generator / Ruby Enumerator.
    Stream,
    /// `#[fluessig(manual)]` — recorded in `api.json` but hand-written per
    /// binding (not auto-bound).
    Manual,
}

/// An op-surface type as pure `&'static` data — the front-end twin of
/// [`fluessig::api::ApiType`], recursive through `&'static` so it lives in a
/// `const`. Lowered to `ApiType` by [`lower_api_type`].
#[derive(Debug, Clone, Copy)]
pub enum ApiTypeDesc {
    /// A scalar name (`"string"`, `"int64"`, `"boolean"`, `"bytes"`, `"void"`, …).
    Scalar(&'static str),
    /// A model/DTO or entity reference (`{ "model": name }`).
    Model(&'static str),
    /// An enum reference (`{ "enum": name }`).
    Enum(&'static str),
    /// A list of the inner type (`{ "list": inner }`).
    List(&'static ApiTypeDesc),
    /// A nullable inner type (`{ "nullable": inner }`) — an `Option<T>` return.
    Nullable(&'static ApiTypeDesc),
}

/// Lower an [`ApiTypeDesc`] to the loader's [`fluessig::api::ApiType`], resolving a
/// bare [`ApiTypeDesc::Model`] against the catalog's declared types via `resolver`.
/// The `#[fluessig::export]` macro can't tell a semantic-scalar/enum/union-typed
/// op param from a model by the token alone (`uid: SessionUid` looks like a model),
/// so it emits `Model(name)` and the classification happens HERE — the catalog
/// cross-check the macro's own comment defers to lowering. A declared scalar
/// (`SessionUid`) → its scalar name; a declared enum → `{ enum }`; a declared union
/// → `{ union }`; anything else stays a `{ model }` ref. Surfaced by the disponent
/// acid test (entl's ops take `&str` / DTO models — never a semantic scalar).
fn lower_api_type(t: &ApiTypeDesc, resolver: &RefResolver) -> ApiType {
    match t {
        ApiTypeDesc::Scalar(s) => ApiType::Scalar((*s).to_string()),
        ApiTypeDesc::Model(m) => resolver.api_named(m),
        ApiTypeDesc::Enum(e) => ApiType::Enum {
            r#enum: (*e).to_string(),
        },
        ApiTypeDesc::List(inner) => ApiType::List {
            list: Box::new(lower_api_type(inner, resolver)),
        },
        ApiTypeDesc::Nullable(inner) => ApiType::Nullable {
            nullable: Box::new(lower_api_type(inner, resolver)),
        },
    }
}

/// Lower one captured op to an [`fluessig::api::ApiOp`]. The name + param names
/// camelCase to the op-surface convention; `readonly`/`destructive` ride the
/// descriptor's flags (the `#[fluessig(readonly)]` / `#[fluessig(destructive)]`
/// op attributes); `stream_error` stays unset (a node-backend concern, not part of
/// the derive authoring surface). Op param/return types are resolved against the
/// catalog via `resolver` (a semantic-scalar param lands as a scalar, not a model).
fn lower_op(op: &OpDescriptor, resolver: &RefResolver) -> ApiOp {
    ApiOp {
        name: camel(op.name),
        doc: op.doc.map(str::to_string),
        shape: match op.kind {
            OpKind::Ctor => Shape::Ctor,
            OpKind::Unary => Shape::Unary,
            OpKind::Stream => Shape::Stream,
            OpKind::Manual => Shape::Manual,
        },
        // `sync` gates the node backend's synchronous projection; `infallible`
        // (only true alongside `sync`, when the Rust return is a bare `T`) drops
        // the `Result` seam from both the node emission and the shared core trait.
        sync: op.sync,
        infallible: op.sync && !op.fallible,
        readonly: op.readonly,
        destructive: op.destructive,
        stream_error: None,
        params: op
            .params
            .iter()
            .map(|p| ApiParam {
                name: camel(p.name),
                ty: lower_api_type(&p.ty, resolver),
                optional: p.optional.then_some(true),
            })
            .collect(),
        returns: lower_api_type(&op.returns, resolver),
        // An op-level `#[fluessig(name = "…")]` pins the exported symbol name
        // across the surface; each backend applies it through its own rename
        // (node ⇒ `#[napi(js_name = "…")]`). An unpinned op keeps an empty map,
        // byte-identical to before this authoring path existed.
        bindings: op_name_bindings(op.name_pin),
    }
}

/// The `bindings` map for an op's `#[fluessig(name = "…")]` pin: the exact
/// export name under every backend's language slug (each backend reads its own
/// key via [`crate::bindgen::pinned_name`]). `None` ⇒ an empty map (default
/// casing everywhere).
fn op_name_bindings(name_pin: Option<&str>) -> std::collections::BTreeMap<String, SymbolBinding> {
    let mut out = std::collections::BTreeMap::new();
    if let Some(name) = name_pin {
        for lang in ["node", "python", "ruby", "php", "mcp"] {
            out.insert(
                lang.to_string(),
                SymbolBinding {
                    name: Some(name.to_string()),
                    ..Default::default()
                },
            );
        }
    }
    out
}

/// Collect op-interface descriptors into the in-memory [`fluessig::api::ApiDoc`]
/// — the same op-layer IR the loader validates and bindgen projects. `name`
/// becomes the api `source`; `version` stamps the emitter field (as the catalog
/// path does).
///
/// Slice 8a Gap 2 materialises the **`models`** array: every entity/DTO an op
/// references — directly, or transitively through a referenced DTO's fields — is
/// flattened into a `models` entry exactly as the TypeSpec op path does (a to-one
/// relation becomes its FK field(s), a polymorphic one prepends the discriminator,
/// to-many relations are dropped; see [`build_models`]). The `entities`, `edges`,
/// and `records` are the same catalog roots [`build_catalog_full`] takes, so the
/// op layer and the model layer are lowered from one consistent catalog.
pub fn build_api(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    interfaces: &[&'static InterfaceDescriptor],
) -> ApiDoc {
    build_api_typed(
        name,
        version,
        entities,
        edges,
        records,
        interfaces,
        TypeDecls::default(),
    )
}

/// Collect op-interface descriptors into the [`fluessig::api::ApiDoc`], with the
/// declared enums/scalars threaded through so an op/model field typed by an enum
/// lowers to `{ enum }` and one typed by a semantic scalar to its scalar name
/// (Slice 8b). The plain [`build_api`] delegates here with empty decls.
#[allow(clippy::too_many_arguments)]
pub fn build_api_typed(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    interfaces: &[&'static InterfaceDescriptor],
    decls: TypeDecls,
) -> ApiDoc {
    let catalog = build_catalog_typed(name, version, entities, edges, records, decls);
    let resolver = RefResolver::new(entities, decls.enums, decls.scalars, decls.unions);
    let api_interfaces: Vec<ApiInterface> = interfaces
        .iter()
        .map(|i| ApiInterface {
            name: i.name.to_string(),
            doc: i.doc.map(str::to_string),
            ops: i.ops.iter().map(|op| lower_op(op, &resolver)).collect(),
        })
        .collect();
    let (models, unions) = records::build_models(&catalog, &api_interfaces);
    ApiDoc {
        fluessig: Versions {
            format: fluessig::FORMAT_VERSION,
            emitter: Some(format!("fluessig-derive/{version}")),
            compiler: None,
        },
        source: Some(name.to_string()),
        models,
        unions,
        interfaces: api_interfaces,
    }
}

/// Render `api.json` — pretty-printed with a trailing newline, matching the
/// TypeSpec emitter's `JSON.stringify(…, null, 2) + "\n"` and the catalog
/// printer's convention.
pub fn to_api_json(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    interfaces: &[&'static InterfaceDescriptor],
) -> String {
    to_api_json_typed(
        name,
        version,
        entities,
        edges,
        records,
        interfaces,
        TypeDecls::default(),
    )
}

/// Render `api.json` for a catalog with declared enums/scalars (Slice 8b).
#[allow(clippy::too_many_arguments)]
pub fn to_api_json_typed(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    interfaces: &[&'static InterfaceDescriptor],
    decls: TypeDecls,
) -> String {
    let api = build_api_typed(name, version, entities, edges, records, interfaces, decls);
    let mut json = serde_json::to_string_pretty(&api).expect("api serializes");
    json.push('\n');
    json
}
