//! The wasm-bindgen (browser JS/TS) template grid — one language's projection of
//! the op shapes into a Rust `cdylib` compiled for `wasm32-unknown-unknown`.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.
//!
//! Unlike the napi/PyO3/Magnus backends, the wasm surface marshals structured
//! values through `serde_wasm_bindgen` (models/enums/lists/unions become plain
//! JS objects honoring the serde attrs), and the real `.d.ts` types are declared
//! in a `typescript_custom_section`. Ops are synchronous by default (#69); an
//! explicitly-async op returns a `Promise` via `wasm_bindgen_futures`. Streams
//! are NOT supported in this MVP — each is emitted as an honest skip comment
//! rather than broken code (the "honest capability edges" convention).

use genco::prelude::*;

use crate::api::{ApiDoc, ApiOp, ApiType, ApiUnion, Shape};

use super::*;

/// This backend's language slug — the key it reads out of every symbol's
/// `bindings` map through the shared [`pinned_name`] / [`variant_token`]
/// resolver. wasm hardcodes no pin; its own rename levers are wasm-bindgen's
/// `#[wasm_bindgen(js_name = …)]` (functions) and serde's `#[serde(rename = …)]`
/// (DTO fields / enum tokens, which `serde_wasm_bindgen` honors on the wire).
const LANG: &str = "wasm";

/// Backend options for the wasm generator — mirrors [`NodeOptions`] /
/// [`PythonOptions`]. The 3-arg [`wasm_binding`] threads `WasmOptions::default()`
/// (structured tagged-object union projection); pass an explicit
/// [`UnionProjection::Envelope`] to opt into the JSON-string carrier.
#[derive(Default, Clone)]
pub struct WasmOptions {
    /// How union return values and nested union DTO fields are lowered.
    pub union_projection: UnionProjection,
}

/// A union eligible for structured serde projection: at least two variants, all
/// of which are model refs (an internally-tagged serde enum wraps model structs).
/// Anything else falls back to the JSON envelope `String` carrier (shared [`ty`]).
fn union_projectable<'a>(api: &'a ApiDoc, name: &str) -> Option<&'a ApiUnion> {
    api.unions.iter().find(|u| u.name == name).filter(|u| {
        u.variants.len() >= 2
            && u.variants
                .iter()
                .all(|v| matches!(&v.ty, ApiType::Model { .. }))
    })
}

/// The wasm `(rust, ts)` spelling of a type, applying structured union projection
/// when [`WasmOptions::union_projection`] asks for it (a union → the serde enum
/// `<Union>Union` / the TS `export type <Union>`). Delegates to the shared [`ty`]
/// for everything else, so envelope mode matches the language default.
fn wasm_ty(api: &ApiDoc, opts: &WasmOptions, t: &ApiType) -> (String, String) {
    match (t, &opts.union_projection) {
        (ApiType::Union { union }, UnionProjection::Structured { .. }) => {
            match union_projectable(api, union) {
                Some(u) => (union_enum_name(&u.name), u.name.clone()),
                None => ty(api, t),
            }
        }
        (ApiType::List { list }, _) => {
            let (r, s) = wasm_ty(api, opts, list);
            (format!("Vec<{r}>"), format!("{s}[]"))
        }
        (ApiType::Nullable { nullable }, _) => {
            let (r, s) = wasm_ty(api, opts, nullable);
            (format!("Option<{r}>"), format!("{s} | null"))
        }
        _ => ty(api, t),
    }
}

/// A param/field type that cannot cross the wasm boundary as a native ABI value
/// (so it rides as a `JsValue` marshalled through `serde_wasm_bindgen`): a model,
/// a value-carrying enum, a list, a nullable, or a union. A string-projected enum
/// (see [`is_string_enum`]) is a plain `String` and stays native.
fn is_complex(api: &ApiDoc, t: &ApiType) -> bool {
    match t {
        ApiType::Enum { r#enum } => !is_string_enum(api, r#enum),
        ApiType::Model { .. }
        | ApiType::List { .. }
        | ApiType::Nullable { .. }
        | ApiType::Union { .. } => true,
        _ => false,
    }
}

/// Does this return type cross back as a native wasm ABI value (a scalar / `void`
/// / string-enum) rather than a `serde_wasm_bindgen`-encoded `JsValue`?
fn is_native_return(api: &ApiDoc, t: &ApiType) -> bool {
    match t {
        ApiType::Scalar(_) => true,
        ApiType::Enum { r#enum } => is_string_enum(api, r#enum),
        _ => false,
    }
}

/// The TS-visible field name for a model field: a `wasm` pin wins, else the
/// normalized lowerCamel spelling (serde's `rename_all = "camelCase"` on the DTO
/// produces exactly this key, so the interface and the wire agree).
fn ts_field_name(f: &crate::api::ApiField) -> String {
    pinned_name(&f.bindings, LANG).unwrap_or_else(|| crate::ir::camel(&snake(&f.name)))
}

/// Append an optional `///` doc block for `doc` to `out`.
fn push_doc(out: &mut String, doc: &Option<String>) {
    if let Some(d) = doc {
        for l in d.lines() {
            out.push_str(&format!("/// {l}\n"));
        }
    }
}

/// Build a unary/ctor op's wasm parameter list: `(decls, deser, args, complex)`.
/// A native param lands as `name: RustTy`; a complex one lands as an annotated
/// `#[wasm_bindgen(unchecked_param_type = "Ts")] name: JsValue` plus a
/// `serde_wasm_bindgen::from_value` deserialize line, so the body sees the real
/// Rust type. `complex` is true when ANY param is marshalled (⇒ the fn must
/// return a `Result` for the deserialize error, even if the op is infallible).
fn build_params(
    api: &ApiDoc,
    opts: &WasmOptions,
    op: &ApiOp,
) -> (String, Vec<String>, String, bool) {
    let sigs = param_sig(api, op);
    let mut decls = Vec::new();
    let mut deser = Vec::new();
    let mut args = Vec::new();
    let mut complex = false;
    for (p, (n, rty)) in op.params.iter().zip(sigs.iter()) {
        let is_cx = is_complex(api, &p.ty) || p.optional == Some(true);
        if is_cx {
            complex = true;
            let (_, ts) = wasm_ty(api, opts, &p.ty);
            let ts = if p.optional == Some(true) {
                format!("{ts} | undefined")
            } else {
                ts
            };
            decls.push(format!(
                "#[wasm_bindgen(unchecked_param_type = {ts:?})] {n}: JsValue"
            ));
            deser.push(format!(
                "let {n}: {rty} = serde_wasm_bindgen::from_value({n}).map_err(err)?;"
            ));
        } else {
            decls.push(format!("{n}: {rty}"));
        }
        args.push(n.clone());
    }
    (decls.join(", "), deser, args.join(", "), complex)
}

/// Render one unary op as a `#[wasm_bindgen]` free function (stateless) or method
/// (`is_method`), delegating to `call` (a callable path such as
/// `<…Impl as …Core>::op` or `self.inner.op`). Honors the #69 sync/infallible
/// split: a synchronous op is a plain fn; an infallible one drops the `Result`
/// seam (unless a struct param forces the deserialize boundary); an async op
/// returns a `Promise`.
fn render_unary(
    api: &ApiDoc,
    opts: &WasmOptions,
    op: &ApiOp,
    call: &str,
    is_method: bool,
) -> String {
    let name = snake(&op.name);
    let js = pinned_name(&op.bindings, LANG).unwrap_or_else(|| crate::ir::camel(&name));
    let (params_decl, deser, args, _cx) = build_params(api, opts, op);
    let sig_params = if is_method {
        if params_decl.is_empty() {
            "&self".to_string()
        } else {
            format!("&self, {params_decl}")
        }
    } else {
        params_decl
    };
    let call_expr = format!("{call}({args})");
    let native = is_native_return(api, &op.returns);
    let is_void = matches!(&op.returns, ApiType::Scalar(s) if s == "void");
    let (ret_rust, ret_ts) = wasm_ty(api, opts, &op.returns);
    // A struct param forces a fallible boundary (the deserialize can fail) even
    // when the op itself is infallible.
    let must_result = !op.infallible || !deser.is_empty();

    let mut out = String::new();
    push_doc(&mut out, &op.doc);
    let deser_block: String = deser.iter().map(|l| format!("{l}\n")).collect();

    if op.is_async {
        // Async: a `Promise` via `future_to_promise`. The core call is driven
        // synchronously into a `Result<JsValue, JsValue>`, then wrapped in a
        // ready future — preserving `await` parity without borrowing `self`
        // across the future.
        let promise_ts = if is_void {
            "Promise<void>".to_string()
        } else {
            format!("Promise<{ret_ts}>")
        };
        out.push_str(&format!(
            "#[wasm_bindgen(js_name = {js:?}, unchecked_return_type = {promise_ts:?})]\n"
        ));
        out.push_str(&format!(
            "pub fn {name}({sig_params}) -> js_sys::Promise {{\n"
        ));
        out.push_str("let result: Result<JsValue, JsValue> = (|| {\n");
        out.push_str(&deser_block);
        if is_void {
            if op.infallible {
                out.push_str(&format!("{call_expr};\nOk(JsValue::UNDEFINED)\n"));
            } else {
                out.push_str(&format!(
                    "{call_expr}.map_err(err)?;\nOk(JsValue::UNDEFINED)\n"
                ));
            }
        } else {
            if op.infallible {
                out.push_str(&format!("let out = {call_expr};\n"));
            } else {
                out.push_str(&format!("let out = {call_expr}.map_err(err)?;\n"));
            }
            out.push_str("Ok(serde_wasm_bindgen::to_value(&out)?)\n");
        }
        out.push_str("})();\n");
        out.push_str("future_to_promise(async move { result })\n}\n");
        return out;
    }

    // ── synchronous ──
    let unchecked = if native || is_void {
        String::new()
    } else {
        format!(", unchecked_return_type = {ret_ts:?}")
    };
    out.push_str(&format!("#[wasm_bindgen(js_name = {js:?}{unchecked})]\n"));

    if is_void {
        if must_result {
            out.push_str(&format!(
                "pub fn {name}({sig_params}) -> Result<(), JsValue> {{\n"
            ));
            out.push_str(&deser_block);
            if op.infallible {
                out.push_str(&format!("{call_expr};\nOk(())\n}}\n"));
            } else {
                out.push_str(&format!("{call_expr}.map_err(err)?;\nOk(())\n}}\n"));
            }
        } else {
            out.push_str(&format!(
                "pub fn {name}({sig_params}) {{\n{call_expr};\n}}\n"
            ));
        }
        return out;
    }

    if native {
        if must_result {
            out.push_str(&format!(
                "pub fn {name}({sig_params}) -> Result<{ret_rust}, JsValue> {{\n"
            ));
            out.push_str(&deser_block);
            if op.infallible {
                out.push_str(&format!("Ok({call_expr})\n}}\n"));
            } else {
                out.push_str(&format!("{call_expr}.map_err(err)\n}}\n"));
            }
        } else {
            out.push_str(&format!(
                "pub fn {name}({sig_params}) -> {ret_rust} {{\n{call_expr}\n}}\n"
            ));
        }
    } else if must_result {
        // serde-encoded return, fallible boundary
        out.push_str(&format!(
            "pub fn {name}({sig_params}) -> Result<JsValue, JsValue> {{\n"
        ));
        out.push_str(&deser_block);
        if op.infallible {
            out.push_str(&format!("let out = {call_expr};\n"));
        } else {
            out.push_str(&format!("let out = {call_expr}.map_err(err)?;\n"));
        }
        out.push_str("Ok(serde_wasm_bindgen::to_value(&out)?)\n}\n");
    } else {
        // serde-encoded return, infallible: a null on encode failure keeps the
        // sync fn total (no error channel to surface it through).
        out.push_str(&format!("pub fn {name}({sig_params}) -> JsValue {{\n"));
        out.push_str(&format!("let out = {call_expr};\n"));
        out.push_str("serde_wasm_bindgen::to_value(&out).unwrap_or(JsValue::NULL)\n}\n");
    }
    out
}

/// The honest capability edge for a stream op: wasm is single-threaded and has no
/// poll-cursor idiom yet, so emit a comment naming the intended future shape
/// rather than broken code.
fn stream_skip(op: &ApiOp) -> String {
    format!(
        "// stream op `{}` is not yet supported by the wasm backend (single-threaded; a batch collect-to-Vec op is the intended shape).\n",
        op.name
    )
}

/// A `@manual` op is recorded but hand-written by the consumer, never auto-bound.
fn manual_note(op: &ApiOp) -> String {
    format!(
        "// @manual: {} — hand-written by the consumer, not auto-bound.\n",
        op.name
    )
}

/// Generate the wasm-bindgen binding with default options (structured tagged
/// unions). A thin wrapper over [`wasm_binding_with_options`].
pub fn wasm_binding(api: &ApiDoc, enums: &[EnumDesc], banner_note: Option<&str>) -> String {
    wasm_binding_with_options(api, enums, banner_note, WasmOptions::default())
}

/// Generate the wasm-bindgen binding: a `typescript_custom_section` of real
/// `.d.ts` types, serde DTO structs / enums / union enums, the shared core
/// traits, and per-interface `#[wasm_bindgen]` surfaces (a handle struct with a
/// `#[wasm_bindgen(constructor)]`, or stateless free functions). Streams are
/// skipped honestly; `@manual` ops are recorded but not bound.
pub fn wasm_binding_with_options(
    api: &ApiDoc,
    enums: &[EnumDesc],
    banner_note: Option<&str>,
    opts: WasmOptions,
) -> String {
    let any_async = api
        .interfaces
        .iter()
        .flat_map(|i| &i.ops)
        .any(|o| o.is_async);
    let any_stream = api
        .interfaces
        .iter()
        .flat_map(|i| &i.ops)
        .any(|o| o.shape == Shape::Stream);

    // ── prelude ──
    let mut prelude = String::new();
    prelude.push_str("// The fixed prelude — wasm-bindgen glue; core calls are fully-qualified.\n");
    prelude.push_str("use wasm_bindgen::prelude::*;\n");
    if any_async {
        prelude.push_str("use wasm_bindgen_futures::future_to_promise;\n");
    }
    if any_stream {
        prelude.push_str(&format!(
            "// The shared streaming contract — Poll/PollStream live in the fluessig-runtime crate.\n{}\n",
            RUNTIME_STREAM_IMPORT.render()
        ));
    }
    prelude.push_str(
        "\n/// A core-layer failure becomes a rejected JS value (wasm-bindgen throws it):\n\
         /// a fallible op returns `Result<_, JsValue>` and raises on `Err`.\n\
         fn err(e: impl std::fmt::Display) -> JsValue {\n\
         JsValue::from_str(&e.to_string())\n}\n",
    );
    if api_uses_bytes(api) {
        prelude.push_str(
            "\n/// Bulk bytes cross into JS as a `Uint8Array` (wasm-bindgen maps `Vec<u8>`).\n\
             pub type Bytes = Vec<u8>;\n",
        );
    }

    // ── typescript_custom_section: the real .d.ts types ──
    let mut ts: Vec<String> = Vec::new();
    for m in &api.models {
        let arrow = arrow_field(m);
        let mut lines = vec![format!("export interface {} {{", m.name)];
        for f in &m.fields {
            if Some(f.name.as_str()) == arrow.map(|a| a.name.as_str()) {
                continue;
            }
            let (_, mut tsty) = wasm_ty(api, &opts, &f.ty);
            if f.nullable {
                tsty = format!("{tsty} | null");
            }
            lines.push(format!("  {}: {tsty};", ts_field_name(f)));
        }
        lines.push("}".into());
        ts.push(lines.join("\n"));
    }
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        let toks: Vec<String> = variants
            .iter()
            .map(|v| format!("{:?}", variant_token(v, LANG)))
            .collect();
        ts.push(format!("export type {name} = {};", toks.join(" | ")));
    }
    if let UnionProjection::Structured { tag_field } = &opts.union_projection {
        for u in &api.unions {
            let Some(u) = union_projectable(api, &u.name) else {
                continue;
            };
            let tf = union_tag_field(u, tag_field);
            let mut variant_ifaces: Vec<String> = Vec::new();
            for v in &u.variants {
                let ApiType::Model { model } = &v.ty else {
                    continue;
                };
                let iface = tagged_variant_name(&u.name, &v.tag);
                variant_ifaces.push(iface.clone());
                let mut lines = vec![
                    format!("export interface {iface} {{"),
                    format!("  {tf}: {:?};", v.tag),
                ];
                if let Some(m) = api.models.iter().find(|m| &m.name == model) {
                    for f in &m.fields {
                        let (_, mut tsty) = wasm_ty(api, &opts, &f.ty);
                        if f.nullable {
                            tsty = format!("{tsty} | null");
                        }
                        lines.push(format!("  {}: {tsty};", ts_field_name(f)));
                    }
                }
                lines.push("}".into());
                ts.push(lines.join("\n"));
            }
            ts.push(format!(
                "export type {} = {};",
                u.name,
                variant_ifaces.join(" | ")
            ));
        }
    }
    let mut body = String::new();
    body.push_str(&prelude);
    body.push('\n');
    if !ts.is_empty() {
        let ts_body = ts.join("\n");
        body.push_str(&format!(
            "#[wasm_bindgen(typescript_custom_section)]\nconst TS_APPEND: &'static str = r#\"\n{ts_body}\n\"#;\n\n"
        ));
    }

    // ── DTO structs ──
    for m in &api.models {
        push_doc(&mut body, &m.doc);
        let arrow = arrow_field(m);
        if let Some(af) = arrow {
            body.push_str(&format!(
                "// note: model `{}` carries an Arrow RecordBatch field `{}`; the wasm backend has\n\
                 // no serde-friendly Arrow marshalling, so that field is omitted from the DTO\n\
                 // (encode it to IPC bytes core-side and expose it via a bytes-returning op).\n",
                m.name, af.name
            ));
        }
        body.push_str("#[derive(serde::Serialize, serde::Deserialize)]\n#[serde(rename_all = \"camelCase\")]\n");
        body.push_str(&format!("pub struct {} {{\n", m.name));
        for f in &m.fields {
            if Some(f.name.as_str()) == arrow.map(|a| a.name.as_str()) {
                continue;
            }
            let (mut r, _) = wasm_ty(api, &opts, &f.ty);
            if f.nullable {
                r = format!("Option<{r}>");
            }
            if let Some(pin) = pinned_name(&f.bindings, LANG) {
                body.push_str(&format!("#[serde(rename = {pin:?})]\n"));
            }
            body.push_str(&format!("pub {}: {r},\n", snake(&f.name)));
        }
        body.push_str("}\n\n");
    }

    // ── enums (name-only → serde string enums honoring the wire token) ──
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        body.push_str("#[derive(serde::Serialize, serde::Deserialize)]\n");
        body.push_str(&format!("pub enum {name} {{\n"));
        for v in variants {
            body.push_str(&format!(
                "#[serde(rename = {:?})]\n",
                variant_token(v, LANG)
            ));
            body.push_str(&format!("{},\n", pascal(&v.name)));
        }
        body.push_str("}\n\n");
    }

    // ── union enums (internally-tagged serde over the variant models) ──
    if let UnionProjection::Structured { tag_field } = &opts.union_projection {
        for u in &api.unions {
            match union_projectable(api, &u.name) {
                Some(u) => {
                    let tf = union_tag_field(u, tag_field);
                    body.push_str("#[derive(serde::Serialize, serde::Deserialize)]\n");
                    body.push_str(&format!("#[serde(tag = {tf:?})]\n"));
                    body.push_str(&format!("pub enum {} {{\n", union_enum_name(&u.name)));
                    for v in &u.variants {
                        let ApiType::Model { model } = &v.ty else {
                            continue;
                        };
                        body.push_str(&format!("#[serde(rename = {:?})]\n", v.tag));
                        body.push_str(&format!("{}({model}),\n", pascal(&v.tag)));
                    }
                    body.push_str("}\n\n");
                }
                None => body.push_str(&format!(
                    "// note: union `{}` is not structurally projectable (needs >=2 model-ref variants); it crosses as the JSON envelope string.\n\n",
                    u.name
                )),
            }
        }
    }

    // ── core traits (shared spine, wasm return mapping) ──
    let mut tt: rust::Tokens = quote!();
    emit_core_traits_with(&mut tt, api, |op| wasm_ty(api, &opts, &op.returns).0);
    body.push_str(&tt.to_file_string().expect("rust renders"));
    body.push('\n');

    // ── per-interface op surface ──
    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        let trait_name = format!("{}Core", i.name);
        let impl_path = format!("crate::core_impl::{}Impl", i.name);

        if has_ctor {
            push_doc(&mut body, &i.doc);
            body.push_str(&format!(
                "#[wasm_bindgen]\npub struct {0} {{\ninner: {1},\n}}\n\n#[wasm_bindgen]\nimpl {0} {{\n",
                i.name, impl_path
            ));
            for op in &i.ops {
                match op.shape {
                    Shape::Ctor => {
                        let name = snake(&op.name);
                        let (params_decl, deser, args, _cx) = build_params(api, &opts, op);
                        push_doc(&mut body, &op.doc);
                        body.push_str("#[wasm_bindgen(constructor)]\n");
                        body.push_str(&format!(
                            "pub fn new({params_decl}) -> Result<{}, JsValue> {{\n",
                            i.name
                        ));
                        for l in &deser {
                            body.push_str(&format!("{l}\n"));
                        }
                        body.push_str(&format!(
                            "Ok({} {{ inner: <{impl_path} as {trait_name}>::{name}({args}).map_err(err)? }})\n}}\n",
                            i.name
                        ));
                    }
                    Shape::Unary => {
                        let call = format!("self.inner.{}", snake(&op.name));
                        body.push_str(&render_unary(api, &opts, op, &call, true));
                    }
                    Shape::Stream => body.push_str(&stream_skip(op)),
                    Shape::Manual => body.push_str(&manual_note(op)),
                }
                body.push('\n');
            }
            body.push_str("}\n\n");
        } else {
            push_doc(&mut body, &i.doc);
            for op in &i.ops {
                match op.shape {
                    Shape::Unary => {
                        let call = format!("<{impl_path} as {trait_name}>::{}", snake(&op.name));
                        body.push_str(&render_unary(api, &opts, op, &call, false));
                    }
                    Shape::Stream => body.push_str(&stream_skip(op)),
                    Shape::Manual => body.push_str(&manual_note(op)),
                    // A ctor in a would-be stateless interface flips has_ctor,
                    // so this arm is unreachable; skip it defensively.
                    Shape::Ctor => {}
                }
                body.push('\n');
            }
        }
    }

    let src = api.source.as_deref().unwrap_or("the fluessig catalog");
    crate::rustfmt::format(format!(
        "//! GENERATED by fluessig bindgen from {src} (api layer). Do not edit.\n{}#![allow(clippy::all)]\n#![allow(unused_imports)]\n\n{body}",
        note_line(banner_note)
    ))
}
