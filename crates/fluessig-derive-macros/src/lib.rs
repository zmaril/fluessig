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
    GenericArgument, Ident, ItemImpl, LitStr, PathArguments, Token, Type,
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
    /// `#[fluessig(default = 0 | false | …)]` — the column's DDL DEFAULT (Slice 8b).
    #[darling(default)]
    default: Option<DefaultLitMeta>,
    /// `#[fluessig(derived(exists|count, of = "rel", filter(k = v)))]` — a derived
    /// field (Slice 8b).
    #[darling(default)]
    derived: Option<DerivedMeta>,
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
pub(crate) fn parse_meta_list<T>(
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
pub(crate) fn lit_str(expr: &syn::Expr) -> darling::Result<String> {
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
        // A polymorphic edge side (Slice 8b): a field typed `<Root>Id` whose family
        // is the edge's `from` is the source, whose family is the `to` is the target
        // (`gh_labeled.subject: GhSubjectId`, `tree_entries.child: GitObjectId`).
        let poly_root = poly_ref_type(&field.ty)
            .map(|(id_enum, _)| id_enum.strip_suffix("Id").unwrap_or(&id_enum).to_string());
        let role = match ref_target(&field.ty)? {
            Some(t) if t == from && !source_taken => {
                source_taken = true;
                quote! { ::fluessig_derive::EdgeRole::Source }
            }
            Some(t) if t == to && !target_taken => {
                target_taken = true;
                quote! { ::fluessig_derive::EdgeRole::Target }
            }
            _ => match poly_root.as_deref() {
                Some(root) if root == from_str && !source_taken => {
                    source_taken = true;
                    quote! { ::fluessig_derive::EdgeRole::Source }
                }
                Some(root) if root == to_str && !target_taken => {
                    target_taken = true;
                    quote! { ::fluessig_derive::EdgeRole::Target }
                }
                _ => quote! { ::fluessig_derive::EdgeRole::Property },
            },
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

// ─────────────────── #[derive(Enum)] / #[derive(Scalar)] ───────────────────

/// The `#[derive(Enum)]` / `#[derive(Scalar)]` option shapes + token lowering
/// live in [`enum_scalar`] (Slice 8b); only the proc-macro entry points sit at
/// the crate root.
mod enum_scalar;

/// Derive an `impl fluessig_derive::EnumType` carrying a `&'static EnumDescriptor`
/// (Slice 8b) — entl's `RefKind` / `FileStatus` / `PrState` / … enums.
#[proc_macro_derive(Enum, attributes(fluessig))]
pub fn derive_enum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let opts = match enum_scalar::EnumOpts::from_derive_input(&input) {
        Ok(o) => o,
        Err(e) => return e.write_errors().into(),
    };
    match enum_scalar::expand_enum(opts) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Derive an `impl fluessig_derive::ScalarType` carrying a `&'static
/// ScalarDescriptor` (Slice 8b) — entl's `Oid` (base `bytes`) and `ArrowBatch`.
#[proc_macro_derive(Scalar, attributes(fluessig))]
pub fn derive_scalar(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let opts = match enum_scalar::ScalarOpts::from_derive_input(&input) {
        Ok(o) => o,
        Err(e) => return e.write_errors().into(),
    };
    enum_scalar::expand_scalar(opts).into()
}

// ─────────────────────────── #[derive(Record)] ───────────────────────────

/// The `#[derive(Record)]` expansion lives in [`record`]; only the proc-macro
/// entry point can sit at the crate root. `#[fluessig(default/derived)]`'s
/// grammar lives in [`field_meta`].
mod field_meta;
mod record;

use field_meta::{DefaultLitMeta, DerivedMeta};

mod union;

/// Derive an `impl fluessig_derive::UnionType` carrying a `&'static
/// UnionDescriptor` — disponent's `union EventPayload`. The union is a Rust `enum`
/// whose single-field tuple variants carry each alternative's body type; the wire
/// tag is the variant name lowerCamelCased (`ToolCall` → `toolCall`) or a
/// per-variant `#[fluessig(tag = "…")]` override.
///
/// ```ignore
/// #[derive(Union)]
/// pub enum EventPayload {
///     State(StateChange),        // tag "state"
///     ToolCall(ToolCallInfo),    // tag "toolCall"
/// }
/// ```
#[proc_macro_derive(Union, attributes(fluessig))]
pub fn derive_union(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let opts = match union::UnionOpts::from_derive_input(&input) {
        Ok(o) => o,
        Err(e) => return e.write_errors().into(),
    };
    match union::expand_union(opts) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Derive a `&'static RecordDescriptor` for a DTO / value struct (Slice 8a Gap 2).
///
/// A record is flat data the op surface passes across — no identity, no key, no
/// entity FK relations. Fields are scalars, references to other records, and
/// lists / `Option`s thereof.
///
/// ```ignore
/// #[derive(Record)]
/// pub struct SinkOptions {
///     pub path: Option<String>,          // Option<T> ⇒ nullable
///     pub renames: Vec<TableRename>,     // Vec<T> ⇒ a list; TableRename is another Record
/// }
/// ```
#[proc_macro_derive(Record, attributes(fluessig))]
pub fn derive_record(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let opts = match record::RecordOpts::from_derive_input(&input) {
        Ok(o) => o,
        Err(e) => return e.write_errors().into(),
    };
    match record::expand_record(opts) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

// ─────────────────────────── shared field lowering ───────────────────────────

/// Emit the `FieldDescriptor { … }` tokens for one struct field, honouring
/// `flatten`, `#[key]` / `#[fluessig(key)]`, `shares(…)`, doc comments, and the
/// scalar / `Id<T>` type mapping.
fn field_descriptor_tokens(field: &FluField) -> syn::Result<proc_macro2::TokenStream> {
    let fname = ident_name(field.ident.as_ref().expect("named field"));
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

    let default_tokens = opt_tokens(field.default.as_ref().map(|d| d.tokens()));
    let derived_tokens = opt_tokens(field.derived.as_ref().map(|d| d.tokens()));

    let span = field_span_tokens(field);
    Ok(quote! {
        ::fluessig_derive::FieldDescriptor {
            name: #fname,
            kind: #kind,
            nullable: #nullable,
            key: #is_key,
            doc: #doc_tokens,
            shares: #shares_tokens,
            default: #default_tokens,
            derived: #derived_tokens,
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
pub(crate) fn span_tokens(span: proc_macro2::Span) -> proc_macro2::TokenStream {
    let file = quote_spanned!(span=> ::core::file!());
    let line = quote_spanned!(span=> ::core::line!());
    quote! { ::fluessig_derive::SourceSpan { file: #file, line: #line } }
}

/// The span tokens for a named struct field — its field-name ident's location.
fn field_span_tokens(field: &FluField) -> proc_macro2::TokenStream {
    span_tokens(field.ident.as_ref().expect("named field").span())
}

/// `Some(<inner>)` / `None` tokens from an optional pre-lowered descriptor payload
/// — the `Option<…>` slot for a field's `default` / `derived` tokens (Slice 8b).
fn opt_tokens(inner: Option<proc_macro2::TokenStream>) -> proc_macro2::TokenStream {
    match inner {
        Some(t) => quote! { ::core::option::Option::Some(#t) },
        None => quote! { ::core::option::Option::None },
    }
}

/// `Some("s")` / `None` tokens from an optional string.
pub(crate) fn option_str(s: Option<&str>) -> proc_macro2::TokenStream {
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
/// foreign-key reference to `T`; `DateTime<_>` ⇒ the `utcDateTime` scalar and
/// `Vec<u8>` ⇒ the `bytes` scalar (Slice 8b stock-scalar tokens); a primitive ⇒ a
/// scalar; any other bare single-segment name (`Oid`, `Json`, `RefKind`) ⇒ a
/// `Named` type resolved at lowering against the declared enums / scalars.
fn field_kind(ty: &Type) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(target) = id_target(ty)? {
        let name = target.to_string();
        return Ok(quote! { ::fluessig_derive::FieldKind::Reference(#name) });
    }
    // `chrono::DateTime<Utc>` (or any `DateTime<_>`) is the utcDateTime scalar.
    if is_generic_named(ty, "DateTime") {
        return Ok(quote! { ::fluessig_derive::FieldKind::Named("utcDateTime") });
    }
    // `Vec<u8>` is the bytes scalar (`Blob.content`); any other `Vec<T>` is a list
    // column (`Dispatch.tags: string[]`), its element a scalar primitive or a bare
    // named type resolved at lowering.
    if let Some(elem) = single_type_arg(ty, "Vec") {
        if is_named(elem, "u8") {
            return Ok(quote! { ::fluessig_derive::FieldKind::Named("bytes") });
        }
        let elem_tokens = if let Ok(kind) = scalar_kind(elem) {
            quote! { ::fluessig_derive::ListElem::Scalar(#kind) }
        } else {
            let name = bare_type_name(elem)?;
            quote! { ::fluessig_derive::ListElem::Named(#name) }
        };
        return Ok(quote! { ::fluessig_derive::FieldKind::List(&#elem_tokens) });
    }
    if let Ok(kind) = scalar_kind(ty) {
        return Ok(quote! { ::fluessig_derive::FieldKind::Scalar(#kind) });
    }
    // any other bare, non-generic single-segment name is a declared enum / semantic
    // scalar / value-struct reference — resolved at lowering.
    let name = bare_type_name(ty)?;
    Ok(quote! { ::fluessig_derive::FieldKind::Named(#name) })
}

/// A single-segment generic path whose head ident is `name` (e.g.
/// `is_generic_named(ty, "DateTime")` matches `DateTime<Utc>`).
fn is_generic_named(ty: &Type, name: &str) -> bool {
    let Type::Path(tp) = ty else { return false };
    tp.qself.is_none()
        && tp.path.segments.last().is_some_and(|s| {
            s.ident == name && matches!(s.arguments, PathArguments::AngleBracketed(_))
        })
}

/// A bare, non-generic single-segment type path's name (`Oid`, `Json`, `RefKind`).
/// Errors (via [`unsupported`]) on anything that isn't a plain name.
fn bare_type_name(ty: &Type) -> syn::Result<String> {
    if let Type::Path(tp) = ty {
        if tp.qself.is_none() {
            if let Some(seg) = tp.path.segments.last() {
                if matches!(seg.arguments, PathArguments::None) {
                    return Ok(seg.ident.to_string());
                }
            }
        }
    }
    Err(unsupported(ty))
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
pub(crate) fn option_inner(ty: &Type) -> Option<&Type> {
    single_type_arg(ty, "Option")
}

/// A field/param ident's catalog name, with any raw-identifier `r#` prefix
/// stripped (`r#type` → `type`) so a Rust keyword column matches the `.tsp`
/// spelling (`` `type` `` in TypeSpec).
pub(crate) fn ident_name(ident: &Ident) -> String {
    let s = ident.to_string();
    s.strip_prefix("r#").map(str::to_string).unwrap_or(s)
}

fn has_bare_key(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("key"))
}

/// Collect `///` doc comments (lowered by rustc to `#[doc = "…"]`) into one
/// string, trimming the single leading space rustdoc inserts and joining lines
/// with `\n` — mirroring the TypeSpec emitter's `getDoc`.
pub(crate) fn doc_string(attrs: &[syn::Attribute]) -> Option<String> {
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
    /// The `records: [SinkOptions, …]` DTO roots — `#[derive(Record)]` value
    /// structs that lower to the catalog's `valueStructs` and are materialised
    /// into `api.json`'s `models` when an op references them (Slice 8a Gap 2).
    records: Vec<Ident>,
    /// The `enums: [RefKind, …]` roots — `#[derive(Enum)]` types lowered into the
    /// catalog's `enums` (Slice 8b).
    enums: Vec<Ident>,
    /// The `unions: [EventPayload, …]` roots — `#[derive(Union)]` types lowered into
    /// the catalog's `unions` (and `api.json`'s `unions` when referenced).
    unions: Vec<Ident>,
    /// The `scalars: [Oid, …]` roots — `#[derive(Scalar)]` types lowered into the
    /// catalog's `scalars` (Slice 8b).
    scalars: Vec<Ident>,
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
        let mut records = Vec::new();
        let mut enums = Vec::new();
        let mut unions = Vec::new();
        let mut scalars = Vec::new();
        let mut api = Vec::new();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            match key.to_string().as_str() {
                "name" => name = Some(input.parse::<LitStr>()?),
                "version" => version = Some(parse_version(input)?),
                "entities" => entities = Some(parse_ident_list(input)?),
                "edges" => edges = parse_ident_list(input)?,
                "records" => records = parse_ident_list(input)?,
                "enums" => enums = parse_ident_list(input)?,
                "unions" => unions = parse_ident_list(input)?,
                "scalars" => scalars = parse_ident_list(input)?,
                "api" => api = parse_ident_list(input)?,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown catalog! field `{other}` — supported: \
                             name, version, entities, edges, records, enums, \
                             unions, scalars, api"
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
            records,
            enums,
            unions,
            scalars,
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
        records,
        enums,
        unions,
        scalars,
        api,
    } = parse_macro_input!(input as CatalogInput);

    // The generated `fluessig_catalog` module nests one level below the
    // invocation scope, so each root's descriptor is reached as
    // `<super::Root as fluessig_derive::<Trait>>::DESCRIPTOR` — one shape for every
    // root kind, so the six lists lower through a single helper.
    let descriptors = |roots: &[Ident], tr: proc_macro2::TokenStream| {
        roots
            .iter()
            .map(|r| quote! { <super::#r as ::fluessig_derive::#tr>::DESCRIPTOR })
            .collect::<Vec<_>>()
    };
    let entity_descriptors = descriptors(&entities, quote!(Entity));
    let edge_descriptors = descriptors(&edges, quote!(Edge));
    let record_descriptors = descriptors(&records, quote!(Record));
    let enum_descriptors = descriptors(&enums, quote!(EnumType));
    let union_descriptors = descriptors(&unions, quote!(UnionType));
    let scalar_descriptors = descriptors(&scalars, quote!(ScalarType));
    let api_descriptors = descriptors(&api, quote!(ApiExport));

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

            /// The record (DTO) descriptors listed in `catalog!`'s `records:`, in
            /// declaration order (Slice 8a Gap 2). Empty when none are given.
            pub const RECORDS: &[&'static ::fluessig_derive::RecordDescriptor] =
                &[ #( #record_descriptors ),* ];

            /// The enum descriptors listed in `catalog!`'s `enums:`, in declaration
            /// order (Slice 8b). Empty when none are given.
            pub const ENUMS: &[&'static ::fluessig_derive::EnumDescriptor] =
                &[ #( #enum_descriptors ),* ];

            /// The union descriptors listed in `catalog!`'s `unions:`, in
            /// declaration order. Empty when none are given.
            pub const UNIONS: &[&'static ::fluessig_derive::UnionDescriptor] =
                &[ #( #union_descriptors ),* ];

            /// The scalar descriptors listed in `catalog!`'s `scalars:`, in
            /// declaration order (Slice 8b). Empty when none are given.
            pub const SCALARS: &[&'static ::fluessig_derive::ScalarDescriptor] =
                &[ #( #scalar_descriptors ),* ];

            /// The op-interface descriptors listed in `catalog!`'s `api:`, in
            /// declaration order (Slice 5). Empty when no `api:` roots are given.
            pub const API: &[&'static ::fluessig_derive::InterfaceDescriptor] =
                &[ #( #api_descriptors ),* ];

            /// The catalog name as declared in `catalog!`.
            pub const NAME: &str = #name;
            /// The catalog version as declared in `catalog!`.
            pub const VERSION: &str = #version;

            /// The declared enums + unions + scalars, grouped for the typed builders.
            fn decls() -> ::fluessig_derive::TypeDecls<'static> {
                ::fluessig_derive::TypeDecls { enums: ENUMS, unions: UNIONS, scalars: SCALARS }
            }

            /// Build the in-memory `fluessig::Catalog` IR from the descriptors
            /// (entities + edges + records → `valueStructs`; enums + scalars → the
            /// `enums` / `scalars` arrays, Slice 8b).
            pub fn catalog() -> ::fluessig_derive::fluessig::Catalog {
                ::fluessig_derive::build_catalog_typed(NAME, VERSION, ENTITIES, EDGES, RECORDS, decls())
            }

            /// Render the `catalog.json` text the existing Rust loader consumes.
            pub fn to_json() -> ::std::string::String {
                ::fluessig_derive::to_catalog_json_typed(NAME, VERSION, ENTITIES, EDGES, RECORDS, decls())
            }

            /// Build the in-memory `api.json` op-layer IR from the `api:` roots,
            /// with the `models` materialised from the entities/records the ops
            /// reference (Slice 8a Gap 2).
            pub fn api() -> ::fluessig_derive::fluessig::api::ApiDoc {
                ::fluessig_derive::build_api_typed(NAME, VERSION, ENTITIES, EDGES, RECORDS, API, decls())
            }

            /// Render the `api.json` text the loader + bindgen consume (Slice 5 +
            /// the Slice 8a Gap 2 `models` layer).
            pub fn api_to_json() -> ::std::string::String {
                ::fluessig_derive::to_api_json_typed(NAME, VERSION, ENTITIES, EDGES, RECORDS, API, decls())
            }
        }
    }
    .into()
}

// ─────────────────────────── #[fluessig::export] ───────────────────────────

/// The op-surface capture lives in [`export`]; only the attribute entry point
/// sits at the crate root.
mod export;

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
    match export::expand_export(item) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// `&T` / `&mut T` → `T` (recursively) — an op param spelled `&str` is a `string`.
pub(crate) fn deref(ty: &Type) -> &Type {
    match ty {
        Type::Reference(r) => deref(&r.elem),
        other => other,
    }
}

/// `Vec<T>` → `Some(T)`.
pub(crate) fn vec_inner(ty: &Type) -> Option<&Type> {
    single_type_arg(ty, "Vec")
}

/// `ty` is the single-segment path `name` (no generics) — e.g. `is_named(t, "u8")`.
pub(crate) fn is_named(ty: &Type, name: &str) -> bool {
    let Type::Path(tp) = ty else { return false };
    tp.qself.is_none()
        && tp.path.segments.len() == 1
        && tp.path.segments[0].ident == name
        && matches!(tp.path.segments[0].arguments, PathArguments::None)
}
