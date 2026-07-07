# fluessig spike — one TypeSpec file, both layers

**`entl.tsp` is a single authored document with two layers, and one compile feeds both:**

```
                      ┌─→ catalog.json   the data model (Layer A records + Layer B relations)
entl.tsp ─ compile ───┤                  → schema codecs (DDL / Mongo / ORM / …)
                      └─→ api.json       the op surface (shapes, params, returns)
                                         → gen.mjs → generated/{core,node,python,ruby}.rs
```

The models (`@Fluessig.entity` / `@key` / `@edge` / `@compose` — the DESIGN.md §3 vocabulary) and
the API (`interface Entl` with `@ctor` / `@stream` / `@manual` op shapes) live in one file; the
decorator library is inlined at the top (it ships as `@fluessig/typespec` later). fluessig never
parses `.tsp` — `extract.mjs` walks the **checked program** (the sanctioned emitter mechanism) and
serializes each layer.

## Run

```sh
npm install
node extract.mjs     # entl.tsp → catalog.json + api.json
node gen.mjs         # api.json → generated/{core,node,python,ruby}.rs
```

Verified: the generated napi (Node) + PyO3 (Python) + Magnus (Ruby) bindings all pass `cargo check`
against a stub impl of the generated `EntlCore` trait. (PyO3 needs
`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`; Magnus needs `RUBY=/opt/homebrew/opt/ruby/bin/ruby` +
`LIBCLANG_PATH=/Library/Developer/CommandLineTools/usr/lib` — same env as entl-ruby.)

## What each half proves

**Schema half (→ `catalog.json`).** From one file we recover, fully resolved: semantic scalars with
their physical carrier (`Oid → string`, `int64 → numeric`), enums, field-level composite-capable
`@key`, docs, nullability (`?`), nesting vs. relations (a bare model like `Signature` is a value
struct; an `@entity`-typed field is a relation), cardinality from the list marker, `@compose` →
composition, and `@edge(CommitParent)` → edge properties as a **checked type reference** (not a
string).

**Binding half (→ `api.json` → Rust).** Every op has a SHAPE, and the per-language idiom is
hand-written once per (language × shape) template — the generator applies it mechanically, so
N ops × M languages collapses to 4 × 3 templates and the idiom survives *by construction*:

| shape | Node (napi) | Python (PyO3) | Ruby (Magnus) |
|---|---|---|---|
| `@ctor` | `#[napi(constructor)]` | `#[new]` | `define_singleton_method("new")` |
| *(unary)* | `AsyncTask` → `Promise<T>` | `py.allow_threads(…)` | plain method under the GVL |
| `@stream` | `next(): Promise<T\|null>` | `__iter__`/`__next__` (`None`→`StopIteration`) | `.next` → value or nil |
| `@manual` | skipped — hand-written | skipped | skipped |

All three stream dressings poll the same core primitive (`PollStream::poll(timeout)` — entl's
`ChangeStream::poll`), so "one sync primitive, every binding dresses it in its own idiom" is
preserved. The generated `NextChangesTask` is near line-for-line the hand-written `NextChangeTask`
in `crates/entl-node/src/lib.rs`. `generated/core.rs` is the one trait you hand-implement over
entl-core; the bindings never touch entl-core directly.

## Gotchas learned

- `op` is a TypeSpec **keyword** — a field named `op` must be backtick-escaped.
- `extern dec …(target: Operation)` needs `using TypeSpec.Reflection;`.
- Our decorators must be **namespaced** (`@Fluessig.key` — bare `@key` collides with the built-in);
  impls bind via the `$decorators.Fluessig` export in `decorators.js`.
- Generated code must import the core trait for `Impl::method` resolution; `PollStream` must be
  `Send + Sync` (the napi stream handle shares it via `Arc`).
- rb-sys picks up Xcode's Ruby 2.6 unless `RUBY=` points at the real interpreter.

## Honest limits (it's a spike)

- Decorator subset: no `@card`/`@assoc`/`@inverse`/`@name`, no polymorphic supertypes yet; API types
  are scalars + flat models (no options bags, enums, optional params). All mechanical extensions.
- Data crosses as JSON strings in `ChangeBatch`; the real design routes bulk data through **Arrow
  C-FFI** (see /translation.md) and keeps this surface for control types.
- `gen.mjs` is JS for spike speed only — it has zero TypeSpec dependency (input: `api.json`). The
  real one is a **Rust back-end in the fluessig crate** (`fluessig bindgen`), per DESIGN §4's
  "everything after the JSON is Rust" rule: the generated `core.rs` must agree with the Rust
  IR/marshalling types, and that agreement should hold by construction. Only `extract.mjs` (the
  checked-program walk) stays JS. Not yet proven: wiring `EntlCore` to the *real* entl-core and
  passing the three existing test suites over generated bindings.
