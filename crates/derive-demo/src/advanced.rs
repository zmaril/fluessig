//! Slice 3 demo: the attribute grammar in action — **flatten** embedding, **edge
//! structs**, and **column sharing** — authored entirely in Rust with the
//! `darling`-parsed `#[fluessig(...)]` grammar.
//!
//! Two little families, deliberately modelled on entl's real ones so the shapes
//! are the production shapes, not toys:
//!
//! * **git objects.** `GitObject` is a root that carries *only* its key + a repo
//!   association (entl FINDINGS #6). `Commit` embeds it with
//!   `#[fluessig(flatten)]`, so the root's columns (`oid`, `repo_id`) land in the
//!   `commits` table itself — the inheritance/abstract-root pattern, expressed as
//!   embedding (§2.3). `CommitParent` is the parent edge: its own row struct
//!   (§2.4) with a local key `idx`, so the edge PK is `(commit_oid, idx)`
//!   (FINDINGS #3).
//! * **issue labels.** `GhIssueLabel` is a join edge from `GhIssue` to `GhLabel`.
//!   A label is identified by `(repo_id, label_name)` and an issue by
//!   `(repo_id, issue_number)`; the two `repo_id`s are the *same repo*, so the
//!   label reference declares `#[fluessig(shares(repo_id))]` — the shared column
//!   is a stated fact (and a real same-repo constraint), not a spelling
//!   coincidence the projection silently dedups (§2.5).
//!
//! Compare against the TypeSpec a `.tsp` author would write for the same graph
//! (`advanced.tsp`, alongside this file): the physical projections are
//! semantically equivalent — same tables, columns, keys, and edge tables —
//! modulo the front-end stamp and the flatten-vs-`extends` spelling
//! (`notes/derive-front-end-decisions.md`, "semantic-equivalence").

use fluessig_derive::{catalog, Edge, Entity, Id};

/// A repository — the single-column-keyed reference target shared by both
/// families.
#[derive(Entity)]
#[fluessig(name = "repos")]
pub struct Repo {
    /// The repo id (a path hash).
    #[key]
    pub id: String,
}

/// The git-object root — the abstract-root-carries-only-its-key shape (entl
/// FINDINGS #6). It is never a table of its own here; it exists to be embedded
/// via `#[fluessig(flatten)]`, contributing `(oid, repo_id)` to each leaf.
/// (Deriving `Entity` gives it a descriptor for the flatten to read; it is
/// deliberately absent from the `catalog!` entity list.)
#[derive(Entity)]
pub struct GitObject {
    /// Object id — the content hash. Globally unique, so the family key is `oid`.
    #[key]
    pub oid: String,
    /// The repo this object was seen in (an association, not a key member).
    pub repo_id: Id<Repo>,
}

/// One commit. `#[fluessig(flatten)]` embeds `GitObject`, so `commits` carries
/// the root's `oid` (key) and `repo_id` columns inline, then its own fields.
#[derive(Entity)]
#[fluessig(name = "commits")]
pub struct Commit {
    /// The git-object identity, embedded inline: contributes `oid`, `repo_id`.
    #[fluessig(flatten)]
    pub object: GitObject,
    /// The full commit message.
    pub message: String,
    /// The first line of the message.
    pub summary: String,
}

/// The commit-parent edge — its own row struct (§2.4). A self-edge from `Commit`
/// to `Commit`: the first `Id<Commit>` is the source, the second the target. The
/// local key `idx` orders a merge's parents; the edge PK is `(commit_oid, idx)`.
#[derive(Edge)]
#[fluessig(name = "commit_parents", edge(from = Commit, to = Commit, expose = "parents"))]
pub struct CommitParent {
    /// The child commit (source side).
    pub commit_oid: Id<Commit>,
    /// The parent commit (target side).
    pub parent_oid: Id<Commit>,
    /// Parent ordinal — the edge's local key.
    #[fluessig(key)]
    pub idx: i32,
}

/// A GitHub label — composite-keyed on `(repo_id, name)`. `ref_cols` spells how
/// referencing sites name those key parts: `label_repo_id` / `label_name`.
#[derive(Entity)]
#[fluessig(
    name = "gh_labels",
    ref_cols(repo_id = "label_repo_id", name = "label_name")
)]
pub struct GhLabel {
    /// The repo the label belongs to.
    #[key]
    pub repo_id: Id<Repo>,
    /// The label text.
    #[key]
    pub name: String,
}

/// A GitHub issue — composite-keyed on `(repo_id, number)`. `ref_cols` spells the
/// `number` key part as `issue_number` at referencing sites.
#[derive(Entity)]
#[fluessig(name = "gh_issues", ref_cols(number = "issue_number"))]
pub struct GhIssue {
    /// The repo the issue belongs to.
    #[key]
    pub repo_id: Id<Repo>,
    /// The issue number within the repo.
    #[key]
    pub number: i32,
}

/// The issue-label edge — a join edge with no local key. The `label` reference
/// `#[fluessig(shares(repo_id))]` declares that the label's leading FK column is
/// the shared physical column `repo_id` — the same repo the issue is in — so the
/// `gh_issue_labels` table carries `repo_id` **once**, not a separate
/// `label_repo_id`.
#[derive(Edge)]
#[fluessig(name = "gh_issue_labels", edge(from = GhIssue, to = GhLabel, expose = "labels"))]
pub struct GhIssueLabel {
    /// The labelled issue (source side): `(repo_id, issue_number)`.
    pub issue: Id<GhIssue>,
    /// The applied label (target side): `(repo_id shared, label_name)`.
    #[fluessig(shares(repo_id))]
    pub label: Id<GhLabel>,
}

// The exporter half. `GitObject` is intentionally omitted from `entities` — it is
// a flatten source, not a table. Both edges are listed under `edges`.
catalog! {
    name: "git_gh_demo",
    version: "0.1.0",
    entities: [Repo, Commit, GhIssue, GhLabel],
    edges: [CommitParent, GhIssueLabel],
}
