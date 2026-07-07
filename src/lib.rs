//! fluessig — describe a typed entity graph once (TypeSpec); project it everywhere.
//!
//! The authored source is a `.tsp` (see `entl.tsp` at the crate root for the full
//! dogfood catalog); `@fluessig/emitter` lowers it to `catalog.json` (the model
//! layer) + `api.json` (the op layer). This crate is everything after the JSON:
//! the IR ([`ir`]), the loader + validator ([`catalog`]), and — as they land —
//! the schema back-ends, data codecs, and binding generator (notes/design.md §4).

pub mod api;
pub mod bindgen;
pub mod catalog;
pub mod codegen;
pub mod data;
pub mod ir;
pub mod observe;
pub mod rustfmt;
pub mod sql;

pub use catalog::{load_catalog, load_catalog_file, Diagnostics};
pub use ir::{
    Cardinality, Catalog, Derived, Entity, EnumDef, Field, RelKind, Relation, Scalar, Struct,
    TypeRef, UnionDef, UnionVariant,
};

/// The `catalog.json` / `api.json` format this crate reads (must match the
/// emitter's stamp). Format 1: named tagged unions.
pub const FORMAT_VERSION: u64 = 1;
