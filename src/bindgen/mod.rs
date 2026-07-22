//! Binding generation (notes/plan.txt Step 5b) — the op layer (`api.json`) projected
//! into committed per-language binding glue. The thesis (translation.md): every
//! op has a SHAPE (ctor | unary | stream | manual), and the idiom for each
//! shape is written ONCE per language as a genco template — AsyncTask→Promise
//! for napi ([`node`]), `detach` for PyO3 ([`python`]), GVL-plain for Magnus
//! ([`ruby`]) — so N ops × M languages collapses to shapes × languages, and
//! `@manual` stays the escape hatch for the truly bespoke. This module holds
//! the shared halves (type map, core-trait emission, the Arrow-model helpers);
//! the deliberately-parallel template grids live one-per-language in the
//! submodules.
//!
//! Each generated file defines the language surface AND the core traits; the
//! consumer hand-writes ONE `core_impl` module implementing the traits over its
//! engine. Generated code references `crate::core_impl::{GitImpl, EntlImpl}` by
//! convention.

mod cpp;
mod cpp_callback;
mod cpp_header;
mod cpp_hpp;
mod fanout;
mod java;
mod java_callback;
mod mcp;
mod node;
mod php;
mod php_callback;
mod python;
mod ruby;
mod ruby_callback;
mod rust_core;
mod wasm;

pub use cpp::{cpp_binding, cpp_header, cpp_hpp};
pub use fanout::{
    common_path_for, external_refs, fan_out_crate, group_module_path, group_table, render_mod_tree,
    render_use_block, resolve_module_paths, ExternalImport, ExternalRef, FanOutSpec, FannedCrate,
    GroupKey, GroupTable, ModEntry, COMMON_MOD, RUNTIME_STREAM_IMPORT,
};
pub use java::{java_binding, java_sources, java_sources_with};
pub use mcp::{manifest as mcp_manifest, mcp_module};
pub use node::{node_binding, node_binding_with_options, NodeOptions};
pub use php::php_binding;
pub use python::{python_binding, python_binding_with_options, PythonOptions};
pub use ruby::{ruby_binding, ruby_binding_with_options, RubyOptions};
pub use rust_core::rust_core_binding;
pub use wasm::{wasm_binding, wasm_binding_with_options, WasmOptions};

use std::collections::BTreeMap;

use genco::prelude::*;

use crate::api::{ApiDoc, ApiOp, ApiType, ApiUnion, ForeignType, Shape};

/// How a backend lowers a tagged discriminated union crossing the FFI. Shared by
/// every structured-capable backend (node/python/ruby); the default is
/// [`UnionProjection::Structured`] with tag field `"type"` — the user is the sole
/// consumer of the generated surface and wants tagged objects by default. The
/// [`UnionProjection::Envelope`] carrier stays reachable as an explicit opt-out
/// (`--*-union-mode envelope`), reproducing the historical JSON-string output.
#[derive(Clone)]
pub enum UnionProjection {
    /// The historical carrier: the union rides as its JSON envelope text
    /// `{"kind": tag, "payload": body}` typed as `String`.
    Envelope,
    /// Structured projection: each union lowers to per-variant tagged objects
    /// (napi `Either{N}` / PyO3 `#[pyclass]` variants / Magnus wrapped classes)
    /// that embed the discriminant as a literal `tag_field` (per-union override
    /// via [`ApiUnion::tag_field`]).
    Structured { tag_field: String },
}

impl Default for UnionProjection {
    fn default() -> Self {
        UnionProjection::Structured {
            tag_field: "type".into(),
        }
    }
}

/// The per-variant tagged type name for a union variant, e.g. `EventPayload` +
/// tag `message` → `EventPayloadMessage`. Shared by every structured backend.
pub(super) fn tagged_variant_name(union_name: &str, tag: &str) -> String {
    format!("{union_name}{}", pascal(tag))
}

/// The Rust enum name a structured union projects its return/field type to, e.g.
/// `EventPayload` → `EventPayloadUnion` (python/ruby wrap the tagged variants in
/// a convertible enum; node uses napi's `Either{N}` instead).
pub(super) fn union_enum_name(union_name: &str) -> String {
    format!("{union_name}Union")
}

/// The discriminant field ident, raw-escaped when the configured tag field is a
/// Rust keyword (the pi default `type` → `r#type`). Shared by every structured
/// backend so the literal-set and getter idents agree.
pub(super) fn tag_ident(tag_field: &str) -> String {
    // The keywords a tag field could realistically collide with.
    const KEYWORDS: &[&str] = &[
        "type", "match", "move", "ref", "self", "impl", "fn", "enum", "struct", "mod", "as", "in",
        "box", "async", "await", "dyn",
    ];
    let n = snake(tag_field);
    if KEYWORDS.contains(&n.as_str()) {
        format!("r#{n}")
    } else {
        n
    }
}

/// The effective discriminant field name for a union: its per-union
/// [`ApiUnion::tag_field`] override, else the backend-global `tag_field`.
pub(super) fn union_tag_field(u: &ApiUnion, global: &str) -> String {
    u.tag_field.clone().unwrap_or_else(|| global.to_string())
}

/// Does a variant model's data field collide with the union's discriminant tag
/// field? The structured projection prepends the tag field to every tagged
/// struct AND then flattens the variant model's real fields inline; a variant
/// model that ALSO carries a field of the tag's name (the discriminant mirrored
/// into the payload — e.g. an `AgentSessionEvent` union tagged on `type` whose
/// variant models each declare their own `type` field) would otherwise emit the
/// field TWICE — invalid Rust (a duplicate `r#type` struct field / an
/// unescaped `type` ident) and a duplicate exposed napi/pyo3 key.
///
/// The deterministic, wire-preserving dedupe rule: the TAG WINS. When a data
/// field's name equals the tag field's name, the backend skips re-emitting that
/// data field — the literal-set tag field already carries it, embedding the
/// variant's discriminant (`"started"`, `"stopped"`, …) exactly as the wire
/// `type` key requires. The exposed field name and its wire value are unchanged;
/// only the redundant second Rust field is dropped. Compared on the snake-cased
/// source names so the check is language-neutral (node snake→camel, python
/// snake, ruby snake all agree on the collision).
pub(super) fn field_is_tag(field_name: &str, tag_field: &str) -> bool {
    snake(field_name) == snake(tag_field)
}

/// The node (napi) rendering of a struct field whose SOURCE name is already
/// `snake`-cased to `raw`: the raw-escaped Rust ident, paired with the `js_name`
/// value (if any) that must pin the ORIGINAL exposed JS spelling. An explicit
/// `pin` (a `bindings[node].name`) always pins; otherwise a keyword field pins
/// the camelCased original so raw-escaping the ident (`type` → `r#type`) never
/// changes the JS-visible field name; a plain, unpinned field pins nothing
/// (`None`) and emits byte-identically to the pre-escape output. Shared by the
/// DTO-field and structured-union-variant-field paths so both escape identically.
pub(super) fn napi_field_render(raw: &str, pin: Option<&str>) -> (String, Option<String>) {
    let ident = escape_rust_keyword(raw);
    let js = pin
        .map(str::to_string)
        .or_else(|| is_rust_keyword(raw).then(|| crate::ir::camel(raw)));
    (ident, js)
}

/// Re-exported so backends (and the php backend when `php.rs` lands its own
/// consumer) reach the pinning type through `crate::bindgen`.
pub use crate::api::SymbolBinding;

// ── the language-agnostic pinning resolver ───────────────────────────────────
//
// Every backend keeps ONLY (i) its default casing rule and (ii) its own rename
// syntax; the DECISION of whether a symbol is pinned (and to what) lives here,
// once, keyed by the backend's `const LANG`. No backend hardcodes a pin, and no
// per-language logic leaks into this table — that is the whole point of the
// generalization over the earlier node-only `jsName` slot.

/// The exact emitted name a symbol is pinned to in `lang`, or `None` (⇒ the
/// backend applies its own default casing). Looks up `bindings[lang].name`.
pub fn pinned_name(bindings: &BTreeMap<String, SymbolBinding>, lang: &str) -> Option<String> {
    bindings.get(lang).and_then(|b| b.name.clone())
}

/// The `(package, module)` group a symbol is pinned into for `lang`, or `None`
/// (⇒ the symbol stays in the single default file). `package` is the grouping
/// key; a missing `module` defaults to the empty string. Both strings are used
/// VERBATIM (no casing transform) so exact package names / deep import paths
/// reproduce byte-for-byte. Consumed only by the opt-in fan-out.
pub fn pinned_group(
    bindings: &BTreeMap<String, SymbolBinding>,
    lang: &str,
) -> Option<(String, String)> {
    let b = bindings.get(lang)?;
    let package = b.package.clone()?;
    let module = b.module.clone().unwrap_or_default();
    Some((package, module))
}

/// One enum variant as the backends see it: the Rust-side member `name`, the
/// language-NEUTRAL wire `value` override (catalog `Variant.value`, when a
/// string), and the per-language `bindings`. The shared, uniform form every
/// backend consumes — no backend takes a bespoke enum shape any more.
#[derive(Clone, Debug)]
pub struct EnumVariant {
    /// The catalog member name; the Rust variant ident is [`variant_ident`] of
    /// it (`pascal`-cased, with hyphens/other separators sanitized to word
    /// boundaries), while the wire discriminant rides [`variant_token`].
    pub name: String,
    /// The neutral wire override (`Variant.value`), when present as a string.
    pub value: Option<String>,
    /// Per-language export-name pins ([`SymbolBinding`]).
    pub bindings: BTreeMap<String, SymbolBinding>,
}

/// An enum as the backends see it: `(enum name, variants)`.
pub type EnumDesc = (String, Vec<EnumVariant>);

impl EnumVariant {
    /// A plain, un-pinned variant carrying only its name — the shape the
    /// name-only catalog enums (and most tests) build.
    pub fn plain(name: impl Into<String>) -> Self {
        EnumVariant {
            name: name.into(),
            value: None,
            bindings: BTreeMap::new(),
        }
    }
}

/// The wire token a value-projecting backend (node `#[napi(value)]`, ruby/php
/// `wire()`) emits for this variant in `lang`: a pinned `bindings[lang].name`
/// wins, then the neutral `value` override, then the default `to_lowercase()`
/// rule. When nothing is pinned/valued this is exactly `name.to_lowercase()`,
/// so an all-default enum emits byte-identically to before pinning existed.
pub fn variant_token(v: &EnumVariant, lang: &str) -> String {
    pinned_name(&v.bindings, lang)
        .or_else(|| v.value.clone())
        .unwrap_or_else(|| v.name.to_lowercase())
}

/// A `(package, module)` group and the symbol names that fell into it — the
/// unit the opt-in fan-out writes to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolGroup {
    pub package: String,
    pub module: String,
    pub symbols: Vec<String>,
}

/// Partition an api doc's symbols (models, unions, ops) by their pinned
/// `(package, module)` group for `lang`, in first-appearance order. Symbols with
/// no group pin are omitted (they stay in the default single file). The group
/// set comes purely from the schema's distinct pairs — there is NO closed
/// registry of packages/modules. Feeds [`fan_out_path`].
pub fn symbol_groups(api: &ApiDoc, lang: &str) -> Vec<SymbolGroup> {
    let mut groups: Vec<SymbolGroup> = Vec::new();
    let mut push = |bindings: &BTreeMap<String, SymbolBinding>, sym: &str| {
        if let Some((package, module)) = pinned_group(bindings, lang) {
            if let Some(g) = groups
                .iter_mut()
                .find(|g| g.package == package && g.module == module)
            {
                g.symbols.push(sym.to_string());
            } else {
                groups.push(SymbolGroup {
                    package,
                    module,
                    symbols: vec![sym.to_string()],
                });
            }
        }
    };
    for m in &api.models {
        push(&m.bindings, &m.name);
    }
    for u in &api.unions {
        push(&u.bindings, &u.name);
    }
    for i in &api.interfaces {
        for op in &i.ops {
            push(&op.bindings, &op.name);
        }
    }
    groups
}

/// Substitute a group's `{package}` / `{module}` tokens into a patterned output
/// path VERBATIM (no casing transform) — modelled on `readme::render_files`'s
/// `{lang}` replace. Exact npm names and deep `../src/*` module paths reproduce
/// byte-for-byte.
pub fn fan_out_path(pattern: &str, group: &SymbolGroup) -> String {
    pattern
        .replace("{package}", &group.package)
        .replace("{module}", &group.module)
}

/// Build one filtered [`ApiDoc`] per pinned `(package, module)` group for
/// `lang`, each carrying ONLY that group's DTO surface (the models + unions
/// whose `bindings[lang]` names that group), paired with the output path from
/// [`fan_out_path`]. Interfaces/ops are dropped from a group file — a fanned
/// file is the DTO surface for its package.
///
/// Opt-in by construction: an api with no group pins for `lang` yields an EMPTY
/// vec, so the caller stays entirely on the single-file path (which is fully
/// supported and byte-identical to today). The group set is exactly the
/// schema's distinct `(package, module)` pairs — there is NO closed registry.
///
/// This is the LOW-LEVEL primitive: it partitions the DTO surface but does NOT
/// resolve a cross-group reference (a group-A DTO field typed as a group-B DTO)
/// into an import — so a bare `fan_out` sub-document does not compile standalone.
/// The cross-package import subsystem ([`fan_out_crate`]) layers on top: it
/// emits each group file's `use crate::<sanitized-path>::Symbol;` imports, homes
/// every enum once in a shared `common` module, and generates the root
/// `#[path]` mod-tree + `pub use` re-exports that make the split output COMPILE.
/// Use [`fan_out_crate`] to produce a compilable crate; `fan_out` alone remains
/// for callers that only need the raw per-group partition.
pub fn fan_out(api: &ApiDoc, lang: &str, pattern: &str) -> Vec<(String, ApiDoc)> {
    symbol_groups(api, lang)
        .into_iter()
        .map(|g| {
            let in_group = |s: &str| g.symbols.iter().any(|n| n == s);
            let sub = ApiDoc {
                fluessig: api.fluessig.clone(),
                source: api.source.clone(),
                models: api
                    .models
                    .iter()
                    .filter(|m| in_group(&m.name))
                    .cloned()
                    .collect(),
                unions: api
                    .unions
                    .iter()
                    .filter(|u| in_group(&u.name))
                    .cloned()
                    .collect(),
                // Interfaces/ops are not fanned out (see KNOWN LIMITATION): a
                // group file is the DTO surface, so the op layer stays empty.
                interfaces: Vec::new(),
                // Top-level consts are a whole-document concern, not a per-group
                // DTO; a fanned-out sub-document carries none.
                consts: Vec::new(),
            };
            (fan_out_path(pattern, &g), sub)
        })
        .collect()
}

/// snake_case for Rust idents (`repoPath` → `repo_path`).
fn snake(s: &str) -> String {
    crate::ir::snake(s)
}

/// The caller's optional extra banner line (e.g. a lint-suppression marker) as
/// a `//! …\n` doc line, or nothing — fluessig itself never bakes tool-specific
/// markers into its output.
fn note_line(note: Option<&str>) -> String {
    note.map(|n| format!("//! {n}\n")).unwrap_or_default()
}

/// napi's `Either` family name for an arity: 2 → `Either`, 3..=26 → `Either{n}`.
/// Hoisted here (from `node`) so `node.rs` stays under the file-size cap; still
/// napi-specific, but colocated with the other shared name helpers.
fn either_name(n: usize) -> String {
    if n == 2 {
        "Either".to_string()
    } else {
        format!("Either{n}")
    }
}

/// The two `#[napi(object)]` arm names of a `#[fluessig(result)]` op's envelope,
/// keyed off the op name: `readBinaryFile` → (`ReadBinaryFileOk`,
/// `ReadBinaryFileErr`). The struct emission and the method return type read this
/// same helper so they always agree. Hoisted here (from `node`) so `node.rs`
/// stays under the file-size cap; `node` reaches it via `use super::*`.
fn result_envelope_names(op_name: &str) -> (String, String) {
    let p = pascal(op_name);
    (format!("{p}Ok"), format!("{p}Err"))
}

/// `changes` → `Changes` (stream class names, task names).
fn pascal(s: &str) -> String {
    let sn = snake(s);
    sn.split('_')
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// Reserved Rust words that are ILLEGAL even as raw identifiers — escaped with a
/// trailing underscore instead of `r#`.
const RUST_HARD_KEYWORDS: &[&str] = &["self", "Self", "super", "crate"];
/// Reserved Rust words that ARE legal as raw identifiers (`r#kw`).
const RUST_RAW_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "dyn", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "static", "struct", "trait", "true", "type", "unsafe", "use", "where", "while", "async",
    "await", "box",
];

/// Would emitting `ident` as a bare Rust identifier collide with a keyword (so it
/// must be raw-escaped / suffixed to compile)? The node + python backends use
/// this to decide whether a DTO field also needs an explicit `js_name`/`name`
/// attr pinning the ORIGINAL exposed name.
pub(super) fn is_rust_keyword(ident: &str) -> bool {
    RUST_HARD_KEYWORDS.contains(&ident) || RUST_RAW_KEYWORDS.contains(&ident)
}

/// Raw-escape an (already language-cased) identifier that collides with a Rust
/// keyword so it is a valid Rust ident: `type` → `r#type`, `self` → `self_`. A
/// non-keyword ident is returned UNCHANGED (byte-identical to the pre-escape
/// emission). Mirrors rust-core's field escaping so the node and python backends
/// escape exactly the SAME keyword set rust-core already does.
pub(super) fn escape_rust_keyword(ident: &str) -> String {
    if RUST_HARD_KEYWORDS.contains(&ident) {
        format!("{ident}_")
    } else if RUST_RAW_KEYWORDS.contains(&ident) {
        format!("r#{ident}")
    } else {
        ident.to_string()
    }
}

/// Sanitize a source enum-variant discriminant into a valid Rust variant
/// IDENTIFIER. Every run of non-alphanumeric characters (a hyphen in a kebab
/// discriminant like `openai-completions`, plus dots / spaces / slashes) is
/// treated as a word boundary and the segments are PascalCased
/// (`openai-completions` → `OpenaiCompletions`); a leading digit is prefixed
/// with `_` (a Rust ident cannot start with a digit).
///
/// Determinism & collision-safety: this is a pure function of the input string,
/// so the same discriminant always maps to the same ident. The ORIGINAL wire
/// discriminant is NEVER emitted through this function — every backend keeps the
/// wire value separately (the neutral [`variant_token`] → node's
/// `#[napi(value = "…")]`, ruby/php `wire()`, java's string map), so only the
/// language-level identifier is sanitized, never the wire value. A source name
/// that is already a valid ident sanitizes to itself, so this is byte-identical
/// to the prior `pascal(name)` emission for every existing (hyphen-free) variant.
pub(super) fn variant_ident(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let p = pascal(&cleaned);
    match p.chars().next() {
        Some(c) if c.is_ascii_digit() => format!("_{p}"),
        _ => p,
    }
}

/// Enums whose variants carry wire values are projected as plain strings in the
/// bindings for now (napi enums can't carry arbitrary values cleanly).
fn is_string_enum(_api: &ApiDoc, name: &str) -> bool {
    // the api layer doesn't carry enum defs — the catalog does; the bindgen
    // caller passes the set of value-carrying enums via this convention:
    matches!(
        name,
        "FileStatus" | "RefKind" | "PrState" | "IssueState" | "Mergeable"
    )
}

/// Does this type mention the `bytes` scalar anywhere? Gates the per-language
/// `Bytes` alias in the prelude, so an api with no bytes surface generates
/// byte-identically to before the alias existed.
fn mentions_bytes(t: &ApiType) -> bool {
    match t {
        ApiType::Scalar(s) => s == "bytes",
        ApiType::List { list } => mentions_bytes(list),
        ApiType::Nullable { nullable } => mentions_bytes(nullable),
        _ => false,
    }
}

fn api_uses_bytes(api: &ApiDoc) -> bool {
    api.interfaces
        .iter()
        .flat_map(|i| &i.ops)
        .any(|op| mentions_bytes(&op.returns) || op.params.iter().any(|p| mentions_bytes(&p.ty)))
}

/// Does any op in the surface have the `Shape::Subscription` shape? Gates each
/// backend's generated `Subscription` handle class, so a surface with no
/// subscription op emits ZERO new lines and its golden stays byte-identical.
/// Shared by node and python (the two backends that lower it today).
pub(super) fn api_uses_subscription(api: &ApiDoc) -> bool {
    api.interfaces
        .iter()
        .flat_map(|i| &i.ops)
        .any(|op| op.shape == Shape::Subscription)
}

/// Does any op in the surface take an [`ApiType::Callback`] param? Gates the C
/// backend's callback prelude (the `CbCtx` newtype + `c_void` import) and node's
/// napi `threadsafe_function` imports, so a callback-free surface emits ZERO new
/// lines and its golden stays byte-identical. Shared by cpp and node.
pub(super) fn api_uses_callback(api: &ApiDoc) -> bool {
    api.interfaces.iter().flat_map(|i| &i.ops).any(|op| {
        op.params
            .iter()
            .any(|p| matches!(&p.ty, ApiType::Callback { .. }))
    })
}

/// The one-line skip-note a backend emits IN PLACE OF lowering a
/// [`Shape::Subscription`] op on a FACTORY-BORN (ctor-less) interface. Such an
/// interface has no public constructor — its instances are handed back by a factory
/// op elsewhere ([`crate::api::load_api`] now accepts this) — but no backend can yet
/// MINT that stateful handle from the factory op (returning an interface instance as
/// a live handle is itself unimplemented), so a `&self` listener registration has no
/// receiver to bind to. Rather than emit a stateless free-function registration that
/// mismatches the `&self` core-trait method (broken code), the backend emits this
/// honest marker — mirroring cpp/wasm, which already defer subscription lowering.
/// The `//` line-comment form suits every backend's Rust output. Full factory-born
/// handle lowering is the documented follow-up.
pub(super) fn subscription_factory_skip_note(iface: &str, op: &str) -> String {
    format!(
        "// subscription `{iface}.{op}`: factory-born (ctor-less) interface — a \
         stateful handle minted from its factory op is not lowered yet; deferred."
    )
}

/// The `(return_type, unsub_expr, ok_expr)` a `Shape::Subscription` op method
/// splices into its template, keyed on `infallible`: a FALLIBLE op throws on `Err`
/// (`{result_ty}<Subscription>`, `{call}.map_err(err)?`, `Ok(Subscription{..})`), an
/// INFALLIBLE one returns the handle straight through (`Subscription`, `{call}`,
/// `Subscription{..}`). `result_ty` is the backend's throwing-result spelling
/// (node `Result`, python `PyResult`); `call` is the core-call expression. Shared
/// by node and python so their register→unsubscribe lowering agrees.
pub(super) fn subscription_method_parts(
    infallible: bool,
    result_ty: &str,
    call: &str,
) -> (String, String, String) {
    let handle = "Subscription { unsub: std::sync::Mutex::new(Some(unsub)) }";
    if infallible {
        (
            "Subscription".to_string(),
            call.to_string(),
            handle.to_string(),
        )
    } else {
        (
            format!("{result_ty}<Subscription>"),
            format!("{call}.map_err(err)?"),
            format!("Ok({handle})"),
        )
    }
}

/// The CORE-trait return spelling for an op, given its ordinary `base` spelling: a
/// FACTORY op — one whose `returns` names a DECLARED interface (bare or behind a
/// single `Nullable`/`List`) — returns the CORE object
/// `Arc<crate::core_impl::{Iface}Impl>` (the exact type the handle class holds in
/// its `core` field), `Option`/`Vec`-wrapped through the container; the binding then
/// wraps that into the handle class. Every non-factory op keeps `base` unchanged, so
/// existing goldens are byte-identical (nothing currently returns an interface).
pub(super) fn core_return_ty(api: &ApiDoc, op: &ApiOp, base: String) -> String {
    match crate::api::returned_interface(api, &op.returns) {
        Some(ir) => {
            let core = format!("Arc<crate::core_impl::{}Impl>", ir.iface());
            match ir {
                crate::api::InterfaceReturn::Bare(_) => core,
                crate::api::InterfaceReturn::Nullable(_) => format!("Option<{core}>"),
                crate::api::InterfaceReturn::List(_) => format!("Vec<{core}>"),
            }
        }
        None => base,
    }
}

/// The `(return_type, body)` a node/python FACTORY-op method splices in to MINT the
/// handle(s) from the core-returned `Arc<Impl>` — `Ok({Iface} { core: … })`, mapped
/// through the `Nullable`/`List` container — mirroring the `Shape::Stream` wrap (a
/// class built from `self.core.{op}(..)`). `call` is the core-call expression;
/// `result_ty` the throwing-result spelling (`Result`/`PyResult`). `None` for a
/// non-factory op. Shared by node and python so their mint lowering agrees.
pub(super) fn interface_mint(
    api: &ApiDoc,
    op: &ApiOp,
    result_ty: &str,
    call: &str,
    infallible: bool,
) -> Option<(String, String)> {
    let ir = crate::api::returned_interface(api, &op.returns)?;
    let iface = ir.iface().to_string();
    // The core value handed back: `Arc<Impl>` straight (infallible) or through the
    // `?`-unwrapped fallible call. Each `Arc<Impl>` (`c`) becomes `{Iface}{core:c}`.
    let cv = if infallible {
        call.to_string()
    } else {
        format!("{call}.map_err(err)?")
    };
    let (ret_inner, value) = match &ir {
        crate::api::InterfaceReturn::Bare(_) => {
            (iface.clone(), format!("{iface} {{ core: {cv} }}"))
        }
        crate::api::InterfaceReturn::Nullable(_) => (
            format!("Option<{iface}>"),
            format!("{cv}.map(|c| {iface} {{ core: c }})"),
        ),
        crate::api::InterfaceReturn::List(_) => (
            format!("Vec<{iface}>"),
            format!("{cv}.into_iter().map(|c| {iface} {{ core: c }}).collect()"),
        ),
    };
    if infallible {
        Some((ret_inner, value))
    } else {
        Some((format!("{result_ty}<{ret_inner}>"), format!("Ok({value})")))
    }
}

/// The `(return_type, body)` for a SYNCHRONOUS unary op's node/python method,
/// covering all three cases uniformly: a FACTORY op mints the handle (see
/// [`interface_mint`]); an infallible op returns the `base_ret` value straight
/// (`{call}`); a fallible op throws (`{result_ty}<base_ret>`, `{call}.map_err(err)`).
/// Collapsing the three into one seam keeps the per-backend arm a single spliced
/// block (node's line budget) and both backends' lowering in agreement.
pub(super) fn unary_body_parts(
    api: &ApiDoc,
    op: &ApiOp,
    result_ty: &str,
    base_ret: &str,
    call: &str,
    infallible: bool,
) -> (String, String) {
    if let Some(m) = interface_mint(api, op, result_ty, call, infallible) {
        return m;
    }
    if infallible {
        (base_ret.to_string(), call.to_string())
    } else {
        (
            format!("{result_ty}<{base_ret}>"),
            format!("{call}.map_err(err)"),
        )
    }
}

/// Whether `iface` is FACTORY-BORN: constructible (a factory op somewhere returns
/// it) yet carrying NO `Shape::Ctor` of its own — the case node/python lower to a
/// ctor-less handle class, and the follow-up backends (cpp/java/ruby/php/wasm)
/// skip-note wholesale (they cannot yet MINT the handle from a factory return).
pub(super) fn is_factory_born(api: &ApiDoc, iface: &str) -> bool {
    let i = match api.interfaces.iter().find(|i| i.name == iface) {
        Some(i) => i,
        None => return false,
    };
    let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
    !has_ctor && crate::api::constructible_interfaces(api).contains(iface)
}

/// The one-line skip-note a FOLLOW-UP backend (cpp/java/ruby/php/wasm) emits in
/// place of lowering a FACTORY op (an op RETURNING an interface handle) — the
/// interface-return sibling of [`subscription_factory_skip_note`]. Minting a live
/// handle from an op return is lowered for node/python only today; rather than
/// marshal the core's `Arc<Impl>` as an ordinary value (broken code), the backend
/// emits this honest marker. Full factory-mint lowering is the documented follow-up.
pub(super) fn interface_return_skip_note(iface: &str, op: &str, target: &str) -> String {
    format!(
        "// op `{iface}.{op}` returns handle `{target}` — minting a live handle from a \
         factory op return is not lowered for this backend yet; deferred."
    )
}

/// The skip-note a FOLLOW-UP backend (cpp/java/ruby/php/wasm) emits for an entire
/// FACTORY-BORN (ctor-less) interface it does not yet bind — the interface's handle
/// is minted only from a factory op's return, which is lowered for node/python only
/// today, so this backend emits no trait-method glue for it (its `&self` methods
/// have no minted receiver). Sibling of [`interface_return_skip_note`]. Deferred.
pub(super) fn factory_born_interface_skip_note(iface: &str) -> String {
    format!(
        "// interface `{iface}`: factory-born (ctor-less) handle — minting its instance \
         from a factory op return is lowered for node/python only today; this backend \
         binds none of its methods. Deferred."
    )
}

/// The field carrying an Arrow `RecordBatch`, when this model is an
/// Arrow-payload DTO (`ChangeBatch.ipc: ArrowBatch`). Such a model is generated
/// as a class HOLDING the batch (`pub(crate) batch: entl_core::RecordBatch`)
/// with lazy per-language accessors — an IPC-bytes getter everywhere, plus the
/// Arrow PyCapsule protocol in Python — so no encoding happens until asked for.
fn arrow_field(m: &crate::api::ApiModel) -> Option<&crate::api::ApiField> {
    m.fields
        .iter()
        .find(|f| matches!(&f.ty, ApiType::Scalar(s) if s == "ArrowBatch"))
}

/// An [`ApiType`] as (rust type, ts type) strings. `bytes` maps to the `Bytes`
/// alias each generated prelude defines for its own language (napi `Buffer` /
/// `Vec<u8>` → python `bytes` / `bytes::Bytes` → ruby binary String), so the
/// rust spelling is uniform and the shared `binding_core_impls!` macro can use
/// one name; only napi consumes the ts half.
fn ty(api: &ApiDoc, t: &ApiType) -> (String, String) {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "string" => ("String".into(), "string".into()),
            "boolean" => ("bool".into(), "boolean".into()),
            "int32" => ("i32".into(), "number".into()),
            "int64" => ("i64".into(), "number".into()),
            "uint8" => ("u8".into(), "number".into()),
            "uint16" => ("u16".into(), "number".into()),
            "uint32" => ("u32".into(), "number".into()),
            "float32" => ("f32".into(), "number".into()),
            // `float` is TypeSpec's f64 alias; both spell an f64 → `number`.
            "float64" | "float" => ("f64".into(), "number".into()),
            "Json" => ("String".into(), "string".into()), // JSON text payload
            "bytes" => ("Bytes".into(), "Buffer".into()),
            "void" => ("()".into(), "void".into()),
            _ => ("String".into(), "string".into()),
        },
        ApiType::Model { model } => (model.clone(), model.clone()),
        ApiType::Enum { r#enum } => {
            if is_string_enum(api, r#enum) {
                ("String".into(), "string".into())
            } else {
                (r#enum.clone(), r#enum.clone())
            }
        }
        ApiType::List { list } => {
            let (r, t) = ty(api, list);
            (format!("Vec<{r}>"), format!("{t}[]"))
        }
        ApiType::Nullable { nullable } => {
            let (r, t) = ty(api, nullable);
            (format!("Option<{r}>"), format!("{t} | null"))
        }
        // a tagged union crosses the FFI as its JSON envelope text
        // `{"kind": tag, "payload": body}` — the same carrier as `Json`
        ApiType::Union { .. } => ("String".into(), "string".into()),
        // a truly-foreign type lowers to its generated per-type OPAQUE HANDLE
        // (rust-core emits the handle struct) — NOT a silent `String`/Json. The
        // handle name is derived deterministically from the source type name, so
        // every reference site and the emitted struct agree. The same name serves
        // the ts half (an opaque nominal type).
        ApiType::Foreign { foreign } => {
            let h = foreign_handle_name(foreign);
            (h.clone(), h)
        }
        // The UNIFORM core-side shape of a callback — what the generated
        // `<Iface>Core` trait sees, regardless of which language supplied it. Each
        // backend's binding wraps its native callable into this box at the FFI
        // boundary (napi `ThreadsafeFunction`, PyO3 `Py<PyAny>`, …). The ts half is
        // the source-language spelling `(argN: T) => void`.
        ApiType::Callback { callback } => {
            let rust_args = callback
                .params
                .iter()
                .map(|p| ty(api, p).0)
                .collect::<Vec<_>>()
                .join(", ");
            let ts_args = callback
                .params
                .iter()
                .enumerate()
                .map(|(i, p)| format!("arg{i}: {}", ty(api, p).1))
                .collect::<Vec<_>>()
                .join(", ");
            (
                format!("Box<dyn Fn({rust_args}) + Send + Sync>"),
                format!("({ts_args}) => void"),
            )
        }
    }
}

/// The deterministic opaque-handle type name for a truly-foreign type, e.g.
/// `http.Server` → `HttpServerHandle`, `ChildProcess` → `ChildProcessHandle`,
/// `fs.ReadStream` → `FsReadStreamHandle`. Derived PURELY from the source
/// [`ForeignType::name`] (split on any non-alphanumeric boundary, each segment
/// initial-upper-cased with its internal casing preserved, `Handle` appended) so
/// the reference sites (via [`ty`]) and the emitted struct (rust-core) always
/// agree, and two occurrences of the same foreign type collapse to ONE handle.
/// A name that would not start with a letter is prefixed `Foreign`.
pub(super) fn foreign_handle_name(f: &ForeignType) -> String {
    let mut out = String::new();
    for seg in f.name.split(|c: char| !c.is_ascii_alphanumeric()) {
        let mut cs = seg.chars();
        if let Some(first) = cs.next() {
            out.push(first.to_ascii_uppercase());
            out.push_str(cs.as_str());
        }
    }
    if !out.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        out.insert_str(0, "Foreign");
    }
    out.push_str("Handle");
    out
}

fn param_sig(api: &ApiDoc, op: &ApiOp) -> Vec<(String, String)> {
    param_sig_with(op, |t| ty(api, t).0)
}

/// The closure-argument variable names a callback wrapper binds, shared by the
/// node and python backends so their generated closures agree: a single arg is
/// the bare `v` (matching the design note's spelling), N args are `v0..v{N-1}`,
/// zero args is empty (the wrapper then forwards the unit `()`).
pub(super) fn callback_arg_vars(n: usize) -> Vec<String> {
    match n {
        1 => vec!["v".to_string()],
        _ => (0..n).map(|i| format!("v{i}")).collect(),
    }
}

/// The `(name, rust_type)` param list, spelling each param type via `ty_of` and
/// wrapping an `optional` param in `Option<…>`. The shared spine behind both the
/// default [`param_sig`] (via the shared [`ty`]) and node's `node_param_sig`
/// (which spells a `bytes` param `Uint8Array`); factoring it here keeps the two
/// from being flagged as a duplicated loop body.
pub(super) fn param_sig_with(
    op: &ApiOp,
    mut ty_of: impl FnMut(&ApiType) -> String,
) -> Vec<(String, String)> {
    op.params
        .iter()
        .map(|p| {
            let r = ty_of(&p.ty);
            let r = if p.optional == Some(true) {
                format!("Option<{r}>")
            } else {
                r
            };
            (snake(&p.name), r)
        })
        .collect()
}

/// Split a `#[fluessig(single_threaded)]` interface out of the surface a NON-node
/// backend emits. A thread-confined `!Send` handle is node-only today: every
/// other backend's handle wrapper (`#[pyclass]`, `#[php_class]`,
/// `#[wasm_bindgen]`, the JNI/C++ glue) would hold the core in a form that
/// requires `Send`, so a `!Send` core cannot be bound. Per the honest-capability-
/// edge doctrine, such a backend must NOT silently emit a `Send`-assuming handle
/// (which would break the consumer's build with a confusing downstream error) —
/// it emits an explicit skip-note and binds NOTHING for the interface (no trait,
/// no handle). Returns `(filtered_api, skip_note)`: run the backend's normal
/// emission over `filtered_api`, then append `skip_note` (a block of `//`
/// comments, empty when no interface is single_threaded ⇒ output byte-identical).
pub(super) fn split_single_threaded(api: &ApiDoc, lang: &str) -> (ApiDoc, String) {
    let mut note = String::new();
    for i in &api.interfaces {
        if i.single_threaded {
            note.push_str(&format!(
                "// interface `{}` is #[fluessig(single_threaded)] (a thread-confined `!Send` \
                 handle) — not supported by the {lang} backend (node-only today); no binding \
                 emitted.\n",
                i.name
            ));
        }
    }
    let mut filtered = api.clone();
    filtered.interfaces.retain(|i| !i.single_threaded);
    (filtered, note)
}

/// Emit the `<Interface>Core` traits, resolving each op's return type via
/// `ret_ty`. This is the single shared spine every language's generated file
/// drives (the traits are implemented once per binding via
/// `entl_core::binding_core_impls!`): python/ruby and the node envelope default
/// pass the shared [`ty`] (a union rides as its `String` envelope), while the
/// node structured projection passes [`node::node_ty`] so a union return is the
/// `Either{N}<…>` that matches its napi `Task::Output`. `has_ctor` selects the
/// stateful `&self` receiver for non-ctor ops.
pub(super) fn emit_core_traits_with(
    t: &mut rust::Tokens,
    api: &ApiDoc,
    ret_ty: impl FnMut(&ApiOp) -> String,
) {
    // The shared default: params via [`param_sig`], and no result-envelope error
    // channel (every backend but node throws on a fallible op). Node reaches the
    // fuller [`emit_core_traits_full`] to spell its `bytes` params `Uint8Array`
    // and give a `#[fluessig(result)]` op a `Result<T, E>` core signature.
    emit_core_traits_full(t, api, ret_ty, param_sig, |_| None)
}

/// The core-trait spine with two extra seams node needs (both no-ops for the
/// other backends, so their output is byte-identical): `param_sig_fn` spells the
/// trait method params (node overrides only `bytes` → `Uint8Array`), and
/// `result_err` returns `Some(E)` for a `#[fluessig(result)]` op so its core
/// method is `fn … -> Result<T, E>` (error-as-value) rather than the default
/// `anyhow::Result<T>` (throw). Passing `param_sig` + `|_| None` reproduces the
/// historical signature exactly.
pub(super) fn emit_core_traits_full(
    t: &mut rust::Tokens,
    api: &ApiDoc,
    mut ret_ty: impl FnMut(&ApiOp) -> String,
    param_sig_fn: impl Fn(&ApiDoc, &ApiOp) -> Vec<(String, String)>,
    result_err: impl Fn(&ApiOp) -> Option<String>,
) {
    // An op on a CONSTRUCTIBLE interface (its own ctor OR a factory op somewhere
    // returns it) gets the stateful `&self` receiver — a factory-born (ctor-less)
    // interface's methods must be `&self` so the minted handle class can call them
    // through `self.core`. Computed once; every non-constructible interface keeps
    // its free-function (`fn {name}(..)`) spelling, so existing goldens are
    // byte-identical (no current interface is factory-born).
    let constructible = crate::api::constructible_interfaces(api);
    for i in &api.interfaces {
        let trait_name = format!("{}Core", i.name);
        let stateful = constructible.contains(&i.name);
        // A `single_threaded` interface lowers to a THREAD-CONFINED handle: the
        // generated node class holds the core in a `RefCell` and reaches it via
        // `borrow_mut()`, so its handle-bound ops take `&mut self` (not `&self`),
        // and the trait sheds its `Send + Sync` supertraits — the whole point is
        // that a `!Send` core can implement it. Guaranteed sync-only + ctor-having
        // by the derive macro + loader, so only the `has_ctor` `&self` arms flip.
        let st = i.single_threaded;
        let recv = if st { "&mut self" } else { "&self" };
        let mut methods: Vec<rust::Tokens> = Vec::new();
        for op in &i.ops {
            if op.shape == Shape::Manual {
                continue;
            }
            let name = snake(&op.name);
            let ps = param_sig_fn(api, op)
                .iter()
                .map(|(n, r)| format!("{n}: {r}"))
                .collect::<Vec<_>>()
                .join(", ");
            // A FACTORY op's core method returns the CORE object
            // `Arc<crate::core_impl::{Iface}Impl>` (Option/Vec-wrapped), which the
            // binding wraps into the handle class; every other op keeps `ret_ty`.
            let ret = core_return_ty(api, op, ret_ty(op));
            let re = result_err(op);
            let sig = match op.shape {
                Shape::Ctor => format!("fn {name}({ps}) -> anyhow::Result<Self>"),
                Shape::Stream => {
                    format!("fn {name}(&self, {ps}) -> anyhow::Result<Box<dyn PollStream<{ret}>>>")
                }
                // A `Shape::Subscription` op REGISTERS the listener (its callback
                // param, spelled the uniform `Box<dyn Fn(Args) + Send + Sync>` by
                // `param_sig_fn`) and returns the UNSUBSCRIBE closure. The
                // hand-written `core_impl` adds the listener to its set and returns a
                // `Box::new(move || { /* deregister */ })`; each backend's generated
                // binding wraps that closure into its `Subscription` handle class.
                // Rides the same stateful `&self` receiver as the unary arms.
                Shape::Subscription if op.infallible => {
                    format!("fn {name}({recv}, {ps}) -> Box<dyn Fn() + Send + Sync>")
                }
                Shape::Subscription => {
                    format!(
                        "fn {name}({recv}, {ps}) -> anyhow::Result<Box<dyn Fn() + Send + Sync>>"
                    )
                }
                // A `#[fluessig(result)]` op returns its error AS A VALUE: the core
                // method is `Result<T, E>` with the EXPLICIT error record `E`, so
                // the binding can build the `{ ok, value } | { ok, error }` envelope
                // instead of throwing. Rides the same `has_ctor` receiver split.
                _ if re.is_some() && stateful => {
                    format!(
                        "fn {name}({recv}, {ps}) -> ::core::result::Result<{ret}, {}>",
                        re.as_deref().unwrap()
                    )
                }
                _ if re.is_some() => {
                    format!(
                        "fn {name}({ps}) -> ::core::result::Result<{ret}, {}>",
                        re.as_deref().unwrap()
                    )
                }
                // An infallible (`#[fluessig(sync)]` + bare-`T` return) unary op
                // drops the `Result` seam entirely — the core method IS the value.
                // `infallible` is only ever set on a unary op, so this rides the
                // same `has_ctor` receiver split as the fallible unary arms below.
                _ if stateful && op.infallible => format!("fn {name}({recv}, {ps}) -> {ret}"),
                _ if op.infallible => format!("fn {name}({ps}) -> {ret}"),
                _ if stateful => format!("fn {name}({recv}, {ps}) -> anyhow::Result<{ret}>"),
                _ => format!("fn {name}({ps}) -> anyhow::Result<{ret}>"),
            };
            methods.push(quote!($sig;));
        }
        // A single_threaded core is thread-confined — it must NOT require
        // `Send + Sync` (that is the wall this variant tears down); an ordinary
        // async-capable core keeps them (its `Arc<Impl>` crosses to a worker).
        let bounds = if st {
            "Sized + 'static"
        } else {
            "Sized + Send + Sync + 'static"
        };
        quote_in! { *t =>
            $['\r']
            $(format!("/// The `{}` contract — implement over the engine in `crate::core_impl`.", i.name))
            pub trait $(&trait_name): $(bounds) {
                $(for m in &methods join ($['\r']) => $m)
            }
            $['\n']
        };
    }
}

/// The `<Interface>Core` traits with the shared envelope return mapping — the
/// carrier python/ruby and the node envelope default all share.
fn emit_core_traits(t: &mut rust::Tokens, api: &ApiDoc) {
    emit_core_traits_with(t, api, |op| ty(api, &op.returns).0);
}

#[cfg(test)]
mod ident_tests {
    use super::*;

    /// The variant-ident sanitizer is byte-identical to the prior `pascal(name)`
    /// emission for every hyphen-free name (guaranteeing existing goldens are
    /// unaffected), and pascal-cases across hyphens/other separators for the
    /// full-orch discriminants (`openai-completions` → `OpenaiCompletions`).
    #[test]
    fn variant_ident_sanitizes_and_stays_byte_identical_for_valid_names() {
        // Identity to pascal() for the shapes existing goldens contain:
        for n in ["dispatch", "Dispatch", "someThing", "some_thing", "http2"] {
            assert_eq!(variant_ident(n), pascal(n), "must equal pascal() for `{n}`");
        }
        // The bug-1 inputs: hyphens (and friends) become word boundaries.
        assert_eq!(variant_ident("openai-completions"), "OpenaiCompletions");
        assert_eq!(variant_ident("source-origin"), "SourceOrigin");
        assert_eq!(variant_ident("a.b c/d"), "ABCD");
        // A leading digit is prefixed so the ident is legal.
        assert_eq!(variant_ident("3d"), "_3d");
    }

    /// The keyword escaper mirrors rust-core: raw idents for the `r#`-legal
    /// keywords, a trailing underscore for the hard ones, unchanged otherwise.
    #[test]
    fn escape_rust_keyword_matches_rust_core() {
        assert_eq!(escape_rust_keyword("type"), "r#type");
        assert_eq!(escape_rust_keyword("match"), "r#match");
        assert_eq!(escape_rust_keyword("self"), "self_");
        assert_eq!(escape_rust_keyword("id"), "id");
        assert!(is_rust_keyword("type") && is_rust_keyword("self"));
        assert!(!is_rust_keyword("id"));
    }
}
