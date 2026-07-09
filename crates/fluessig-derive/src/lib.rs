//! The fluessig Rust derive front end — support layer (Slices 1–2).
//!
//! `derive-front-end.md` §1 splits the front end in two: a **derive → descriptor**
//! step (pure `&'static` data, no behaviour) and a **descriptor → catalog** step
//! (a plain function that prints the same `catalog.json` the Rust loader already
//! consumes). This crate owns the descriptor vocabulary and the printer; the
//! `#[derive(Entity)]` / `catalog!` macros live in `fluessig-derive-macros` and
//! are re-exported here so a downstream crate depends on one name.
//!
//! Slice 1 scope: one scalar-only entity end-to-end. Slice 2 adds **references**:
//! a field typed [`Id<T>`] (or `Option<Id<T>>`) declares a foreign key resolved
//! from the Rust type — `@fk` disappears from the authoring surface — and a
//! composite-key target spells its reference columns once via
//! `#[fluessig(ref_cols(field = "col", …))]`. Edges, inheritance/`flatten`,
//! polymorphism (`abstract_root`), the op surface, and spans are Slices 3–6
//! (`notes/derive-front-end-decisions.md`).

use std::collections::HashMap;
use std::marker::PhantomData;

/// The engine crate, re-exported so macro-generated code can name the IR
/// (`::fluessig_derive::fluessig::Catalog`) without the downstream crate having
/// to depend on `fluessig` directly.
pub use fluessig;

// The derive macro and the trait share the name `Entity` (they live in different
// namespaces — macro vs type — exactly like `serde::Serialize`), so a single
// `use fluessig_derive::Entity;` brings in both.
pub use fluessig_derive_macros::{catalog, Entity};

use fluessig::ir::{
    Cardinality, Catalog, Entity as IrEntity, Field, RelKind, Relation, Scalar, TypeRef, Versions,
};

/// A type that describes a stored entity as pure `&'static` data. Implemented by
/// `#[derive(Entity)]`.
pub trait Entity {
    /// The descriptor the derive expands to.
    const DESCRIPTOR: &'static EntityDescriptor;
}

/// A typed foreign-key value: `Id<T>` stands in for `T`'s primary key at a
/// referencing site (Slice 2). `#[derive(Entity)]` reads the `T` in a field
/// typed `Id<T>` / `Option<Id<T>>` and lowers the field to a single foreign-key
/// relation targeting `T` — so `@fk` never appears in the authoring surface, and
/// a typo'd target is a plain rustc "cannot find type" error.
///
/// It carries no runtime key payload yet: the derive front end reads *types*, it
/// never constructs an entity, so a `PhantomData<T>` marker is all Slice 2 needs.
/// (Row-value storage — `Id<T>` actually holding `T`'s key — is a later concern,
/// out of scope here.)
pub struct Id<T: ?Sized>(PhantomData<T>);

/// A whole entity, captured by the derive: its model name, optional physical
/// table override (`#[fluessig(name = "…")]`), doc comment, columns, and the
/// reference-column spelling other entities use when they point at it by
/// `Id<Self>` (Slice 2, [`RefColDescriptor`]).
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
    /// How a referencing site spells this entity's key columns (`ref_cols(…)`).
    /// Declared once on the *target* — the reference-column spelling lives on the
    /// referenced entity, not each site (`notes/derive-front-end-decisions.md`,
    /// Slice 2). Empty ⇒ a single-column target takes the referencing field name;
    /// a composite target lists a spelling per key member here.
    pub ref_cols: &'static [RefColDescriptor],
}

/// One `ref_cols(field = "column")` entry: a key field of the declaring entity
/// and the column name a referencing `Id<T>` site materialises for that key part.
#[derive(Debug, Clone, Copy)]
pub struct RefColDescriptor {
    /// A `#[key]` field of the declaring entity.
    pub field: &'static str,
    /// The column name referencing sites use for that key part.
    pub column: &'static str,
}

/// One column. Slice 1 fields are scalars; Slice 2 adds [`FieldKind::Reference`],
/// a foreign key discovered from an `Id<T>` field type.
#[derive(Debug, Clone, Copy)]
pub struct FieldDescriptor {
    /// The field (column) name.
    pub name: &'static str,
    /// Whether the field is a scalar column or a foreign-key reference.
    pub kind: FieldKind,
    /// `Option<T>` in the source ⇒ `true` (a nullable column / nullable FK).
    pub nullable: bool,
    /// Marked `#[key]` ⇒ a primary-key member.
    pub key: bool,
    /// The field's `///` doc comment, if any.
    pub doc: Option<&'static str>,
}

/// What a field carries: a scalar value, or a reference to another entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// A scalar column (Slice 1).
    Scalar(ScalarKind),
    /// A foreign key `Id<T>`: the referenced entity's model name (Slice 2). The
    /// FK columns are resolved at lowering time from the *target's* key +
    /// `ref_cols`, so the site only needs to carry the target name.
    Reference(&'static str),
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

/// An index over the descriptors being lowered together, so a reference field can
/// resolve its target's key spelling. Built once per [`build_catalog`] call.
struct RefResolver<'a> {
    by_name: HashMap<&'static str, &'a EntityDescriptor>,
}

impl<'a> RefResolver<'a> {
    fn new(entities: &[&'a EntityDescriptor]) -> Self {
        RefResolver {
            by_name: entities.iter().map(|e| (e.name, *e)).collect(),
        }
    }

    /// The foreign-key columns a field materialises to reference `target`.
    ///
    /// A single-column target takes the referencing **field name** (`repo_id:
    /// Id<Repo>` ⇒ `["repo_id"]`) — the site names it, matching how Slice-1
    /// scalar fields map name→column. A composite (multi-key) target can't be
    /// named from one field, so its columns come from the target's key order,
    /// each spelled by the target's `ref_cols` override (else the key field's own
    /// name) — the reference spelling declared once on the target.
    ///
    /// A dangling target (typo'd `Id<T>`) resolves to `[field_name]`; the emitted
    /// `relation.to` still points at the missing entity, so the loader catches it.
    fn fk_columns(&self, field_name: &str, target: &str) -> Vec<String> {
        let Some(desc) = self.by_name.get(target) else {
            return vec![field_name.to_string()];
        };
        let key_fields: Vec<&FieldDescriptor> = desc.fields.iter().filter(|f| f.key).collect();
        if key_fields.len() <= 1 {
            return vec![field_name.to_string()];
        }
        key_fields
            .iter()
            .map(|kf| {
                desc.ref_cols
                    .iter()
                    .find(|rc| rc.field == kf.name)
                    .map(|rc| rc.column.to_string())
                    .unwrap_or_else(|| kf.name.to_string())
            })
            .collect()
    }
}

/// Lower one descriptor to an [`fluessig::ir::Field`], resolving references
/// against the sibling descriptors via `resolver`.
fn lower_field(f: &FieldDescriptor, resolver: &RefResolver) -> Field {
    let (ty, relation) = match f.kind {
        FieldKind::Scalar(kind) => {
            let (name, base) = kind.catalog();
            (
                TypeRef::Scalar {
                    name: name.to_string(),
                    base: base.map(str::to_string),
                },
                None,
            )
        }
        FieldKind::Reference(target) => {
            let ty = TypeRef::Ref {
                name: target.to_string(),
                entity: true,
            };
            let rel = Relation {
                to: target.to_string(),
                cardinality: Cardinality::One,
                kind: RelKind::Association,
                properties: None,
                table: None,
                fk_columns: Some(resolver.fk_columns(f.name, target)),
                type_column: None,
                source_columns: None,
                source_type_column: None,
            };
            (ty, Some(rel))
        }
    };
    Field {
        name: f.name.to_string(),
        ty,
        nullable: f.nullable,
        doc: f.doc.map(str::to_string),
        key: f.key,
        column: None,
        default: None,
        derived: None,
        relation,
    }
}

/// Lower one descriptor to an [`fluessig::ir::Entity`].
fn lower_entity(d: &EntityDescriptor, resolver: &RefResolver) -> IrEntity {
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
        fields: d.fields.iter().map(|f| lower_field(f, resolver)).collect(),
    }
}

/// Collect descriptors into the in-memory [`fluessig::ir::Catalog`] — the same IR
/// the loader validates. `name` becomes the catalog `source`; `version` stamps
/// the emitter field (format 1 has no dedicated version slot, so it rides the
/// stamp rather than inventing an unknown field the loader would reject).
pub fn build_catalog(name: &str, version: &str, entities: &[&'static EntityDescriptor]) -> Catalog {
    let resolver = RefResolver::new(entities);
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
        entities: entities
            .iter()
            .map(|d| lower_entity(d, &resolver))
            .collect(),
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
