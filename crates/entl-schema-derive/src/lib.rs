// straitjacket-allow-file:duplication — a 28-table schema catalog is inherently
// repetitive: the Actions tables (gh_workflow_runs / gh_jobs / gh_steps /
// gh_check_runs / gh_commit_statuses) share status / conclusion / timestamp field
// blocks BY DESIGN, faithfully to entl.tsp. These are DISTINCT entities with
// distinct keys and relations, not a copy that wants a helper.
//! entl-schema-derive — **THE ACID TEST** (Slice 8b): entl's COMPLETE catalog
//! (all 28 tables + the full binding op surface) authored with the `fluessig`
//! Rust derive front end, ADDITIVELY alongside the TypeSpec `entl.tsp` it mirrors,
//! to prove the derive front end can carry a real production schema.
//!
//! This is the Rust-derive re-run of the plan.txt acid test ("write the full
//! entl.tsp before freezing the IR"). Every model here is line-for-line parallel
//! to `../../entl.tsp`; the parity test (`tests/parity.rs`) asserts the
//! derive-emitted `catalog.json` / `api.json` project to the SAME physical tables,
//! enums, and scalars and the SAME ops + models as entl's committed
//! TypeSpec-emitted `catalog.json` / `api.json`.
//!
//! What the derive front end needed to grow to carry entl (Slice 8b):
//!
//! * **`#[derive(Enum)]`** — entl's six enums (`RefKind`, `FileStatus`, `PrState`,
//!   `IssueState`, `Mergeable`, `SinkTarget`); a field typed by one lowers to
//!   `TypeRef::Enum`, and (the Gap-2-flagged case) an enum-typed DTO field lowers
//!   to `{ enum }` in `api.json`'s models.
//! * **`#[derive(Scalar)]`** — entl's semantic scalars `Oid` (base `bytes`) and
//!   `ArrowBatch`; plus the stock scalars `utcDateTime` (from `DateTime<Utc>`),
//!   `bytes` (from `Vec<u8>`), and `Json`.
//! * **`#[fluessig(default = …)]`** — DDL defaults (`parent_count = 0`,
//!   `is_merge = false`, …).
//! * **`#[fluessig(derived(exists, of = "parents", filter(idx = 1)))]`** — the
//!   `commits.is_merge` derived field (DESIGN §9.3).
//! * **polymorphic edges** — a `<Root>Id`-typed edge field whose family is the
//!   edge's `from` is the source discriminator (`gh_labeled.subject: GhSubjectId`),
//!   whose family is the `to` is the target discriminator
//!   (`tree_entries.child: GitObjectId`).

use fluessig_derive::{catalog, export, AbstractRoot, Edge, Entity, Enum, Id, Record, Scalar};

// ═════════════════════════════════════════════════════════════════════════════
// Stock-type markers — zero-dep stand-ins the derive maps to built-in scalars.
// The derive front end reads *types* (tokens), never values, so a marker suffices;
// this keeps the acid-test crate dependency-free while `entl.tsp` spells the same
// types as `utcDateTime` / `bytes` / `Json`.
// ═════════════════════════════════════════════════════════════════════════════

/// Stand-in for `chrono::DateTime<Utc>` — the derive maps `DateTime<_>` to the
/// `utcDateTime` scalar.
pub struct DateTime<Tz>(core::marker::PhantomData<Tz>);
/// The `Utc` timezone marker (only its name matters to the derive).
pub struct Utc;
/// The stock `Json` scalar (base `string`) — `GhEvent.payload`, `Statement.params`.
pub struct Json;

// ═════════════════════════════════════════════════════════════════════════════
// Scalars & enums
// ═════════════════════════════════════════════════════════════════════════════

/// a git object id (stored as raw bytes; hex is a representation view — DESIGN §9.1)
#[derive(Scalar, Clone, Debug, PartialEq, Eq, Hash)]
#[fluessig(extends = "bytes")]
pub struct Oid(pub Vec<u8>);

/// one Arrow RecordBatch, held columnar and surfaced per language: an IPC-bytes
/// getter everywhere, plus the Arrow PyCapsule protocol (zero-copy C Data
/// Interface) in Python. (Opaque `scalar ArrowBatch;` in `.tsp`.)
#[derive(Scalar)]
pub struct ArrowBatch;

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "lowercase")]
pub enum RefKind {
    Branch,
    Tag,
    Remote,
    Head,
}

/// file_changes.status / FileDiff.status — git status codes; stored as the single letters
#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "lowercase")]
pub enum FileStatus {
    #[fluessig(value = "A")]
    Added,
    #[fluessig(value = "M")]
    Modified,
    #[fluessig(value = "D")]
    Deleted,
    #[fluessig(value = "R")]
    Renamed,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IssueState {
    Open,
    Closed,
}

/// GitHub computes mergeability lazily
#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Mergeable {
    Mergeable,
    Conflicting,
    Unknown,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "lowercase")]
pub enum SinkTarget {
    Sqlite,
    Jsonl,
    Postgres,
}

// ═════════════════════════════════════════════════════════════════════════════
// Git side
// ═════════════════════════════════════════════════════════════════════════════

/// A repository entl has synced.
#[derive(Entity)]
#[fluessig(name = "repos")]
pub struct Repo {
    /// path hash
    #[key]
    pub id: String,
    pub path: String,
    pub remote_url: Option<String>,
    pub host: Option<String>,
    pub owner: Option<String>,
    pub name: Option<String>,
    /// from the forge (`repos/{o}/{r}`), when a GitHub sync has run
    pub default_branch: Option<String>,
    /// the repo's homepage URL from the forge, when set
    pub homepage_url: Option<String>,
    pub first_synced_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
}

/// A git note (`refs/notes/*`): free-form text attached to any object after the
/// fact, without rewriting it. Mutable per-repo state like refs — each sync
/// replaces the repo's rows.
#[derive(Entity)]
#[fluessig(name = "git_notes")]
pub struct GitNote {
    #[key]
    pub repo_id: Id<Repo>,
    /// full notes ref name, e.g. `refs/notes/commits`
    #[key]
    pub notes_ref: String,
    /// the object the note annotates (usually a commit)
    #[key]
    pub annotated_oid: Oid,
    pub note: String,
}

/// The git object family — the polymorphic root (DESIGN §2 Layer B). A ref or a
/// tree entry may point at ANY object; the family guarantees one key type (Oid)
/// so a polymorphic reference is the typeable pair (type, oid). Concrete leaves:
/// Commit, Tree, Blob.
///
/// No family-level `tag_col` / `ref_col` here: unlike GhSubject, GitObject's
/// reference spelling varies per site (entry_type/child_oid on tree entries), so
/// sites pin their own column names.
#[derive(AbstractRoot)]
#[fluessig(abstract_root(Commit, Tree, Blob))]
pub struct GitObject {
    /// object id — the content hash (binary; hex via `lower(hex(oid))`).
    /// Globally unique, so the family key is oid alone.
    #[key]
    pub oid: Oid,
    /// the repo this object was seen in (an association, not a key member)
    pub repo_id: Id<Repo>,
}

/// One row per commit, walked from every ref. Author and committer time/identity
/// are kept separate; `summary` is the first line of `message`.
#[derive(Entity)]
#[fluessig(name = "commits", extends = GitObject)]
pub struct Commit {
    /// root tree this commit points at
    pub tree_oid: Id<Tree>,
    /// full commit message
    pub message: String,
    /// first line of the message
    pub summary: String,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub author_when: Option<DateTime<Utc>>,
    pub author_tz: Option<String>,
    pub committer_name: Option<String>,
    pub committer_email: Option<String>,
    pub committer_when: Option<DateTime<Utc>>,
    pub committer_tz: Option<String>,
    #[fluessig(default = 0)]
    pub parent_count: i32,
    /// The §9.3 v1 carve-out: a REAL derived field — true iff a second parent
    /// exists. Resolved and checked at catalog load.
    #[fluessig(default = false, derived(exists, of = "parents", filter(idx = 1)))]
    pub is_merge: bool,
    #[fluessig(default = false)]
    pub gpg_signed: bool,
}

/// this commit's parents, ordered by `idx` (merges have several)
#[derive(Edge)]
#[fluessig(name = "commit_parents", edge(from = Commit, to = Commit, expose = "parents"))]
pub struct CommitParent {
    pub commit_oid: Id<Commit>,
    pub parent_oid: Id<Commit>,
    /// @key on an edge field = local key; edge PK = source key + local key (FINDINGS #3)
    #[fluessig(key)]
    pub idx: i32,
}

#[derive(Entity)]
#[fluessig(name = "trees", extends = GitObject)]
pub struct Tree {}

/// The polymorphic edge already in production: tree_entries stores the
/// (type, key) pair as (entry_type, child_oid). Exposed as `Tree::entries`.
#[derive(Edge)]
#[fluessig(name = "tree_entries", edge(from = Tree, to = GitObject, expose = "entries"))]
pub struct TreeEntry {
    pub tree_oid: Id<Tree>,
    /// PK = (tree_oid, name)
    #[fluessig(key)]
    pub name: String,
    pub path: String,
    /// 100644 / 100755 / 120000 / 040000 / 160000
    pub mode: String,
    /// The object this entry points at — a polymorphic family reference spelled
    /// `(entry_type, child_oid)` (a legacy per-site override; the family default
    /// would be its own spelling).
    #[fluessig(cols(tag = "entry_type", key = "child_oid"))]
    pub child: GitObjectId,
}

#[derive(Entity)]
#[fluessig(name = "blobs", extends = GitObject)]
pub struct Blob {
    pub size: i64,
    #[fluessig(default = false)]
    pub is_binary: bool,
    pub content_text: Option<String>,
    pub content_sha: Option<String>,
    /// raw file bytes (object ingest / --objects); enables `entl rebuild`
    pub content: Option<Vec<u8>>,
}

/// Per-commit file changes — a WEAK ENTITY: identified by (owner, path), no
/// global id of its own. FK-in-PK falls out naturally.
#[derive(Entity)]
#[fluessig(name = "file_changes")]
pub struct FileChange {
    #[key]
    pub commit_oid: Id<Commit>,
    #[key]
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub additions: Option<i32>,
    pub deletions: Option<i32>,
    pub blob_oid: Option<Id<Blob>>,
    pub old_blob_oid: Option<Id<Blob>>,
}

/// Branches, tags, remote-tracking refs, and HEAD — one row each.
#[derive(Entity)]
#[fluessig(name = "refs")]
pub struct Ref {
    #[key]
    pub repo_id: Id<Repo>,
    /// short ref name, e.g. `main` or `origin/main`
    #[key]
    pub name: String,
    pub kind: RefKind,
    /// Today: implicitly commit-typed (no discriminator column). Authored as
    /// Id<Commit> for byte parity; the honest family-typed upgrade is FINDINGS #7.
    pub target_oid: Id<Commit>,
    #[fluessig(default = false)]
    pub is_symbolic: bool,
    pub upstream: Option<String>,
}

/// Merge-conflict hot zones (the north star).
#[derive(Entity)]
#[fluessig(name = "conflicts")]
pub struct Conflict {
    #[key]
    pub repo_id: Id<Repo>,
    /// hex text today, while every other oid column is blob — schema
    /// inconsistency, FINDINGS #8.
    #[key]
    pub merge_oid: String,
    #[key]
    pub path: String,
    pub unresolved: bool,
}

/// Per-resource sync bookkeeping (etags, cursors, watermarks).
#[derive(Entity)]
#[fluessig(name = "sync_state")]
pub struct SyncState {
    #[key]
    pub resource: String,
    pub cursor: Option<String>,
    pub etag: Option<String>,
    pub watermark: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

// ═════════════════════════════════════════════════════════════════════════════
// Forge side (GitHub)
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Entity)]
#[fluessig(name = "gh_users")]
pub struct GhUser {
    #[key]
    pub id: i64,
    pub login: String,
    /// `type` is a Rust keyword too — r# instead of .tsp's backticks
    pub r#type: Option<String>,
    pub name: Option<String>,
}

/// The PR/Issue family — the SECOND polymorphic root already in production. Root
/// carries ONLY the key (FINDINGS #6). Every referencing site spells the columns
/// identically (subject_type / repo_id / subject_number), declared once here.
#[derive(AbstractRoot)]
#[fluessig(
    abstract_root(GhPullRequest, GhIssue),
    tag_col = "subject_type",
    ref_cols(number = "subject_number")
)]
pub struct GhSubject {
    #[key]
    pub repo_id: Id<Repo>,
    #[key]
    pub number: i32,
}

/// Pull requests and their lifecycle. Four tables reference this PR as
/// (repo_id, pr_number), so the spelling is declared here once.
#[derive(Entity)]
#[fluessig(
    name = "gh_pull_requests",
    extends = GhSubject,
    ref_cols(repo_id = "repo_id", number = "pr_number")
)]
pub struct GhPullRequest {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: PrState,
    pub author_id: Option<Id<GhUser>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
    pub merge_commit_oid: Option<Id<Commit>>,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub additions: Option<i32>,
    pub deletions: Option<i32>,
    pub changed_files: Option<i32>,
    #[fluessig(default = false)]
    pub is_draft: bool,
    pub mergeable: Option<Mergeable>,
    /// head commit CI rollup: SUCCESS | FAILURE | PENDING | … (open set — stays text)
    pub checks: Option<String>,
    /// PR head commit (for base...head diffs)
    pub head_oid: Option<Id<Commit>>,
    /// base branch tip at fetch time
    pub base_oid: Option<Id<Commit>>,
}

#[derive(Entity)]
#[fluessig(name = "gh_issues", extends = GhSubject)]
pub struct GhIssue {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: IssueState,
    pub author_id: Option<Id<GhUser>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// gh_labeled — a join off the polymorphic GhSubject family: the SOURCE side is
/// the (type, key) pair (repo_id, subject_type, subject_number).
#[derive(Edge)]
#[fluessig(name = "gh_labeled", edge(from = GhSubject, to = GhLabel, expose = "labels"))]
pub struct GhLabeled {
    /// → subject_type, repo_id, subject_number (spelling from the family)
    pub subject: GhSubjectId,
    /// GhLabel's key is (repo_id, name); `shares(repo_id)` folds the first FK
    /// component onto the subject's repo_id column (a real same-repo constraint),
    /// and the second takes GhLabel's `label_name` reference spelling.
    #[fluessig(key, shares(repo_id))]
    pub label: Id<GhLabel>,
}

/// gh_assignees has DDL but no writer today — modeled faithfully.
#[derive(Edge)]
#[fluessig(name = "gh_assignees", edge(from = GhSubject, to = GhUser, expose = "assignees"))]
pub struct GhAssignee {
    pub subject: GhSubjectId,
    pub user_id: Id<GhUser>,
}

/// plain join table (PK = all columns): the PR's commits
#[derive(Edge)]
#[fluessig(name = "gh_pr_commits", edge(from = GhPullRequest, to = Commit, expose = "commits"))]
pub struct GhPrCommit {
    pub pr: Id<GhPullRequest>,
    pub commit_oid: Id<Commit>,
}

/// plain join table: requested reviewers
#[derive(Edge)]
#[fluessig(
    name = "gh_requested_reviewers",
    edge(from = GhPullRequest, to = GhUser, expose = "requested_reviewers")
)]
pub struct GhRequestedReviewer {
    pub pr: Id<GhPullRequest>,
    pub user_id: Id<GhUser>,
}

/// Issue/PR comments — subject is a polymorphic reference into the GhSubject family.
#[derive(Entity)]
#[fluessig(name = "gh_comments")]
pub struct GhComment {
    #[key]
    pub id: i64,
    /// → subject_type, repo_id, subject_number
    pub subject: GhSubjectId,
    pub author_id: Option<Id<GhUser>>,
    pub body: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Entity)]
#[fluessig(name = "gh_labels", ref_cols(name = "label_name"))]
pub struct GhLabel {
    #[key]
    pub repo_id: Id<Repo>,
    #[key]
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// PR reviews — globally id-keyed, referencing the PR by its composite key.
#[derive(Entity)]
#[fluessig(name = "gh_pr_reviews")]
pub struct GhPrReview {
    #[key]
    pub id: i64,
    /// multi-column FK to a composite-keyed entity — (repo_id, pr_number)
    pub pr: Id<GhPullRequest>,
    pub reviewer_id: Option<Id<GhUser>>,
    pub state: Option<String>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub body: Option<String>,
}

#[derive(Entity)]
#[fluessig(name = "gh_review_comments")]
pub struct GhReviewComment {
    #[key]
    pub id: i64,
    pub pr: Id<GhPullRequest>,
    pub path: Option<String>,
    pub line: Option<i32>,
    pub side: Option<String>,
    pub commit_oid: Option<Id<Commit>>,
    pub author_id: Option<Id<GhUser>>,
    pub body: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    /// threaded replies — a nullable self-association; field name = column name
    pub in_reply_to: Option<Id<GhReviewComment>>,
}

/// Raw GitHub event stream (the /events feed).
#[derive(Entity)]
#[fluessig(name = "gh_events")]
pub struct GhEvent {
    #[key]
    pub repo_id: Id<Repo>,
    #[key]
    pub id: String,
    pub r#type: Option<String>,
    /// denormalized pair: the association + a plain login column (FINDINGS #9)
    pub actor_id: Option<Id<GhUser>>,
    pub actor_login: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    /// the event's type-specific JSON
    pub payload: Option<Json>,
}

// ── Actions / Checks ──

#[derive(Entity)]
#[fluessig(name = "gh_workflows")]
pub struct GhWorkflow {
    #[key]
    pub id: i64,
    pub repo_id: Id<Repo>,
    pub name: Option<String>,
    pub path: Option<String>,
    pub state: Option<String>,
}

#[derive(Entity)]
#[fluessig(name = "gh_workflow_runs")]
pub struct GhWorkflowRun {
    #[key]
    pub id: i64,
    pub repo_id: Id<Repo>,
    pub workflow_id: Option<Id<GhWorkflow>>,
    pub head_oid: Option<Id<Commit>>,
    pub head_branch: Option<String>,
    pub event: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub run_number: Option<i32>,
    pub run_attempt: Option<i32>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub run_started_at: Option<DateTime<Utc>>,
}

#[derive(Entity)]
#[fluessig(name = "gh_jobs")]
pub struct GhJob {
    #[key]
    pub id: i64,
    pub run_id: Id<GhWorkflowRun>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub runner_name: Option<String>,
}

/// Weak entity again: a step is (job, number), no global id.
#[derive(Entity)]
#[fluessig(name = "gh_steps")]
pub struct GhStep {
    #[key]
    pub job_id: Id<GhJob>,
    #[key]
    pub number: i32,
    pub name: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Entity)]
#[fluessig(name = "gh_check_runs")]
pub struct GhCheckRun {
    #[key]
    pub id: i64,
    pub repo_id: Id<Repo>,
    pub commit_oid: Option<Id<Commit>>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Entity)]
#[fluessig(name = "gh_commit_statuses")]
pub struct GhCommitStatus {
    #[key]
    pub id: i64,
    pub repo_id: Id<Repo>,
    pub commit_oid: Id<Commit>,
    pub context: Option<String>,
    pub state: Option<String>,
    pub description: Option<String>,
    pub target_url: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

// ═════════════════════════════════════════════════════════════════════════════
// The API surface — op shapes → api.json → generated bindings
// ═════════════════════════════════════════════════════════════════════════════

/// What one git load produced.
#[derive(Record)]
pub struct GitStats {
    pub new_commits: i64,
    pub file_changes: i64,
    pub refs: i64,
}

/// What one GitHub load produced.
#[derive(Record)]
pub struct GithubStats {
    pub events: i64,
    pub pull_requests: i64,
    pub reviews: i64,
    pub review_comments: i64,
    pub issues: i64,
    pub comments: i64,
    pub workflow_runs: i64,
    pub check_runs: i64,
    pub users: i64,
}

/// What a sink() cycle produced: git + forge counts, and rows applied to the target.
#[derive(Record)]
pub struct SinkStats {
    pub new_commits: i64,
    pub file_changes: i64,
    pub refs: i64,
    pub pull_requests: i64,
    pub issues: i64,
    pub events: i64,
    pub workflow_runs: i64,
    pub check_runs: i64,
    pub rows: i64,
}

/// One changed file between two commits.
#[derive(Record)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub additions: i64,
    pub deletions: i64,
    /// full single-file unified patch; empty for binary files (matching GitHub)
    pub patch: String,
}

/// One change-stream batch. The rows stay columnar (Arrow): `ipc` yields them as
/// one Arrow IPC stream.
#[derive(Record)]
pub struct ChangeBatch {
    pub table: String,
    pub op: String,
    pub ipc: ArrowBatch,
}

/// One driver-plan statement: the host runs it verbatim against its own client.
#[derive(Record)]
pub struct Statement {
    pub sql: String,
    pub params: Json,
    /// canonical source table (null for cross-table DDL) — for per-table tallies
    pub table: Option<String>,
}

#[derive(Record)]
pub struct TableRename {
    pub from: String,
    pub to: String,
}

#[derive(Record)]
pub struct SinkOptions {
    pub target: SinkTarget,
    /// the SQLite file, the JSONL directory, or the Postgres URL
    pub path: Option<String>,
    /// also pull GitHub (default true; needs a token)
    pub github: Option<bool>,
    /// only write these tables (default: all)
    pub tables: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub rename: Option<Vec<TableRename>>,
    /// target schema (Postgres only; default "entl")
    pub schema: Option<String>,
    /// also store the object graph (trees/blobs + raw content) so the store can rebuild the repo
    pub objects: Option<bool>,
}

#[derive(Record)]
pub struct ExtractOptions {
    /// duckdb | sqlite | jsonl | postgres
    pub source: String,
    pub path: String,
    pub tables: Option<Vec<String>>,
    pub schema: Option<String>,
}

#[derive(Record)]
pub struct RebuildOptions {
    /// duckdb | sqlite | jsonl | postgres.
    pub source: String,
    pub dest: String,
    /// output directory for the reconstructed repo
    pub out: String,
    pub schema: Option<String>,
}

#[derive(Record)]
pub struct ChangesOptions {
    pub github: Option<bool>,
    pub objects: Option<bool>,
}

#[derive(Record)]
pub struct DriverPlanOptions {
    pub tables: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub rename: Option<Vec<TableRename>>,
    pub schema: Option<String>,
}

/// Stateless, repo-scoped git helpers (no database handle). All unary. A unit
/// struct keeps every op root a *type* (the sketch's `mod git`, in the derive
/// spelling the front end supports).
pub struct Git;

/// Stateless git helpers (no database handle). All unary.
#[export]
impl Git {
    /// Diff two commits (`base...head` when threeDot).
    #[fluessig(async)]
    pub fn diff_commits(repo_path: &str, base: &str, head: &str, three_dot: bool) -> Vec<FileDiff> {
        let _ = (repo_path, base, head, three_dot);
        Vec::new()
    }

    /// A file's content at a commit — None when absent or binary.
    #[fluessig(async)]
    pub fn file_at(repo_path: &str, commit: &str, path: &str) -> Option<String> {
        let _ = (repo_path, commit, path);
        None
    }

    #[fluessig(async)]
    pub fn branch_exists(repo_path: &str, name: &str) -> bool {
        let _ = (repo_path, name);
        false
    }

    #[fluessig(async)]
    pub fn current_branch(repo_path: &str) -> String {
        let _ = repo_path;
        String::new()
    }

    /// Commit subjects+bodies along a branch (JSON).
    #[fluessig(async)]
    pub fn commit_bodies(repo_path: &str, branch: &str) -> String {
        let _ = (repo_path, branch);
        String::new()
    }

    /// Remote branch names matching a pattern (trailing-`*` glob). Fetches first.
    #[fluessig(async)]
    pub fn ls_remote_heads(repo_path: &str, pattern: &str) -> Vec<String> {
        let _ = (repo_path, pattern);
        Vec::new()
    }
}

/// An open entl database. Heavy ops are unary (off-thread in async hosts).
pub struct Entl {
    _private: (),
}

#[export]
impl Entl {
    /// Open (or create) the .duckdb at `dbPath` and apply the schema.
    #[fluessig(ctor)]
    pub fn open(db_path: &str) -> Self {
        let _ = db_path;
        Entl { _private: () }
    }

    /// Load git history from `repoPath` (one-way, incremental).
    #[fluessig(async)]
    pub fn load_git(&self, repo_path: &str) -> GitStats {
        let _ = repo_path;
        unimplemented!()
    }

    /// Load GitHub data (events/PRs/issues/Actions). Needs a token.
    #[fluessig(async)]
    pub fn load_github(&self, repo_path: &str) -> GithubStats {
        let _ = repo_path;
        unimplemented!()
    }

    /// Run a SQL query; JSON rows back.
    #[fluessig(async)]
    pub fn query(&self, sql: &str) -> String {
        let _ = sql;
        String::new()
    }

    /// Run a SQL query; the result as one Arrow IPC stream (the dataframe on-ramp).
    #[fluessig(async)]
    pub fn query_arrow(&self, sql: &str) -> Vec<u8> {
        let _ = sql;
        Vec::new()
    }

    /// Pull `repoPath` and sync it into a target store, in one call.
    #[fluessig(async)]
    pub fn sink(&self, repo_path: &str, options: SinkOptions) -> SinkStats {
        let _ = (repo_path, options);
        unimplemented!()
    }

    /// Read a store back into canonical rows (JSON; oids hex, timestamps RFC3339).
    #[fluessig(async)]
    pub fn extract(&self, options: ExtractOptions) -> String {
        let _ = options;
        String::new()
    }

    /// Stream the change batches from one pull (the stream plane).
    #[fluessig(stream)]
    pub fn changes(
        &self,
        repo_path: &str,
        options: Option<ChangesOptions>,
    ) -> impl Iterator<Item = ChangeBatch> {
        let _ = (repo_path, options);
        std::iter::empty()
    }

    /// Backfill this store into a driver target: stream {sql, params} for the host to execute.
    #[fluessig(stream)]
    pub fn driver_plan(
        &self,
        options: Option<DriverPlanOptions>,
    ) -> impl Iterator<Item = Statement> {
        let _ = options;
        std::iter::empty()
    }

    /// Reconstruct a git repo from a store (needs objects: true at sink time). Returns commits rebuilt.
    #[fluessig(async)]
    pub fn rebuild(&self, options: RebuildOptions) -> i64 {
        let _ = options;
        0
    }

    /// Poll-loop sync with a host callback — hand-written per binding.
    #[fluessig(manual)]
    pub fn watch(&self, repo_path: &str, interval_secs: i32) {
        let _ = (repo_path, interval_secs);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// The exporter — replaces `(cd emitter && node emit.mjs ../entl.tsp)`.
// ═════════════════════════════════════════════════════════════════════════════

catalog! {
    name: "entl.tsp",
    version: "0",
    entities: [
        // git side
        Repo, GitNote, GitObject, Commit, Tree, Blob, FileChange, Ref, Conflict,
        SyncState,
        // forge side
        GhUser, GhSubject, GhPullRequest, GhIssue, GhComment, GhLabel, GhPrReview,
        GhReviewComment, GhEvent, GhWorkflow, GhWorkflowRun, GhJob, GhStep,
        GhCheckRun, GhCommitStatus,
    ],
    edges: [
        CommitParent, TreeEntry, GhLabeled, GhAssignee, GhPrCommit,
        GhRequestedReviewer,
    ],
    records: [
        GitStats, GithubStats, SinkStats, FileDiff, ChangeBatch, Statement,
        TableRename, SinkOptions, ExtractOptions, RebuildOptions, ChangesOptions,
        DriverPlanOptions,
    ],
    enums: [RefKind, FileStatus, PrState, IssueState, Mergeable, SinkTarget],
    scalars: [Oid, ArrowBatch],
    api: [Git, Entl],
}
