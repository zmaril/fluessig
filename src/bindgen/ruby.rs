//! The Magnus (Ruby) template grid — one language's projection of the op shapes.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.

use genco::prelude::*;

use crate::api::{ApiDoc, ApiOp, ApiType, Shape};

use super::*;

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
                });
            }
            groups.push((model.clone(), format!("{}_arg", snake(&model)), skipped));
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

/// Generate the Magnus (Ruby) binding: plain-Rust DTOs + enums (with parse),
/// wrapped output classes with getters, GVL-plain methods with trailing
/// optionals, `.next`-nil streams, and a `register()` for `#[magnus::init]`.
pub fn ruby_binding(
    api: &ApiDoc,
    enums: &[(String, Vec<String>)],
    banner_note: Option<&str>,
) -> String {
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
    let mut t: rust::Tokens = quote! {
        use std::sync::Arc;
        use std::time::Duration;
        use magnus::{function, method, prelude::*, Error, Ruby};

        fn rberr(e: impl std::fmt::Display) -> Error {
            let ruby = magnus::Ruby::get().expect($(quoted(&gvl_panic)));
            Error::new(ruby.exception_runtime_error(), e.to_string())
        }

        $("/// One poll result from a core stream (the sync primitive every stream shape dresses).")
        pub enum Poll<T> {
            Item(T),
            Idle,
            Closed,
        }

        $("/// The one sync primitive: a blocking, timeout-bounded poll.")
        pub trait PollStream<T>: Send + Sync {
            fn poll(&self, timeout: Duration) -> Poll<T>;
        }
    };
    if api_uses_bytes(api) {
        quote_in! { t =>
            $['\n']
            $("/// Bulk bytes cross into Ruby as a binary String (via magnus's `bytes` feature).")
            pub type Bytes = bytes::Bytes;
        };
    }
    t.line();

    // ── enums: plain Rust + parse-from-string (Ruby passes lowercase names) ──
    let crossing = crossing_enums(api);
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        let vs: Vec<String> = variants.iter().map(|v| pascal(v)).collect();
        let arms: Vec<String> = variants
            .iter()
            .map(|v| format!("{:?} => Ok(Self::{}),", v.to_lowercase(), pascal(v)))
            .collect();
        let expect = variants
            .iter()
            .map(|v| v.to_lowercase())
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
                .map(|v| format!("Self::{} => {:?},", pascal(v), v.to_lowercase()))
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
            let getters: Vec<rust::Tokens> = m
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
                    $(for f in &fields join ($['\r']) => $f)
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

    emit_core_traits(&mut t, api);

    // ── the surface ──
    let mut registrations: Vec<String> = Vec::new();
    for m in &api.models {
        if outputs.contains(&m.name) {
            registrations.push(format!(
                "let c = class.define_class({:?}, ruby.class_object())?;",
                m.name
            ));
            for f in &m.fields {
                let n = snake(&f.name);
                registrations.push(format!(
                    "c.define_method({n:?}, method!({}::get_{n}, 0))?;",
                    m.name
                ));
            }
        }
    }

    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        let trait_name = format!("{}Core", i.name);
        let impl_path = format!("crate::core_impl::{}Impl", i.name);

        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            let (item, _) = ty(api, &op.returns);
            registrations.push(format!(
                "let s = class.define_class({class:?}, ruby.class_object())?;"
            ));
            registrations.push(format!(
                "s.define_method(\"next\", method!({class}::next, 0))?;"
            ));
            quote_in! { t =>
                $['\r']
                $(format!("/// Poll-based stream from `{}.{}` — `.next` returns the next item or nil.", i.name, op.name))
                #[magnus::wrap(class = $(quoted(format!("{module}::{class}"))), free_immediately, size)]
                pub struct $(&class) {
                    stream: Box<dyn PollStream<$(&item)>>,
                }
                impl $(&class) {
                    fn next(&self) -> Option<$(&item)> {
                        loop {
                            match self.stream.poll(Duration::from_millis(500)) {
                                Poll::Item(v) => return Some(v),
                                Poll::Idle => continue,
                                Poll::Closed => return None, $("// nil ends iteration")
                            }
                        }
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
                let (ret, _) = ty(api, &op.returns);
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
                        registrations.push(format!(
                            "class.define_method({name:?}, method!({}::{name}, {arity}))?;",
                            i.name
                        ));
                        if rb_is_list_return(op) {
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
                let p = rb_op_pieces(api, op);
                let (fn_params, arity) = (p.fn_params, p.arity);
                let prelude = format!("{}{}", p.scan.unwrap_or_default(), p.prelude);
                let args = p.args;
                let (ret, _) = ty(api, &op.returns);
                registrations.push(format!(
                    "class.define_singleton_method({name:?}, function!({name}, {arity}))?;"
                ));
                if let Some(doc) = &op.doc {
                    for line in doc.lines() {
                        quote_in! { t => $['\r']$(format!("/// {line}")) };
                    }
                }
                if rb_is_list_return(op) {
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
    crate::rustfmt::format(format!(
        "//! GENERATED by fluessig bindgen from {src} (api layer). Do not edit.\n{}#![allow(clippy::all)]\n\n{body}",
        note_line(banner_note)
    ))
}
