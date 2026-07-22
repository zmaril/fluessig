//! The ext-php-rs (PHP) template grid — one language's projection of the op shapes.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.
//!
//! The callback + subscription slice (an `ApiType::Callback` param, a
//! `Shape::Subscription` op) lives in the sibling [`super::php_callback`] module.
//! PHP callbacks are DOCUMENTED SYNC-ONLY (the coordinator ruling): the generated
//! `PhpCb` newtype asserts `Send`/`Sync` over a `!Send` PHP callable, sound only
//! under synchronous same-request-thread invocation (off-thread is UB) — see the
//! LOUD marker on `PhpCb` and notes/callback-function-types.md.

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
            "int32" | "int64" | "uint8" | "uint16" | "uint32" => "int",
            "float32" | "float64" | "float" => "float",
            "bytes" => "string",
            "void" => "void",
            _ => "string",
        }
        .to_string(),
        ApiType::Model { model } => model.clone(),
        ApiType::Enum { .. } => "string".to_string(),
        ApiType::List { .. } => "array".to_string(),
        ApiType::Nullable { nullable } => format!("?{}", php_doc_ty(nullable)),
        // a union envelope and a foreign handle both ride the string carrier here
        ApiType::Union { .. } | ApiType::Foreign { .. } => "string".to_string(),
        // A callback param crosses in as a PHP `callable` (a `Closure`); the
        // generated method wraps it into the uniform core `Box<dyn Fn>` via the
        // sync-only `PhpCb` newtype (see php_callback.rs / notes/callback-function-
        // types.md — PHP callbacks are documented sync-only).
        ApiType::Callback { .. } => "callable".to_string(),
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
    // A `single_threaded` interface is a thread-confined `!Send` handle — node-only
    // today; the ext-php-rs `#[php_class]` glue requires `Send`, so php cannot bind
    // a `!Send` core. Split it out (emit nothing) + append an honest skip-note
    // rather than a silent `Send`-assuming handle. No such interface ⇒ empty note.
    let (api_owned, st_note) = crate::bindgen::split_single_threaded(api, "php");
    let api = &api_owned;
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
    // The PhpCb callback wrapper (whenever a callback param appears; a
    // subscription op always carries one) + the opaque `#[php_class]`
    // Subscription handle. Both gated, so a callback/subscription-free schema
    // stays byte-identical. The `PhpCb` doc is the sync-only compile-visible
    // marker (the coordinator ruling — off-thread invocation is UB).
    if api_uses_callback(api) {
        quote_in! { t => $(super::php_callback::PHP_CALLBACK_PRELUDE) };
    }
    if api_uses_subscription(api) {
        quote_in! { t => $(super::php_callback::PHP_SUBSCRIPTION_PRELUDE) };
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
                // A callback param crosses in as a raw callable `&Zval` (not the
                // uniform core box); the conv prelude wraps it. Every other param
                // keeps its shared `ty` spelling.
                let sig = super::php_callback::php_param_sig(api, op);
                let ps = sig
                    .iter()
                    .map(|(n, r)| format!("{n}: {r}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let names = sig
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                let sep = if ps.is_empty() { "" } else { ", " };
                let (ret, _) = ty(api, &op.returns);
                // The callback-conv prelude (wraps each callable `&Zval` into the
                // uniform core `Box<dyn Fn>`, shadowing `{name}`); empty for a
                // callback-free op, so its body stays byte-identical. A
                // callback-carrying op is forced fallible (the conv marshals via
                // `?`), so it always rides the `PhpResult` throw seam.
                let convs = super::php_callback::callback_conv_lines(api, op);
                let prelude = convs.join("\n");
                let has_cb = super::php_callback::op_uses_callback(op);
                match op.shape {
                    Shape::Ctor => quote_in! { methods =>
                        $['\r']
                        pub fn __construct($(&ps)) -> PhpResult<Self> {
                            $(&prelude)
                            Ok(Self { core: Arc::new(<$(&impl_path) as $(&trait_name)>::$(&name)($(&names)).map_err(err)?) })
                        }
                    },
                    Shape::Unary => {
                        // An op export-name pin lands as ext-php-rs `#[rename("…")]`
                        // (un-pinned ⇒ no attr, byte-identical). PHP is ALREADY
                        // synchronous (the async/sync default is a node-only
                        // projection), so the only default-inversion effect here is
                        // INFALLIBILITY: a bare-`T` core drops the `PhpResult`/throw
                        // seam entirely — UNLESS the op carries a callback param,
                        // whose conv marshals via `?` and forces the throw seam back.
                        if let Some(nm) = pinned_name(&op.bindings, LANG) {
                            quote_in! { methods => $['\r']$(format!("#[rename({nm:?})]")) };
                        }
                        if op.infallible && !has_cb {
                            quote_in! { methods =>
                                $['\r']
                                pub fn $(&name)(&self$sep$(&ps)) -> $(&ret) {
                                    self.core.$(&name)($(&names))
                                }
                            }
                        } else if op.infallible {
                            quote_in! { methods =>
                                $['\r']
                                pub fn $(&name)(&self$sep$(&ps)) -> PhpResult<$(&ret)> {
                                    $(&prelude)
                                    Ok(self.core.$(&name)($(&names)))
                                }
                            }
                        } else {
                            quote_in! { methods =>
                                $['\r']
                                pub fn $(&name)(&self$sep$(&ps)) -> PhpResult<$(&ret)> {
                                    $(&prelude)
                                    self.core.$(&name)($(&names)).map_err(err)
                                }
                            }
                        }
                    }
                    Shape::Stream => {
                        let class = pascal(&op.name);
                        quote_in! { methods =>
                            $['\r']
                            pub fn $(&name)(&self$sep$(&ps)) -> PhpResult<$(&class)> {
                                $(&prelude)
                                Ok($(&class) { stream: self.core.$(&name)($(&names)).map_err(err)? })
                            }
                        }
                    }
                    // A subscription op REGISTERS the listener (its one callback
                    // param, wrapped by the `prelude` into the uniform `Box<dyn
                    // Fn>`) through the core, then hands back an owning `#[php_class]`
                    // `Subscription` handle holding the core's returned unsubscribe
                    // closure. Always `PhpResult<Subscription>`: even an infallible
                    // core op's callable-conv can raise (a non-callable `Zval`), and
                    // a fallible core op throws on `Err` via the same `err` seam.
                    // Always `&self` (a subscription op requires a stateful iface).
                    Shape::Subscription => {
                        if let Some(nm) = pinned_name(&op.bindings, LANG) {
                            quote_in! { methods => $['\r']$(format!("#[rename({nm:?})]")) };
                        }
                        let register = if op.infallible {
                            format!("let unsub = self.core.{name}({names});")
                        } else {
                            format!("let unsub = self.core.{name}({names}).map_err(err)?;")
                        };
                        quote_in! { methods =>
                            $['\r']
                            pub fn $(&name)(&self$sep$(&ps)) -> PhpResult<Subscription> {
                                $(&prelude)
                                $(&register)
                                Ok(Subscription { unsub: std::sync::Mutex::new(Some(unsub)) })
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
                // A callback param crosses in as a callable `&Zval`; the conv
                // prelude wraps it into the uniform core `Box<dyn Fn>` (and forces
                // the `PhpResult` throw seam, since the conv marshals via `?`). A
                // stateless subscription op is unreachable (the loader requires a
                // subscription op's interface to be stateful), so only Ctor/Unary/
                // Stream land here.
                let sig = super::php_callback::php_param_sig(api, op);
                let ps = sig
                    .iter()
                    .map(|(n, r)| format!("{n}: {r}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let names = sig
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                let (ret, _) = ty(api, &op.returns);
                let convs = super::php_callback::callback_conv_lines(api, op);
                let prelude = convs.join("\n");
                let has_cb = super::php_callback::op_uses_callback(op);
                // op export-name pin ⇒ `#[rename("…")]`; infallible ⇒ drop the
                // `PhpResult`/throw seam (see the method arm above).
                if let Some(nm) = pinned_name(&op.bindings, LANG) {
                    quote_in! { methods => $['\r']$(format!("#[rename({nm:?})]")) };
                }
                if op.infallible && !has_cb {
                    quote_in! { methods =>
                        $['\r']
                        pub fn $(&name)($(&ps)) -> $(&ret) {
                            <$(&impl_path) as $(&trait_name)>::$(&name)($(&names))
                        }
                    }
                } else if op.infallible {
                    quote_in! { methods =>
                        $['\r']
                        pub fn $(&name)($(&ps)) -> PhpResult<$(&ret)> {
                            $(&prelude)
                            Ok(<$(&impl_path) as $(&trait_name)>::$(&name)($(&names)))
                        }
                    }
                } else {
                    quote_in! { methods =>
                        $['\r']
                        pub fn $(&name)($(&ps)) -> PhpResult<$(&ret)> {
                            $(&prelude)
                            <$(&impl_path) as $(&trait_name)>::$(&name)($(&names)).map_err(err)
                        }
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
    let out = crate::rustfmt::format(format!(
        "//! GENERATED by fluessig bindgen from {src} (api layer). Do not edit.\n{}#![allow(clippy::all)]\n#![allow(unused_imports)]\n\n{body}",
        note_line(banner_note)
    ));
    format!("{out}{st_note}")
}
