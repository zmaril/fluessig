//! `#[derive(Union)]` expansion — the tagged-union authoring derive (disponent's
//! `union EventPayload`). A union is a Rust `enum` whose variants are the wire
//! alternatives, each a single-field tuple variant carrying that variant's body
//! type (`State(StateChange)`); the variant tag is the enum-variant name
//! lowerCamelCased (`State` → `state`, `ToolCall` → `toolCall`), or a per-variant
//! `#[fluessig(tag = "…")]` override. It lowers to the catalog's `unions` (and the
//! op layer's `api.json unions` when referenced), and a field typed by the union
//! lowers to `TypeRef::Union`.
//!
//! The proc-macro entry point stays in the crate root (proc-macro derives must);
//! the darling option shapes + token lowering live here, split out to keep the root
//! under the file-size budget (mirroring `enum_scalar.rs` / `record.rs`).

use darling::ast::{Data, Fields};
use darling::util::Ignored;
use darling::{FromDeriveInput, FromVariant};
use quote::quote;
use syn::{Ident, PathArguments, Type};

use crate::{doc_string, option_str, span_tokens};

/// The container-level options on a `#[derive(Union)]` enum: just its doc comment
/// (the wire discriminant is per-variant).
#[derive(FromDeriveInput)]
#[darling(attributes(fluessig), supports(enum_any), forward_attrs(doc))]
pub(crate) struct UnionOpts {
    ident: Ident,
    attrs: Vec<syn::Attribute>,
    data: Data<UnionVariantOpts, Ignored>,
}

/// One `#[derive(Union)]` variant: its Rust ident, an optional explicit wire `tag`
/// override, and its single body-type field (`State(StateChange)` → `StateChange`).
#[derive(FromVariant)]
#[darling(attributes(fluessig))]
pub(crate) struct UnionVariantOpts {
    ident: Ident,
    #[darling(default)]
    tag: Option<String>,
    fields: Fields<Type>,
}

pub(crate) fn expand_union(opts: UnionOpts) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &opts.ident;
    let variants = opts
        .data
        .take_enum()
        .expect("supports(enum_any) guarantees an enum");

    let mut variant_tokens = Vec::new();
    for v in &variants {
        // the wire tag: explicit `tag` override, else the variant ident lowerCamelCased.
        let tag = v
            .tag
            .clone()
            .unwrap_or_else(|| pascal_to_camel(&v.ident.to_string()));
        // the body type: exactly one unnamed field carrying the variant's payload type.
        if v.fields.style != darling::ast::Style::Tuple || v.fields.fields.len() != 1 {
            return Err(syn::Error::new(
                v.ident.span(),
                "a #[derive(Union)] variant must carry exactly one body type, \
                 e.g. `State(StateChange)`",
            ));
        }
        let body = bare_type_name(&v.fields.fields[0])?;
        variant_tokens.push(quote! {
            ::fluessig_derive::UnionVariantDescriptor { tag: #tag, ty: #body }
        });
    }

    let name_str = ident.to_string();
    let doc_tokens = option_str(doc_string(&opts.attrs).as_deref());
    let span = span_tokens(ident.span());
    Ok(quote! {
        impl ::fluessig_derive::UnionType for #ident {
            const DESCRIPTOR: &'static ::fluessig_derive::UnionDescriptor =
                &::fluessig_derive::UnionDescriptor {
                    name: #name_str,
                    doc: #doc_tokens,
                    variants: &[ #( #variant_tokens ),* ],
                    span: #span,
                };
        }
    })
}

/// `PascalCase` → `lowerCamelCase` — the wire-tag rule for a union variant name
/// (`State` → `state`, `ToolCall` → `toolCall`). Only the first character is
/// lowered; the rest are preserved, so an already-camel tail survives.
fn pascal_to_camel(ident: &str) -> String {
    let mut chars = ident.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// A single-segment, non-generic type path's name — a union variant's body type
/// (`State(StateChange)` → `"StateChange"`), resolved at lowering against the
/// catalog's declared types exactly as an entity's `Named` field is.
fn bare_type_name(ty: &Type) -> syn::Result<String> {
    let Type::Path(tp) = ty else {
        return Err(unsupported_union_body(ty));
    };
    if tp.qself.is_some() {
        return Err(unsupported_union_body(ty));
    }
    match tp.path.segments.last() {
        Some(seg) if matches!(seg.arguments, PathArguments::None) => Ok(seg.ident.to_string()),
        _ => Err(unsupported_union_body(ty)),
    }
}

fn unsupported_union_body(ty: &Type) -> syn::Error {
    syn::Error::new_spanned(
        ty,
        "unsupported union variant body — a variant carries exactly one named body \
         type, e.g. `State(StateChange)`",
    )
}
