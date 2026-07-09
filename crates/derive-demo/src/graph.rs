//! Slice 2 demo: an entity **graph** with foreign keys, authored entirely in
//! Rust. Where Slice 1 proved one scalar table end-to-end, this proves
//! references — a field typed `Id<T>` declares a foreign key resolved from the
//! Rust type, so `@fk` never appears. It exercises every Slice-2 shape:
//!
//! * a **single-column FK** — `PullRequest.repo_id: Id<Repo>` (also FK-in-PK);
//! * a **nullable FK** — `Option<Id<GhUser>>`;
//! * a **composite-key target** — `PullRequest` keys on `(repo_id, number)` and
//!   declares `ref_cols(number = "pr_number")`, so `Review.pr: Id<PullRequest>`
//!   materialises the two FK columns `(repo_id, pr_number)` with the spelling
//!   the *target* declares, once.
//!
//! Compare against the TypeSpec a `.tsp` author would write for the same graph
//! (`graph.tsp`, alongside this file). The catalogs are semantically equivalent —
//! same tables, columns, keys, and FK columns — modulo the front-end stamp and
//! `source` name (`notes/derive-front-end-decisions.md`, "semantic-equivalence").

use fluessig_derive::{catalog, Entity, Id};

/// A repository — a single-column-keyed reference target.
#[derive(Entity)]
#[fluessig(name = "repos")]
pub struct Repo {
    /// The repo id (a path hash).
    #[key]
    pub id: String,
    /// The remote URL, when known.
    pub remote_url: Option<String>,
}

/// A GitHub user — the target of the nullable author/reviewer FKs.
#[derive(Entity)]
#[fluessig(name = "gh_users")]
pub struct GhUser {
    /// The GitHub user id.
    #[key]
    pub id: i64,
    /// The login handle.
    pub login: String,
}

/// A pull request — a **composite-key** entity: identified by `(repo_id,
/// number)`. `ref_cols(number = "pr_number")` declares that referencing sites
/// spell the `number` key part as the column `pr_number` (and `repo_id` as-is);
/// the spelling lives here, on the target, not at each referencing site.
#[derive(Entity)]
#[fluessig(name = "pull_requests", ref_cols(number = "pr_number"))]
pub struct PullRequest {
    /// The repo the PR belongs to — a single-column FK that is also a key member
    /// (FK-in-PK).
    #[key]
    pub repo_id: Id<Repo>,
    /// The PR number within the repo.
    #[key]
    pub number: i32,
    /// The PR title.
    pub title: Option<String>,
    /// The PR author — a nullable FK to `gh_users`.
    pub author_id: Option<Id<GhUser>>,
}

/// A PR review — globally id-keyed, referencing the composite-keyed
/// `PullRequest` by a single `Id<PullRequest>` field that expands to the two FK
/// columns `(repo_id, pr_number)`.
#[derive(Entity)]
#[fluessig(name = "reviews")]
pub struct Review {
    /// The review id.
    #[key]
    pub id: i64,
    /// The reviewed pull request — a composite FK `(repo_id, pr_number)`.
    pub pr: Id<PullRequest>,
    /// The reviewer — a nullable FK to `gh_users`.
    pub reviewer_id: Option<Id<GhUser>>,
}

// The exporter half: `catalog!` collects the graph into a `fluessig_catalog`
// module that prints the `catalog.json` the loader consumes. Declaration order
// is deliberately not topological — the reference resolver indexes all
// descriptors first, so a target may appear after the site that references it.
catalog! {
    name: "fk_graph_demo",
    version: "0.1.0",
    entities: [Repo, GhUser, PullRequest, Review],
}
