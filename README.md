<p align="center"><img src="assets/logo.png" alt="fluessig" width="200"></p>

# fluessig

Describe a typed entity graph **once**; project it **everywhere** — DDL, ORM
models, format codecs, language bindings, and an Arrow-fed data plane, all
generated from a single catalog.

> [!NOTE]
> fluessig is mid-pivot and moving fast. The strategic front end is a Rust
> `#[derive(Entity)]` surface; the original TypeSpec front end still drives
> every consumer today and is retired only once the derives reproduce every
> consumer catalog byte for byte. Both are documented here — reach for the
> derive front end for new work.

## The model

A fluessig **catalog** is the single source of truth for a data model: its
entities, their fields and keys, the relations between them, and the docs. From
that one catalog fluessig generates DDL (per-dialect `CREATE TABLE` for DuckDB,
Postgres, SQLite), ORM surfaces (SQLAlchemy models, TypeScript table types),
language bindings (Rust / Node / Python / Ruby read planes), and even a
per-language README.

The pipeline has two stages: a **front end** lowers your schema to
`catalog.json` (plus `api.json`), and **`fluessig-gen`** — the Rust engine at
this repo's root — generates code from those artifacts. The store is a derived
cache: regenerate and re-ingest on any schema change.

## Install

There's no published crate yet — build the engine from a checkout:

```sh
cargo build --release      # -> target/release/fluessig-gen
```

The TypeSpec front end also needs the emitter's npm deps (`cd emitter && npm
install`); the derive front end needs only the Rust workspace.

## Usage

fluessig has **two front ends** — reach for the derive front end for new work,
the TypeSpec one for parity with today's consumers.

### The derive front end (Rust-first — the direction)

Author your schema as ordinary Rust structs and emit the same `catalog.json`
the engine consumes. `#[derive(Entity)]` and `#[derive(Edge)]` describe scalars
and edges; `Id<T>` fields are typed foreign keys; `#[key]` marks key fields (a
composite key is just several); and the attribute grammar (`flatten`, edges,
`shares`, `ref_cols`) rides on `#[fluessig(...)]` attributes.
`fluessig_derive::catalog! { ... }` collects your entities into an exporter
module. Then `cargo fluessig emit` runs the crate's exporter and writes the
catalog:

```sh
cargo fluessig emit                    # -> catalog.json (default bin: fluessig-emit)
cargo fluessig emit --bin my-emit -o schema/catalog.json
```

This is Slices 1–3 of the [derive front-end plan][decisions]: the derives,
`Id<T>` FKs plus composite keys, and the attribute grammar have landed on
`main`. Polymorphism, the op surface, span-accurate docs, a drift guard, and
the TypeSpec-retirement migration are still ahead — so until parity is proven,
the TypeSpec front end stays.

[decisions]: notes/derive-front-end-decisions.md

### The TypeSpec front end (current — what consumers run today)

Author the schema in TypeSpec and lower it with the Node emitter, then generate
from the catalog with the Rust engine:

```sh
cd emitter && npm install
node emit.mjs path/to/schema.tsp --out schema/   # -> catalog.json + api.json
cargo run --bin fluessig-gen -- --help           # generate DDL / ORM / bindings
```

Both [entl] and [disponent] drive this path today (see **Consumers**).

## README multiplexing

fluessig generates per-language READMEs like this one from a single template:
write the doc once with `fl:` directives and render a variant per language.

```sh
fluessig-gen --readme template.md --readme-out README.md --readme-lang rust
fluessig-gen --readme template.md --readme-out 'README-{lang}.md' --readme-langs rust,node,python,ruby
```

Directives are each an HTML comment on their own line, so the raw template
still renders on GitHub:

- `{{ key }}` interpolates `lang`, `lang.slug`, `lang.fence`, `lang.install`,
  `lang.ext`, `pkg`, `catalog.name`.
- `<!-- fl:only rust node -->` … `<!-- fl:end -->` keeps a block for some
  languages; `fl:except` drops it for them.
- `<!-- fl:each -->` with `<!-- fl:lang rust -->` variants (plus `default`)
  expands a block once per language.

The four targets, in canonical order, are **rust** (`cargo add`), **node/bun**
(`bun add`), **python**, and **ruby**. Unknown keys, unterminated blocks, and
missing variants are hard errors.

## Layout

```
src/                            the engine: catalog/api loaders, per-dialect DDL,
                                the README multiplexer (readme.rs), fluessig-gen.
emitter/                        the TypeSpec front end (emit.mjs) + its tests.
typespec/                       the TypeSpec library shared with consumers.
crates/fluessig-derive          the Rust derive front end (entities as structs).
crates/fluessig-derive-macros   its proc-macros: #[derive(Entity)], catalog!, ...
crates/cargo-fluessig           the `cargo fluessig emit` subcommand.
crates/derive-demo              worked examples + gates for the derive front end.
notes/                          design docs (design.md, derive-front-end*.md).
scripts/                        dev.sh (setup), coverage.sh.
```

## Build & test

```sh
scripts/dev.sh                       # build the Rust workspace + install emitter deps
cargo test                           # engine + derive-crate + fixture tests
(cd emitter && node test.mjs)        # the TypeSpec emitter's tests
scripts/coverage.sh                  # cargo-llvm-cov summary (--html for a report)
```

## Consumers

[entl] and [disponent] both keep their schema as a fluessig catalog and
generate from it at build time. They locate this repo via `FLUESSIG_DIR` — a
sibling `../fluessig` checkout by default, or a pinned clone in CI — and run the
TypeSpec emitter plus `fluessig-gen` from their own `scripts/gen.sh`.

[entl]: https://github.com/zmaril/entl
[disponent]: https://github.com/zmaril/disponent

## Contributing

Issues and PRs welcome. PR titles follow [Conventional Commits] (`type(scope):
summary`) — CI checks it. Run `cargo fmt`, `cargo clippy -- -D warnings`, and
`cargo test` (plus the emitter's `node test.mjs`) before pushing.

[Conventional Commits]: https://www.conventionalcommits.org

## Conventions & gotchas

- **Byte-stable emitter output.** `@typespec/compiler` is pinned exact in
  `emitter/package.json` — a caret range let a patch bump reorder the emitter's
  output and churn every consumer's committed catalog. Pin it exact; bump it
  deliberately.
- **Enums carry their wire value.** The Node emitter lowers a TypeSpec enum to
  its members and keeps the wire value when it differs from the name; on the SQL
  side an enum column is `text`.
- **Conventional Commits.** Commit messages follow the Conventional Commits
  spec.

## License

MIT © Zack Maril
