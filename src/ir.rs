//! The IR — serde mirror of `catalog.json` (format 0, frozen by
//! `emitter/catalog.schema.json`). Deliberately its own vocabulary: nothing here
//! names `arrow`, TypeSpec, or any store — those are all codecs over this
//! (notes/design.md §2/§4).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A whole lowered catalog: the model layer of one authored `.tsp`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub fluessig: Versions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub scalars: Vec<Scalar>,
    pub enums: Vec<EnumDef>,
    pub entities: Vec<Entity>,
    pub relation_properties: Vec<Struct>,
    pub value_structs: Vec<Struct>,
}

/// The version stamp the emitter writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Versions {
    pub format: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler: Option<String>,
}

/// A semantic scalar (`scalar Oid extends bytes`) — logical name + physical carrier.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scalar {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<Variant>,
}

/// `value` is the stored wire value when it differs from the name (`added: "A"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// A stored entity (table / collection / node label). `abstract` + `extends`
/// carry the polymorphic families (abstract roots, concrete leaves).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entity {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    #[serde(
        rename = "abstract",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub is_abstract: bool,
    #[serde(rename = "extends", default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Own (non-inherited) key field names, declaration order. Use
    /// [`Catalog::flattened_key`] for the full PK including inherited members.
    pub key: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub fields: Vec<Field>,
}

/// A value struct or an edge-property struct: fields, no independent identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Struct {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: TypeRef,
    pub nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// PK member (own level; edge structs: local key — FINDINGS #1/#3).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub key: bool,
    /// `@name` column override (scalar fields; a relation's `@name` is its table).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// `@defaultValue` — DDL DEFAULT (FINDINGS #4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// `@derived` — DESIGN §9.3, the v1 exists/count slice (virtual projection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived: Option<Derived>,
    /// Present iff the field's innermost type is an entity (Layer B).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<Relation>,
}

/// A derived field: one aggregate over one same-entity to-many relation,
/// filtered only by literal equality on the relation's edge properties.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derived {
    /// `exists` | `count` (the v1 slice of the closed family).
    pub agg: String,
    /// The name of a to-many relation field on the same entity.
    pub of: String,
    /// Literal-equality filter on edge-property fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<serde_json::Map<String, Value>>,
}

/// A type reference — fluessig's own taxonomy (Layer A), never `arrow::DataType`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "k", rename_all = "lowercase", deny_unknown_fields)]
pub enum TypeRef {
    /// A (possibly semantic) scalar; `base` is the physical carrier when refined.
    Scalar {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    /// A model reference; `entity: true` ⇒ a relation target, `false` ⇒ a value struct.
    Ref {
        name: String,
        entity: bool,
    },
    Enum {
        name: String,
    },
    List {
        of: Box<TypeRef>,
    },
}

impl TypeRef {
    /// The innermost named type (through lists).
    pub fn innermost(&self) -> &TypeRef {
        match self {
            TypeRef::List { of } => of.innermost(),
            other => other,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Cardinality {
    One,
    Many,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelKind {
    Association,
    Composition,
}

/// Layer B: a declared relation between entities (DESIGN §2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Relation {
    /// Target entity (possibly an abstract family root ⇒ polymorphic).
    pub to: String,
    pub cardinality: Cardinality,
    pub kind: RelKind,
    /// Edge-property struct name (`@edge`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<String>,
    /// `@name`: the association/edge table's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// `@fk`: target-side FK column name(s) (FINDINGS #2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fk_columns: Option<Vec<String>>,
    /// `@fk`'s second arg: the polymorphic discriminator column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_column: Option<String>,
    /// `@fkSource`: source-side column name(s) on an association/edge table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_columns: Option<Vec<String>>,
    /// `@fkSource`'s second arg: the discriminator when the SOURCE is an abstract family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type_column: Option<String>,
}

impl Catalog {
    pub fn entity(&self, name: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| e.name == name)
    }

    pub fn edge_struct(&self, name: &str) -> Option<&Struct> {
        self.relation_properties.iter().find(|s| s.name == name)
    }

    pub fn value_struct(&self, name: &str) -> Option<&Struct> {
        self.value_structs.iter().find(|s| s.name == name)
    }

    /// The full primary key of an entity: inherited (family-root) members first,
    /// then own — matching column order in the physical projections.
    pub fn flattened_key(&self, entity: &Entity) -> Vec<String> {
        let mut key = match entity.extends.as_deref().and_then(|p| self.entity(p)) {
            Some(parent) => self.flattened_key(parent),
            None => Vec::new(),
        };
        key.extend(entity.key.iter().cloned());
        key
    }

    /// All fields of an entity including inherited ones, root-first.
    pub fn flattened_fields<'a>(&'a self, entity: &'a Entity) -> Vec<&'a Field> {
        let mut fields = match entity.extends.as_deref().and_then(|p| self.entity(p)) {
            Some(parent) => self.flattened_fields(parent),
            None => Vec::new(),
        };
        fields.extend(entity.fields.iter());
        fields
    }

    /// The physical table name: `@name` override, else the model name as-is
    /// snake_cased (the dumb rule — DESIGN §3 "Naming").
    pub fn table_name(&self, entity: &Entity) -> String {
        entity.table.clone().unwrap_or_else(|| snake(&entity.name))
    }
}

/// lowerCamel / PascalCase → snake_case (no pluralization — the dumb rule).
pub fn snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_cases() {
        assert_eq!(snake("Commit"), "commit");
        assert_eq!(snake("GhPullRequest"), "gh_pull_request");
        assert_eq!(snake("authorWhen"), "author_when");
        assert_eq!(snake("oid"), "oid");
    }
}
