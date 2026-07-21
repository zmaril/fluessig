//! The MCP projection — the op layer as an MCP tool surface (issue #4).
//!
//! Two halves over one walk:
//! - [`manifest`]: the tools manifest as JSON — one tool per projected op
//!   (`<iface>_<op>`), input schemas from the DTO models (docs ported from the
//!   tsp, unions as discriminated `oneOf` envelopes), `readOnlyHint` /
//!   `destructiveHint` from `@readonly` / `@destructive`.
//! - [`mcp_module`]: a generated Rust module — plain-serde DTO structs, one
//!   `<Iface>Mcp` trait per interface, a `dispatch()` that maps (tool name,
//!   JSON args) onto the trait call, and the manifest embedded as
//!   `TOOLS_JSON`. A stdio server crate over any fluessig-described engine is
//!   a read-line loop around `dispatch()`.
//!
//! Lowering rules: `@ctor` and `@manual` don't project (the server holds the
//! open instance; manual is per-binding); `@stream` projects as a CURSOR tool
//! (`after`/`limit` params, one page per call — the poll shape is already
//! cursor-friendly); ops whose surface mentions a binary carrier (`bytes`,
//! `ArrowBatch`) are skipped — MCP tools speak JSON.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

use crate::api::{ApiDoc, ApiOp, ApiType, Shape};

use super::{pinned_name, snake, EnumDesc};

/// This backend's language slug — the key it reads out of every symbol's
/// `bindings` map via the shared [`pinned_name`] resolver. MCP hardcodes no pin;
/// its rename levers are the DTO field's serde `rename` and the matching
/// manifest property name.
const LANG: &str = "mcp";

/// Does this type reach a binary carrier (`bytes` / `ArrowBatch`), directly or
/// through model fields? Such ops don't project (MCP tools speak JSON).
fn mentions_binary(api: &ApiDoc, t: &ApiType, seen: &mut Vec<String>) -> bool {
    match t {
        ApiType::Scalar(s) => s == "bytes" || s == "ArrowBatch",
        ApiType::List { list } => mentions_binary(api, list, seen),
        ApiType::Nullable { nullable } => mentions_binary(api, nullable, seen),
        ApiType::Model { model } => {
            if seen.contains(model) {
                return false;
            }
            seen.push(model.clone());
            api.models
                .iter()
                .find(|m| &m.name == model)
                .is_some_and(|m| m.fields.iter().any(|f| mentions_binary(api, &f.ty, seen)))
        }
        ApiType::Union { union } => api
            .unions
            .iter()
            .find(|u| &u.name == union)
            .is_some_and(|u| u.variants.iter().any(|v| mentions_binary(api, &v.ty, seen))),
        ApiType::Enum { .. } | ApiType::Foreign { .. } => false,
    }
}

fn op_is_binary(api: &ApiDoc, op: &ApiOp) -> bool {
    let mut seen = Vec::new();
    mentions_binary(api, &op.returns, &mut seen)
        || op
            .params
            .iter()
            .any(|p| mentions_binary(api, &p.ty, &mut seen))
}

/// Does this op become a tool?
fn projects(api: &ApiDoc, op: &ApiOp) -> bool {
    matches!(op.shape, Shape::Unary | Shape::Stream) && !op_is_binary(api, op)
}

// ── the manifest half ────────────────────────────────────────────────────────

/// A type's JSON Schema; model defs collect into `defs` (per-tool `$defs`).
fn schema_of(
    api: &ApiDoc,
    enums: &[EnumDesc],
    t: &ApiType,
    defs: &mut BTreeMap<String, Value>,
) -> Value {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "string" => json!({"type": "string"}),
            "boolean" => json!({"type": "boolean"}),
            "int32" | "int64" => json!({"type": "integer"}),
            "float64" => json!({"type": "number"}),
            "utcDateTime" | "offsetDateTime" => json!({"type": "string", "format": "date-time"}),
            "Json" => json!({"description": "opaque JSON"}),
            "void" => json!({"type": "null"}),
            _ => json!({"type": "string"}), // semantic scalars ride as strings
        },
        ApiType::Enum { r#enum } => {
            // Each member's schema token is an `mcp` pin when present, else the
            // catalog name verbatim (the pre-pinning behaviour, byte-identical).
            let variants: Vec<String> = enums
                .iter()
                .find(|(n, _)| n == r#enum)
                .map(|(_, vs)| {
                    vs.iter()
                        .map(|v| pinned_name(&v.bindings, LANG).unwrap_or_else(|| v.name.clone()))
                        .collect()
                })
                .unwrap_or_default();
            json!({"type": "string", "enum": variants})
        }
        ApiType::List { list } => {
            json!({"type": "array", "items": schema_of(api, enums, list, defs)})
        }
        ApiType::Nullable { nullable } => {
            json!({"anyOf": [schema_of(api, enums, nullable, defs), {"type": "null"}]})
        }
        ApiType::Model { model } => {
            if !defs.contains_key(model) {
                defs.insert(model.clone(), Value::Null); // cycle guard
                let m = api
                    .models
                    .iter()
                    .find(|m| &m.name == model)
                    .expect("model in api.json");
                let mut props = Map::new();
                let mut required = Vec::new();
                for f in &m.fields {
                    // The wire property name is an `mcp` pin when present, else
                    // the field name verbatim (byte-identical un-pinned). The
                    // generated serde DTO carries the matching `rename`.
                    let key = pinned_name(&f.bindings, LANG).unwrap_or_else(|| f.name.clone());
                    props.insert(key.clone(), schema_of(api, enums, &f.ty, defs));
                    if !f.nullable {
                        required.push(Value::String(key));
                    }
                }
                let mut def = Map::new();
                def.insert("type".into(), "object".into());
                if let Some(doc) = &m.doc {
                    def.insert("description".into(), doc.clone().into());
                }
                def.insert("properties".into(), Value::Object(props));
                if !required.is_empty() {
                    def.insert("required".into(), Value::Array(required));
                }
                def.insert("additionalProperties".into(), false.into());
                defs.insert(model.clone(), Value::Object(def));
            }
            json!({"$ref": format!("#/$defs/{model}")})
        }
        // a tagged union: the discriminated envelope, one branch per variant
        ApiType::Union { union } => {
            let u = api
                .unions
                .iter()
                .find(|u| &u.name == union)
                .expect("union in api.json");
            let branches: Vec<Value> = u
                .variants
                .iter()
                .map(|v| {
                    json!({
                        "type": "object",
                        "properties": {
                            "kind": {"const": v.tag},
                            "payload": schema_of(api, enums, &v.ty, defs),
                        },
                        "required": ["kind", "payload"],
                        "additionalProperties": false,
                    })
                })
                .collect();
            let mut sch = Map::new();
            if let Some(doc) = &u.doc {
                sch.insert("description".into(), doc.clone().into());
            }
            sch.insert("oneOf".into(), Value::Array(branches));
            Value::Object(sch)
        }
        // A foreign type has no JSON projection MCP can speak — an opaque handle
        // lives only in rust-core — so it surfaces as an opaque description.
        ApiType::Foreign { .. } => json!({"description": "opaque handle"}),
    }
}

/// The MCP tools manifest: `{"tools": [{name, description, inputSchema,
/// annotations}, …]}`. `enums` carries the catalog's enum variants (the api
/// layer doesn't) — same convention as the binding generators.
pub fn manifest(api: &ApiDoc, enums: &[EnumDesc]) -> Value {
    let mut tools = Vec::new();
    for i in &api.interfaces {
        for op in i.ops.iter().filter(|op| projects(api, op)) {
            let mut defs = BTreeMap::new();
            let mut props = Map::new();
            let mut required = Vec::new();
            for p in &op.params {
                props.insert(p.name.clone(), schema_of(api, enums, &p.ty, &mut defs));
                if p.optional != Some(true) {
                    required.push(Value::String(p.name.clone()));
                }
            }
            if op.shape == Shape::Stream {
                props.insert(
                    "after".into(),
                    json!({"type": "integer", "description": "resume cursor: return items after this index"}),
                );
                props.insert(
                    "limit".into(),
                    json!({"type": "integer", "description": "max items in this page"}),
                );
            }
            let mut input = Map::new();
            input.insert("type".into(), "object".into());
            input.insert("properties".into(), Value::Object(props));
            if !required.is_empty() {
                input.insert("required".into(), Value::Array(required));
            }
            input.insert("additionalProperties".into(), false.into());
            if !defs.is_empty() {
                input.insert(
                    "$defs".into(),
                    Value::Object(defs.into_iter().collect::<Map<_, _>>()),
                );
            }

            let mut tool = Map::new();
            tool.insert(
                "name".into(),
                format!("{}_{}", snake(&i.name), snake(&op.name)).into(),
            );
            let mut desc = op.doc.clone().unwrap_or_default();
            if op.shape == Shape::Stream {
                if !desc.is_empty() {
                    desc.push(' ');
                }
                desc.push_str(
                    "(Paged: pass `after` from the last item you saw; one page per call.)",
                );
            }
            tool.insert("description".into(), desc.into());
            tool.insert("inputSchema".into(), Value::Object(input));
            let mut ann = Map::new();
            if op.readonly {
                ann.insert("readOnlyHint".into(), true.into());
            }
            if op.destructive {
                ann.insert("destructiveHint".into(), true.into());
            }
            if !ann.is_empty() {
                tool.insert("annotations".into(), Value::Object(ann));
            }
            tools.push(Value::Object(tool));
        }
    }
    json!({"tools": tools})
}

// ── the Rust-module half ─────────────────────────────────────────────────────

/// An [`ApiType`] as the plain-serde Rust type the generated module speaks.
fn rust_ty(t: &ApiType) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "string" | "utcDateTime" | "offsetDateTime" => "String".into(),
            "boolean" => "bool".into(),
            "int32" => "i32".into(),
            "int64" => "i64".into(),
            "float64" => "f64".into(),
            "Json" => "serde_json::Value".into(),
            "void" => "()".into(),
            _ => "String".into(),
        },
        ApiType::Model { model } => model.clone(),
        ApiType::Enum { .. } => "String".into(),
        ApiType::List { list } => format!("Vec<{}>", rust_ty(list)),
        ApiType::Nullable { nullable } => format!("Option<{}>", rust_ty(nullable)),
        // the JSON envelope {"kind": tag, "payload": body}, as a Value
        ApiType::Union { .. } => "serde_json::Value".into(),
        // a foreign handle has no MCP projection; carried as its string form
        ApiType::Foreign { .. } => "String".into(),
    }
}

/// Is this model reachable from any projected op? (Binary-carrier models and
/// models only used by skipped ops stay out of the generated module.)
fn projected_models(api: &ApiDoc) -> Vec<&crate::api::ApiModel> {
    let mut wanted: Vec<String> = Vec::new();
    let push_type = |t: &ApiType, wanted: &mut Vec<String>| {
        fn walk(api: &ApiDoc, t: &ApiType, wanted: &mut Vec<String>) {
            match t {
                ApiType::Model { model } => {
                    if !wanted.contains(model) {
                        wanted.push(model.clone());
                        if let Some(m) = api.models.iter().find(|m| &m.name == model) {
                            for f in &m.fields {
                                walk(api, &f.ty, wanted);
                            }
                        }
                    }
                }
                ApiType::List { list } => walk(api, list, wanted),
                ApiType::Nullable { nullable } => walk(api, nullable, wanted),
                _ => {}
            }
        }
        walk(api, t, wanted)
    };
    for i in &api.interfaces {
        for op in i.ops.iter().filter(|op| projects(api, op)) {
            push_type(&op.returns, &mut wanted);
            for p in &op.params {
                push_type(&p.ty, &mut wanted);
            }
        }
    }
    api.models
        .iter()
        .filter(|m| wanted.contains(&m.name))
        .collect()
}

/// The generated Rust MCP module: serde DTOs + `<Iface>Mcp` traits +
/// `dispatch()` + the embedded `TOOLS_JSON` manifest.
pub fn mcp_module(api: &ApiDoc, enums: &[EnumDesc], banner_note: Option<&str>) -> String {
    let src = api.source.as_deref().unwrap_or("the fluessig catalog");
    let mut out = String::new();
    out.push_str(&format!(
        "//! GENERATED by fluessig mcp from {src} (api layer). Do not edit.\n"
    ));
    if let Some(n) = banner_note {
        out.push_str(&format!("//! {n}\n"));
    }
    out.push_str("#![allow(clippy::all)]\n\n");

    // ── DTOs ──
    for m in projected_models(api) {
        if let Some(doc) = &m.doc {
            for line in doc.lines() {
                out.push_str(&format!("/// {line}\n"));
            }
        }
        out.push_str("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]\n");
        out.push_str("#[serde(rename_all = \"camelCase\")]\n");
        out.push_str(&format!("pub struct {} {{\n", m.name));
        for f in &m.fields {
            let ty = rust_ty(&f.ty);
            let ty = if f.nullable {
                format!("Option<{ty}>")
            } else {
                ty
            };
            // An `mcp` pin fixes the exact wire name via serde `rename`,
            // overriding the struct-level `rename_all = "camelCase"`; combined
            // with the nullable default/skip when both apply. Un-pinned ⇒ the
            // original attrs, byte-identical.
            let pin = pinned_name(&f.bindings, LANG);
            match (&pin, f.nullable) {
                (Some(nm), true) => out.push_str(&format!(
                    "    #[serde(rename = \"{nm}\", default, skip_serializing_if = \"Option::is_none\")]\n"
                )),
                (Some(nm), false) => {
                    out.push_str(&format!("    #[serde(rename = \"{nm}\")]\n"))
                }
                (None, true) => out.push_str(
                    "    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n",
                ),
                (None, false) => {}
            }
            out.push_str(&format!("    pub {}: {},\n", snake(&f.name), ty));
        }
        out.push_str("}\n\n");
    }

    // ── traits ──
    for i in &api.interfaces {
        let ops: Vec<&ApiOp> = i.ops.iter().filter(|op| projects(api, op)).collect();
        if ops.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "/// The `{}` MCP contract — implement over the open engine.\n",
            i.name
        ));
        out.push_str(&format!("pub trait {}Mcp {{\n", i.name));
        for op in &ops {
            let params: Vec<String> = op
                .params
                .iter()
                .map(|p| {
                    let ty = rust_ty(&p.ty);
                    let ty = if p.optional == Some(true) {
                        format!("Option<{ty}>")
                    } else {
                        ty
                    };
                    format!("{}: {}", snake(&p.name), ty)
                })
                .collect();
            let ret = rust_ty(&op.returns);
            match op.shape {
                Shape::Stream => out.push_str(&format!(
                    "    /// One page of `{}` (cursor: items after `after`, at most `limit`).\n    fn {}(&self, {}after: Option<i64>, limit: Option<u32>) -> anyhow::Result<Vec<{ret}>>;\n",
                    op.name,
                    snake(&op.name),
                    params.iter().map(|p| format!("{p}, ")).collect::<String>(),
                )),
                _ => out.push_str(&format!(
                    "    fn {}({}) -> anyhow::Result<{ret}>;\n",
                    snake(&op.name),
                    std::iter::once("&self".to_string())
                        .chain(params.iter().cloned())
                        .collect::<Vec<_>>()
                        .join(", "),
                )),
            }
        }
        out.push_str("}\n\n");
    }

    // ── dispatch ──
    let ifaces: Vec<&crate::api::ApiInterface> = api
        .interfaces
        .iter()
        .filter(|i| i.ops.iter().any(|op| projects(api, op)))
        .collect();
    let generics: Vec<String> = ifaces
        .iter()
        .enumerate()
        .map(|(n, i)| format!("T{}: {}Mcp", n, i.name))
        .collect();
    let args_sig: Vec<String> = ifaces
        .iter()
        .enumerate()
        .map(|(n, i)| format!("{}: &T{}", snake(&i.name), n))
        .collect();
    out.push_str("/// One JSON tool call → the trait call → the JSON result.\n");
    out.push_str(&format!(
        "pub fn dispatch<{}>({}, tool: &str, args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {{\n",
        generics.join(", "),
        args_sig.join(", "),
    ));
    out.push_str("    match tool {\n");
    for i in &ifaces {
        for op in i.ops.iter().filter(|op| projects(api, op)) {
            let tool = format!("{}_{}", snake(&i.name), snake(&op.name));
            out.push_str(&format!("        \"{tool}\" => {{\n"));
            let mut call_args = Vec::new();
            for p in &op.params {
                let ty = rust_ty(&p.ty);
                let n = snake(&p.name);
                if p.optional == Some(true) {
                    out.push_str(&format!(
                        "            let {n}: Option<{ty}> = opt_arg(args, \"{}\")?;\n",
                        p.name
                    ));
                } else {
                    out.push_str(&format!(
                        "            let {n}: {ty} = arg(args, \"{}\")?;\n",
                        p.name
                    ));
                }
                call_args.push(n);
            }
            if op.shape == Shape::Stream {
                out.push_str("            let after: Option<i64> = opt_arg(args, \"after\")?;\n");
                out.push_str("            let limit: Option<u32> = opt_arg(args, \"limit\")?;\n");
                call_args.push("after".into());
                call_args.push("limit".into());
            }
            out.push_str(&format!(
                "            Ok(serde_json::to_value({}.{}({})?)?)\n",
                snake(&i.name),
                snake(&op.name),
                call_args.join(", "),
            ));
            out.push_str("        }\n");
        }
    }
    out.push_str("        _ => anyhow::bail!(\"unknown tool: {tool}\"),\n");
    out.push_str("    }\n}\n\n");

    // param extraction helpers
    out.push_str(
        "fn arg<T: serde::de::DeserializeOwned>(args: &serde_json::Value, name: &str) -> anyhow::Result<T> {\n\
         \x20   let v = args.get(name).ok_or_else(|| anyhow::anyhow!(\"missing required argument: {name}\"))?;\n\
         \x20   Ok(serde_json::from_value(v.clone()).map_err(|e| anyhow::anyhow!(\"argument {name}: {e}\"))?)\n\
         }\n\n\
         fn opt_arg<T: serde::de::DeserializeOwned>(args: &serde_json::Value, name: &str) -> anyhow::Result<Option<T>> {\n\
         \x20   match args.get(name) {\n\
         \x20       None | Some(serde_json::Value::Null) => Ok(None),\n\
         \x20       Some(v) => Ok(Some(serde_json::from_value(v.clone()).map_err(|e| anyhow::anyhow!(\"argument {name}: {e}\"))?)),\n\
         \x20   }\n\
         }\n\n",
    );

    // ── the embedded manifest ──
    let manifest_json =
        serde_json::to_string_pretty(&manifest(api, enums)).expect("manifest serializes");
    out.push_str("/// The MCP tools manifest (name, description, inputSchema, annotations).\n");
    out.push_str(&format!(
        "pub const TOOLS_JSON: &str = r###\"{manifest_json}\"###;\n"
    ));

    crate::rustfmt::format(out)
}
