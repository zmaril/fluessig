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
// Slice 8a Gap 2: both front ends now materialise the entities AND DTOs the ops
// reference as FLATTENED `models` in api.json (a to-one relation → its FK
// field(s); the referenced set closed transitively). The comparison covers BOTH
// the ops AND the models (see tests/api_typespec_equivalence.rs).

import "../../typespec/lib.tsp";
using Fluessig;

// ── DTOs / value structs (plain models, no @entity) the ops pass across ──
model LoadStats {
  commits: int64;
  refs: int64;
}

model TableRename {
  oldName: string;
  newName: string;
}

model SinkOptions {
  path?: string;
  renames: TableRename[];
}

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

  /** Load and report a stats DTO (a value-struct return). */
  load(): LoadStats;

  /** Sink to a target described by a DTO param. */
  sink(options: SinkOptions): int64;
}

/** Stateless git helpers (no database handle). All unary. */
interface GitHelpers {
  /** Whether `name` is a branch in the repo at `repo_path`. */
  branchExists(repoPath: string, name: string): boolean;

  /** The current branch name at `repo_path`. */
  currentBranch(repoPath: string): string;
}
