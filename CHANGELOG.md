# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
adhere to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
