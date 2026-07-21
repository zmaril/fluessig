//! The C header (`.h`) projection of the op surface — plain C wrapped in an
//! include guard and an `extern "C"` block. NOT Rust; not run through rustfmt.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.
//!
//! Shares the crossing classifier ([`super::cpp::classify`]) with the Rust
//! export layer and the C++ wrapper, so every prototype here matches the
//! `extern "C"` symbol it fronts.

use crate::api::{ApiDoc, ApiOp, ApiType, CallbackSig, Shape};

use super::cpp::{
    c_enum_name, c_list_name, c_model_name, c_stream_name, c_value_type, classify, classify_param,
    collect_list_elems, field_name, list_elem_token, op_symbol, Cross,
};
use super::*;

/// `Flavor` → `FL_FLAVOR`, `isolation_vm` → `ISOLATION_VM` (C macro casing).
fn upper_snake(s: &str) -> String {
    snake(s).to_uppercase()
}

/// The C by-value member/return type for a scalar-ish crossing.
fn c_val(c: &Cross) -> String {
    c_value_type(c)
}

/// One C IN parameter declaration (may expand to two comma-joined decls, e.g.
/// `const uint8_t* p, size_t p_len`).
fn c_in_param(c: &Cross, name: &str) -> String {
    match c {
        Cross::I32 => format!("int32_t {name}"),
        Cross::I64 => format!("int64_t {name}"),
        Cross::U8 => format!("uint8_t {name}"),
        Cross::U16 => format!("uint16_t {name}"),
        Cross::U32 => format!("uint32_t {name}"),
        Cross::F32 => format!("float {name}"),
        Cross::F64 => format!("double {name}"),
        Cross::Bool => format!("bool {name}"),
        Cross::Str | Cross::StrEnum | Cross::Union => format!("const char* {name}"),
        Cross::Bytes => format!("const uint8_t* {name}, size_t {name}_len"),
        Cross::Enum(e) => format!("{} {name}", c_enum_name(e)),
        Cross::Model(m) => format!("const {}* {name}", c_model_name(m)),
        Cross::List(inner) => format!("const {}* {name}, size_t {name}_len", c_val(inner)),
        Cross::Nullable(inner) => match inner.as_ref() {
            Cross::Str | Cross::StrEnum | Cross::Union => format!("const char* {name}"),
            Cross::Model(m) => format!("const {}* {name}", c_model_name(m)),
            Cross::Enum(e) => format!("const {}* {name}", c_enum_name(e)),
            other => format!("const {}* {name}", c_val(other)),
        },
        Cross::Void => String::new(),
    }
}

/// The C OUT parameter decl(s) for a return crossing (leading, before `err_out`).
fn c_out_params(c: &Cross) -> Vec<String> {
    match c {
        Cross::Void => Vec::new(),
        Cross::I32 => vec!["int32_t* out".into()],
        Cross::I64 => vec!["int64_t* out".into()],
        Cross::U8 => vec!["uint8_t* out".into()],
        Cross::U16 => vec!["uint16_t* out".into()],
        Cross::U32 => vec!["uint32_t* out".into()],
        Cross::F32 => vec!["float* out".into()],
        Cross::F64 => vec!["double* out".into()],
        Cross::Bool => vec!["bool* out".into()],
        Cross::Enum(e) => vec![format!("{}* out", c_enum_name(e))],
        Cross::Str | Cross::StrEnum | Cross::Union => vec!["char** out".into()],
        Cross::Bytes => vec!["FlBytes* out".into()],
        Cross::Model(m) => vec![format!("{}* out", c_model_name(m))],
        Cross::List(inner) => vec![format!("{}* out", c_list_name(&list_elem_token(inner)))],
        Cross::Nullable(inner) => {
            let mut v = vec!["bool* has_out".to_string()];
            v.extend(c_out_params(inner));
            v
        }
    }
}

/// Join the IN params + a receiver prefix into a C parameter list fragment. A
/// callback param expands to a fn-ptr + `void* ctx` pair (one comma-joined entry).
fn in_list(api: &ApiDoc, op: &ApiOp) -> Vec<String> {
    op.params
        .iter()
        .filter_map(|p| {
            let name = snake(&p.name);
            if let ApiType::Callback { callback } = &p.ty {
                Some(c_callback_in_param(api, callback, &name))
            } else {
                let s = c_in_param(&classify_param(api, p), &name);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            }
        })
        .collect()
}

/// A callback IN param's C decls: the fn pointer `void (*name)(void* ctx, Args)`
/// plus its `void* name_ctx` context, comma-joined into one list entry.
fn c_callback_in_param(api: &ApiDoc, sig: &CallbackSig, name: &str) -> String {
    let vars = callback_arg_vars(sig.params.len());
    let args: Vec<String> = sig
        .params
        .iter()
        .zip(&vars)
        .map(|(p, v)| format!("{} {v}", c_val(&classify(api, p))))
        .collect();
    let arg_list = std::iter::once("void* ctx".to_string())
        .chain(args)
        .collect::<Vec<_>>()
        .join(", ");
    format!("void (*{name})({arg_list}), void* {name}_ctx")
}

/// Generate the C header string.
pub fn cpp_header(api: &ApiDoc, enums: &[EnumDesc], banner_note: Option<&str>) -> String {
    // A `single_threaded` interface is a thread-confined `!Send` handle — node-only
    // today; the C ABI cdylib (`cpp_binding`) emits nothing for it, so this header
    // must not declare its symbols either. Split it out + note it honestly.
    let (api_owned, st_note) = crate::bindgen::split_single_threaded(api, "cpp");
    let api = &api_owned;
    let uses_bytes = api_uses_bytes(api);
    let src = api.source.as_deref().unwrap_or("fluessig");
    let guard = format!(
        "FLUESSIG_{}_H",
        src.chars()
            .map(|c| if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            })
            .collect::<String>()
    );

    let mut s = String::new();
    s.push_str(&format!(
        "/* GENERATED by fluessig bindgen from {src} (C header). Do not edit. */\n"
    ));
    if let Some(n) = banner_note {
        s.push_str(&format!("/* {n} */\n"));
    }
    s.push_str(&format!("#ifndef {guard}\n#define {guard}\n\n"));
    s.push_str("#include <stdint.h>\n#include <stddef.h>\n#include <stdbool.h>\n\n");
    s.push_str("#ifdef __cplusplus\nextern \"C\" {\n#endif\n\n");

    // ── FlPoll (always) ──
    s.push_str("/* The pull-cursor poll result. */\n");
    s.push_str("typedef enum FlPoll {\n    FL_POLL_ITEM = 0,\n    FL_POLL_IDLE = 1,\n    FL_POLL_CLOSED = 2,\n    FL_POLL_FAILED = 3\n} FlPoll;\n\n");

    // ── FlBytes (gated) ──
    if uses_bytes {
        s.push_str("/* Owned byte buffer; free with fl_bytes_free. */\n");
        s.push_str("typedef struct FlBytes {\n    uint8_t* data;\n    size_t len;\n} FlBytes;\n\n");
    }

    // ── enums (real, int-backed; string-enums cross as char*) ──
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        let cname = c_enum_name(name);
        s.push_str(&format!("typedef enum {cname} {{\n"));
        let arms: Vec<String> = variants
            .iter()
            .enumerate()
            .map(|(idx, v)| {
                format!(
                    "    FL_{}_{} = {idx}",
                    upper_snake(name),
                    upper_snake(&v.name)
                )
            })
            .collect();
        s.push_str(&arms.join(",\n"));
        s.push_str(&format!("\n}} {cname};\n\n"));
    }

    // ── list structs (for every list element type that appears) ──
    let list_elems = collect_list_elems(api);
    for (token, inner) in &list_elems {
        let name = c_list_name(token);
        let inner_c = c_val(inner);
        let elem_ty = if inner_c == "char*" {
            "char**".to_string()
        } else {
            format!("{inner_c}*")
        };
        s.push_str(&format!(
            "typedef struct {name} {{\n    {elem_ty} data;\n    size_t len;\n}} {name};\n\n"
        ));
    }

    // ── DTO structs ──
    for m in &api.models {
        let cname = c_model_name(&m.name);
        s.push_str(&format!("typedef struct {cname} {{\n"));
        for f in &m.fields {
            let name = field_name(f);
            let mut c = classify(api, &f.ty);
            if f.nullable {
                c = Cross::Nullable(Box::new(c));
            }
            match &c {
                Cross::Nullable(inner) if !member_is_ptr(inner) => {
                    s.push_str(&format!("    bool has_{name};\n"));
                    s.push_str(&format!("    {} {name};\n", c_val(inner)));
                }
                Cross::Nullable(inner) => {
                    // pointer-carrying nullable: NULL = absent
                    s.push_str(&format!("    {} {name};\n", ptr_member(inner)));
                }
                _ => s.push_str(&format!("    {} {name};\n", c_val(&c))),
            }
        }
        s.push_str(&format!("}} {cname};\n\n"));
    }

    // ── opaque handle + stream cursor typedefs ──
    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        if has_ctor {
            s.push_str(&format!("typedef struct {0} {0};\n", i.name));
        }
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let cur = c_stream_name(&i.name, op);
            s.push_str(&format!("typedef struct {cur} {cur};\n"));
        }
    }
    // The opaque subscription handle (once, gated) — a subscription op returns it.
    if api_uses_subscription(api) {
        s.push_str("typedef struct Subscription Subscription;\n");
    }
    s.push('\n');

    // ── op prototypes ──
    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        s.push_str(&format!("/* {} */\n", i.name));
        for op in &i.ops {
            s.push_str(&op_prototype(api, &i.name, op, has_ctor));
        }
        s.push('\n');
    }

    // ── free functions ──
    s.push_str("/* memory management */\n");
    s.push_str("void fl_string_free(char* p);\n");
    s.push_str("void fl_error_free(char* p);\n");
    if uses_bytes {
        s.push_str("void fl_bytes_free(FlBytes* b);\n");
    }
    for m in &api.models {
        if model_owns_heap(api, m) {
            s.push_str(&format!(
                "void fl_{}_free({}* p);\n",
                snake(&m.name),
                c_model_name(&m.name)
            ));
        }
    }
    for (token, _) in &list_elems {
        s.push_str(&format!(
            "void fl_{}_list_free({}* p);\n",
            snake(token),
            c_list_name(token)
        ));
    }
    for i in &api.interfaces {
        if i.ops.iter().any(|o| o.shape == Shape::Ctor) {
            s.push_str(&format!("void {0}_free({0}* self);\n", i.name));
        }
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let cur = c_stream_name(&i.name, op);
            s.push_str(&format!("void {cur}_close({cur}* s);\n"));
        }
    }
    // The subscription handle lifecycle (once, gated): unsubscribe early + free.
    if api_uses_subscription(api) {
        s.push_str("void Subscription_unsubscribe(Subscription* s);\n");
        s.push_str("void Subscription_free(Subscription* s);\n");
    }

    s.push_str("\n#ifdef __cplusplus\n}\n#endif\n");
    s.push_str(&format!("#endif /* {guard} */\n"));
    s.push_str(&st_note);
    s
}

/// Whether a crossing surfaces as a pointer struct member (NULL = absent).
fn member_is_ptr(c: &Cross) -> bool {
    matches!(
        c,
        Cross::Str | Cross::StrEnum | Cross::Union | Cross::Model(_)
    )
}

/// The pointer member type for a pointer-carrying nullable field.
fn ptr_member(c: &Cross) -> String {
    match c {
        Cross::Str | Cross::StrEnum | Cross::Union => "char*".into(),
        Cross::Model(m) => format!("{}*", c_model_name(m)),
        _ => "char*".into(),
    }
}

/// Does a model hold heap members (so it needs a `fl_<m>_free`)?
fn model_owns_heap(api: &ApiDoc, m: &crate::api::ApiModel) -> bool {
    m.fields.iter().any(|f| {
        let c = classify(api, &f.ty);
        matches!(
            c,
            Cross::Str
                | Cross::StrEnum
                | Cross::Union
                | Cross::Bytes
                | Cross::Model(_)
                | Cross::List(_)
        ) || (f.nullable && member_is_ptr(&c))
    })
}

/// One op's C prototype line(s), branching on shape + the fallibility axis.
fn op_prototype(api: &ApiDoc, iface: &str, op: &ApiOp, has_ctor: bool) -> String {
    let sym = op_symbol(iface, op);
    let recv = if has_ctor && op.shape != Shape::Ctor {
        Some(format!("{iface}* self"))
    } else {
        None
    };
    let ins = in_list(api, op);
    let join = |parts: Vec<String>| parts.join(", ");

    match op.shape {
        Shape::Ctor => {
            let mut parts = ins;
            parts.push(format!("{iface}** out"));
            parts.push("char** err_out".into());
            format!("int {iface}_new({});\n", join(parts))
        }
        Shape::Manual => format!("/* @manual {iface}::{} */\n", op.name),
        // A subscription op registers the listener (its callback param, a fn-ptr +
        // ctx pair) and hands back an opaque `Subscription*`. Infallible ⇒ void +
        // `out`; fallible ⇒ int status + `err_out`.
        Shape::Subscription => {
            let mut parts = Vec::new();
            if let Some(r) = &recv {
                parts.push(r.clone());
            }
            parts.extend(ins);
            parts.push("Subscription** out".into());
            if op.infallible {
                format!("void {sym}({});\n", join(parts))
            } else {
                parts.push("char** err_out".into());
                format!("int {sym}({});\n", join(parts))
            }
        }
        Shape::Stream => {
            let cur = c_stream_name(iface, op);
            let item = classify(api, &op.returns);
            let mut parts = Vec::new();
            if let Some(r) = &recv {
                parts.push(r.clone());
            }
            parts.extend(ins.clone());
            parts.push(format!("{cur}** out"));
            parts.push("char** err_out".into());
            let mut s = format!("int {sym}({});\n", join(parts));
            s.push_str(&format!(
                "FlPoll {cur}_next({cur}* s, uint32_t timeout_ms, {}* item_out, char** err_out);\n",
                c_val(&item)
            ));
            s
        }
        Shape::Unary => {
            let ret = classify(api, &op.returns);
            let mut lead = Vec::new();
            if let Some(r) = &recv {
                lead.push(r.clone());
            }
            lead.extend(ins);
            if op.infallible {
                // Infallible: value returned directly (scalar/str/enum) or a
                // single out-param (compound); no err_out.
                match &ret {
                    Cross::Void => format!("void {sym}({});\n", join(lead)),
                    Cross::I32
                    | Cross::I64
                    | Cross::F64
                    | Cross::Bool
                    | Cross::Enum(_)
                    | Cross::Str
                    | Cross::StrEnum
                    | Cross::Union => {
                        format!("{} {sym}({});\n", c_val(&ret), join(lead))
                    }
                    _ => {
                        let mut parts = lead;
                        parts.extend(c_out_params(&ret));
                        format!("void {sym}({});\n", join(parts))
                    }
                }
            } else {
                let mut parts = lead;
                parts.extend(c_out_params(&ret));
                parts.push("char** err_out".into());
                format!("int {sym}({});\n", join(parts))
            }
        }
    }
}
