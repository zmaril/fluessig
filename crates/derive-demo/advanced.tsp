// The TypeSpec an author would write for the SAME schema the Slice 3 derive demo
// (`src/advanced.rs`) authors in Rust. Used for the semantic-equivalence
// comparison in the Slice 3 gate: emit this via the TypeSpec path and confirm the
// derive catalog projects to the same physical tables/columns/keys/edge tables
// (modulo the front-end stamp + `source` name; see
// notes/derive-front-end-decisions.md).
//
// The front-end differences the projection normalises away:
//
//   * INHERITANCE. TypeSpec spells the git-object family with an `@abstract` root
//     and `Commit extends GitObject`; the root's columns reach `commits` through
//     `extends`. The derive spells the same shape as `#[fluessig(flatten)]`
//     embedding — the root's columns are spliced into the leaf's own field list.
//     Both project to a `commits` table carrying (oid, repo_id, message, summary)
//     keyed on oid.
//
//   * EDGES. TypeSpec puts the relation on the entity (`@edge`/`@fk`/`@fkSource`
//     on `Commit.parents`); the derive puts it on the edge's own row struct
//     (`#[derive(Edge)]`). Both project to the same `commit_parents` /
//     `gh_issue_labels` tables.
//
//   * COLUMN SHARING. TypeSpec makes `repo_id` shared by naming it in BOTH the
//     edge's `@fk` (target) and `@fkSource` (source) column lists; the derive
//     makes it a declared fact with `#[fluessig(shares(repo_id))]`. Both fold the
//     two into one physical `repo_id` column.

import "../../typespec/lib.tsp";
using Fluessig;

@entity
@name("repos")
model Repo {
  @key id: string;
}

// The git-object family root — abstract, carries only its key + a repo
// association (FINDINGS #6). The derive spells this as a flatten source.
@entity
@abstract
model GitObject {
  @key oid: string;

  @fk(#["repo_id"])
  repo: Repo;
}

@entity
@name("commits")
model Commit extends GitObject {
  message: string;
  summary: string;

  @edge(CommitParent)
  @name("commit_parents")
  @fk(#["parent_oid"])
  @fkSource(#["commit_oid"])
  parents: Commit[];
}

/** edge properties on Commit.parents — local key only (edge PK = source + idx) */
model CommitParent {
  @key idx: int32;
}

@entity
@name("gh_labels")
model GhLabel {
  @key
  @fk(#["repo_id"])
  repo: Repo;

  @key name: string;
}

@entity
@name("gh_issues")
model GhIssue {
  @key
  @fk(#["repo_id"])
  repo: Repo;

  @key number: int32;

  // The join edge to labels. `repo_id` is named in BOTH the target @fk and the
  // source @fkSource, so it is a shared physical column (the derive's
  // `shares(repo_id)`); `label_name` / `issue_number` are the reference spellings.
  @name("gh_issue_labels")
  @fk(#["repo_id", "label_name"])
  @fkSource(#["repo_id", "issue_number"])
  labels: GhLabel[];
}
