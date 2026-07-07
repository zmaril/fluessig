# fluessig

Describe a typed entity graph **once** (TypeSpec); project it **everywhere** — DDL,
ORM models, format codecs, language bindings, and an Arrow-fed data plane.

fluessig is a **build-time schema tool**, not a runtime library. You author a `.tsp`
with the fluessig decorators, the emitter lowers it to `catalog.json` (the data model)
and `api.json` (the op surface), and the `fluessig-gen` binary generates per-dialect DDL,
ORM/typed-table code, and language-binding surfaces from that catalog. See
[DESIGN.md](./DESIGN.md).

## Layout

```
src/                the engine (Rust): IR, catalog loader/validator, SQL back-ends,
                    data codecs, and the binding generator.
src/bin/            fluessig-gen — the code generator CLI.
emitter/            @fluessig/emitter — the catalog printer (TypeSpec → catalog.json + api.json).
typespec/           @fluessig/typespec — the decorator library (@entity, @key, @compose, …).
spike/              the format-codec spike that proved the design.
tests/              tool tests. entl.tsp + catalog.json + api.json are a committed FIXTURE
                    (a copy of entl's real catalog) the tests run against.
```

## Build & test

```sh
cargo test                                  # the Rust engine + fixture tests
(cd emitter && npm install && node test.mjs)  # the emitter
```

## Lowering a schema

```sh
# .tsp → catalog.json + api.json (beside the input, or --out <dir>)
(cd emitter && node emit.mjs ../entl.tsp)

# catalog.json → generated code (DDL module, ORM models, typed tables, bindings)
cargo run --bin fluessig-gen -- catalog.json out/schema_gen.rs --docs out/schema_docs.json
```

## Consumers

The first consumer is [entl](https://github.com/zmaril/entl), which authors its schema in
`entl.tsp` and generates its DuckDB/Postgres/SQLite DDL, SQLAlchemy models, Drizzle tables,
and its napi/PyO3/Magnus binding surfaces from this tool. entl pins fluessig by git ref and
invokes `fluessig-gen` + the emitter at codegen time.
