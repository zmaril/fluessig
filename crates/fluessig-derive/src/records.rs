//! Slice 8a Gap 2 — DTO / value-struct descriptors + the `api.json` `models`
//! layer.
//!
//! `#[derive(Record)]` declares a DTO (flat data the op surface passes across);
//! it lowers to a `fluessig::ir::Struct` in the catalog's `valueStructs`. When an
//! op references an entity or DTO — directly, or transitively through a referenced
//! DTO's fields — that shape is materialised into `api.json`'s `models`, FLATTENED
//! exactly as the TypeSpec op path does:
//!
//! * a to-one relation becomes its FK field(s) — the shape the ledger a consumer
//!   queries actually has — with the camelCased discriminator prepended when the
//!   relation is polymorphic;
//! * to-many relations are omitted (fetch children by their own op);
//! * the referenced set is closed transitively (a DTO holding another DTO, or a
//!   list of one, pulls that DTO in too).
//!
//! This is a direct Rust port of the TypeSpec emitter's model closure
//! (`emitter/emit.mjs`: `dtoFields` / `keyColumns` / the referenced-set fixpoint),
//! operating over the same lowered `Catalog` — so a derive-authored api surface
//! produces the SAME `models` the TypeSpec path produces for the equivalent
//! interface.
//!
//! Casing: the derive catalog spells field names snake_case (the physical-column
//! convention); the model layer camelCases them to the binding-surface convention,
//! exactly as op names/params are camelCased — so the materialised models match the
//! TypeSpec path (whose properties are authored camelCase) field-for-field.

use std::collections::{HashMap, HashSet};

use fluessig::api::{ApiField, ApiInterface, ApiModel, ApiType, ApiUnion, ApiUnionVariant};
use fluessig::ir::{
    camel, snake, Cardinality, Catalog, Entity as IrEntity, Field, Relation, Struct, TypeRef,
};

use crate::{RefResolver, ScalarKind, SourceSpan};

// ─────────────────────────── descriptors ───────────────────────────

/// A DTO / value struct captured by `#[derive(Record)]` (Slice 8a Gap 2): its
/// model name, doc, and flat data fields. Lowered to a `fluessig::ir::Struct` in
/// the catalog's `valueStructs`, and materialised into `api.json`'s `models` when
/// an op references it.
#[derive(Debug, Clone, Copy)]
pub struct RecordDescriptor {
    /// The record (struct) name — its `models` entry name in `api.json`.
    pub name: &'static str,
    /// The struct's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The record's fields, in declaration order.
    pub fields: &'static [RecordFieldDescriptor],
    /// The `.rs` source location of this record's declaration (Slice 6 spans).
    pub span: SourceSpan,
}

/// One field of a [`RecordDescriptor`]: a name, an op-surface data type, and
/// nullability. Records are flat data (no keys, no entity FK relations), so a
/// field is a scalar, a reference to another record/model, or a list thereof.
#[derive(Debug, Clone, Copy)]
pub struct RecordFieldDescriptor {
    /// The field name (camelCased at lowering, matching the op-surface
    /// convention op names/params already follow).
    pub name: &'static str,
    /// The field's data type.
    pub ty: RecordTypeDesc,
    /// `Option<T>` in the source ⇒ `true` (a nullable field).
    pub nullable: bool,
    /// The field's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The `.rs` source location of this field's declaration (Slice 6 spans).
    pub span: SourceSpan,
}

/// The data type of a [`RecordFieldDescriptor`] as pure `&'static` data —
/// recursive through `&'static` so it lives in a `const`. Lowered to a
/// `fluessig::ir::TypeRef` by [`lower_record_type`].
#[derive(Debug, Clone, Copy)]
pub enum RecordTypeDesc {
    /// A scalar column (`String`, `i64`, `bool`, …).
    Scalar(ScalarKind),
    /// A reference to another record / model, by name (`{ "model": name }` on the
    /// op surface). Records reference other records (`SinkOptions` → `TableRename`),
    /// so this lowers to a value-struct reference (`entity: false`).
    Model(&'static str),
    /// A bare **named** type the macro can't classify from the token alone (Slice
    /// 8b): a declared enum (`FileStatus`, `SinkTarget`), a declared or stock
    /// semantic scalar (`ArrowBatch`, `Json`), or a reference to another record.
    /// Resolved at lowering against the catalog's declared enums / scalars, exactly
    /// as an entity's `Named` field is.
    Named(&'static str),
    /// A list of the inner type (`Vec<T>`).
    List(&'static RecordTypeDesc),
}

/// Lower a [`RecordTypeDesc`] to the catalog's [`fluessig::ir::TypeRef`], resolving
/// a [`RecordTypeDesc::Named`] against the catalog's declared enums / scalars via
/// `resolver` (Slice 8b).
fn lower_record_type(t: &RecordTypeDesc, resolver: &RefResolver) -> TypeRef {
    match t {
        RecordTypeDesc::Scalar(kind) => {
            let (name, base) = kind.catalog();
            TypeRef::Scalar {
                name: name.to_string(),
                base: base.map(str::to_string),
            }
        }
        // a record referencing another record is a value-struct reference, not an
        // entity relation — flat data carrying no key of its own.
        RecordTypeDesc::Model(name) => TypeRef::Ref {
            name: name.to_string(),
            entity: false,
        },
        RecordTypeDesc::Named(name) => resolver.resolve_named(name),
        RecordTypeDesc::List(inner) => TypeRef::List {
            of: Box::new(lower_record_type(inner, resolver)),
        },
    }
}

/// Lower one [`RecordDescriptor`] to a `fluessig::ir::Struct` — a catalog value
/// struct. Records are flat, so no field carries a relation.
pub(crate) fn lower_record(r: &RecordDescriptor, resolver: &RefResolver) -> Struct {
    Struct {
        name: r.name.to_string(),
        doc: r.doc.map(str::to_string),
        fields: r
            .fields
            .iter()
            .map(|f| Field {
                name: f.name.to_string(),
                ty: lower_record_type(&f.ty, resolver),
                nullable: f.nullable,
                doc: f.doc.map(str::to_string),
                key: false,
                column: None,
                default: None,
                derived: None,
                relation: None,
            })
            .collect(),
    }
}

// ─────────────────────────── the `models` layer ───────────────────────────

/// Lower one catalog [`TypeRef`] to the op-surface [`ApiType`] — the port of the
/// emitter's `apiTypeOfRef` for a model field type (an entity/value-struct ref
/// crosses as `{ model }`, an enum as `{ enum }`, a union as `{ union }`).
fn api_type_of_ref(ty: &TypeRef) -> ApiType {
    match ty {
        TypeRef::Scalar { name, .. } => ApiType::Scalar(name.clone()),
        TypeRef::Ref { name, .. } => ApiType::Model {
            model: name.clone(),
        },
        TypeRef::Enum { name } => ApiType::Enum {
            r#enum: name.clone(),
        },
        TypeRef::List { of } => ApiType::List {
            list: Box::new(api_type_of_ref(of)),
        },
        TypeRef::Union { name } => ApiType::Union {
            union: name.clone(),
        },
    }
}

/// A field's physical column name: an explicit `@name`/`column` override, else the
/// snake_cased field name (the emitter's `f.column ?? snakeName(f.name)`).
fn field_column(f: &Field) -> String {
    f.column.clone().unwrap_or_else(|| snake(&f.name))
}

/// The FK `(column, type)` pairs of a to-one relation: the relation's own
/// `fkColumns` when pinned, else the target's key column names — each carrying the
/// target key member's type. Shared by [`model_key_columns`] and [`dto_fields`] so
/// the FK-expansion (target-key lookup + fk-name default) isn't spelled twice.
fn relation_fk(catalog: &Catalog, rel: &Relation) -> Vec<(String, TypeRef)> {
    let target = catalog.entity(&rel.to).expect("validated: target exists");
    let target_key = model_key_columns(catalog, target);
    let fk = rel
        .fk_columns
        .clone()
        .unwrap_or_else(|| target_key.iter().map(|(n, _)| n.clone()).collect());
    fk.into_iter()
        .zip(target_key.into_iter().map(|(_, ty)| ty))
        .collect()
}

/// An entity's key as `(physical column, scalar type)` pairs, relation key members
/// expanding through their target's key — the port of the emitter's `keyColumns`,
/// mirroring `sql.rs`'s own key expansion. Inherited (family) key members ride in
/// via `flattened_fields` / `flattened_key`.
fn model_key_columns(catalog: &Catalog, e: &IrEntity) -> Vec<(String, TypeRef)> {
    let fields = catalog.flattened_fields(e);
    let mut out = Vec::new();
    for k in catalog.flattened_key(e) {
        let f = fields
            .iter()
            .find(|f| f.name == k)
            .expect("validated: key field exists");
        match &f.relation {
            None => out.push((field_column(f), f.ty.clone())),
            Some(rel) => out.extend(relation_fk(catalog, rel)),
        }
    }
    out
}

/// One model's api-DTO fields (name camelCased, type, nullable) — the port of the
/// emitter's `dtoFields`: scalars as-is; a to-one relation → its FK field(s), with
/// the camelCased discriminator prepended when polymorphic; to-many relations
/// omitted. Operates on the model's OWN fields (inherited members reach a leaf via
/// its own key relations, exactly as the emitter does).
fn dto_fields(catalog: &Catalog, fields: &[Field]) -> Vec<(String, TypeRef, bool)> {
    let mut out = Vec::new();
    for f in fields {
        match &f.relation {
            None => out.push((camel(&f.name), f.ty.clone(), f.nullable)),
            Some(rel) if rel.cardinality == Cardinality::Many => {} // fetch by its own op
            Some(rel) => {
                // discriminator first when polymorphic (matches the emitter layout)
                if let Some(tc) = &rel.type_column {
                    out.push((
                        camel(tc),
                        TypeRef::Scalar {
                            name: "string".to_string(),
                            base: None,
                        },
                        f.nullable,
                    ));
                }
                for (col, ty) in relation_fk(catalog, rel) {
                    out.push((camel(&col), ty, f.nullable));
                }
            }
        }
    }
    out
}

/// Seed the referenced sets from one op param/return `ApiType` (unwrapping
/// list/nullable) — the emitter's `seedApiType`: a `{ model }` seeds a model, a
/// `{ union }` seeds a union.
fn seed_api_type(t: &ApiType, models: &mut HashSet<String>, unions: &mut HashSet<String>) {
    match t {
        ApiType::Model { model } => {
            models.insert(model.clone());
        }
        ApiType::Union { union } => {
            unions.insert(union.clone());
        }
        ApiType::List { list } => seed_api_type(list, models, unions),
        ApiType::Nullable { nullable } => seed_api_type(nullable, models, unions),
        _ => {}
    }
}

/// Close one catalog field/variant `TypeRef` into the referenced sets (the
/// emitter's `addTypeRef`): unwrap `list`, then a `Ref` grows the model set and a
/// named `Union` grows the union set. Returns whether either set grew.
fn add_type_ref(ty: &TypeRef, models: &mut HashSet<String>, unions: &mut HashSet<String>) -> bool {
    let inner = ty.innermost();
    match inner {
        TypeRef::Ref { name, .. } => models.insert(name.clone()),
        TypeRef::Union { name } => unions.insert(name.clone()),
        _ => false,
    }
}

/// Materialise `api.json`'s `models` + `unions` arrays (Slice 8a Gap 2 + union
/// authoring): the entities/DTOs the ops reference — seeded from op params/returns,
/// then closed transitively over referenced models' LOWERED fields AND referenced
/// unions' variants — flattened, in the emitter's candidate order (value structs
/// first, then entities). The `unions` array carries the referenced unions, sorted
/// by name, with each variant lowered to its op-surface type. A direct Rust port of
/// the emitter's model closure, over the same lowered catalog, so it equals the
/// TypeSpec path.
pub(crate) fn build_models(
    catalog: &Catalog,
    interfaces: &[ApiInterface],
) -> (Vec<ApiModel>, Vec<ApiUnion>) {
    // candidate models, emitter order: value structs first, then entities.
    let candidates: Vec<(&str, Option<&str>, &[Field])> = catalog
        .value_structs
        .iter()
        .map(|s| (s.name.as_str(), s.doc.as_deref(), s.fields.as_slice()))
        .chain(
            catalog
                .entities
                .iter()
                .map(|e| (e.name.as_str(), e.doc.as_deref(), e.fields.as_slice())),
        )
        .collect();

    // every candidate's lowered DTO fields, by name.
    let lowered: HashMap<&str, Vec<(String, TypeRef, bool)>> = candidates
        .iter()
        .map(|(name, _, fields)| (*name, dto_fields(catalog, fields)))
        .collect();

    // seed the referenced sets from the ops' param/return model/union references.
    let mut referenced: HashSet<String> = HashSet::new();
    let mut referenced_unions: HashSet<String> = HashSet::new();
    for i in interfaces {
        for op in &i.ops {
            for p in &op.params {
                seed_api_type(&p.ty, &mut referenced, &mut referenced_unions);
            }
            seed_api_type(&op.returns, &mut referenced, &mut referenced_unions);
            // A `#[fluessig(result)]` op's error RECORD is referenced by the op
            // too (node materialises it as the envelope's `error` arm), so it must
            // join `models` even though it appears in no param/return position.
            if let Some(e) = &op.result_error {
                referenced.insert(e.clone());
            }
        }
    }

    // close transitively: models referenced by referenced models' LOWERED fields,
    // and unions reached through those fields; then the referenced unions' variant
    // bodies pull in their own models/unions (the emitter's twin-set fixpoint).
    let mut grew = true;
    while grew {
        grew = false;
        for name in referenced.clone() {
            for (_, ty, _) in lowered.get(name.as_str()).into_iter().flatten() {
                grew |= add_type_ref(ty, &mut referenced, &mut referenced_unions);
            }
        }
        for uname in referenced_unions.clone() {
            if let Some(u) = catalog.union_def(&uname) {
                for v in &u.variants {
                    grew |= add_type_ref(&v.ty, &mut referenced, &mut referenced_unions);
                }
            }
        }
    }

    let models = candidates
        .iter()
        .filter(|(name, _, _)| referenced.contains(*name))
        .map(|(name, doc, _)| ApiModel {
            name: name.to_string(),
            doc: doc.map(str::to_string),
            fields: lowered[name]
                .iter()
                .map(|(fname, ty, nullable)| ApiField {
                    name: fname.clone(),
                    ty: api_type_of_ref(ty),
                    nullable: *nullable,
                    bindings: Default::default(),
                })
                .collect(),
            bindings: Default::default(),
        })
        .collect();

    // referenced unions, sorted by name (the emitter's `[...referencedUnions].sort()`).
    let mut union_names: Vec<&String> = referenced_unions.iter().collect();
    union_names.sort();
    let unions = union_names
        .into_iter()
        .filter_map(|name| catalog.union_def(name))
        .map(|u| ApiUnion {
            name: u.name.clone(),
            doc: u.doc.clone(),
            tag_field: None,
            variants: u
                .variants
                .iter()
                .map(|v| ApiUnionVariant {
                    tag: v.tag.clone(),
                    ty: api_type_of_ref(&v.ty),
                })
                .collect(),
            bindings: Default::default(),
        })
        .collect();

    (models, unions)
}
