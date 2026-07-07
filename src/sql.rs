//! The SQL DDL back-end (plan.txt Step 3) — project a validated [`Catalog`] into
//! per-table DDL for all three dialects. The parity gate (`tests/sql_parity.rs`)
//! holds the output against entl's hand-written templates in every dialect.
//!
//! straitjacket-allow-file:duplication — `key_columns` and `tables` both expand a
//! relation's target key into FK columns (one small shared lowering); rustfmt
//! adoption surfaced the clone. A `fk_pairs()` extraction is a fine follow-up.
//!
//! Two layers:
//! - [`tables`] lowers the catalog into structured [`TableDef`]s — the physical
//!   projection (relations → FK columns / association tables, keys → PKs,
//!   families → per-leaf tables). This is what the parity gate compares.
//! - [`render`] turns a `TableDef` into `CREATE TABLE IF NOT EXISTS` text in the
//!   template style (`__table__` placeholder capable, matching entl's sinks).

use std::collections::BTreeMap;

use crate::ir::{snake, Cardinality, Catalog, Entity, Field, TypeRef};

/// The SQL dialects fluessig projects to — one lowering, three type maps.
/// Representation choices per dialect (matching entl's sinks/store): Postgres +
/// SQLite carry oids/bytes as hex TEXT; SQLite stores booleans as INTEGER 0/1
/// and timestamps as RFC3339 TEXT; DuckDB (the engine store) keeps raw BLOB
/// oids/bytes and native TIMESTAMP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
    Sqlite,
    Duckdb,
}

/// One physical table, structured — the unit the parity gate compares.
#[derive(Debug, Clone, PartialEq)]
pub struct TableDef {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    /// PK column names, in key-declaration order. Empty = keyless.
    pub pk: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    /// Dialect type name (`text`, `bigint`, `timestamptz`, …).
    pub ty: String,
    pub not_null: bool,
    /// DDL DEFAULT as a SQL literal, when declared (`@defaultValue`).
    pub default: Option<String>,
    /// The field's doc (flows into the generated schema reference, not the DDL).
    pub doc: Option<String>,
}

impl TableDef {
    fn column(
        &mut self,
        name: impl Into<String>,
        ty: impl Into<String>,
        not_null: bool,
    ) -> &mut ColumnDef {
        self.columns.push(ColumnDef {
            name: name.into(),
            ty: ty.into(),
            not_null,
            default: None,
            doc: None,
        });
        self.columns.last_mut().unwrap()
    }
}

/// Map a Layer-A type to a column type for `dialect`. Semantic scalars choose
/// their representation here (Oid: hex text in the portable sinks, raw blob in
/// DuckDB — the design's "oids as hex text off DuckDB").
fn sql_type(dialect: Dialect, ty: &TypeRef) -> String {
    fn is_int(n: &str) -> bool {
        matches!(
            n,
            "int8" | "int16" | "int32" | "int64" | "uint8" | "uint16" | "uint32" | "safeint"
        )
    }
    /// A TypeSpec builtin (or semantic scalar) name → column type. `None` = not
    /// a name we know (caller then tries the base — the `extends` chain root).
    /// NB: match by NAME, not base — the numeric builtins all root at `numeric`
    /// (int64.baseScalar → … → numeric), so the base is useless for widths.
    fn by_name(dialect: Dialect, name: &str) -> Option<&'static str> {
        use Dialect::*;
        Some(match (dialect, name) {
            // fluessig semantic scalars
            (Postgres | Sqlite, "Oid" | "bytes") => "text", // hex text
            (Duckdb, "Oid" | "bytes") => "blob",            // raw bytes in the engine store
            (_, "Json") => "text",
            // TypeSpec builtins
            (_, "string" | "url") => "text",
            (Sqlite, "boolean") => "integer", // 0/1 (BOOL_COLUMNS coercion on extract)
            (_, "boolean") => "boolean",
            (Sqlite, n) if is_int(n) || n == "uint64" => "integer", // sqlite is typeless anyway
            (_, "int8" | "int16" | "int32" | "uint8" | "uint16") => "integer",
            (_, "int64" | "uint32" | "safeint") => "bigint",
            (_, "uint64") => "numeric",
            (Sqlite, "float32" | "float64" | "float") => "real",
            (_, "float32") => "real",
            (_, "float64" | "float") => "double precision",
            (_, "decimal" | "numeric") => "numeric",
            (Postgres, "utcDateTime" | "offsetDateTime") => "timestamptz",
            (Duckdb, "utcDateTime" | "offsetDateTime") => "timestamp",
            (Sqlite, "utcDateTime" | "offsetDateTime" | "plainDate" | "plainTime") => "text", // RFC3339
            (_, "plainDate") => "date",
            (_, "plainTime") => "time",
            (_, "duration") => "interval",
            _ => return None,
        })
    }
    match ty {
        TypeRef::Scalar { name, base } => by_name(dialect, name)
            .or_else(|| base.as_deref().and_then(|b| by_name(dialect, b)))
            .unwrap_or("text")
            .to_string(),
        TypeRef::Enum { .. } => "text".to_string(), // enums store their wire value
        // nesting → a json column (sqlite has no json type: text)
        TypeRef::Ref { .. } | TypeRef::List { .. } => match dialect {
            Dialect::Postgres => "jsonb".to_string(),
            Dialect::Duckdb => "json".to_string(),
            Dialect::Sqlite => "text".to_string(),
        },
    }
}

/// A SQL literal for a `@defaultValue`.
fn sql_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        other => other.to_string(),
    }
}

/// The physical column name of a scalar field.
fn col_name(f: &Field) -> String {
    f.column.clone().unwrap_or_else(|| snake(&f.name))
}

/// The expanded key of an entity as `(column name, column type)` pairs — scalars
/// directly, relation members through their FK columns (FINDINGS #1), recursively.
fn key_columns(c: &Catalog, d: Dialect, e: &Entity) -> Vec<(String, String)> {
    let fields = c.flattened_fields(e);
    let mut out = Vec::new();
    for name in c.flattened_key(e) {
        let f = fields
            .iter()
            .find(|f| f.name == name)
            .expect("validated: key field exists");
        match &f.relation {
            None => out.push((col_name(f), sql_type(d, &f.ty))),
            Some(rel) => {
                let target = c.entity(&rel.to).expect("validated: target exists");
                let target_key = key_columns(c, d, target);
                let fk = rel
                    .fk_columns
                    .clone()
                    .unwrap_or_else(|| target_key.iter().map(|(n, _)| n.clone()).collect());
                for (fk_col, (_, ty)) in fk.iter().zip(target_key.iter()) {
                    out.push((fk_col.clone(), ty.clone()));
                }
            }
        }
    }
    out
}

/// Lower a validated catalog into every physical table, keyed by table name:
/// one per concrete entity + one per to-many relation (association/edge tables).
pub fn tables(c: &Catalog, d: Dialect) -> BTreeMap<String, TableDef> {
    let mut out = BTreeMap::new();

    // ── entity tables ──
    for e in c.entities.iter().filter(|e| !e.is_abstract) {
        let mut t = TableDef {
            name: c.table_name(e),
            columns: Vec::new(),
            pk: Vec::new(),
        };
        for f in c.flattened_fields(e) {
            match &f.relation {
                None => {
                    let col = t.column(col_name(f), sql_type(d, &f.ty), !f.nullable);
                    col.default = f.default.as_ref().map(sql_literal);
                    col.doc = f.doc.clone();
                }
                Some(rel) if rel.cardinality == Cardinality::One => {
                    // discriminator first when polymorphic (matches gh_comments layout)
                    if let Some(tc) = &rel.type_column {
                        t.column(tc.clone(), "text", !f.nullable);
                    }
                    let target = c.entity(&rel.to).expect("validated");
                    let target_key = key_columns(c, d, target);
                    let fk = rel
                        .fk_columns
                        .clone()
                        .unwrap_or_else(|| target_key.iter().map(|(n, _)| n.clone()).collect());
                    for (fk_col, (_, ty)) in fk.iter().zip(target_key.iter()) {
                        // column sharing: an FK column may already exist (e.g. a key member)
                        if !t.columns.iter().any(|col| &col.name == fk_col) {
                            let col = t.column(fk_col.clone(), ty.clone(), !f.nullable);
                            col.doc = f.doc.clone();
                        }
                    }
                }
                Some(_) => {} // to-many → its own table below
            }
        }
        // PK: key members expanded to columns
        t.pk = key_columns(c, d, e).into_iter().map(|(n, _)| n).collect();
        out.insert(t.name.clone(), t);
    }

    // ── association / edge tables (to-many relations) ──
    for e in &c.entities {
        for f in e.fields.iter().filter(|f| {
            f.relation
                .as_ref()
                .is_some_and(|r| r.cardinality == Cardinality::Many)
        }) {
            let rel = f.relation.as_ref().unwrap();
            let name = rel.table.clone().unwrap_or_else(|| snake(&f.name));
            let mut t = TableDef {
                name: name.clone(),
                columns: Vec::new(),
                pk: Vec::new(),
            };

            // source side: the declaring entity's key (+ discriminator when the source is a family)
            let src_key = key_columns(c, d, e);
            let src_cols = rel
                .source_columns
                .clone()
                .unwrap_or_else(|| src_key.iter().map(|(n, _)| n.clone()).collect());
            let data_src: Vec<&String> = src_cols
                .iter()
                .filter(|n| Some(n.as_str()) != rel.source_type_column.as_deref())
                .collect();
            let mut src_col_names = Vec::new();
            for (src_col, (_, ty)) in data_src.iter().zip(src_key.iter()) {
                t.column((*src_col).clone(), ty.clone(), true);
                src_col_names.push((*src_col).clone());
            }
            if let Some(tc) = &rel.source_type_column {
                // insert the discriminator at its authored position
                let pos = src_cols
                    .iter()
                    .position(|n| n == tc)
                    .unwrap_or(t.columns.len());
                t.columns.insert(
                    pos.min(t.columns.len()),
                    ColumnDef {
                        name: tc.clone(),
                        ty: "text".into(),
                        not_null: true,
                        default: None,
                        doc: None,
                    },
                );
                src_col_names.insert(pos.min(src_col_names.len()), tc.clone());
            }

            // edge properties, before the target side when the local key demands it?
            // No — physical layouts differ per table (commit_parents: target then props;
            // tree_entries: props then target). Column ORDER is not part of the parity
            // contract (compared as sets); emission order: source, props, type, target.
            let mut prop_key = Vec::new();
            if let Some(props) = rel.properties.as_deref().and_then(|p| c.edge_struct(p)) {
                for pf in &props.fields {
                    let col = t.column(col_name(pf), sql_type(d, &pf.ty), !pf.nullable);
                    col.default = pf.default.as_ref().map(sql_literal);
                    col.doc = pf.doc.clone();
                    if pf.key {
                        prop_key.push(col_name(pf));
                    }
                }
            }

            // target side: discriminator + FK columns
            if let Some(tc) = &rel.type_column {
                t.column(tc.clone(), "text", true);
            }
            let target = c.entity(&rel.to).expect("validated");
            let target_key = key_columns(c, d, target);
            let fk = rel
                .fk_columns
                .clone()
                .unwrap_or_else(|| target_key.iter().map(|(n, _)| n.clone()).collect());
            let mut target_col_names = Vec::new();
            for (fk_col, (_, ty)) in fk.iter().zip(target_key.iter()) {
                if !t.columns.iter().any(|col| &col.name == fk_col) {
                    t.column(fk_col.clone(), ty.clone(), true);
                }
                target_col_names.push(fk_col.clone());
            }
            if let Some(tc) = &rel.type_column {
                target_col_names.insert(0, tc.clone());
            }

            // PK: with a local key → source + local key (FINDINGS #3);
            // without → all columns (plain join table).
            let mut pk = if prop_key.is_empty() && rel.properties.is_none() {
                let mut pk = src_col_names.clone();
                pk.extend(target_col_names.clone());
                pk
            } else if prop_key.is_empty() {
                t.columns.iter().map(|c| c.name.clone()).collect()
            } else {
                let mut pk = src_col_names.clone();
                pk.extend(prop_key);
                pk
            };
            // column sharing (FINDINGS #2): a column serving both sides appears once
            let mut seen = std::collections::HashSet::new();
            pk.retain(|c| seen.insert(c.clone()));
            t.pk = pk;
            out.insert(name, t);
        }
    }

    out
}

/// Render one table as `CREATE TABLE IF NOT EXISTS` in the entl template style.
/// `table_name` lets a sink substitute a renamed target (`__table__`).
pub fn render(t: &TableDef, table_name: &str) -> String {
    let mut out = format!("CREATE TABLE IF NOT EXISTS \"{table_name}\" (\n");
    let single_pk = (t.pk.len() == 1).then(|| t.pk[0].as_str());
    let mut lines = Vec::new();
    for col in &t.columns {
        let mut line = format!("  \"{}\" {}", col.name, col.ty);
        if Some(col.name.as_str()) == single_pk {
            line.push_str(" PRIMARY KEY");
        } else {
            if let Some(d) = &col.default {
                line.push_str(&format!(" DEFAULT {d}"));
            }
            if col.not_null {
                line.push_str(" NOT NULL");
            }
        }
        lines.push(line);
    }
    if t.pk.len() > 1 {
        let cols =
            t.pk.iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ");
        lines.push(format!("  PRIMARY KEY ({cols})"));
    }
    out.push_str(&lines.join(",\n"));
    out.push_str("\n);\n");
    out
}

/// The drift fingerprint: a content hash of (catalog, dialect, extras). Written
/// into [`_fluessig_meta`](meta_ddl) by every DDL artifact and exposed here so a
/// caller can compare cheaply — `IF NOT EXISTS` alone would silently keep a
/// stale shape (DESIGN §4). Editing the extras trips it too (§9.5).
pub fn fingerprint(c: &Catalog, dialect: Dialect, extras: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(
        serde_json::to_string(c)
            .expect("catalog serializes")
            .as_bytes(),
    );
    h.update(format!("|{dialect:?}|").as_bytes());
    h.update(extras.unwrap_or("").as_bytes());
    let out = h.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// The `_fluessig_meta` DDL: a one-row table recording the fingerprint +
/// versions. `generated_at` defaults at the database so the DDL text stays
/// deterministic. Rebuild flow: compare `fingerprint` to [`fingerprint`]; on
/// mismatch drop everything and re-apply.
fn meta_ddl(c: &Catalog, dialect: Dialect, fp: &str) -> String {
    let now = match dialect {
        Dialect::Postgres | Dialect::Duckdb => "now()",
        Dialect::Sqlite => "CURRENT_TIMESTAMP",
    };
    let ts = match dialect {
        Dialect::Postgres => "timestamptz",
        Dialect::Duckdb => "timestamp",
        Dialect::Sqlite => "text",
    };
    let emitter = c.fluessig.emitter.as_deref().unwrap_or("");
    let compiler = c.fluessig.compiler.as_deref().unwrap_or("");
    format!(
        "CREATE TABLE IF NOT EXISTS \"_fluessig_meta\" (\n  \"fingerprint\" text NOT NULL,\n  \"format\" bigint NOT NULL,\n  \"emitter\" text,\n  \"compiler\" text,\n  \"generated_at\" {ts} DEFAULT {now}\n);\nDELETE FROM \"_fluessig_meta\";\nINSERT INTO \"_fluessig_meta\" (\"fingerprint\", \"format\", \"emitter\", \"compiler\") VALUES ('{fp}', {}, '{emitter}', '{compiler}');\n",
        c.fluessig.format
    )
}

/// The derived-field views (DESIGN §9.3, v1 slice): per entity with `@derived`
/// fields, a `<table>_derived` view of the entity's key + each derived value —
/// `exists`/`count` over one to-many relation, filtered by literal equality on
/// edge properties. Virtual only; nothing is stored or trigger-maintained.
pub fn derived_views(c: &Catalog, dialect: Dialect) -> Vec<String> {
    let mut out = Vec::new();
    for e in c.entities.iter().filter(|e| !e.is_abstract) {
        let fields = c.flattened_fields(e);
        let derived: Vec<&Field> = fields
            .iter()
            .filter(|f| f.derived.is_some())
            .copied()
            .collect();
        if derived.is_empty() {
            continue;
        }
        let table = c.table_name(e);
        let key_cols = key_columns(c, dialect, e);
        let mut selects: Vec<String> = key_cols.iter().map(|(n, _)| format!("t.\"{n}\"")).collect();
        for f in derived {
            let der = f.derived.as_ref().unwrap();
            let rel_field = fields
                .iter()
                .find(|rf| rf.name == der.of)
                .expect("validated");
            let rel = rel_field.relation.as_ref().expect("validated");
            let edge_table = rel.table.clone().unwrap_or_else(|| snake(&rel_field.name));
            // join: edge source columns ↔ this entity's key columns (positional)
            let src_cols = rel
                .source_columns
                .clone()
                .unwrap_or_else(|| key_cols.iter().map(|(n, _)| n.clone()).collect());
            let mut conds: Vec<String> = src_cols
                .iter()
                .zip(key_cols.iter())
                .map(|(s, (k, _))| format!("x.\"{s}\" = t.\"{k}\""))
                .collect();
            if let Some(filter) = &der.filter {
                let props = rel.properties.as_deref().and_then(|p| c.edge_struct(p));
                for (k, v) in filter {
                    let col = props
                        .and_then(|s| s.fields.iter().find(|pf| &pf.name == k))
                        .map(col_name)
                        .unwrap_or_else(|| snake(k));
                    conds.push(format!("x.\"{col}\" = {}", sql_literal(v)));
                }
            }
            let cond = conds.join(" AND ");
            let expr = match der.agg.as_str() {
                "count" => format!("(SELECT count(*) FROM \"{edge_table}\" x WHERE {cond})"),
                _ => format!("EXISTS (SELECT 1 FROM \"{edge_table}\" x WHERE {cond})"),
            };
            selects.push(format!("{expr} AS \"{}\"", col_name(f)));
        }
        let create = match dialect {
            Dialect::Sqlite => "CREATE VIEW IF NOT EXISTS", // sqlite has no OR REPLACE
            _ => "CREATE OR REPLACE VIEW",
        };
        out.push(format!(
            "{create} \"{table}_derived\" AS\nSELECT {}\nFROM \"{table}\" t;\n",
            selects.join(", ")
        ));
    }
    out
}

/// Generate a committed Rust module carrying the schema as code: per-dialect
/// table lists (name, PK, `__table__`-templated DDL) as consts, self-contained
/// (no fluessig dependency at runtime). This is entl's generated-file
/// convention (`tables.gen.ts`, `models.py`) applied to the Rust side — the
/// consumer commits the output and regenerates when the `.tsp` changes; a
/// regenerates-identically test guards drift.
pub fn rust_schema_module(c: &Catalog, banner_note: Option<&str>) -> String {
    let src = c.source.as_deref().unwrap_or("the fluessig catalog");
    let mut out = String::new();
    out.push_str(&format!(
        "//! GENERATED by fluessig from {src} — DO NOT EDIT.\n\
         //!\n\
         //! The schema as code: per-dialect DDL templates (`__table__` is\n\
         //! substituted with the possibly-renamed target name, matching the sink\n\
         //! convention) + primary keys, lowered from the fluessig catalog.\n",
    ));
    // The caller's optional extra banner line (e.g. a lint-suppression marker) —
    // fluessig itself never bakes tool-specific markers into its output.
    if let Some(note) = banner_note {
        out.push_str(&format!("//!\n//! {note}\n"));
    }
    out.push_str(
        "\n\
         /// One table's schema, as code.\n\
         pub struct TableSchema {\n\
         \x20   pub name: &'static str,\n\
         \x20   pub pk: &'static [&'static str],\n\
         \x20   pub ddl: &'static str,\n\
         }\n\n",
    );
    for (dialect, const_name) in [
        (Dialect::Postgres, "PG_TABLES"),
        (Dialect::Sqlite, "SQLITE_TABLES"),
        (Dialect::Duckdb, "DUCKDB_TABLES"),
    ] {
        out.push_str(&format!(
            "/// The {dialect:?} projection of every physical table in the catalog.\n\
             pub const {const_name}: &[TableSchema] = &[\n"
        ));
        for (name, def) in tables(c, dialect) {
            let ddl = render(&def, "__table__");
            out.push_str(&format!(
                "    TableSchema {{ name: {name:?}, pk: &{pk:?}, ddl: r#\"{ddl}\"# }},\n",
                pk = def.pk
            ));
        }
        out.push_str("];\n\n");
    }
    // Every BOOLEAN (table, column) — for coercing SQLite's 0/1 on extract.
    // Derived from the catalog, so it can't silently miss a column (the old
    // hand-kept list did: conflicts.unresolved).
    out.push_str(
        "/// Every BOOLEAN `(table, column)` in the schema — for coercing SQLite's 0/1.\n\
         pub const BOOL_COLUMNS: &[(&str, &str)] = &[\n",
    );
    for (name, def) in tables(c, Dialect::Postgres) {
        for col in &def.columns {
            if col.ty == "boolean" {
                out.push_str(&format!("    ({name:?}, {:?}),\n", col.name));
            }
        }
    }
    out.push_str("];\n");
    crate::rustfmt::format(out)
}

/// The schema reference as data — per physical table: docs + columns (name,
/// dialect type, flags). Consumed by the docs-site generator (a committed
/// artifact, like `schema_gen.rs`), so the site never re-implements the
/// physical lowering. Table docs come from the entity's doc (relation tables:
/// the relation field's doc); column docs from field docs.
pub fn schema_docs_json(c: &Catalog, dialect: Dialect) -> String {
    // table -> its doc (entities + relation fields for association/edge tables)
    let mut table_docs: BTreeMap<String, String> = BTreeMap::new();
    for e in &c.entities {
        if !e.is_abstract {
            if let Some(doc) = &e.doc {
                table_docs.insert(c.table_name(e), doc.clone());
            }
        }
        for f in &e.fields {
            if let (Some(rel), Some(doc)) = (&f.relation, &f.doc) {
                if let Some(tname) = &rel.table {
                    table_docs
                        .entry(tname.clone())
                        .or_insert_with(|| doc.clone());
                }
            }
        }
    }
    let out: Vec<serde_json::Value> = tables(c, dialect)
        .iter()
        .map(|(name, def)| {
            serde_json::json!({
                "name": name,
                "desc": table_docs.get(name),
                "cols": def.columns.iter().map(|col| serde_json::json!({
                    "name": col.name,
                    "type": col.ty,
                    "notNull": col.not_null,
                    "pk": def.pk.contains(&col.name),
                    "def": col.default,
                    "desc": col.doc,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&out).expect("serializes")
}

/// All DDL for a catalog: every table, the derived views, `_fluessig_meta`
/// (stamped with [`fingerprint`]), and — last — the raw `extras` appended
/// verbatim (§9.5; hashed into the fingerprint, so editing extras trips drift
/// detection).
pub fn ddl(c: &Catalog, dialect: Dialect, extras: Option<&str>) -> String {
    let mut parts: Vec<String> = tables(c, dialect)
        .values()
        .map(|t| render(t, &t.name))
        .collect();
    parts.extend(derived_views(c, dialect));
    parts.push(meta_ddl(c, dialect, &fingerprint(c, dialect, extras)));
    if let Some(x) = extras {
        parts.push(x.to_string());
    }
    parts.join("\n")
}
