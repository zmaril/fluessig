//! The ext-php-rs (PHP) template grid — one language's projection of the op shapes.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.

use genco::prelude::*;

use crate::api::{ApiDoc, ApiType, Shape};

use super::*;

/// This backend's language slug — the key it reads out of every symbol's
/// `bindings` map via the shared [`pinned_name`] / [`variant_token`] resolver.
/// PHP hardcodes no pin; ext-php-rs (0.13.x / derive 0.10.2) renames methods to
/// camelCase by default, and its per-method rename lever is the bare
/// `#[rename("…")]` attribute — the only two things this backend owns.
const LANG: &str = "php";

/// The PHP-visible type name for an [`ApiType`] — what PHP's own type system
/// sees (`string`/`int`/`float`/`bool`/`array`), used only in the generated
/// method docblocks. ext-php-rs signatures themselves speak Rust types, so the
/// Rust half of the shared [`ty`] still drives every actual signature; this is
/// documentation, not codegen.
fn php_doc_ty(t: &ApiType) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "string" | "Json" => "string",
            "boolean" => "bool",
            "int32" | "int64" => "int",
            "float64" => "float",
            "bytes" => "string",
            "void" => "void",
            _ => "string",
        }
        .to_string(),
        ApiType::Model { model } => model.clone(),
        ApiType::Enum { .. } => "string".to_string(),
        ApiType::List { .. } => "array".to_string(),
        ApiType::Nullable { nullable } => format!("?{}", php_doc_ty(nullable)),
        ApiType::Union { .. } => "string".to_string(),
    }
}

/// The `/// PHP: Iface::op(int $x): string` docblock hint — the PHP-facing
/// signature, alongside the Rust one ext-php-rs actually compiles.
fn php_sig_note(iface: &str, op: &crate::api::ApiOp) -> String {
    let params = op
        .params
        .iter()
        .map(|p| format!("{} ${}", php_doc_ty(&p.ty), snake(&p.name)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "/// PHP: {iface}::{}({params}): {}",
        snake(&op.name),
        php_doc_ty(&op.returns)
    )
}

/// Generate the ext-php-rs (PHP) binding: `#[php_class]` DTOs + enums, the core
/// traits, per-interface `#[php_class]`/`#[php_impl]` surfaces (a stateful
/// handle with a `__construct`, or a stateless class of static methods),
/// `next()`-null stream cursors, and the `#[php_module]` registrar.
pub fn php_binding(api: &ApiDoc, enums: &[EnumDesc], banner_note: Option<&str>) -> String {
    // The shared streaming-contract import flows through the use-emitter
    // ([`RUNTIME_STREAM_IMPORT`]) rather than a hardcoded string, so every
    // generated `use` line has one emission path; renders byte-identically.
    let runtime_import = RUNTIME_STREAM_IMPORT.render();
    let mut t: rust::Tokens = quote! {
        $("// The fixed prelude — generated code uses fully-qualified paths elsewhere.")
        use std::sync::Arc;
        use std::time::Duration;
        use ext_php_rs::prelude::*;
        $("// The shared streaming contract — Poll/PollStream live in the fluessig-runtime crate.")
        $(&runtime_import)

        $("/// A core-layer failure becomes a thrown PHP exception (PHP is synchronous,")
        $("/// so a fallible op returns `PhpResult` and ext-php-rs raises on `Err`).")
        fn err(e: impl std::fmt::Display) -> PhpException {
            PhpException::default(e.to_string())
        }
    };
    if api_uses_bytes(api) {
        quote_in! { t =>
            $['\n']
            $("/// Bulk bytes cross into PHP as a binary string (ext-php-rs `Binary<u8>`,")
            $("/// which packs to a PHP string rather than an array of ints).")
            pub type Bytes = ext_php_rs::binary::Binary<u8>;
        };
    }
    t.line();

    // ── enums: plain Rust + parse/wire (PHP sees the snake_case wire token as a
    // string; a value-carrying enum crosses the boundary as that string) ──
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        // The wire token comes from the shared resolver (a `php` pin wins, then
        // the neutral `Variant.value`, else `to_lowercase()`); un-pinned is
        // exactly the old `to_lowercase()`, so the emission is byte-identical.
        let vs: Vec<String> = variants.iter().map(|v| pascal(&v.name)).collect();
        let arms: Vec<String> = variants
            .iter()
            .map(|v| {
                format!(
                    "{:?} => Ok(Self::{}),",
                    variant_token(v, LANG),
                    pascal(&v.name)
                )
            })
            .collect();
        let wire_arms: Vec<String> = variants
            .iter()
            .map(|v| format!("Self::{} => {:?},", pascal(&v.name), variant_token(v, LANG)))
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
                $("/// The wire token PHP sees for this variant.")
                pub fn wire(&self) -> &$("'static") str {
                    match self {
                        $(for a in &wire_arms join ($['\r']) => $a)
                    }
                }
            }
        };
    }
    t.line();

    // ── DTO models: a `#[php_class]` holding the fields, with getter methods ──
    for m in &api.models {
        if let Some(doc) = &m.doc {
            for line in doc.lines() {
                quote_in! { t => $['\r']$(format!("/// {line}")) };
            }
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
                quote!(pub(crate) $n: $r,)
            })
            .collect();
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
                // The Rust getter ident stays `snake`; ext-php-rs renames it to
                // camelCase in PHP by default. A `php` pin overrides that with
                // the ext-php-rs `#[rename("…")]` attribute (derive 0.10.2 syntax:
                // a bare attr taking a string literal). Un-pinned ⇒ no attr,
                // byte-identical to the default camelCase.
                let n = snake(&f.name);
                match pinned_name(&f.bindings, LANG) {
                    Some(nm) => {
                        let attr = format!("#[rename({nm:?})]");
                        quote! {
                            $attr
                            pub fn $(&n)(&self) -> $r {
                                self.$(&n).clone()
                            }
                        }
                    }
                    None => quote! {
                        pub fn $(&n)(&self) -> $r {
                            self.$(&n).clone()
                        }
                    },
                }
            })
            .collect();
        quote_in! { t =>
            $['\r']
            #[php_class]
            #[derive(Clone)]
            pub struct $(&m.name) {
                $(for f in &fields join ($['\r']) => $f)
            }
            #[php_impl]
            impl $(&m.name) {
                $(for g in &getters join ($['\r']) => $g)
            }
            $['\n']
        };
    }

    emit_core_traits(&mut t, api);

    // ── per-interface surface ──
    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        let trait_name = format!("{}Core", i.name);
        let impl_path = format!("crate::core_impl::{}Impl", i.name);

        // stream cursor classes — one `#[php_class]` per stream op, `next()`
        // returning the next item or null (PHP calls it until it resolves null).
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            let (item, _) = ty(api, &op.returns);
            quote_in! { t =>
                $['\r']
                $(format!("/// Poll-based stream from `{}.{}` — call `next()` until it returns null.", i.name, op.name))
                #[php_class]
                pub struct $(&class) {
                    stream: Box<dyn PollStream<$(&item)>>,
                }
                #[php_impl]
                impl $(&class) {
                    $("/// The next item, or null once the stream is exhausted. A terminal")
                    $("/// `Poll::Failed` throws a PHP exception (mirrors node's default")
                    $("/// throw-mode): the sync cursor has no error-as-event surface, so a")
                    $("/// mid-stream core failure surfaces as `err(e)` out of `next()`.")
                    pub fn next(&self) -> PhpResult<Option<$(&item)>> {
                        loop {
                            match self.stream.poll(Duration::from_millis(500)) {
                                Poll::Item(v) => return Ok(Some(v)),
                                Poll::Idle => continue,
                                Poll::Closed => return Ok(None), $("// null ends iteration")
                                Poll::Failed(e) => return Err(err(e)), $("// throws on failure")
                            }
                        }
                    }
                }
                $['\n']
            };
        }

        if has_ctor {
            // stateful handle: a `#[php_class]` holding the core, `__construct`
            // building it, instance methods delegating to it.
            let mut methods: rust::Tokens = quote!();
            for op in &i.ops {
                let name = snake(&op.name);
                if op.shape != Shape::Manual {
                    if let Some(doc) = &op.doc {
                        for line in doc.lines() {
                            quote_in! { methods => $['\r']$(format!("/// {line}")) };
                        }
                    }
                    quote_in! { methods => $['\r']$(php_sig_note(&i.name, op)) };
                }
                let ps = param_sig(api, op)
                    .iter()
                    .map(|(n, r)| format!("{n}: {r}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let names = param_sig(api, op)
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                let sep = if ps.is_empty() { "" } else { ", " };
                let (ret, _) = ty(api, &op.returns);
                match op.shape {
                    Shape::Ctor => quote_in! { methods =>
                        $['\r']
                        pub fn __construct($(&ps)) -> PhpResult<Self> {
                            Ok(Self { core: Arc::new(<$(&impl_path) as $(&trait_name)>::$(&name)($(&names)).map_err(err)?) })
                        }
                    },
                    Shape::Unary => quote_in! { methods =>
                        $['\r']
                        pub fn $(&name)(&self$sep$(&ps)) -> PhpResult<$(&ret)> {
                            self.core.$(&name)($(&names)).map_err(err)
                        }
                    },
                    Shape::Stream => {
                        let class = pascal(&op.name);
                        quote_in! { methods =>
                            $['\r']
                            pub fn $(&name)(&self$sep$(&ps)) -> PhpResult<$(&class)> {
                                Ok($(&class) { stream: self.core.$(&name)($(&names)).map_err(err)? })
                            }
                        }
                    }
                    Shape::Manual => quote_in! { methods =>
                        $['\r']
                        $(format!("// @manual: {} — hand-written in lib.rs.", op.name))
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
                #[php_class]
                pub struct $(&i.name) {
                    $("// pub(crate): the @manual ops in lib.rs extend this class and need the core")
                    pub(crate) core: Arc<$(&impl_path)>,
                }

                #[php_impl]
                impl $(&i.name) {
                    $methods
                }
                $['\n']
            };
        } else {
            // stateless interface → a `#[php_class]` exposing static methods
            // (matches the hand-written `Atilla::version()` shape).
            let mut methods: rust::Tokens = quote!();
            for op in &i.ops {
                let name = snake(&op.name);
                if op.shape == Shape::Manual {
                    quote_in! { methods => $['\r']$(format!("// @manual: {}.{} — hand-written in lib.rs.", i.name, op.name)) };
                    continue;
                }
                if let Some(doc) = &op.doc {
                    for line in doc.lines() {
                        quote_in! { methods => $['\r']$(format!("/// {line}")) };
                    }
                }
                quote_in! { methods => $['\r']$(php_sig_note(&i.name, op)) };
                let ps = param_sig(api, op)
                    .iter()
                    .map(|(n, r)| format!("{n}: {r}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let names = param_sig(api, op)
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                let (ret, _) = ty(api, &op.returns);
                quote_in! { methods =>
                    $['\r']
                    pub fn $(&name)($(&ps)) -> PhpResult<$(&ret)> {
                        <$(&impl_path) as $(&trait_name)>::$(&name)($(&names)).map_err(err)
                    }
                }
            }
            if let Some(doc) = &i.doc {
                for line in doc.lines() {
                    quote_in! { t => $['\r']$(format!("/// {line}")) };
                }
            }
            quote_in! { t =>
                $['\r']
                #[php_class]
                pub struct $(&i.name);

                #[php_impl]
                impl $(&i.name) {
                    $methods
                }
                $['\n']
            };
        }
    }

    // ext-php-rs auto-registers every `#[php_class]`, so the module body just
    // returns the builder — same as the hand-written binding.
    quote_in! { t =>
        $['\r']
        $("/// Registers the extension's surface with PHP.")
        #[php_module]
        pub fn module(module: ModuleBuilder) -> ModuleBuilder {
            module
        }
    };

    let src = api.source.as_deref().unwrap_or("the fluessig catalog");
    let body = t.to_file_string().expect("rust renders");
    crate::rustfmt::format(format!(
        "//! GENERATED by fluessig bindgen from {src} (api layer). Do not edit.\n{}#![allow(clippy::all)]\n\n{body}",
        note_line(banner_note)
    ))
}
