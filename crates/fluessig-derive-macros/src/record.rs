//! `#[derive(Record)]` expansion (Slice 8a Gap 2) — the DTO / value-struct derive.
//!
//! A record is flat data the op surface passes across (no identity, no key, no
//! entity FK relations); its fields are scalars, references to other records, and
//! lists / `Option`s thereof. The proc-macro entry point lives at the crate root
//! (proc-macro derives must); the parsing + token lowering live here. Shared field
//! helpers (`FluField`, `option_str`, `doc_string`, span/type helpers) are the
//! root module's — visible to this descendant module.

use darling::ast::Data;
use darling::util::Ignored;
use darling::FromDeriveInput;
use quote::quote;
use syn::{Ident, PathArguments, Type};

use crate::{
    doc_string, field_span_tokens, ident_name, option_inner, option_str, scalar_kind, span_tokens,
    vec_inner, FluField,
};

/// The container-level options on a `#[derive(Record)]` DTO struct — no attributes
/// today beyond the doc comment; records are flat data.
#[derive(FromDeriveInput)]
#[darling(attributes(fluessig), supports(struct_named), forward_attrs(doc))]
pub(crate) struct RecordOpts {
    ident: Ident,
    attrs: Vec<syn::Attribute>,
    data: Data<Ignored, FluField>,
}

/// Expand a `#[derive(Record)]` struct to an `impl fluessig_derive::Record`
/// carrying its `&'static RecordDescriptor`.
pub(crate) fn expand_record(opts: RecordOpts) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &opts.ident;
    let fields = opts
        .data
        .take_struct()
        .expect("supports(struct_named) guarantees a struct")
        .fields;
    let name_str = ident.to_string();
    let doc_tokens = option_str(doc_string(&opts.attrs).as_deref());

    let mut field_descriptors = Vec::new();
    for field in &fields {
        field_descriptors.push(record_field_tokens(field)?);
    }
    let span = span_tokens(ident.span());
    Ok(quote! {
        impl ::fluessig_derive::Record for #ident {
            const DESCRIPTOR: &'static ::fluessig_derive::RecordDescriptor =
                &::fluessig_derive::RecordDescriptor {
                    name: #name_str,
                    doc: #doc_tokens,
                    fields: &[ #( #field_descriptors ),* ],
                    span: #span,
                };
        }
    })
}

/// Emit the `RecordFieldDescriptor { … }` tokens for one record field.
fn record_field_tokens(field: &FluField) -> syn::Result<proc_macro2::TokenStream> {
    let fname = ident_name(field.ident.as_ref().expect("named field"));
    let doc_tokens = option_str(doc_string(&field.attrs).as_deref());
    let (ty_tokens, nullable) = record_type(&field.ty)?;
    let span = field_span_tokens(field);
    Ok(quote! {
        ::fluessig_derive::RecordFieldDescriptor {
            name: #fname,
            ty: #ty_tokens,
            nullable: #nullable,
            doc: #doc_tokens,
            span: #span,
        }
    })
}

/// Map a record field type to `(RecordTypeDesc tokens, nullable)`. `Option<T>` ⇒
/// nullable (unwrapping to `T`); everything else via [`record_type_inner`].
fn record_type(ty: &Type) -> syn::Result<(proc_macro2::TokenStream, bool)> {
    if let Some(inner) = option_inner(ty) {
        let (tokens, already_opt) = record_type(inner)?;
        if already_opt {
            return Err(syn::Error::new_spanned(
                ty,
                "nested Option is not supported",
            ));
        }
        return Ok((tokens, true));
    }
    Ok((record_type_inner(ty)?, false))
}

/// The non-`Option` record type token: `Vec<T>` ⇒ a list; a primitive ⇒ a scalar;
/// any other bare type name ⇒ a `Named` type (a declared enum / semantic scalar /
/// value-struct reference), resolved at lowering against the catalog's declared
/// enums / scalars (Slice 8b — `FileDiff.status: FileStatus` → `{ enum }`,
/// `ChangeBatch.ipc: ArrowBatch` → the scalar, `SinkOptions.rename: Vec<TableRename>`
/// → the record reference).
fn record_type_inner(ty: &Type) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(elem) = vec_inner(ty) {
        let inner = record_type_inner(elem)?;
        return Ok(quote! { ::fluessig_derive::RecordTypeDesc::List(&#inner) });
    }
    if let Ok(kind) = scalar_kind(ty) {
        return Ok(quote! { ::fluessig_derive::RecordTypeDesc::Scalar(#kind) });
    }
    let name = bare_type_name(ty)?;
    Ok(quote! { ::fluessig_derive::RecordTypeDesc::Named(#name) })
}

/// A single-segment, non-generic type path's name — a record's reference to
/// another record (`renames: Vec<TableRename>` ⇒ `"TableRename"`).
fn bare_type_name(ty: &Type) -> syn::Result<String> {
    let Type::Path(tp) = ty else {
        return Err(unsupported_record_type(ty));
    };
    if tp.qself.is_some() {
        return Err(unsupported_record_type(ty));
    }
    match tp.path.segments.last() {
        Some(seg) if matches!(seg.arguments, PathArguments::None) => Ok(seg.ident.to_string()),
        _ => Err(unsupported_record_type(ty)),
    }
}

fn unsupported_record_type(ty: &Type) -> syn::Error {
    syn::Error::new_spanned(
        ty,
        "unsupported record field type — supported: scalar primitives (i8..i64, \
         u8..u64, f32/f64, bool, String), a reference to another record by name, \
         and `Vec<…>` / `Option<…>` of any.",
    )
}
