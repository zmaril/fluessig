//! `#[fluessig::export]` op-surface capture (Slice 5) — turning an `impl` block's
//! methods into an `InterfaceDescriptor` (op name / params / return / kind). The
//! proc-macro attribute entry point stays in the crate root; the capture + the
//! op-surface type mapping live here, split out to keep the root under the
//! file-size budget.

use quote::quote;
use syn::{
    Attribute, FnArg, GenericArgument, Ident, ImplItem, ItemImpl, Pat, PathArguments, ReturnType,
    Type,
};

use crate::{deref, doc_string, is_named, option_inner, option_str, span_tokens, vec_inner};

pub(crate) fn expand_export(item: ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let self_ty = &item.self_ty;
    let iface_ident = self_ty_ident(self_ty)?;
    let iface_name = iface_ident.to_string();
    let iface_span = span_tokens(iface_ident.span());
    let iface_doc = option_str(doc_string(&item.attrs).as_deref());

    // Build the op descriptors from the ORIGINAL methods (op-kind tags + docs
    // intact), then re-emit the impl with the `#[fluessig(…)]` tags stripped so
    // the methods still compile.
    let mut ops = Vec::new();
    let mut cleaned = item.clone();
    for it in &mut cleaned.items {
        let ImplItem::Fn(f) = it else { continue };
        ops.push(op_descriptor_tokens(f)?);
        f.attrs.retain(|a| !a.path().is_ident("fluessig"));
    }

    Ok(quote! {
        #cleaned

        impl ::fluessig_derive::ApiExport for #self_ty {
            const DESCRIPTOR: &'static ::fluessig_derive::InterfaceDescriptor =
                &::fluessig_derive::InterfaceDescriptor {
                    name: #iface_name,
                    doc: #iface_doc,
                    ops: &[ #( #ops ),* ],
                    span: #iface_span,
                };
        }
    })
}

/// The `OpDescriptor { … }` tokens for one method: its snake_case name, doc, op
/// kind, params (receiver excluded), and return type.
fn op_descriptor_tokens(f: &syn::ImplItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let name = f.sig.ident.to_string();
    let doc = option_str(doc_string(&f.attrs).as_deref());
    let meta = method_meta(&f.attrs)?;
    let kind_tokens = meta.kind.tokens();
    // The per-op async marker (`#[fluessig(async)]` ⇒ `Some(true)`,
    // `#[fluessig(sync)]` ⇒ `Some(false)`, neither ⇒ `None`). Synchronous is the
    // global default; `Some(false)` and `None` both lower to a sync binding.
    let is_async = match meta.is_async {
        Some(true) => quote! { ::core::option::Option::Some(true) },
        Some(false) => quote! { ::core::option::Option::Some(false) },
        None => quote! { ::core::option::Option::None },
    };
    // Fallibility is read off the Rust return type: a `Result<T>` return is
    // fallible (keeps the error seam), a bare `T` is infallible. Only consulted
    // for a SYNCHRONOUS op (async ops always cross the `Result` seam).
    let fallible = returns_result(&f.sig);
    let name_pin = option_str(meta.name_pin.as_deref());
    let readonly = meta.readonly;
    let destructive = meta.destructive;
    let params = param_descriptors(&f.sig)?;
    let returns = return_descriptor(meta.kind, &f.sig)?;
    let span = span_tokens(f.sig.ident.span());
    Ok(quote! {
        ::fluessig_derive::OpDescriptor {
            name: #name,
            doc: #doc,
            kind: #kind_tokens,
            is_async: #is_async,
            fallible: #fallible,
            name_pin: #name_pin,
            readonly: #readonly,
            destructive: #destructive,
            params: &[ #( #params ),* ],
            returns: #returns,
            span: #span,
        }
    })
}

/// Whether a method's declared return type is a `Result<…>` (fallible) — the
/// sync path uses this to decide between an infallible `-> T` node seam and a
/// throwing `-> napi::Result<T>` one. A bare `T` (or no return) is infallible.
fn returns_result(sig: &syn::Signature) -> bool {
    let ReturnType::Type(_, ty) = &sig.output else {
        return false;
    };
    let Type::Path(tp) = &**ty else { return false };
    tp.path
        .segments
        .last()
        .map(|s| s.ident == "Result")
        .unwrap_or(false)
}

/// The op-shaping tags on a method: its kind (`ctor` / plain unary / `stream` /
/// `manual`) plus the `readonly` / `destructive` FLAGS that compose with it (a
/// readonly op is still unary/stream; a destructive op likewise). Tags may ride one
/// `#[fluessig(a, b)]` or several `#[fluessig(a)] #[fluessig(b)]` — so
/// `@readonly @stream` (disponent's `events` / `driverPlan`) is expressible either
/// way. At most one KIND; the flags default off.
struct MethodMeta {
    kind: OpKindChoice,
    /// The per-op async marker — `Some(true)` = `#[fluessig(async)]` (opt into the
    /// async projection), `Some(false)` = `#[fluessig(sync)]` (the redundant
    /// explicit-synchronous marker), `None` = the global default. Legal only on a
    /// plain unary op. Synchronous is the global default, so an untagged op is
    /// `None`.
    is_async: Option<bool>,
    readonly: bool,
    destructive: bool,
    /// `#[fluessig(name = "…")]` — an explicit op export-name pin.
    name_pin: Option<String>,
}

/// One parsed tag inside a `#[fluessig(…)]` op attribute. The `async` keyword is
/// a Rust keyword and does NOT parse as a `syn::Meta::Path` (bare-ident) tag, so
/// it is peeled off explicitly; everything else (`sync` / `readonly` / kinds /
/// the `name = "…"` pin) rides the general `Meta` grammar.
enum OpTag {
    /// The `async` keyword — force the async projection.
    Async(proc_macro2::Span),
    /// Any other tag: a bare flag / kind (`Meta::Path`) or the pin (`NameValue`).
    /// Boxed — `syn::Meta` dwarfs the `Span`, and the clippy size gate would
    /// otherwise flag the variant imbalance.
    Meta(Box<syn::Meta>),
}

impl syn::parse::Parse for OpTag {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.peek(syn::Token![async]) {
            let kw = input.parse::<syn::Token![async]>()?;
            Ok(OpTag::Async(kw.span))
        } else {
            Ok(OpTag::Meta(Box::new(input.parse()?)))
        }
    }
}

fn method_meta(attrs: &[Attribute]) -> syn::Result<MethodMeta> {
    let mut kind: Option<OpKindChoice> = None;
    // `is_async` = the resolved per-op override; `async_span` marks whichever of
    // `sync`/`async` set it, for the "unary-only" / "conflicting" diagnostics.
    let mut is_async: Option<bool> = None;
    let mut async_span: Option<proc_macro2::Span> = None;
    let mut readonly = false;
    let mut destructive = false;
    let mut name_pin: Option<String> = None;
    for a in attrs {
        if !a.path().is_ident("fluessig") {
            continue;
        }
        // One attr may carry several comma-separated tags: bare flags
        // (`#[fluessig(readonly, stream)]`), the `async` keyword, and/or the
        // name-value pin (`#[fluessig(name = "…")]`). `async` is a keyword, so the
        // list is parsed through [`OpTag`] rather than the raw `Meta` grammar.
        let tags = a.parse_args_with(
            syn::punctuated::Punctuated::<OpTag, syn::Token![,]>::parse_terminated,
        )?;
        for tag in tags {
            let m = match tag {
                OpTag::Async(span) => {
                    if let Some(prev) = is_async {
                        if !prev {
                            return Err(syn::Error::new(
                                span,
                                "#[fluessig(async)] conflicts with #[fluessig(sync)] — an op \
                                 forces at most one projection",
                            ));
                        }
                    }
                    is_async = Some(true);
                    async_span = Some(span);
                    continue;
                }
                OpTag::Meta(m) => *m,
            };
            match m {
                syn::Meta::Path(p) => {
                    let id = p.get_ident().ok_or_else(|| {
                        syn::Error::new_spanned(&p, "expected a bare op tag (e.g. `sync`)")
                    })?;
                    match id.to_string().as_str() {
                        "sync" => {
                            if is_async == Some(true) {
                                return Err(syn::Error::new(
                                    id.span(),
                                    "#[fluessig(sync)] conflicts with #[fluessig(async)] — an op \
                                     forces at most one projection",
                                ));
                            }
                            is_async = Some(false);
                            async_span = Some(id.span());
                        }
                        "readonly" => readonly = true,
                        "destructive" => destructive = true,
                        kind_tag @ ("ctor" | "stream" | "manual") => {
                            if kind.is_some() {
                                return Err(syn::Error::new_spanned(
                                    id,
                                    "an exported method has at most one op kind \
                                     (ctor / stream / manual)",
                                ));
                            }
                            kind = Some(match kind_tag {
                                "ctor" => OpKindChoice::Ctor,
                                "stream" => OpKindChoice::Stream,
                                _ => OpKindChoice::Manual,
                            });
                        }
                        other => {
                            return Err(syn::Error::new_spanned(
                                id,
                                format!(
                                    "unknown op tag `{other}` — an exported method is tagged with \
                                     an op kind (#[fluessig(ctor)] / #[fluessig(stream)] / \
                                     #[fluessig(manual)], or untagged for a plain unary op), the \
                                     projection overrides #[fluessig(async)] / #[fluessig(sync)] \
                                     (synchronous is the default), the flags \
                                     #[fluessig(readonly)] / #[fluessig(destructive)], and/or the \
                                     export-name pin #[fluessig(name = \"…\")]"
                                ),
                            ))
                        }
                    }
                }
                syn::Meta::NameValue(nv) => {
                    if !nv.path.is_ident("name") {
                        return Err(syn::Error::new_spanned(
                            &nv.path,
                            "unknown op name-value tag — the only one is \
                             #[fluessig(name = \"…\")] (the export-name pin)",
                        ));
                    }
                    let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = &nv.value
                    else {
                        return Err(syn::Error::new_spanned(
                            &nv.value,
                            "#[fluessig(name = \"…\")] expects a string literal",
                        ));
                    };
                    name_pin = Some(s.value());
                }
                syn::Meta::List(l) => {
                    return Err(syn::Error::new_spanned(
                        l,
                        "unexpected nested list in an op tag",
                    ))
                }
            }
        }
    }
    // `sync` / `async` compose only with a plain unary op — a ctor is always a
    // synchronous constructor, a stream is always async-iterable, a manual op is
    // hand-written, so none has a projection to flip. Pairing them is an
    // authoring error.
    if is_async.is_some() && kind.is_some() {
        let span = async_span.unwrap_or_else(proc_macro2::Span::call_site);
        return Err(syn::Error::new(
            span,
            "#[fluessig(sync)] / #[fluessig(async)] apply only to a plain unary op \
             (not a ctor / stream / manual)",
        ));
    }
    Ok(MethodMeta {
        kind: kind.unwrap_or(OpKindChoice::Unary),
        is_async,
        readonly,
        destructive,
        name_pin,
    })
}

/// The parsed op kind — a macro-local mirror of `fluessig_derive::OpKind`, so the
/// return lowering can branch on it while the emitted tokens name the real enum.
#[derive(Clone, Copy)]
enum OpKindChoice {
    Ctor,
    Unary,
    Stream,
    Manual,
}

impl OpKindChoice {
    fn tokens(self) -> proc_macro2::TokenStream {
        let v = match self {
            OpKindChoice::Ctor => quote!(Ctor),
            OpKindChoice::Unary => quote!(Unary),
            OpKindChoice::Stream => quote!(Stream),
            OpKindChoice::Manual => quote!(Manual),
        };
        quote! { ::fluessig_derive::OpKind::#v }
    }
}

/// The `ParamDescriptor { … }` tokens for each typed param (the receiver is
/// skipped). An `Option<T>` param lowers to `optional: true` carrying the
/// UNWRAPPED `T` (params use `optional`; returns use `nullable`).
fn param_descriptors(sig: &syn::Signature) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    let mut out = Vec::new();
    for arg in &sig.inputs {
        let FnArg::Typed(pt) = arg else { continue };
        let Pat::Ident(pi) = &*pt.pat else {
            return Err(syn::Error::new_spanned(
                &pt.pat,
                "an exported op param must be a plain name, e.g. `repo_path: &str`",
            ));
        };
        let name = pi.ident.to_string();
        let (ty, optional) = match option_inner(&pt.ty) {
            Some(inner) => (base_api_type(inner)?, true),
            None => (base_api_type(&pt.ty)?, false),
        };
        let span = span_tokens(pi.ident.span());
        out.push(quote! {
            ::fluessig_derive::ParamDescriptor { name: #name, ty: #ty, optional: #optional, span: #span }
        });
    }
    Ok(out)
}

/// The `returns:` `ApiTypeDesc` tokens for a method, by op kind: a `ctor` is
/// always `void`; a `stream` carries its `impl Iterator<Item = T>` item (with any
/// `Result<T>` unwrapped); a unary/manual return is the type itself
/// (`Result<T>` transparent, `()` ⇒ `void`, `Option<T>` ⇒ `nullable T`).
fn return_descriptor(
    kind: OpKindChoice,
    sig: &syn::Signature,
) -> syn::Result<proc_macro2::TokenStream> {
    let void = quote! { ::fluessig_derive::ApiTypeDesc::Scalar("void") };
    match kind {
        OpKindChoice::Ctor => Ok(void),
        OpKindChoice::Stream => {
            let ty = return_ty(sig)?;
            let item = iterator_item(unwrap_result(ty))?;
            base_api_type(unwrap_result(item))
        }
        OpKindChoice::Unary | OpKindChoice::Manual => match &sig.output {
            ReturnType::Default => Ok(void),
            ReturnType::Type(_, ty) => {
                let ty = unwrap_result(ty);
                if is_unit(ty) {
                    Ok(void)
                } else if let Some(inner) = option_inner(ty) {
                    let inner = base_api_type(inner)?;
                    Ok(quote! { ::fluessig_derive::ApiTypeDesc::Nullable(&#inner) })
                } else {
                    base_api_type(ty)
                }
            }
        },
    }
}

/// The declared return type of a method that must have one (`stream` ops) — an
/// error at the arrow otherwise.
fn return_ty(sig: &syn::Signature) -> syn::Result<&Type> {
    match &sig.output {
        ReturnType::Type(_, ty) => Ok(ty),
        ReturnType::Default => Err(syn::Error::new_spanned(
            &sig.ident,
            "a #[fluessig(stream)] op must return `impl Iterator<Item = T>`",
        )),
    }
}

/// Map a Rust type to the op-surface `ApiTypeDesc` VALUE tokens: references are
/// stripped; `String`/`&str` ⇒ `string`; `Vec<u8>` ⇒ `bytes` and `Vec<T>` ⇒ a
/// list; primitive scalars map to their op-surface names; any other single-name
/// path is a model/entity reference (`{ model: Name }`).
fn base_api_type(ty: &Type) -> syn::Result<proc_macro2::TokenStream> {
    let ty = deref(ty);
    if let Some(elem) = vec_inner(ty) {
        if is_named(elem, "u8") {
            return Ok(quote! { ::fluessig_derive::ApiTypeDesc::Scalar("bytes") });
        }
        let inner = base_api_type(elem)?;
        return Ok(quote! { ::fluessig_derive::ApiTypeDesc::List(&#inner) });
    }
    let Type::Path(tp) = ty else {
        return Err(unsupported_op_type(ty));
    };
    if tp.qself.is_some() || tp.path.segments.len() != 1 {
        return Err(unsupported_op_type(ty));
    }
    let seg = &tp.path.segments[0];
    if !matches!(seg.arguments, PathArguments::None) {
        return Err(unsupported_op_type(ty));
    }
    let name = seg.ident.to_string();
    if let Some(scalar) = api_scalar_name(&name) {
        return Ok(quote! { ::fluessig_derive::ApiTypeDesc::Scalar(#scalar) });
    }
    // Any other bare type name is a model/entity reference. (Distinguishing a
    // catalog enum — which lowers to `{ enum }` — from a model can't be done from
    // the token alone; that would need a catalog cross-check at lowering. Op
    // signatures here reference entities/DTOs, so `{ model }` is the right shape.)
    Ok(quote! { ::fluessig_derive::ApiTypeDesc::Model(#name) })
}

/// A primitive Rust scalar → its op-surface scalar name, else `None` (⇒ a model
/// reference). `String`/`str` both map to `string`.
fn api_scalar_name(ident: &str) -> Option<&'static str> {
    Some(match ident {
        "String" | "str" => "string",
        "bool" => "boolean",
        "i8" => "int8",
        "i16" => "int16",
        "i32" => "int32",
        "i64" => "int64",
        "u8" => "uint8",
        "u16" => "uint16",
        "u32" => "uint32",
        "u64" => "uint64",
        "f32" => "float32",
        "f64" => "float64",
        _ => return None,
    })
}

/// `Result<T, …>` / `fluessig::Result<T>` → `T` (the op surface has no error
/// channel, so the `Result` wrapper is transparent); any other type is returned
/// unchanged.
fn unwrap_result(ty: &Type) -> &Type {
    let Type::Path(tp) = ty else { return ty };
    let Some(seg) = tp.path.segments.last() else {
        return ty;
    };
    if seg.ident != "Result" {
        return ty;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return ty;
    };
    args.args
        .iter()
        .find_map(|a| match a {
            GenericArgument::Type(t) => Some(t),
            _ => None,
        })
        .unwrap_or(ty)
}

/// `impl Iterator<Item = T>` → `T`. A `#[fluessig(stream)]` op must return an
/// `impl Iterator` (the shape bindgen maps to a JS async iterator / Python
/// generator / Ruby Enumerator).
fn iterator_item(ty: &Type) -> syn::Result<&Type> {
    let Type::ImplTrait(it) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "a #[fluessig(stream)] op must return `impl Iterator<Item = T>`",
        ));
    };
    for bound in &it.bounds {
        let syn::TypeParamBound::Trait(tb) = bound else {
            continue;
        };
        let Some(seg) = tb.path.segments.last() else {
            continue;
        };
        if seg.ident != "Iterator" {
            continue;
        }
        let PathArguments::AngleBracketed(args) = &seg.arguments else {
            continue;
        };
        for a in &args.args {
            if let GenericArgument::AssocType(assoc) = a {
                if assoc.ident == "Item" {
                    return Ok(&assoc.ty);
                }
            }
        }
    }
    Err(syn::Error::new_spanned(
        ty,
        "a #[fluessig(stream)] op must return `impl Iterator<Item = T>` (with an `Item =` binding)",
    ))
}

/// The unit type `()` — a unary op returning it is `void`.
fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(t) if t.elems.is_empty())
}

/// The `Self` type's name ident (`impl Entl` → `Entl`) — the interface name.
fn self_ty_ident(ty: &Type) -> syn::Result<Ident> {
    match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|s| s.ident.clone())
            .ok_or_else(|| syn::Error::new_spanned(ty, "expected a named type")),
        other => Err(syn::Error::new_spanned(
            other,
            "#[fluessig::export] applies to `impl <Name>` blocks",
        )),
    }
}

fn unsupported_op_type(ty: &Type) -> syn::Error {
    syn::Error::new_spanned(
        ty,
        "unsupported op-surface type — supported: scalar primitives (i8..i64, \
         u8..u64, f32/f64, bool, String/&str), `Vec<u8>` (bytes), `Vec<T>` \
         (list), `Option<T>`, a model/entity name, and (returns only) \
         `impl Iterator<Item = T>` for a #[fluessig(stream)] op.",
    )
}
