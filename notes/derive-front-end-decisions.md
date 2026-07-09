# Derive front end — decisions and implementation plan

Follow-up to [`derive-front-end.md`](./derive-front-end.md) (PR #20). That doc was an
exploration that deliberately left the strategic forks open. This note records the
decisions taken on those forks, then sketches **how the front end will look** and
**what it will take to get there**. Same ground rule as its parent: the catalog contract
(`catalog.json` / `api.json`), the Rust loader, and every back end are unchanged — the
front end is the only moving part.

## Decisions

1. **Audience: Rust-first developers, exclusively.** The "a Python/TS shop authors
   schemas with zero Rust" positioning from `design.md` §1 is retired. fluessig is a
   normal Rust crate that happens to ship a library in every other language; the schema
   author is a Rust developer. This is the reversal `derive-front-end.md` §4 called "the
   single biggest strategic consequence" — taken deliberately, not by default.

2. **TypeSpec is retired, not kept as a second front end.** The derive front end
   *replaces* the TypeSpec front end rather than competing with it indefinitely. The path
   is therefore a **migration**, not permanent dual-grammar upkeep: build the derive
   front end, port `entl.tsp` and `disponent.tsp` to derives, then delete the TypeSpec
   emitter and remove Node from the toolchain (the `design.md` §1 packaging milestone —
   bundling the TypeSpec compiler — is deleted with it, per §4 "Gained"). Until the port
   is proven at parity, the TypeSpec emitter stays in the tree; it is removed only once
   derives reproduce every consumer catalog byte-for-byte.

3. **Polymorphic references use generated named key enums, not an opaque generic.**
   `abstract_root(Commit, Tree, Blob)` generates a real sum type and reference sites use
   it natively:

   ```rust
   pub enum GitObjectId { Commit(Oid), Tree(Oid), Blob(Oid) }
   pub enum GhSubjectId { GhPullRequest(Id<Repo>, i32), GhIssue(Id<Repo>, i32) }
   ```

   The alternative floated in §5 — an opaque `PolyId<T>` — was rejected. The keys across a
   family are heterogeneous (`GitObject` keys on a scalar `Oid`; `GhSubject` on a composite
   `(repo, number)`), so a family needs a sum type regardless; `PolyId<T>` would still have
   to generate that enum internally and merely hide its name, at the cost of routing every
   read through a wrapper API — reintroducing exactly the fluessig-concept-to-learn the
   "think in Rust" thesis exists to delete. A native `enum` + `match` is the most Rust-first
   option and expresses per-variant key differences for free.

   The one real cost of named enums — `GitObjectId` is a name the user never typed, so
   "where does this come from?" is a documentation burden — is mitigated by **also exposing
   the enum through a trait alias**: `<GitObject as AbstractRoot>::Id`. That gives the
   conjured name a go-to-definition answer (the trait impl), and the convention to document
   is one line: `abstract_root(A, B, C)` generates `<Root>Id`.

4. **Reflection substrate: build the descriptor layer on `syn` + `darling` from scratch;
   do not adopt a reflection substrate.** The instinct to not roll our own reflection is
   sound in general, but three facts specific to a build-time schema tool blunt the payoff:

   - **We own a proc-macro crate regardless.** Source-span capture (`file!()`/`line!()`),
     the generated key enums, `catalog!`, and `#[export] impl` → `api.json` are all *code
     generation*, which no reflection substrate does — `facet` and `bevy_reflect` both only
     *capture existing shape*. A substrate could replace the descriptor-*capture* half at
     best, never the generation half. That halves, not eliminates, the surface.
   - **`darling` is already the right attribute-grammar tool** for the macro we're writing
     anyway — it parses `edge(from=…, to=…)`, `ref_cols(...)`, `shares(col)` (the §5
     "darling-tier" call) with no pre-1.0 runtime dependency.
   - **The two things a substrate would save are the two it's weakest at here:** type-level
     `Id<Tree>` resolution is *more direct in `syn`* (the macro sees the literal tokens)
     than reconstructing from a monomorphized type's `type_params`; and source spans it
     can't capture at all.

   `bevy_reflect` is a straight no — it is a *runtime* system (`TypeRegistry` / `Reflect` /
   values), the wrong paradigm for a `&'static` build-time descriptor, and pulls a heavy
   Bevy-coupled dep tree on a game-engine release cadence. `facet` is the only defensible
   substrate if we ever reverse this — its `const SHAPE: &'static Shape` +
   `define_attr_grammar!` namespaced attributes + native doc capture are genuinely close to
   the Layer-A descriptor spec — but it is pre-1.0 with the attribute design explicitly "in
   flux," which is a poor foundation under a tool that promises byte-stable catalog output.
   If adopted later it would be for the capture half only, hard-pinned. For now: `syn` +
   `darling`.

## How it will look

The architecture is the one committed in `derive-front-end.md` §1: **derive →
`&'static EntityDescriptor` → exporter**. The macro expands to pure data; a separate step
collects descriptors and writes the catalog `fluessig-gen` already consumes. Concretely:

**Entities.** The field *is* the column; the key type carries the reference, so `@fk`
mostly disappears:

```rust
#[derive(Entity)]
pub struct Commit {
    #[key] pub oid: Oid,
    pub tree_oid: Id<Tree>,               // was: @fk(#["tree_oid"]) tree: Tree
    pub author_id: Option<Id<GhUser>>,
    /// Author timestamp, UTC.
    pub authored_at: Timestamp,
}
```

`Id<T>` resolves through `<T as Entity>`; a typo'd target is a rustc error with
rust-analyzer completion. Doc comments (`///`) flow into the descriptor and on to the
generated ORM/site.

**Edges** are their own row structs (converging with `design.md` §5 decision #1, flat
batches):

```rust
#[derive(Edge)]
#[fluessig(edge(from = Commit, to = Commit))]
pub struct CommitParent {
    pub child_oid: Id<Commit>,
    pub parent_oid: Id<Commit>,
}
```

**Polymorphic families** — the abstract root declares the closed leaf set and the shared
key spelling once; the derive generates `<Root>Id` and the `AbstractRoot` trait alias:

```rust
#[derive(AbstractRoot)]
#[fluessig(abstract_root(Commit, Tree, Blob), tag_col = "obj_type", ref_col = "obj_oid")]
pub struct GitObject { #[key] pub oid: Oid }
// generates: enum GitObjectId { Commit(Oid), Tree(Oid), Blob(Oid) }
//            impl AbstractRoot for GitObject { type Id = GitObjectId; }
```

**The op surface** is the impl that actually runs — `api.json` is derived from it, so
declaration/implementation drift is impossible:

```rust
#[fluessig::export]
impl Entl {
    #[fluessig(ctor)]   pub fn open(path: &str) -> Entl { … }
                        pub fn commit(&self, oid: Oid) -> Option<Commit> { … }
    #[fluessig(stream)] pub fn commits(&self) -> impl Iterator<Item = Commit> { … }
    #[fluessig(manual)] pub fn query_arrow(&self, sql: &str) -> ArrowBatch { … }
}
```

**The catalog** is an explicit root list (reachability from roots — no `inventory`/`linkme`
link-section magic), consumed two ways: a `cargo fluessig emit` bin that writes the
catalog (replacing `node emit.mjs`), and a drift-guard `#[test]` that regenerates in
memory, runs the **full Rust loader validation** with `file:line` spans, and diffs against
the checked-in catalog:

```rust
fluessig::catalog! {
    name: "entl", version: "0.1.0",
    entities: [Commit, Tree, Blob, GitObject, CommitParent, /* … */],
    api: [Entl],
}
```

## What it will take

Build `fluessig-derive` as a **second front end against the frozen catalog**, prove it by
migration, then retire TypeSpec. Sliced so each step ends at a byte-diffable checkpoint
against the existing TypeSpec output.

- **Slice 1 — end-to-end skeleton.** New `fluessig-derive` crate: `#[derive(Entity)]` →
  `&'static EntityDescriptor` for one simple entity (scalar fields + `#[key]`); `catalog!`
  collecting it; `cargo fluessig emit` writing `catalog.json`. Gate: byte-identical to the
  TypeSpec emitter's output for that one entity. This is the whole pattern in miniature —
  everything after is filling in descriptor richness.
- **Slice 2 — references.** `Id<T>` typed keys with foreign-key resolution via `syn` path
  parsing (`@fk` disappears); composite keys via `ref_cols(...)` declared on the referenced
  entity. Gate: an entity graph with single + composite FKs matches.
- **Slice 3 — attribute grammar (`darling`).** `#[fluessig(flatten)]` embedding for
  inheritance/abstract roots; edge structs (`edge(from, to)`); `shares(col)`. This is where
  the §5 "the grammar is a proposal, not a survivor of implementation contact" risk gets
  retired — first contact with real parsing.
- **Slice 4 — polymorphism.** `abstract_root(...)` generating the named key enums +
  `AbstractRoot::Id` trait alias; tag/key column spelling declared on the family; the
  per-site `cols(...)` override retained only for legacy variance (entl FINDINGS #7).
- **Slice 5 — op surface.** `#[fluessig::export]` on an impl → `api.json`; `ctor` / plain /
  `stream` / `manual` op kinds. Gate: entl's `api.json` reproduced from the impl.
- **Slice 6 — spans + docs.** `file!()`/`line!()` and `///` into the descriptor so loader
  diagnostics point at `.rs` lines the way they point at `.tsp` lines today. (Span quality
  is a known Rust rough edge — verify the diagnostics actually land on the right line.)
- **Slice 7 — drift guard.** The regenerate-validate-diff `#[test]`, wired into CI the way
  the `node` drift job is today.
- **Slice 8 — migration + retirement.** Port `entl.tsp` (all 28 tables — the acid test)
  then `disponent.tsp` to derives; confirm both catalogs are byte-identical; then delete the
  TypeSpec emitter and remove Node from the toolchain.

The **first implementation slice** is Slice 1: it is small, it exercises the entire
derive → descriptor → exporter → `fluessig-gen` path, and its byte-diff gate is the
cheapest possible test of the core claim before any of the ergonomic surface is built.
