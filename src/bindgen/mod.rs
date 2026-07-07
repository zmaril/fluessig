//! Binding generation (plan.txt Step 5b) — the op layer (`api.json`) projected
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

mod node;
mod python;
mod ruby;

pub use node::node_binding;
pub use python::python_binding;
pub use ruby::ruby_binding;

use genco::prelude::*;

use crate::api::{ApiDoc, ApiOp, ApiType, Shape};

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
            "float64" => ("f64".into(), "number".into()),
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
    }
}

fn param_sig(api: &ApiDoc, op: &ApiOp) -> Vec<(String, String)> {
    op.params
        .iter()
        .map(|p| {
            let (r, _) = ty(api, &p.ty);
            let r = if p.optional == Some(true) {
                format!("Option<{r}>")
            } else {
                r
            };
            (snake(&p.name), r)
        })
        .collect()
}

/// The `(snake name, "n: ty, …" param list, rust return type)` triple every
/// per-op emission loop opens with.
fn op_sig(api: &ApiDoc, op: &ApiOp) -> (String, String, String) {
    let name = snake(&op.name);
    let params: Vec<String> = param_sig(api, op)
        .iter()
        .map(|(n, r)| format!("{n}: {r}"))
        .collect();
    let (ret, _) = ty(api, &op.returns);
    (name, params.join(", "), ret)
}

/// The `<Interface>Core` traits — identical across every language's generated
/// file (each binding implements them once via `entl_core::binding_core_impls!`).
fn emit_core_traits(t: &mut rust::Tokens, api: &ApiDoc) {
    for i in &api.interfaces {
        let trait_name = format!("{}Core", i.name);
        let mut methods: Vec<rust::Tokens> = Vec::new();
        for op in &i.ops {
            if op.shape == Shape::Manual {
                continue;
            }
            let (name, ps, ret) = op_sig(api, op);
            let sig = match op.shape {
                Shape::Ctor => format!("fn {name}({ps}) -> anyhow::Result<Self>"),
                Shape::Stream => {
                    format!("fn {name}(&self, {ps}) -> anyhow::Result<Box<dyn PollStream<{ret}>>>")
                }
                _ if i.ops.iter().any(|o| o.shape == Shape::Ctor) => {
                    format!("fn {name}(&self, {ps}) -> anyhow::Result<{ret}>")
                }
                _ => format!("fn {name}({ps}) -> anyhow::Result<{ret}>"),
            };
            methods.push(quote!($sig;));
        }
        quote_in! { *t =>
            $['\r']
            $(format!("/// The `{}` contract — implement over the engine in `crate::core_impl`.", i.name))
            pub trait $(&trait_name): Sized + Send + Sync + $("'static") {
                $(for m in &methods join ($['\r']) => $m)
            }
            $['\n']
        };
    }
}
