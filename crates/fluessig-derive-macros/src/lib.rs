//! Proc-macros for the fluessig Rust derive front end (Slices 1–2).
//!
//! Two macros, mirroring `derive-front-end.md` §1 "derive → descriptor →
//! exporter":
//!
//! * [`macro@Entity`] expands a plain struct to an
//!   `impl fluessig_derive::Entity` carrying a `&'static EntityDescriptor` —
//!   pure data, no runtime behaviour, no file writes.
//! * [`catalog!`] collects a list of `#[derive(Entity)]` types into a callable
//!   that produces the `catalog.json` structure the existing Rust loader
//!   consumes.
//!
//! Slice 1 was scalar-only. Slice 2 adds **references**: a field typed `Id<T>`
//! (or `Option<Id<T>>`) is resolved by `syn` path parsing to a foreign key
//! targeting `T` (so `@fk` disappears), and a composite-key target spells its
//! reference columns once via `#[fluessig(ref_cols(field = "col", …))]`. Still
//! out of scope: edges, `flatten`/inheritance, polymorphism (`abstract_root`),
//! the op surface, and spans — Slices 3–6 (`notes/derive-front-end-decisions.md`).
//! The attribute grammar here is parsed with plain `syn`; the richer
//! `darling`-tier grammar lands in Slice 3.

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    Data, DeriveInput, Fields, GenericArgument, Ident, LitStr, PathArguments, Token, Type,
};

/// Derive a `&'static EntityDescriptor` for an entity struct.
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
///
/// #[derive(Entity)]
/// #[fluessig(name = "reviews")]
/// pub struct Review {
///     #[key] pub id: i64,
///     pub pr: Id<PullRequest>,             // a foreign key, resolved from the type
///     pub reviewer: Option<Id<User>>,      // Option<Id<T>> ⇒ a nullable FK
/// }
/// ```
///
/// A composite-key target declares how referencing sites spell its columns:
///
/// ```ignore
/// #[derive(Entity)]
/// #[fluessig(name = "pull_requests", ref_cols(number = "pr_number"))]
/// pub struct PullRequest {
///     #[key] pub repo_id: Id<Repo>,
///     #[key] pub number: i32,
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

    // container attributes: #[fluessig(name = "table", ref_cols(field = "col", …))]
    let ContainerAttrs { table, ref_cols } = container_attrs(&input)?;
    let table_tokens = match table {
        Some(name) => quote! { ::core::option::Option::Some(#name) },
        None => quote! { ::core::option::Option::None },
    };
    let ref_col_tokens = ref_cols.iter().map(|(field, column)| {
        quote! {
            ::fluessig_derive::RefColDescriptor { field: #field, column: #column }
        }
    });

    let mut field_descriptors = Vec::new();
    for field in &named.named {
        let fname = field.ident.as_ref().expect("named field").to_string();
        let is_key = has_key_attr(field);
        let doc = doc_string(&field.attrs);
        let (kind, nullable) = map_field_type(&field.ty)?;

        let doc_tokens = match doc {
            Some(d) => quote! { ::core::option::Option::Some(#d) },
            None => quote! { ::core::option::Option::None },
        };

        field_descriptors.push(quote! {
            ::fluessig_derive::FieldDescriptor {
                name: #fname,
                kind: #kind,
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
                    ref_cols: &[ #( #ref_col_tokens ),* ],
                };
        }
    })
}

/// Map a Rust field type to a `FieldKind` token and its nullability.
/// `Option<T>` ⇒ nullable, unwrapping to the inner type. A field typed `Id<T>`
/// lowers to `FieldKind::Reference("T")` (Slice 2); a primitive scalar lowers to
/// `FieldKind::Scalar(…)` (Slice 1). Anything else (an edge, a nested container)
/// is a Slice 3+ concern and is rejected with a pointed message.
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
    let kind = field_kind(ty)?;
    Ok((kind, false))
}

/// Resolve a non-`Option` field type to its `FieldKind` token: `Id<T>` ⇒ a
/// foreign-key reference to `T`, else a scalar primitive.
fn field_kind(ty: &Type) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(target) = id_target(ty)? {
        let name = target.to_string();
        return Ok(quote! { ::fluessig_derive::FieldKind::Reference(#name) });
    }
    let kind = scalar_kind(ty)?;
    Ok(quote! { ::fluessig_derive::FieldKind::Scalar(#kind) })
}

/// A field typed `Id<T>` (Slice 2) → `Some(T)`, resolved by `syn` path parsing.
/// The macro sees the literal `T` token, so a typo'd target is a plain rustc
/// "cannot find type" error at the field. `Id` with anything but one type
/// argument (`Id`, `Id<A, B>`, `Id<'a>`) is an authoring error.
fn id_target(ty: &Type) -> syn::Result<Option<Ident>> {
    let Type::Path(tp) = ty else {
        return Ok(None);
    };
    if tp.qself.is_some() {
        return Ok(None);
    }
    let seg = match tp.path.segments.last() {
        Some(seg) if seg.ident == "Id" => seg,
        _ => return Ok(None),
    };
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return Err(syn::Error::new_spanned(
            ty,
            "Id<T> needs a single entity type argument, e.g. Id<Repo>",
        ));
    };
    let types: Vec<&Type> = args
        .args
        .iter()
        .filter_map(|a| match a {
            GenericArgument::Type(t) => Some(t),
            _ => None,
        })
        .collect();
    let [Type::Path(target)] = types.as_slice() else {
        return Err(syn::Error::new_spanned(
            ty,
            "Id<T> takes exactly one entity type argument, e.g. Id<Repo>",
        ));
    };
    match target.path.segments.last() {
        Some(seg) if matches!(seg.arguments, PathArguments::None) => Ok(Some(seg.ident.clone())),
        _ => Err(syn::Error::new_spanned(
            ty,
            "Id<T>'s target must be a plain entity name, e.g. Id<Repo>",
        )),
    }
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
        "unsupported field type — supported so far: scalar primitives \
         (i8..i64, u8..u64, f32/f64, bool, String), foreign keys Id<T>, and \
         Option<…> of either. Edges, inheritance (flatten), and polymorphism \
         arrive in Slices 3–4.",
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

/// The container-level `#[fluessig(…)]` attributes the derive understands.
struct ContainerAttrs {
    /// `name = "table"` — the physical table override.
    table: Option<String>,
    /// `ref_cols(field = "column", …)` — how referencing `Id<Self>` sites spell
    /// this entity's key columns (Slice 2). `(key_field, column)` pairs.
    ref_cols: Vec<(String, String)>,
}

/// Parse `#[fluessig(name = "table", ref_cols(field = "col", …))]`.
fn container_attrs(input: &DeriveInput) -> syn::Result<ContainerAttrs> {
    let mut table = None;
    let mut ref_cols = Vec::new();
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
            } else if meta.path.is_ident("ref_cols") {
                // ref_cols(field = "col", field2 = "col2", …)
                meta.parse_nested_meta(|rc| {
                    let field = rc
                        .path
                        .get_ident()
                        .ok_or_else(|| rc.error("ref_cols entry must be `field = \"column\"`"))?
                        .to_string();
                    let value = rc.value()?;
                    let lit: LitStr = value.parse()?;
                    ref_cols.push((field, lit.value()));
                    Ok(())
                })
            } else {
                Err(meta.error(
                    "unknown fluessig attribute — supported: `name = \"…\"`, \
                     `ref_cols(field = \"col\", …)`",
                ))
            }
        })?;
    }
    Ok(ContainerAttrs { table, ref_cols })
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
