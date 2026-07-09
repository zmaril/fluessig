// The TypeSpec an author would write for the SAME foreign-key graph the Slice 2
// derive demo (`src/graph.rs`) authors in Rust. Used for the semantic-equivalence
// comparison in the Slice 2 gate: emit this via the TypeSpec path and confirm the
// derive catalog projects to the same physical tables/columns/keys/FK columns
// (modulo the front-end stamp + `source` name — the legitimately-differing
// front-end identity fields; see notes/derive-front-end-decisions.md).
//
// Note the front-end difference the projection normalises away: in TypeSpec a
// to-one relation is a field typed as the target with `@fk(#[...])` naming the
// columns (field name = the relation name, e.g. `repo`); in the derive the field
// name IS the FK column (`repo_id: Id<Repo>`) and the columns are resolved from
// the type. Both project to the same physical columns.

import "../../typespec/lib.tsp";
using Fluessig;

@entity
@name("repos")
model Repo {
  @key id: string;
  remoteUrl?: string;
}

@entity
@name("gh_users")
model GhUser {
  @key id: int64;
  login: string;
}

@entity
@name("pull_requests")
model PullRequest {
  @key @fk(#["repo_id"]) repo: Repo;
  @key number: int32;
  title?: string;
  @fk(#["author_id"]) author?: GhUser;
}

@entity
@name("reviews")
model Review {
  @key id: int64;
  @fk(#["repo_id", "pr_number"]) pr: PullRequest;
  @fk(#["reviewer_id"]) reviewer?: GhUser;
}
