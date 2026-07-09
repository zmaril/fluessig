//! Proc-macros for the fluessig Rust derive front end (Slice 1).
//!
//! Two macros, mirroring `derive-front-end.md` §1 "derive → descriptor →
//! exporter":
//!
//! * [`macro@Entity`] expands a plain scalar-only struct to an
//!   `impl fluessig_derive::Entity` carrying a `&'static EntityDescriptor` —
//!   pure data, no runtime behaviour, no file writes.
//! * [`catalog!`] collects a list of `#[derive(Entity)]` types into a callable
//!   that produces the `catalog.json` structure the existing Rust loader
//!   consumes.
//!
//! Slice 1 is deliberately scalar-only: no `Id<T>` / foreign keys, no edges, no
//! polymorphism, no op surface. Those are Slices 2–5 (see
//! `notes/derive-front-end-decisions.md`). The attribute grammar here is parsed
//! with plain `syn`; the richer `darling`-tier grammar lands in Slice 3.

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    Data, DeriveInput, Fields, GenericArgument, Ident, LitStr, PathArguments, Token, Type,
};

/// Derive a `&'static EntityDescriptor` for a scalar-only entity struct.
///
/// ```ignore
/// #[derive(Entity)]
/// #[fluessig(name = "users")]      // optional; defaults to snake_case(StructName)
/// pub struct User {
///     /// The user's unique id.
///     #[key] pub id: i64,
///     pub login: String,
///     pub name: Option<String>,    // Option<T> ⇒ nullable
///     pub admin: bool,
/// }
/// ```
#[proc_macro_derive(Entity, attributes(fluessig, key))]
pub fn derive_entity(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_entity(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_entity(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &input.ident;

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            ident,
            "#[derive(Entity)] only supports structs",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new_spanned(
            ident,
            "#[derive(Entity)] requires a struct with named fields",
        ));
    };

    // container attribute: #[fluessig(name = "table")]
    let table = container_table(&input)?;
    let table_tokens = match table {
        Some(name) => quote! { ::core::option::Option::Some(#name) },
        None => quote! { ::core::option::Option::None },
    };

    let mut field_descriptors = Vec::new();
    for field in &named.named {
        let fname = field.ident.as_ref().expect("named field").to_string();
        let is_key = has_key_attr(field);
        let doc = doc_string(&field.attrs);
        let (scalar_kind, nullable) = map_field_type(&field.ty)?;

        let doc_tokens = match doc {
            Some(d) => quote! { ::core::option::Option::Some(#d) },
            None => quote! { ::core::option::Option::None },
        };

        field_descriptors.push(quote! {
            ::fluessig_derive::FieldDescriptor {
                name: #fname,
                scalar: #scalar_kind,
                nullable: #nullable,
                key: #is_key,
                doc: #doc_tokens,
            }
        });
    }

    let entity_doc = doc_string(&input.attrs);
    let entity_doc_tokens = match entity_doc {
        Some(d) => quote! { ::core::option::Option::Some(#d) },
        None => quote! { ::core::option::Option::None },
    };

    let name_str = ident.to_string();

    Ok(quote! {
        impl ::fluessig_derive::Entity for #ident {
            const DESCRIPTOR: &'static ::fluessig_derive::EntityDescriptor =
                &::fluessig_derive::EntityDescriptor {
                    name: #name_str,
                    table: #table_tokens,
                    doc: #entity_doc_tokens,
                    fields: &[ #( #field_descriptors ),* ],
                };
        }
    })
}

/// Map a Rust field type to a `ScalarKind` token and its nullability.
/// `Option<T>` ⇒ nullable, unwrapping to the inner scalar. Slice 1 accepts only
/// primitive scalars; anything else (a reference type, `Id<T>`, a nested
/// container) is a Slice 2+ concern and is rejected with a pointed message.
fn map_field_type(ty: &Type) -> syn::Result<(proc_macro2::TokenStream, bool)> {
    if let Some(inner) = option_inner(ty) {
        let (kind, already_opt) = map_field_type(inner)?;
        if already_opt {
            return Err(syn::Error::new_spanned(
                ty,
                "nested Option is not supported",
            ));
        }
        return Ok((kind, true));
    }
    let kind = scalar_kind(ty)?;
    Ok((kind, false))
}

/// The primitive-type → `ScalarKind` mapping. Kept in lock-step with the
/// `ScalarKind` enum in `fluessig-derive`.
fn scalar_kind(ty: &Type) -> syn::Result<proc_macro2::TokenStream> {
    let Type::Path(tp) = ty else {
        return Err(unsupported(ty));
    };
    if tp.qself.is_some() || tp.path.segments.len() != 1 {
        return Err(unsupported(ty));
    }
    let seg = &tp.path.segments[0];
    if !matches!(seg.arguments, PathArguments::None) {
        return Err(unsupported(ty));
    }
    let variant = match seg.ident.to_string().as_str() {
        "i8" => "I8",
        "i16" => "I16",
        "i32" => "I32",
        "i64" => "I64",
        "u8" => "U8",
        "u16" => "U16",
        "u32" => "U32",
        "u64" => "U64",
        "f32" => "F32",
        "f64" => "F64",
        "bool" => "Bool",
        "String" => "StringTy",
        _ => return Err(unsupported(ty)),
    };
    let variant = Ident::new(variant, seg.ident.span());
    Ok(quote! { ::fluessig_derive::ScalarKind::#variant })
}

fn unsupported(ty: &Type) -> syn::Error {
    syn::Error::new_spanned(
        ty,
        "unsupported field type for Slice 1 — only scalar primitives \
         (i8..i64, u8..u64, f32/f64, bool, String) and Option<…> of them are \
         supported. Foreign keys (Id<T>), edges, and polymorphism arrive in \
         Slices 2–4.",
    )
}

/// `Option<T>` → `Some(T)`.
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

/// Container-level `#[fluessig(name = "table")]` → the physical table override.
fn container_table(input: &DeriveInput) -> syn::Result<Option<String>> {
    let mut table = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("fluessig") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value = meta.value()?;
                let lit: LitStr = value.parse()?;
                table = Some(lit.value());
                Ok(())
            } else {
                Err(meta.error("unknown fluessig attribute — Slice 1 supports only `name = \"…\"`"))
            }
        })?;
    }
    Ok(table)
}

fn has_key_attr(field: &syn::Field) -> bool {
    field.attrs.iter().any(|a| a.path().is_ident("key"))
}

/// Collect `///` doc comments (lowered by rustc to `#[doc = "…"]`) into one
/// string, trimming the single leading space rustdoc inserts and joining lines
/// with `\n` — mirroring the TypeSpec emitter's `getDoc`.
fn doc_string(attrs: &[syn::Attribute]) -> Option<String> {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                let raw = s.value();
                lines.push(raw.strip_prefix(' ').unwrap_or(&raw).to_string());
            }
        }
    }
    if lines.is_empty() {
        return None;
    }
    // Trim leading/trailing blank lines, keep internal structure.
    while lines.first().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.remove(0);
    }
    while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

// ─────────────────────────── catalog! ───────────────────────────

struct CatalogInput {
    name: LitStr,
    version: LitStr,
    entities: Vec<Ident>,
}

impl Parse for CatalogInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut entities = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            match key.to_string().as_str() {
                "name" => name = Some(input.parse::<LitStr>()?),
                "version" => version = Some(parse_version(input)?),
                "entities" => {
                    let content;
                    syn::bracketed!(content in input);
                    let list: Punctuated<Ident, Token![,]> =
                        Punctuated::parse_terminated(&content)?;
                    entities = Some(list.into_iter().collect());
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown catalog! field `{other}` — Slice 1 supports \
                             name, version, entities"
                        ),
                    ))
                }
            }
            // optional trailing comma between fields
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(CatalogInput {
            name: name
                .ok_or_else(|| syn::Error::new(input.span(), "catalog! is missing `name`"))?,
            version: version
                .ok_or_else(|| syn::Error::new(input.span(), "catalog! is missing `version`"))?,
            entities: entities
                .ok_or_else(|| syn::Error::new(input.span(), "catalog! is missing `entities`"))?,
        })
    }
}

/// Accept `version: "0.1.0"` or `version: 0` (both spellings appear in the design
/// notes); normalise to a string.
fn parse_version(input: ParseStream) -> syn::Result<LitStr> {
    if input.peek(LitStr) {
        input.parse::<LitStr>()
    } else {
        let lit: syn::LitInt = input.parse()?;
        Ok(LitStr::new(&lit.to_string(), lit.span()))
    }
}

/// Collect `#[derive(Entity)]` types into a catalog exporter.
///
/// Expands to a module `fluessig_catalog` exposing:
/// * `catalog() -> fluessig::Catalog` — the validated in-memory IR;
/// * `to_json() -> String` — the `catalog.json` text the loader consumes.
///
/// ```ignore
/// fluessig_derive::catalog! {
///     name: "user_demo",
///     version: "0.1.0",
///     entities: [User],
/// }
/// ```
#[proc_macro]
pub fn catalog(input: TokenStream) -> TokenStream {
    let CatalogInput {
        name,
        version,
        entities,
    } = parse_macro_input!(input as CatalogInput);

    // The generated `fluessig_catalog` module nests one level below the
    // invocation scope, so entity paths are reached through `super::`.
    let descriptors = entities.iter().map(|e| {
        quote! { <super::#e as ::fluessig_derive::Entity>::DESCRIPTOR }
    });

    quote! {
        /// Generated by `fluessig_derive::catalog!` — the exporter half of the
        /// derive front end (`derive-front-end.md` §2.8).
        pub mod fluessig_catalog {
            /// The catalog descriptors listed in `catalog!`, in declaration order.
            pub const ENTITIES: &[&'static ::fluessig_derive::EntityDescriptor] =
                &[ #( #descriptors ),* ];

            /// The catalog name as declared in `catalog!`.
            pub const NAME: &str = #name;
            /// The catalog version as declared in `catalog!`.
            pub const VERSION: &str = #version;

            /// Build the in-memory `fluessig::Catalog` IR from the descriptors.
            pub fn catalog() -> ::fluessig_derive::fluessig::Catalog {
                ::fluessig_derive::build_catalog(NAME, VERSION, ENTITIES)
            }

            /// Render the `catalog.json` text the existing Rust loader consumes.
            pub fn to_json() -> ::std::string::String {
                ::fluessig_derive::to_catalog_json(NAME, VERSION, ENTITIES)
            }
        }
    }
    .into()
}
