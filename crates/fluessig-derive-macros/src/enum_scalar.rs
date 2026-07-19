//! `#[derive(Enum)]` + `#[derive(Scalar)]` expansion (Slice 8b) — entl's six
//! enums and its `Oid` / `ArrowBatch` semantic scalars. The proc-macro entry
//! points stay in the crate root (proc-macro derives must); the darling option
//! shapes + token lowering live here, split out to keep the root under the
//! file-size budget (mirroring `record.rs`).

use darling::ast::Data;
use darling::util::Ignored;
use darling::{FromDeriveInput, FromVariant};
use quote::quote;
use syn::Ident;

use crate::{doc_string, option_str, span_tokens};

// ─────────────────────────── #[derive(Enum)] ───────────────────────────

/// The container-level options on a `#[derive(Enum)]` enum:
/// `#[fluessig(rename_all = "SCREAMING_SNAKE_CASE")]` + per-variant
/// `#[fluessig(value = "A", name = "…")]`.
#[derive(FromDeriveInput)]
#[darling(attributes(fluessig), supports(enum_any), forward_attrs(doc))]
pub(crate) struct EnumOpts {
    ident: Ident,
    attrs: Vec<syn::Attribute>,
    data: Data<FluVariant, Ignored>,
    /// A casing rule applied to every variant's catalog name — `lowercase`,
    /// `UPPERCASE`, or `SCREAMING_SNAKE_CASE`. A per-variant `name` overrides it.
    #[darling(default)]
    rename_all: Option<String>,
}

/// One `#[derive(Enum)]` variant: its Rust ident, an optional stored wire `value`
/// (`added: "A"`), and an optional explicit catalog `name` override.
#[derive(FromVariant)]
#[darling(attributes(fluessig))]
pub(crate) struct FluVariant {
    ident: Ident,
    #[darling(default)]
    value: Option<String>,
    #[darling(default)]
    name: Option<String>,
}
pub(crate) fn expand_enum(opts: EnumOpts) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &opts.ident;
    let variants = opts
        .data
        .take_enum()
        .expect("supports(enum_any) guarantees an enum");
    let rename = opts.rename_all.as_deref();

    let variant_tokens = variants.iter().map(|v| {
        // catalog name: explicit `name` override, else `rename_all`, else the ident.
        let cat_name = v
            .name
            .clone()
            .unwrap_or_else(|| apply_rename(&v.ident.to_string(), rename));
        let value_tokens = option_str(v.value.as_deref());
        quote! {
            ::fluessig_derive::EnumVariantDescriptor { name: #cat_name, value: #value_tokens }
        }
    });

    let name_str = ident.to_string();
    let doc_tokens = option_str(doc_string(&opts.attrs).as_deref());
    let span = span_tokens(ident.span());
    Ok(quote! {
        impl ::fluessig_derive::EnumType for #ident {
            const DESCRIPTOR: &'static ::fluessig_derive::EnumDescriptor =
                &::fluessig_derive::EnumDescriptor {
                    name: #name_str,
                    doc: #doc_tokens,
                    variants: &[ #( #variant_tokens ),* ],
                    span: #span,
                };
        }
    })
}

/// Apply a `rename_all` casing rule to a PascalCase variant ident. The rules the
/// fixtures need: `lowercase` (`Branch` → `branch`), `UPPERCASE`,
/// `SCREAMING_SNAKE_CASE` (`Open` → `OPEN`, `PullRequest` → `PULL_REQUEST`), and
/// `snake_case` (`ExeDev` → `exe_dev`, `ToolCall` → `tool_call` — disponent's enum
/// wire values are its snake_case member names). `None` leaves the ident unchanged.
pub(crate) fn apply_rename(ident: &str, rule: Option<&str>) -> String {
    match rule {
        Some("lowercase") => ident.to_lowercase(),
        Some("UPPERCASE") => ident.to_uppercase(),
        Some("SCREAMING_SNAKE_CASE") => screaming_snake(ident, true),
        Some("snake_case") => screaming_snake(ident, false),
        _ => ident.to_string(),
    }
}

/// PascalCase → snake_case, optionally SCREAMING: insert `_` before each interior
/// uppercase boundary, then case each character. `PullRequest` → `pull_request`
/// (or `PULL_REQUEST` when `scream`).
fn screaming_snake(ident: &str, scream: bool) -> String {
    let mut out = String::new();
    for (i, c) in ident.chars().enumerate() {
        if c.is_ascii_uppercase() && i != 0 {
            out.push('_');
        }
        out.push(if scream {
            c.to_ascii_uppercase()
        } else {
            c.to_ascii_lowercase()
        });
    }
    out
}

// ─────────────────────────── #[derive(Scalar)] ───────────────────────────

/// The container-level options on a `#[derive(Scalar)]` type:
/// `#[fluessig(extends = "bytes")]`.
#[derive(FromDeriveInput)]
#[darling(attributes(fluessig), supports(struct_any), forward_attrs(doc))]
pub(crate) struct ScalarOpts {
    ident: Ident,
    attrs: Vec<syn::Attribute>,
    /// Required by darling's derive-input shape; a scalar's fields are irrelevant
    /// (it's a marker type — `struct Oid;` / `struct Oid(Vec<u8>)`).
    #[allow(dead_code)]
    data: Data<Ignored, Ignored>,
    /// The physical carrier this scalar refines (`scalar Oid extends bytes`).
    #[darling(default)]
    extends: Option<String>,
}

/// Expand a `#[derive(Scalar)]` type to an `impl fluessig_derive::ScalarType`.
pub(crate) fn expand_scalar(opts: ScalarOpts) -> proc_macro2::TokenStream {
    let ident = &opts.ident;
    let name_str = ident.to_string();
    let base_tokens = option_str(opts.extends.as_deref());
    let doc_tokens = option_str(doc_string(&opts.attrs).as_deref());
    let span = span_tokens(ident.span());
    quote! {
        impl ::fluessig_derive::ScalarType for #ident {
            const DESCRIPTOR: &'static ::fluessig_derive::ScalarDescriptor =
                &::fluessig_derive::ScalarDescriptor {
                    name: #name_str,
                    base: #base_tokens,
                    doc: #doc_tokens,
                    span: #span,
                };
        }
    }
}
