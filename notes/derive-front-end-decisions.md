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

2. **TypeSpec is retired, not kept as a second front end. — DONE.** The derive front end
   *replaces* the TypeSpec front end rather than competing with it indefinitely. The path
   is therefore a **migration**, not permanent dual-grammar upkeep: build the derive
   front end, port `entl.tsp` and `disponent.tsp` to derives, then delete the TypeSpec
   emitter and remove Node from the toolchain (the `design.md` §1 packaging milestone —
   bundling the TypeSpec compiler — is deleted with it, per §4 "Gained"). Until the port
   is proven at parity, the TypeSpec emitter stays in the tree; it is removed only once
   derives reproduce every consumer catalog byte-for-byte.

   **Completed.** Both consumers (entl#77, disponent#77) are migrated + merged, and the
   TypeSpec front end has been removed: the `@fluessig/emitter` Node emitter, the
   `@fluessig/typespec` decorator library, all `.tsp` sources, and Node are gone from the
   toolchain. The `#[derive(Entity)]` front end is now the only front end;
   `cargo fluessig emit` replaces `node emit.mjs`. The emitted `catalog.json` / `api.json`
   are retained as frozen fixtures (entl's parity target + the engine's dogfood catalogs).

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
migration, then retire TypeSpec. Sliced so each step ends at a **semantic-equivalence
checkpoint**: the derived catalog loads clean through the Rust validator and drives
`fluessig-gen` to equivalent output — not a byte-for-byte JSON diff against the TypeSpec
emitter (front-end identity fields — the emitter/compiler stamp and `source` name —
legitimately differ). Byte-identity was dropped as the gate by the owner.

- **Slice 1 — end-to-end skeleton.** New `fluessig-derive` crate: `#[derive(Entity)]` →
  `&'static EntityDescriptor` for one simple entity (scalar fields + `#[key]`); `catalog!`
  collecting it; `cargo fluessig emit` writing `catalog.json`. Gate: the emitted catalog
  loads clean through the existing Rust loader/validator and drives `fluessig-gen` to
  equivalent output (same DDL/PK) for that one entity. This is the whole pattern in
  miniature — everything after is filling in descriptor richness.
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
- **Slice 8a — migration prerequisites.** Two capabilities deferred by earlier slices that
  the entl port exercises, closed before the port itself:
  - *Extends-aware composite-key FK resolution* (deferred in Slice 4). `RefResolver` now
    follows `extends` to a family leaf's **inherited** composite key, so a direct
    `Id<Leaf>` FK into a composite-keyed family (`Watch.bug: Id<Bug>`, where `Bug extends
    Ticket` keyed `(project_id, seq)`) spells its two FK columns correctly — the shape
    Slice 4's poly demo avoided (only *polymorphic* references, never a direct leaf FK).
    Gate: `crates/derive-demo/src/leaf_fk.rs` + `leaf_fk.tsp` project identically.
  - *The `api.json` DTO/`models` layer* (deferred in Slice 5). Slice 5 scoped `api.json` to
    ops (empty `models`). `#[derive(Record)]` now declares DTOs (→ catalog `valueStructs`),
    and `build_api` materialises the `models` array — the entities/DTOs the ops reference,
    flattened (to-one relation → FK field(s), polymorphic → discriminator-prepended,
    to-many dropped) and closed transitively — a direct port of the TypeSpec emitter's model
    closure, so a derive-authored api surface produces the SAME `models` the TypeSpec path
    does. Gate: the api demo's ops + models match `api.tsp` field-for-field.
- **Slice 8b — migration + retirement.** Port `entl.tsp` (all 28 tables — the acid test)
  then `disponent.tsp` to derives; confirm both catalogs are semantically equivalent (each
  loads clean and drives every consumer to the same output); then delete the TypeSpec
  emitter and remove Node from the toolchain.

The **first implementation slice** is Slice 1: it is small, it exercises the entire
derive → descriptor → exporter → `fluessig-gen` path, and its load-clean-and-drive-`fluessig-gen`
gate is the cheapest possible test of the core claim before any of the ergonomic surface is
built.

### Slice 8b addendum — the disponent acid test (`crates/disponent-schema-derive`)

Porting `disponent.tsp` (the second acid test, after entl) surfaced authoring capabilities the
entl port never exercised. The catalog contract, loader, and back ends were already union-aware
(`ir::UnionDef` / `TypeRef::Union`, `api::ApiUnion`, `ApiOp::readonly` / `destructive`) — the
front end was the only moving part, as ever:

- **Union authoring (feature A).** `#[derive(Union)]` on a Rust `enum` of single-field tuple
  variants (`State(StateChange)`) captures a `UnionDescriptor`; the wire tag is the variant name
  lowerCamelCased (`ToolCall` → `toolCall`) or a per-variant `#[fluessig(tag = "…")]`. `catalog!`
  grows a `unions: [...]` list; a union-typed field lowers to `TypeRef::Union` (twin
  `<col>_kind` + `<col>` columns), and the model closure pulls a referenced union's variant
  bodies into `api.json`'s `models` + `unions` (a port of the emitter's twin-set fixpoint).
- **`@readonly` / `@destructive` op hints (features B + C).** `#[fluessig(readonly)]` /
  `#[fluessig(destructive)]` are FLAGS on an exported op that compose with its kind (a
  `@readonly @stream` op is both) — lowering to `api.json` `"readonly"` / `"destructive"` and,
  downstream, the MCP `readOnlyHint` / `destructiveHint`.

Four smaller front-end gaps the same port surfaced (each a lowering fix mirroring the TypeSpec
emitter, all additive — entl stayed green):

1. **Scalar refinement roots.** A semantic scalar refining a *refined* builtin (`Cents extends
   int64`, and `int64` roots at `numeric`) records its ROOT carrier at a field TypeRef
   (`numeric`), while the `scalars` DECLARATION array keeps the immediate `int64` — the emitter's
   `while (root.baseScalar)` walk. entl's `Oid extends bytes` roots one hop, so it never showed.
2. **Semantic-scalar / enum op params.** `#[fluessig::export]` can't tell `uid: SessionUid` (a
   scalar) from a model at the token; the classification now happens at lowering against the
   declared types (a declared scalar → the bare scalar name, an enum → `{enum}`), the catalog
   cross-check the macro comment already deferred to.
3. **`Id`-suffixed scalars.** A declared scalar ending in `Id` (`FanoutId` / `MessageId` /
   `DispatchId`) collides with the `<Root>Id` poly-reference heuristic; a name that is a declared
   named type but not a family root is disambiguated to a plain scalar column at lowering.
4. **List columns + the `url` / `snake_case` stock surface.** `Vec<T>` (T ≠ u8) entity columns
   (`Dispatch.tags: string[]`) lower to `TypeRef::List`; `url` joins the stock string scalars;
   `rename_all = "snake_case"` joins the enum casing rules (disponent's wire values are its
   snake_case member names).

Gate: `crates/disponent-schema-derive/tests/parity.rs` — the derive-emitted catalog/api project
to the SAME physical tables (columns + order + PK order), enums, scalars, unions, ops (every
readonly/destructive flag), models, and api-unions as disponent's committed artifacts.

### Synchronous ops are the DEFAULT, across all backends — `#[fluessig(async)]` is the opt-out + op export-name pins

The next acid test after entl/disponent is **pidgin** (formerly atilla — the pi binding surface,
now node + python + php, ruby soon): ~157 symbols that are almost entirely fluessig-generatable,
except its ops are deliberately **synchronous and infallible** under **exact export names**
(`atillaNativeVersion(): string`, `#[napi(js_name = …)]`). The old default fought that on every
backend — the node backend wrapped every unary op in `AsyncTask` → `Promise<T>` over a `Result`
seam, and name pins applied only to DTO fields. So the model was **inverted**: synchronous is now
the GLOBAL DEFAULT, `#[fluessig(async)]` is the opt-out, and op export-name pins apply across every
backend. Proven in `crates/derive-demo/src/native.rs` + `tests/api_gate.rs`. Async-ness is decided
in exactly ONE place — the per-op `#[fluessig(async)]` label — meaning the same thing on every
backend; there is no catalog-level default. entl/disponent stay byte-identical because their
IO-bound ops carry `#[fluessig(async)]` per-op (below), NOT because of any catalog lever.

- **Synchronous by default; `#[fluessig(async)]` opts out.** A plain unary op with no marker
  generates a **synchronous** binding in every backend — node a plain `#[napi] pub fn name(..)`
  (no `AsyncTask`, no `Promise`, no per-op `Task`), python/php/ruby a plain method (they were
  already synchronous). `#[fluessig(async)]` on a unary op is the opt-out: it restores the
  historical async projection (node `AsyncTask` → `Promise<T>`). In `api.json` async is
  serialized — `"async": true` appears only on an op that opts in; a synchronous (default) op has
  no `async` field. `#[fluessig(sync)]` is retained as a (now redundant) explicit force-sync marker
  — it resolves identically to no marker. Both `sync`/`async` compose only with a plain unary op
  (the macro rejects them on ctor/stream/manual — a ctor is always a synchronous constructor, a
  stream always async-iterable). The marker lives on `OpDescriptor.is_async: Option<bool>` (macro
  authoring level: `Some(true)`/`Some(false)`/`None`) and lowers to `ApiOp.is_async: bool` (the IR:
  `Some(false)` and `None` both become `false`); backends read `ApiOp.is_async` directly.
- **Infallibility is inferred from the Rust return type.** A **synchronous** op whose Rust return
  is a bare `T` (not `Result<T>`) is **infallible** (`"infallible": true`) — node emits `-> T` with
  a direct core call (no `.map_err`), python drops its `PyResult`/raise, php its `PhpResult`, ruby
  its `Result<_, Error>`, and the SHARED core-trait method drops its `anyhow::Result` wrapper
  (`fn name(..) -> T`, in `emit_core_traits_with`, `src/bindgen/mod.rs`). A `Result<T>` return keeps
  the throwing/raising seam. Ruby is the one honest edge: its arg marshaling (`scan_args` /
  `TryConvert`) is itself fallible, so a truly no-raise `-> T` is emitted only for a zero-marshaling
  op (no params, non-list) — the atilla `atillaNativeVersion()` shape; a param'd / list-returning
  infallible op keeps the `Result<_, Error>` seam (the CORE call drops its `.map_err`, the
  marshaling can still raise).
- **No catalog-level async default — async is a per-op label, uniform everywhere.** There is no
  `default_async` catalog lever: async-ness is decided in exactly ONE place, the per-op
  `#[fluessig(async)]` label, meaning the same thing on every backend. The IO-bound ops in the
  entl / disponent parity catalogs and the four-kind `Db`/`GitHelpers` demo therefore carry a
  per-op `#[fluessig(async)]` (entl's ops hit DuckDB; disponent's drive tmux/subprocesses — genuinely
  IO-bound, so async is the correct label), which keeps their async projection and committed
  goldens byte-identical. The entl/disponent parity gate compares op surfaces with an async-agnostic
  reducer (`api_lines`: `Interface.op [shape](params) -> ret`), so the labels don't affect parity.
  The owner will rewrite the real entl / disponent derives later; the vendored parity copies carry
  the labels so their intent is honest.
- **`#[fluessig(name = "…")]` — op export-name pins, every backend.** The op-level twin of the
  DTO-field `SymbolBinding`/`pinned_name` mechanism: an explicit export name lowered onto
  `ApiOp.bindings` (every language slug) so each backend reproduces the exact spelling — node
  `#[napi(js_name = "…")]`, python `#[pyo3(name = "…")]`, php ext-php-rs `#[rename("…")]`, ruby the
  `define_(singleton_)method` name (the Rust fn ident stays snake). An unpinned op keeps each
  backend's default casing, byte-identical.

Together these generate atilla's `#[napi(js_name = "atillaNativeVersion")] pub fn …() -> String`
verbatim (modulo the core-seam body) in node, and its synchronous/infallible twin in python / php /
ruby — the sync-default surface pidgin needs to generate its (all-sync) bindings. The remaining
atilla tail (a later slice): binary `Uint8Array`/`Buffer` arg spelling + result-envelope shaping;
the AgentBridge callback bridge stays hand-written.

### The node "tail": `Uint8Array`/`Buffer` binary spelling + the `{ ok, value } | { ok, error }` result envelope

Two of the remaining tail features above, so pidgin (formerly atilla) can generate more of its
hand-written napi. Both are node-backend concerns; consistent with the `#59`/`#69` op-flag
precedent, proven in `crates/derive-demo/src/binary.rs` + `tests/api_gate.rs` (the AgentBridge
callback bridge stays `@manual` by design).

- **Position-aware binary spelling (no annotation).** pi/pidgin spell binary the JS-idiomatic way:
  a `bytes` **param** is a `Uint8Array` (a read-only view — `detectSupportedImageMimeType(buffer:
  Uint8Array)`), a `bytes` **return** is a `Buffer` (an owned buffer — `readBinaryFile(path):
  Buffer`). `Buffer extends Uint8Array`, but the printed `.d.ts` differs, so byte-exact conformance
  needs the split. Node now spells `bytes` **position-aware by default** — no new schema attribute:
  the OUT half lives in `node_ty` (→ `napi::bindgen_prelude::Buffer`, a return / a DTO field), the
  IN half in `node_param_sig`/`node_param_ty` (→ `napi::bindgen_prelude::Uint8Array`, a param),
  both fully-qualified so napi's `.d.ts` generator names them directly (no alias to resolve, no
  `ts_return_type` hint). The core-trait params flow through the same `node_param_sig` (via the new
  `emit_core_traits_full` seam), so the handle-method → core call type-checks. Every other backend
  keeps the shared `Bytes` spelling, byte-identical.
- **`#[fluessig(result)]` — the `{ ok, value } | { ok, error }` result envelope.** pidgin's ~13
  `NodeExecutionEnvCore` methods hand their error back AS A VALUE — a discriminated `{ ok: true,
  value: T } | { ok: false, error: E }` object the shim reparses — rather than throwing. The
  op-level `#[fluessig(result)]` marker (mirroring `sync`/`async`, a projection modifier on a
  **synchronous unary op**) opts in; the error type `E` is a normal `#[derive(Record)]` (`FileError
  { code, message, path? }`) spelled as the op's `Result<T, E>` return and captured off that return
  (the macro rejects `anyhow::Result<T>` — the marker demands an explicit, named error record).
  Node emits two `#[napi(object)]` arms (`<Op>Ok { ok, value }` / `<Op>Err { ok, error }`) and a
  method returning `napi::bindgen_prelude::Either<…Ok, …Err>` built from the core's `Result<T, E>`
  VALUE; the core-trait method returns `Result<T, E>` (not `anyhow::Result<T>`, the throw seam). It
  is strictly **opt-in and node-only**: a default fallible op still throws, and the other backends
  treat a `result` op as an ordinary fallible op (their core traits keep the `anyhow`/generic seam),
  so their goldens are unperturbed. The error record joins `api.json`'s `models` purely through the
  op's `result_error` reference (`build_models` seeds it), even though it appears in no param/return
  position. **Design notes for the owner:** (1) a *typed discriminated object* (two tagged
  `#[napi(object)]` arms behind `Either`) is the chosen target — the more fluessig-idiomatic end
  state — over pidgin's current serialized JSON-string envelope the shim `JSON.parse`s; the pidgin
  campaign byte-diffs against the real repo to confirm it's acceptable. (2) napi collapses the `ok`
  bool to `ok: boolean` in its `.d.ts` (the same limitation the structured-union `type: string` tags
  hit) — the exact `ok: true` / `ok: false` discrimination is an external-`.d.ts` concern, out of
  scope here; the structural `{ ok, value } | { ok, error }` shape is exact.

### `#[fluessig(single_threaded)]` — a thread-confined handle over a `!Send` core (node)

Some native cores are inherently thread-local. pidgin's UI-state cores — `TuiCore`
(`Tui<LoggingTerminal>` with `Rc<RefCell<dyn Component>>` + boxed non-Send closures),
`InputCore`, `SelectListCore` — are `!Send`. napi CLASS instances are thread-confined (they
never cross threads), so a hand-written `#[napi]` class can hold such a core fine (atilla's
`crates/atilla-napi/src/tui.rs` does exactly this: `struct TuiCore { tui: Tui<LoggingTerminal> }`,
`&mut self` methods, no `Arc`). But fluessig's ORDINARY generated handle holds the core as
`Arc<crate::core_impl::<Iface>Impl>`, which forces `Impl: Send + Sync` — needed only for the ASYNC
projection, where the `Arc` clones onto a threadpool worker. That is a hard `!Send` wall a `Mutex`
cannot fix, so those cores could be hand-written but not GENERATED.

`#[fluessig(single_threaded)]` is an **interface-level** marker on the exported `impl` block (the
op flags — `ctor`/`stream`/`async`/`readonly`/… — ride the METHODS; this one rides the impl),
consistent with the `#59`/`#69`/`#74` per-op-flag precedent. It lowers to
`api.json`'s `ApiInterface.single_threaded` (skip-if-false, so every existing interface stays
byte-identical) and, on the **node backend only**, projects a THREAD-CONFINED handle:

- the generated `<Iface>Core` trait sheds `Send + Sync` (`pub trait TuiCore: Sized + 'static`), so
  a `!Send` core can implement it, and its handle-bound ops take `&mut self`;
- the handle holds the core by plain ownership inside a `RefCell` — `pub(crate) core:
  RefCell<crate::core_impl::TuiImpl>`, **no `Arc`, no `Send`/`Sync`** — and its `&self` napi methods
  reach `&mut` through `borrow_mut()` (`self.core.borrow_mut().tick()`); the ctor builds
  `RefCell::new(<Impl as Core>::open(…)?)`.

Proven end to end in `crates/derive-demo/src/single_threaded.rs` (a genuinely `!Send` `Tui` core:
`PhantomData<*const ()>` + `Rc<RefCell<…>>`), `crates/derive-demo/tests/api_gate.rs`, and the byte
goldens in `tests/single_threaded.rs`. It **unblocks pidgin's UI-state handle classes** and is
**node-only for now** (extendable when another thread-confined runtime needs it).

**Node-only, fail LOUD elsewhere (honest capability edge).** A thread-confined `!Send` handle is a
node concept; every other backend's handle wrapper (`#[pyclass]`, `#[php_class]`, `#[wasm_bindgen]`,
the JNI/C++ glue) would hold the core in a `Send`-requiring form. So python/php/ruby/wasm/java/cpp
do NOT silently emit a `Send`-assuming handle for a single_threaded interface (which would break the
consumer's build with a confusing downstream error) — they emit NOTHING for it plus an explicit
skip-note, e.g. `// interface \`Tui\` is #[fluessig(single_threaded)] (a thread-confined \`!Send\`
handle) — not supported by the python backend (node-only today); no binding emitted.`

**Rejected marker combinations.** A single_threaded interface may carry ONLY synchronous ops — an
async or stream op needs a `Send` core for the threadpool, incompatible with a thread-confined
`!Send` handle. The derive macro rejects the authoring path with a SPANNED compile error at the
offending method; the loader (`load_api`) re-checks the lowered / hand-written `api.json`. The
actual messages a developer sees, verbatim:

- `#[fluessig(single_threaded)]` + an `#[fluessig(async)]` op (macro, spanned at the method):

  > `#[fluessig(single_threaded)] interface: an async op is not allowed — a thread-confined \`!Send\` handle cannot serve an async op (the async projection clones the core onto a threadpool worker, which requires a \`Send\` core). Drop #[fluessig(async)], or drop #[fluessig(single_threaded)]`

- `#[fluessig(single_threaded)]` + an `#[fluessig(stream)]` op (macro, spanned at the method):

  > `#[fluessig(single_threaded)] interface: a stream op is not allowed — a thread-confined \`!Send\` handle cannot serve a stream op (streams poll off a threadpool worker, which requires a \`Send\` core). Drop #[fluessig(stream)], or drop #[fluessig(single_threaded)]`

- the same surface reaching the loader as `api.json` (an async or stream op on a
  `single_threaded` interface):

  > `op \`Tui.slow\`: a #[fluessig(single_threaded)] interface may carry only synchronous ops — an async or stream op needs a \`Send\` core for the threadpool, which is incompatible with a thread-confined \`!Send\` handle`

An unknown interface tag is likewise rejected, e.g. an op flag mistakenly placed on the `impl`:
`unknown interface tag \`readonly\` — the only #[fluessig(…)] tag on an exported \`impl\` block is
#[fluessig(single_threaded)] (the thread-confined handle opt-in). Op markers (ctor / stream / async
/ readonly / …) ride the METHODS, not the impl`.
