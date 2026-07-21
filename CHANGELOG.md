# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
adhere to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Removed
- The TypeSpec front end — the `@fluessig/emitter` Node emitter, the
  `@fluessig/typespec` decorator library, all `.tsp` sources, and Node from the
  toolchain. The Rust `#[derive(Entity)]` front end is now the only front end;
  `cargo fluessig emit` replaces `node emit.mjs`. The emitted `catalog.json` /
  `api.json` are kept as frozen fixtures (entl's parity target + the engine's
  dogfood catalogs).

### Added
- Java (JNI) bindgen backend (`src/bindgen/java.rs`): projects the op surface
  into Java classes backed by a Rust JNI `cdylib`. Emits two artifacts — the Rust
  JNI glue (`Java_fluessig_<Class>_<method>` extern fns + `<Interface>Core`
  traits) and the `.java` classes (package `fluessig`, `System.loadLibrary`). Op
  shapes map: sync infallible → direct blocking native; sync fallible → a
  `RuntimeException` throw seam; `#[fluessig(async)]` → `CompletableFuture` via
  `supplyAsync`; stream → a poll cursor exposing `Optional<Item> next()`. Enums
  ride as their wire `String` (+ a generated `enum` class), unions as their JSON
  envelope `String`, `ArrowBatch` as `byte[]`, models as first-class Java
  objects. JNI (the `jni` crate) was chosen over Panama/FFM: synchronous (matches
  sync-default + the php/ruby precedent), constructs Java objects directly, and
  runs on every JDK vs FFM needing 22+. A real javac/JVM round-trip
  (`crates/java-demo{,-schema}`, `crates/java-demo/run.sh`) and an additive,
  non-required `java-roundtrip` CI leg prove it end to end. See
  `notes/java-backend.md`.
- Node backend: a per-stream-op error model. The DEFAULT (unannotated) is
  idiomatic native TS — a mid-stream core failure REJECTS the pull, so the
  `for await` loop throws (safe by default, no silent-swallow). Opting a stream
  op into `@streamError` flips it to error-AS-EVENT (mirror-a-library mode, e.g.
  pi): the failure is yielded as a terminal error EVENT (a value via
  `napi::Either`) and the stream then completes, never rejecting. Setup/creation
  (ctor, unary, stream construction) always throw a napi error in both modes. A
  `Poll::Failed(String)` arm carries the core mid-stream `Result` failure through
  the shared `Poll<T>` in either mode; a core that emits its terminal error as a
  normal union variant still flows through `Poll::Item` unchanged.
- `@streamError(...)` TypeSpec decorator (+ `stream_error` on `ApiOp` and the api
  schema): presence opts a stream op into error-as-event mode; a bare
  `@streamError` uses pi's `{ type: "error", reason, error }` verbatim, and the
  optional args override the emitted event JS shape — tag field js-name/value and
  reason/message field js-names. Loader-checked to be stream-only. A core wanting
  pi's exact error-as-event contract (e.g. atilla's pi harness ops) must annotate
  its stream ops `@streamError`.

## [0.1.0] - 2026-07-07

### Added
- Initial release, extracted from the entl monorepo into its own project.
- The engine (`src/`): IR, catalog loader/validator, per-dialect SQL DDL back-ends
  (Postgres/SQLite/DuckDB), data codecs, and the language-binding generator.
- `fluessig-gen`: the code generator CLI (DDL module, ORM read planes, typed tables,
  napi/PyO3/Magnus binding surfaces).
- `@fluessig/emitter` + `@fluessig/typespec`: the TypeSpec front end.
- Consumer-agnostic generated-file banners: named after the catalog's own `source`,
  with all consumer-specific prose supplied via `--banner-note`.

[Unreleased]: https://github.com/zmaril/fluessig/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/zmaril/fluessig/releases/tag/v0.1.0
