<!-- housekeeper:description Describe a typed entity graph once; project it everywhere — DDL, ORM models, format codecs, and language bindings. -->
<!-- housekeeper:topics codegen, orm, rust, schema, sql -->
<p align="center"><img src="assets/logo.png" alt="fluessig" width="200"></p>

# fluessig

Describe a typed entity graph **once**; project it **everywhere** — DDL, ORM
models, format codecs, language bindings, and an Arrow-fed data plane, all
generated from a single catalog.

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

The front end needs only the Rust workspace — no Node, no npm.

## Usage

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

Once you have a `catalog.json` (plus `api.json`), generate from it with the
Rust engine:

```sh
cargo run --bin fluessig-gen -- --help           # generate DDL / ORM / bindings
```

The derive front end is the [only front end][decisions]: the derives, `Id<T>`
FKs plus composite keys, the attribute grammar, polymorphism, the op surface,
span-accurate docs, and the drift guard have all landed. Both [entl] and
[disponent] author their schemas as Rust derives (see **Consumers**). The
earlier TypeSpec front end has been retired.

[decisions]: notes/derive-front-end-decisions.md

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
crates/fluessig-derive          the Rust derive front end (entities as structs).
crates/fluessig-derive-macros   its proc-macros: #[derive(Entity)], catalog!, ...
crates/cargo-fluessig           the `cargo fluessig emit` subcommand.
crates/derive-demo              worked examples + gates for the derive front end.
notes/                          design docs (design.md, derive-front-end*.md).
scripts/                        dev.sh (setup), coverage.sh.
```

## Build & test

```sh
scripts/dev.sh                       # build the Rust workspace
cargo test                           # engine + derive-crate + fixture tests
scripts/coverage.sh                  # cargo-llvm-cov summary (--html for a report)
```

## Consumers

[entl] and [disponent] both keep their schema as a fluessig catalog and
generate from it at build time. They locate this repo via `FLUESSIG_DIR` — a
sibling `../fluessig` checkout by default, or a pinned clone in CI — and run the
derive front end plus `fluessig-gen` from their own `scripts/gen.sh`.

[entl]: https://github.com/zmaril/entl
[disponent]: https://github.com/zmaril/disponent

## Contributing

Issues and PRs welcome. PR titles follow [Conventional Commits] (`type(scope):
summary`) — CI checks it. Run `cargo fmt`, `cargo clippy -- -D warnings`, and
`cargo test` before pushing.

[Conventional Commits]: https://www.conventionalcommits.org

## Conventions & gotchas

- **Byte-stable catalog output.** The derive front end emits a deterministic
  `catalog.json`; consumers commit the output and regenerate when their schema
  changes. A catalog reorder is a real regression — the drift guard catches it.
- **Enums carry their wire value.** The front end lowers an enum to its members
  and keeps the wire value when it differs from the name; on the SQL side an
  enum column is `text`.
- **Conventional Commits.** Commit messages follow the Conventional Commits
  spec.

## License

MIT © Zack Maril
