//! The fluessig Rust derive front end — support layer (Slices 1–5).
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
//! Slice 4 lands **polymorphism** (`notes/derive-front-end-decisions.md`,
//! Decision #3): a **family** is an abstract root + a closed set of concrete
//! leaves that share one key, and a polymorphic reference is a (type-tag, key)
//! column pair.
//!
//! * **`#[derive(AbstractRoot)]`** on a family root (with
//!   `#[fluessig(abstract_root(Commit, Tree, Blob), tag_col = …, ref_col = …)]`)
//!   generates a real native sum type `<Root>Id` — one variant per leaf, each
//!   carrying the family key (heterogeneous across families: `GitObjectId`
//!   carries a scalar `Oid`, `GhSubjectId` a composite `(Id<Repo>, i32)`) — plus
//!   `impl AbstractRoot for <Root> { type Id = <Root>Id; }` so the conjured name
//!   is discoverable via `<Root as AbstractRoot>::Id`. The root is also an
//!   `Entity` (marked abstract, no table, carrying the family key).
//! * **Leaves** declare `#[fluessig(extends = Root)]`; they lower to the catalog
//!   `extends`, inheriting the family key + columns through the loader's
//!   `flattened_*` (the same shape the TypeSpec `extends` path produces).
//! * **Polymorphic references** — a field typed `<Root>Id`
//!   ([`FieldKind::PolyReference`]) lowers to the (tag, key) column pair, spelled
//!   from the family (`tag_col` / `ref_col` / `ref_cols`) unless the site pins
//!   its own with `#[fluessig(cols(tag = …, key = …))]` (legacy per-site
//!   variance, entl FINDINGS #7).
//!
//! Slice 5 lands the **op surface** (`derive-front-end.md` §2.7): the
//! `#[fluessig::export]` attribute macro captures a `impl` block's methods into
//! an [`InterfaceDescriptor`] — op name, params, return, and op kind
//! ([`OpKind`]: `ctor` / plain unary / `stream` / `manual`) — and `catalog!`'s
//! `api:` root list lowers those into the `api.json` op layer via [`build_api`],
//! the same file the loader validates and bindgen projects. Still out of scope:
//! spans — Slice 6.

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// The engine crate, re-exported so macro-generated code can name the IR
/// (`::fluessig_derive::fluessig::Catalog`) without the downstream crate having
/// to depend on `fluessig` directly.
pub use fluessig;

// The derive macros and their traits share names (`Entity`, `Edge`) — they live
// in different namespaces (macro vs type), exactly like `serde::Serialize` — so
// a single `use fluessig_derive::{Entity, Edge};` brings in both halves.
pub use fluessig_derive_macros::{catalog, export, AbstractRoot, Edge, Entity};

use fluessig::api::{ApiDoc, ApiInterface, ApiOp, ApiParam, ApiType, Shape};
use fluessig::ir::{
    camel, Cardinality, Catalog, Entity as IrEntity, Field, RelKind, Relation, Scalar, Struct,
    TypeRef, Versions,
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

/// A polymorphic family root (Slice 4, Decision #3). `#[derive(AbstractRoot)]`
/// generates the named key enum `<Root>Id` — one variant per leaf carrying the
/// family key — and implements this trait so the generated name has a
/// go-to-definition answer: `<Root as AbstractRoot>::Id`. The one convention to
/// document is `abstract_root(A, B, C)` generates `<Root>Id`.
pub trait AbstractRoot {
    /// The generated key enum (`<Root>Id`).
    type Id;
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

// `Id<T>` is a zero-size typed marker, so it carries the standard value traits
// *unconditionally* — a `#[derive]` would wrongly bound them on `T`, which would
// then propagate onto every generated `<Root>Id` enum that carries an `Id<T>`
// key part (Slice 4). Hand-written impls keep the enums `Debug`/`Clone`/`Eq`/
// `Hash` regardless of the referenced entity's own traits.
impl<T: ?Sized> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> Copy for Id<T> {}
impl<T: ?Sized> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Id")
    }
}
impl<T: ?Sized> PartialEq for Id<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl<T: ?Sized> Eq for Id<T> {}
impl<T: ?Sized> Hash for Id<T> {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}
impl<T: ?Sized> Default for Id<T> {
    fn default() -> Self {
        Id(PhantomData)
    }
}

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
    /// The abstract family root this entity extends (a concrete leaf), if any
    /// (Slice 4). Lowered to the catalog `extends` — the leaf inherits the family
    /// key + columns through the loader's `flattened_*`, so its own `key` is empty
    /// and it never re-lists the root's columns.
    pub extends: Option<&'static str>,
    /// For a family root (`#[derive(AbstractRoot)]`): the closed leaf set.
    /// Non-empty ⇒ this entity is an abstract root (`is_abstract`), has no table
    /// of its own, and carries the family key the leaves share (Slice 4).
    pub abstract_leaves: &'static [&'static str],
    /// For a family root: the generated key-enum type name (`<Name>Id`), so a
    /// polymorphic reference site typed `<Name>Id` resolves to this family.
    pub id_enum: Option<&'static str>,
    /// For a family root: the discriminator (type-tag) column a polymorphic
    /// reference to the family materialises (`tag_col`). A per-site
    /// `cols(tag = …)` overrides it.
    pub tag_col: Option<&'static str>,
    /// For a single-column-keyed family root: the key column a polymorphic
    /// reference spells by default (`ref_col`). Composite families spell each key
    /// part via [`EntityDescriptor::ref_cols`] instead; a per-site `cols(key = …)`
    /// overrides a single-key family's spelling.
    pub ref_col: Option<&'static str>,
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
    /// A polymorphic family reference (Slice 4): a field typed `<Root>Id` (the
    /// generated key enum) lowers to the (type-tag, key) column pair into the
    /// abstract root. The enum type name resolves to the family at lowering; any
    /// per-site `cols(tag = …, key = …)` override rides in the [`PolyRef`].
    PolyReference(PolyRef),
}

/// A polymorphic reference site (Slice 4): the generated family key-enum named at
/// the field, plus optional per-site column-name overrides. With no override the
/// spelling comes from the family (`tag_col` / `ref_col` / `ref_cols`); the
/// override exists only for legacy per-site variance (entl FINDINGS #7).
#[derive(Debug, Clone, Copy)]
pub struct PolyRef {
    /// The generated key-enum type named at the site (`"GhSubjectId"`), resolved
    /// to its family via [`EntityDescriptor::id_enum`] at lowering.
    pub id_enum: &'static str,
    /// `cols(tag = "…")`: a per-site discriminator column, overriding the
    /// family's `tag_col`.
    pub tag_col: Option<&'static str>,
    /// `cols(key = "…")`: a per-site key column (single-key families only),
    /// overriding the family's `ref_col`.
    pub ref_col: Option<&'static str>,
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

/// `"GhSubjectId"` → `"GhSubject"` — the family name a generated key enum names by
/// the `<Root>Id` convention (Decision #3). Used only as the fallback when a
/// polymorphic reference's enum doesn't resolve to a known family, so
/// `relation.to` still names the intended (missing) entity for the loader to flag.
fn strip_id(id_enum: &str) -> String {
    id_enum.strip_suffix("Id").unwrap_or(id_enum).to_string()
}

/// A descriptor's key fields (its `#[key]` members, flatten-expanded).
fn key_fields(desc: &'static EntityDescriptor) -> Vec<&'static FieldDescriptor> {
    expanded_fields(desc)
        .into_iter()
        .filter(|f| f.key)
        .collect()
}

/// Spell a composite target's key columns as referencing sites see them: each key
/// part via the target's `ref_cols` override, else the key field's own name.
/// Shared by the `Id<T>` FK resolver ([`RefResolver::fk_columns`]) and the
/// polymorphic-reference resolver ([`RefResolver::poly_reference`]) — both spell a
/// multi-column target key the same way.
fn spell_composite_key(desc: &EntityDescriptor, keys: &[&FieldDescriptor]) -> Vec<String> {
    keys.iter()
        .map(|kf| {
            desc.ref_cols
                .iter()
                .find(|rc| rc.field == kf.name)
                .map(|rc| rc.column.to_string())
                .unwrap_or_else(|| kf.name.to_string())
        })
        .collect()
}

/// An index over the descriptors being lowered together, so a reference field can
/// resolve its target's key spelling. Built once per [`build_catalog`] call.
struct RefResolver {
    by_name: HashMap<&'static str, &'static EntityDescriptor>,
    /// Family roots keyed by their generated key-enum name, so a
    /// [`FieldKind::PolyReference`] typed `<Root>Id` finds its family (Slice 4).
    by_id_enum: HashMap<&'static str, &'static EntityDescriptor>,
}

impl RefResolver {
    fn new(entities: &[&'static EntityDescriptor]) -> Self {
        RefResolver {
            by_name: entities.iter().map(|e| (e.name, *e)).collect(),
            by_id_enum: entities
                .iter()
                .filter_map(|e| e.id_enum.map(|id| (id, *e)))
                .collect(),
        }
    }

    /// Resolve a polymorphic reference to `(target family, tag column, fk
    /// columns)` (Slice 4). The family comes from the enum name; the tag is the
    /// per-site override else the family's `tag_col`; the fk columns are the
    /// family key spelled by the per-site override / family `ref_col` (single
    /// key) or each key part via the family's `ref_cols` (composite). An
    /// unresolved enum falls back to the `Id`-stripped name so the loader reports
    /// the missing family.
    fn poly_reference(&self, pr: &PolyRef) -> (String, Option<String>, Vec<String>) {
        let family = self.by_id_enum.get(pr.id_enum).copied();
        let to = family
            .map(|f| f.name.to_string())
            .unwrap_or_else(|| strip_id(pr.id_enum));
        let tag = pr
            .tag_col
            .or_else(|| family.and_then(|f| f.tag_col))
            .map(str::to_string);
        let fk = match family {
            None => pr.ref_col.map(|c| vec![c.to_string()]).unwrap_or_default(),
            Some(desc) => {
                let keys = key_fields(desc);
                if keys.len() <= 1 {
                    // single-key family: site override → family ref_col → key name
                    let col = pr
                        .ref_col
                        .or(desc.ref_col)
                        .map(str::to_string)
                        .or_else(|| keys.first().map(|f| f.name.to_string()))
                        .unwrap_or_default();
                    vec![col]
                } else {
                    // composite family: each key part via the family's ref_cols
                    spell_composite_key(desc, &keys)
                }
            }
        };
        (to, tag, fk)
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
        let Some(&desc) = self.by_name.get(target) else {
            return vec![field_name.to_string()];
        };
        let keys = key_fields(desc);
        if keys.len() <= 1 {
            return vec![field_name.to_string()];
        }
        spell_composite_key(desc, &keys)
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
        FieldKind::PolyReference(pr) => {
            let (to, type_column, fk) = resolver.poly_reference(&pr);
            let ty = TypeRef::Ref {
                name: to.clone(),
                entity: true,
            };
            let rel = Relation {
                to,
                cardinality: Cardinality::One,
                kind: RelKind::Association,
                properties: None,
                table: None,
                fk_columns: Some(fk),
                type_column,
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
/// A family root (`abstract_leaves` non-empty) lowers to an abstract entity with
/// no table of its own; a leaf carries its `extends` so the loader inherits the
/// family key + columns (Slice 4).
fn lower_entity(d: &'static EntityDescriptor, resolver: &RefResolver) -> IrEntity {
    let fields = expanded_fields(d);
    let key = fields
        .iter()
        .filter(|f| f.key)
        .map(|f| f.name.to_string())
        .collect();
    let is_abstract = !d.abstract_leaves.is_empty();
    IrEntity {
        name: d.name.to_string(),
        // an abstract root has no table of its own (@name belongs on the leaves)
        table: if is_abstract {
            None
        } else {
            d.table.map(str::to_string)
        },
        is_abstract,
        extends: d.extends.map(str::to_string),
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

// ═════════════════════════════════════════════════════════════════════════════
// Slice 5 — the op surface (`api.json`)
//
// `derive-front-end.md` §2.7: **the impl that actually runs IS the interface**.
// `#[fluessig::export]` on an `impl` block captures each method's shape (name,
// params, return, op kind) into an [`InterfaceDescriptor`] — pure `&'static`
// data, exactly like [`EntityDescriptor`] — and `catalog!`'s `api:` root list
// lowers those descriptors into the same `api.json` the loader + bindgen already
// consume, so declaration/implementation drift is impossible.
// ═════════════════════════════════════════════════════════════════════════════

/// A type (or unit-struct "namespace") whose `#[fluessig::export] impl` block
/// was captured into an op interface. `#[fluessig::export]` expands to an
/// `impl ApiExport for Self` carrying the `&'static InterfaceDescriptor`, so
/// `catalog!`'s `api:` list can reach it as `<T as ApiExport>::DESCRIPTOR` —
/// the op-surface twin of [`Entity`]/[`Edge`].
pub trait ApiExport {
    /// The descriptor the `#[fluessig::export]` macro expands to.
    const DESCRIPTOR: &'static InterfaceDescriptor;
}

/// One op interface — the `#[fluessig::export] impl <Name>` block: its name (the
/// `Self` type), the impl block's `///` doc, and the ops it exposes, in
/// declaration order. Lowers to one `api.json` `ApiInterface`.
#[derive(Debug, Clone, Copy)]
pub struct InterfaceDescriptor {
    /// The interface name — the `Self` type of the exported impl (`"Entl"`).
    pub name: &'static str,
    /// The impl block's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The captured ops, in declaration order.
    pub ops: &'static [OpDescriptor],
}

/// One captured method: its Rust (snake_case) name, doc, op kind, params, and
/// return type. The name/param names are camelCased at lowering to match the
/// `api.json` op-surface convention (the TypeSpec `interface` path spells them
/// lowerCamel too).
#[derive(Debug, Clone, Copy)]
pub struct OpDescriptor {
    /// The Rust method name (snake_case); camelCased at lowering.
    pub name: &'static str,
    /// The method's `///` doc comment, if any.
    pub doc: Option<&'static str>,
    /// The op kind — `ctor` / plain unary / `stream` / `manual`.
    pub kind: OpKind,
    /// The method params (receiver excluded), in declaration order.
    pub params: &'static [ParamDescriptor],
    /// The return type as an op-surface type. A `ctor` is always `void`; a
    /// `stream` carries its iterator's `Item` type (the per-batch type); a
    /// `Result<T>` wrapper is transparent (unwrapped to `T`).
    pub returns: ApiTypeDesc,
}

/// One op param: its Rust (snake_case) name (camelCased at lowering), its
/// op-surface type, and whether it is optional (an `Option<T>` param lowers to
/// `optional: true` carrying the *unwrapped* `T` — params use `optional`,
/// returns use `nullable`, mirroring the TypeSpec op path).
#[derive(Debug, Clone, Copy)]
pub struct ParamDescriptor {
    /// The Rust param name (snake_case); camelCased at lowering.
    pub name: &'static str,
    /// The param's op-surface type.
    pub ty: ApiTypeDesc,
    /// `Option<T>` param ⇒ `true`.
    pub optional: bool,
}

/// The four op kinds (`derive-front-end.md` §2.7). Mirrors [`fluessig::api::Shape`];
/// kept as its own front-end enum so the descriptor layer doesn't depend on the
/// loader's serde types at the capture site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// `#[fluessig(ctor)]` — a constructor. Returns `void` on the op surface.
    Ctor,
    /// An untagged method — a plain unary op (the default).
    Unary,
    /// `#[fluessig(stream)]` — returns an iterator/stream; bindgen maps it to a
    /// JS async iterator / Python generator / Ruby Enumerator.
    Stream,
    /// `#[fluessig(manual)]` — recorded in `api.json` but hand-written per
    /// binding (not auto-bound).
    Manual,
}

/// An op-surface type as pure `&'static` data — the front-end twin of
/// [`fluessig::api::ApiType`], recursive through `&'static` so it lives in a
/// `const`. Lowered to `ApiType` by [`lower_api_type`].
#[derive(Debug, Clone, Copy)]
pub enum ApiTypeDesc {
    /// A scalar name (`"string"`, `"int64"`, `"boolean"`, `"bytes"`, `"void"`, …).
    Scalar(&'static str),
    /// A model/DTO or entity reference (`{ "model": name }`).
    Model(&'static str),
    /// An enum reference (`{ "enum": name }`).
    Enum(&'static str),
    /// A list of the inner type (`{ "list": inner }`).
    List(&'static ApiTypeDesc),
    /// A nullable inner type (`{ "nullable": inner }`) — an `Option<T>` return.
    Nullable(&'static ApiTypeDesc),
}

/// Lower an [`ApiTypeDesc`] to the loader's [`fluessig::api::ApiType`].
fn lower_api_type(t: &ApiTypeDesc) -> ApiType {
    match t {
        ApiTypeDesc::Scalar(s) => ApiType::Scalar((*s).to_string()),
        ApiTypeDesc::Model(m) => ApiType::Model {
            model: (*m).to_string(),
        },
        ApiTypeDesc::Enum(e) => ApiType::Enum {
            r#enum: (*e).to_string(),
        },
        ApiTypeDesc::List(inner) => ApiType::List {
            list: Box::new(lower_api_type(inner)),
        },
        ApiTypeDesc::Nullable(inner) => ApiType::Nullable {
            nullable: Box::new(lower_api_type(inner)),
        },
    }
}

/// Lower one captured op to an [`fluessig::api::ApiOp`]. The name + param names
/// camelCase to the op-surface convention; `readonly`/`destructive`/`stream_error`
/// stay at their (unset) defaults — those op annotations are not part of Slice 5.
fn lower_op(op: &OpDescriptor) -> ApiOp {
    ApiOp {
        name: camel(op.name),
        doc: op.doc.map(str::to_string),
        shape: match op.kind {
            OpKind::Ctor => Shape::Ctor,
            OpKind::Unary => Shape::Unary,
            OpKind::Stream => Shape::Stream,
            OpKind::Manual => Shape::Manual,
        },
        readonly: false,
        destructive: false,
        stream_error: None,
        params: op
            .params
            .iter()
            .map(|p| ApiParam {
                name: camel(p.name),
                ty: lower_api_type(&p.ty),
                optional: p.optional.then_some(true),
            })
            .collect(),
        returns: lower_api_type(&op.returns),
        bindings: Default::default(),
    }
}

/// Collect op-interface descriptors into the in-memory [`fluessig::api::ApiDoc`]
/// — the same op-layer IR the loader validates and bindgen projects. `name`
/// becomes the api `source`; `version` stamps the emitter field (as the catalog
/// path does). `models` / `unions` stay empty: Slice 5 is the OP surface — the
/// DTO/model layer (`#[derive(Record)]`, and materialising referenced entities
/// as flattened api models the way the TypeSpec path does) is a separate concern.
pub fn build_api(name: &str, version: &str, interfaces: &[&'static InterfaceDescriptor]) -> ApiDoc {
    ApiDoc {
        fluessig: Versions {
            format: fluessig::FORMAT_VERSION,
            emitter: Some(format!("fluessig-derive/{version}")),
            compiler: None,
        },
        source: Some(name.to_string()),
        models: Vec::new(),
        unions: Vec::new(),
        interfaces: interfaces
            .iter()
            .map(|i| ApiInterface {
                name: i.name.to_string(),
                doc: i.doc.map(str::to_string),
                ops: i.ops.iter().map(lower_op).collect(),
            })
            .collect(),
    }
}

/// Render `api.json` — pretty-printed with a trailing newline, matching the
/// TypeSpec emitter's `JSON.stringify(…, null, 2) + "\n"` and the catalog
/// printer's convention.
pub fn to_api_json(
    name: &str,
    version: &str,
    interfaces: &[&'static InterfaceDescriptor],
) -> String {
    let api = build_api(name, version, interfaces);
    let mut json = serde_json::to_string_pretty(&api).expect("api serializes");
    json.push('\n');
    json
}
