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
//! Slice 4 adds **polymorphism** (Decision #3):
//!
//! * [`macro@AbstractRoot`] — `#[derive(AbstractRoot)]` with
//!   `#[fluessig(abstract_root(Commit, Tree, Blob), tag_col = …, ref_col = …)]`
//!   generates the native key enum `<Root>Id` (one variant per leaf, each
//!   carrying the family key — heterogeneous across families), an
//!   `impl AbstractRoot for <Root>` alias, and the root's (abstract) `Entity`
//!   descriptor.
//! * A leaf declares `#[fluessig(extends = Root)]`; a polymorphic reference site
//!   names the generated enum natively (`subject: GhSubjectId`) and lowers to the
//!   (tag, key) pair, with an optional per-site `#[fluessig(cols(tag = …,
//!   key = …))]` override.
//!
//! Slice 5 adds the **op surface**:
//!
//! * `#[fluessig::export]` — the attribute macro on an `impl` block captures each
//!   method's shape (name, params, return, op kind — `#[fluessig(ctor)]` /
//!   untagged unary / `#[fluessig(stream)]` / `#[fluessig(manual)]`) into an
//!   `InterfaceDescriptor`, and re-emits the impl with the op-kind tags consumed.
//!   `catalog!` gains an `api:` root list that lowers those into `api.json`.
//!
//! Field/type mapping (`Id<T>`, `<Root>Id`, scalars, `Option<…>`, op params and
//! returns) stays `syn`-level — the macro sees the literal tokens, which is more
//! direct than reconstructing from a monomorphised type
//! (`notes/derive-front-end-decisions.md`, decision #4).
//!
//! Slice 6 adds **source spans**: each descriptor gains a
//! `fluessig_derive::SourceSpan` carrying the declaration's `file!()` + `line!()`
//! ([`span_tokens`] emits the built-ins *carrying* the item's span, so rustc
//! resolves them to the real `.rs` line per field). Spans feed loader diagnostics
//! only — they never enter the lowered catalog.

use darling::ast::{Data, NestedMeta};
use darling::util::Ignored;
use darling::{FromDeriveInput, FromField, FromMeta};
use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    Attribute, FnArg, GenericArgument, Ident, ImplItem, ItemImpl, LitStr, Pat, PathArguments,
    ReturnType, Token, Type,
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
    /// `extends = Root` — this entity is a concrete leaf of the family rooted at
    /// `Root` (Slice 4). Lowered to the catalog `extends`.
    #[darling(default)]
    extends: Option<syn::Path>,
}

/// The container-level options on an `#[derive(AbstractRoot)]` family root:
/// `#[fluessig(abstract_root(A, B, …), tag_col = "…", ref_col = "…", ref_cols(…))]`
/// (Slice 4). The root is also an `Entity` (abstract); the derive additionally
/// generates the `<Root>Id` key enum + the `AbstractRoot` alias.
#[derive(FromDeriveInput)]
#[darling(attributes(fluessig), supports(struct_named), forward_attrs(doc))]
struct AbstractRootOpts {
    ident: Ident,
    attrs: Vec<syn::Attribute>,
    data: Data<Ignored, FluField>,
    #[darling(default)]
    name: Option<String>,
    #[darling(default)]
    ref_cols: RefCols,
    /// The closed leaf set: `abstract_root(Commit, Tree, Blob)`.
    abstract_root: AbstractLeaves,
    /// The discriminator column a polymorphic reference to this family spells.
    #[darling(default)]
    tag_col: Option<String>,
    /// For a single-column-keyed family: the key column a polymorphic reference
    /// spells by default. Composite families use `ref_cols(…)` instead.
    #[darling(default)]
    ref_col: Option<String>,
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
    /// `cols(tag = "…", key = "…")` — a per-site column-spelling override on a
    /// polymorphic reference field (Slice 4). Only meaningful when the field is
    /// typed `<Root>Id`.
    #[darling(default)]
    cols: Option<ColsMeta>,
}

/// `cols(tag = "…", key = "…")` — the per-site spelling override of a polymorphic
/// reference's (tag, key) columns (Slice 4, entl FINDINGS #7).
#[derive(FromMeta, Default)]
struct ColsMeta {
    #[darling(default)]
    tag: Option<String>,
    #[darling(default)]
    key: Option<String>,
}

/// `abstract_root(Commit, Tree, Blob)` — the closed leaf set of a family, as bare
/// idents (they name the generated `<Root>Id` enum variants; the leaf entities
/// point back via `extends`).
#[derive(Default)]
struct AbstractLeaves(Vec<Ident>);

impl FromMeta for AbstractLeaves {
    fn from_list(items: &[NestedMeta]) -> darling::Result<Self> {
        parse_meta_list(items, |item| match item {
            NestedMeta::Meta(syn::Meta::Path(p)) => p.get_ident().cloned().ok_or_else(|| {
                darling::Error::custom("abstract_root(Leaf, …) expects bare leaf names")
                    .with_span(p)
            }),
            other => Err(
                darling::Error::custom("abstract_root(Leaf, …) expects bare leaf names")
                    .with_span(other),
            ),
        })
        .map(AbstractLeaves)
    }
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
    let fields = opts
        .data
        .take_struct()
        .expect("supports(struct_named) guarantees a struct")
        .fields;
    // `extends = Root` names the abstract family this leaf belongs to.
    let extends = opts
        .extends
        .as_ref()
        .map(path_ident)
        .transpose()?
        .map(|i| i.to_string());
    entity_descriptor_impl(EntityDescriptorArgs {
        ident: &opts.ident,
        attrs: &opts.attrs,
        fields: &fields,
        name: opts.name.as_deref(),
        ref_cols: &opts.ref_cols,
        extends: extends.as_deref(),
        abstract_leaves: &[],
        id_enum: None,
        tag_col: None,
        ref_col: None,
    })
}

/// The inputs to [`entity_descriptor_impl`] — the one place a
/// `&'static EntityDescriptor` `impl Entity` is emitted, shared by
/// `#[derive(Entity)]` (plain entities + leaves) and `#[derive(AbstractRoot)]`
/// (family roots). Grouped in a struct so the polymorphic fields don't balloon
/// the arity (and to keep the two callers a single lowering, not a copy).
struct EntityDescriptorArgs<'a> {
    ident: &'a Ident,
    attrs: &'a [syn::Attribute],
    fields: &'a [FluField],
    name: Option<&'a str>,
    ref_cols: &'a RefCols,
    extends: Option<&'a str>,
    abstract_leaves: &'a [String],
    id_enum: Option<&'a str>,
    tag_col: Option<&'a str>,
    ref_col: Option<&'a str>,
}

/// Emit `impl fluessig_derive::Entity` carrying the `&'static EntityDescriptor`.
fn entity_descriptor_impl(a: EntityDescriptorArgs) -> syn::Result<proc_macro2::TokenStream> {
    let ident = a.ident;
    let table_tokens = option_str(a.name);
    let ref_col_tokens = a.ref_cols.0.iter().map(|(field, column)| {
        quote! {
            ::fluessig_derive::RefColDescriptor { field: #field, column: #column }
        }
    });

    let mut field_descriptors = Vec::new();
    for field in a.fields {
        field_descriptors.push(field_descriptor_tokens(field)?);
    }

    let entity_doc_tokens = option_str(doc_string(a.attrs).as_deref());
    let name_str = ident.to_string();
    let extends_tokens = option_str(a.extends);
    let leaves_tokens = a.abstract_leaves.iter().map(|l| quote! { #l });
    let id_enum_tokens = option_str(a.id_enum);
    let tag_col_tokens = option_str(a.tag_col);
    let ref_col_tokens_single = option_str(a.ref_col);
    let span = span_tokens(ident.span());

    Ok(quote! {
        impl ::fluessig_derive::Entity for #ident {
            const DESCRIPTOR: &'static ::fluessig_derive::EntityDescriptor =
                &::fluessig_derive::EntityDescriptor {
                    name: #name_str,
                    table: #table_tokens,
                    doc: #entity_doc_tokens,
                    fields: &[ #( #field_descriptors ),* ],
                    ref_cols: &[ #( #ref_col_tokens ),* ],
                    extends: #extends_tokens,
                    abstract_leaves: &[ #( #leaves_tokens ),* ],
                    id_enum: #id_enum_tokens,
                    tag_col: #tag_col_tokens,
                    ref_col: #ref_col_tokens_single,
                    span: #span,
                };
        }
    })
}

// ─────────────────────────── #[derive(AbstractRoot)] ───────────────────────────

/// Derive a polymorphic family root (Slice 4, Decision #3).
///
/// ```ignore
/// #[derive(AbstractRoot)]
/// #[fluessig(abstract_root(Commit, Tree, Blob), tag_col = "obj_type", ref_col = "obj_oid")]
/// pub struct GitObject {
///     #[key] pub oid: String,
///     pub repo_id: Id<Repo>,
/// }
/// // generates: pub enum GitObjectId { Commit(String), Tree(String), Blob(String) }
/// //            impl AbstractRoot for GitObject { type Id = GitObjectId; }
/// //            impl Entity for GitObject { … abstract, carries (oid, repo_id) … }
/// ```
///
/// The enum carries the family key per variant — heterogeneous across families
/// (a composite-keyed root generates `GhSubjectId { GhPullRequest(Id<Repo>, i32),
/// … }`). Leaves point back with `#[fluessig(extends = GitObject)]`.
#[proc_macro_derive(AbstractRoot, attributes(fluessig, key))]
pub fn derive_abstract_root(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let opts = match AbstractRootOpts::from_derive_input(&input) {
        Ok(o) => o,
        Err(e) => return e.write_errors().into(),
    };
    match expand_abstract_root(opts) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_abstract_root(opts: AbstractRootOpts) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &opts.ident;
    let fields = opts
        .data
        .take_struct()
        .expect("supports(struct_named) guarantees a struct")
        .fields;

    let leaves = &opts.abstract_root.0;
    if leaves.is_empty() {
        return Err(syn::Error::new_spanned(
            ident,
            "abstract_root(...) needs at least one leaf, e.g. abstract_root(Commit, Tree, Blob)",
        ));
    }

    // The family key's field types, in declaration order — the payload each
    // generated enum variant carries. Heterogeneous families fall out for free:
    // the tokens are whatever the key fields are typed (a scalar, an `Id<T>`, …).
    let key_types: Vec<&Type> = fields
        .iter()
        .filter(|f| f.key || has_bare_key(&f.attrs))
        .map(|f| &f.ty)
        .collect();
    if key_types.is_empty() {
        return Err(syn::Error::new_spanned(
            ident,
            "an abstract_root must declare the family key with #[key]",
        ));
    }

    let id_enum = Ident::new(&format!("{ident}Id"), ident.span());
    let variants = leaves
        .iter()
        .map(|leaf| quote! { #leaf ( #( #key_types ),* ) });

    let leaf_names: Vec<String> = leaves.iter().map(|l| l.to_string()).collect();
    let id_enum_str = id_enum.to_string();
    let descriptor = entity_descriptor_impl(EntityDescriptorArgs {
        ident,
        attrs: &opts.attrs,
        fields: &fields,
        name: opts.name.as_deref(),
        ref_cols: &opts.ref_cols,
        extends: None,
        abstract_leaves: &leaf_names,
        id_enum: Some(&id_enum_str),
        tag_col: opts.tag_col.as_deref(),
        ref_col: opts.ref_col.as_deref(),
    })?;

    Ok(quote! {
        /// The generated family key enum (Slice 4, Decision #3): one variant per
        /// leaf, each carrying the family key. Discoverable as
        /// `<Root as fluessig_derive::AbstractRoot>::Id`.
        #[derive(Debug, Clone, PartialEq)]
        pub enum #id_enum { #( #variants ),* }

        impl ::fluessig_derive::AbstractRoot for #ident {
            type Id = #id_enum;
        }

        #descriptor
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

    let span = span_tokens(ident.span());
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
                    span: #span,
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
    } else if let Some((id_enum, poly_nullable)) = poly_ref_type(&field.ty) {
        // A field typed `<Root>Id` is a polymorphic family reference (Slice 4);
        // `cols(tag = …, key = …)` carries any per-site spelling override.
        let tag = option_str(field.cols.as_ref().and_then(|c| c.tag.as_deref()));
        let key = option_str(field.cols.as_ref().and_then(|c| c.key.as_deref()));
        (
            quote! { ::fluessig_derive::FieldKind::PolyReference(
                ::fluessig_derive::PolyRef { id_enum: #id_enum, tag_col: #tag, ref_col: #key }
            ) },
            poly_nullable,
        )
    } else {
        map_field_type(&field.ty)?
    };

    let span = field_span_tokens(field);
    Ok(quote! {
        ::fluessig_derive::FieldDescriptor {
            name: #fname,
            kind: #kind,
            nullable: #nullable,
            key: #is_key,
            doc: #doc_tokens,
            shares: #shares_tokens,
            span: #span,
        }
    })
}

/// The `SourceSpan { file, line }` tokens for a declaration at `span` (Slice 6).
/// `file!()` / `line!()` are emitted *carrying* `span`, so rustc resolves the
/// built-ins against the declaration's real source location — the `.rs` file:line
/// the design (`derive-front-end.md` §2.1) wants loader diagnostics to point at.
/// This is the `file!()`/`line!()` route the design suggests over
/// `proc_macro2::Span::start()`, which is unreliable on stable; the built-ins
/// resolve per-field with exact line fidelity.
fn span_tokens(span: proc_macro2::Span) -> proc_macro2::TokenStream {
    let file = quote_spanned!(span=> ::core::file!());
    let line = quote_spanned!(span=> ::core::line!());
    quote! { ::fluessig_derive::SourceSpan { file: #file, line: #line } }
}

/// The span tokens for a named struct field — its field-name ident's location.
fn field_span_tokens(field: &FluField) -> proc_macro2::TokenStream {
    span_tokens(field.ident.as_ref().expect("named field").span())
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

/// A field typed `<Root>Id` — a bare single-segment path identifier ending in
/// `Id` (capital I), NOT the generic `Id<T>` and NOT `Id` itself — is a
/// polymorphic family reference (Slice 4, Decision #3: reference sites name the
/// generated key enum natively). Returns the enum type name + nullability;
/// `Id<T>` (angle-bracketed) and scalar primitives fall through to the Slice-2
/// mapping. The `<Root>Id` naming convention is the signal — the enum resolves to
/// its family at lowering, so a name that doesn't is caught there, not here.
fn poly_ref_type(ty: &Type) -> Option<(String, bool)> {
    let (inner, nullable) = match option_inner(ty) {
        Some(i) => (i, true),
        None => (ty, false),
    };
    let Type::Path(tp) = inner else {
        return None;
    };
    if tp.qself.is_some() || tp.path.segments.len() != 1 {
        return None;
    }
    let seg = &tp.path.segments[0];
    // a generic (`Id<T>`, `Option<…>`) is not a family enum name
    if !matches!(seg.arguments, PathArguments::None) {
        return None;
    }
    let name = seg.ident.to_string();
    // `<Root>Id`: ends in "Id" with a nonempty root before it. (Bare `Id` is the
    // Slice-2 FK marker; scalars like `Oid` end in a lowercase "id" and miss.)
    (name.len() > 2 && name.ends_with("Id")).then_some((name, nullable))
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
         polymorphic family references <Root>Id, Option<…> of any, and a \
         #[fluessig(flatten)] embedded struct.",
    )
}

/// `Wrapper<T>` (matched by the last path segment's name) → `Some(T)` — the
/// single-type-argument extractor shared by the `Option<T>` / `Vec<T>` lookups.
fn single_type_arg<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != wrapper {
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

/// `Option<T>` → `Some(T)`.
fn option_inner(ty: &Type) -> Option<&Type> {
    single_type_arg(ty, "Option")
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
    /// The `api: [Entl, …]` op roots — types whose `#[fluessig::export] impl`
    /// blocks are lowered into `api.json` alongside the entity catalog (Slice 5).
    api: Vec<Ident>,
}

impl Parse for CatalogInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut entities = None;
        let mut edges = Vec::new();
        let mut api = Vec::new();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            match key.to_string().as_str() {
                "name" => name = Some(input.parse::<LitStr>()?),
                "version" => version = Some(parse_version(input)?),
                "entities" => entities = Some(parse_ident_list(input)?),
                "edges" => edges = parse_ident_list(input)?,
                "api" => api = parse_ident_list(input)?,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown catalog! field `{other}` — supported: \
                             name, version, entities, edges, api"
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
            api,
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

/// Collect `#[derive(Entity)]` / `#[derive(Edge)]` types + `#[fluessig::export]`
/// op roots into a catalog exporter.
///
/// Expands to a module `fluessig_catalog` exposing:
/// * `catalog() -> fluessig::Catalog` / `to_json()` — the `catalog.json` layer;
/// * `api() -> fluessig::api::ApiDoc` / `api_to_json()` — the `api.json` op layer
///   (Slice 5), lowered from the `api:` root list's `#[fluessig::export]` impls.
///
/// ```ignore
/// fluessig_derive::catalog! {
///     name: "git_demo",
///     version: "0.1.0",
///     entities: [Repo, Commit],
///     edges: [CommitParent],
///     api: [Entl],
/// }
/// ```
#[proc_macro]
pub fn catalog(input: TokenStream) -> TokenStream {
    let CatalogInput {
        name,
        version,
        entities,
        edges,
        api,
    } = parse_macro_input!(input as CatalogInput);

    // The generated `fluessig_catalog` module nests one level below the
    // invocation scope, so entity/edge/api paths are reached through `super::`.
    let entity_descriptors = entities.iter().map(|e| {
        quote! { <super::#e as ::fluessig_derive::Entity>::DESCRIPTOR }
    });
    let edge_descriptors = edges.iter().map(|e| {
        quote! { <super::#e as ::fluessig_derive::Edge>::DESCRIPTOR }
    });
    let api_descriptors = api.iter().map(|a| {
        quote! { <super::#a as ::fluessig_derive::ApiExport>::DESCRIPTOR }
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

            /// The op-interface descriptors listed in `catalog!`'s `api:`, in
            /// declaration order (Slice 5). Empty when no `api:` roots are given.
            pub const API: &[&'static ::fluessig_derive::InterfaceDescriptor] =
                &[ #( #api_descriptors ),* ];

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

            /// Build the in-memory `api.json` op-layer IR from the `api:` roots.
            pub fn api() -> ::fluessig_derive::fluessig::api::ApiDoc {
                ::fluessig_derive::build_api(NAME, VERSION, API)
            }

            /// Render the `api.json` text the loader + bindgen consume (Slice 5).
            pub fn api_to_json() -> ::std::string::String {
                ::fluessig_derive::to_api_json(NAME, VERSION, API)
            }
        }
    }
    .into()
}

// ─────────────────────────── #[fluessig::export] ───────────────────────────

/// Capture a `#[fluessig::export] impl` block as an op interface (Slice 5,
/// `derive-front-end.md` §2.7 — "the impl that actually runs IS the interface").
///
/// Each method's signature is captured into an `OpDescriptor` (name, params,
/// return, op kind); the whole set expands to an
/// `impl fluessig_derive::ApiExport for Self` carrying a
/// `&'static InterfaceDescriptor`, which `catalog!`'s `api:` root list lowers
/// into `api.json`. The impl block itself is re-emitted unchanged except that the
/// op-kind tags are consumed, so the methods still compile and run.
///
/// ```ignore
/// #[fluessig::export]
/// impl Entl {
///     #[fluessig(ctor)]   pub fn open(path: &str) -> Entl { … }
///                         pub fn commit(&self, oid: &str) -> Option<Commit> { … }
///     #[fluessig(stream)] pub fn commits(&self) -> impl Iterator<Item = Commit> { … }
///     #[fluessig(manual)] pub fn watch(&self, secs: i32) { … }
/// }
/// ```
///
/// Op kinds: `#[fluessig(ctor)]` (a constructor — `void` on the surface), an
/// untagged method (plain unary), `#[fluessig(stream)]` (returns
/// `impl Iterator<Item = T>` — its `Item` is the per-batch return), and
/// `#[fluessig(manual)]` (recorded but hand-written per binding). A
/// `fluessig::Result<T>` return is transparent (unwrapped to `T`).
#[proc_macro_attribute]
pub fn export(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemImpl);
    match expand_export(item) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_export(item: ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
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
    let (kind_tokens, kind) = method_kind(&f.attrs)?;
    let params = param_descriptors(&f.sig)?;
    let returns = return_descriptor(kind, &f.sig)?;
    let span = span_tokens(f.sig.ident.span());
    Ok(quote! {
        ::fluessig_derive::OpDescriptor {
            name: #name,
            doc: #doc,
            kind: #kind_tokens,
            params: &[ #( #params ),* ],
            returns: #returns,
            span: #span,
        }
    })
}

/// The op kind of a method: the first `#[fluessig(ctor|stream|manual)]` tag, else
/// plain unary. Returns the `OpKind` tokens and the parsed kind (the return
/// lowering needs the kind — a `ctor` is `void`, a `stream` unwraps its `Item`).
fn method_kind(attrs: &[Attribute]) -> syn::Result<(proc_macro2::TokenStream, OpKindChoice)> {
    for a in attrs {
        if a.path().is_ident("fluessig") {
            let id: Ident = a.parse_args()?;
            let choice = match id.to_string().as_str() {
                "ctor" => OpKindChoice::Ctor,
                "stream" => OpKindChoice::Stream,
                "manual" => OpKindChoice::Manual,
                other => {
                    return Err(syn::Error::new_spanned(
                        &id,
                        format!(
                            "unknown op kind `{other}` — an exported method is tagged \
                             #[fluessig(ctor)], #[fluessig(stream)], #[fluessig(manual)], \
                             or left untagged (a plain unary op)"
                        ),
                    ))
                }
            };
            return Ok((choice.tokens(), choice));
        }
    }
    Ok((OpKindChoice::Unary.tokens(), OpKindChoice::Unary))
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

/// `&T` / `&mut T` → `T` (recursively) — an op param spelled `&str` is a `string`.
fn deref(ty: &Type) -> &Type {
    match ty {
        Type::Reference(r) => deref(&r.elem),
        other => other,
    }
}

/// `Vec<T>` → `Some(T)`.
fn vec_inner(ty: &Type) -> Option<&Type> {
    single_type_arg(ty, "Vec")
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

/// `ty` is the single-segment path `name` (no generics) — e.g. `is_named(t, "u8")`.
fn is_named(ty: &Type, name: &str) -> bool {
    let Type::Path(tp) = ty else { return false };
    tp.qself.is_none()
        && tp.path.segments.len() == 1
        && tp.path.segments[0].ident == name
        && matches!(tp.path.segments[0].arguments, PathArguments::None)
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
