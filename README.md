<p align="center"><img src="assets/logo.png" alt="fluessig" width="200"></p>

# fluessig

Describe a typed entity graph **once** (TypeSpec); project it **everywhere** — DDL,
ORM models, format codecs, language bindings, and an Arrow-fed data plane.

fluessig is a **build-time schema tool**, not a runtime library. You author a `.tsp`
with the fluessig decorators, the emitter lowers it to `catalog.json` (the data model)
and `api.json` (the op surface), and the `fluessig-gen` binary generates per-dialect DDL,
ORM/typed-table code, and language-binding surfaces from that catalog. See
[notes/design.md](./notes/design.md).

## Install

fluessig has two halves — a Rust crate (the generator) and a Node emitter (the
TypeSpec front end):

```sh
# the generator (fluessig-gen on your PATH)
cargo install --git https://github.com/zmaril/fluessig fluessig
# or from a checkout:
cargo install --path .

# the emitter (TypeSpec -> catalog.json + api.json)
cd emitter && npm install
```

Consumers typically pin fluessig by git ref and invoke both at codegen time. Needs a
Rust toolchain (1.75+) and Node 20+.

## Usage

```sh
# .tsp -> catalog.json + api.json (beside the input, or --out <dir>)
(cd emitter && node emit.mjs ../entl.tsp)

# catalog.json -> generated code (DDL module, ORM models, typed tables, bindings)
cargo run --bin fluessig-gen -- catalog.json out/schema_gen.rs --docs out/schema_docs.json
```

Generated-file banners are consumer-agnostic: fluessig names the banner after the
catalog's own `source` and bakes in no consumer paths; pass anything project-specific
(a lint-suppression marker, a regenerate hint) via `--banner-note`.

## README multiplexing

`fluessig-gen` also renders **one Markdown template per target language**, so a
single quickstart shows Rust, Node/Bun, Python, or Ruby code depending on the
output target. The template stays valid Markdown — every directive is an HTML
comment on its own line, so it reads fine on GitHub unrendered.

```sh
# fan out over every language: {lang} in the path expands to the slug
cargo run --bin fluessig-gen -- catalog.json out/schema_gen.rs \
  --readme quickstart.tpl.md --readme-out 'out/README.{lang}.md' --readme-pkg entl

# or render a single language to a fixed path
cargo run --bin fluessig-gen -- catalog.json out/schema_gen.rs \
  --readme quickstart.tpl.md --readme-out README.md --readme-lang rust
```

Flags:

| flag | meaning |
| --- | --- |
| `--readme <template.md>` | activate README rendering |
| `--readme-out <pattern>` | output path; a `{lang}` in it fans out over every target |
| `--readme-lang <slug>` | the single target when `--readme-out` has no `{lang}` |
| `--readme-langs <slug,…>` | subset to render when the pattern has `{lang}` (default: all four) |
| `--readme-pkg <name>` | package name for `{pkg}` in install lines (default: the catalog name, else `yourpkg`) |

The four language slugs are `rust`, `node`, `python`, `ruby`.

### Template directives

- **Interpolation** — `{{ key }}` (whitespace inside the braces is flexible).
  Keys: `lang` (display name, e.g. `Python`), `lang.slug`, `lang.fence` (the
  code-fence tag), `lang.install` (the install one-liner with `{pkg}` already
  substituted), `lang.ext` (source extension), `pkg`, and `catalog.name`. An
  unknown key is an error — the renderer never silently drops.
- **`<!-- fl:only SLUG [SLUG…] -->` … `<!-- fl:end -->`** — keep the enclosed
  lines only for the listed targets. **`<!-- fl:except SLUG [SLUG…] -->`** keeps
  them for every target *but* those listed. Slugs are space- or comma-separated.
- **`<!-- fl:each -->` … `<!-- fl:end -->`** — the multiplexer. Inside,
  `<!-- fl:lang SLUG -->` markers split the block into per-language sections;
  anything before the first marker is a shared preamble emitted for every target.
  Only the section matching the target is emitted. `<!-- fl:lang default -->` is a
  fallback; with no match and no default, rendering fails (strict).

Blocks nest (an `fl:only` around an `fl:each`, interpolation anywhere). An
unterminated block, a stray `fl:end`/`fl:lang`, or a missing variant is an error,
so a broken template fails the build rather than emitting something wrong.

## Layout

```
src/                the engine (Rust): IR, catalog loader/validator, SQL back-ends,
                    data codecs, and the binding generator.
src/bin/            fluessig-gen — the code generator CLI.
emitter/            @fluessig/emitter — the catalog printer (TypeSpec -> catalog.json + api.json).
typespec/           @fluessig/typespec — the decorator library (@entity, @key, @compose, ...).
spike/              the format-codec spike that proved the design.
notes/              design.md, findings.md, plan.txt.
tests/              tool tests. entl.tsp + catalog.json + api.json are a committed FIXTURE
                    (a copy of entl's real catalog) the tests run against.
```

## Build & test

```sh
cargo test                                    # the Rust engine + fixture tests
(cd emitter && npm install && node test.mjs)  # the emitter
```

## Consumers

The first consumer is [entl](https://github.com/zmaril/entl), which authors its schema in
`entl.tsp` and generates its DuckDB/Postgres/SQLite DDL, SQLAlchemy models, Drizzle tables,
and its napi/PyO3/Magnus binding surfaces from this tool.

## Contributing

Issues and PRs welcome. PR titles follow
[Conventional Commits](https://www.conventionalcommits.org) (`type(scope): summary`) —
CI checks it. Run `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` before pushing.

## License

[MIT](./LICENSE) © Zack Maril.
