//! The data plane — Layer C (DESIGN §5, v1 slice): typed change batches in,
//! grouped statements out.
//!
//! A [`Mutation`] is a unit of typed input (one table, one op, n rows); the
//! caller assembles whatever must land together into a [`Transaction`]; the
//! codec compiles one Transaction into one [`Plan`] of atomic [`Step`]s. The
//! v1 SQL posture: fully transactional, so every plan is **one step** whose
//! statements are topologically ordered (referenced-before-referencing for
//! writes, reversed for deletes). fluessig executes nothing — the caller runs
//! each Step inside its own transaction (exactly how entl's driver sink already
//! streams `{sql, params}` to a host).
//!
//! Rows are JSON values in v1 (what entl's Arrow→JSON cell conversion already
//! produces); the zero-copy Arrow `RecordBatch` path arrives with the Arrow
//! front-end (notes/plan.txt Step 2) so the arrow version can be aligned with entl's
//! duckdb re-export once, deliberately.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::ir::{Cardinality, Catalog};
use crate::sql::{tables, Dialect, TableDef};

/// What happened to the rows. The fixed Layer-C vocabulary — `Upsert` is the
/// recommended default for change-stream replay (idempotent under at-least-once
/// delivery); `Insert` fails on key conflict; `Delete` rows carry key columns only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Insert,
    Upsert,
    Delete,
}

/// One unit of typed input: rows for a single physical table.
///
/// Addressed by **table name** (not entity name): edge/association tables
/// (`commit_parents`, …) are first-class write targets in the flat canonical
/// encoding (DESIGN §5 — children/edges arrive as their own flat batches).
#[derive(Debug, Clone)]
pub struct Mutation {
    pub table: String,
    pub op: Op,
    /// The columns present in `rows`, in row order. May be a subset of the
    /// table's columns (absent nullable columns are simply not written).
    pub columns: Vec<String>,
    /// Row-major values, `columns`-ordered. For `Delete`: key columns only.
    pub rows: Vec<Vec<Value>>,
}

/// The unit of atomic INTENT: what must land together.
#[derive(Debug, Clone, Default)]
pub struct Transaction {
    pub mutations: Vec<Mutation>,
}

/// One executable statement (the host runs it verbatim).
#[derive(Debug, Clone)]
pub struct Statement {
    pub sql: String,
    pub params: Vec<Value>,
    /// The canonical table this acts on — for per-table tallies at the host.
    pub table: String,
}

/// One atomic unit for the caller to execute (one `BEGIN…COMMIT`).
#[derive(Debug, Clone)]
pub struct Step {
    pub statements: Vec<Statement>,
}

/// The sink's honest answer to a Transaction's intent. SQL is fully
/// transactional, so v1 plans are always one step.
#[derive(Debug, Clone)]
pub struct Plan {
    pub steps: Vec<Step>,
}

/// The SQL data codec: statement generation + ordering over a validated catalog.
pub struct SqlCodec {
    dialect: Dialect,
    defs: BTreeMap<String, TableDef>,
    /// Topological rank per table (parents < children); cycles broken arbitrarily.
    rank: BTreeMap<String, usize>,
}

impl SqlCodec {
    pub fn new(catalog: &Catalog, dialect: Dialect) -> Self {
        let defs = tables(catalog, dialect);
        let rank = topo_rank(catalog, &defs);
        Self {
            dialect,
            defs,
            rank,
        }
    }

    pub fn table(&self, name: &str) -> Option<&TableDef> {
        self.defs.get(name)
    }

    fn placeholder(&self, i: usize) -> String {
        match self.dialect {
            Dialect::Postgres => format!("${i}"),
            Dialect::Sqlite | Dialect::Duckdb => "?".to_string(),
        }
    }

    /// `INSERT … ON CONFLICT (pk) DO UPDATE SET …` (all three dialects speak this;
    /// no PK ⇒ plain INSERT; nothing beyond the PK ⇒ DO NOTHING).
    pub fn upsert_sql(&self, def: &TableDef, table_name: &str, cols: &[String]) -> String {
        let quoted: Vec<String> = cols.iter().map(|c| format!("\"{c}\"")).collect();
        let ph: Vec<String> = (1..=cols.len()).map(|i| self.placeholder(i)).collect();
        let base = format!(
            "INSERT INTO \"{table_name}\" ({}) VALUES ({})",
            quoted.join(", "),
            ph.join(", ")
        );
        if def.pk.is_empty() {
            return base;
        }
        let conflict: Vec<String> = def.pk.iter().map(|c| format!("\"{c}\"")).collect();
        let non_pk: Vec<&String> = cols.iter().filter(|c| !def.pk.contains(c)).collect();
        if non_pk.is_empty() {
            format!("{base} ON CONFLICT ({}) DO NOTHING", conflict.join(", "))
        } else {
            let set: Vec<String> = non_pk
                .iter()
                .map(|c| format!("\"{c}\" = excluded.\"{c}\""))
                .collect();
            format!(
                "{base} ON CONFLICT ({}) DO UPDATE SET {}",
                conflict.join(", "),
                set.join(", ")
            )
        }
    }

    pub fn insert_sql(&self, table_name: &str, cols: &[String]) -> String {
        let quoted: Vec<String> = cols.iter().map(|c| format!("\"{c}\"")).collect();
        let ph: Vec<String> = (1..=cols.len()).map(|i| self.placeholder(i)).collect();
        format!(
            "INSERT INTO \"{table_name}\" ({}) VALUES ({})",
            quoted.join(", "),
            ph.join(", ")
        )
    }

    /// `DELETE … WHERE pk = …` — delete rows carry key columns only (§5).
    pub fn delete_sql(&self, def: &TableDef, table_name: &str) -> Result<String, String> {
        if def.pk.is_empty() {
            return Err(format!(
                "{table_name}: cannot delete by key — table has no primary key"
            ));
        }
        let cond: Vec<String> = def
            .pk
            .iter()
            .enumerate()
            .map(|(i, c)| format!("\"{c}\" = {}", self.placeholder(i + 1)))
            .collect();
        Ok(format!(
            "DELETE FROM \"{table_name}\" WHERE {}",
            cond.join(" AND ")
        ))
    }

    /// Compile a Transaction into a Plan. v1 SQL: one Step; statements ordered
    /// deletes-first (children before parents) then writes (parents before
    /// children). Strict column checking: a column the table doesn't have is an
    /// error, never a silent drop.
    pub fn plan(&self, tx: &Transaction) -> Result<Plan, String> {
        // validate + partition
        let mut writes: Vec<&Mutation> = Vec::new();
        let mut deletes: Vec<&Mutation> = Vec::new();
        for m in &tx.mutations {
            let def = self
                .defs
                .get(&m.table)
                .ok_or_else(|| format!("unknown table {} (not in the catalog)", m.table))?;
            for c in &m.columns {
                if !def.columns.iter().any(|col| &col.name == c) {
                    return Err(format!("{}: unknown column {c}", m.table));
                }
            }
            if m.op == Op::Delete {
                // delete rows must address the full key
                for k in &def.pk {
                    if !m.columns.contains(k) {
                        return Err(format!(
                            "{}: delete rows must carry key column {k}",
                            m.table
                        ));
                    }
                }
                deletes.push(m);
            } else {
                writes.push(m);
            }
        }
        // order: children-before-parents for deletes, parents-before-children for writes
        let rank = |m: &Mutation| self.rank.get(&m.table).copied().unwrap_or(0);
        deletes.sort_by_key(|m| std::cmp::Reverse(rank(m)));
        writes.sort_by_key(|m| rank(m));

        let mut statements = Vec::new();
        for m in deletes.into_iter().chain(writes) {
            let def = &self.defs[&m.table];
            let sql = match m.op {
                Op::Upsert => self.upsert_sql(def, &m.table, &m.columns),
                Op::Insert => self.insert_sql(&m.table, &m.columns),
                Op::Delete => self.delete_sql(def, &m.table)?,
            };
            for row in &m.rows {
                if row.len() != m.columns.len() {
                    return Err(format!(
                        "{}: row has {} values for {} columns",
                        m.table,
                        row.len(),
                        m.columns.len()
                    ));
                }
                let params = match m.op {
                    // deletes bind the key columns, in PK order
                    Op::Delete => def
                        .pk
                        .iter()
                        .map(|k| {
                            let i = m
                                .columns
                                .iter()
                                .position(|c| c == k)
                                .expect("checked above");
                            row[i].clone()
                        })
                        .collect(),
                    _ => row.clone(),
                };
                statements.push(Statement {
                    sql: sql.clone(),
                    params,
                    table: m.table.clone(),
                });
            }
        }
        Ok(Plan {
            steps: vec![Step { statements }],
        })
    }
}

/// Rank tables so referenced tables sort before referencing ones: entity tables
/// rank after their to-one association targets (self-references ignored);
/// association/edge tables rank after both endpoints (abstract endpoints: after
/// every leaf). Cycles are broken by iteration cap — ordering is best-effort
/// hygiene, not the integrity mechanism (DESIGN §5).
fn topo_rank(c: &Catalog, defs: &BTreeMap<String, TableDef>) -> BTreeMap<String, usize> {
    // physical table -> the tables it references
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let leaves_of = |name: &str| -> Vec<String> {
        match c.entity(name) {
            Some(e) if e.is_abstract => c
                .entities
                .iter()
                .filter(|l| l.extends.as_deref() == Some(&e.name))
                .map(|l| c.table_name(l))
                .collect(),
            Some(e) => vec![c.table_name(e)],
            None => vec![],
        }
    };
    for e in c.entities.iter().filter(|e| !e.is_abstract) {
        let this = c.table_name(e);
        let mut d = Vec::new();
        for f in crate::ir::Catalog::flattened_fields(c, e) {
            let Some(rel) = &f.relation else { continue };
            match rel.cardinality {
                Cardinality::One => {
                    for t in leaves_of(&rel.to) {
                        if t != this {
                            d.push(t);
                        }
                    }
                }
                Cardinality::Many => {
                    // the association/edge table depends on both endpoints
                    let edge = rel
                        .table
                        .clone()
                        .unwrap_or_else(|| crate::ir::snake(&f.name));
                    let mut ends = vec![this.clone()];
                    ends.extend(leaves_of(&rel.to));
                    deps.entry(edge).or_default().extend(ends);
                }
            }
        }
        deps.entry(this).or_default().extend(d);
    }
    // relax ranks (n is tiny; cap breaks cycles)
    let mut rank: BTreeMap<String, usize> = defs.keys().map(|k| (k.clone(), 0)).collect();
    for _ in 0..defs.len() {
        let mut changed = false;
        for (t, ds) in &deps {
            let max_dep = ds
                .iter()
                .filter_map(|d| rank.get(d))
                .copied()
                .max()
                .unwrap_or(0);
            if let Some(r) = rank.get_mut(t) {
                if *r <= max_dep {
                    let next = max_dep + 1;
                    if *r != next {
                        *r = next;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    rank
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_catalog;

    const CATALOG: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog.json"));

    fn codec(d: Dialect) -> SqlCodec {
        SqlCodec::new(&load_catalog(CATALOG).unwrap(), d)
    }

    #[test]
    fn upsert_sql_per_dialect() {
        let pg = codec(Dialect::Postgres);
        let def = pg.table("commits").unwrap().clone();
        let cols = vec!["oid".to_string(), "message".to_string()];
        assert_eq!(
            pg.upsert_sql(&def, "commits", &cols),
            "INSERT INTO \"commits\" (\"oid\", \"message\") VALUES ($1, $2) \
             ON CONFLICT (\"oid\") DO UPDATE SET \"message\" = excluded.\"message\""
        );
        let lite = codec(Dialect::Sqlite);
        let def = lite.table("commit_parents").unwrap().clone();
        let cols = ["commit_oid", "parent_oid", "idx"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            lite.upsert_sql(&def, "commit_parents", &cols),
            "INSERT INTO \"commit_parents\" (\"commit_oid\", \"parent_oid\", \"idx\") VALUES (?, ?, ?) \
             ON CONFLICT (\"commit_oid\", \"idx\") DO UPDATE SET \"parent_oid\" = excluded.\"parent_oid\""
        );
    }

    #[test]
    fn delete_addresses_the_key_only() {
        let pg = codec(Dialect::Postgres);
        let def = pg.table("gh_pull_requests").unwrap().clone();
        assert_eq!(
            pg.delete_sql(&def, "gh_pull_requests").unwrap(),
            "DELETE FROM \"gh_pull_requests\" WHERE \"repo_id\" = $1 AND \"number\" = $2"
        );
    }

    #[test]
    fn plan_orders_parents_before_children_and_reverses_deletes() {
        let pg = codec(Dialect::Postgres);
        let m = |table: &str, op: Op, cols: &[&str]| Mutation {
            table: table.into(),
            op,
            columns: cols.iter().map(|s| s.to_string()).collect(),
            rows: vec![cols.iter().map(|_| Value::from(1)).collect()],
        };
        // scrambled: edge first, then the entity it references
        let tx = Transaction {
            mutations: vec![
                m(
                    "commit_parents",
                    Op::Upsert,
                    &["commit_oid", "parent_oid", "idx"],
                ),
                m(
                    "commits",
                    Op::Upsert,
                    &[
                        "oid",
                        "repo_id",
                        "tree_oid",
                        "message",
                        "summary",
                        "parent_count",
                        "is_merge",
                        "gpg_signed",
                    ],
                ),
                m("repos", Op::Upsert, &["id", "path"]),
            ],
        };
        let plan = pg.plan(&tx).unwrap();
        assert_eq!(plan.steps.len(), 1, "SQL is one-step-transactional");
        let order: Vec<&str> = plan.steps[0]
            .statements
            .iter()
            .map(|s| s.table.as_str())
            .collect();
        assert_eq!(order, ["repos", "commits", "commit_parents"]);

        // deletes reverse: children first
        let tx = Transaction {
            mutations: vec![
                m("commits", Op::Delete, &["oid"]),
                m("commit_parents", Op::Delete, &["commit_oid", "idx"]),
            ],
        };
        let order: Vec<String> = pg.plan(&tx).unwrap().steps[0]
            .statements
            .iter()
            .map(|s| s.table.clone())
            .collect();
        assert_eq!(order, ["commit_parents", "commits"]);
    }

    #[test]
    fn strict_column_and_key_checking() {
        let pg = codec(Dialect::Postgres);
        let bad = Transaction {
            mutations: vec![Mutation {
                table: "commits".into(),
                op: Op::Upsert,
                columns: vec!["oid".into(), "ghost".into()],
                rows: vec![vec![Value::from("a"), Value::from("b")]],
            }],
        };
        assert!(pg.plan(&bad).unwrap_err().contains("unknown column ghost"));

        let bad = Transaction {
            mutations: vec![Mutation {
                table: "commits".into(),
                op: Op::Delete,
                columns: vec!["message".into()], // not the key
                rows: vec![vec![Value::from("x")]],
            }],
        };
        assert!(pg.plan(&bad).unwrap_err().contains("key column oid"));
    }
}
