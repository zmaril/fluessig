//! Field-level attribute grammar added in Slice 8b: `#[fluessig(default = …)]`
//! (a DDL default) and `#[fluessig(derived(exists|count, of = "rel", filter(k = v)))]`
//! (a derived field). Split out of the crate root to keep it under the file-size
//! budget; the shared field helpers (`lit_str`, `option_str`, `parse_meta_list`)
//! are the root module's, re-used here.

use darling::ast::NestedMeta;
use darling::FromMeta;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::Token;

use crate::lit_str;

/// A `#[fluessig(default = …)]` literal (Slice 8b) — parsed off the attribute and
/// re-emitted as a `fluessig_derive::DefaultLit`.
#[derive(Clone)]
pub(crate) enum DefaultLitMeta {
    Int(i64),
    Bool(bool),
    Float(f64),
    Str(String),
}

impl FromMeta for DefaultLitMeta {
    fn from_value(value: &syn::Lit) -> darling::Result<Self> {
        match value {
            syn::Lit::Int(i) => Ok(DefaultLitMeta::Int(i.base10_parse()?)),
            syn::Lit::Bool(b) => Ok(DefaultLitMeta::Bool(b.value)),
            syn::Lit::Float(f) => Ok(DefaultLitMeta::Float(f.base10_parse()?)),
            syn::Lit::Str(s) => Ok(DefaultLitMeta::Str(s.value())),
            other => Err(darling::Error::unexpected_lit_type(other)),
        }
    }
    fn from_bool(value: bool) -> darling::Result<Self> {
        Ok(DefaultLitMeta::Bool(value))
    }
    fn from_string(value: &str) -> darling::Result<Self> {
        Ok(DefaultLitMeta::Str(value.to_string()))
    }
}

impl DefaultLitMeta {
    /// The `fluessig_derive::DefaultLit` tokens this literal expands to.
    pub(crate) fn tokens(&self) -> proc_macro2::TokenStream {
        match self {
            DefaultLitMeta::Int(i) => quote! { ::fluessig_derive::DefaultLit::Int(#i) },
            DefaultLitMeta::Bool(b) => quote! { ::fluessig_derive::DefaultLit::Bool(#b) },
            DefaultLitMeta::Float(f) => quote! { ::fluessig_derive::DefaultLit::Float(#f) },
            DefaultLitMeta::Str(s) => quote! { ::fluessig_derive::DefaultLit::Str(#s) },
        }
    }
}

/// A `#[fluessig(derived(exists|count, of = "rel", filter(k = v)))]` declaration
/// (Slice 8b) — one aggregate over one same-entity to-many relation, filtered by
/// literal equality on the relation's edge properties.
#[derive(Default)]
pub(crate) struct DerivedMeta {
    agg: String,
    of: String,
    filter: Vec<(String, i64)>,
}

impl FromMeta for DerivedMeta {
    fn from_list(items: &[NestedMeta]) -> darling::Result<Self> {
        let mut agg = None;
        let mut of = None;
        let mut filter = Vec::new();
        let mut errors = darling::Error::accumulator();
        for item in items {
            match item {
                // the bare aggregate word: `exists` / `count`
                NestedMeta::Meta(syn::Meta::Path(p)) if agg.is_none() => match p.get_ident() {
                    Some(id) => agg = Some(id.to_string()),
                    None => errors.push(
                        darling::Error::custom("derived(...) expects a bare aggregate name")
                            .with_span(p),
                    ),
                },
                // `of = "relation"`
                NestedMeta::Meta(syn::Meta::NameValue(nv)) if nv.path.is_ident("of") => {
                    match lit_str(&nv.value) {
                        Ok(s) => of = Some(s),
                        Err(e) => errors.push(e),
                    }
                }
                // `filter(k = v, …)` — literal-equality on edge properties
                NestedMeta::Meta(syn::Meta::List(list)) if list.path.is_ident("filter") => {
                    match list.parse_args_with(
                        Punctuated::<syn::MetaNameValue, Token![,]>::parse_terminated,
                    ) {
                        Ok(pairs) => {
                            for nv in pairs {
                                let key = nv
                                    .path
                                    .get_ident()
                                    .map(|i| i.to_string())
                                    .unwrap_or_default();
                                if let syn::Expr::Lit(syn::ExprLit {
                                    lit: syn::Lit::Int(i),
                                    ..
                                }) = &nv.value
                                {
                                    match i.base10_parse::<i64>() {
                                        Ok(v) => filter.push((key, v)),
                                        Err(e) => errors.push(e.into()),
                                    }
                                } else {
                                    errors.push(
                                        darling::Error::custom(
                                            "derived filter values must be integer literals",
                                        )
                                        .with_span(&nv.value),
                                    );
                                }
                            }
                        }
                        Err(e) => errors.push(e.into()),
                    }
                }
                other => errors.push(
                    darling::Error::custom(
                        "derived(...) expects `exists|count, of = \"rel\", filter(k = v)`",
                    )
                    .with_span(other),
                ),
            }
        }
        errors.finish()?;
        Ok(DerivedMeta {
            agg: agg.ok_or_else(|| {
                darling::Error::custom("derived(...) needs an aggregate (exists|count)")
            })?,
            of: of
                .ok_or_else(|| darling::Error::custom("derived(...) needs `of = \"relation\"`"))?,
            filter,
        })
    }
}

impl DerivedMeta {
    /// The `fluessig_derive::DerivedDesc` tokens this declaration expands to.
    pub(crate) fn tokens(&self) -> proc_macro2::TokenStream {
        let agg = &self.agg;
        let of = &self.of;
        let pairs = self.filter.iter().map(|(k, v)| quote! { (#k, #v) });
        quote! {
            ::fluessig_derive::DerivedDesc {
                agg: #agg,
                of: #of,
                filter: &[ #( #pairs ),* ],
            }
        }
    }
}
