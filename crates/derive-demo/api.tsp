// The TypeSpec an author would write for the SAME op surface the Slice 5 derive
// demo (`src/api.rs`) authors in Rust with `#[fluessig::export]`. Used for the
// semantic-equivalence comparison in the Slice 5 gate: emit this via the
// TypeSpec path and confirm the derive `api.json` carries the same OPS — same op
// names, kinds (ctor/unary/stream/manual), params (name + type + optional), and
// returns — modulo the front-end stamp + `source` name.
//
// The front-end difference the op comparison normalises away is that TypeSpec
// declares the op surface in a SEPARATE `interface`, kept in sync with the
// hand-written core by discipline; the derive captures it from the `impl` block
// that actually runs. Both lower to the same `api.json` op shapes.
//
// NB: the TypeSpec path additionally materialises the entities the ops reference
// (`Repo`, `PullRequest`) as FLATTENED DTO `models` in api.json; the Slice-5
// derive path emits the op surface only (empty `models`). The comparison is at
// the OP level, per notes/derive-front-end-decisions.md (Slice 5 is the op
// surface; the DTO/model layer is a separate concern).

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

/** An open demo database. Heavy ops are unary; the change feed is a stream. */
interface Db {
  /** Open (or create) the store at `path` and apply the schema. */
  @ctor open(path: string): void;

  /** Look up one repo by id — `None` when absent. */
  repo(id: string): Repo | null;

  /** Count the pull requests in a repo. */
  pullRequestCount(repoId: string): int64;

  /** The repo ids known to the store. */
  repos(limit?: int32): string[];

  /** Stream every pull request in a repo (the stream plane). */
  @stream pullRequests(repoId: string): PullRequest;

  /** Poll-loop sync with a host callback — hand-written per binding. */
  @manual watch(intervalSecs: int32): void;
}

/** Stateless git helpers (no database handle). All unary. */
interface GitHelpers {
  /** Whether `name` is a branch in the repo at `repo_path`. */
  branchExists(repoPath: string, name: string): boolean;

  /** The current branch name at `repo_path`. */
  currentBranch(repoPath: string): string;
}
