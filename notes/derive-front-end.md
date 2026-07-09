<!-- straitjacket-allow-file:duplication — this doc quotes its companion sketch (entl_derive_sketch.rs) verbatim by design. -->
# fluessig — the Rust derive front end (design exploration)

Companion to [design.md](./design.md). **Status: exploration, not scheduled.** Nothing here
changes the v1 plan; the point of writing it down is that the catalog contract makes this
adoptable later without touching the core — and that the positioning question it raises
(§4) deserves a deliberate answer rather than a default.

The full acid test — all 28 entl tables + the complete op surface, translated — is checked
in beside this doc as [entl_derive_sketch.rs](./entl_derive_sketch.rs). Read it side by side
with [entl.tsp](../entl.tsp).

---

## 0. The question

design.md §1 fixes the authoring model: TypeSpec is the source of truth, and a Python/TS/Go
shop authors schemas with zero Rust. This doc explores the inverse positioning:

> People think in **Rust**. fluessig is a bolt-on, not a way of doing things you fit
> yourself into: **a normal Rust crate that happens to ship a library in every other
> language.**

Concretely: the user's ordinary Rust structs — the ones their ingest code actually
constructs — *are* the schema. A derive extracts the model; the same `catalog.json` /
`api.json` flow out; DDL, ORM read planes, and napi/pyo3/Magnus bindings are generated
exactly as today. No `.tsp`, no Node, no second copy of the model.

## 1. Why proc macros alone can't do it — and what can

The naive objection to "just use proc macros" stands: a proc macro emits Rust tokens into a
Rust compilation, and fluessig's outputs are almost entirely not Rust (DDL, `models.py`,
`tables.gen.ts`, Ruby surfaces, README quickstarts). Macros writing files to disk is a known
anti-pattern — non-hermetic, breaks incremental compilation, invisible to build systems.

But that objection only rules out proc macros as the *generator*. The proven pattern splits
the roles:

1. **Derive → descriptor.** The macro expands to a `&'static EntityDescriptor` — pure data
   (fields, keys, relations, doc comments, `file!()`/`line!()` spans). No runtime behavior,
   no files.
2. **Exporter → catalog.** A separate step (a tiny bin target, a `cargo fluessig`
   subcommand, or a generated `#[test]`) collects descriptors and writes `catalog.json` +
   `api.json` — the artifacts `fluessig-gen` already consumes, unchanged.

This is the protoc architecture with Rust as the IDL. The Rust front end is just another
catalog printer; design.md §4 explicitly reserved the slot ("other front-ends target the
same catalog").

**Prior art, by role:**

| Role | Precedent |
|---|---|
| derive captures shape, consumers interpret | serde (the archetype) |
| shape → out-of-language artifact | schemars (JSON Schema), ts-rs / specta (TypeScript), utoipa (OpenAPI) |
| Rust crate → polyglot native libraries | **uniffi** (Mozilla), diplomat (ICU4X), interoptopus |
| reflection substrate (option: build on, not roll) | bevy_reflect, facet |
| descriptor collection without a central list | inventory, linkme (see §2.8 for why we'd avoid both) |
| relation spelling in derive-land | sea-orm, diesel |

uniffi is the load-bearing precedent: "annotate a normal Rust crate, ship
Python/Kotlin/Swift/Ruby" is its exact pitch, proven in production. Nobody in that family
also does schema/DDL/data-plane. That gap is this design's wedge.

## 2. The design

### 2.1 Descriptors, not behavior

Every fluessig derive (`Entity`, `Record`, `Scalar`, `Enum`) expands to descriptor data
only. The one exception is the generated per-family key enums (§2.5), which are real types
because that's the feature. Descriptors carry source spans so that loader diagnostics —
which stay in the Rust core, exactly as today — point at `.rs` lines the way they point at
`.tsp` lines now.

### 2.2 Fields are columns; keys are types

The single biggest ergonomic result of the acid test: **most of `@fk` disappears.** In
TypeSpec, the model-level name and the column name are different things, so entl.tsp needs
`@fk(#["tree_oid"]) tree: Tree` on nearly every association. In Rust the field *is* the
column, and the key type carries the reference:

```rust
pub tree_oid: Id<Tree>,          // .tsp: @fk(#["tree_oid"]) tree: Tree;
pub author_id: Option<Id<GhUser>>,
```

`Id<T>` resolves through `<T as Entity>` — a typo'd or missing target is a **rustc** error
with rust-analyzer completion, which recovers most of the checked-reference advantage that
motivated choosing TypeSpec over GraphQL-SDL directives in the first place.

Composite keys fan one field out to several columns. The spelling is declared **once, on
the referenced entity**, not at every site (motivation and evidence in §2.5's option 1 —
the same mechanism serves plain composite FKs):

```rust
#[fluessig(name = "gh_pull_requests", extends = GhSubject,
           ref_cols(repo_id = "repo_id", number = "pr_number"))]
pub struct GhPullRequest { ... }

// all four referencing tables then write, with no per-site pin:
pub pr: Id<GhPullRequest>,       // → repo_id, pr_number
```

### 2.3 Inheritance = flatten

Rust has no `extends`. The family/inheritance pattern (design.md §3) is spelled as
embedding, with the derive recording family membership:

```rust
#[derive(Entity)]
#[fluessig(name = "commits", extends = GitObject)]
pub struct Commit {
    #[fluessig(flatten)]
    pub object: GitObject,       // contributes (oid, repo_id) first — column-order parity
    ...
}
```

FINDINGS #6 (abstract roots carry *only* their key, to preserve per-leaf column-order
parity) accidentally makes this cheap: the embedded root is two fields. The loader keeps
enforcing the same rules as today (abstract roots, concrete leaves, uniform key type).

### 2.4 Edges are row structs — and Layer C already agreed

TypeSpec puts the relation on the entity (`@edge(CommitParent) parents: Commit[]`); the
derive front end puts it on the edge table, which is its own honest row struct:

```rust
#[derive(Entity)]
#[fluessig(name = "commit_parents", edge(from = Commit, to = Commit, expose = "parents"))]
pub struct CommitParent {
    #[fluessig(key)] pub commit_oid: Id<Commit>,
    pub parent_oid: Id<Commit>,
    #[fluessig(key)] pub idx: i32,   // FINDINGS #3: edge PK = source key + local key
}
```

This is not a compromise forced by Rust — it converges on a decision already made: Layer
C's canonical input is flat batches, edges arriving as their own Mutation (design.md §5,
decision #1). The struct the schema declares is the batch the data plane receives. The
graph-level view (`Commit::parents`, used by `@derived`, `@closure`, ORM codegen, the
Mermaid codec) is reassembled by the catalog from `expose`.

Cost, stated honestly: the relationship story is distributed across edge structs instead of
readable on the entity. The catalog (and any docs codec) is where the centralized view
lives.

### 2.5 Polymorphism

The physical model is unchanged from design.md §2: a family (abstract root + closed leaf
set sharing one key shape) makes "reference to any member" the typeable column pair
**(type tag, key)** — `(entry_type, child_oid)` on tree_entries, `(subject_type, repo_id,
subject_number)` on gh_comments. Two directions exist in production: polymorphic *targets*
(a reference into a family) and polymorphic *sources* (an edge whose own identity is the
(type, key) pair — gh_labeled, gh_assignees).

A naive derive spelling stacks four costs onto every referencing site: the tag column name,
the key column fan-out names, legacy-parity renames, and column sharing. The first draft of
the sketch reproduced TypeSpec's density at exactly these sites. Three mechanisms remove it:

**(1) Family-declared reference spelling.** Where every site spells the reference
identically — true for all three GhSubject sites in entl — the spelling is a property of
the *family* and is declared once:

```rust
#[derive(Entity, Clone)]
#[fluessig(abstract_root(GhPullRequest, GhIssue),
           tag_col = "subject_type",
           tag_values(GhPullRequest = "pr", GhIssue = "issue"))]
pub struct GhSubject {
    #[fluessig(key)]                              pub repo_id: Id<Repo>,
    #[fluessig(key, ref_col = "subject_number")]  pub number: i32,
}
```

Referencing sites become bare fields. A per-site `cols(tag = …, key = …)` override remains
for families whose spelling genuinely varies by site — GitObject is referenced as
`entry_type`/`child_oid` on tree entries but would be `target_type`/`target_oid` on the
FINDINGS #7 refs upgrade — so tree_entries keeps exactly one pin, and it's a legacy-DDL
fact, not a language failure. Greenfield schemas take defaults (`{field}_type`,
`{field}_{keycol}`) and write nothing.

**(2) Generated key enums.** `abstract_root(Commit, Tree, Blob)` names the closed leaf set
— design.md's "every polymorphic target set is closed and known at catalog build time"
rule, made syntactic, and the thing that lets a single macro expansion generate code — and
emits a real sum type per family:

```rust
pub enum GitObjectId  { Commit(Oid), Tree(Oid), Blob(Oid) }
pub enum GhSubjectId  { GhPullRequest(Id<Repo>, i32), GhIssue(Id<Repo>, i32) }
```

Ingest code constructs `GitObjectId::Blob(oid)`: the tag can never disagree with the key,
`match` works, and adding a leaf is one edit that exhaustiveness-checks every construction
site. This is the strongest "think in Rust" moment in the design — the polymorphic
reference stops being a fluessig concept to learn and becomes an enum the user already
understands. The leaf list is declared on the root *and* each leaf declares `extends`; the
loader validates the two agree.

**(3) `shares(col)` — column sharing as a declared fact.** gh_labeled's label FK
`(repo_id, label_name)` reuses the subject's `repo_id`. Rather than a spelling coincidence
the loader silently dedups, the sharing is declared — which also states the real
constraint (the label must belong to the *same repo* as the subject):

```rust
pub struct GhLabeled {
    #[fluessig(key)]                   pub subject: GhSubjectId,
    #[fluessig(key, shares(repo_id))]  pub label: Id<GhLabel>,  // → repo_id (shared), label_name
}
```

**Escape hatch.** For anything still too contorted: raw scalar columns plus type-level FK
declarations (the sea-orm shape). Verbose, but every column is visible exactly as the DDL
will have it, and it guarantees no polymorphic shape is ever inexpressible — only
occasionally inelegant.

After (1)–(3), the entire polymorphic surface of entl carries **one** per-site attribute
(tree_entries' parity pin). The before/after is visible in the sketch's gh_labeled and
gh_comments.

### 2.6 Value structs, scalars, composition

- Value structs are `#[derive(Record)]` — plain models, no identity, no table.
- Semantic scalars are newtypes: `#[derive(Scalar)] #[fluessig(extends = "bytes")] struct
  Oid(Vec<u8>)`. A pleasant upgrade over `.tsp`: the opaque `scalar ArrowBatch;` becomes
  `struct ArrowBatch(pub RecordBatch)` — declaration and implementation are one artifact.
- Composition children, like edges, are their own row structs carrying a parent-key column,
  marked `#[fluessig(compose(parent = X))]` — again the flat Layer-C shape. The document
  codec's aggregate assembly is unaffected.
- Nullability is `Option<T>`; `.tsp`'s `string | null` return-type unions dissolve into
  `Option<String>`.

### 2.7 The op surface: the impl is the interface

TypeSpec declares `interface Entl` and the hand-implemented Rust core trait separately,
kept in agreement by discipline. Here they are one artifact — the uniffi consumption model:

```rust
#[fluessig::export]
impl Entl {
    #[fluessig(ctor)]
    pub fn open(db_path: &str) -> fluessig::Result<Self> { ... }

    pub fn load_git(&self, repo_path: &str) -> fluessig::Result<GitStats> { ... }

    #[fluessig(stream)]   // → JS async iterator / Python generator / Ruby Enumerator
    pub fn changes(&self, repo_path: &str, options: Option<ChangesOptions>)
        -> fluessig::Result<impl Iterator<Item = fluessig::Result<ChangeBatch>>> { ... }

    #[fluessig(manual)]   // hand-written per binding, exactly as today
    pub fn watch(&self, repo_path: &str, interval_secs: i32) { ... }
}
```

`api.json` is derived from the impl that actually runs; drift between declaration and
implementation is impossible. Op shapes stay the same four (`ctor`/unary/`stream`/`manual`),
so the bindgen back end (plan.txt Step 5b) is unchanged.

### 2.8 The exporter and the drift guard

```rust
fluessig::catalog! {
    name: "entl", version: 0,
    entities: [Repo, GitNote, GitObject, Commit, CommitParent, /* … */],
    api: [git, Entl],
}
```

Reachability from explicit roots — each descriptor lists the descriptors it references —
rather than inventory/linkme link-section magic (flaky on some targets, notably wasm), and
the explicit list doubles as "what's in this catalog." `catalog!` expands to a function
consumed two ways:

1. a bin target (or `cargo fluessig emit`) that writes `catalog.json` + `api.json` —
   replacing `(cd emitter && node emit.mjs ../entl.tsp)` entirely;
2. a ts-rs-style `#[test]` that regenerates in memory, runs the **full loader validation**
   (family rules, key arity, `shares()` type compatibility, root-list/`extends` agreement)
   with file:line spans, and diffs against the checked-in catalog. Schema errors fail
   `cargo test`; a stale catalog fails with "re-run the exporter."

All semantic validation stays in the Rust loader — the derive front end obeys the same
"if a rule can live in Rust, it does" division as the TypeSpec emitter (design.md §4), so
every front end passes through one validator.

## 3. The acid test

The whole of entl.tsp — 28 tables, both polymorphic families, every edge/join table, the
derived-field carve-out, and the full Git/Entl op surface — translates
([entl_derive_sketch.rs](./entl_derive_sketch.rs)). Nothing structural was lost: every
FINDINGS item survives, most with the same comment attached. Scorecard:

- **~24 of 28 tables get lighter**, dominated by `@fk` elimination (field = column) and
  `Option` (no `?` decorations to mirror).
- **The polymorphic surface lands at parity or better** once §2.5's mechanisms are applied;
  exactly one per-site parity pin remains in the whole catalog.
- **The mirror problem disappears.** Today's architecture maintains entl.tsp *and* Rust row
  types that must agree with it. Here the annotated structs are the row types; there is no
  second copy and no parity surface between them.
- **Two artifacts collapse to one** on the op side (interface declaration = core impl).
- **Rust-keyword warts are a wash**: `` `type` `` in .tsp ↔ `r#type` in Rust.

## 4. What this changes in the decision record

design.md's "Why TypeSpec" table rejected YAML, GraphQL SDL, TypeQL, Gel, and DBML — but
never weighed a Rust derive front end. Weighed now:

**Gained.** Node leaves the toolchain entirely (the §1 packaging milestone — bundling the
TypeSpec compiler — is deleted, and the emitter/core lockstep-release problem with it).
rustc + rust-analyzer become the schema IDE. References are compile-checked. The schema and
the ingest row types are one artifact. Positioning sharpens into an empty niche: uniffi's
proven pitch plus schema/DDL/data-plane, which no one in that family has.

**Lost.** Non-Rust authorship — the "a Python shop authors with zero Rust" story from §1
dies; the schema author is necessarily a Rust developer. This is the single biggest
strategic consequence and the reason this doc is a positioning question, not a syntax
question. Also lost: TypeSpec's checker and language machinery (unions over literals,
templates, a schema-language LSP; string-valued enums are covered by `#[fluessig(value)]`),
and the legibility of an IDL document that non-Rust readers can study — the sketch reads
like Rust, which is the point *and* the cost.

**Unchanged.** `catalog.json` and `api.json` (byte-identical goals), the Rust
loader/validator, every schema and data codec, the bindgen back end, `fluessig-gen`'s CLI,
the testing strategy, and the entl-parity gates. The front end is the only moving part —
which is the original architecture paying rent: the decision is reversible in both
directions, and TypeSpec can remain a second front end for IDL-first users or quietly
retire.

## 5. Costs and open questions

- **Generated-name magic.** `GhSubjectId` / `GitObjectId` / `GhSubjectTag` are types
  conjured by the root's derive. rust-analyzer handles generated types, but "where does
  this come from" is a real documentation burden — the one place ergonomics was bought with
  explicitness. Fallback if it rankles: an opaque `PolyId<GhSubject>` generic gets ~80% of
  the compression with zero generated names.
- **Attribute grammar.** `edge(from = …, to = …, expose = …)`, `ref_cols(…)`, `shares(…)`
  are darling-tier parsing, not checker-tier. The grammar in the sketch is a proposal, not
  a survivor of implementation contact.
- **Double declaration of family membership.** `abstract_root(leaves…)` on the root and
  `extends` on each leaf; the loader checks agreement. Accepted to make the enum
  generatable from a single expansion site; it also satisfies the closed-set rule
  syntactically.
- **Macro maintenance.** A descriptor-emitting derive + `catalog!` + `export` is a real
  crate to own — smaller than maintaining a `.tsp` parser would have been, larger than the
  current TS emitter (which is deliberately trivial). Building on facet/bevy_reflect for
  Layer A could shrink it; unevaluated.
- **Does TypeSpec stay?** If this front end is built, decide whether TypeSpec remains a
  supported second front end (two grammars to keep semantically aligned against one
  catalog) or is retired. Leaning at exploration time: whichever front end the *first
  external consumer* reaches for wins; don't pay for two until someone asks.

## 6. Decision posture

Nothing here blocks or reshapes v1. The recommended sequencing, if the Rust-first
positioning is adopted: finish the v1 spine on the TypeSpec front end as planned (it
exists and is spike-proven), then build `fluessig-derive` as the second front end against
the frozen catalog format, port entl.tsp to derives as the parity test (the sketch is the
target state), and let the front ends compete for the front door. The catalog is the hedge
that makes this a sequencing question instead of a rewrite.
