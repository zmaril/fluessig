import "./decorators.js";
using TypeSpec.Reflection;

// ─────────────────────────────────────────────────────────────────────────────
// The fluessig decorator library (ships as @fluessig/typespec; inlined here so
// this demo catalog is ONE self-contained .tsp file). Two decorator families
// over one document:
//   Layer B (relations):  @entity  @key  @edge(Props)  @compose
//   Binding surface:      @ctor  @stream  @manual   (op shapes)
// ─────────────────────────────────────────────────────────────────────────────
namespace Fluessig {
  extern dec entity(target: Model);
  extern dec key(target: ModelProperty);
  extern dec edge(target: ModelProperty, props: Model);
  extern dec compose(target: ModelProperty);
  extern dec ctor(target: Operation);
  extern dec stream(target: Operation);
  extern dec manual(target: Operation);
}

// ═════════════════════════════════════════════════════════════════════════════
// The data model — Layer A (records) + Layer B (relations) → catalog.json
// ═════════════════════════════════════════════════════════════════════════════

/** a git object id */
scalar Oid extends string;

enum ReviewState {
  approved,
  changesRequested,
  commented,
}

/** value struct: no identity, embeds wherever used */
model Signature {
  name: string;
  email: string;
}

@Fluessig.entity
model Commit {
  /** the commit hash */
  @Fluessig.key oid: Oid;

  message: string;
  author: Signature; // nested struct → jsonb in PG, subdoc in Mongo
  authorWhen?: utcDateTime; // optional → nullable

  /** to-many self association; edge properties live on CommitParent */
  @Fluessig.edge(CommitParent)
  parents: Commit[];
}

/** edge properties on Commit.parents */
model CommitParent {
  idx: int32;
}

@Fluessig.entity
model GhUser {
  @Fluessig.key id: int64;
  login: string;
}

@Fluessig.entity
model GhPullRequest {
  @Fluessig.key id: int64;
  title: string;
  state: ReviewState;
  author: GhUser; // to-one association (bare entity-typed field)

  /** owned aggregate → embedded in a doc store, child table elsewhere */
  @Fluessig.compose
  reviews: GhPrReview[];
}

@Fluessig.entity
model GhPrReview {
  @Fluessig.key id: int64;
  state: ReviewState;
}

// ═════════════════════════════════════════════════════════════════════════════
// The API surface — op shapes → api.json → generated napi/PyO3/Magnus bindings
// ═════════════════════════════════════════════════════════════════════════════

/** What one git load produced. */
model GitStats {
  newCommits: int64;
  fileChanges: int64;
}

/** One change-stream batch (rows as JSON until Arrow FFI lands). */
model ChangeBatch {
  table: string;
  `op`: string; // (op is a TypeSpec keyword — backtick-escaped)
  rowsJson: string;
}

/** An open entl database. */
interface Entl {
  /** Open (or create) the store at `dbPath` and apply the schema. */
  @Fluessig.ctor open(dbPath: string): void;

  /** Load git history from `repoPath` (one-way, incremental). */
  loadGit(repoPath: string): GitStats;

  /** Run a SQL query; JSON rows back. */
  query(sql: string): string;

  /** Stream the change batches from one pull of `repoPath`. */
  @Fluessig.stream changes(repoPath: string, github: boolean): ChangeBatch;
}
