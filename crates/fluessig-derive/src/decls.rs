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

/// A named **tagged union** (`#[derive(Union)]`) — a closed set of variants, each a
/// wire tag carrying one body type. It lowers to the catalog's `unions` (and the
/// op layer's `api.json` `unions` when an op transitively references it), and a
/// field typed by the union lowers to `TypeRef::Union`. Carries disponent's
/// `union EventPayload` (nine variants: state / message / toolCall / …). Surfaced
/// by the disponent migration acid test — entl exercised no unions.
pub trait UnionType {
    /// The descriptor the derive expands to.
    const DESCRIPTOR: &'static UnionDescriptor;
}

/// A whole union captured by `#[derive(Union)]`: its catalog name, doc, and
/// variants. Lowers to `fluessig::ir::UnionDef` (catalog) and — when referenced —
/// `fluessig::api::ApiUnion` (op layer).
#[derive(Debug, Clone, Copy)]
pub struct UnionDescriptor {
    /// The union's catalog name (its `unions` entry name in `catalog.json`).
    pub name: &'static str,
    /// The union's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The variants, in declaration order.
    pub variants: &'static [UnionVariantDescriptor],
    /// The `.rs` source location of this union's declaration (Slice 6 spans).
    pub span: SourceSpan,
}

/// One union variant: its wire tag (the discriminator — a Rust enum variant name
/// lowerCamelCased, or a per-variant `#[fluessig(tag = "…")]` override) and the
/// name of its single body type, resolved at lowering against the catalog's
/// declared enums / scalars / value structs (exactly as an entity's `Named` field
/// is — a variant body `StateChange` → a value-struct ref).
#[derive(Debug, Clone, Copy)]
pub struct UnionVariantDescriptor {
    /// The variant's wire tag (`state`, `toolCall`, …).
    pub tag: &'static str,
    /// The variant's body type name (`StateChange`, `AgentMessage`, …).
    pub ty: &'static str,
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
