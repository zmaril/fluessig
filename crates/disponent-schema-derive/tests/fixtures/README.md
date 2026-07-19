# Vendored disponent schema snapshot

`catalog.json` and `api.json` here are a **static snapshot** of disponent's
TypeSpec-emitted `schema/catalog.json` / `schema/api.json`, copied byte-for-byte
into this crate so the `parity.rs` gate is self-contained in fluessig CI (which
checks out only fluessig — the disponent sibling repo is not present).

Unlike the entl parity fixtures (which fluessig's own codegen regenerates), these
are an **external** snapshot. If disponent's schema changes, refresh these files by
copying `disponent/schema/{catalog,api}.json` over them and re-running
`cargo test -p disponent-schema-derive`.
