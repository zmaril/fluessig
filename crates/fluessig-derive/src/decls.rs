//! Slice 8b descriptor types — the `#[derive(Enum)]` / `#[derive(Scalar)]`
//! descriptors + traits, and the `#[fluessig(default/derived)]` payloads. Split
//! out of the crate root to keep it under the file-size budget; re-exported at the
//! root (`pub use decls::*`) so macro-generated code names them as
//! `::fluessig_derive::EnumDescriptor` etc.

use crate::SourceSpan;

/// A named **enum** (`#[derive(Enum)]`, Slice 8b) — a closed set of variants, each
/// with an optional stored wire value (`added: "A"`). It lowers to the catalog's
/// `enums`, and a field typed by the enum lowers to `TypeRef::Enum`. Needed to
/// carry entl's six enums (`RefKind`, `FileStatus`, `PrState`, …).
pub trait EnumType {
    /// The descriptor the derive expands to.
    const DESCRIPTOR: &'static EnumDescriptor;
}

/// A whole enum captured by `#[derive(Enum)]`: its catalog name, doc, and variants
/// (each a catalog name + optional stored wire value). Lowers to
/// `fluessig::ir::EnumDef`.
#[derive(Debug, Clone, Copy)]
pub struct EnumDescriptor {
    /// The enum's catalog name.
    pub name: &'static str,
    /// The enum's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The variants, in declaration order.
    pub variants: &'static [EnumVariantDescriptor],
    /// The `.rs` source location of this enum's declaration (Slice 6 spans).
    pub span: SourceSpan,
}

/// One enum variant: its catalog name (after any `rename_all` / per-variant
/// `name`), and the stored wire value when it differs from the name
/// (`#[fluessig(value = "A")]` → `Some("A")`).
#[derive(Debug, Clone, Copy)]
pub struct EnumVariantDescriptor {
    /// The variant's catalog name.
    pub name: &'static str,
    /// The stored wire value, when it differs from the name.
    pub value: Option<&'static str>,
}

/// A named **semantic scalar** (`#[derive(Scalar)]`, Slice 8b — `scalar Oid extends
/// bytes`): a logical name plus an optional physical carrier (`base`). Lowers to
/// the catalog's `scalars`, and a field typed by the scalar lowers to
/// `TypeRef::Scalar { name, base }`. Carries entl's `Oid` (base `bytes`) and
/// `ArrowBatch` (no base).
pub trait ScalarType {
    /// The descriptor the derive expands to.
    const DESCRIPTOR: &'static ScalarDescriptor;
}

/// A whole scalar captured by `#[derive(Scalar)]`: its catalog name, physical
/// carrier (`#[fluessig(extends = "bytes")]` → `Some("bytes")`), and doc.
#[derive(Debug, Clone, Copy)]
pub struct ScalarDescriptor {
    /// The scalar's catalog name.
    pub name: &'static str,
    /// The physical carrier it refines (`extends`), or `None` for a root scalar.
    pub base: Option<&'static str>,
    /// The scalar's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The `.rs` source location of this scalar's declaration (Slice 6 spans).
    pub span: SourceSpan,
}

/// A `#[fluessig(default = …)]` literal as pure `&'static` data — lowered to a
/// `serde_json::Value` at catalog build (Slice 8b). entl uses integer (`0`) and
/// boolean (`false`) defaults; float / string are carried for completeness.
#[derive(Debug, Clone, Copy)]
pub enum DefaultLit {
    /// An integer default (`@defaultValue(0)`).
    Int(i64),
    /// A boolean default (`@defaultValue(false)`).
    Bool(bool),
    /// A floating-point default.
    Float(f64),
    /// A string default.
    Str(&'static str),
}

impl DefaultLit {
    /// The `serde_json::Value` this literal lowers to in the catalog's `default`
    /// slot — the same shape the TypeSpec `@defaultValue` path emits.
    pub fn to_value(self) -> serde_json::Value {
        match self {
            DefaultLit::Int(i) => serde_json::Value::from(i),
            DefaultLit::Bool(b) => serde_json::Value::from(b),
            DefaultLit::Float(f) => serde_json::Value::from(f),
            DefaultLit::Str(s) => serde_json::Value::from(s),
        }
    }
}

/// A `#[fluessig(derived(…))]` declaration as pure `&'static` data (Slice 8b):
/// one aggregate (`exists` / `count`) over one same-entity to-many relation,
/// filtered by literal equality on the relation's edge properties. Lowers to
/// `fluessig::ir::Derived`.
#[derive(Debug, Clone, Copy)]
pub struct DerivedDesc {
    /// `exists` | `count` — the v1 slice of the closed aggregate family.
    pub agg: &'static str,
    /// The name of a to-many relation field on the same entity.
    pub of: &'static str,
    /// Literal-equality filter on edge-property fields — `(field, value)` pairs
    /// (entl's only use is `filter(idx = 1)`).
    pub filter: &'static [(&'static str, i64)],
}
