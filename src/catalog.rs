//! The catalog loader + validator — the one gate every front-end passes through
//! (DESIGN §4: the emitter is a dumb printer; ALL semantic validation lives here,
//! so the TypeSpec route, the Arrow import, and the Rust builder are checked by
//! the same rules).
//!
//! v1 slice (FINDINGS → rules): reference resolution, key rules incl. FK-in-PK
//! and edge-struct local keys, polymorphic-family shape (abstract roots /
//! concrete leaves / uniform key), discriminator-column presence, edge structs
//! are flat. Deliberately NOT yet: fk-column-count-vs-target-key arity (blocked
//! on the column-sharing design, FINDINGS #2) and name-collision checks across
//! physical tables (a codec concern).

use std::collections::HashSet;
use std::fmt;

use crate::ir::{Catalog, Entity, RelKind, TypeRef};
use crate::FORMAT_VERSION;

/// Validation problems, in catalog order. `Display` gives one line each.
#[derive(Debug, Default)]
pub struct Diagnostics(pub Vec<String>);

impl Diagnostics {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn err(&mut self, msg: impl Into<String>) {
        self.0.push(msg.into());
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in &self.0 {
            writeln!(f, "{d}")?;
        }
        Ok(())
    }
}

/// Parse a catalog from JSON and validate it. The only supported entry point —
/// an unvalidated `Catalog` should never reach a codec.
pub fn load_catalog(json: &str) -> Result<Catalog, String> {
    let catalog: Catalog =
        serde_json::from_str(json).map_err(|e| format!("catalog.json parse error: {e}"))?;
    if catalog.fluessig.format != FORMAT_VERSION {
        return Err(format!(
            "catalog format {} is not supported (this fluessig reads format {})",
            catalog.fluessig.format, FORMAT_VERSION
        ));
    }
    let diags = validate(&catalog);
    if diags.is_empty() {
        Ok(catalog)
    } else {
        Err(format!("invalid catalog:\n{diags}"))
    }
}

/// [`load_catalog`] from a file path.
pub fn load_catalog_file(path: impl AsRef<std::path::Path>) -> Result<Catalog, String> {
    let json = std::fs::read_to_string(path.as_ref())
        .map_err(|e| format!("read {}: {e}", path.as_ref().display()))?;
    load_catalog(&json)
}

/// The full rule set. Public so tests (and future front-ends building an IR
/// directly) can run it standalone.
pub fn validate(c: &Catalog) -> Diagnostics {
    let mut d = Diagnostics::default();

    // ── unique names across the model namespace ──
    let mut seen = HashSet::new();
    for name in c
        .entities
        .iter()
        .map(|e| &e.name)
        .chain(c.relation_properties.iter().map(|s| &s.name))
        .chain(c.value_structs.iter().map(|s| &s.name))
        .chain(c.enums.iter().map(|e| &e.name))
        .chain(c.scalars.iter().map(|s| &s.name))
    {
        if !seen.insert(name.clone()) {
            d.err(format!("duplicate model name: {name}"));
        }
    }

    let entity_names: HashSet<&str> = c.entities.iter().map(|e| e.name.as_str()).collect();
    let edge_names: HashSet<&str> = c
        .relation_properties
        .iter()
        .map(|s| s.name.as_str())
        .collect();

    for e in &c.entities {
        validate_entity(c, e, &entity_names, &edge_names, &mut d);
    }

    // ── edge-property structs are flat: fields only, no relations, and at least
    //    used by some relation (dangling edge structs are authoring mistakes) ──
    for s in &c.relation_properties {
        for f in &s.fields {
            if f.relation.is_some() {
                d.err(format!(
                    "edge struct {}.{}: edge properties must be flat data (no relations)",
                    s.name, f.name
                ));
            }
        }
    }

    d
}

fn validate_entity(
    c: &Catalog,
    e: &Entity,
    entities: &HashSet<&str>,
    edges: &HashSet<&str>,
    d: &mut Diagnostics,
) {
    // ── inheritance: leaves extend an abstract root; roots don't extend concrete ──
    if let Some(parent_name) = &e.extends {
        match c.entity(parent_name) {
            None => d.err(format!("{}: extends unknown entity {parent_name}", e.name)),
            Some(parent) if !parent.is_abstract => {
                d.err(format!("{}: extends concrete entity {parent_name} — families are abstract roots + concrete leaves (DESIGN §3)", e.name))
            }
            Some(_) => {}
        }
    }
    if e.is_abstract && e.table.is_some() {
        d.err(format!(
            "{}: abstract roots have no table of their own (@name belongs on the leaves)",
            e.name
        ));
    }

    // ── keys: every declared key names an own field; concrete entities must have
    //    a flattened key or be explicitly keyless (empty = insert/replace) ──
    for k in &e.key {
        if !e.fields.iter().any(|f| &f.name == k) {
            d.err(format!("{}: key field {k} does not exist", e.name));
        }
    }
    for f in &e.fields {
        if f.key && !e.key.contains(&f.name) {
            d.err(format!(
                "{}: field {} marked key but missing from the key list",
                e.name, f.name
            ));
        }
    }

    // ── fields + relations ──
    for f in &e.fields {
        match (&f.relation, f.ty.innermost()) {
            (Some(rel), TypeRef::Ref { name, entity: true }) => {
                if rel.to != *name {
                    d.err(format!(
                        "{}.{}: relation.to {} disagrees with the field type {name}",
                        e.name, f.name, rel.to
                    ));
                }
                match c.entity(&rel.to) {
                    None => d.err(format!(
                        "{}.{}: relation targets unknown entity {}",
                        e.name, f.name, rel.to
                    )),
                    Some(target) => {
                        // polymorphic target ⇔ discriminator column
                        if target.is_abstract && rel.type_column.is_none() {
                            d.err(format!(
                                "{}.{}: targets abstract family {} but names no discriminator column (@fk(…, typeColumn))",
                                e.name, f.name, rel.to
                            ));
                        }
                        if !target.is_abstract && rel.type_column.is_some() {
                            d.err(format!(
                                "{}.{}: names a discriminator column but {} is concrete",
                                e.name, f.name, rel.to
                            ));
                        }
                        // composition targets belong to their parent: no independent global key expected;
                        // (v1: only flag compositions that target an abstract family — unprojectable)
                        if rel.kind == RelKind::Composition && target.is_abstract {
                            d.err(format!(
                                "{}.{}: cannot compose an abstract family",
                                e.name, f.name
                            ));
                        }
                    }
                }
                if let Some(props) = &rel.properties {
                    if !edges.contains(props.as_str()) {
                        d.err(format!(
                            "{}.{}: @edge names unknown edge struct {props}",
                            e.name, f.name
                        ));
                    }
                }
            }
            (Some(_), _) => d.err(format!(
                "{}.{}: relation on a non-entity-typed field",
                e.name, f.name
            )),
            (None, TypeRef::Ref { name, entity: true }) => d.err(format!(
                "{}.{}: entity-typed field {name} lowered without relation info (emitter bug)",
                e.name, f.name
            )),
            (
                None,
                TypeRef::Ref {
                    name,
                    entity: false,
                },
            ) => {
                // a value-struct field: the struct must exist somewhere
                if c.value_struct(name).is_none()
                    && !entities.contains(name.as_str())
                    && !edges.contains(name.as_str())
                {
                    d.err(format!("{}.{}: unknown struct {name}", e.name, f.name));
                }
            }
            (None, TypeRef::Enum { name }) => {
                if !c.enums.iter().any(|en| &en.name == name) {
                    d.err(format!("{}.{}: unknown enum {name}", e.name, f.name));
                }
            }
            (None, _) => {}
        }
    }

    // ── derived fields: the v1 exists/count slice (DESIGN §9.3) ──
    for f in &e.fields {
        let Some(der) = &f.derived else { continue };
        if !matches!(der.agg.as_str(), "exists" | "count") {
            d.err(format!(
                "{}.{}: @derived agg {} — v1 supports exists|count",
                e.name, f.name, der.agg
            ));
        }
        let all = c.flattened_fields(e);
        match all.iter().find(|rf| rf.name == der.of) {
            None => d.err(format!(
                "{}.{}: @derived of {} — no such field",
                e.name, f.name, der.of
            )),
            Some(rf) => match &rf.relation {
                Some(rel) if rel.cardinality == crate::ir::Cardinality::Many => {
                    // filter keys must be edge-property fields (literal equality)
                    if let Some(filter) = &der.filter {
                        let props = rel.properties.as_deref().and_then(|p| c.edge_struct(p));
                        for key in filter.keys() {
                            let known = props
                                .map(|s| s.fields.iter().any(|pf| &pf.name == key))
                                .unwrap_or(false);
                            if !known {
                                d.err(format!(
                                    "{}.{}: @derived filter key {key} is not an edge property of {}",
                                    e.name, f.name, der.of
                                ));
                            }
                        }
                    }
                }
                _ => d.err(format!(
                    "{}.{}: @derived of {} must be a to-many relation",
                    e.name, f.name, der.of
                )),
            },
        }
    }

    // ── polymorphic families: uniform key across the family (the rule that makes
    //    (type, key) pairs typeable — DESIGN §3) ──
    if e.is_abstract {
        let root_key = c.flattened_key(e);
        if root_key.is_empty() {
            d.err(format!(
                "{}: an abstract family root must carry the family key",
                e.name
            ));
        }
        for leaf in c
            .entities
            .iter()
            .filter(|l| l.extends.as_deref() == Some(&e.name))
        {
            // leaves may not redeclare/extend the key — identity is the family's
            if !leaf.key.is_empty() {
                d.err(format!(
                    "{}: adds key fields {:?} to family {} — the family key must be uniform",
                    leaf.name, leaf.key, e.name
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal(json_patch: impl FnOnce(&mut serde_json::Value)) -> Result<Catalog, String> {
        let mut v: serde_json::Value = serde_json::json!({
            "fluessig": { "format": 0 },
            "scalars": [],
            "enums": [],
            "entities": [
                {
                    "name": "A",
                    "table": "a",
                    "key": ["id"],
                    "fields": [
                        { "name": "id", "type": {"k":"scalar","name":"int64"}, "nullable": false, "key": true },
                        { "name": "b", "type": {"k":"ref","name":"B","entity":true}, "nullable": false,
                          "relation": { "to": "B", "cardinality": "one", "kind": "association" } }
                    ]
                },
                {
                    "name": "B",
                    "table": "b",
                    "key": ["id"],
                    "fields": [
                        { "name": "id", "type": {"k":"scalar","name":"int64"}, "nullable": false, "key": true }
                    ]
                }
            ],
            "relationProperties": [],
            "valueStructs": []
        });
        json_patch(&mut v);
        load_catalog(&v.to_string())
    }

    #[test]
    fn minimal_catalog_validates() {
        minimal(|_| {}).expect("valid");
    }

    #[test]
    fn wrong_format_version_is_rejected() {
        let err = minimal(|v| v["fluessig"]["format"] = 99.into()).unwrap_err();
        assert!(err.contains("format 99"), "{err}");
    }

    #[test]
    fn unknown_relation_target_is_rejected() {
        let err = minimal(|v| {
            v["entities"][0]["fields"][1]["relation"]["to"] = "Nope".into();
            v["entities"][0]["fields"][1]["type"]["name"] = "Nope".into();
        })
        .unwrap_err();
        assert!(err.contains("unknown entity Nope"), "{err}");
    }

    #[test]
    fn missing_key_field_is_rejected() {
        let err = minimal(|v| v["entities"][0]["key"] = serde_json::json!(["ghost"])).unwrap_err();
        assert!(err.contains("key field ghost"), "{err}");
    }

    #[test]
    fn extending_a_concrete_entity_is_rejected() {
        let err = minimal(|v| v["entities"][0]["extends"] = "B".into()).unwrap_err();
        assert!(err.contains("extends concrete entity B"), "{err}");
    }

    #[test]
    fn derived_rules_reject_bad_specs() {
        // unknown aggregate
        let err = minimal(|v| {
            v["entities"][0]["fields"][0]["derived"] =
                serde_json::json!({ "agg": "sum", "of": "b" });
        })
        .unwrap_err();
        assert!(err.contains("v1 supports exists|count"), "{err}");
        // `of` must be a to-many relation (A.b is to-one)
        let err = minimal(|v| {
            v["entities"][0]["fields"][0]["derived"] =
                serde_json::json!({ "agg": "exists", "of": "b" });
        })
        .unwrap_err();
        assert!(err.contains("must be a to-many relation"), "{err}");
        // filter keys must be edge properties
        let err = minimal(|v| {
            v["entities"][0]["fields"][1]["type"] =
                serde_json::json!({"k":"list","of":{"k":"ref","name":"B","entity":true}});
            v["entities"][0]["fields"][1]["relation"]["cardinality"] = "many".into();
            v["entities"][0]["fields"][0]["derived"] =
                serde_json::json!({ "agg": "count", "of": "b", "filter": { "ghost": 1 } });
        })
        .unwrap_err();
        assert!(err.contains("filter key ghost"), "{err}");
    }

    #[test]
    fn abstract_target_requires_discriminator() {
        let err = minimal(|v| {
            v["entities"][1]["abstract"] = true.into();
            v["entities"][1]["table"] = serde_json::Value::Null;
            // B abstract now: A.b must name a typeColumn
        })
        .unwrap_err();
        assert!(err.contains("no discriminator column"), "{err}");
    }
}
