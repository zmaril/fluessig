//! entl_derive_sketch.rs — HYPOTHETICAL: the complete entl catalog (all 28
//! tables + the full binding op surface) authored with a `fluessig` derive
//! front end. Line-for-line parallel to ../entl.tsp; the design it sketches
//! is written up in ./derive-front-end.md. Nothing in here compiles — the
//! `fluessig` crate surface it assumes does not exist.
//!
//! Ergonomic mechanisms applied to the polymorphic surface (doc §2.5):
//!
//!   1. FAMILY-DECLARED REFERENCE SPELLING — an abstract root (or any
//!      much-referenced entity) declares ONCE how references to it spell
//!      their columns (`tag_col`, `ref_col`). Referencing sites become bare
//!      fields. Per-site `cols(...)` remains only where legacy DDL genuinely
//!      differs per site (GitObject: entry_type/child_oid vs target_type/…).
//!
//!   2. GENERATED KEY ENUMS — `abstract_root(Leaf, …)` names the closed leaf
//!      set (the design doc's "closed and known at build time" rule, made
//!      syntactic — it's what lets the derive generate code) and emits a real
//!      sum type per family:
//!
//!          pub enum GitObjectId { Commit(Oid), Tree(Oid), Blob(Oid) }
//!          pub enum GhSubjectId { GhPullRequest(Id<Repo>, i32),
//!                                 GhIssue(Id<Repo>, i32) }
//!
//!      plus a tag enum (GitObjectTag / GhSubjectTag) whose catalog string
//!      values ("commit", "tree", …) are the stored discriminators. Ingest
//!      code constructs `GitObjectId::Blob(oid)` — the tag can never disagree
//!      with the key, and adding a leaf is one edit that exhaustiveness-checks
//!      every construction site.
//!
//!   3. `shares(col)` — column sharing between two FKs is a declared fact
//!      (and a real constraint: the label must live in the SAME repo as the
//!      subject), not a spelling coincidence the loader silently dedups.
//!
//! Assumed crate surface (none of this exists yet):
//!   fluessig::{Entity, Record, Scalar, Enum}  — descriptor-emitting derives
//!   fluessig::Id<T>       — T's key; renders as T's declared reference columns
//!   fluessig::Json        — the stock Json scalar
//!   <Family>Id / <Family>Tag — generated per abstract_root, as above
//!
//! The derives generate NO runtime behavior beyond the Id enums — each expands
//! to a `&'static EntityDescriptor` (fields, keys, relations, docs,
//! file!()/line!() spans) referenced by the exporter at the bottom. Every
//! struct is a real row type; there is no mirrored second set.

use chrono::{DateTime, Utc};
use fluessig::{Entity, Enum, Id, Json, Record, Scalar};

// ═════════════════════════════════════════════════════════════════════════════
// Scalars & enums
// ═════════════════════════════════════════════════════════════════════════════

/// a git object id (stored as raw bytes; hex is a representation view — DESIGN §9.1)
#[derive(Scalar, Clone, PartialEq, Eq, Hash)]
#[fluessig(extends = "bytes")]
pub struct Oid(pub Vec<u8>);

// (Json is a stock fluessig scalar — no local declaration needed.)

/// one Arrow RecordBatch, held columnar and surfaced per language: an IPC-bytes
/// getter everywhere, plus the Arrow PyCapsule protocol (zero-copy C Data
/// Interface) in Python. (Opaque `scalar ArrowBatch;` in .tsp; here the scalar
/// wraps the actual runtime type.)
#[derive(Scalar)]
pub struct ArrowBatch(pub arrow::record_batch::RecordBatch);

#[derive(Enum, Clone, Copy)]
pub enum RefKind {
    Branch,
    Tag,
    Remote,
    Head,
}

/// file_changes.status / FileDiff.status — git status codes; stored as the single letters
#[derive(Enum, Clone, Copy)]
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

// ═════════════════════════════════════════════════════════════════════════════
// Git side
// ═════════════════════════════════════════════════════════════════════════════

/// A repository entl has synced.
#[derive(Entity)]
#[fluessig(name = "repos")]
pub struct Repo {
    /// path hash
    #[fluessig(key)]
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
    #[fluessig(key)]
    pub repo_id: Id<Repo>,

    /// full notes ref name, e.g. `refs/notes/commits`
    #[fluessig(key)]
    pub notes_ref: String,

    /// the object the note annotates (usually a commit)
    #[fluessig(key)]
    pub annotated_oid: Oid,

    pub note: String,
}

/// The git object family — the polymorphic root (DESIGN §2 Layer B). A ref or a
/// tree entry may point at ANY object; the family guarantees one key type (Oid)
/// so a polymorphic reference is the typeable pair (type, oid).
///
/// `abstract_root(Commit, Tree, Blob)` closes the leaf set and generates:
///
///     pub enum GitObjectId  { Commit(Oid), Tree(Oid), Blob(Oid) }
///     pub enum GitObjectTag { Commit, Tree, Blob }   // stored: "commit"|"tree"|"blob"
///
/// No family-level tag_col / ref_col here: unlike GhSubject, GitObject's
/// reference spelling varies per site (entry_type/child_oid on tree entries;
/// target_type/target_oid on the future annotated-tag refs, FINDINGS #7), so
/// sites pin their own column names.
#[derive(Entity, Clone)]
#[fluessig(abstract_root(Commit, Tree, Blob))]
pub struct GitObject {
    /// object id — the content hash (binary; hex via `lower(hex(oid))`).
    /// Globally unique, so the family key is oid alone.
    #[fluessig(key)]
    pub oid: Oid,

    /// the repo this object was seen in (an association, not a key member)
    pub repo_id: Id<Repo>,
}

/// One row per commit, walked from every ref. Author and committer time/identity
/// are kept separate; `summary` is the first line of `message`.
#[derive(Entity)]
#[fluessig(name = "commits", extends = GitObject)]
pub struct Commit {
    /// contributes the family columns (oid, repo_id) first — column-order
    /// parity with the .tsp inheritance layout
    #[fluessig(flatten)]
    pub object: GitObject,

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
    /// exists. `of = "parents"` names the relation CommitParent exposes below;
    /// resolved and checked at catalog load.
    #[fluessig(default = false, derived(exists, of = "parents", filter(idx = 1)))]
    pub is_merge: bool,

    #[fluessig(default = false)]
    pub gpg_signed: bool,
}

/// this commit's parents, ordered by `idx` (merges have several)
/// — edge properties on the relation exposed as `Commit::parents`
#[derive(Entity)]
#[fluessig(name = "commit_parents", edge(from = Commit, to = Commit, expose = "parents"))]
pub struct CommitParent {
    #[fluessig(key)]
    pub commit_oid: Id<Commit>,

    pub parent_oid: Id<Commit>,

    /// FINDINGS #3: @key on an edge field = local key; edge PK = source key + local key
    #[fluessig(key)]
    pub idx: i32,
}

#[derive(Entity)]
#[fluessig(name = "trees", extends = GitObject)]
pub struct Tree {
    #[fluessig(flatten)]
    pub object: GitObject,
    // relation `entries` lives on TreeEntry below
}

/// The polymorphic edge already in production: tree_entries stores the
/// (type, key) pair as (entry_type, child_oid). Exposed as `Tree::entries`.
#[derive(Entity)]
#[fluessig(name = "tree_entries", edge(from = Tree, to = GitObject, expose = "entries"))]
pub struct TreeEntry {
    #[fluessig(key)]
    pub tree_oid: Id<Tree>,

    /// PK = (tree_oid, name)
    #[fluessig(key)]
    pub name: String,

    /// A real sum type: ingest writes `GitObjectId::Blob(oid)` — tag and key
    /// can't disagree, and `match` is exhaustive when the family grows.
    /// The cols(...) pin is pure legacy parity (defaults would be
    /// child_type / child_oid); a greenfield schema writes the bare field.
    #[fluessig(cols(tag = "entry_type", key = "child_oid"))]
    pub child: GitObjectId,

    pub path: String,
    /// 100644 / 100755 / 120000 / 040000 / 160000
    pub mode: String,
}

#[derive(Entity)]
#[fluessig(name = "blobs", extends = GitObject)]
pub struct Blob {
    #[fluessig(flatten)]
    pub object: GitObject,

    pub size: i64,

    #[fluessig(default = false)]
    pub is_binary: bool,

    pub content_text: Option<String>,
    pub content_sha: Option<String>,

    /// raw file bytes (object ingest / --objects); enables `entl rebuild`
    pub content: Option<Vec<u8>>,
}

/// Per-commit file changes — a WEAK ENTITY: identified by (owner, path), no
/// global id of its own. FK-in-PK falls out naturally: the key fields are
/// Id-typed. (FINDINGS #5's compose-child tension is unchanged by the front end.)
#[derive(Entity)]
#[fluessig(name = "file_changes")]
pub struct FileChange {
    #[fluessig(key)]
    pub commit_oid: Id<Commit>,

    #[fluessig(key)]
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
    #[fluessig(key)]
    pub repo_id: Id<Repo>,

    /// short ref name, e.g. `main` or `origin/main`
    #[fluessig(key)]
    pub name: String,

    pub kind: RefKind,

    /// Today: implicitly commit-typed (no discriminator column). The honest
    /// model is the family sum type —
    ///     #[fluessig(cols(tag = "target_type", key = "target_oid"))]
    ///     pub target: GitObjectId,
    /// — but that adds a target_type column, i.e. a real schema change.
    /// Authored as Id<Commit> for byte parity; the upgrade is FINDINGS #7.
    pub target_oid: Id<Commit>,

    #[fluessig(default = false)]
    pub is_symbolic: bool,

    pub upstream: Option<String>,
}

/// Merge-conflict hot zones (the north star). Populated by the `entl conflicts`
/// pass: replay every historical 2-parent merge with gix's 3-way tree merge and
/// record the paths that conflicted.
#[derive(Entity)]
#[fluessig(name = "conflicts")]
pub struct Conflict {
    #[fluessig(key)]
    pub repo_id: Id<Repo>,

    /// hex text today, while every other oid column is blob — schema
    /// inconsistency, FINDINGS #8 (louder here: a String among Oids).
    #[fluessig(key)]
    pub merge_oid: String,

    #[fluessig(key)]
    pub path: String,

    pub unresolved: bool,
}

/// Per-resource sync bookkeeping (etags, cursors, watermarks).
#[derive(Entity)]
#[fluessig(name = "sync_state")]
pub struct SyncState {
    #[fluessig(key)]
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
    #[fluessig(key)]
    pub id: i64,
    pub login: String,
    /// `type` is a Rust keyword too — r# instead of .tsp's backticks
    pub r#type: Option<String>,
    pub name: Option<String>,
}

/// The PR/Issue family — the SECOND polymorphic root already in production.
/// Root carries ONLY the key (FINDINGS #6) — which makes the flatten-embed
/// cheap: two fields.
///
/// THE OPTION-1 PAYOFF LIVES HERE. Every site that references this family
/// spells the columns identically (gh_comments, gh_labeled, gh_assignees:
/// subject_type / repo_id / subject_number), so the spelling is declared ONCE,
/// on the family, and the three sites below become bare fields.
///
/// Generated:
///     pub enum GhSubjectId  { GhPullRequest(Id<Repo>, i32), GhIssue(Id<Repo>, i32) }
///     pub enum GhSubjectTag { GhPullRequest, GhIssue }   // stored: "pr" | "issue"
#[derive(Entity, Clone)]
#[fluessig(
    abstract_root(GhPullRequest, GhIssue),
    tag_col = "subject_type",
    tag_values(GhPullRequest = "pr", GhIssue = "issue")
)]
pub struct GhSubject {
    /// referenced as-is: repo_id
    #[fluessig(key)]
    pub repo_id: Id<Repo>,

    /// how referencing sites spell this key part
    #[fluessig(key, ref_col = "subject_number")]
    pub number: i32,
}

/// Pull requests and their lifecycle. `mergeable` + `checks` are the live
/// conflict and CI signals; head/base oids drive on-demand base...head diffs.
///
/// Option 1 generalized to a CONCRETE entity: four tables reference this PR
/// (gh_pr_commits, gh_requested_reviewers, gh_pr_reviews, gh_review_comments),
/// all as (repo_id, pr_number) — so the reference spelling is declared here,
/// once, and all four sites drop their per-site cols(...) pins.
#[derive(Entity)]
#[fluessig(
    name = "gh_pull_requests",
    extends = GhSubject,
    ref_cols(repo_id = "repo_id", number = "pr_number")
)]
pub struct GhPullRequest {
    #[fluessig(flatten)]
    pub subject: GhSubject,

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
    #[fluessig(flatten)]
    pub subject: GhSubject,

    pub title: Option<String>,
    pub body: Option<String>,
    pub state: IssueState,

    pub author_id: Option<Id<GhUser>>,

    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
}

// ── Edges & join tables off the GhSubject family ──
//
// v1 of this sketch had the densest attributes in the file here. With the
// family declaring its own reference spelling, the polymorphic SOURCE side of
// each edge is now a bare enum-typed key field.

#[derive(Entity)]
#[fluessig(name = "gh_labeled", edge(from = GhSubject, to = GhLabel, expose = "labels"))]
pub struct GhLabeled {
    /// → subject_type, repo_id, subject_number (spelling from the family)
    #[fluessig(key)]
    pub subject: GhSubjectId,

    /// GhLabel's key is (repo_id, name). `shares(repo_id)` collapses the first
    /// component onto the subject's repo_id column — declaring the REAL
    /// constraint (the label must belong to the same repo as the subject) —
    /// and the second takes its default spelling {field}_{keycol} = label_name.
    /// Net columns: repo_id (shared), label_name. Exact legacy parity, and
    /// both remaining attributes state model facts, not column spellings.
    #[fluessig(key, shares(repo_id))]
    pub label: Id<GhLabel>,
}

/// gh_assignees has DDL but no writer today — todo.txt; modeled faithfully.
#[derive(Entity)]
#[fluessig(name = "gh_assignees", edge(from = GhSubject, to = GhUser, expose = "assignees"))]
pub struct GhAssignee {
    #[fluessig(key)]
    pub subject: GhSubjectId,

    #[fluessig(key)]
    pub user_id: Id<GhUser>,
}

/// plain join table (PK = all columns): the PR's commits
#[derive(Entity)]
#[fluessig(name = "gh_pr_commits", edge(from = GhPullRequest, to = Commit, expose = "commits"))]
pub struct GhPrCommit {
    /// → (repo_id, pr_number), spelling from GhPullRequest's ref_cols
    #[fluessig(key)]
    pub pr: Id<GhPullRequest>,

    #[fluessig(key)]
    pub commit_oid: Id<Commit>,
}

/// plain join table: requested reviewers
#[derive(Entity)]
#[fluessig(
    name = "gh_requested_reviewers",
    edge(from = GhPullRequest, to = GhUser, expose = "requested_reviewers")
)]
pub struct GhRequestedReviewer {
    #[fluessig(key)]
    pub pr: Id<GhPullRequest>,

    #[fluessig(key)]
    pub user_id: Id<GhUser>,
}

/// Issue/PR comments — subject is a polymorphic reference into the GhSubject
/// family. (v1 needed type_col + key_cols pins here; now: one bare field.)
#[derive(Entity)]
#[fluessig(name = "gh_comments")]
pub struct GhComment {
    #[fluessig(key)]
    pub id: i64,

    /// → subject_type, repo_id, subject_number
    pub subject: GhSubjectId,

    pub author_id: Option<Id<GhUser>>,

    pub body: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Entity)]
#[fluessig(name = "gh_labels")]
pub struct GhLabel {
    #[fluessig(key)]
    pub repo_id: Id<Repo>,

    #[fluessig(key)]
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// PR reviews — globally id-keyed, referencing the PR by its composite key.
#[derive(Entity)]
#[fluessig(name = "gh_pr_reviews")]
pub struct GhPrReview {
    #[fluessig(key)]
    pub id: i64,

    /// multi-column FK to a composite-keyed entity — (repo_id, pr_number),
    /// spelling from GhPullRequest's ref_cols; no per-site pin
    pub pr: Id<GhPullRequest>,

    pub reviewer_id: Option<Id<GhUser>>,

    pub state: Option<String>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub body: Option<String>,
}

#[derive(Entity)]
#[fluessig(name = "gh_review_comments")]
pub struct GhReviewComment {
    #[fluessig(key)]
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

/// Raw GitHub event stream (the /events feed) — entl's "did anything happen?"
/// signal AND a queryable activity log. The feed is capped (~300 events / 90
/// days): complete going FORWARD from when polling starts.
#[derive(Entity)]
#[fluessig(name = "gh_events")]
pub struct GhEvent {
    #[fluessig(key)]
    pub repo_id: Id<Repo>,

    #[fluessig(key)]
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
    #[fluessig(key)]
    pub id: i64,

    pub repo_id: Id<Repo>,

    pub name: Option<String>,
    pub path: Option<String>,
    pub state: Option<String>,
}

#[derive(Entity)]
#[fluessig(name = "gh_workflow_runs")]
pub struct GhWorkflowRun {
    #[fluessig(key)]
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
    #[fluessig(key)]
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
    #[fluessig(key)]
    pub job_id: Id<GhJob>,

    #[fluessig(key)]
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
    #[fluessig(key)]
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
    #[fluessig(key)]
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
// The API surface — op shapes → api.json → generated bindings (Step 5b)
//
// The hand-implemented core IS the declaration: #[fluessig::export] records
// each method's shape (ctor / unary / stream / manual) into api.json; bindgen
// renders napi/pyo3/Magnus glue + typed TS/Python/Ruby surfaces from that.
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

/// One change-stream batch. The rows stay columnar (Arrow): `ipc` yields them
/// as one Arrow IPC stream; the Python class additionally speaks the Arrow
/// PyCapsule protocol for zero-copy import. No per-row `_op`: `op`/`table`
/// ride this envelope.
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

#[derive(Enum, Clone, Copy)]
pub enum SinkTarget {
    Sqlite,
    Jsonl,
    Postgres,
}

#[derive(Record)]
pub struct TableRename {
    pub from: String,
    pub to: String,
}

#[derive(Record, Default)]
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
    /// duckdb | sqlite | jsonl | postgres. (Still named `source`, not `from`:
    /// the op surface stays binding-friendly — `from` is a Python keyword.)
    pub source: String,

    pub dest: String,

    /// output directory for the reconstructed repo
    pub out: String,

    pub schema: Option<String>,
}

#[derive(Record, Default)]
pub struct ChangesOptions {
    pub github: Option<bool>,
    pub objects: Option<bool>,
}

#[derive(Record, Default)]
pub struct DriverPlanOptions {
    pub tables: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub rename: Option<Vec<TableRename>>,
    pub schema: Option<String>,
}

/// Stateless, repo-scoped git helpers (no database handle). All unary.
#[fluessig::export]
pub mod git {
    use super::*;

    /// Diff two commits (`base...head` when three_dot).
    pub fn diff_commits(
        repo_path: &str,
        base: &str,
        head: &str,
        three_dot: bool,
    ) -> fluessig::Result<Vec<FileDiff>> {
        todo!()
    }

    /// A file's content at a commit — None when absent or binary.
    pub fn file_at(repo_path: &str, commit: &str, path: &str) -> fluessig::Result<Option<String>> {
        todo!()
    }

    pub fn branch_exists(repo_path: &str, name: &str) -> fluessig::Result<bool> {
        todo!()
    }

    pub fn current_branch(repo_path: &str) -> fluessig::Result<String> {
        todo!()
    }

    /// Commit subjects+bodies along a branch (JSON).
    pub fn commit_bodies(repo_path: &str, branch: &str) -> fluessig::Result<String> {
        todo!()
    }

    /// Remote branch names matching a pattern (trailing-`*` glob). Fetches first.
    pub fn ls_remote_heads(repo_path: &str, pattern: &str) -> fluessig::Result<Vec<String>> {
        todo!()
    }
}

/// An open entl database. Heavy ops are unary (off-thread in async hosts).
pub struct Entl {
    // the hand-implemented core
}

#[fluessig::export]
impl Entl {
    /// Open (or create) the .duckdb at `db_path` and apply the schema.
    #[fluessig(ctor)]
    pub fn open(db_path: &str) -> fluessig::Result<Self> {
        todo!()
    }

    /// Load git history from `repo_path` (one-way, incremental).
    pub fn load_git(&self, repo_path: &str) -> fluessig::Result<GitStats> {
        todo!()
    }

    /// Load GitHub data (events/PRs/issues/Actions). Needs a token.
    pub fn load_github(&self, repo_path: &str) -> fluessig::Result<GithubStats> {
        todo!()
    }

    /// Run a SQL query; JSON rows back.
    pub fn query(&self, sql: &str) -> fluessig::Result<String> {
        todo!()
    }

    /// Run a SQL query; the result as one Arrow IPC stream (schema + all
    /// batches) — the dataframe on-ramp.
    pub fn query_arrow(&self, sql: &str) -> fluessig::Result<Vec<u8>> {
        todo!()
    }

    /// Pull `repo_path` and sync it into a target store, in one call.
    pub fn sink(&self, repo_path: &str, options: SinkOptions) -> fluessig::Result<SinkStats> {
        todo!()
    }

    /// Read a store back into canonical rows (JSON; oids hex, timestamps RFC3339).
    pub fn extract(&self, options: ExtractOptions) -> fluessig::Result<String> {
        todo!()
    }

    /// Stream the change batches from one pull (the stream plane).
    /// bindgen maps Iterator → JS async iterator / Python generator / Ruby Enumerator.
    #[fluessig(stream)]
    pub fn changes(
        &self,
        repo_path: &str,
        options: Option<ChangesOptions>,
    ) -> fluessig::Result<impl Iterator<Item = fluessig::Result<ChangeBatch>>> {
        todo!()
    }

    /// Backfill this store into a driver target: stream {sql, params} for the host to execute.
    #[fluessig(stream)]
    pub fn driver_plan(
        &self,
        options: Option<DriverPlanOptions>,
    ) -> fluessig::Result<impl Iterator<Item = fluessig::Result<Statement>>> {
        todo!()
    }

    /// Reconstruct a git repo from a store (needs objects: true at sink time). Returns commits rebuilt.
    pub fn rebuild(&self, options: RebuildOptions) -> fluessig::Result<i64> {
        todo!()
    }

    /// Poll-loop sync with a host callback (ThreadsafeFunction / GVL re-entry)
    /// — hand-written per binding, exactly as before.
    #[fluessig(manual)]
    pub fn watch(&self, repo_path: &str, interval_secs: i32) {
        todo!()
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// The exporter — replaces `(cd emitter && node emit.mjs ../entl.tsp)`
// ═════════════════════════════════════════════════════════════════════════════

fluessig::catalog! {
    name: "entl",
    version: 0,
    entities: [
        // git side
        Repo, GitNote, GitObject, Commit, CommitParent, Tree, TreeEntry, Blob,
        FileChange, Ref, Conflict, SyncState,
        // forge side
        GhUser, GhSubject, GhPullRequest, GhIssue, GhLabeled, GhAssignee,
        GhPrCommit, GhRequestedReviewer, GhComment, GhLabel, GhPrReview,
        GhReviewComment, GhEvent, GhWorkflow, GhWorkflowRun, GhJob, GhStep,
        GhCheckRun, GhCommitStatus,
    ],
    api: [git, Entl],
}

#[cfg(test)]
mod schema_tests {
    #[test]
    fn catalog_is_current() {
        // regenerates in-memory, runs full loader validation (family rules —
        // including abstract_root leaf list vs extends declarations, key
        // arity, shares() type compatibility) with file:line spans, and diffs
        // against the checked-in catalog.json.
        fluessig::assert_catalog_current!(super::__fluessig_catalog(), "catalog.json");
    }
}
