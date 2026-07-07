import "./typespec/lib.tsp";
using Fluessig;

// ═════════════════════════════════════════════════════════════════════════════
// entl.tsp — the COMPLETE entl catalog in fluessig's language: all 28 tables +
// the full binding op surface. This is the acid test from plan.txt ("write the
// full entl.tsp before freezing the IR") and the future single source of truth
// that entl's schema templates, generated read models, and hand-written binding
// glue regenerate from (DESIGN §6, decision #14).
//
// Decorators come from @fluessig/typespec (./typespec/lib.tsp). Primary keys
// use TypeSpec's built-in @key. PROPOSED vocabulary discovered by this file is
// recorded with evidence in FINDINGS.md.
// ═════════════════════════════════════════════════════════════════════════════

// ═════════════════════════════════════════════════════════════════════════════
// Scalars & enums
// ═════════════════════════════════════════════════════════════════════════════

/** a git object id (stored as raw bytes; hex is a representation view — DESIGN §9.1) */
scalar Oid extends bytes;

// (Json is a stock Fluessig scalar now — no local declaration needed.)

/** one Arrow RecordBatch, held columnar and surfaced per language: an IPC-bytes
 * getter everywhere, plus the Arrow PyCapsule protocol (zero-copy C Data
 * Interface) in Python */
scalar ArrowBatch;

enum RefKind {
  branch,
  tag,
  remote,
  head,
}

/** file_changes.status / FileDiff.status — git status codes; stored as the single letters */
enum FileStatus {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
}

enum PrState {
  OPEN,
  CLOSED,
  MERGED,
}

enum IssueState {
  OPEN,
  CLOSED,
}

/** GitHub computes mergeability lazily */
enum Mergeable {
  MERGEABLE,
  CONFLICTING,
  UNKNOWN,
}

// ═════════════════════════════════════════════════════════════════════════════
// Git side
// ═════════════════════════════════════════════════════════════════════════════

/** A repository entl has synced. */
@entity
@name("repos")
model Repo {
  @key id: string; // path hash
  path: string;
  remoteUrl?: string;
  host?: string;
  owner?: string;
  name?: string;
  /** from the forge (`repos/{o}/{r}`), when a GitHub sync has run */
  defaultBranch?: string;

  /** the repo's homepage URL from the forge, when set */
  homepageUrl?: string;

  firstSyncedAt?: utcDateTime;
  lastSyncedAt?: utcDateTime;
}

/**
 * A git note (`refs/notes/*`): free-form text attached to any object after the
 * fact, without rewriting it. Mutable per-repo state like refs — each sync
 * replaces the repo's rows. The default ref is `refs/notes/commits`; other
 * namespaces ride the notesRef column.
 */
@entity
@name("git_notes")
model GitNote {
  @key
  @fk(#["repo_id"])
  repo: Repo;

  /** full notes ref name, e.g. `refs/notes/commits` */
  @key notesRef: string;

  /** the object the note annotates (usually a commit) */
  @key annotatedOid: Oid;

  note: string;
}

/**
 * The git object family — the polymorphic root (DESIGN §2 Layer B). A ref or a
 * tree entry may point at ANY object; the family guarantees one key type (Oid)
 * so a polymorphic reference is the typeable pair (type, oid). Concrete leaves:
 * Commit, Tree, Blob. (Annotated tag objects are a known future member —
 * today's ingest resolves refs straight to commits; see FINDINGS #7.)
 */
@entity
@abstract
model GitObject {
  /** object id — the content hash (binary; hex via `lower(hex(oid))`). Globally unique, so the family key is oid alone. */
  @key oid: Oid;

  /** the repo this object was seen in (an association, not a key member) */
  @fk(#["repo_id"])
  repo: Repo;
}

/**
 * One row per commit, walked from every ref. Author and committer time/identity
 * are kept separate; `summary` is the first line of `message`.
 */
@entity
@name("commits")
model Commit extends GitObject {
  /** root tree this commit points at */
  @fk(#["tree_oid"])
  tree: Tree;

  /** full commit message */
  message: string;

  /** first line of the message */
  summary: string;

  authorName?: string;
  authorEmail?: string;
  authorWhen?: utcDateTime;
  authorTz?: string;
  committerName?: string;
  committerEmail?: string;
  committerWhen?: utcDateTime;
  committerTz?: string;

  @defaultValue(0)
  parentCount: int32;

  /**
   * The §9.3 v1 carve-out: a REAL derived field — true iff a second parent
   * exists. Today it is ALSO a stored column (the source computes it; parity
   * with the hand-written templates) and the codec additionally emits a
   * `commits_derived` view computing it from `parents`. At the Step-5 swap the
   * stored column drops and the view becomes the only projection.
   */
  @defaultValue(false)
  @derived("exists", "parents", #{ idx: 1 })
  isMerge: boolean;

  @defaultValue(false)
  gpgSigned: boolean;

  /** this commit's parents, ordered by `idx` (merges have several) */
  @edge(CommitParent)
  @name("commit_parents")
  @fk(#["parent_oid"])
  @fkSource(#["commit_oid"])
  parents: Commit[];
}

/** edge properties on Commit.parents */
model CommitParent {
  @key idx: int32; // PROPOSED: @key on an edge-struct field = local key; edge PK = source key + local key (FINDINGS #3)
}

@entity
@name("trees")
model Tree extends GitObject {
  /**
   * The polymorphic edge already in production: tree_entries stores the
   * (type, key) pair as (entry_type, child_oid) — exactly DESIGN §2's
   * projection of a polymorphic association.
   */
  @edge(TreeEntry)
  @name("tree_entries")
  @fk(#["child_oid"], "entry_type")
  @fkSource(#["tree_oid"])
  entries: GitObject[];
}

/** edge properties on Tree.entries */
model TreeEntry {
  @key name: string; // PK = (tree_oid, name)
  path: string;
  mode: string; // 100644 / 100755 / 120000 / 040000 / 160000
}

@entity
@name("blobs")
model Blob extends GitObject {
  size: int64;

  @defaultValue(false)
  isBinary: boolean;

  contentText?: string;
  contentSha?: string;

  /** raw file bytes (object ingest / --objects); enables `entl rebuild` */
  content?: bytes;
}

/**
 * Per-commit file changes — a WEAK ENTITY: identified by (owner, path), no
 * global id of its own. Authored as an entity with FK-in-PK; whether it should
 * *also* be a @compose child of Commit (embed in doc stores) is an open
 * modeling tension recorded in FINDINGS #5.
 */
@entity
@name("file_changes")
model FileChange {
  @key
  @fk(#["commit_oid"])
  commit: Commit;

  @key path: string;

  oldPath?: string;
  status: FileStatus;
  additions?: int32;
  deletions?: int32;

  @fk(#["blob_oid"])
  blob?: Blob;

  @fk(#["old_blob_oid"])
  oldBlob?: Blob;
}

/** Branches, tags, remote-tracking refs, and HEAD — one row each. */
@entity
@name("refs")
model Ref {
  @key
  @fk(#["repo_id"])
  repo: Repo;

  /** short ref name, e.g. `main` or `origin/main` */
  @key name: string;

  kind: RefKind;

  /**
   * Today: implicitly commit-typed (no discriminator column). The honest model
   * is `target: GitObject` — an annotated-tag ref targets a tag object, not a
   * commit — but that adds a target_type column, i.e. a real schema change.
   * Authored as Commit for byte parity; the upgrade is FINDINGS #7.
   */
  @fk(#["target_oid"])
  target: Commit;

  @defaultValue(false)
  isSymbolic: boolean;

  upstream?: string;
}

/**
 * Merge-conflict hot zones (the north star). Populated by the `entl conflicts`
 * pass: replay every historical 2-parent merge with gix's 3-way tree merge and
 * record the paths that conflicted. `unresolved` = git would have required a
 * human, vs auto-resolved.
 */
@entity
@name("conflicts")
model Conflict {
  @key
  @fk(#["repo_id"])
  repo: Repo;

  /** hex text today, while every other oid column is blob — schema inconsistency, FINDINGS #8 */
  @key mergeOid: string;

  @key path: string;

  unresolved: boolean;
}

/** Per-resource sync bookkeeping (etags, cursors, watermarks). */
@entity
@name("sync_state")
model SyncState {
  @key resource: string;
  cursor?: string;
  etag?: string;
  watermark?: utcDateTime;
  lastSyncedAt?: utcDateTime;
  lastError?: string;
}

// ═════════════════════════════════════════════════════════════════════════════
// Forge side (GitHub)
// ═════════════════════════════════════════════════════════════════════════════

@entity
@name("gh_users")
model GhUser {
  @key id: int64;
  login: string;
  `type`?: string;
  name?: string;
}

/**
 * The PR/Issue family — the SECOND polymorphic root already in production:
 * gh_comments / gh_labeled / gh_assignees reference a subject as the pair
 * (subject_type, subject_number) against this family's shared key
 * (repo, number). Root carries ONLY the key: sharing data fields via
 * inheritance would break per-leaf column-order parity (FINDINGS #6).
 */
@entity
@abstract
model GhSubject {
  @key
  @fk(#["repo_id"])
  repo: Repo;

  @key number: int32;

  // Edges FROM the polymorphic family: the SOURCE side is itself the
  // (type, key) pair — (repo_id, subject_type, subject_number). Join tables
  // add no columns to the leaf tables, so relations (unlike data fields) are
  // safe to share on the abstract root without breaking column-order parity.
  // (gh_assignees has DDL but no writer today — todo.txt; modeled faithfully.)
  @name("gh_labeled")
  @fk(#["repo_id", "label_name"])
  @fkSource(#["repo_id", "subject_type", "subject_number"], "subject_type")
  labels: GhLabel[];

  @name("gh_assignees")
  @fk(#["user_id"])
  @fkSource(#["repo_id", "subject_type", "subject_number"], "subject_type")
  assignees: GhUser[];
}

/**
 * Pull requests and their lifecycle. `mergeable` + `checks` are the live
 * conflict and CI signals; head/base oids drive on-demand base...head diffs.
 */
@entity
@name("gh_pull_requests")
model GhPullRequest extends GhSubject {
  title?: string;
  body?: string;
  state: PrState;

  @fk(#["author_id"])
  author?: GhUser;

  createdAt?: utcDateTime;
  updatedAt?: utcDateTime;
  closedAt?: utcDateTime;
  mergedAt?: utcDateTime;

  @fk(#["merge_commit_oid"])
  mergeCommit?: Commit;

  headRef?: string;
  baseRef?: string;
  additions?: int32;
  deletions?: int32;
  changedFiles?: int32;

  @defaultValue(false)
  isDraft: boolean;

  mergeable?: Mergeable;

  /** head commit CI rollup: SUCCESS | FAILURE | PENDING | … (open set — stays text) */
  checks?: string;

  /** PR head commit (for base...head diffs) */
  @fk(#["head_oid"])
  head?: Commit;

  /** base branch tip at fetch time */
  @fk(#["base_oid"])
  base?: Commit;

  /** plain join table (PK = all columns): the PR's commits */
  @name("gh_pr_commits")
  @fk(#["commit_oid"])
  @fkSource(#["repo_id", "pr_number"])
  commits: Commit[];

  /** plain join table: requested reviewers */
  @name("gh_requested_reviewers")
  @fk(#["user_id"])
  @fkSource(#["repo_id", "pr_number"])
  requestedReviewers: GhUser[];
}

@entity
@name("gh_issues")
model GhIssue extends GhSubject {
  title?: string;
  body?: string;
  state: IssueState;

  @fk(#["author_id"])
  author?: GhUser;

  createdAt?: utcDateTime;
  updatedAt?: utcDateTime;
  closedAt?: utcDateTime;
}

/** Issue/PR comments — subject is a polymorphic reference into the GhSubject family. */
@entity
@name("gh_comments")
model GhComment {
  @key id: int64;

  /** the (type, key) pair: (subject_type, repo_id + subject_number) */
  @fk(#["repo_id", "subject_number"], "subject_type")
  subject: GhSubject;

  @fk(#["author_id"])
  author?: GhUser;

  body?: string;
  createdAt?: utcDateTime;
}

@entity
@name("gh_labels")
model GhLabel {
  @key
  @fk(#["repo_id"])
  repo: Repo;

  @key name: string;
  color?: string;
  description?: string;
}

/** PR reviews — globally id-keyed, referencing the PR by its composite key. */
@entity
@name("gh_pr_reviews")
model GhPrReview {
  @key id: int64;

  /** multi-column FK to a composite-keyed entity (repo_id, pr_number) */
  @fk(#["repo_id", "pr_number"])
  pr: GhPullRequest;

  @fk(#["reviewer_id"])
  reviewer?: GhUser;

  state?: string;
  submittedAt?: utcDateTime;
  body?: string;
}

@entity
@name("gh_review_comments")
model GhReviewComment {
  @key id: int64;

  @fk(#["repo_id", "pr_number"])
  pr: GhPullRequest;

  path?: string;
  line?: int32;
  side?: string;

  @fk(#["commit_oid"])
  commit?: Commit;

  @fk(#["author_id"])
  author?: GhUser;

  body?: string;
  createdAt?: utcDateTime;

  /** threaded replies — a nullable self-association */
  @fk(#["in_reply_to"])
  inReplyTo?: GhReviewComment;
}

/**
 * Raw GitHub event stream (the /events feed) — entl's "did anything happen?"
 * signal AND a queryable activity log. The feed is capped (~300 events / 90
 * days): complete going FORWARD from when polling starts.
 */
@entity
@name("gh_events")
model GhEvent {
  @key
  @fk(#["repo_id"])
  repo: Repo;

  @key id: string;

  `type`?: string;

  /** denormalized pair: the association + a plain login column (FINDINGS #9) */
  @fk(#["actor_id"])
  actor?: GhUser;

  actorLogin?: string;
  createdAt?: utcDateTime;

  /** the event's type-specific JSON */
  payload?: Json;
}

// ── Actions / Checks ──

@entity
@name("gh_workflows")
model GhWorkflow {
  @key id: int64;

  @fk(#["repo_id"])
  repo: Repo;

  name?: string;
  path?: string;
  state?: string;
}

@entity
@name("gh_workflow_runs")
model GhWorkflowRun {
  @key id: int64;

  @fk(#["repo_id"])
  repo: Repo;

  @fk(#["workflow_id"])
  workflow?: GhWorkflow;

  @fk(#["head_oid"])
  head?: Commit;

  headBranch?: string;
  event?: string;
  status?: string;
  conclusion?: string;
  runNumber?: int32;
  runAttempt?: int32;
  createdAt?: utcDateTime;
  updatedAt?: utcDateTime;
  runStartedAt?: utcDateTime;
}

@entity
@name("gh_jobs")
model GhJob {
  @key id: int64;

  @fk(#["run_id"])
  run: GhWorkflowRun;

  name?: string;
  status?: string;
  conclusion?: string;
  startedAt?: utcDateTime;
  completedAt?: utcDateTime;
  runnerName?: string;
}

/** Weak entity again: a step is (job, number), no global id. */
@entity
@name("gh_steps")
model GhStep {
  @key
  @fk(#["job_id"])
  job: GhJob;

  @key number: int32;

  name?: string;
  status?: string;
  conclusion?: string;
  startedAt?: utcDateTime;
  completedAt?: utcDateTime;
}

@entity
@name("gh_check_runs")
model GhCheckRun {
  @key id: int64;

  @fk(#["repo_id"])
  repo: Repo;

  @fk(#["commit_oid"])
  commit?: Commit;

  name?: string;
  status?: string;
  conclusion?: string;
  startedAt?: utcDateTime;
  completedAt?: utcDateTime;
}

@entity
@name("gh_commit_statuses")
model GhCommitStatus {
  @key id: int64;

  @fk(#["repo_id"])
  repo: Repo;

  @fk(#["commit_oid"])
  commit: Commit;

  context?: string;
  state?: string;
  description?: string;
  targetUrl?: string;
  createdAt?: utcDateTime;
}

// ═════════════════════════════════════════════════════════════════════════════
// The API surface — op shapes → api.json → generated bindings (Step 5b)
// ═════════════════════════════════════════════════════════════════════════════

/** What one git load produced. */
model GitStats {
  newCommits: int64;
  fileChanges: int64;
  refs: int64;
}

/** What one GitHub load produced. */
model GithubStats {
  events: int64;
  pullRequests: int64;
  reviews: int64;
  reviewComments: int64;
  issues: int64;
  comments: int64;
  workflowRuns: int64;
  checkRuns: int64;
  users: int64;
}

/** What a sink() cycle produced: git + forge counts, and rows applied to the target. */
model SinkStats {
  newCommits: int64;
  fileChanges: int64;
  refs: int64;
  pullRequests: int64;
  issues: int64;
  events: int64;
  workflowRuns: int64;
  checkRuns: int64;
  rows: int64;
}

/** One changed file between two commits. */
model FileDiff {
  path: string;
  oldPath?: string;
  status: FileStatus;
  additions: int64;
  deletions: int64;

  /** full single-file unified patch; empty for binary files (matching GitHub) */
  patch: string;
}

/** One change-stream batch. The rows stay columnar (Arrow): `ipc` yields them as
 * one Arrow IPC stream (a single RecordBatch, schema included), and the Python
 * class additionally speaks the Arrow PyCapsule protocol for zero-copy import
 * (`pa.record_batch(batch)`). Values are native Arrow types — oids are Binary,
 * timestamps Timestamp(µs) — unlike query()/extract()'s canonical JSON (hex
 * oids, RFC3339). No per-row `_op`: `op`/`table` ride this envelope. */
model ChangeBatch {
  table: string;
  `op`: string;
  ipc: ArrowBatch;
}

/** One driver-plan statement: the host runs it verbatim against its own client. */
model Statement {
  sql: string;
  params: Json;

  /** canonical source table (null for cross-table DDL) — for per-table tallies */
  table?: string;
}

enum SinkTarget {
  sqlite,
  jsonl,
  postgres,
}

model TableRename {
  from: string;
  to: string;
}

model SinkOptions {
  target: SinkTarget;

  /** the SQLite file, the JSONL directory, or the Postgres URL */
  path?: string;

  /** also pull GitHub (default true; needs a token) */
  github?: boolean;

  /** only write these tables (default: all) */
  tables?: string[];

  exclude?: string[];
  rename?: TableRename[];

  /** target schema (Postgres only; default "entl") */
  schema?: string;

  /** also store the object graph (trees/blobs + raw content) so the store can rebuild the repo */
  objects?: boolean;
}

model ExtractOptions {
  /** duckdb | sqlite | jsonl | postgres */
  source: string;

  path: string;
  tables?: string[];
  schema?: string;
}

model RebuildOptions {
  /** duckdb | sqlite | jsonl | postgres. (Named `source`, not `from`: `from` is a
   * Python keyword and awkward in JS — the op surface stays binding-friendly.) */
  source: string;

  dest: string;

  /** output directory for the reconstructed repo */
  out: string;

  schema?: string;
}

model ChangesOptions {
  github?: boolean;
  objects?: boolean;
}

model DriverPlanOptions {
  tables?: string[];
  exclude?: string[];
  rename?: TableRename[];
  schema?: string;
}

/** Stateless, repo-scoped git helpers (no database handle). All unary. */
interface Git {
  /** Diff two commits (`base...head` when threeDot). */
  diffCommits(repoPath: string, base: string, head: string, threeDot: boolean): FileDiff[];

  /** A file's content at a commit — null when absent or binary. */
  fileAt(repoPath: string, commit: string, path: string): string | null;

  branchExists(repoPath: string, name: string): boolean;
  currentBranch(repoPath: string): string;

  /** Commit subjects+bodies along a branch (JSON). */
  commitBodies(repoPath: string, branch: string): string;

  /** Remote branch names matching a pattern (trailing-`*` glob). Fetches first. */
  lsRemoteHeads(repoPath: string, pattern: string): string[];
}

/** An open entl database. Heavy ops are unary (off-thread in async hosts). */
interface Entl {
  /** Open (or create) the .duckdb at `dbPath` and apply the schema. */
  @ctor open(dbPath: string): void;

  /** Load git history from `repoPath` (one-way, incremental). */
  loadGit(repoPath: string): GitStats;

  /** Load GitHub data (events/PRs/issues/Actions). Needs a token. */
  loadGithub(repoPath: string): GithubStats;

  /** Run a SQL query; JSON rows back. */
  query(sql: string): string;

  /** Run a SQL query; the result as one Arrow IPC stream (schema + all batches) —
   * the dataframe on-ramp (pyarrow → pandas/polars, apache-arrow JS, red-arrow). */
  queryArrow(sql: string): bytes;

  /** Pull `repoPath` and sync it into a target store, in one call. */
  sink(repoPath: string, options: SinkOptions): SinkStats;

  /** Read a store back into canonical rows (JSON; oids hex, timestamps RFC3339). */
  extract(options: ExtractOptions): string;

  /** Stream the change batches from one pull (the stream plane). */
  @stream changes(repoPath: string, options?: ChangesOptions): ChangeBatch;

  /** Backfill this store into a driver target: stream {sql, params} for the host to execute. */
  @stream driverPlan(options?: DriverPlanOptions): Statement;

  /** Reconstruct a git repo from a store (needs objects: true at sink time). Returns commits rebuilt. */
  rebuild(options: RebuildOptions): int64;

  /** Poll-loop sync with a host callback (ThreadsafeFunction / GVL re-entry) — hand-written per binding. */
  @manual watch(repoPath: string, intervalSecs: int32): void;
}
