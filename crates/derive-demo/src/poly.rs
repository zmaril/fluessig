//! Slice 4 demo: **polymorphic families** — an abstract root + a closed set of
//! concrete leaves that share one key, referenced by a (type-tag, key) column
//! pair — authored entirely in Rust (`notes/derive-front-end-decisions.md`,
//! Decision #3). Two families, deliberately modelled on entl's real ones so the
//! shapes are the production shapes:
//!
//! * **git objects** — a **scalar-keyed** family. `GitObject` is the root,
//!   keyed on `oid`; `Commit` / `Tree` / `Blob` are the leaves.
//!   `#[derive(AbstractRoot)] #[fluessig(abstract_root(Commit, Tree, Blob))]`
//!   generates `GitObjectId { Commit(String), Tree(String), Blob(String) }` and
//!   `impl AbstractRoot for GitObject`. The family declares its reference
//!   spelling once (`tag_col = "obj_type"`, `ref_col = "obj_oid"`); `GitRef`
//!   references it with that spelling, while `TreeEntry` pins a per-site
//!   `cols(tag = "entry_type", key = "child_oid")` override (entl FINDINGS #7).
//! * **gh subjects** — a **composite-keyed** family. `GhSubject` is the root,
//!   keyed on `(repo_id, number)`; `GhPullRequest` / `GhIssue` are the leaves.
//!   The generated enum carries the composite key —
//!   `GhSubjectId { GhPullRequest(Id<Repo>, i32), GhIssue(Id<Repo>, i32) }` —
//!   proving heterogeneous key handling. `GhComment` references the family with
//!   the family-declared spelling (`subject_type`, `repo_id`, `subject_number`).
//!
//! Compare against the TypeSpec a `.tsp` author would write for the same graph
//! (`poly.tsp`, alongside this file): the physical projections are semantically
//! equivalent — same tables, columns, keys, per-leaf key sharing, and (tag, key)
//! reference columns — modulo the front-end stamp and the `extends`-vs-generated
//! spelling.

use fluessig_derive::{catalog, AbstractRoot, Entity, Id};

/// A repository — the single-column-keyed reference target the composite family
/// keys on.
#[derive(Entity)]
#[fluessig(name = "repos")]
pub struct Repo {
    /// The repo id (a path hash).
    #[key]
    pub id: String,
}

// ── Scalar-keyed family: GitObject = Commit | Tree | Blob, keyed by `oid` ──

/// The git-object family root (entl DESIGN §2 Layer B). `abstract_root(Commit,
/// Tree, Blob)` closes the leaf set and generates the key enum + the
/// `AbstractRoot` alias; `tag_col` / `ref_col` spell how a polymorphic reference
/// to the family names its (type, key) columns. The root carries the family key
/// (`oid`) + a repo association, and is never a table of its own.
#[derive(AbstractRoot)]
#[fluessig(
    abstract_root(Commit, Tree, Blob),
    tag_col = "obj_type",
    ref_col = "obj_oid"
)]
pub struct GitObject {
    /// Object id — the content hash. Globally unique ⇒ the family key is `oid`.
    #[key]
    pub oid: String,
    /// The repo this object was seen in (an association, not a key member).
    pub repo_id: Id<Repo>,
}

/// One commit — a leaf of the GitObject family. `extends` inherits the family key
/// (`oid`) + columns (`repo_id`) into the `commits` table.
#[derive(Entity)]
#[fluessig(name = "commits", extends = GitObject)]
pub struct Commit {
    /// The full commit message.
    pub message: String,
    /// The first line of the message.
    pub summary: String,
}

/// A tree — a leaf with no own columns beyond the inherited family key (entl's
/// `Tree` is likewise just the object identity plus its `entries` edge).
#[derive(Entity)]
#[fluessig(name = "trees", extends = GitObject)]
pub struct Tree {}

/// A blob — a leaf carrying content metadata on top of the family key.
#[derive(Entity)]
#[fluessig(name = "blobs", extends = GitObject)]
pub struct Blob {
    /// The blob size in bytes.
    pub size: i64,
    /// Whether the blob is binary.
    pub is_binary: bool,
}

/// A git ref — a **polymorphic reference** into the GitObject family using the
/// family-declared spelling: `target` materialises `(obj_type, obj_oid)`.
#[derive(Entity)]
#[fluessig(name = "git_refs")]
pub struct GitRef {
    /// The ref id.
    #[key]
    pub id: i64,
    /// The object the ref points at — any leaf of the family. Spelled
    /// `(obj_type, obj_oid)` from GitObject's `tag_col` / `ref_col`.
    pub target: GitObjectId,
}

/// A tree entry — a **polymorphic reference** with a **per-site `cols(...)`
/// override** (legacy spelling `entry_type` / `child_oid`), proving the override
/// path coexists with the family default.
#[derive(Entity)]
#[fluessig(name = "tree_entries")]
pub struct TreeEntry {
    /// The tree this entry belongs to.
    #[key]
    pub tree_oid: Id<Tree>,
    /// The entry name within the tree.
    #[key]
    pub name: String,
    /// The object this entry points at — spelled `(entry_type, child_oid)`,
    /// overriding the family's default `(obj_type, obj_oid)`.
    #[fluessig(cols(tag = "entry_type", key = "child_oid"))]
    pub child: GitObjectId,
}

// ── Composite-keyed family: GhSubject = GhPullRequest | GhIssue, key (repo, num) ──

/// The PR/Issue family root — a **composite** key `(repo_id, number)`. The
/// generated enum carries the whole key per variant:
/// `GhSubjectId { GhPullRequest(Id<Repo>, i32), GhIssue(Id<Repo>, i32) }`. The
/// family declares its reference spelling once — `tag_col = "subject_type"` and
/// `ref_cols(number = "subject_number")` (the `repo_id` part keeps its name).
#[derive(AbstractRoot)]
#[fluessig(
    abstract_root(GhPullRequest, GhIssue),
    tag_col = "subject_type",
    ref_cols(number = "subject_number")
)]
pub struct GhSubject {
    /// The repo the subject belongs to.
    #[key]
    pub repo_id: Id<Repo>,
    /// The subject number within the repo.
    #[key]
    pub number: i32,
}

/// A pull request — a leaf of the GhSubject family. `extends` inherits the
/// composite family key `(repo_id, number)` into `gh_pull_requests`.
#[derive(Entity)]
#[fluessig(name = "gh_pull_requests", extends = GhSubject)]
pub struct GhPullRequest {
    /// The PR title.
    pub title: String,
    /// The PR state (open/closed/merged).
    pub state: String,
}

/// An issue — a leaf of the GhSubject family, sharing the same composite key.
#[derive(Entity)]
#[fluessig(name = "gh_issues", extends = GhSubject)]
pub struct GhIssue {
    /// The issue title.
    pub title: String,
}

/// An issue/PR comment — a **polymorphic reference** into the GhSubject family
/// using the family-declared spelling: `subject` materialises
/// `(subject_type, repo_id, subject_number)`.
#[derive(Entity)]
#[fluessig(name = "gh_comments")]
pub struct GhComment {
    /// The comment id.
    #[key]
    pub id: i64,
    /// The subject (PR or issue) the comment is on — a (type, key) pair.
    pub subject: GhSubjectId,
}

// The exporter half. The family roots (`GitObject`, `GhSubject`) ARE entities
// (abstract) and are listed; the leaves point back via `extends`.
catalog! {
    name: "poly_demo",
    version: "0.1.0",
    entities: [
        Repo,
        GitObject, Commit, Tree, Blob, GitRef, TreeEntry,
        GhSubject, GhPullRequest, GhIssue, GhComment,
    ],
}
