//! Proc-macros for the fluessig Rust derive front end (Slices 1–3).
//!
//! Three macros, mirroring `derive-front-end.md` §1 "derive → descriptor →
//! exporter":
//!
//! * [`macro@Entity`] expands a plain struct to an
//!   `impl fluessig_derive::Entity` carrying a `&'static EntityDescriptor` —
//!   pure data, no runtime behaviour, no file writes.
//! * [`macro@Edge`] expands an edge struct to an `impl fluessig_derive::Edge`
//!   carrying a `&'static EdgeDescriptor` (`derive-front-end.md` §2.4).
//! * [`catalog!`] collects `#[derive(Entity)]` / `#[derive(Edge)]` types into a
//!   callable that produces the `catalog.json` structure the existing Rust
//!   loader consumes.
//!
//! Slice 1 was scalar-only; Slice 2 added `Id<T>` **references**. Slice 3 makes
//! the **attribute grammar real** — it is parsed with [`darling`] rather than
//! hand-rolled `syn`, and unlocks three shapes:
//!
//! * `#[fluessig(flatten)]` — a field embeds another struct's columns inline
//!   (inheritance / abstract-root-carries-only-its-key, §2.3);
//! * `#[derive(Edge)] #[fluessig(edge(from = A, to = B))]` — an edge as its own
//!   row struct (§2.4);
//! * `#[fluessig(shares(col))]` — a reference's leading FK column shares a
//!   physical column as a declared fact (§2.5).
//!
//! Field type mapping (`Id<T>`, scalars, `Option<…>`) stays `syn`-level — the
//! macro sees the literal tokens, which is more direct than reconstructing from a
//! monomorphised type (`notes/derive-front-end-decisions.md`, decision #4).
//! Still out of scope: polymorphism (`abstract_root`), the op surface, and spans
//! — Slices 4–6.

use darling::ast::{Data, NestedMeta};
use darling::util::Ignored;
use darling::{FromDeriveInput, FromField, FromMeta};
use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    GenericArgument, Ident, LitStr, PathArguments, Token, Type,
};

// ─────────────────────────── darling attribute grammar ───────────────────────────

/// The container-level options on an `#[derive(Entity)]` struct:
/// `#[fluessig(name = "table", ref_cols(field = "col", …))]`.
#[derive(FromDeriveInput)]
#[darling(attributes(fluessig), supports(struct_named), forward_attrs(doc))]
struct EntityOpts {
    ident: Ident,
    attrs: Vec<syn::Attribute>,
    data: Data<Ignored, FluField>,
    #[darling(default)]
    name: Option<String>,
    #[darling(default)]
    ref_cols: RefCols,
}

/// The container-level options on an `#[derive(Edge)]` struct:
/// `#[fluessig(name = "table", edge(from = A, to = B, expose = "field"))]`.
#[derive(FromDeriveInput)]
#[darling(attributes(fluessig), supports(struct_named), forward_attrs(doc))]
struct EdgeOpts {
    ident: Ident,
    attrs: Vec<syn::Attribute>,
    data: Data<Ignored, FluField>,
    #[darling(default)]
    name: Option<String>,
    edge: EdgeMeta,
}

/// The `edge(from = A, to = B, expose = "field")` nested meta-list.
#[derive(FromMeta)]
struct EdgeMeta {
    from: syn::Path,
    to: syn::Path,
    #[darling(default)]
    expose: Option<String>,
}

/// A field's `#[fluessig(…)]` options, shared by entities and edges. `#[key]`
/// (the Slice 1/2 spelling) and `#[fluessig(key)]` (the edge-struct spelling
/// from §2.4) both mark a key; `flatten` embeds, `shares(col, …)` declares
/// column sharing.
#[derive(FromField)]
#[darling(attributes(fluessig), forward_attrs(doc, key))]
struct FluField {
    ident: Option<Ident>,
    ty: Type,
    attrs: Vec<syn::Attribute>,
    #[darling(default, rename = "flatten")]
    is_flatten: bool,
    #[darling(default)]
    key: bool,
    #[darling(default)]
    shares: Shares,
}

/// `ref_cols(field = "col", …)` — arbitrary field-name keys → column names.
/// darling has no first-class arbitrary-key map, so this is a hand-written
/// `FromMeta` over the nested list (the one place the grammar leans on a custom
/// impl rather than a derive; see the PR notes).
#[derive(Default)]
struct RefCols(Vec<(String, String)>);

impl FromMeta for RefCols {
    fn from_list(items: &[NestedMeta]) -> darling::Result<Self> {
        parse_meta_list(items, |item| match item {
            NestedMeta::Meta(syn::Meta::NameValue(nv)) => {
                let key = nv.path.get_ident().ok_or_else(|| {
                    darling::Error::custom("ref_cols entry must be `field = \"column\"`")
                        .with_span(&nv.path)
                })?;
                let col = lit_str(&nv.value)?;
                Ok((key.to_string(), col))
            }
            other => Err(
                darling::Error::custom("ref_cols entries must be `field = \"column\"`")
                    .with_span(other),
            ),
        })
        .map(RefCols)
    }
}

/// `shares(col, …)` — bare column names a reference's leading FK columns share.
#[derive(Default)]
struct Shares(Vec<String>);

impl FromMeta for Shares {
    fn from_list(items: &[NestedMeta]) -> darling::Result<Self> {
        parse_meta_list(items, |item| match item {
            NestedMeta::Meta(syn::Meta::Path(p)) => {
                p.get_ident().map(|id| id.to_string()).ok_or_else(|| {
                    darling::Error::custom("shares(col) expects a bare column name").with_span(p)
                })
            }
            other => Err(
                darling::Error::custom("shares(col, …) expects bare column names").with_span(other),
            ),
        })
        .map(Shares)
    }
}

/// Walk a nested meta-list, mapping each item with `parse` and accumulating any
/// per-item errors — so one malformed entry reports against its own span
/// without discarding the rest. Returns the collected items, or every
/// accumulated error. The shared spine behind the hand-written `FromMeta` impls
/// (`RefCols`, `Shares`); each supplies only its per-item match.
fn parse_meta_list<T>(
    items: &[NestedMeta],
    parse: impl Fn(&NestedMeta) -> darling::Result<T>,
) -> darling::Result<Vec<T>> {
    let mut out = Vec::new();
    let mut errors = darling::Error::accumulator();
    for item in items {
        match parse(item) {
            Ok(v) => out.push(v),
            Err(e) => errors.push(e),
        }
    }
    errors.finish()?;
    Ok(out)
}

/// Extract a string-literal value from an attribute expression.
fn lit_str(expr: &syn::Expr) -> darling::Result<String> {
    if let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = expr
    {
        Ok(s.value())
    } else {
        Err(darling::Error::custom("expected a string literal").with_span(expr))
    }
}

// ─────────────────────────── #[derive(Entity)] ───────────────────────────

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
/// ```
///
/// Slice 3 adds `#[fluessig(flatten)]` — embed a root struct's columns inline —
/// and `#[fluessig(shares(col))]` on a reference field:
///
/// ```ignore
/// #[derive(Entity)]
/// #[fluessig(name = "commits")]
/// pub struct Commit {
///     #[fluessig(flatten)] pub object: GitObject,  // contributes (oid, repo_id)
///     pub message: String,
/// }
/// ```
#[proc_macro_derive(Entity, attributes(fluessig, key))]
pub fn derive_entity(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let opts = match EntityOpts::from_derive_input(&input) {
        Ok(o) => o,
        Err(e) => return e.write_errors().into(),
    };
    match expand_entity(opts) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_entity(opts: EntityOpts) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &opts.ident;
    let fields = opts
        .data
        .take_struct()
        .expect("supports(struct_named) guarantees a struct")
        .fields;

    let table_tokens = option_str(opts.name.as_deref());
    let ref_col_tokens = opts.ref_cols.0.iter().map(|(field, column)| {
        quote! {
            ::fluessig_derive::RefColDescriptor { field: #field, column: #column }
        }
    });

    let mut field_descriptors = Vec::new();
    for field in &fields {
        field_descriptors.push(field_descriptor_tokens(field)?);
    }

    let entity_doc_tokens = option_str(doc_string(&opts.attrs).as_deref());
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

// ─────────────────────────── #[derive(Edge)] ───────────────────────────

/// Derive a `&'static EdgeDescriptor` for an edge struct — the edge as its own
/// row type (`derive-front-end.md` §2.4).
///
/// ```ignore
/// #[derive(Edge)]
/// #[fluessig(name = "commit_parents", edge(from = Commit, to = Commit, expose = "parents"))]
/// pub struct CommitParent {
///     pub commit_oid: Id<Commit>,      // source-side FK
///     pub parent_oid: Id<Commit>,      // target-side FK
///     #[fluessig(key)] pub idx: i32,   // local key (edge PK = source key + local key)
/// }
/// ```
///
/// An `Id<from>` field becomes a source column, an `Id<to>` field a target
/// column (for a self-edge the first is the source, the second the target, by
/// declaration order); every other field is an edge property.
#[proc_macro_derive(Edge, attributes(fluessig, key))]
pub fn derive_edge(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let opts = match EdgeOpts::from_derive_input(&input) {
        Ok(o) => o,
        Err(e) => return e.write_errors().into(),
    };
    match expand_edge(opts) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_edge(opts: EdgeOpts) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &opts.ident;
    let fields = opts
        .data
        .take_struct()
        .expect("supports(struct_named) guarantees a struct")
        .fields;

    let from = path_ident(&opts.edge.from)?;
    let to = path_ident(&opts.edge.to)?;
    let from_str = from.to_string();
    let to_str = to.to_string();

    let table_tokens = option_str(opts.name.as_deref());
    let expose_tokens = option_str(opts.edge.expose.as_deref());
    let edge_doc_tokens = option_str(doc_string(&opts.attrs).as_deref());
    let name_str = ident.to_string();

    // Classify each field: the first Id<from> is the source, the first Id<to> is
    // the target (a self-edge takes them in declaration order), everything else
    // is an edge property.
    let mut source_taken = false;
    let mut target_taken = false;
    let mut edge_fields = Vec::new();
    for field in &fields {
        let role = match ref_target(&field.ty)? {
            Some(t) if t == from && !source_taken => {
                source_taken = true;
                quote! { ::fluessig_derive::EdgeRole::Source }
            }
            Some(t) if t == to && !target_taken => {
                target_taken = true;
                quote! { ::fluessig_derive::EdgeRole::Target }
            }
            _ => quote! { ::fluessig_derive::EdgeRole::Property },
        };
        let fd = field_descriptor_tokens(field)?;
        edge_fields.push(quote! {
            ::fluessig_derive::EdgeFieldDescriptor { field: #fd, role: #role }
        });
    }

    Ok(quote! {
        impl ::fluessig_derive::Edge for #ident {
            const DESCRIPTOR: &'static ::fluessig_derive::EdgeDescriptor =
                &::fluessig_derive::EdgeDescriptor {
                    name: #name_str,
                    table: #table_tokens,
                    doc: #edge_doc_tokens,
                    from: #from_str,
                    to: #to_str,
                    expose: #expose_tokens,
                    fields: &[ #( #edge_fields ),* ],
                };
        }
    })
}

// ─────────────────────────── shared field lowering ───────────────────────────

/// Emit the `FieldDescriptor { … }` tokens for one struct field, honouring
/// `flatten`, `#[key]` / `#[fluessig(key)]`, `shares(…)`, doc comments, and the
/// scalar / `Id<T>` type mapping.
fn field_descriptor_tokens(field: &FluField) -> syn::Result<proc_macro2::TokenStream> {
    let fname = field.ident.as_ref().expect("named field").to_string();
    let is_key = field.key || has_bare_key(&field.attrs);
    let doc_tokens = option_str(doc_string(&field.attrs).as_deref());
    let shares = &field.shares.0;
    let shares_tokens = quote! { &[ #( #shares ),* ] };

    let (kind, nullable) = if field.is_flatten {
        let ty = &field.ty;
        (
            quote! { ::fluessig_derive::FieldKind::Flatten(
                <#ty as ::fluessig_derive::Entity>::DESCRIPTOR
            ) },
            false,
        )
    } else {
        map_field_type(&field.ty)?
    };

    Ok(quote! {
        ::fluessig_derive::FieldDescriptor {
            name: #fname,
            kind: #kind,
            nullable: #nullable,
            key: #is_key,
            doc: #doc_tokens,
            shares: #shares_tokens,
        }
    })
}

/// `Some("s")` / `None` tokens from an optional string.
fn option_str(s: Option<&str>) -> proc_macro2::TokenStream {
    match s {
        Some(v) => quote! { ::core::option::Option::Some(#v) },
        None => quote! { ::core::option::Option::None },
    }
}

/// The last path segment as an `Ident` — the entity name in `edge(from = …)`.
fn path_ident(p: &syn::Path) -> syn::Result<Ident> {
    p.segments
        .last()
        .map(|s| s.ident.clone())
        .ok_or_else(|| syn::Error::new_spanned(p, "expected an entity name"))
}

/// `Id<T>` (or `Option<Id<T>>`) → `Some(T)`: the reference target for edge-field
/// role classification.
fn ref_target(ty: &Type) -> syn::Result<Option<Ident>> {
    let inner = option_inner(ty).unwrap_or(ty);
    id_target(inner)
}

/// Map a Rust field type to a `FieldKind` token and its nullability.
/// `Option<T>` ⇒ nullable, unwrapping to the inner type. A field typed `Id<T>`
/// lowers to `FieldKind::Reference("T")`; a primitive scalar lowers to
/// `FieldKind::Scalar(…)`.
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

/// A field typed `Id<T>` → `Some(T)`, resolved by `syn` path parsing. The macro
/// sees the literal `T` token, so a typo'd target is a plain rustc "cannot find
/// type" error at the field. `Id` with anything but one type argument (`Id`,
/// `Id<A, B>`, `Id<'a>`) is an authoring error.
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
         (i8..i64, u8..u64, f32/f64, bool, String), foreign keys Id<T>, \
         Option<…> of either, and a #[fluessig(flatten)] embedded struct. \
         Polymorphism (abstract_root) arrives in Slice 4.",
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

fn has_bare_key(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("key"))
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
    edges: Vec<Ident>,
}

impl Parse for CatalogInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut entities = None;
        let mut edges = Vec::new();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            match key.to_string().as_str() {
                "name" => name = Some(input.parse::<LitStr>()?),
                "version" => version = Some(parse_version(input)?),
                "entities" => entities = Some(parse_ident_list(input)?),
                "edges" => edges = parse_ident_list(input)?,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown catalog! field `{other}` — supported: \
                             name, version, entities, edges"
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
            edges,
        })
    }
}

/// Parse `[A, B, C]` into a list of idents.
fn parse_ident_list(input: ParseStream) -> syn::Result<Vec<Ident>> {
    let content;
    syn::bracketed!(content in input);
    let list: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated(&content)?;
    Ok(list.into_iter().collect())
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

/// Collect `#[derive(Entity)]` / `#[derive(Edge)]` types into a catalog exporter.
///
/// Expands to a module `fluessig_catalog` exposing:
/// * `catalog() -> fluessig::Catalog` — the validated in-memory IR;
/// * `to_json() -> String` — the `catalog.json` text the loader consumes.
///
/// ```ignore
/// fluessig_derive::catalog! {
///     name: "git_demo",
///     version: "0.1.0",
///     entities: [Repo, Commit],
///     edges: [CommitParent],
/// }
/// ```
#[proc_macro]
pub fn catalog(input: TokenStream) -> TokenStream {
    let CatalogInput {
        name,
        version,
        entities,
        edges,
    } = parse_macro_input!(input as CatalogInput);

    // The generated `fluessig_catalog` module nests one level below the
    // invocation scope, so entity/edge paths are reached through `super::`.
    let entity_descriptors = entities.iter().map(|e| {
        quote! { <super::#e as ::fluessig_derive::Entity>::DESCRIPTOR }
    });
    let edge_descriptors = edges.iter().map(|e| {
        quote! { <super::#e as ::fluessig_derive::Edge>::DESCRIPTOR }
    });

    quote! {
        /// Generated by `fluessig_derive::catalog!` — the exporter half of the
        /// derive front end (`derive-front-end.md` §2.8).
        pub mod fluessig_catalog {
            /// The entity descriptors listed in `catalog!`, in declaration order.
            pub const ENTITIES: &[&'static ::fluessig_derive::EntityDescriptor] =
                &[ #( #entity_descriptors ),* ];

            /// The edge descriptors listed in `catalog!`, in declaration order.
            pub const EDGES: &[&'static ::fluessig_derive::EdgeDescriptor] =
                &[ #( #edge_descriptors ),* ];

            /// The catalog name as declared in `catalog!`.
            pub const NAME: &str = #name;
            /// The catalog version as declared in `catalog!`.
            pub const VERSION: &str = #version;

            /// Build the in-memory `fluessig::Catalog` IR from the descriptors.
            pub fn catalog() -> ::fluessig_derive::fluessig::Catalog {
                ::fluessig_derive::build_catalog_with_edges(NAME, VERSION, ENTITIES, EDGES)
            }

            /// Render the `catalog.json` text the existing Rust loader consumes.
            pub fn to_json() -> ::std::string::String {
                ::fluessig_derive::to_catalog_json_with_edges(NAME, VERSION, ENTITIES, EDGES)
            }
        }
    }
    .into()
}
