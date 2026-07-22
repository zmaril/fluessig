//! The Magnus (Ruby) template grid — one language's projection of the op shapes.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.

use genco::prelude::*;

use crate::api::{ApiDoc, ApiOp, ApiType, ApiUnion, CallbackSig, Shape};

use super::*;

/// This backend's language slug — the key it reads out of every symbol's
/// `bindings` map via the shared [`pinned_name`] / [`variant_token`] resolver.
/// Ruby hardcodes no pin; its rename lever is the method-name string it hands
/// `define_method` and the enum `wire()` token.
const LANG: &str = "ruby";

/// Backend options for the Magnus (Ruby) generator. The 3-arg [`ruby_binding`]
/// threads `RubyOptions::default()`, whose [`UnionProjection::default`] is
/// structured tagged-object projection; pass [`UnionProjection::Envelope`] to opt
/// back into the historical JSON-string carrier.
#[derive(Default, Clone)]
pub struct RubyOptions {
    /// How union return values and nested union DTO fields are lowered.
    pub union_projection: UnionProjection,
}

/// A union eligible for structured projection: at least two variants, all of
/// which are model refs. Magnus imposes no upper arity cap (the tagged variants
/// ride a plain Rust enum that lowers to the matched wrapped class), so any such
/// union projects; a mixed or degenerate union falls back to the JSON envelope.
fn rb_structured_union<'a>(api: &'a ApiDoc, name: &str) -> Option<&'a ApiUnion> {
    api.unions.iter().find(|u| u.name == name).filter(|u| {
        u.variants.len() >= 2
            && u.variants
                .iter()
                .all(|v| matches!(&v.ty, ApiType::Model { .. }))
    })
}

/// The ruby `(rust, _)` spelling of a type, applying structured union projection
/// when [`RubyOptions::union_projection`] asks for it: a union lowers to its
/// generated `{Union}Union` enum (an `IntoValue` wrapper over the per-variant
/// wrapped classes). Delegates to the shared [`ty`] for everything else, so
/// envelope mode is byte-identical to the historical output.
fn ruby_ty(api: &ApiDoc, opts: &RubyOptions, t: &ApiType) -> (String, String) {
    match (t, &opts.union_projection) {
        (ApiType::Union { union }, UnionProjection::Structured { .. }) => {
            match rb_structured_union(api, union) {
                Some(u) => {
                    let n = union_enum_name(&u.name);
                    (n.clone(), n)
                }
                None => ty(api, t),
            }
        }
        (ApiType::List { list }, _) => {
            let (r, s) = ruby_ty(api, opts, list);
            (format!("Vec<{r}>"), format!("{s}[]"))
        }
        (ApiType::Nullable { nullable }, _) => {
            let (r, s) = ruby_ty(api, opts, nullable);
            (format!("Option<{r}>"), format!("{s} | null"))
        }
        _ => ty(api, t),
    }
}

/// Ruby's `<Interface>Core` traits: the shared [`emit_core_traits_with`] spine
/// driven with ruby's structured return mapping ([`ruby_ty`]) so a
/// union-returning op's core-trait signature matches the wrapped method return
/// (`{Union}Union`). In envelope mode `ruby_ty` delegates to `ty`, so the output
/// is byte-identical to the historical default.
fn emit_core_traits_ruby(t: &mut rust::Tokens, api: &ApiDoc, opts: &RubyOptions) {
    emit_core_traits_with(t, api, |op| ruby_ty(api, opts, &op.returns).0);
}

/// The models an op surface RETURNS (directly, in lists, or nullable) — these
/// get Ruby classes with getters; input bags are flattened away instead.
fn output_models(api: &ApiDoc) -> Vec<String> {
    fn walk(t: &ApiType, out: &mut Vec<String>) {
        match t {
            ApiType::Model { model } => out.push(model.clone()),
            ApiType::List { list } => walk(list, out),
            ApiType::Nullable { nullable } => walk(nullable, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for i in &api.interfaces {
        for op in &i.ops {
            walk(&op.returns, &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The non-string enums that actually cross the Ruby boundary as values and so
/// need IntoValue/TryConvert: those appearing as a field of an OUTPUT model (a
/// getter returns the enum) or as the element of a `List` (an enum-list input
/// param). A scalar enum that only rides an input DTO is flattened to a String
/// and parsed in the prelude, so it needs neither.
fn crossing_enums(api: &ApiDoc) -> std::collections::HashSet<String> {
    let outputs = output_models(api);
    let mut set = std::collections::HashSet::new();
    for m in &api.models {
        let is_output = outputs.contains(&m.name);
        for f in &m.fields {
            match &f.ty {
                ApiType::Enum { r#enum } if is_output => {
                    set.insert(r#enum.clone());
                }
                ApiType::List { list } => {
                    if let ApiType::Enum { r#enum } = &**list {
                        set.insert(r#enum.clone());
                    }
                }
                _ => {}
            }
        }
    }
    for i in &api.interfaces {
        for op in &i.ops {
            for p in &op.params {
                if let ApiType::List { list } = &p.ty {
                    if let ApiType::Enum { r#enum } = &**list {
                        set.insert(r#enum.clone());
                    }
                }
            }
        }
    }
    set
}

/// Ruby flattening: like Python's, plus — enum fields arrive as Strings (parsed
/// in the prelude) and input-model-typed fields (e.g. `rename: TableRename[]`)
/// are not exposed (passed as None; a kwargs follow-up).
struct RbParam {
    name: String,
    rust_ty: String,
    optional: bool,
    /// build-struct group (model name), when flattened.
    group: Option<String>,
    /// original field name inside the group.
    field: Option<String>,
    /// enum name to parse from String in the prelude.
    parse_enum: Option<String>,
    /// callback signature, when this param is an [`ApiType::Callback`] — the Ruby
    /// method takes a `Proc` (spelled in `rust_ty`) and the prelude wraps it into
    /// the uniform core `Box<dyn Fn>` via [`super::ruby_callback::rust_callback_conv`].
    callback: Option<CallbackSig>,
}

/// A flattened group: (model, var, skipped-fields).
type RbGroup = (String, String, Vec<String>);

fn rb_flatten(api: &ApiDoc, op: &ApiOp) -> (Vec<RbParam>, Vec<RbGroup>) {
    // returns (params, groups: (model, var, skipped-fields))
    let mut params = Vec::new();
    let mut groups = Vec::new();
    for p in &op.params {
        let model_name = match &p.ty {
            ApiType::Model { model } => Some(model.clone()),
            _ => None,
        };
        if let Some(model) = model_name {
            let m = api
                .models
                .iter()
                .find(|m| m.name == model)
                .expect("model in api.json");
            let mut skipped = Vec::new();
            for f in &m.fields {
                let is_input_model = match &f.ty {
                    ApiType::Model { .. } => true,
                    ApiType::List { list } => matches!(**list, ApiType::Model { .. }),
                    _ => false,
                };
                if is_input_model {
                    skipped.push(snake(&f.name));
                    continue;
                }
                let (enum_name, base_ty) = match &f.ty {
                    ApiType::Enum { r#enum } if !is_string_enum(api, r#enum) => {
                        (Some(r#enum.clone()), "String".to_string())
                    }
                    other => (None, ty(api, other).0),
                };
                params.push(RbParam {
                    name: snake(&f.name),
                    rust_ty: if f.nullable {
                        format!("Option<{base_ty}>")
                    } else {
                        base_ty
                    },
                    optional: f.nullable,
                    group: Some(model.clone()),
                    field: Some(snake(&f.name)),
                    parse_enum: enum_name,
                    callback: None,
                });
            }
            groups.push((model.clone(), format!("{}_arg", snake(&model)), skipped));
        } else if let ApiType::Callback { callback } = &p.ty {
            // A callback param: the Ruby method takes a `Proc`; the prelude wraps
            // it into the uniform core `Box<dyn Fn>` (shadowing `{name}`), which
            // flows straight into the core call. (Never optional in pi's surface.)
            params.push(RbParam {
                name: snake(&p.name),
                rust_ty: "magnus::block::Proc".to_string(),
                optional: false,
                group: None,
                field: None,
                parse_enum: None,
                callback: Some(callback.clone()),
            });
        } else {
            let (r, _) = ty(api, &p.ty);
            let optional = p.optional == Some(true);
            params.push(RbParam {
                name: snake(&p.name),
                rust_ty: if optional { format!("Option<{r}>") } else { r },
                optional,
                group: None,
                field: None,
                parse_enum: None,
                callback: None,
            });
        }
    }
    (params, groups)
}

/// List returns cross into Ruby as RArray (magnus has no blanket Vec<Wrapped> impl).
fn rb_is_list_return(op: &ApiOp) -> bool {
    matches!(op.returns, ApiType::List { .. })
}

struct RbPieces {
    fn_params: String,
    arity: i64,
    prelude: String,
    args: String,
    /// scan_args destructuring lines, when the op has optional params (variadic).
    scan: Option<String>,
}

fn rb_op_pieces(api: &ApiDoc, op: &ApiOp) -> RbPieces {
    let (flat, groups) = rb_flatten(api, op);
    let has_optional = flat.iter().any(|p| p.optional);
    let fn_params = if has_optional {
        "args: &[magnus::Value]".to_string()
    } else {
        flat.iter()
            .map(|p| format!("{}: {}", p.name, p.rust_ty))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let arity: i64 = if has_optional { -1 } else { flat.len() as i64 };
    let scan = if has_optional {
        let req: Vec<&RbParam> = flat.iter().filter(|p| !p.optional).collect();
        let opt: Vec<&RbParam> = flat.iter().filter(|p| p.optional).collect();
        // scan_args handles the common case, but two shapes need positional
        // extraction by hand: an op with >9 optionals (scan_args' tuples cap at
        // 9 — disponent's DispatchSpec) and a ctor with optionals (a caller
        // passes `nil` to skip an earlier optional and set a later one, e.g.
        // `open(nil, "none")`, which scan_args rejects). Manual extraction maps
        // a missing-or-nil arg to `None` and is a superset of the scan_args form.
        if opt.len() > 9 || op.shape == Shape::Ctor {
            let mut out = String::new();
            for (i, p) in req.iter().enumerate() {
                out.push_str(&format!(
                    "let {n}: {ty} = magnus::TryConvert::try_convert(args.get({i}).copied().ok_or_else(|| rberr(\"wrong number of arguments\"))?)?;\n",
                    n = p.name, ty = p.rust_ty
                ));
            }
            for (j, p) in opt.iter().enumerate() {
                let i = req.len() + j;
                out.push_str(&format!(
                    "let {n}: {ty} = match args.get({i}).copied() {{ Some(v) if !v.is_nil() => Some(magnus::TryConvert::try_convert(v)?), _ => None }};\n",
                    n = p.name, ty = p.rust_ty
                ));
            }
            Some(out)
        } else {
            let req_tys = req
                .iter()
                .map(|p| p.rust_ty.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let opt_tys = opt
                .iter()
                .map(|p| p.rust_ty.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let req_names = req
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let opt_names = opt
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let req_tuple = if req.is_empty() {
                "()".to_string()
            } else {
                format!("({req_tys},)")
            };
            let mut out = format!(
                "let a = magnus::scan_args::scan_args::<{req_tuple}, ({opt_tys},), (), (), (), ()>(args)?;\n"
            );
            if !req.is_empty() {
                out.push_str(&format!("let ({req_names},) = a.required;\n"));
            }
            out.push_str(&format!("let ({opt_names},) = a.optional;\n"));
            Some(out)
        }
    } else {
        None
    };
    let mut prelude = String::new();
    // A callback param is wrapped into the uniform core `Box<dyn Fn>` first (it
    // shadows `{name}`, which then flows into the core call); no group/enum
    // marshaling touches it.
    for p in flat.iter().filter(|p| p.callback.is_some()) {
        prelude.push_str(&super::ruby_callback::rust_callback_conv(
            api,
            p.callback.as_ref().unwrap(),
            &p.name,
        ));
        prelude.push('\n');
    }
    for p in flat.iter().filter(|p| p.parse_enum.is_some()) {
        let e = p.parse_enum.as_ref().unwrap();
        if p.optional {
            // an optional enum arrives as Option<String>: parse the inner value.
            prelude.push_str(&format!(
                "let {n} = {n}.map(|s| {e}::parse(&s)).transpose().map_err(rberr)?;\n",
                n = p.name
            ));
        } else {
            prelude.push_str(&format!(
                "let {n} = {e}::parse(&{n}).map_err(rberr)?;\n",
                n = p.name
            ));
        }
    }
    for (model, var, skipped) in &groups {
        let mut fields: Vec<String> = flat
            .iter()
            .filter(|p| p.group.as_deref() == Some(model))
            .map(|p| p.field.clone().unwrap())
            .collect();
        fields.extend(skipped.iter().map(|f| format!("{f}: None")));
        prelude.push_str(&format!(
            "let {var} = {model} {{ {} }};\n",
            fields.join(", ")
        ));
    }
    let args = op
        .params
        .iter()
        .map(|p| match &p.ty {
            ApiType::Model { model } => {
                let var = format!("{}_arg", snake(model));
                if p.optional == Some(true) {
                    format!("Some({var})")
                } else {
                    var
                }
            }
            _ => snake(&p.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    RbPieces {
        fn_params,
        arity,
        prelude,
        args,
        scan,
    }
}

/// Emit, for every structurally-projected union, one `#[magnus::wrap]` class per
/// variant (the discriminant as a `type` getter set to the literal plus getters
/// for the variant model's fields, and a `From<VariantModel>` conversion), then
/// the `{Union}Union` enum wrapping them with an `IntoValue` that lowers to the
/// matched wrapped class. Pushes the class + method registrations onto `regs`.
/// Nothing is emitted in envelope mode.
fn emit_rb_union_variants(
    t: &mut rust::Tokens,
    api: &ApiDoc,
    opts: &RubyOptions,
    module: &str,
    regs: &mut Vec<String>,
) {
    let UnionProjection::Structured { tag_field } = &opts.union_projection else {
        return;
    };
    for u in &api.unions {
        let Some(u) = rb_structured_union(api, &u.name) else {
            quote_in! { *t =>
                $['\r']
                $(format!("// note: union {} is not structurally projectable (needs >=2 model-ref variants) — kept as the JSON envelope carrier.", u.name))
            };
            continue;
        };
        let field = union_tag_field(u, tag_field);
        let ident = tag_ident(&field);
        let tag_getter = format!("get_{}", snake(&field));
        let mut arms: Vec<String> = Vec::new();
        for v in &u.variants {
            let sname = tagged_variant_name(&u.name, &v.tag);
            arms.push(format!(
                "Self::{}(v) => v.into_value_with(ruby),",
                pascal(&v.tag)
            ));
            let ApiType::Model { model } = &v.ty else {
                continue;
            };
            let Some(m) = api.models.iter().find(|m| &m.name == model) else {
                continue;
            };
            // struct fields: the tag first, then the variant model's real fields
            let mut struct_fields: Vec<rust::Tokens> = Vec::new();
            struct_fields.push(quote!($(format!("pub {ident}: String,"))));
            let mut getters: Vec<rust::Tokens> = Vec::new();
            getters.push(quote! {
                fn $(&tag_getter)(&self) -> String {
                    self.$(&ident).clone()
                }
            });
            let mut from_fields: Vec<String> = Vec::new();
            from_fields.push(format!("{ident}: {:?}.into(),", v.tag));
            // register the tag getter (Ruby method name = the discriminant field)
            regs.push(format!(
                "let c = class.define_class({:?}, ruby.class_object())?;",
                sname
            ));
            regs.push(format!(
                "c.define_method({:?}, method!({sname}::{tag_getter}, 0))?;",
                field
            ));
            for f in &m.fields {
                let (r, _) = ruby_ty(api, opts, &f.ty);
                let r = if f.nullable {
                    format!("Option<{r}>")
                } else {
                    r
                };
                let fname = snake(&f.name);
                struct_fields.push(quote!($(format!("pub {fname}: {r},"))));
                getters.push(quote! {
                    fn get_$(&fname)(&self) -> $(&r) {
                        self.$(&fname).clone()
                    }
                });
                from_fields.push(format!("{fname}: v.{fname},"));
                regs.push(format!(
                    "c.define_method({fname:?}, method!({sname}::get_{fname}, 0))?;"
                ));
            }
            quote_in! { *t =>
                $['\r']
                $(format!("/// `{}` union variant `{}` — the tag `{}` rides as the `{}` getter's literal.", u.name, v.tag, v.tag, field))
                #[magnus::wrap(class = $(quoted(format!("{module}::{sname}"))), free_immediately, size)]
                #[derive(Clone)]
                pub struct $(&sname) {
                    $(for f in &struct_fields join ($['\r']) => $f)
                }
                impl $(&sname) {
                    $(for g in &getters join ($['\r']) => $g)
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
        let enum_name = union_enum_name(&u.name);
        quote_in! { *t =>
            $['\r']
            $(format!("/// The `{}` tagged union — its `IntoValue` lowers to the matched variant's", u.name))
            $("/// wrapped Ruby class (a tagged object carrying the discriminant getter).")
            #[derive(Clone)]
            pub enum $(&enum_name) {
                $(for v in &u.variants join ($['\r']) => $(format!("{}({}),", pascal(&v.tag), tagged_variant_name(&u.name, &v.tag))))
            }
            impl magnus::IntoValue for $(&enum_name) {
                fn into_value_with(self, ruby: &magnus::Ruby) -> magnus::Value {
                    match self {
                        $(for a in &arms join ($['\r']) => $a)
                    }
                }
            }
            $['\n']
        };
    }
}

/// Generate the Magnus (Ruby) binding with default options: structured
/// tagged-object union projection (per-variant `#[magnus::wrap]` classes wrapped
/// in a `{Union}Union` enum, tag field `"type"`). A thin wrapper over
/// [`ruby_binding_with_options`]; pass [`UnionProjection::Envelope`] to opt into
/// the JSON-string carrier.
pub fn ruby_binding(api: &ApiDoc, enums: &[EnumDesc], banner_note: Option<&str>) -> String {
    ruby_binding_with_options(api, enums, banner_note, &RubyOptions::default())
}

/// Generate the Magnus (Ruby) binding: plain-Rust DTOs + enums (with parse),
/// wrapped output classes with getters, GVL-plain methods with trailing
/// optionals, `.next`-nil streams, and a `register()` for `#[magnus::init]`.
/// `opts` selects union projection (structured wrapped classes vs. the JSON
/// envelope).
pub fn ruby_binding_with_options(
    api: &ApiDoc,
    enums: &[EnumDesc],
    banner_note: Option<&str>,
    opts: &RubyOptions,
) -> String {
    // A `single_threaded` interface is a thread-confined `!Send` handle — node-only
    // today; the Magnus glue holds the core in a `Send`-requiring wrapper, so ruby
    // cannot bind a `!Send` core. Split it out (emit nothing) + append an honest
    // skip-note rather than a silent `Send`-assuming handle. None ⇒ byte-identical.
    let (api_owned, st_note) = crate::bindgen::split_single_threaded(api, "ruby");
    let api = &api_owned;
    let outputs = output_models(api);
    // The Ruby module/root class name: the stateful (ctor-bearing) interface's
    // name — the class the DTO classes nest under and stateless interfaces hang
    // their singleton methods on. entl → "Entl"; disponent → "Disponent".
    let module = api
        .interfaces
        .iter()
        .find(|i| i.ops.iter().any(|o| o.shape == Shape::Ctor))
        .or_else(|| api.interfaces.first())
        .map(|i| i.name.clone())
        .unwrap_or_else(|| "Root".to_string());
    let gvl_panic = format!("{} called outside the Ruby GVL", module.to_lowercase());
    // Does any op project a `stream` class? Only then do the blocking `poll`
    // sites (and so the `without_gvl` GVL-release helper) exist.
    let has_stream = api
        .interfaces
        .iter()
        .any(|i| i.ops.iter().any(|o| o.shape == Shape::Stream));
    // The shared streaming-contract import flows through the use-emitter
    // ([`RUNTIME_STREAM_IMPORT`]) rather than a hardcoded string, so every
    // generated `use` line has one emission path; renders byte-identically.
    let runtime_import = RUNTIME_STREAM_IMPORT.render();
    let mut t: rust::Tokens = quote! {
        use std::sync::Arc;
        use std::time::Duration;
        use magnus::{function, method, prelude::*, Error, Ruby};
        $("// The shared streaming contract — Poll/PollStream live in the fluessig-runtime crate.")
        $(&runtime_import)

        fn rberr(e: impl std::fmt::Display) -> Error {
            let ruby = magnus::Ruby::get().expect($(quoted(&gvl_panic)));
            Error::new(ruby.exception_runtime_error(), e.to_string())
        }
    };
    if api_uses_bytes(api) {
        quote_in! { t =>
            $['\n']
            $("/// Bulk bytes cross into Ruby as a binary String (via magnus's `bytes` feature).")
            pub type Bytes = bytes::Bytes;
        };
    }
    if has_stream {
        // The blocking `PollStream::poll` at every stream site runs with the Ruby
        // GVL released via `rb_thread_call_without_gvl` — so an idling/blocking
        // stream does not stall the whole Ruby VM (verified against ruby 3.3.6: a
        // background thread advanced during a blocking `each`). `rb-sys` exposes
        // this at the top level (magnus does NOT re-export it), so the CONSUMER
        // crate must depend on `rb-sys` (~0.9) directly, alongside magnus. See
        // notes/async-iterable-streams-ruby.md.
        quote_in! { t =>
            $['\n']
            use std::ffi::c_void;
            use std::ptr;
            $['\n']
            $("/// Run `func` with the Ruby GVL released; returns its result once the GVL is")
            $("/// re-acquired. `func` MUST NOT touch any Ruby object (no Value/alloc) — extract")
            $("/// the poll result here, act on it (yield/raise) only after this returns.")
            fn without_gvl<F, R>(func: F) -> R
            where
                F: FnOnce() -> R, $("// no Send bound: runs on the same OS thread")
            {
                unsafe extern "C" fn trampoline<F, R>(data: *mut c_void) -> *mut c_void
                where
                    F: FnOnce() -> R,
                {
                    let slot = &mut *(data as *mut Option<F>);
                    let f = slot.take().expect("gvl closure already consumed");
                    Box::into_raw(Box::new(f())) as *mut c_void
                }
                let mut slot: Option<F> = Some(func);
                let result_ptr = unsafe {
                    rb_sys::rb_thread_call_without_gvl(
                        Some(trampoline::<F, R>),
                        &mut slot as *mut Option<F> as *mut c_void,
                        None,
                        ptr::null_mut(),
                    )
                };
                *unsafe { Box::from_raw(result_ptr as *mut R) }
            }
        };
    }
    t.line();

    // ── enums: plain Rust + parse-from-string (Ruby passes lowercase names) ──
    let crossing = crossing_enums(api);
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        // The wire token comes from the shared resolver (a `ruby` pin wins, then
        // the neutral `Variant.value`, else `to_lowercase()`); un-pinned is
        // exactly the old `to_lowercase()`, so the emission is byte-identical.
        let vs: Vec<String> = variants.iter().map(|v| variant_ident(&v.name)).collect();
        let arms: Vec<String> = variants
            .iter()
            .map(|v| {
                format!(
                    "{:?} => Ok(Self::{}),",
                    variant_token(v, LANG),
                    variant_ident(&v.name)
                )
            })
            .collect();
        let expect = variants
            .iter()
            .map(|v| variant_token(v, LANG))
            .collect::<Vec<_>>()
            .join(" | ");
        quote_in! { t =>
            $['\n']
            #[derive(Clone, Copy, PartialEq)]
            pub enum $name {
                $(for v in &vs join ($['\r']) => $v,)
            }
            impl $name {
                pub fn parse(s: &str) -> anyhow::Result<Self> {
                    match s.to_ascii_lowercase().as_str() {
                        $(for a in &arms join ($['\r']) => $a)
                        other => Err(anyhow::anyhow!($(quoted(format!("unknown {name}: {{other}} (expected {expect})")))))
                    }
                }
            }
        };
        // Only enums that cross as values (an output field, or an enum-list
        // input) get the value codecs; a scalar enum on an input DTO is passed
        // as a String and parsed in the prelude, so it needs none of this.
        if crossing.contains(name) {
            let wire_arms: Vec<String> = variants
                .iter()
                .map(|v| {
                    format!(
                        "Self::{} => {:?},",
                        variant_ident(&v.name),
                        variant_token(v, LANG)
                    )
                })
                .collect();
            quote_in! { t =>
                $['\r']
                impl $name {
                    pub fn wire(&self) -> &$("'static") str {
                        match self {
                            $(for a in &wire_arms join ($['\r']) => $a)
                        }
                    }
                }
                $("/// A getter returning this enum hands Ruby its wire string.")
                impl magnus::IntoValue for $name {
                    fn into_value_with(self, ruby: &magnus::Ruby) -> magnus::Value {
                        ruby.str_new(self.wire()).as_value()
                    }
                }
                impl magnus::TryConvert for $name {
                    fn try_convert(val: magnus::Value) -> Result<Self, magnus::Error> {
                        Self::parse(&<String as magnus::TryConvert>::try_convert(val)?).map_err(rberr)
                    }
                }
                $("// SAFETY: the enum owns its data (a Copy discriminant) — no borrow from")
                $("// the Ruby value survives, so it is sound in owning positions like")
                $("// `Vec<Self>` (an enum-list input param).")
                unsafe impl magnus::try_convert::TryConvertOwned for $name {}
            };
        }
    }
    t.line();

    // ── DTO structs: plain Rust; output models get wrapped Ruby classes + getters ──
    for m in &api.models {
        let is_output = outputs.contains(&m.name);
        if let Some(doc) = &m.doc {
            for line in doc.lines() {
                quote_in! { t => $['\r']$(format!("/// {line}")) };
            }
        }
        if let Some(af) = arrow_field(m) {
            // Arrow-payload DTO: the wrapped class holds the RecordBatch; the
            // payload getter encodes to IPC bytes only when called.
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
                        fn get_$(&n)(&self) -> $r {
                            self.$(&n).clone()
                        }
                    }
                })
                .collect();
            let ipc = snake(&af.name);
            quote_in! { t =>
                $['\r']
                #[magnus::wrap(class = $(quoted(format!("{module}::{}", m.name))), free_immediately, size)]
                #[derive(Clone)]
                pub struct $(&m.name) {
                    $(for f in &storage join ($['\r']) => $f)
                    $("// the rows, still columnar — encoded only when the getter is hit")
                    pub(crate) batch: entl_core::RecordBatch,
                }
                impl $(&m.name) {
                    $(for g in &getters join ($['\r']) => $g)
                    $("/// The rows as one Arrow IPC stream, as a binary String (red-arrow decodes it).")
                    fn get_$(&ipc)(&self) -> Result<Bytes, Error> {
                        Ok(Bytes::from(entl_core::batch_ipc(&self.batch).map_err(rberr)?))
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
                let (r, _) = ty(api, &f.ty);
                let r = if f.nullable {
                    format!("Option<{r}>")
                } else {
                    r
                };
                let n = snake(&f.name);
                quote!(pub $n: $r,)
            })
            .collect();
        if is_output {
            // output models are the ones that carry union-typed fields as tagged
            // objects — project their storage + getters via `ruby_ty` (input bags
            // never reach here; they stay the envelope `fields` below).
            let out_fields: Vec<rust::Tokens> = m
                .fields
                .iter()
                .map(|f| {
                    let (r, _) = ruby_ty(api, opts, &f.ty);
                    let r = if f.nullable {
                        format!("Option<{r}>")
                    } else {
                        r
                    };
                    let n = snake(&f.name);
                    quote!(pub $n: $r,)
                })
                .collect();
            let getters: Vec<rust::Tokens> = m
                .fields
                .iter()
                .map(|f| {
                    let (r, _) = ruby_ty(api, opts, &f.ty);
                    let r = if f.nullable {
                        format!("Option<{r}>")
                    } else {
                        r
                    };
                    let n = snake(&f.name);
                    quote! {
                        fn get_$(&n)(&self) -> $r {
                            self.$(&n).clone()
                        }
                    }
                })
                .collect();
            quote_in! { t =>
                $['\r']
                #[magnus::wrap(class = $(quoted(format!("{module}::{}", m.name))), free_immediately, size)]
                #[derive(Clone)]
                pub struct $(&m.name) {
                    $(for f in &out_fields join ($['\r']) => $f)
                }
                impl $(&m.name) {
                    $(for g in &getters join ($['\r']) => $g)
                }
                $['\n']
            };
        } else {
            quote_in! { t =>
                $['\r']
                #[derive(Clone)]
                pub struct $(&m.name) {
                    $(for f in &fields join ($['\r']) => $f)
                }
                $['\n']
            };
        }
    }

    // ── the surface ──
    let mut registrations: Vec<String> = Vec::new();

    // per-variant wrapped classes (+ the {Union}Union enum) for structured unions
    emit_rb_union_variants(&mut t, api, opts, &module, &mut registrations);

    emit_core_traits_ruby(&mut t, api, opts);

    // The RubyCb callback wrapper (whenever a callback param appears; a
    // subscription op always carries one) + the opaque Subscription handle class.
    // Both gated, so a callback/subscription-free schema stays byte-identical.
    if crate::bindgen::api_uses_callback(api) {
        quote_in! { t =>
            $['\r']
            $(super::ruby_callback::RUBY_CALLBACK_PRELUDE)
        };
    }
    if crate::bindgen::api_uses_subscription(api) {
        quote_in! { t =>
            $['\r']
            $(super::ruby_callback::subscription_ruby_prelude(&module))
        };
        registrations.push(
            "let subscription = class.define_class(\"Subscription\", ruby.class_object())?;"
                .to_string(),
        );
        registrations.push(
            "subscription.define_method(\"unsubscribe\", method!(Subscription::unsubscribe, 0))?;"
                .to_string(),
        );
    }

    for m in &api.models {
        if outputs.contains(&m.name) {
            registrations.push(format!(
                "let c = class.define_class({:?}, ruby.class_object())?;",
                m.name
            ));
            for f in &m.fields {
                // The internal Rust getter is always `get_{snake}`; the
                // Ruby-visible method name is a `ruby` pin when present, else the
                // default `snake` (byte-identical un-pinned).
                let n = snake(&f.name);
                let rb = pinned_name(&f.bindings, LANG).unwrap_or_else(|| n.clone());
                registrations.push(format!(
                    "c.define_method({rb:?}, method!({}::get_{n}, 0))?;",
                    m.name
                ));
            }
        }
    }

    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        let trait_name = format!("{}Core", i.name);
        let impl_path = format!("crate::core_impl::{}Impl", i.name);

        // stream classes: an idiomatic Ruby `each` (yields each event to a block,
        // returns an Enumerator when called with NO block) alongside the retained
        // `.next` poll cursor. The error model is chosen per-op by `stream_error`,
        // mirroring node/python: `None` (unannotated) = throw-mode — a mid-stream
        // `Poll::Failed` RAISES out of `each`; `Some(shape)` = error-AS-EVENT — the
        // failure is yielded as a terminal `<Class>ErrorEvent` then the block ENDS,
        // NEVER raising. `Poll::Failed(String)` is the core→binding channel in BOTH
        // modes; only the `each`-loop terminal arm differs.
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            let (item, _) = ruby_ty(api, opts, &op.returns);
            registrations.push(format!(
                "let s = class.define_class({class:?}, ruby.class_object())?;"
            ));
            registrations.push(format!(
                "s.define_method(\"next\", method!({class}::next, 0))?;"
            ));
            registrations.push(format!(
                "s.define_method(\"each\", method!({class}::each, 0))?;"
            ));

            // The `each` loop's terminal-failure arm is the ONLY thing that differs
            // by error model (mirrors node/python's `match &op.stream_error`).
            let each_failed: rust::Tokens = match &op.stream_error {
                // ── DEFAULT throw-mode (unannotated): raise on Poll::Failed ──
                None => quote! {
                    Poll::Failed(e) => return Err(rberr(e)), $("// throw-mode: raises in Ruby")
                },
                // ── OPT-IN event-mode (@streamError): error-as-event ──
                Some(se) => {
                    let err_evt = format!("{class}ErrorEvent");
                    quote! {
                        Poll::Failed(e) => {
                            $("// error-AS-EVENT: hand the failure out as the terminal event,")
                            $("// then END the block — NEVER raise (mirrors node/python's")
                            $("// `@streamError` contract). A started stream is done once its")
                            $("// terminal event has been yielded, so `break` ends iteration.")
                            let _: magnus::Value = ruby.yield_value($(&err_evt) {
                                type_: $(quoted(se.tag_value.clone())).into(),
                                reason: "error".into(),
                                error: e,
                            })?;
                            break;
                        }
                    }
                }
            };

            // event-mode: emit + register the terminal error-event wrap class. Its
            // three String fields carry `{ tag_name: tag_value, reason: "error",
            // error: e }`; the Ruby-visible getter names come from the schema
            // (`se.tag_name` / `se.reason_name` / `se.error_name`) via `define_method`.
            if let Some(se) = &op.stream_error {
                let err_evt = format!("{class}ErrorEvent");
                registrations.push(format!(
                    "let ev = class.define_class({err_evt:?}, ruby.class_object())?;"
                ));
                registrations.push(format!(
                    "ev.define_method({:?}, method!({err_evt}::get_type_, 0))?;",
                    se.tag_name
                ));
                registrations.push(format!(
                    "ev.define_method({:?}, method!({err_evt}::get_reason, 0))?;",
                    se.reason_name
                ));
                registrations.push(format!(
                    "ev.define_method({:?}, method!({err_evt}::get_error, 0))?;",
                    se.error_name
                ));
                quote_in! { t =>
                    $['\r']
                    $(format!("/// The terminal error event yielded (NEVER raised) when `{}.{}`'s core stream", i.name, op.name))
                    $("/// fails after it has started — the opt-in `@streamError` (error-as-event)")
                    $("/// model. A read-only carrier for a `Poll::Failed`; normal typed error")
                    $("/// variants ride out through `Poll::Item` and need no such class.")
                    #[magnus::wrap(class = $(quoted(format!("{module}::{err_evt}"))), free_immediately, size)]
                    #[derive(Clone)]
                    pub struct $(&err_evt) {
                        pub type_: String,
                        pub reason: String,
                        pub error: String,
                    }
                    impl $(&err_evt) {
                        fn get_type_(&self) -> String {
                            self.type_.clone()
                        }
                        fn get_reason(&self) -> String {
                            self.reason.clone()
                        }
                        fn get_error(&self) -> String {
                            self.error.clone()
                        }
                    }
                    $['\n']
                };
            }

            quote_in! { t =>
                $['\r']
                $(format!("/// Poll-based stream from `{}.{}`.", i.name, op.name))
                $("///")
                $("/// Primary surface: `each` — yields each event to a block, and returns an")
                $("/// `Enumerator` when called with NO block (so `.lazy`/`.map`/`.next` compose).")
                $("/// Retained surface: the `.next` poll cursor (fallible since P1) for consumers")
                $("/// that want an explicit pull rather than a block.")
                #[magnus::wrap(class = $(quoted(format!("{module}::{class}"))), free_immediately, size)]
                pub struct $(&class) {
                    stream: Box<dyn PollStream<$(&item)>>,
                }
                impl $(&class) {
                    $("// A terminal `Poll::Failed` raises a Ruby RuntimeError (mirrors node's")
                    $("// default throw-mode): the sync `.next` cursor has no error-as-event")
                    $("// surface, so a mid-stream core failure surfaces as `rberr(e)`.")
                    fn next(&self) -> Result<Option<$(&item)>, Error> {
                        loop {
                            $("// GVL released around the blocking poll; the `Poll<item>` is a")
                            $("// pure-Rust value, so no Ruby object is touched while released.")
                            let poll = without_gvl(|| self.stream.poll(Duration::from_millis(500)));
                            match poll {
                                Poll::Item(v) => return Ok(Some(v)),
                                Poll::Idle => continue,
                                Poll::Closed => return Ok(None), $("// nil ends iteration")
                                Poll::Failed(e) => return Err(rberr(e)), $("// raises on failure")
                            }
                        }
                    }
                    $("// Idiomatic streaming surface. With a block: yield each event, skipping")
                    $("// idle polls and ending at `Poll::Closed`. With NO block: return an")
                    $("// `Enumerator` over `each`, so `.lazy`/`.map`/`.next` compose (Ruby >= 3.1")
                    $("// for an Enumerator built from a yielding method). `Obj<Self>` is the")
                    $("// receiver so `enumeratorize` has the Ruby self value and the field is")
                    $("// still reachable via Deref. The blocking `poll` runs with the GVL")
                    $("// RELEASED (via `without_gvl` → `rb_thread_call_without_gvl`), so an")
                    $("// idling/blocking stream does not stall other Ruby threads; the Ruby")
                    $("// ops (yield/raise) stay OUTSIDE the released region. See")
                    $("// notes/async-iterable-streams-ruby.md.")
                    fn each(ruby: &Ruby, rb_self: magnus::typed_data::Obj<Self>) -> Result<magnus::Value, Error> {
                        $("// No block => hand back an Enumerator so `.lazy`/`.map`/`.next` work.")
                        if !ruby.block_given() {
                            return Ok(rb_self.enumeratorize("each", ()).as_value());
                        }
                        loop {
                            $("// GVL released around the blocking poll; the `Poll<item>` is a")
                            $("// pure-Rust value, so no Ruby object is touched while released.")
                            $("// yield_value / the ErrorEvent yield / the raise all run AFTER.")
                            let poll = without_gvl(|| rb_self.stream.poll(Duration::from_millis(500)));
                            match poll {
                                Poll::Item(v) => {
                                    let _: magnus::Value = ruby.yield_value(v)?;
                                }
                                Poll::Idle => continue,
                                Poll::Closed => break,
                                $each_failed
                            }
                        }
                        $("// `each` returns the receiver, like `Array#each`.")
                        Ok(rb_self.as_value())
                    }
                }
                $("// Backstop: an early `break` out of the block leaves the stream")
                $("// un-exhausted, so `Drop` guarantees the core stream is closed and its")
                $("// resources released (the `close()` default is an idempotent no-op).")
                impl Drop for $(&class) {
                    fn drop(&mut self) {
                        self.stream.close();
                    }
                }
                $['\n']
            };
        }

        if has_ctor {
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
                let p = rb_op_pieces(api, op);
                let (fn_params, arity) = (p.fn_params, p.arity);
                let prelude = format!("{}{}", p.scan.unwrap_or_default(), p.prelude);
                let args = p.args;
                let (ret, _) = ruby_ty(api, opts, &op.returns);
                match op.shape {
                    Shape::Ctor => {
                        registrations.push(format!(
                            "class.define_singleton_method(\"new\", function!({}::new, {arity}))?;",
                            i.name
                        ));
                        quote_in! { methods =>
                            $['\r']
                            fn new($(&fn_params)) -> Result<Self, Error> {
                                $prelude
                                Ok(Self { core: Arc::new(<$(&impl_path) as $(&trait_name)>::$(&name)($(&args)).map_err(rberr)?) })
                            }
                        }
                    }
                    Shape::Unary => {
                        // An op export-name pin overrides the RUBY method name (the
                        // `define_method` string); the Rust fn ident stays snake.
                        // Un-pinned ⇒ `rb_name == name`, byte-identical. Ruby is
                        // ALREADY synchronous (the async/sync default is a node-only
                        // projection). INFALLIBILITY drops the `Result`/raise seam —
                        // but only cleanly for a zero-marshaling op (no `scan_args`
                        // prelude, non-list): Ruby arg conversion is itself fallible,
                        // so a param'd / list-returning infallible op keeps the
                        // `Result` seam (the CORE call drops its `.map_err`, the
                        // marshaling can still raise) — the honest capability edge.
                        let rb_name =
                            pinned_name(&op.bindings, LANG).unwrap_or_else(|| name.clone());
                        registrations.push(format!(
                            "class.define_method({rb_name:?}, method!({}::{name}, {arity}))?;",
                            i.name
                        ));
                        if op.infallible {
                            if rb_is_list_return(op) {
                                quote_in! { methods =>
                                    $['\r']
                                    fn $(&name)(&self, $(&fn_params)) -> Result<magnus::RArray, Error> {
                                        $prelude
                                        let out = self.core.$(&name)($(&args));
                                        let ruby = Ruby::get().map_err(|e| rberr(e))?;
                                        let ary = ruby.ary_new();
                                        for v in out {
                                            ary.push(v)?;
                                        }
                                        Ok(ary)
                                    }
                                }
                            } else if prelude.trim().is_empty() {
                                quote_in! { methods =>
                                    $['\r']
                                    fn $(&name)(&self, $(&fn_params)) -> $(&ret) {
                                        self.core.$(&name)($(&args))
                                    }
                                }
                            } else {
                                quote_in! { methods =>
                                    $['\r']
                                    fn $(&name)(&self, $(&fn_params)) -> Result<$(&ret), Error> {
                                        $prelude
                                        Ok(self.core.$(&name)($(&args)))
                                    }
                                }
                            }
                        } else if rb_is_list_return(op) {
                            quote_in! { methods =>
                                $['\r']
                                fn $(&name)(&self, $(&fn_params)) -> Result<magnus::RArray, Error> {
                                    $prelude
                                    let out = self.core.$(&name)($(&args)).map_err(rberr)?;
                                    let ruby = Ruby::get().map_err(|e| rberr(e))?;
                                    let ary = ruby.ary_new();
                                    for v in out {
                                        ary.push(v)?;
                                    }
                                    Ok(ary)
                                }
                            }
                        } else {
                            quote_in! { methods =>
                                $['\r']
                                fn $(&name)(&self, $(&fn_params)) -> Result<$(&ret), Error> {
                                    $prelude
                                    self.core.$(&name)($(&args)).map_err(rberr)
                                }
                            }
                        }
                    }
                    Shape::Stream => {
                        let class = pascal(&op.name);
                        registrations.push(format!(
                            "class.define_method({name:?}, method!({}::{name}, {arity}))?;",
                            i.name
                        ));
                        quote_in! { methods =>
                            $['\r']
                            fn $(&name)(&self, $(&fn_params)) -> Result<$(&class), Error> {
                                $prelude
                                Ok($(&class) { stream: self.core.$(&name)($(&args)).map_err(rberr)? })
                            }
                        }
                    }
                    // A subscription op REGISTERS the listener (its one callback
                    // param, wrapped by the `prelude` into the uniform `Box<dyn
                    // Fn>`) via the core, then hands back an owning `Subscription`
                    // handle holding the core's returned unsubscribe closure. The
                    // Ruby method name honours an op export-name pin (`rb_name`);
                    // the Rust fn ident stays snake. Always `&self` (a subscription
                    // op requires a stateful interface — enforced by the loader).
                    Shape::Subscription => {
                        let rb_name =
                            pinned_name(&op.bindings, LANG).unwrap_or_else(|| name.clone());
                        registrations.push(format!(
                            "class.define_method({rb_name:?}, method!({}::{name}, {arity}))?;",
                            i.name
                        ));
                        // Infallible ⇒ the core returns the unsubscribe closure
                        // straight through; fallible ⇒ throw on `Err` (the same
                        // `rberr` seam the unary arms use). The Ruby arg marshaling
                        // (building the `Proc` box) can still raise, so both keep
                        // the `Result<Subscription, Error>` return.
                        let register = if op.infallible {
                            format!("let unsub = self.core.{name}({args});")
                        } else {
                            format!("let unsub = self.core.{name}({args}).map_err(rberr)?;")
                        };
                        quote_in! { methods =>
                            $['\r']
                            fn $(&name)(&self, $(&fn_params)) -> Result<Subscription, Error> {
                                $prelude
                                $(&register)
                                Ok(Subscription { unsub: std::sync::Mutex::new(Some(unsub)) })
                            }
                        }
                    }
                    Shape::Manual => quote_in! { methods =>
                        $['\r']
                        $(format!("// @manual: {} — hand-written in lib.rs if this binding offers it.", op.name))
                    },
                }
            }
            if let Some(doc) = &i.doc {
                for line in doc.lines() {
                    quote_in! { t => $['\r']$(format!("/// {line}")) };
                }
            }
            quote_in! { t =>
                $['\r']
                #[magnus::wrap(class = $(quoted(i.name.as_str())), free_immediately, size)]
                pub struct $(&i.name) {
                    core: Arc<$(&impl_path)>,
                }

                impl $(&i.name) {
                    $methods
                }
                $['\n']
            };
        } else {
            // stateless interface → singleton methods on the Entl class
            for op in &i.ops {
                let name = snake(&op.name);
                if op.shape == Shape::Manual {
                    continue;
                }
                // A subscription op on a ctor-less (factory-born) interface has no
                // stateless singleton form (its `&self` registration needs a
                // receiver). Emit the honest skip-note instead of broken glue.
                if op.shape == Shape::Subscription {
                    quote_in! { t => $['\r']$(super::subscription_factory_skip_note(&i.name, &op.name)) };
                    continue;
                }
                let p = rb_op_pieces(api, op);
                let (fn_params, arity) = (p.fn_params, p.arity);
                let prelude = format!("{}{}", p.scan.unwrap_or_default(), p.prelude);
                let args = p.args;
                let (ret, _) = ruby_ty(api, opts, &op.returns);
                // op export-name pin overrides the RUBY singleton-method name; the
                // Rust fn symbol in `function!` stays snake (see the method arm).
                let rb_name = pinned_name(&op.bindings, LANG).unwrap_or_else(|| name.clone());
                registrations.push(format!(
                    "class.define_singleton_method({rb_name:?}, function!({name}, {arity}))?;"
                ));
                if let Some(doc) = &op.doc {
                    for line in doc.lines() {
                        quote_in! { t => $['\r']$(format!("/// {line}")) };
                    }
                }
                if op.infallible {
                    if rb_is_list_return(op) {
                        quote_in! { t =>
                            $['\r']
                            fn $(&name)($(&fn_params)) -> Result<magnus::RArray, Error> {
                                $prelude
                                let out = <$(&impl_path) as $(&trait_name)>::$(&name)($(&args));
                                let ruby = Ruby::get().map_err(|e| rberr(e))?;
                                let ary = ruby.ary_new();
                                for v in out {
                                    ary.push(v)?;
                                }
                                Ok(ary)
                            }
                            $['\n']
                        };
                    } else if prelude.trim().is_empty() {
                        quote_in! { t =>
                            $['\r']
                            fn $(&name)($(&fn_params)) -> $(&ret) {
                                <$(&impl_path) as $(&trait_name)>::$(&name)($(&args))
                            }
                            $['\n']
                        };
                    } else {
                        quote_in! { t =>
                            $['\r']
                            fn $(&name)($(&fn_params)) -> Result<$(&ret), Error> {
                                $prelude
                                Ok(<$(&impl_path) as $(&trait_name)>::$(&name)($(&args)))
                            }
                            $['\n']
                        };
                    }
                } else if rb_is_list_return(op) {
                    quote_in! { t =>
                        $['\r']
                        fn $(&name)($(&fn_params)) -> Result<magnus::RArray, Error> {
                            $prelude
                            let out = <$(&impl_path) as $(&trait_name)>::$(&name)($(&args)).map_err(rberr)?;
                            let ruby = Ruby::get().map_err(|e| rberr(e))?;
                            let ary = ruby.ary_new();
                            for v in out {
                                ary.push(v)?;
                            }
                            Ok(ary)
                        }
                        $['\n']
                    };
                } else {
                    quote_in! { t =>
                        $['\r']
                        fn $(&name)($(&fn_params)) -> Result<$(&ret), Error> {
                            $prelude
                            <$(&impl_path) as $(&trait_name)>::$(&name)($(&args)).map_err(rberr)
                        }
                        $['\n']
                    };
                }
            }
        }
    }

    quote_in! { t =>
        $['\r']
        $(format!("/// Register the {module} class + every generated method (called from #[magnus::init])."))
        pub fn register(ruby: &Ruby) -> Result<(), Error> {
            let class = ruby.define_class($(quoted(&module)), ruby.class_object())?;
            $(for r in &registrations join ($['\r']) => $r)
            Ok(())
        }
    };

    let src = api.source.as_deref().unwrap_or("the fluessig catalog");
    let body = t.to_file_string().expect("rust renders");
    let out = crate::rustfmt::format(format!(
        "//! GENERATED by fluessig bindgen from {src} (api layer). Do not edit.\n{}#![allow(clippy::all)]\n\n{body}",
        note_line(banner_note)
    ));
    format!("{out}{st_note}")
}
