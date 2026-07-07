# fluessig — design

*flüssig* (German, "fluid"): describe your data once, in **TypeSpec**; let it flow into whatever
shape a store needs. fluessig is **a schema language + a generator + a data-marshalling runtime** —
it turns one model into DDL, ORM mappings, and format codecs for many stores: relational, document,
graph, columnar, and wire formats.

Status: design (TypeSpec emitter route spike-proven). First consumer + dogfood:
[entl](../../AGENTS.md), whose driver sink (turning a change stream into SQL statements) becomes
fluessig's first SQL data codec.

---

## 1. What it is (and isn't)

fluessig has three faces over **one** data model:

1. **A schema language** — **TypeSpec** used as an *authored data-modeling IDL* (its `model` /
   `scalar` / `union` / `enum` core; no `interface`/`op` API constructs). A fluessig **decorator
   library** (`@fluessig/typespec`, npm) carries our semantics: `@entity`, `@key`, `@compose`,
   `@edge`, `@card`, … The source of truth is a set of `.tsp` files checked into git.
2. **A generator** — `tsp compile` with the fluessig **emitter** (`@fluessig/emitter`, npm)
   produces **`catalog.json`**: a fully *resolved* snapshot of the checked type graph (templates
   instantiated, aliases resolved, decorators applied). The `fluessig` binary
   (`generate|import-arrow|validate`) consumes `catalog.json`. A Python / TS / Go shop uses the
   binary + npx with zero Rust. This is the protoc / Prisma model, with Node confined to
   authoring time.
3. **A runtime library** — Rust (+ napi/pyo3 bindings) that loads a catalog and does the
   high-throughput **data marshalling**: change batches of records → `INSERT`s / Mongo documents /
   JSONL / Parquet. This is the leg the schema-language world doesn't have, and our
   differentiation.

**One honest framing up front:** relational, document, and graph are equal projections **for
writes** — the pitch is *one model, many write targets*. fluessig has no query layer; how you *read*
each store is dictated by the physical shape that store's codec chose (§2).

It is **not**: a query engine, an API server, a migration *runner*, or an ORM. It emits artifacts
and statements; the caller applies them. It holds no DB connections and does no I/O to databases
(a pure library — same discipline as entl-core).

### Why TypeSpec — the decision record

The language question splits along the model's own two layers (§2): a **record layer** (types,
nesting, unions, semantic scalars) and a **relationship layer** (cardinality, composition,
edge properties, polymorphic targets). Candidates were rejected in this order:

- **YAML/TOML/JSON** — generic trees; relations become blobs you interpret; validation is
  JSON-Schema-over-a-tree instead of a typed parse. (YAML additionally: footguns + archived
  `serde_yaml`; TOML: awkward deep nesting; JSON: no comments.)
- **Arrow-as-model** — columnar-record-shaped; no notion of relationship; couples the type enum to
  `arrow-rs`. Arrow is a *codec and data carrier*, never the master (§2).
- **DBML** — readable, but relational to the core (SQL physical types, FK-shaped refs, no
  nesting/composition, no edge properties). Adopting it collapses the multi-paradigm thesis. Its
  one edge (diagrams) we get from a Mermaid `erDiagram` codec instead.
- **GraphQL SDL** — excellent at the native-syntax tier (types, nullability, lists,
  docs) and workable at the directive tier (keys, indexes) — but its ceiling is real: no type
  aliases or generics, no unions over scalars, no map type, no imports/namespaces, and directive
  arguments are constant literals only, so anything expression-shaped degrades into string
  mini-DSLs (the documented Amplify `@auth` / Hasura-permissions failure mode). Cross-references
  are stringly typed (`@relation(properties: "CommitParent")`).
- **TypeQL** (TypeDB; MPL 2.0; native Rust/pest parser) — the strongest *relationship* layer any
  shipped language has: n-ary relations with named roles, edge properties as plain ownership
  (`relation tree-entry … owns entry-name`), inheritance-driven polymorphism. But it has **no
  record layer**: no value structs (a `Signature` must be flattened or wrongly promoted to an
  entity), no unions, no `bytes`, no nesting — and the grammar evolves with the database's needs,
  not a generator's. Quarry for Layer-B semantics, not a substrate.
- **Gel SDL / ESDL** (Apache 2.0) — the best *overall* semantics ever shipped for a typed entity
  graph: links with link properties, `single`/`multi` cardinality keywords, computed fields,
  constraint expressions, abstract types with polymorphic links. The company is winding down (the
  team joined Vercel in early 2026; the product stays open-source and effectively frozen) — which
  disqualifies it as a living substrate but *improves* it as a quarry: the code is
  Apache-licensed and frozen, and the data model is formally specified in their paper
  (arXiv:2507.16089). fluessig's decorator semantics are specified against ESDL's feature list.
- **TypeSpec** (Microsoft, MIT) — **chosen.** A layered language whose bottom layer is a pure
  type-graph core: `model`, `scalar X extends Y` (semantic-over-physical refinement in native
  syntax), unions including over scalars and literals, `enum`, `alias`, templates/generics,
  `Record<V>` maps, namespaces + imports, doc comments, and **typed decorators** whose arguments
  are checked references, not strings. The `interface`/`op` API layer is simply not used —
  Microsoft's own JSON Schema emitter ships for exactly this data-only mode.

**Consumption model (spike-proven):** fluessig never parses `.tsp`. The fluessig emitter walks the
**checked program** (the sanctioned emitter API — the same mechanism as the openapi3/json-schema
emitters) and serializes it to `catalog.json`. This sidesteps writing/maintaining a parser, gets
template instantiation and alias resolution for free, and keeps the Rust core free of Node. A
native Rust `.tsp` front-end can be added later without changing anything downstream, because the
Rust core's actual input is the catalog, not the language.

**Division of labor:** TypeSpec's grammar provides **Layer A** natively; fluessig's decorators
provide **Layer B**, with semantics specified against Gel's link model and TypeQL's
role/edge-ownership model. The one construct neither prior language has — composition/embed
intent — is fluessig's own contribution, fittingly, since it is the load-bearing idea of the
multi-paradigm thesis.

**Cost accepted:** Node.js in the *authoring* toolchain. **v1:** `cargo install --git …` for the
`fluessig` binary + an ambient Node/npm for the `.tsp → catalog.json` step — fine for the entl
dogfood. The TypeSpec compiler version is pinned and compiler upgrades are releases (upstream drift
is a real, managed risk). **Later (a packaging milestone, not a v1 gate):** bundle the TypeSpec
compiler + catalog printer into one executable so authoring needs no ambient Node. Runtime and
generator are native consumers of `catalog.json` regardless.

### Locked decisions

| Decision | Choice | Consequence |
|---|---|---|
| Source of truth | **TypeSpec** (`.tsp`, authored) + `@fluessig/typespec` decorators | Layer A is native syntax; Layer B is typed decorators; no string DSLs. |
| Language ↔ core interchange | **`catalog.json`** — versioned, fully resolved | Rust core never embeds Node; other front-ends (Arrow import, builder) target the same catalog. |
| Schema change model | **Stateless rebuild + fingerprint detection** | Idempotent DDL; a catalog hash in `_fluessig_meta` lets callers *detect* drift (`IF NOT EXISTS` alone silently keeps stale shapes). No ALTER / diff / history in v1. |
| Scope | **Schema + data codecs** | The data plane has a specified contract (§5): change batches in, grouped statements out. |
| Targets (eventual) | **SQL, Mongo, ORM codegen, file formats, graph** | Built spine-first (§7), not all at once. |

---

## 2. The core question: how do we model the data?

The tempting shortcut is "an entity is an Arrow `Schema`." **We reject that**, for two reasons:

1. **Arrow is columnar-table-shaped.** It models a *record* (typed fields, including nested
   Struct/List/Map) beautifully — but it has no notion of a *relationship*. Foreign keys, edges,
   embedding: none of it exists in Arrow. Build the model on Arrow and the relational assumption is
   baked in; a property graph becomes impossible to project cleanly, because in a graph an edge is
   a first-class thing with its own properties, not a column.
2. **It couples us to `arrow-rs`.** If our type enum *is* `arrow::DataType`, we can never drop the
   dependency and Arrow's physical choices leak into every codec.

So fluessig models data as a **typed entity graph** — the classical *conceptual*
(Entity–Relationship) layer that sits **above** any physical store. TypeSpec is how you *write*
it; these are its semantics. Two layers:

### Layer A — the record/type layer (what Arrow maps to)

Entities have fields; fields have a **logical type**. The taxonomy is *inspired by* Arrow + Avro
logical types, but is **our own enum**, so Arrow is a codec and nothing more.

```
Type =
  | Scalar(Text | I8..I64 | U8..U64 | F32 | F64 | Bool | Timestamp{unit,tz} | Date
           | Oid | Uuid | Bytes | Decimal{precision,scale} | Json | Enum{variants})
  | Struct(StructRef)       // named value struct — a plain TypeSpec model, no identity
  | List(Type)              // repeated
  | Map(Type)               // string-keyed (TypeSpec Record<V>); non-string keys deferred
  | Union(Type…)            // tagged; TypeSpec unions — projection rules per codec, minimal in v1
```

Semantic scalars are declared in the language itself — `scalar Oid extends bytes` — the
semantic-over-physical split of Avro/Arrow *logical types* in native syntax rather than
documentation. Parameterized scalars (precision/scale, timestamp unit/tz) are carried by
decorators (`@precision(38, 9)`), since no mainstream schema grammar parameterizes scalars.
**Arrow maps onto this layer only**, and row data flows as Arrow `RecordBatch`es through it.

Nullability is expressed with TypeSpec's optional marker (`field?: T`); `T | null` unions are
rejected by the validator to keep one spelling.

### Layer B — the relationship layer (what Arrow can't express)

Relations are **first-class**, declared between entities — never inferred from data. The IR
bakes in the lessons of the git stress test (edge properties are the *common* case; polymorphic
targets are load-bearing):

```
Relation {
  to:          OneOf<[EntityRef]>,      // 1 = plain; >1 = polymorphic (union or abstract supertype)
  cardinality: Card { min, max: Option<u32> },   // from list marker + optional @card range
  kind:        Composition | Association,        // "embed vs. reference" — the DDD aggregate boundary
  properties:  Option<StructRef>,       // edge properties (graph-native; assoc-table in SQL)
  ends:        { from: Name, to: Name },// named ends (TypeQL roles, binary only in v1)
  inverse:     Option<FieldRef>,        // checked reference, not a string
}
```

- **Cardinality** — min/max, not just one/many: `@card(1, 2)` (git parents on non-octopus hosts)
  is expressible; codecs may only enforce the coarse one/many split.
- **Composition vs. Association** — does the child's *identity and lifecycle* belong to the parent
  (compose → embed) or is it an independent thing being referenced (associate → link)? This single
  distinction tells a document store to nest vs. reference: the **aggregate boundary** from DDD,
  and the construct no prior schema language carries.
- **Edge properties** — data *on the relationship itself* (an `idx`, a `name`, a `mode`). Native
  in a property graph; an association table in SQL; a bridge shape in documents. The git exercise
  showed these are the common case, not the exotic one — they are **in v1** (§6).
- **Polymorphic targets** — a relation may target an abstract supertype family (git: a tree entry
  is Blob | Tree | Commit; a tag targets any object). This is a **parity** need, not speculation: a
  ref's `target_oid` points at *any* object — an annotated-tag ref targets a tag, not a commit — and
  a submodule gitlink is a tree entry targeting a commit; storing these as implicitly commit-typed
  is a latent data-model bug. It's first-class in the IR because it changes the relation's **arity**
  in the type system (`to: OneOf<[EntityRef]>`), not a feature beside it — so retrofitting it later
  would be a breaking catalog-format + codec-API change, whereas modeling it now is one enum variant
  every v1 codec handles with a `len == 1` fast path. Implemented minimally (the `(type, key)`
  column pair, § 3/§ 6); codecs choose a richer strategy later (§ projection table).

**Validation rules the catalog enforces** (not per-codec surprises):
- A composition target belongs to exactly one parent relation and may not be the target of any
  association or hold its own `@key` visibility outside the aggregate (DDD: aggregate internals
  are not externally referenced; physically: an embedded json child can't receive an FK).
- Edge-property structs are value structs (no `@entity`, no relations of their own).
- Every polymorphic target set is closed and known at catalog build time.

**The key idea:** the modeler declares *intent* (cardinality + kind + edge props); each **codec
decides the physical shape**. That keeps relational, document, and graph as *equal* projections
instead of one being primary. (Honest scope note: they are equal projections *for writes*. Read
patterns are dictated by the physical choice each codec made, and fluessig deliberately offers no
query layer — the pitch is "one model, many write targets.")

### Layer C — the change layer (how data moves)

Layers A and B describe what data *is*; Layer C describes how it *changes*: a fixed vocabulary of
operations (`Insert` / `Upsert` / `Delete`), a unit of atomic *intent* (the **Transaction**), and
the sink's honest answer on atomicity (a **Plan** of atomic **Steps**). Unlike A and B it is
**not authored** — nothing about it appears in `.tsp`; it is fluessig-defined and merely *typed
by* the catalog (an op is always "on this entity, addressed by its `@key`"). Conceptually it is a
typed, Arrow-native CDC envelope (the Debezium op/after-image model, minus the JSON), and it is
the actual product surface: every source targets Layer C, every sink implements it. Full
contract: §5.

### How the one model projects to every store

| Model construct | Relational (PG/SQLite/Duck) | Document (Mongo) | Graph (neo4j) | Columnar (Arrow/Parquet) | Proto |
|---|---|---|---|---|---|
| Entity | table | collection | node label | RecordBatch schema | message |
| Scalar field | typed column | field | property | typed column | scalar field |
| Struct / List / Map | `json`/`jsonb` column | native nesting | property / sub-node | native nested | nested / `repeated` |
| Key | PRIMARY KEY | `_id` | node key | (none) | — |
| Association to-one | FK column | stored id | edge | id column | id field |
| Association to-many | FK on other side | array of ids | edges | — | repeated id |
| ManyToMany / edge props | association table | array / bridge doc | **edge with properties** | own batch | bridge message |
| Composition | child table *or* embedded json | **embedded subdoc/array** | sub-node + edge | nested | nested message |
| Polymorphic association | `(target_type, target_key)` column pair (no FK) *or* single-table w/ discriminator | type-tagged ref `{t, id}` | edge (target label free) | type + key columns | oneof of ids |
| Inheritance / abstract | per-type tables (v1) | discriminator field | labels | per-type batches | — |

Columnar/wire formats only see Layer A (records); relations become plain id columns or their own
batches — a Parquet file isn't a graph, and that's fine.

### Prior art we're standing on (the model)

- **Entity–Relationship model** (Chen, 1976) — the conceptual layer above relational.
- **Gel/ESDL + its formal paper** (arXiv:2507.16089) — links, link properties, cardinality
  keywords, abstract types: the semantic checklist for our decorators.
- **TypeQL / PERA** — relations with roles; edge properties as plain ownership on the relation.
- **DDD aggregates** — the composition/association (embed-vs-reference) boundary.
- **Property graph / openCypher / ISO GQL** — nodes + edges + properties.
- **Avro / Arrow logical types** — the semantic-over-physical scalar taxonomy.

---

## 3. The schema language (TypeSpec + `@fluessig/typespec`)

A catalog is a TypeSpec package: `main.tsp` (+ imports — real modules, one of SDL's missing
pieces). Models take one of three roles: **entities** (`@entity`, must have a key), **value
structs** (plain models — embeddable records without identity), and **edge-property structs**
(plain models referenced by `@edge`). Example (entl's own tables):

```typespec
import "@fluessig/typespec";
using Fluessig;

@catalog("entl", version: 0)
namespace Entl;

/** a git object id */
scalar Oid extends bytes;

/** value struct: no identity, embeds wherever used */
model Signature {
  name: string;
  email: string;
  when?: utcDateTime;
  tzOffsetMinutes: int16;
}

@entity
model Commit {
  @key oid: Oid;
  message: string;
  author: Signature;                    // struct → jsonb in PG, subdoc in Mongo
  committer: Signature;
  isMerge: boolean;

  @edge(CommitParent)                   // association to-many with edge properties
  parents: Commit[];                    // recursive; ordered via the edge struct
}

/** edge properties on Commit.parents */
model CommitParent {
  idx: uint8;
}

@entity
model GhPullRequest {
  @key id: int64;
  title: string;

  @compose                              // aggregate: embed in a doc store; child table elsewhere
  reviews: GhPrReview[];
}

@entity
model GhPrReview {
  @key id: int64;
  state: ReviewState;
  author?: string;
}

enum ReviewState { approved, changes_requested, commented, dismissed }
```

- **Cardinality is free** from the type: `Commit[]` = to-many, `author: GhUser` = to-one, `?` =
  nullable/optional (min 0). `@card(min, max)` refines ranges. Many-to-many vs one-to-many is
  declared, not inferred from an inverse's presence (Amplify's lesson: inference breaks silently
  when someone forgets the inverse field): `@assoc(many: true)` or an explicit `@inverse(Model.field)`
  — a **checked model-property reference**, not a string.
- **Relation kind** defaults to Association when a field's type is an entity; `@compose` flips it.
- **Edge properties** attach via `@edge(StructRef)` — a typed reference the compiler verifies.
- **Polymorphic targets** — **decided: the abstract-supertype route only** (`@entity @abstract
  model GitObject { @key oid: Oid } … model Blob extends GitObject`), the Gel/TypeQL pattern.
  Ad-hoc unions of entities (`target: Blob | Tree | Commit`) are rejected by the v1 validator with
  a "introduce a supertype" hint: the supertype names the family (so codecs *can* offer
  single-table projections later), guarantees a uniform key type across members (an ad-hoc union
  could mix an `Oid`-keyed and an `int64`-keyed entity, making the `(type, key)` reference pair
  untypeable), and makes membership one edit instead of n. Inheritance is restricted to this
  pattern: abstract roots, concrete leaves; only leaves are instantiable; no
  entity-extends-concrete-entity.
- **Docs** are TypeSpec doc comments; they flow into the catalog and every generated artifact.
- **Physical tuning lives in per-codec decorator namespaces** — the escape hatch every predecessor
  grew ad hoc, designed in from day one: `@index`, `@unique`, `@default` are core (portable);
  `@Pg.index(method: "gin")`, `@Pg.check("end_date > start_date")`, `@Mongo.shardKey`,
  `@Mongo.validatorLevel` are namespaced, carried opaquely in the catalog, and interpreted only by
  their codec. `@Pg.check` is *deliberately honest* about being raw dialect SQL rather than
  pretending a string mini-DSL is portable. The core IR stays clean forever.
- **Naming** — **decided: the core stays dumb.** lowerCamel field → snake_case column, model →
  snake_case *singular* table; no pluralization heuristics (English-only magic is a long-term
  liability). `@name("…")` overrides at any level — entl writes `@name("commits")` etc., making
  parity explicit rather than inferred. Codecs handle quoting, reserved words, and identifier
  length limits (PG's 63 bytes) with deterministic truncation + hash.
- **Versioned twice.** `@catalog(name, version)` stamps the *user's* schema; the emitter stamps
  the *catalog format* version + emitter/compiler versions into `catalog.json`. Both are
  compatibility surfaces.
- **v1 keeps it lean:** inheritance is supported only as the abstract-supertype polymorphism
  pattern above (no field inheritance across entities yet — `model X extends Y` where Y is a
  plain struct is allowed as spread-style reuse; entity-extends-entity beyond `@abstract` roots
  is rejected by the validator). Expressions (checks, computed fields, non-literal defaults) are
  deferred; the reserved slot is the per-dialect raw decorators.

---

## 4. Architecture — IR at the center, everything else a codec

```
 AUTHORING (Node, once)          INTERCHANGE            CORE (Rust)                BACK-ENDS
┌──────────────────────┐   ┌──────────────────┐   ┌──────────────────────┐   ┌──────────────────────────────┐
│ schema.tsp            │   │                  │   │ Catalog (IR)         │──▶│ SQL DDL (pg / sqlite / duck) │
│  + @fluessig/typespec │──▶│  catalog.json    │──▶│  Entity              │──▶│ Mongo (validator + _id)      │
│  tsp compile          │   │  (versioned,     │   │   ├ Field · Type     │──▶│ ORM (SQLAlchemy / Drizzle)   │
│  @fluessig/emitter    │   │   fully resolved)│   │   └ Key              │──▶│ Mermaid erDiagram · docs     │
└──────────────────────┘   │                  │   │  Relation            │──▶│ Proto (.proto)               │
 OTHER FRONT-ENDS          │                  │   │   ├ to (1..n) · card │   └──────────────────────────────┘
  arrow::Schema import ───▶│                  │   │   ├ kind · props     │    DATA BACK-ENDS (move rows)
  Rust builder ───────────▶│                  │   │   └ ends · inverse   │──▶ INSERTs · Mongo docs · JSONL · Parquet
 DATA (always Arrow)       └──────────────────┘   │  Validation          │
  Transaction {Mutation…} ──────────────────────▶│                      │
                                                  └──────────────────────┘
```

- **The TypeSpec emitter is a minimal catalog *printer***: walk the checked program, serialize
  types + fluessig decorator state verbatim, exit. It contains **no validation, no naming policy,
  no projection logic** — if a rule can live in Rust, it does. All *semantic* validation
  (composition rules, key rules, relation-target checks, supertype rules) lives in the Rust
  catalog loader, so every front-end — emitter, Arrow import, builder — passes through the same
  validator. Diagnostics carry source spans from the catalog so errors point at `.tsp` lines.

  *Alternative considered and rejected: doing schema back-ends as native TypeSpec emitters (all-TS
  schema side, Rust for data only).* Tempting — it inherits the emitter framework, tooling, and an
  npm contribution surface. It founders on one fact: the data runtime must load the schema
  regardless (Rust cannot walk TypeSpec's in-memory type graph), so a serialized catalog, a Rust
  loader, and a Rust Layer-A type mirror exist in *any* architecture — all-TS wouldn't remove
  them, only relocate schema generation. Worse, it splits a single projection decision across two
  languages: the DDL generator and the data marshaller must agree exactly on names (incl.
  truncation at PG's 63-byte limit), physical types, key column order, and `(type, key)` pair
  layout — split, that agreement needs either duplicated rules held together by a conformance
  corpus, or a fattened per-target "physical catalog" handoff. Colocating both halves in Rust
  makes the agreement **by construction**. Cost accepted: schema-codec plugins are Rust
  (`trait Codec`), not npm emitters, and the TypeSpec ecosystem contributes the language, compiler,
  and LSP — not back-ends.
- **`catalog.json` is internal** (decided): emitter and Rust core release in lockstep and the
  format may churn freely; it stays versioned as cheap insurance, but carries no public-compat
  promise. Going public later is a one-way door we can open, not one we must hold open now.
- **The Arrow import** (`Catalog::from_arrow`) maps `arrow::Schema` (+ field metadata) into
  Layer-A entities and can *emit a starter `.tsp`* so Arrow users get a scaffold. Import is
  documented-lossy: types outside our enum (dictionary, f16, …) map by rule or fail loudly; the
  field-metadata key convention (`fluessig.type = "Oid"`) is what makes `Entity → arrow::Schema →
  Entity` round-trip on the Layer-A subset.
- **Schema back-ends** are pure functions `&Catalog -> Artifacts`. Stateless: `CREATE TABLE IF NOT
  EXISTS`; rebuild = drop + re-emit. Every SQL DDL artifact also creates `_fluessig_meta(catalog
  hash, versions, generated_at)` and the generator exposes the expected hash, so a caller can
  cheaply detect "schema drifted → rebuild required" instead of `IF NOT EXISTS` silently keeping a
  stale shape. (Mongo: a `_fluessig_meta` document; files: a sidecar/footer key.)
- **Data back-ends** implement the change-batch contract (§5). entl's `driver.rs` is the SQL one,
  retrofitted to read `entity.key` from the IR instead of `parse_pk`-ing SQL.

Sync and I/O-free — returns text/plans; the caller executes them (exactly how the driver sink
already streams `{sql, params}` to a host).

---

## 5. The data plane — Layer C in full

This is the differentiated half of fluessig and is specified at the same level as the model.

```
Mutation {
  entity:  EntityRef,
  op:      Insert | Upsert | Delete,     // one op per batch; no partial Update in v1 — Upsert covers it
  rows:    RecordBatch,                  // Arrow, always: after-images (key columns only, for Delete)
}

Transaction { mutations: Vec<Mutation> } // the unit of atomic INTENT — what must land together

Plan {
  steps: Vec<Step>,                      // each Step = one atomic unit of sink CAPABILITY
}
Step { statements: Vec<Statement> }      // Statement = {sql, params} | BsonDoc | Bytes…
```

- **A Mutation is *not* a transaction — it goes *in* one.** A Mutation is a unit of typed input:
  one entity, one op, n rows. Atomicity lives one level up: the caller assembles whatever must
  land together — a source transaction, one aggregate (parent mutation + composition-child
  mutation + edge mutations, under the flat encoding below), a poll cycle — into a
  **Transaction**, and the runtime compiles one Transaction into one Plan.
- **Steps are the sink's honest answer to the Transaction's intent.** A Step is one atomic unit
  for the caller to execute (one `BEGIN…COMMIT`, one Mongo session transaction / bulkWrite, one
  file write). A fully transactional sink yields a **one-step Plan** — the input Transaction maps
  to exactly one sink transaction; weaker sinks yield more steps, and every codec declares a
  **capability profile** (ops supported × atomic scope: statement / entity / transaction / none),
  checked at plan time, so "this sink can't do that" surfaces as an error, never as silent
  partiality. One invariant holds regardless of capability: a composition aggregate, and an edge
  with its endpoints, are never split across steps.
  *(v1 posture: profiles are built out as codecs need them — SQL starts one-step-transactional,
  append-only files reject Upsert/Delete — and the negotiation generalizes incrementally, not up front.)*
- **State, not events — and after-images only, by argument, not accident.** Layer C carries keyed
  *after-images* (bare keys for deletes) — the CDC/state-transfer model, not domain events.
  Before-images are excluded from the baseline because a source-supplied before-image is a claim
  about *the sink's* state that the source cannot guarantee: under at-least-once delivery,
  replays, and cross-Transaction reordering, "before" is only valid relative to a sink state the
  source never observes. The one authoritative before-image lives in the sink itself — where SQL
  triggers get `OLD` for free — so anything whose *correctness* needs it (stored derived values,
  §9.3) is maintained sink-side, never in the stateless runtime. A design consequence, not a
  hardship: sources like entl's (scan-a-repo snapshots) often *have* no diff to offer. Reserved:
  an optional, additive `before: RecordBatch` slot on Mutation may arrive later as an
  *optimization hint* (minimal partial updates, delete verification) — correctness will never
  depend on it.
- **Delivery semantics.** At-least-once delivery is the assumed baseline: `Upsert` and `Delete`
  are idempotent by construction, so replaying a Transaction is safe. `Insert` (fail on key
  conflict) is for callers who can guarantee exactly-once.

- **Operations.** The source stream carries inserts, updates, and deletes (entl does); the
  contract maps them to three ops. `Insert` is plain append (fails on key conflict). `Upsert` is
  insert-or-replace-by-key (`ON CONFLICT (key) DO UPDATE` / Mongo `replaceOne(upsert)`) — source
  *updates* map to `Upsert` in v1 (full-row replace; a partial `Update` op with a column mask is
  deferred), and `Upsert` is the recommended default for change-stream replay because it makes
  re-delivery idempotent. `Delete` batches carry **key columns only**; deleting a composition
  parent deletes the aggregate — projected as `ON DELETE CASCADE` on composition child tables in
  SQL (cascade *is* the lifecycle semantics of composition; associations never cascade), and as
  deleting the document in stores that embedded the children. Append-only sinks (Parquet/JSONL)
  reject Upsert/Delete in v1 with a clear error rather than pretending.
- **Association data** rides in the batch as plain key-typed columns (FK values); a polymorphic
  association is a `(type, key)` column pair using catalog-defined type tags. The runtime
  validates FK column types against the target entity's key type.
- **Composition data** — **decided: flat is canonical.** Children arrive as their own Mutation
  carrying a parent-key column, exactly like associations and edges — *everything* entering the
  runtime is a flat batch of rows with key columns. Rationale: flat is what Arrow is good at (no
  deep `List<Struct>` assembly at the source), what SQL/graph/columnar want directly, what real
  sources emit anyway (git plumbing, paginated APIs), and it streams — nested would force sources
  to buffer whole aggregates. The one codec that wants nesting (documents) performs **aggregate
  assembly** itself (group children by parent key at marshal time); that cost is contained in one
  codec instead of taxing every source. Nested input can be added later as a normalize step
  without changing the contract.
- **Edge-property relations**: the edge is its own Mutation (`commit_parents`: from-key, to-key,
  props) — symmetric with how every codec ultimately stores it.
- **Ordering & atomicity.** The runtime topologically orders statements within a Plan by
  association dependencies (referenced-before-referencing on insert, reversed on delete) and
  places a composition aggregate in one Step. Steps are the caller's transaction boundaries;
  fluessig never opens connections. Ordering is best-effort hygiene, not the integrity
  mechanism — a change stream can deliver referencing rows in an earlier *batch* than their
  targets, which no intra-Plan ordering can fix; integrity across batches is governed by the FK
  policy (§6) and ultimately by the source (sinks are rebuildable projections, §8).
- **Schema conformance & coercion.** Strict by default: batch schema must match the entity's
  Layer-A projection. A `lenient` mode permits a documented whitelist only: lossless numeric
  widening, Utf8↔LargeUtf8, timestamp unit rescaling, absent nullable columns → null. Extra
  columns: error (strict) / ignored-with-warning (lenient). Never silent truncation.
- **Throughput posture.** Columnar in, row-oriented statements out is the unavoidable pivot for
  SQL/Mongo; the runtime amortizes it (multi-row `INSERT ... VALUES` chunks, prepared-statement
  reuse via stable statement shapes per entity, `COPY` as a fast path where the caller opts in).
  For columnar sinks there is no pivot at all — RecordBatch in, Parquet out.

---

## 6. What v1 models vs. implements

The **model** (§2) is the full entity graph — types are designed so Mongo-embed, neo4j-edge, and
polymorphic targets slot in *without reshaping the IR later*.

**v1's mission (decided): bindgen + models, replacing entl's hand-written surface.** entl's schema
templates, generated read planes (`tables.gen.ts` / `models.py`), and hand-written binding glue
(the napi/PyO3/Magnus `lib.rs` files) get ripped out and regenerated from one `entl.tsp`. So v1 is
**two verticals off one authored document**: the *models* vertical (catalog → SQL DDL + data codec
+ the entl-consumed ORM read planes) and the *bindgen* vertical (api layer → generated binding glue
over one hand-implemented core trait — spike-proven, see `spike/` and `/translation.md`; each
language's existing test suite is bindgen's parity gate, exactly as the DDL templates are for
models). Mongo / graph / further ORM targets remain post-v1 fan-out. The first implementation
exercises the SQL vertical:

- v1 **models**: entities, scalar + nested fields, keys (incl. composite), relations (cardinality
  ranges + Association/Composition + edge properties + polymorphic targets + named ends).
- v1 **implements** (the spine): the decorator lib + emitter → `catalog.json`; the Rust
  loader/validator (incl. abstract-supertype families); the Arrow import; the SQL DDL back-end
  incl. `_fluessig_meta`; the SQL data codec with the full §5 contract; **edge-property
  association tables** (entl parity requires `commit_parents(commit, parent, idx)`, and the git
  stress test showed edge props are the common case, not the exotic one — deferring them would
  contradict the dogfood); **composite keys** (cheap in SQL — `PRIMARY KEY (a, b)`; key order =
  declaration order; the Mongo policy is fixed *now* so `@key` never overpromises: a composite key
  projects to a **compound `_id` embedded document** with canonical field order — never a
  synthesized/hashed key, which would break the transparency of upsert-by-key); polymorphic
  associations to supertype families as the `(type, key)` column-pair strategy (per-concrete-type
  tables in v1; single-table projection is a later per-codec opt-in); and the **extras mechanism**
  (§9.5) — fingerprinted raw-DDL append, required for entl parity; and a **minimal `@derived`**
  (§9.3's `exists`/`count` slice, virtual projection only) so entl's `isMerge` is a real derived
  field rather than source-computed.

  **SQL FK policy — provisional, decide during the SQL codec (build step 4).** The analysis so
  far: FKs are projections of association intent, not the integrity mechanism — sinks are
  rebuildable projections fed by streams that may deliver across-batch out of order, which no
  constraint timing fully survives. Candidate defaults: *acyclic* associations get real FKs,
  emitted `DEFERRABLE INITIALLY DEFERRED` where the dialect supports it (PG; MySQL cannot defer);
  relations on a *cycle* in the association graph — including self-referential ones like
  `Commit.parents` — get key-typed columns + indexes but **no `REFERENCES` clause**; *composition*
  child tables always get an enforced FK with `ON DELETE CASCADE` (the aggregate ships together in
  one Step, so ordering is guaranteed). Per-relation override: `@Pg.fk(enforced |
  deferred | none)`. Later, a `--checks` artifact can emit integrity *queries* (the LinkML
  SQLValidationGenerator idea) so callers can audit what constraints don't enforce. Nothing here
  is load-bearing for the IR — the decision can wait for entl parity data (what does the current
  DDL do?) without blocking anything upstream.
- v1 **defers** (but must not preclude): Mongo, ORM codegen, Proto, graph, n-ary relations
  (`ends` reserves the slot), entity inheritance beyond abstract polymorphism roots, the
  **derived-artifact family** (§9.1 views, §9.2 `@closure`, §9.4 derived to-one, and `@derived`
  beyond `isMerge`'s `exists`/`count` slice — specced so the IR *reserves* (not freezes) the slots),
  expressions beyond that family (checks/computed/non-literal
  defaults — the per-dialect raw decorators and §9.5 extras are the interim slot), Union-type
  projection beyond json.

## 7. Build order — spine first, then fan out

0. **`@fluessig/typespec` + `@fluessig/emitter`** (TS, thin) — decorators + checked-program walk →
   `catalog.json`. Freeze the catalog format with a JSON Schema + fixture corpus.
1. **IR + loader** (Rust) — `Catalog / Entity / Field / Type / Key / Relation`; all semantic
   validation; span-carrying diagnostics. `fluessig validate`.
2. **Arrow front-end** — `arrow::Schema (+ metadata) -> Entity` and back (Layer-A round-trip,
   documented-lossy import). `fluessig import-arrow` emits a starter `.tsp`.
3. **SQL DDL back-end** — Postgres first; reproduce entl's current `commits` / `commit_parents` /
   `gh_*` templates *exactly* (incl. the edge table) and assert parity before anything is deleted;
   emit `_fluessig_meta`.
4. **SQL data codec** — implement §5 (Insert/Upsert/Delete, grouping, ordering, coercion); port
   entl `driver.rs` to the IR (delete `parse_pk`).
5. **Prove the vertical** end-to-end against a live store (reuse the PGlite test); wire entl-core
   to fluessig and keep entl's node/python tests green (the real dogfood).
6. *Then* fan out as independent codecs against the frozen IR: SQLite/DuckDB DDL → **Mermaid
   erDiagram** (cheap, high-leverage docs win) → ORM codegen → Mongo (first real test of
   Composition + `(type,key)` refs) → JSONL/Parquet → Proto → graph.

**Testing & conformance (cross-cutting, not a phase).** fluessig is tested three ways, mirroring
entl-testkit: (a) **static / unit tests** — the loader + validator (accept valid catalogs, reject
each rule violation with the *right* error) and per-codec **golden outputs** (a fixed catalog → the
exact `catalog.json` / DDL / models); (b) **property tests (proptest)** — generate random *valid*
catalogs + random Arrow batches and assert invariants: `arrow-import → catalog → arrow` round-trips
on the Layer-A subset, and `Transaction → Plan → execute → read-back` equals the input for
transactional sinks; (c) a **cross-codec conformance corpus** — one catalog run through every codec,
so a new codec proves itself against shared fixtures. The **entl dogfood** (real repos, incl.
annotated tags exercising polymorphism) is the top-level property test.

## 8. Non-goals (v1)

- No schema **diffing / ALTER / migration history** in v1 — but migration is a tiered *roadmap*,
  not a permanent non-goal. **Tier 1 (v1):** the fingerprint-driven **drop-and-recreate** flow
  *is* the simple migration engine — detect drift via `_fluessig_meta`, rebuild, replay the
  stream. Corollary, stated openly: v1 sinks are treated as **rebuildable projections** of
  re-derivable data (exactly entl's change-stream case) — fluessig v1 is not for sinks that are
  the system of record through a schema change. **Tier 2 (with ORM codegen):** where an ORM is
  the target, fluessig emits models and **defers to that ORM's migration story** (Alembic,
  drizzle-kit, …) rather than competing with it. **Tier 3 (later):** a real diff/ALTER engine
  against the catalog, for system-of-record sinks.
- No **query** layer, no API server, no connection management, no runtime DB reflection.
- No expression language (constraints/computed/derived); per-dialect raw decorators are the
  honest escape hatch.
- No n-ary relations (binary + bridge entities; IR reserves `ends`).
- Graph/Mongo are **design constraints** (the model must not preclude them), not v1 deliverables.

## 9. Derived artifacts — specced now, built later (entl will need them)

Motivating evidence: entl's `migrations/duckdb/extras.sql` — recursive graph macros
(`ancestors`, `first_parent_chain`), an oid/hex readability layer, ~19 secondary indexes — the
file's own comment calls it "what the drizzle schema can't express." Decomposed against the
catalog, most of it turns out to be *model-derivable*. The organizing test:

> **A derivation belongs in the schema when it is a function of the catalog's structure. It
> becomes a query the moment it needs a free-form expression — and then it belongs in extras
> (§9.5) or in the consumer's code.**

Everything below is post-v1 and **non-binding on the IR until built** (customer #2/#3 may reshape
it) — *except* two v1 carve-outs: §9.5 **extras** (entl parity) and a **minimal slice of §9.3
`@derived`** (just enough for entl's `isMerge`). The rest is specced now only so the decorator
vocabulary and IR *reserve* the slots, not freeze them.

### 9.1 Representation views — zero annotation

A bytes-carrier semantic scalar (`scalar Oid extends bytes`) already tells a codec everything a
readable layer needs. The DuckDB codec's physical choice is "BLOB for memcmp joins + small
indexes, hex lazily" — and once that choice exists, the layer is mechanical: for every entity with
such columns, emit a `<table>_hex` view projecting each through `lower(hex(col))`, plus a per-
scalar input helper (`CREATE MACRO oid(h) AS unhex(h)`). Codec flag (`--views readable`), no
schema annotation. Generalizes to any semantic scalar with a canonical text form (`Uuid`, …).
This is Layer A's semantic/physical split paying rent: no other tool retains the distinction the
view is bridging.

### 9.2 Relation closures — `@closure`

Both entl macros are instances of one abstraction: **transitive closure over a self-referential
association, optionally filtered by literal equality on declared edge properties**. That is a
function of the model — the moment the catalog holds `parents: Commit[]` with edge struct
`CommitParent { idx }`, the ancestry DAG and its first-parent spine exist; the macros are just
DuckDB's spelling.

```typespec
@edge(CommitParent)
@closure("ancestors")                              // full reachability
@closure("first_parent_chain", along: #{idx: 0})   // walk only idx=0 edges
parents: Commit[];
```

Admissibility rule (same as everywhere in fluessig): `along` filters are literal equalities on
checked edge-property references — no expression language. Projections: DuckDB → `CREATE OR
REPLACE MACRO … AS TABLE WITH RECURSIVE …`; Postgres → a `RETURNS TABLE` function, same CTE;
SQLite → recursive view; **Cypher → nothing** (`-[:PARENT*]->` is native — a derived thing that is
*free* in one paradigm is a property of the model, confirming the abstraction level); ORM codegen
→ a helper method wrapping the CTE. Generated helpers take a start *key* plus an optional depth
bound; entl's start-from-a-ref-name is a one-line consumer wrapper (resolving a ref first is
composition, not closure).

### 9.3 Derived aggregate fields — `@derived` (the closed family)

The grammar — one aggregate, one declared relation hop, literal-equality filters only:

```
Derived = Agg( relation-of-E , of: field? , where: {field = literal, …}? )
Agg     = count | exists | notExists | min | max | sum        (avg = sugar, if ever)
```

```typespec
@derived(count, of: Commit.parents)                      parentCount: uint32;
@derived(exists, of: Commit.parents, where: #{idx: 1})   isMerge: boolean;   // ≥2 parents, no operators needed
@derived(max, of: GhPrReview.submittedAt)                lastReviewAt?: utcDateTime;
@derived(exists, of: GhPullRequest.reviews, where: #{state: ReviewState.approved})
                                                         hasApproval: boolean;
```

Why this family is closed (the four properties): **statically typeable** (`count → uint`,
`exists → boolean`, `min/max/sum →` type of `of`); **nullability derivable and enforced** (`count`
never null; `min/max` over a possibly-empty relation are nullable — the validator requires the
`?`); **idiomatically projectable everywhere** (SQL view; Mongo `$size`/`$max`; Cypher
`size((c)-[:PARENT]->())`; Gel shipped `:= count(.parents)` as a headline feature); and
**incrementally maintainable exactly where theory says** (self-maintainable views, Gupta–Mumick):
count/exists/sum are delta-maintainable on Insert, need the old value on Delete; min/max break on
delete-of-extremum; Upsert needs a before-image in every case.

**Maintenance doctrine** (a corollary of Layer C's after-images-only, §5 — no discretion left):
the stateless runtime **never computes stored derived values**; whatever maintains them lives
where the authoritative before-image lives — the sink. Default projection is **virtual**: a
`<table>_derived` SQL view (or folded into §9.1's readable views), Mongo at read time, Cypher
inline, Parquet materialized at export. Opt-in `stored: true` → the codec emits sink-side
maintenance (SQL triggers, where `OLD` is native and the whole matrix goes green); codecs that
can't honor `stored` refuse via the Layer C capability profile. One pleasing interaction with an
existing decision: for **composition** relations, the Step invariant co-locates parent and
children at marshal time, so document codecs may compute composition-derived fields during
aggregate assembly and store them *for free, correctly* — association-derived fields never get
that luxury. The embed-vs-reference boundary turns out to also be the boundary of cheap
write-time derivation.

**The boundary, precisely:** in the family iff *one aggregate from the fixed set, over one
declared relation (or declared `@closure` — then virtual-only, since transitively
non-maintainable), filtered only by literal equality on declared fields*. Out: same-row
arithmetic (`endDate - startDate` → `@Pg.generated(…)` escape hatch), multi-hop aggregates,
any comparison or operator. If one more operator is ever admitted, it is `atLeast(k)` — a single
literal — and it is the last one.

**v1 carve-out (`isMerge`).** `isMerge` ships as a *real* derived field —
`@derived(exists, of: parents, where: #{idx: 1})` — projected as the virtual default (a
`<table>_derived` view, or folded into §9.1's readable views), replacing the source-computed flag.
This is the minimum slice we build in v1: the `exists`/`count` aggregates with literal-equality
filters, **virtual projection only** (no `stored`/trigger maintenance yet). The rest of the family
lands post-v1.

### 9.4 Derived to-one restrictions

Same grammar, a restriction instead of an aggregate:

```typespec
@derivedRelation(of: Commit.parents, where: #{idx: 0})
firstParent?: Commit;
```

Cheap, obviously useful to entl (`git log --first-parent` joins), and specified alongside §9.3 so
the two share admissibility rules.

### 9.5 Extras — the formalized escape hatch (**v1**)

Whatever survives 9.1–9.4 is genuinely dialect-specific, and the design is to make the extras
file a first-class input rather than a side door: `@Duck.extras("./extras.duckdb.sql")` at
catalog level (or `--extras`), appended after generated DDL on every rebuild, and **hashed into
the `_fluessig_meta` fingerprint** — editing extras trips drift detection, so "rebuild required"
stays truthful across the whole DDL surface, generated and raw alike. This ships in v1: entl
parity depends on it.

Scorecard for entl's actual file: hex layer → §9.1, zero annotation; both macros → §9.2, fully
portable; the 19 indexes → core `@index` / `@Duck.index`; residue → §9.5. Three of four buckets
model-derivable.

## 10. Prior art & competitive landscape

The scan (2026-07, updated after the language spike) confirms the *pattern* is proven but the
*specific bundle* is open.

**Schema-language substrates and quarries:**

- **TypeSpec** (MIT) — chosen substrate; layered design and emitter API are exactly the
  protoc-style consumption model we need; Azure teams already use custom decorators + custom
  emitters at scale.
- **Gel / EdgeDB** (Apache 2.0; company winding down — team → Vercel early 2026, product stays OSS;
  formal model paper arXiv:2507.16089) — the best link/link-property/cardinality semantics shipped; our decorator
  vocabulary is specified against it. Its post-mortem independently endorses fluessig's exact
  slice (declarative language-agnostic schema as source of truth = "good idea") and its cause of
  death (building a whole database + protocol + cloud) is precisely what our non-goals exclude.
- **TypeQL / TypeDB** (MPL 2.0, native Rust/pest parser) — strongest relationship layer (roles,
  edge props as ownership, `@card`); no record layer (no structs/unions/bytes). Quarry for
  relation semantics; rejected as substrate.
- **GraphQL-SDL-directive systems** — Amplify/AppSync (`@hasMany`…→DynamoDB), Neo4j GraphQL
  (`@relationship(properties:)`→Cypher; origin of our edge-prop shape), Dgraph, Fauna (RIP):
  proof the annotate-a-schema→storage mechanism works in production, and the cautionary tale
  (string mini-DSLs) our tier-3 analysis is built on.
- **Prisma** — bespoke DSL → migrations + client for SQL + Mongo; the multi-store crown, but its
  own runtime, no graph, no Arrow, no embeddability.
- **LinkML** — nearest schema-side neighbor: authored YAML metamodel → SQL DDL/JSON
  Schema/Pydantic/SQLAlchemy/RDF + convenience data loading; its `inlined` is our
  Composition. Python-centric, no Arrow, data path is convenience-grade.
- **DBML / DbSchema / Hackolade** — relational-core DSL / GUI modelers; diagrams we get from a
  Mermaid codec instead.

**Data-plane neighbors (the other half):**

- **dlt** — the "feed it data, it sinks automatically" experience, many destinations — but schema
  is *inferred from data*, no conceptual model, relational-flatten only, Python-only.
- **Kafka Connect + Schema Registry / Airbyte / Meltano / Estuary Flow / Redpanda Connect** —
  pipeline platforms: many sinks, registry- or inference-governed schemas, running services rather
  than embeddable libraries.
- **ADBC / Arrow Flight SQL** — Arrow↔DB *connectivity*; no schema generation, no marshalling
  semantics. Confirms the Arrow leg is unoccupied.
- **Atlas** — declarative schema→DDL done right, HCL/SQL-defined and relational-only; a bar for
  our DDL quality, not a competitor to the model.

**The gap fluessig fills:** a single, **vendor-neutral**, authored-**TypeSpec**-sourced tool that
treats **relational + document + graph as equal write projections**, *plus* multi-language **ORM
codegen**, *plus* a high-throughput **Arrow marshalling runtime** with a specified change-batch
contract. Every incumbent covers one axis: authored conceptual schema (LinkML, Prisma, Gel†) *or*
automatic multi-destination sinking (dlt, Connect, Estuary) — none has both in one embeddable
library. Roughly: *LinkML's model rigor, dlt's sink ergonomics, Arrow's throughput, Gel's
semantics, TypeSpec's grammar — as a library.*

## 11. Decision log & remaining opens

Resolved:

1. **Composition input encoding** → flat child batches with a parent-key column are canonical;
   document codecs do aggregate assembly (§5).
2. **Ops** → the stream carries insert/update/delete; source updates map to `Upsert` (full-row
   replace) in v1; composition deletes cascade (§5).
3. **`catalog.json`** → internal: emitter and Rust core release in lockstep; the format stays
   versioned (cheap insurance) but carries no public-compat promise. Going public later is a
   one-way door we can open, not one we must hold open now.
4. **Authoring toolchain** → **v1:** `cargo install --git` for the `fluessig` binary + ambient Node
   for the `.tsp → catalog.json` step (fine for the dogfood). Bundling the TypeSpec compiler +
   catalog printer into one no-ambient-Node executable is a later **packaging milestone**, not a v1
   gate. Generator/runtime are Node-free consumers of the catalog regardless.
5. **Polymorphism** → abstract-supertype families only; ad-hoc entity unions rejected (§3).
6. **Naming** → dumb core: snake_case singular default, `@name` for everything else (§3).
7. **Composite keys** → in v1 IR + SQL; Mongo projects compound `_id` documents, never synthesized
   keys (§6).
8. **Emitter scope** → the TS side is a minimal catalog printer and nothing more; all validation,
   naming, physical layout, schema generation, and data marshalling live in Rust, so DDL and
   marshaller agree by construction (§4). All-TS schema emitters considered and rejected — the
   Rust catalog loader is unavoidable either way, so all-TS would only relocate schema generation
   while splitting projection decisions across two languages.
9. **Migrations** → tiered roadmap: v1 = fingerprint-driven drop-and-recreate; ORM-codegen targets
   defer to the ORM's own migration tooling; a real diff/ALTER engine later (§8).
10. **Change semantics are a modeled layer (Layer C)** → fixed, fluessig-defined, not authored in
    `.tsp`. Vocabulary: a **Mutation** (one entity, one op, n rows) goes in a **Transaction**
    (the unit of atomic intent), which compiles to a **Plan** of **Steps** (each Step one atomic
    unit the sink can actually guarantee), with per-codec capability profiles (§2, §5).
11. **After-images only** → a source-supplied before-image is a claim about sink state the source
    cannot guarantee (replays, reordering), so correctness never builds on it; the authoritative
    before-image lives in the sink, and sink-side machinery (triggers/`OLD`) is where stored
    derived values are maintained. An optional additive `before` slot is reserved as a pure
    optimization hint (§5).
12. **Derived artifacts** → specced in §9, built post-v1 except extras: representation views
    (zero annotation), `@closure`, the closed `@derived` aggregate family
    (virtual-by-default, `stored` opt-in with sink-side maintenance), derived to-one
    restrictions; **extras** formalized and fingerprinted **in v1** for entl parity; and a **minimal
    `@derived`** (the `exists`/`count` slice, virtual-only) ships in v1 so `isMerge` is a real
    derived field, not source-computed.
13. **Testing** → static/unit (loader + validator + per-codec golden outputs), proptest (random
    valid catalogs + Arrow batches → round-trip/invariants), and a cross-codec conformance corpus;
    entl (incl. annotated tags for polymorphism) is the top-level property test.
14. **v1 scope = bindgen + models, replacing entl** → entl's schema templates, generated read
    planes, and hand-written binding glue are the dogfood target and get removed in favor of
    fluessig output. Bindgen is a v1 spine vertical (plan.txt Step 5b), not fan-out: a Rust
    `fluessig bindgen` back-end consuming the op layer's `api.json`, with per-language idiom
    templates keyed on op shape (`@ctor`/unary/`@stream`/`@manual`) — proven in `spike/`; each
    binding's existing test suite is its parity gate.

Still open:

- **SQL FK policy** — deliberately deferred to build step 4, when entl's existing DDL and real
  stream ordering are in front of us. The provisional tiering (deferred FKs on acyclic
  associations / bare columns on cycles / cascade on compositions) is written up in §6 as the
  starting hypothesis, not a commitment. Nothing in the IR depends on the outcome.

- **ORM codegen mechanism**: in-crate templates per language vs. a `trait Codec` plugin boundary
  for out-of-tree codecs. (Leaning: a trait, built-ins as impls.)
- **Mongo aggregate assembly**: flat-canonical input (decision 1) means the Mongo codec must group
  child batches under parents at marshal time — define the assembly window/contract (children and
  parent co-delivered in one Plan? buffered by the caller?) before the Mongo codec starts.
- **Partial updates**: an `Update` op with a column mask, when a consumer needs
  cheaper-than-full-row-replace semantics.
- **Single-table inheritance** projection as a per-codec opt-in for supertype families
  (`@Pg.singleTable`), once a consumer wants git's actual objects-table shape.