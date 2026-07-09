//! The fluessig Rust derive front end — support layer (Slice 1).
//!
//! `derive-front-end.md` §1 splits the front end in two: a **derive → descriptor**
//! step (pure `&'static` data, no behaviour) and a **descriptor → catalog** step
//! (a plain function that prints the same `catalog.json` the Rust loader already
//! consumes). This crate owns the descriptor vocabulary and the printer; the
//! `#[derive(Entity)]` / `catalog!` macros live in `fluessig-derive-macros` and
//! are re-exported here so a downstream crate depends on one name.
//!
//! Slice 1 scope: one scalar-only entity end-to-end. No `Id<T>` / foreign keys,
//! edges, polymorphism, or op surface — those are Slices 2–5
//! (`notes/derive-front-end-decisions.md`).

/// The engine crate, re-exported so macro-generated code can name the IR
/// (`::fluessig_derive::fluessig::Catalog`) without the downstream crate having
/// to depend on `fluessig` directly.
pub use fluessig;

// The derive macro and the trait share the name `Entity` (they live in different
// namespaces — macro vs type — exactly like `serde::Serialize`), so a single
// `use fluessig_derive::Entity;` brings in both.
pub use fluessig_derive_macros::{catalog, Entity};

use fluessig::ir::{Catalog, Entity as IrEntity, Field, Scalar, TypeRef, Versions};

/// A type that describes a stored entity as pure `&'static` data. Implemented by
/// `#[derive(Entity)]`.
pub trait Entity {
    /// The descriptor the derive expands to.
    const DESCRIPTOR: &'static EntityDescriptor;
}

/// A whole entity, captured by the derive: its model name, optional physical
/// table override (`#[fluessig(name = "…")]`), doc comment, and columns.
#[derive(Debug, Clone, Copy)]
pub struct EntityDescriptor {
    /// The model (struct) name.
    pub name: &'static str,
    /// The physical table override, or `None` to let the loader snake_case `name`.
    pub table: Option<&'static str>,
    /// The struct's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The columns, in declaration order.
    pub fields: &'static [FieldDescriptor],
}

/// One scalar column.
#[derive(Debug, Clone, Copy)]
pub struct FieldDescriptor {
    /// The field (column) name.
    pub name: &'static str,
    /// The scalar carrier.
    pub scalar: ScalarKind,
    /// `Option<T>` in the source ⇒ `true`.
    pub nullable: bool,
    /// Marked `#[key]` ⇒ a primary-key member.
    pub key: bool,
    /// The field's `///` doc comment, if any.
    pub doc: Option<&'static str>,
}

/// The scalar carriers Slice 1 understands. The mapping to the catalog's scalar
/// vocabulary (`int64`, `string`, …) lives in [`ScalarKind::catalog`], kept in
/// lock-step with the primitive→variant mapping in the derive macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Bool,
    /// `String` — spelled `StringTy` to avoid colliding with the `String` type.
    StringTy,
}

impl ScalarKind {
    /// The catalog scalar name and its base carrier, matching what the TypeSpec
    /// emitter records for the equivalent built-in (integers/floats refine
    /// `numeric`; `string`/`boolean` are roots with no base).
    pub fn catalog(self) -> (&'static str, Option<&'static str>) {
        use ScalarKind::*;
        match self {
            I8 => ("int8", Some("numeric")),
            I16 => ("int16", Some("numeric")),
            I32 => ("int32", Some("numeric")),
            I64 => ("int64", Some("numeric")),
            U8 => ("uint8", Some("numeric")),
            U16 => ("uint16", Some("numeric")),
            U32 => ("uint32", Some("numeric")),
            U64 => ("uint64", Some("numeric")),
            F32 => ("float32", Some("numeric")),
            F64 => ("float64", Some("numeric")),
            Bool => ("boolean", None),
            StringTy => ("string", None),
        }
    }
}

/// Lower one descriptor to an [`fluessig::ir::Field`].
fn lower_field(f: &FieldDescriptor) -> Field {
    let (name, base) = f.scalar.catalog();
    Field {
        name: f.name.to_string(),
        ty: TypeRef::Scalar {
            name: name.to_string(),
            base: base.map(str::to_string),
        },
        nullable: f.nullable,
        doc: f.doc.map(str::to_string),
        key: f.key,
        column: None,
        default: None,
        derived: None,
        relation: None,
    }
}

/// Lower one descriptor to an [`fluessig::ir::Entity`].
fn lower_entity(d: &EntityDescriptor) -> IrEntity {
    let key = d
        .fields
        .iter()
        .filter(|f| f.key)
        .map(|f| f.name.to_string())
        .collect();
    IrEntity {
        name: d.name.to_string(),
        table: d.table.map(str::to_string),
        is_abstract: false,
        extends: None,
        key,
        doc: d.doc.map(str::to_string),
        fields: d.fields.iter().map(lower_field).collect(),
    }
}

/// Collect descriptors into the in-memory [`fluessig::ir::Catalog`] — the same IR
/// the loader validates. `name` becomes the catalog `source`; `version` stamps
/// the emitter field (format 1 has no dedicated version slot, so it rides the
/// stamp rather than inventing an unknown field the loader would reject).
pub fn build_catalog(name: &str, version: &str, entities: &[&'static EntityDescriptor]) -> Catalog {
    Catalog {
        fluessig: Versions {
            format: fluessig::FORMAT_VERSION,
            emitter: Some(format!("fluessig-derive/{version}")),
            compiler: None,
        },
        source: Some(name.to_string()),
        scalars: Vec::<Scalar>::new(),
        unions: Vec::new(),
        enums: Vec::new(),
        entities: entities.iter().map(|d| lower_entity(d)).collect(),
        relation_properties: Vec::new(),
        value_structs: Vec::new(),
    }
}

/// Render `catalog.json` — pretty-printed with a trailing newline, matching the
/// TypeSpec emitter's `JSON.stringify(…, null, 2) + "\n"`.
pub fn to_catalog_json(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
) -> String {
    let catalog = build_catalog(name, version, entities);
    let mut json = serde_json::to_string_pretty(&catalog).expect("catalog serializes");
    json.push('\n');
    json
}
