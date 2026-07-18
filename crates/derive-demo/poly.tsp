// The TypeSpec an author would write for the SAME polymorphic-family schema the
// Slice 4 derive demo (`src/poly.rs`) authors in Rust. Used for the
// semantic-equivalence comparison in the Slice 4 gate: emit this via the
// TypeSpec path and confirm the derive catalog projects to the same physical
// tables/columns/keys/per-leaf key sharing and (tag, key) reference columns
// (modulo the front-end stamp + `source` name; see
// notes/derive-front-end-decisions.md).
//
// The front-end difference the projection normalises away is POLYMORPHISM
// SPELLING:
//
//   * TypeSpec spells a family with `@abstract` on the root + `Leaf extends Root`;
//     the root's key + columns reach each leaf table through `extends`. The derive
//     spells the same shape as `#[derive(AbstractRoot)] #[fluessig(abstract_root(
//     Commit, Tree, Blob))]` on the root (which ALSO generates the `<Root>Id` key
//     enum) + `#[fluessig(extends = Root)]` on the leaves. Both project to
//     per-leaf tables that share the family key.
//
//   * A polymorphic reference is `@fk(#[key…], typeColumn) field: Family` in
//     TypeSpec; in the derive it's a field typed `<Root>Id`, spelled from the
//     family (`tag_col`/`ref_col`/`ref_cols`) or a per-site `cols(...)` override.
//     Both project to a (type-tag, key) column pair.

import "../../typespec/lib.tsp";
using Fluessig;

@entity
@name("repos")
model Repo {
  @key id: string;
}

// ── Scalar-keyed family: GitObject = Commit | Tree | Blob, keyed by `oid` ──

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
}

@entity
@name("trees")
model Tree extends GitObject {}

@entity
@name("blobs")
model Blob extends GitObject {
  size: int64;
  isBinary: boolean;
}

@entity
@name("git_refs")
model GitRef {
  @key id: int64;

  // family-declared spelling: (obj_type, obj_oid)
  @fk(#["obj_oid"], "obj_type")
  target: GitObject;
}

@entity
@name("tree_entries")
model TreeEntry {
  @key
  @fk(#["tree_oid"])
  tree: Tree;

  @key name: string;

  // per-site override spelling: (entry_type, child_oid)
  @fk(#["child_oid"], "entry_type")
  child: GitObject;
}

// ── Composite-keyed family: GhSubject = GhPullRequest | GhIssue, key (repo, num) ──

@entity
@abstract
model GhSubject {
  @key
  @fk(#["repo_id"])
  repo: Repo;

  @key number: int32;
}

@entity
@name("gh_pull_requests")
model GhPullRequest extends GhSubject {
  title: string;
  state: string;
}

@entity
@name("gh_issues")
model GhIssue extends GhSubject {
  title: string;
}

@entity
@name("gh_comments")
model GhComment {
  @key id: int64;

  // the (type, key) pair: (subject_type, repo_id + subject_number)
  @fk(#["repo_id", "subject_number"], "subject_type")
  subject: GhSubject;
}
