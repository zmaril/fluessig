//! Slice 5 demo: the **op surface**, authored as the `impl` block that actually
//! runs. `#[fluessig::export]` on the impl captures each method's shape (name,
//! params, return, op kind) into an op interface; `catalog!`'s `api:` root list
//! lowers those into `api.json` — the same file the loader validates and bindgen
//! projects into per-language glue. The impl IS the interface, so
//! declaration/implementation drift is impossible (`derive-front-end.md` §2.7).
//!
//! The ops here exercise all four op kinds over entities from the earlier demos
//! (`graph::{Repo, PullRequest}`):
//!
//! * `#[fluessig(ctor)]` — a constructor (`void` on the op surface);
//! * a unary method returning an entity / `Option<entity>` — `#[fluessig(async)]`
//!   here, since these ops are IO-bound (the async `AsyncTask`/`Promise` shape);
//!   a plain unary op with no marker is synchronous (see the `native` demo);
//! * `#[fluessig(stream)]` — returns `impl Iterator<Item = entity>` (bindgen maps
//!   it to a JS async iterator / Python generator / Ruby Enumerator);
//! * `#[fluessig(manual)]` — recorded in `api.json` but hand-written per binding.
//!
//! It also exercises the op-level FLAGS that compose with any kind — here
//! `#[fluessig(worker)]` on the `repo` lookup (`"worker": true` → the MCP
//! `workerHint`), the sibling of the `readonly` / `destructive` flags.
//!
//! Slice 8a Gap 2 adds the **DTO / `models` layer**: `#[derive(Record)]` declares
//! the flat data DTOs the ops pass across (`LoadStats`, `SinkOptions`,
//! `TableRename`), and the entities/DTOs the ops reference — directly or
//! transitively — are materialised into `api.json`'s `models`, flattened exactly
//! as the TypeSpec op path does (a to-one relation → its FK field(s); to-many
//! omitted; the referenced set closed transitively).
//!
//! Compare against the TypeSpec an author would write for the same surface
//! (`api.tsp`): `interface Db { @ctor open(...): void; … @stream commits(): PullRequest; }`
//! plus the referenced entity/DTO `models`. The two `api.json`s are now
//! semantically equivalent at BOTH the op level (kinds, params, returns) and the
//! model level (flattened DTO fields) — see `tests/api_typespec_equivalence.rs`.

use fluessig_derive::{catalog, export, Record};

use crate::graph::{GhUser, PullRequest, Repo, Review};

// ── DTOs / value structs the ops pass across (Slice 8a Gap 2) ────────────────
//
// `#[derive(Record)]` declares a DTO: flat data, no identity/table/key. An op
// that takes or returns one — directly, or transitively through another DTO's
// fields — pulls it into `api.json`'s `models`, flattened exactly as the
// TypeSpec op path materialises the DTOs its ops reference.

/// What one load produced — a plain scalar DTO returned by `Db::load`.
#[derive(Record)]
pub struct LoadStats {
    /// New commits ingested.
    pub commits: i64,
    /// Refs seen.
    pub refs: i64,
}

/// One table rename — a DTO referenced *transitively*, only through
/// `SinkOptions.renames`, so it proves the referenced-model closure grows past
/// the ops' direct references.
#[derive(Record)]
pub struct TableRename {
    /// The source table name.
    pub old_name: String,
    /// The destination table name.
    pub new_name: String,
}

/// Options for `Db::sink` — a DTO with a nullable scalar and a **list of another
/// DTO** (`renames`), exercising the list + transitive-closure paths.
#[derive(Record)]
pub struct SinkOptions {
    /// The destination path (the SQLite file / JSONL dir / Postgres URL).
    pub path: Option<String>,
    /// Per-table renames applied on the way out.
    pub renames: Vec<TableRename>,
}

/// A tiny stateful handle whose `impl` is the op interface. The engine behind it
/// is irrelevant to the schema — only the method shapes are captured.
pub struct Db {
    // the hand-implemented core; the derive reads only the signatures.
    _private: (),
}

/// An open demo database. Heavy ops are unary; the change feed is a stream.
#[export]
impl Db {
    /// Open (or create) the store at `path` and apply the schema.
    #[fluessig(ctor)]
    pub fn open(path: &str) -> Self {
        let _ = path;
        Db { _private: () }
    }

    /// Look up one repo by id — `None` when absent. An observe-only lookup, so
    /// it's safe on a worker-role MCP surface (`#[fluessig(worker)]`).
    #[fluessig(async)]
    #[fluessig(worker)]
    pub fn repo(&self, id: &str) -> Option<Repo> {
        let _ = id;
        None
    }

    /// Count the pull requests in a repo.
    #[fluessig(async)]
    pub fn pull_request_count(&self, repo_id: &str) -> i64 {
        let _ = repo_id;
        0
    }

    /// The repo ids known to the store.
    #[fluessig(async)]
    pub fn repos(&self, limit: Option<i32>) -> Vec<String> {
        let _ = limit;
        Vec::new()
    }

    /// Stream every pull request in a repo (the stream plane). bindgen maps the
    /// `Iterator` to a JS async iterator / Python generator / Ruby Enumerator.
    #[fluessig(stream)]
    pub fn pull_requests(&self, repo_id: &str) -> impl Iterator<Item = PullRequest> {
        let _ = repo_id;
        std::iter::empty()
    }

    /// Poll-loop sync with a host callback — hand-written per binding, exactly as
    /// before; recorded in `api.json` but not auto-bound.
    #[fluessig(manual)]
    pub fn watch(&self, interval_secs: i32) {
        let _ = interval_secs;
    }

    /// Load and report a stats DTO (a `#[derive(Record)]` return, Slice 8a Gap 2).
    #[fluessig(async)]
    pub fn load(&self) -> LoadStats {
        LoadStats {
            commits: 0,
            refs: 0,
        }
    }

    /// Sink to a target described by a DTO param — `SinkOptions` (and, through it,
    /// `TableRename`) materialise into `api.json`'s `models`.
    #[fluessig(async)]
    pub fn sink(&self, options: SinkOptions) -> i64 {
        let _ = options;
        0
    }
}

/// A stateless, repo-scoped helper group — a unit struct whose `#[export] impl`
/// carries only associated (no-`self`) unary ops. This is the derive spelling of
/// the sketch's stateless `mod git` (`entl_derive_sketch.rs`); an impl on a unit
/// struct keeps every op root a *type* (uniform `api:` list, no module special
/// case). All ops are unary.
pub struct GitHelpers;

/// Stateless git helpers (no database handle). All unary.
#[export]
impl GitHelpers {
    /// Whether `name` is a branch in the repo at `repo_path`.
    #[fluessig(async)]
    pub fn branch_exists(repo_path: &str, name: &str) -> bool {
        let _ = (repo_path, name);
        false
    }

    /// The current branch name at `repo_path`.
    #[fluessig(async)]
    pub fn current_branch(repo_path: &str) -> String {
        let _ = repo_path;
        String::new()
    }
}

// The exporter half: the entity catalog roots + the `api:` op roots. `to_json()`
// prints `catalog.json`; `api_to_json()` prints `api.json` (Slice 5).
catalog! {
    name: "api_demo",
    version: "0.1.0",
    entities: [Repo, PullRequest, GhUser, Review],
    records: [LoadStats, TableRename, SinkOptions],
    api: [Db, GitHelpers],
}
