//! The fluessig Rust derive front end — support layer (Slices 1–3).
//!
//! `derive-front-end.md` §1 splits the front end in two: a **derive → descriptor**
//! step (pure `&'static` data, no behaviour) and a **descriptor → catalog** step
//! (a plain function that prints the same `catalog.json` the Rust loader already
//! consumes). This crate owns the descriptor vocabulary and the printer; the
//! `#[derive(Entity)]` / `#[derive(Edge)]` / `catalog!` macros live in
//! `fluessig-derive-macros` and are re-exported here so a downstream crate
//! depends on one name.
//!
//! Slice 1 was one scalar entity end-to-end. Slice 2 added **references**: a
//! field typed [`Id<T>`] (or `Option<Id<T>>`) declares a foreign key resolved
//! from the Rust type — `@fk` disappears from the authoring surface — and a
//! composite-key target spells its reference columns once via
//! `#[fluessig(ref_cols(field = "col", …))]`. Slice 3 lands the **attribute
//! grammar** (parsed with `darling` in the macro crate) and the shapes it
//! unlocks:
//!
//! * **`#[fluessig(flatten)]`** — a field embeds another struct's fields inline
//!   ([`FieldKind::Flatten`]). This is the inheritance / abstract-root-carries-
//!   only-its-key pattern from §2.3 (entl FINDINGS #6): the embedded root's
//!   columns land in the leaf's own column set. (The polymorphic
//!   `abstract_root`-generates-enums machinery is Slice 4 — flatten here is the
//!   embedding mechanism only, no generated key enums.)
//! * **Edge structs** — `#[derive(Edge)] #[fluessig(edge(from = A, to = B))]` on
//!   a struct whose fields are the edge's own columns ([`EdgeDescriptor`]). Its
//!   source/target `Id<T>` fields become the edge table's source/target FK
//!   columns; any remaining fields are edge properties (its local key, entl
//!   FINDINGS #3). Lowered exactly as the TypeSpec `@edge` path lands in
//!   `catalog.json`: a to-many relation field on the `from` entity plus (when it
//!   has local properties) an edge struct in `relationProperties`.
//! * **`shares(col)`** — states that a reference's leading FK column *is* an
//!   existing physical column of that name in the same table (a declared fact
//!   and a real same-table constraint), rather than a spelling coincidence the
//!   physical projection silently dedups.
//!
//! Still out of scope: polymorphism (`abstract_root`), the op surface, and spans
//! — Slices 4–6 (`notes/derive-front-end-decisions.md`).

use std::collections::HashMap;
use std::marker::PhantomData;

/// The engine crate, re-exported so macro-generated code can name the IR
/// (`::fluessig_derive::fluessig::Catalog`) without the downstream crate having
/// to depend on `fluessig` directly.
pub use fluessig;

// The derive macros and their traits share names (`Entity`, `Edge`) — they live
// in different namespaces (macro vs type), exactly like `serde::Serialize` — so
// a single `use fluessig_derive::{Entity, Edge};` brings in both halves.
pub use fluessig_derive_macros::{catalog, Edge, Entity};

use fluessig::ir::{
    Cardinality, Catalog, Entity as IrEntity, Field, RelKind, Relation, Scalar, Struct, TypeRef,
    Versions,
};

/// A type that describes a stored entity as pure `&'static` data. Implemented by
/// `#[derive(Entity)]`.
pub trait Entity {
    /// The descriptor the derive expands to.
    const DESCRIPTOR: &'static EntityDescriptor;
}

/// A type that describes an edge (its own row struct — `derive-front-end.md`
/// §2.4) as pure `&'static` data. Implemented by `#[derive(Edge)]`.
pub trait Edge {
    /// The descriptor the derive expands to.
    const DESCRIPTOR: &'static EdgeDescriptor;
}

/// A typed foreign-key value: `Id<T>` stands in for `T`'s primary key at a
/// referencing site (Slice 2). `#[derive(Entity)]` reads the `T` in a field
/// typed `Id<T>` / `Option<Id<T>>` and lowers the field to a single foreign-key
/// relation targeting `T` — so `@fk` never appears in the authoring surface, and
/// a typo'd target is a plain rustc "cannot find type" error.
///
/// It carries no runtime key payload yet: the derive front end reads *types*, it
/// never constructs an entity, so a `PhantomData<T>` marker is all it needs.
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
    /// The columns, in declaration order. A [`FieldKind::Flatten`] field embeds
    /// another descriptor's columns at its position (Slice 3).
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
    /// A key field of the declaring entity.
    pub field: &'static str,
    /// The column name referencing sites use for that key part.
    pub column: &'static str,
}

/// One column. Slice 1 fields are scalars; Slice 2 adds [`FieldKind::Reference`],
/// a foreign key discovered from an `Id<T>` field type; Slice 3 adds
/// [`FieldKind::Flatten`], an embedded struct.
#[derive(Debug, Clone, Copy)]
pub struct FieldDescriptor {
    /// The field (column) name. For a [`FieldKind::Flatten`] field this is the
    /// embedding field's own name and is never itself a column.
    pub name: &'static str,
    /// Whether the field is a scalar column, a foreign-key reference, or an
    /// inline-embedded struct.
    pub kind: FieldKind,
    /// `Option<T>` in the source ⇒ `true` (a nullable column / nullable FK).
    pub nullable: bool,
    /// Marked `#[key]` (or `#[fluessig(key)]`) ⇒ a primary-key member.
    pub key: bool,
    /// The field's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// `shares(col, …)`: for a reference field, the physical column name(s) its
    /// leading FK column(s) share with an existing same-table column — a stated
    /// fact, not a silent dedup (Slice 3, `derive-front-end.md` §2.5). Empty for
    /// the common case where the reference's own spelling is authoritative.
    pub shares: &'static [&'static str],
}

/// What a field carries: a scalar value, a reference to another entity, or an
/// inline-embedded struct. (No `PartialEq`/`Eq`: [`FieldKind::Flatten`] holds a
/// `&'static EntityDescriptor`, which has no meaningful value equality.)
#[derive(Debug, Clone, Copy)]
pub enum FieldKind {
    /// A scalar column (Slice 1).
    Scalar(ScalarKind),
    /// A foreign key `Id<T>`: the referenced entity's model name (Slice 2). The
    /// FK columns are resolved at lowering time from the *target's* key +
    /// `ref_cols`, so the site only needs to carry the target name.
    Reference(&'static str),
    /// A `#[fluessig(flatten)]` field: the embedded entity's descriptor, whose
    /// columns are spliced in at this field's position (Slice 3). The embedding
    /// field name itself is not a column.
    Flatten(&'static EntityDescriptor),
}

/// The scalar carriers the derive understands. The mapping to the catalog's
/// scalar vocabulary (`int64`, `string`, …) lives in [`ScalarKind::catalog`],
/// kept in lock-step with the primitive→variant mapping in the derive macro.
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

/// An edge — its own row struct (`derive-front-end.md` §2.4). `#[derive(Edge)]`
/// with `#[fluessig(edge(from = A, to = B))]` captures the source/target
/// entities, an optional physical table + exposed relation-field name, and the
/// edge's own fields.
#[derive(Debug, Clone, Copy)]
pub struct EdgeDescriptor {
    /// The edge struct's name (its `relationProperties` name when it carries
    /// local fields, e.g. `CommitParent`).
    pub name: &'static str,
    /// The physical edge-table override; `None` ⇒ snake_case(`name`).
    pub table: Option<&'static str>,
    /// The struct's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The source entity (`edge(from = …)`).
    pub from: &'static str,
    /// The target entity (`edge(to = …)`).
    pub to: &'static str,
    /// The relation-field name exposed on the `from` entity (`edge(expose = …)`);
    /// `None` ⇒ snake_case(`name`).
    pub expose: Option<&'static str>,
    /// The edge's own columns, in declaration order.
    pub fields: &'static [EdgeFieldDescriptor],
}

/// One edge field, tagged with the role the lowering gives it: a source-side FK,
/// a target-side FK, or an edge property (a local-key or payload column).
#[derive(Debug, Clone, Copy)]
pub struct EdgeFieldDescriptor {
    /// The underlying column descriptor (name, kind, nullability, key, doc,
    /// shares).
    pub field: FieldDescriptor,
    /// Whether this field is the source FK, the target FK, or an edge property.
    pub role: EdgeRole,
}

/// The role an edge field plays in the lowered edge table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeRole {
    /// An `Id<from>` field — a source-side FK column.
    Source,
    /// An `Id<to>` field — a target-side FK column.
    Target,
    /// Any other field — an edge property (its local key / payload).
    Property,
}

/// The entity fields with every [`FieldKind::Flatten`] expanded to the embedded
/// descriptor's columns, in place (Slice 3). Recursive: an embedded struct may
/// itself flatten another.
fn expanded_fields(d: &'static EntityDescriptor) -> Vec<&'static FieldDescriptor> {
    let mut out = Vec::new();
    for f in d.fields {
        match f.kind {
            FieldKind::Flatten(inner) => out.extend(expanded_fields(inner)),
            _ => out.push(f),
        }
    }
    out
}

/// An index over the descriptors being lowered together, so a reference field can
/// resolve its target's key spelling. Built once per [`build_catalog`] call.
struct RefResolver {
    by_name: HashMap<&'static str, &'static EntityDescriptor>,
}

impl RefResolver {
    fn new(entities: &[&'static EntityDescriptor]) -> Self {
        RefResolver {
            by_name: entities.iter().map(|e| (e.name, *e)).collect(),
        }
    }

    /// The foreign-key columns a field materialises to reference `target`.
    ///
    /// A single-column target takes the referencing **field name** (`repo_id:
    /// Id<Repo>` ⇒ `["repo_id"]`) — the site names it, matching how scalar fields
    /// map name→column. A composite (multi-key) target can't be named from one
    /// field, so its columns come from the target's key order, each spelled by
    /// the target's `ref_cols` override (else the key field's own name) — the
    /// reference spelling declared once on the target. Flatten-embedded keys
    /// participate: the target's key is read from its *expanded* fields.
    ///
    /// A dangling target (typo'd `Id<T>`) resolves to `[field_name]`; the emitted
    /// `relation.to` still points at the missing entity, so the loader catches it.
    fn fk_columns(&self, field_name: &str, target: &str) -> Vec<String> {
        let Some(desc) = self.by_name.get(target) else {
            return vec![field_name.to_string()];
        };
        let key_fields: Vec<&FieldDescriptor> = expanded_fields(desc)
            .into_iter()
            .filter(|f| f.key)
            .collect();
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

    /// The FK columns of a reference, with any `shares(col, …)` override applied
    /// to the leading columns (Slice 3): `shares(repo_id)` states the first FK
    /// column *is* the shared physical column `repo_id`, replacing the target's
    /// own spelling for that key part so the physical projection folds the two
    /// into one column instead of dedup-by-coincidence.
    fn fk_columns_shared(&self, field_name: &str, target: &str, shares: &[&str]) -> Vec<String> {
        let mut cols = self.fk_columns(field_name, target);
        for (slot, shared) in cols.iter_mut().zip(shares.iter()) {
            *slot = shared.to_string();
        }
        cols
    }
}

/// Lower one scalar/reference descriptor to an [`fluessig::ir::Field`], resolving
/// references against the sibling descriptors via `resolver`. Flatten fields are
/// expanded by the caller and never reach here.
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
                fk_columns: Some(resolver.fk_columns_shared(f.name, target, f.shares)),
                type_column: None,
                source_columns: None,
                source_type_column: None,
            };
            (ty, Some(rel))
        }
        FieldKind::Flatten(_) => unreachable!("flatten fields are expanded before lowering"),
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

/// Lower one descriptor to an [`fluessig::ir::Entity`], expanding flatten fields.
fn lower_entity(d: &'static EntityDescriptor, resolver: &RefResolver) -> IrEntity {
    let fields = expanded_fields(d);
    let key = fields
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
        fields: fields.iter().map(|f| lower_field(f, resolver)).collect(),
    }
}

/// Lower an edge descriptor to (the to-many relation field to attach to its
/// `from` entity, an optional `relationProperties` edge struct). Matches how the
/// TypeSpec `@edge` path lands in `catalog.json`: source/target FK columns live
/// on the relation field (`fkColumns` / `sourceColumns`), and only the edge's own
/// local fields become the `relationProperties` struct.
fn lower_edge(e: &'static EdgeDescriptor, resolver: &RefResolver) -> (Field, Option<Struct>) {
    let mut source_columns = Vec::new();
    let mut target_columns = Vec::new();
    let mut prop_fields = Vec::new();
    for ef in e.fields {
        let f = &ef.field;
        match (ef.role, f.kind) {
            (EdgeRole::Source, FieldKind::Reference(t)) => {
                source_columns.extend(resolver.fk_columns_shared(f.name, t, f.shares));
            }
            (EdgeRole::Target, FieldKind::Reference(t)) => {
                target_columns.extend(resolver.fk_columns_shared(f.name, t, f.shares));
            }
            _ => prop_fields.push(lower_field(f, resolver)),
        }
    }

    let table = e
        .table
        .map(str::to_string)
        .unwrap_or_else(|| fluessig::ir::snake(e.name));
    let expose = e
        .expose
        .map(str::to_string)
        .unwrap_or_else(|| fluessig::ir::snake(e.name));

    let properties = if prop_fields.is_empty() {
        None
    } else {
        Some(e.name.to_string())
    };

    let rel = Relation {
        to: e.to.to_string(),
        cardinality: Cardinality::Many,
        kind: RelKind::Association,
        properties: properties.clone(),
        table: Some(table),
        fk_columns: Some(target_columns),
        type_column: None,
        source_columns: Some(source_columns),
        source_type_column: None,
    };
    let field = Field {
        name: expose,
        ty: TypeRef::List {
            of: Box::new(TypeRef::Ref {
                name: e.to.to_string(),
                entity: true,
            }),
        },
        nullable: false,
        doc: e.doc.map(str::to_string),
        key: false,
        column: None,
        default: None,
        derived: None,
        relation: Some(rel),
    };

    let edge_struct = properties.map(|name| Struct {
        name,
        doc: e.doc.map(str::to_string),
        fields: prop_fields,
    });
    (field, edge_struct)
}

/// Collect entity descriptors into the in-memory [`fluessig::ir::Catalog`] — the
/// same IR the loader validates. See [`build_catalog_with_edges`] for the
/// full form; this is the no-edges convenience kept for Slice 1/2 callers.
pub fn build_catalog(name: &str, version: &str, entities: &[&'static EntityDescriptor]) -> Catalog {
    build_catalog_with_edges(name, version, entities, &[])
}

/// Collect entity + edge descriptors into the in-memory [`fluessig::ir::Catalog`]
/// (Slice 3). `name` becomes the catalog `source`; `version` stamps the emitter
/// field (format 1 has no dedicated version slot, so it rides the stamp rather
/// than inventing an unknown field the loader would reject).
///
/// Each edge contributes a to-many relation field to its `from` entity plus, when
/// it carries local fields, one `relationProperties` edge struct — the same shape
/// the TypeSpec `@edge` path produces.
pub fn build_catalog_with_edges(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
) -> Catalog {
    let resolver = RefResolver::new(entities);
    let mut ir_entities: Vec<IrEntity> = entities
        .iter()
        .map(|d| lower_entity(d, &resolver))
        .collect();
    let mut relation_properties = Vec::new();
    for edge in edges {
        let (rel_field, edge_struct) = lower_edge(edge, &resolver);
        if let Some(from) = ir_entities.iter_mut().find(|e| e.name == edge.from) {
            from.fields.push(rel_field);
        }
        if let Some(s) = edge_struct {
            relation_properties.push(s);
        }
    }
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
        entities: ir_entities,
        relation_properties,
        value_structs: Vec::new(),
    }
}

/// Render `catalog.json` — pretty-printed with a trailing newline, matching the
/// TypeSpec emitter's `JSON.stringify(…, null, 2) + "\n"`. No-edges convenience
/// kept for Slice 1/2 callers; see [`to_catalog_json_with_edges`].
pub fn to_catalog_json(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
) -> String {
    to_catalog_json_with_edges(name, version, entities, &[])
}

/// Render `catalog.json` for a catalog with edges (Slice 3).
pub fn to_catalog_json_with_edges(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
) -> String {
    let catalog = build_catalog_with_edges(name, version, entities, edges);
    let mut json = serde_json::to_string_pretty(&catalog).expect("catalog serializes");
    json.push('\n');
    json
}
