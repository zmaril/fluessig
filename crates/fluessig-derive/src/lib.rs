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
//! the same file the loader validates and bindgen projects.
//!
//! Slice 6 adds **source spans** (`derive-front-end.md` §2.1/§2.8): every
//! descriptor carries a [`SourceSpan`] — the declaration's `file!()` + `line!()`,
//! captured by the macros — and [`validate_with_spans`] runs the full Rust loader
//! validation, then annotates each diagnostic with the `.rs` file:line of the
//! entity/field it names, the way a `.tsp`-authored error points at the `.tsp`
//! line. Spans live only in the descriptor side channel; they never reach the
//! lowered [`Catalog`], so `catalog.json` / `api.json` are byte-for-byte
//! unchanged.

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
pub use fluessig_derive_macros::{
    catalog, export, AbstractRoot, Edge, Entity, Enum, Record, Scalar,
};

use fluessig::api::{ApiDoc, ApiInterface, ApiOp, ApiParam, ApiType, Shape};
use fluessig::ir::{
    camel, Cardinality, Catalog, Entity as IrEntity, Field, RelKind, Relation, Scalar, Struct,
    TypeRef, Versions,
};

mod decls;
pub use decls::{
    DefaultLit, DerivedDesc, EnumDescriptor, EnumType, EnumVariantDescriptor, ScalarDescriptor,
    ScalarType,
};

mod records;
pub use records::{RecordDescriptor, RecordFieldDescriptor, RecordTypeDesc};

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

/// A **DTO / value struct** — a plain data record the op surface passes across
/// (`#[derive(Record)]`, Slice 8a Gap 2, `derive-front-end.md` §2.7 / the sketch's
/// `#[derive(Record)]`). Unlike an [`Entity`] it has no identity, no table, and no
/// key; it is a shape ops accept and return (`SinkOptions`, `SinkStats`, …). It
/// lowers to a `fluessig::ir::Struct` in the catalog's `valueStructs`, and — when
/// an op references it (directly or transitively) — it is materialised into
/// `api.json`'s `models` array, exactly as the TypeSpec op path materialises the
/// DTOs its ops reference.
pub trait Record {
    /// The descriptor the derive expands to.
    const DESCRIPTOR: &'static RecordDescriptor;
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

/// The `.rs` source location of a derive-authored declaration (Slice 6,
/// `derive-front-end.md` §2.1/§2.8). The derive macros capture each entity /
/// field / op's `file!()` + `line!()` into its descriptor so the loader's
/// diagnostics — which stay in the Rust core and name the offending entity or
/// field — can be annotated with the Rust `file:line`, the way a `.tsp`-authored
/// error points at the `.tsp` line today.
///
/// Spans live **only** in the descriptor layer (a build-time side channel). They
/// are never lowered into [`fluessig::ir::Catalog`] / [`fluessig::api::ApiDoc`],
/// so `catalog.json` / `api.json` stay byte-for-byte identical to the TypeSpec
/// path — the design frames spans as powering loader diagnostics, not as catalog
/// payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    /// The source file (`file!()`), as rustc records it (workspace-relative).
    pub file: &'static str,
    /// The 1-based line (`line!()`); `0` marks a synthetic / hand-written
    /// descriptor with no real location.
    pub line: u32,
}

impl SourceSpan {
    /// A hand-written or synthetic descriptor with no captured location — used by
    /// tests that build descriptors by hand rather than through the derive. A
    /// `line` of `0` suppresses the `file:line` prefix on an annotated diagnostic
    /// (an honest "no location" rather than a fabricated one).
    pub const UNKNOWN: SourceSpan = SourceSpan {
        file: "<unknown>",
        line: 0,
    };
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
    /// The `.rs` source location of this entity's declaration (Slice 6) — the
    /// struct's `file!()` + `line!()`. Feeds loader diagnostics only; never
    /// lowered into the catalog, so `catalog.json` is byte-for-byte unchanged.
    pub span: SourceSpan,
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
    /// `#[fluessig(default = …)]` — the DDL DEFAULT for this column (Slice 8b,
    /// entl FINDINGS #4). `None` when the field has no default.
    pub default: Option<DefaultLit>,
    /// `#[fluessig(derived(exists|count, of = "rel", filter(k = v)))]` — a derived
    /// field (Slice 8b, DESIGN §9.3 the v1 exists/count slice). `None` for a
    /// plain stored column.
    pub derived: Option<DerivedDesc>,
    /// The `.rs` source location of this field's declaration (Slice 6) — the
    /// field name's `file!()` + `line!()`. Feeds loader diagnostics only; never
    /// lowered into the catalog.
    pub span: SourceSpan,
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
    /// A field typed by a bare **named** type the macro can't classify from the
    /// token alone (Slice 8b): a declared enum (`RefKind`), a declared or stock
    /// semantic scalar (`Oid`, `Json`, `utcDateTime`, `bytes`), or a reference to
    /// another value struct. Resolved at lowering against the catalog's declared
    /// enums / scalars ([`RefResolver::resolve_named`]).
    Named(&'static str),
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
    /// The `.rs` source location of this edge struct's declaration (Slice 6).
    pub span: SourceSpan,
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
/// part via a matching `ref_cols` override, else the key field's own name.
/// Shared by the `Id<T>` FK resolver ([`RefResolver::fk_columns`]) and the
/// polymorphic-reference resolver ([`RefResolver::poly_reference`]) — both spell a
/// multi-column target key the same way. Takes the `ref_cols` list rather than a
/// descriptor so the FK resolver can pass the target's *inherited* spellings
/// (gathered along `extends` for a family leaf — Slice 8a Gap 1).
fn spell_composite_key(ref_cols: &[RefColDescriptor], keys: &[&FieldDescriptor]) -> Vec<String> {
    keys.iter()
        .map(|kf| {
            ref_cols
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
    /// Declared enum names, so a [`FieldKind::Named`] typed by an enum resolves to
    /// `TypeRef::Enum` (Slice 8b).
    enum_names: std::collections::HashSet<&'static str>,
    /// Declared semantic scalars → their `base`, so a `Named` typed by a scalar
    /// resolves to `TypeRef::Scalar { name, base }` (Slice 8b).
    scalar_bases: HashMap<&'static str, Option<&'static str>>,
}

impl RefResolver {
    fn new(
        entities: &[&'static EntityDescriptor],
        enums: &[&'static EnumDescriptor],
        scalars: &[&'static ScalarDescriptor],
    ) -> Self {
        RefResolver {
            by_name: entities.iter().map(|e| (e.name, *e)).collect(),
            by_id_enum: entities
                .iter()
                .filter_map(|e| e.id_enum.map(|id| (id, *e)))
                .collect(),
            enum_names: enums.iter().map(|e| e.name).collect(),
            scalar_bases: scalars.iter().map(|s| (s.name, s.base)).collect(),
        }
    }

    /// Resolve a [`FieldKind::Named`] type name to a [`TypeRef`] (Slice 8b): a
    /// declared enum → `Enum`; a declared semantic scalar → `Scalar { name, base }`
    /// with the declared carrier; a **stock** scalar (`Json` → `string`,
    /// `utcDateTime` / `bytes` — roots with no base) → `Scalar`; anything else → a
    /// value-struct reference (`Ref { entity: false }`), so a record referencing
    /// another record still closes. Mirrors what the TypeSpec front end records for
    /// the equivalent named type.
    fn resolve_named(&self, name: &str) -> TypeRef {
        if self.enum_names.contains(name) {
            return TypeRef::Enum {
                name: name.to_string(),
            };
        }
        if let Some(base) = self.scalar_bases.get(name) {
            return TypeRef::Scalar {
                name: name.to_string(),
                base: base.map(str::to_string),
            };
        }
        if let Some(base) = stock_scalar_base(name) {
            return TypeRef::Scalar {
                name: name.to_string(),
                base: base.map(str::to_string),
            };
        }
        TypeRef::Ref {
            name: name.to_string(),
            entity: false,
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
                    spell_composite_key(desc.ref_cols, &keys)
                }
            }
        };
        (to, tag, fk)
    }

    /// A target's full key fields, following `extends` to gather the **inherited**
    /// family key (root-first), mirroring the loader's `flattened_key` (Slice 8a
    /// Gap 1). A concrete leaf that `extends` a family root re-lists none of the
    /// family key on itself — it lives on the root — so an `Id<Leaf>` reference
    /// must walk `extends` to see the whole (possibly composite) key; without this
    /// walk a composite-keyed leaf looks single-keyed and its FK is under-spelled.
    fn flattened_key_fields(
        &self,
        desc: &'static EntityDescriptor,
    ) -> Vec<&'static FieldDescriptor> {
        let mut keys = match desc.extends.and_then(|p| self.by_name.get(p)) {
            Some(&parent) => self.flattened_key_fields(parent),
            None => Vec::new(),
        };
        keys.extend(key_fields(desc));
        keys
    }

    /// The reference-column spellings visible for a target, gathered along
    /// `extends` (the leaf's own first, then the inherited root's — so a nearer
    /// level's `ref_cols` overrides a farther one). A family leaf inherits the
    /// root's `ref_cols`, so `Id<Leaf>` spells the composite FK the same way a
    /// polymorphic reference to the family does (Slice 8a Gap 1).
    fn flattened_ref_cols(&self, desc: &'static EntityDescriptor) -> Vec<RefColDescriptor> {
        let mut cols: Vec<RefColDescriptor> = desc.ref_cols.to_vec();
        if let Some(&parent) = desc.extends.and_then(|p| self.by_name.get(p)) {
            cols.extend(self.flattened_ref_cols(parent));
        }
        cols
    }

    /// The foreign-key columns a field materialises to reference `target`.
    ///
    /// A single-column target takes the referencing **field name** (`repo_id:
    /// Id<Repo>` ⇒ `["repo_id"]`) — the site names it, matching how scalar fields
    /// map name→column. A composite (multi-key) target can't be named from one
    /// field, so its columns come from the target's key order, each spelled by
    /// the target's `ref_cols` override (else the key field's own name) — the
    /// reference spelling declared once on the target. Flatten-embedded keys
    /// participate: the target's key is read from its *expanded* fields; a family
    /// leaf's inherited key is read by following `extends` to the root (Slice 8a
    /// Gap 1), so `Id<Leaf>` into a composite-keyed family spells the whole key.
    ///
    /// A dangling target (typo'd `Id<T>`) resolves to `[field_name]`; the emitted
    /// `relation.to` still points at the missing entity, so the loader catches it.
    fn fk_columns(&self, field_name: &str, target: &str) -> Vec<String> {
        let Some(&desc) = self.by_name.get(target) else {
            return vec![field_name.to_string()];
        };
        let keys = self.flattened_key_fields(desc);
        if keys.len() <= 1 {
            return vec![field_name.to_string()];
        }
        spell_composite_key(&self.flattened_ref_cols(desc), &keys)
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

/// A **stock** (built-in) semantic scalar's carrier, for the names the TypeSpec
/// front end treats as built-ins rather than declared scalars (Slice 8b): `Json`
/// refines `string`; `utcDateTime` / `offsetDateTime` / `bytes` are roots. `None`
/// ⇒ not a stock scalar (the resolver then tries a value-struct reference). The
/// numeric / `string` / `boolean` builtins never reach here — they arrive as
/// primitive [`ScalarKind`]s from the macro.
fn stock_scalar_base(name: &str) -> Option<Option<&'static str>> {
    match name {
        "Json" => Some(Some("string")),
        "utcDateTime" | "offsetDateTime" | "bytes" => Some(None),
        _ => None,
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
        FieldKind::Named(name) => (resolver.resolve_named(name), None),
        FieldKind::Flatten(_) => unreachable!("flatten fields are expanded before lowering"),
    };
    Field {
        name: f.name.to_string(),
        ty,
        nullable: f.nullable,
        doc: f.doc.map(str::to_string),
        key: f.key,
        column: None,
        default: f.default.map(DefaultLit::to_value),
        derived: f.derived.map(lower_derived),
        relation,
    }
}

/// Lower a [`DerivedDesc`] to the catalog's [`fluessig::ir::Derived`] (Slice 8b):
/// the aggregate + relation name carry over, and the `(field, value)` filter pairs
/// become the `{field: value}` map the loader validates and `sql.rs` renders into
/// the `<table>_derived` view.
fn lower_derived(d: DerivedDesc) -> fluessig::ir::Derived {
    let filter = if d.filter.is_empty() {
        None
    } else {
        Some(
            d.filter
                .iter()
                .map(|(k, v)| (k.to_string(), serde_json::Value::from(*v)))
                .collect(),
        )
    };
    fluessig::ir::Derived {
        agg: d.agg.to_string(),
        of: d.of.to_string(),
        filter,
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
    // Discriminator columns for a polymorphic edge side (Slice 8b): a poly SOURCE
    // (`subject: GhSubjectId` on `gh_labeled`) carries `sourceTypeColumn`; a poly
    // TARGET (`child: GitObjectId` on `tree_entries`) carries `typeColumn`.
    let mut source_type_column = None;
    let mut type_column = None;
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
            (EdgeRole::Source, FieldKind::PolyReference(pr)) => {
                let (_to, tag, cols) = resolver.poly_reference(&pr);
                source_columns = cols;
                source_type_column = tag;
            }
            (EdgeRole::Target, FieldKind::PolyReference(pr)) => {
                let (_to, tag, cols) = resolver.poly_reference(&pr);
                target_columns = cols;
                type_column = tag;
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
        type_column,
        source_columns: Some(source_columns),
        source_type_column,
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

/// The registry of declared enums + scalars a catalog carries alongside its
/// entities (Slice 8b): the enums / scalars a `Named` field type resolves against,
/// and the arrays lowered into the catalog's `enums` / `scalars`. Grouped so the
/// full-form builders don't grow two more positional slices.
#[derive(Clone, Copy, Default)]
pub struct TypeDecls<'a> {
    /// The `#[derive(Enum)]` descriptors — lowered to the catalog's `enums`.
    pub enums: &'a [&'static EnumDescriptor],
    /// The `#[derive(Scalar)]` descriptors — lowered to the catalog's `scalars`.
    pub scalars: &'a [&'static ScalarDescriptor],
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
    build_catalog_full(name, version, entities, edges, &[])
}

/// Collect entities + edges + **records** into the in-memory catalog (Slice 8a
/// Gap 2). Records lower to `valueStructs` — the DTO layer the op surface
/// materialises into `api.json`'s `models`. The no-records form
/// [`build_catalog_with_edges`] delegates here with an empty record slice, so the
/// Slice 1–5 callers are unchanged.
pub fn build_catalog_full(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
) -> Catalog {
    build_catalog_typed(
        name,
        version,
        entities,
        edges,
        records,
        TypeDecls::default(),
    )
}

/// Collect entities + edges + records + **declared enums/scalars** into the
/// catalog (Slice 8b). The enums lower to `enums`, the scalars to `scalars`, and
/// both feed the [`RefResolver`] so a `Named` field type resolves to the right
/// `TypeRef`. The plain [`build_catalog_full`] delegates here with empty decls, so
/// the Slice 1–8a callers are unchanged.
pub fn build_catalog_typed(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    decls: TypeDecls,
) -> Catalog {
    let resolver = RefResolver::new(entities, decls.enums, decls.scalars);
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
        scalars: decls
            .scalars
            .iter()
            .map(|s| Scalar {
                name: s.name.to_string(),
                base: s.base.map(str::to_string),
            })
            .collect(),
        unions: Vec::new(),
        enums: decls.enums.iter().map(|e| lower_enum(e)).collect(),
        entities: ir_entities,
        relation_properties,
        value_structs: records
            .iter()
            .map(|r| records::lower_record(r, &resolver))
            .collect(),
    }
}

/// Lower an [`EnumDescriptor`] to the catalog's [`fluessig::ir::EnumDef`] (Slice
/// 8b): each variant's catalog name + optional stored wire value (`added: "A"` →
/// `value: Some("A")`).
fn lower_enum(e: &EnumDescriptor) -> fluessig::ir::EnumDef {
    fluessig::ir::EnumDef {
        name: e.name.to_string(),
        variants: e
            .variants
            .iter()
            .map(|v| fluessig::ir::Variant {
                name: v.name.to_string(),
                value: v.value.map(serde_json::Value::from),
                bindings: Default::default(),
            })
            .collect(),
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
    to_catalog_json_full(name, version, entities, edges, &[])
}

/// Render `catalog.json` for a catalog with edges + records (Slice 8a Gap 2).
pub fn to_catalog_json_full(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
) -> String {
    to_catalog_json_typed(
        name,
        version,
        entities,
        edges,
        records,
        TypeDecls::default(),
    )
}

/// Render `catalog.json` for a catalog with edges + records + declared
/// enums/scalars (Slice 8b).
pub fn to_catalog_json_typed(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    decls: TypeDecls,
) -> String {
    let catalog = build_catalog_typed(name, version, entities, edges, records, decls);
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
    /// The `.rs` source location of the exported `impl` block (Slice 6).
    pub span: SourceSpan,
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
    /// The `.rs` source location of this method's declaration (Slice 6).
    pub span: SourceSpan,
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
    /// The `.rs` source location of this param's declaration (Slice 6).
    pub span: SourceSpan,
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
/// path does).
///
/// Slice 8a Gap 2 materialises the **`models`** array: every entity/DTO an op
/// references — directly, or transitively through a referenced DTO's fields — is
/// flattened into a `models` entry exactly as the TypeSpec op path does (a to-one
/// relation becomes its FK field(s), a polymorphic one prepends the discriminator,
/// to-many relations are dropped; see [`build_models`]). The `entities`, `edges`,
/// and `records` are the same catalog roots [`build_catalog_full`] takes, so the
/// op layer and the model layer are lowered from one consistent catalog.
pub fn build_api(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    interfaces: &[&'static InterfaceDescriptor],
) -> ApiDoc {
    build_api_typed(
        name,
        version,
        entities,
        edges,
        records,
        interfaces,
        TypeDecls::default(),
    )
}

/// Collect op-interface descriptors into the [`fluessig::api::ApiDoc`], with the
/// declared enums/scalars threaded through so an op/model field typed by an enum
/// lowers to `{ enum }` and one typed by a semantic scalar to its scalar name
/// (Slice 8b). The plain [`build_api`] delegates here with empty decls.
#[allow(clippy::too_many_arguments)]
pub fn build_api_typed(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    interfaces: &[&'static InterfaceDescriptor],
    decls: TypeDecls,
) -> ApiDoc {
    let catalog = build_catalog_typed(name, version, entities, edges, records, decls);
    let api_interfaces: Vec<ApiInterface> = interfaces
        .iter()
        .map(|i| ApiInterface {
            name: i.name.to_string(),
            doc: i.doc.map(str::to_string),
            ops: i.ops.iter().map(lower_op).collect(),
        })
        .collect();
    ApiDoc {
        fluessig: Versions {
            format: fluessig::FORMAT_VERSION,
            emitter: Some(format!("fluessig-derive/{version}")),
            compiler: None,
        },
        source: Some(name.to_string()),
        models: records::build_models(&catalog, &api_interfaces),
        unions: Vec::new(),
        interfaces: api_interfaces,
    }
}

/// Render `api.json` — pretty-printed with a trailing newline, matching the
/// TypeSpec emitter's `JSON.stringify(…, null, 2) + "\n"` and the catalog
/// printer's convention.
pub fn to_api_json(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    interfaces: &[&'static InterfaceDescriptor],
) -> String {
    to_api_json_typed(
        name,
        version,
        entities,
        edges,
        records,
        interfaces,
        TypeDecls::default(),
    )
}

/// Render `api.json` for a catalog with declared enums/scalars (Slice 8b).
#[allow(clippy::too_many_arguments)]
pub fn to_api_json_typed(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
    records: &[&'static RecordDescriptor],
    interfaces: &[&'static InterfaceDescriptor],
    decls: TypeDecls,
) -> String {
    let api = build_api_typed(name, version, entities, edges, records, interfaces, decls);
    let mut json = serde_json::to_string_pretty(&api).expect("api serializes");
    json.push('\n');
    json
}

// ═════════════════════════════════════════════════════════════════════════════
// Slice 6 — source spans in loader diagnostics
//
// `derive-front-end.md` §2.1/§2.8: the descriptors carry each declaration's
// `.rs` file:line, and the loader — which stays in the Rust core and names the
// offending entity/field — has its diagnostics annotated with that location, so
// a schema authored in Rust that fails validation points at the `.rs` line the
// way a `.tsp`-authored error names the `.tsp` location. Spans ride the
// descriptor side channel ONLY: they never touch the lowered catalog, so
// `catalog.json` / `api.json` stay byte-for-byte unchanged.
// ═════════════════════════════════════════════════════════════════════════════

/// One loader diagnostic annotated with the `.rs` source location of the entity
/// or field it names (Slice 6). `Display` renders `file:line: message` when a
/// span was resolved, else the bare message — a diagnostic whose locus isn't a
/// captured entity/field (a cross-model name clash, an `edge struct …` message)
/// keeps the loader's own wording, unlocated rather than mislocated.
#[derive(Debug, Clone)]
pub struct SpannedDiagnostic {
    /// The resolved `.rs` location of the offending declaration, if the
    /// diagnostic named a known entity/field carrying a real span.
    pub span: Option<SourceSpan>,
    /// The loader's original message (unchanged).
    pub message: String,
}

impl fmt::Display for SpannedDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.span {
            Some(s) if s.line != 0 => write!(f, "{}:{}: {}", s.file, s.line, self.message),
            _ => f.write_str(&self.message),
        }
    }
}

/// Render a whole diagnostic list one-per-line — the derive front end's twin of
/// [`fluessig::Diagnostics`]'s `Display`, but each line located at its `.rs`
/// source.
pub fn render_diagnostics(diags: &[SpannedDiagnostic]) -> String {
    let mut out = String::new();
    for d in diags {
        out.push_str(&d.to_string());
        out.push('\n');
    }
    out
}

/// A name→span index over the descriptors being validated, so a loader
/// diagnostic — which leads with the offending `Entity` or `Entity.field` name —
/// maps back to the `.rs` file:line that declared it.
struct SpanIndex {
    entities: HashMap<String, SourceSpan>,
    fields: HashMap<(String, String), SourceSpan>,
}

impl SpanIndex {
    fn build(entities: &[&'static EntityDescriptor], edges: &[&'static EdgeDescriptor]) -> Self {
        let mut ix = SpanIndex {
            entities: HashMap::new(),
            fields: HashMap::new(),
        };
        for e in entities {
            ix.entities.insert(e.name.to_string(), e.span);
            // flatten-expanded so an embedded column resolves to where it was
            // actually declared, not the embedding site.
            for f in expanded_fields(e) {
                ix.fields
                    .insert((e.name.to_string(), f.name.to_string()), f.span);
            }
        }
        for edge in edges {
            // an edge surfaces as a to-many field on its `from` entity; a loader
            // diagnostic on it reads `{from}.{expose}`.
            let expose = edge
                .expose
                .map(str::to_string)
                .unwrap_or_else(|| fluessig::ir::snake(edge.name));
            ix.fields.insert((edge.from.to_string(), expose), edge.span);
            for ef in edge.fields {
                ix.fields.insert(
                    (edge.name.to_string(), ef.field.name.to_string()),
                    ef.field.span,
                );
            }
        }
        ix
    }

    /// Annotate one loader message: resolve the leading `Entity` / `Entity.field`
    /// locus (the text before the first `:`) to a captured span. A locus that
    /// isn't a bare entity/field name resolves to no span and keeps the bare
    /// message.
    fn annotate(&self, message: String) -> SpannedDiagnostic {
        let locus = message.split_once(':').map_or("", |(l, _)| l.trim());
        let span = match locus.split_once('.') {
            Some((owner, field)) => self
                .fields
                .get(&(owner.to_string(), field.to_string()))
                .or_else(|| self.entities.get(owner))
                .copied(),
            None => self.entities.get(locus).copied(),
        };
        SpannedDiagnostic { span, message }
    }
}

/// Build the derived catalog, run the **full Rust loader validation** (the same
/// `fluessig::catalog::validate` every front end passes through), and annotate any
/// diagnostics with the `.rs` file:line of the offending declaration (Slice 6).
///
/// This is the derive front end's diagnostic bridge: the loader keeps naming the
/// offending `Entity` / `Entity.field` exactly as it does for the TypeSpec path,
/// and the span index maps that name back to the Rust source it was authored in.
/// A clean schema returns the validated [`Catalog`]; a broken one returns every
/// diagnostic, located. Spans never enter the catalog, so a clean run's IR is
/// byte-identical to [`build_catalog_with_edges`].
pub fn validate_with_spans(
    name: &str,
    version: &str,
    entities: &[&'static EntityDescriptor],
    edges: &[&'static EdgeDescriptor],
) -> Result<Catalog, Vec<SpannedDiagnostic>> {
    let catalog = build_catalog_with_edges(name, version, entities, edges);
    let diags = fluessig::catalog::validate(&catalog);
    if diags.is_empty() {
        return Ok(catalog);
    }
    let index = SpanIndex::build(entities, edges);
    Err(diags.0.into_iter().map(|m| index.annotate(m)).collect())
}
