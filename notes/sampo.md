# Wrapping sampo for multi-ecosystem publishing

*One logical library, four registries: can fluessig lean on sampo to release it,
and what should the wrapper actually be?*

**Status:** exploration — 2026-07-08

---

## 1. The problem

Every fluessig consumer ships **one logical library across several package
registries at once**. entl is the extreme case: a Rust core + CLI on crates.io,
a napi package `@x/node` on npm, a PyO3 wheel on PyPI (built via `maturin`), and
a Magnus gem on RubyGems. disponent is the same shape one binding smaller:
`disponent-core` + `disponent-cli` on crates.io and `@disponent/node` on npm.
"Publishing" for one of these projects is therefore not `cargo publish` — it is a
**coordinated, version-locked release of a single library across ecosystems**,
where the crate, the npm package, and the wheel that wrap the same core must move
together or not at all.

Today the fleet has **zero release automation**, at any layer:

- **fluessig publishes nothing.** It is consumed by git ref — disponent-core
  pins `fluessig = { git = "…", rev = "15a2e1d…" }`
  (`disponent/crates/disponent-core/Cargo.toml`) — and its npm halves are marked
  `"private": true` at `version` `0.0.0` (`fluessig/emitter/package.json`,
  `fluessig/typespec/package.json`). fluessig itself is `0.1.0` on no registry
  (`fluessig/Cargo.toml`).
- **Consumers are pinned at placeholder versions.** disponent's workspace is
  `version = "0.1.0"` (`disponent/Cargo.toml`); `@disponent/node` is `0.0.0`.
  entl is `0.0.0` across the board — workspace, `@x/node`, the `entl` wheel — and
  the Magnus **gemspec is not even written yet**.
- **No changelogs** anywhere except fluessig's own hand-kept `CHANGELOG.md`.

So the question is not "how do we type `cargo publish`" — it is "what runs the
whole coordinated multi-registry release, and does fluessig have a role in it?"

## 2. What sampo is

**sampo** (bruits/sampo, MIT, pre-1.0) is a **changesets-style, Rust,
multi-ecosystem monorepo release orchestrator** — the model that JS
`changesets` / Lerna made popular, rebuilt in Rust and generalized past npm.
Three pieces ship:

1. a **CLI** (`sampo`) — `crates/sampo/src/cli.rs`;
2. a **GitHub Action** implementing the Release-PR flow;
3. a **GitHub bot** that nags PRs missing a changeset.

The workflow is the changesets discipline:

- `sampo add` writes a changeset to `.sampo/changesets/*.md` — a Markdown file
  whose frontmatter maps `<ecosystem>/<package>` to a bump level
  (`minor` / `major` / `patch`).
- `sampo release` **consumes** the pending changesets, computes the SemVer bumps
  (including the internal-dependency cascade, transitively, and fixed/linked
  version grouping), writes each package's `CHANGELOG.md`, and bumps manifests.
- `sampo publish` **publishes** to the registries in **topological order** and
  creates annotated git tags.

Publishing is delegated to native tooling through per-ecosystem **adapters**
(`crates/sampo-core/src/adapters.rs`): `cargo publish` for crates.io
(`crates/sampo-core/src/adapters/cargo.rs:54`), one of `npm` / `pnpm` / `yarn` /
`bun` `publish` for npm, `mix` / `gleam` for Hex, `uv build` + `uv publish` for
PyPI (`crates/sampo-core/src/adapters/pypi/pip.rs:257`), and `composer validate`
for Packagist. The set of ecosystems it knows is declared in
`crates/sampo-core/src/config.rs:17-38`.

Current released versions at time of writing: CLI **0.19.0**, core **0.15.0**,
Action **0.17.0**.

## 3. Does it fit? (Q1)

**Strong fit for the cargo + npm + PyPI parts** — exactly the three ecosystems
entl and disponent need — and it covers the *whole* chain end to end: changeset →
version bump → changelog → tag → GitHub release → actual registry publish →
dependency-graph ordering. The honest division of labor:

**Covered by sampo:**

- SemVer bumps from changesets, with the internal-dependency cascade;
- per-package `CHANGELOG.md` generation;
- git tagging and GitHub releases;
- the actual publishes to crates.io, npm, and PyPI;
- topological publish ordering;
- fixed / linked version grouping.

**Still owned by the consumer or CI — NOT by sampo:**

- **RubyGems — no adapter exists.** sampo's ecosystem set is
  cargo / npm / hex / pypi / packagist only (`config.rs:17-38`); the prototype
  confirmed `ruby` / `rubygems` / `gem` all fail as `unsupported package kind`.
  entl's Magnus gem cannot ride the same train.
- **The platform-binary build matrix.** sampo publishes what is **already
  built** — it does not cross-build artifacts. The napi `.node` per OS/arch, the
  abi3 wheels, and the gem are `cdylib` build matrices, not single files, and
  producing them stays CI's job. sampo runs *after* that matrix, not instead of
  it.
- **A prerequisite fluessig itself creates.** `disponent-core` depends on
  `fluessig` by git rev with no version, and crates.io **forbids git deps
  without a version**. So `disponent-core` is not crates.io-publishable until
  fluessig is on crates.io and depended on **by version**. The prototype hit
  exactly this wall at `cargo publish --dry-run` (§4).

## 4. Prototype: sampo dry-run on disponent (evidence)

A thin, **dry-run-only** prototype ran sampo against a throwaway copy of
disponent (the `origin` remote removed; the real `/workspace/disponent`
untouched). **Nothing was published; no tags or releases were created on any real
repo.** sampo **0.19.0** was installed via `cargo install sampo`.

**Discovery is partial.** sampo found the three cargo crates —
`cargo/disponent-core`, `cargo/disponent-cli`, `cargo/disponent-node` — but did
**not** discover `@disponent/node` as an npm package. Every npm-side reference
missed:

```
@disponent/node    ERROR: Invalid package reference '@disponent/node':
                          unsupported package kind '@disponent'
npm/@disponent/node  MISS: Package ... not found in the workspace
npm/disponent        MISS
```

The cause is structural: disponent has **no npm workspace root** — there is no
top-level `package.json` with a `workspaces` field (the only `package.json` in
the repo is `crates/disponent-node/package.json`, and it declares no
`workspaces`). sampo's npm adapter finds no npm workspace and therefore no npm
packages; the `crates/disponent-node` directory is seen **only** as the cdylib
crate `cargo/disponent-node`, its co-located `package.json` invisible. The mixed
cargo+npm graph is **half-discovered**.

**Fixed-group cascade works.** With the three cargo crates grouped `fixed`, one
`minor` changeset on `disponent-core` alone cascaded a single shared bump across
the whole group:

```
Planned releases:
  disponent-cli: 0.1.0 -> 0.2.0
  disponent-core: 0.1.0 -> 0.2.0
  disponent-node: 0.1.0 -> 0.2.0
Dry-run: no files modified, no tags created.
```

This also covers the internal dependency cascade — `disponent-cli` and
`disponent-node` both depend on `disponent-core`.

**The publish plan is correct and topological.** `disponent-core →
disponent-cli → disponent-node` (dependency first), all targeting crates.io:

```
Publish plan:
  - disponent-core (Cargo)
  - disponent-cli (Cargo)
  - disponent-node (Cargo)
Running: cargo publish --manifest-path .../crates/disponent-core/Cargo.toml --dry-run --allow-dirty
error: failed to verify manifest at `.../crates/disponent-core/Cargo.toml`
Caused by:
  all dependencies must have a version requirement specified when publishing.
  dependency `fluessig` does not specify a version
```

That failure is **disponent's own crates.io blocker, not a sampo bug** — it is
the §3 prerequisite (unversioned `fluessig` git dep) surfacing exactly where
predicted. No npm package appears in the plan, consistent with the discovery
gap.

**Snags found:**

- **A single shared tag is not expressible.** A first attempt with
  `tag_format = "disponent-v{version}"` aborted:
  `tag conflict: 'cargo/disponent-core' and 'cargo/disponent-cli' both render to
  git tag 'disponent-v0.1.0'`. sampo tags **per package**, so `tag_format` must
  include `{package_name}` (or `{ecosystem}`). The shared *version* comes from
  the `fixed` group, not from a shared tag.
- **Detached HEAD refused** — sampo needs a real branch
  (`Unable to determine current git branch (detached HEAD)`).
- **`disponent-core` not crates.io-publishable** until fluessig is versioned
  (above).

The `.sampo/config.toml` the prototype settled on:

```toml
# Sampo configuration
version = 1

[github]
repository = "zmaril/disponent"

[git]
# Tags are PER PACKAGE, so {package_name} is required for uniqueness — a single
# shared tag makes every crate render the same tag and sampo aborts.
tag_format = "{package_name}-v{version}"

[changelog]
show_commit_hash = true
show_acknowledgments = true

[packages]
# disponent's publishable units release together as ONE version (the logical
# library). Modeled on sampo's own fixed = [["cargo/sampo", "npm/sampo"]].
# NOTE: only the three cargo crates are discoverable — no npm workspace root, so
# @disponent/node is NOT discovered and cannot be referenced as npm/@disponent/node.
fixed = [["cargo/disponent-core", "cargo/disponent-cli", "cargo/disponent-node"]]
ignore_unpublished = false
```

That config is committed as a static example at
`notes/examples/sampo/disponent.config.toml` — it is **not** added into the real
disponent repo.

## 5. What "fluessig wraps sampo" should look like (Q2)

Three shapes are on the table:

- **(a) codegen** — fluessig generates sampo config and scaffolding **into**
  consumer repos as a codegen output, the same way it emits DDL and binding glue.
- **(b) a CLI verb** — `fluessig publish` / `fluessig release` shells into sampo.
- **(c) a library API** — fluessig links sampo and drives it in-process.

**Recommendation: (a), scoped thin — not (b) or (c).**

The reasoning starts from fluessig's identity. The README states it plainly:
fluessig is a **"build-time schema tool, not a runtime library"**
(`fluessig/README.md`). Options (b) and (c) both make fluessig a **release-time
runtime dependency** of every consumer — a new maintenance seam, a new failure
surface, and a layer of indirection that hides sampo for no gain. sampo is a
perfectly good CLI already; wrapping its command line adds nothing.

fluessig's **unique** leverage is different: it is the only tool in the chain
that **knows the binding surfaces it just emitted**. So it can generate exactly
what sampo **cannot infer**:

- the `[packages]` **fixed group** — every binding of one logical library moves
  under one version, which fluessig knows because it produced them;
- the **npm workspace root** (`package.json` with `workspaces:
  ["crates/*-node"]`) that makes `@x/node` discoverable — this **directly closes
  the prototype's discovery gap** (§4);
- a sensible `tag_format` (with `{package_name}`, per the §4 snag);
- a release GitHub Action workflow.

This is the **same shape as the in-flight README-multiplexing work**
(`src/readme.rs`, PR #16), which renders non-code text artifacts from the
catalog. A workflow/config generator is that shape again: plain strings, no
rustfmt pass. Mechanically it slots into the existing per-flag `fluessig-gen`
seam that each consumer already drives from `scripts/gen.sh` / `bun run gen` — a
new output flag, e.g. `--release-scaffold <dir>` (or a `fluessig init-release`
subcommand), emitting `.sampo/config.toml` + the npm workspace-root
`package.json` + `.github/workflows/release.yml`.

**Temper this honestly.** sampo already auto-discovers packages, so fluessig's
*incremental* value is modest: the fixed/linked grouping, the npm-workspace-root
fix, and one standard workflow. That is worth generating **only once ≥2
consumers actually need it**. For a single repo, roughly 30 lines of
hand-written `.sampo/config.toml` (like §4's) is enough, and building a generator
would be premature.

**UX sketch for disponent.** A developer runs `sampo add` on a change; the PR
carries the changeset; on merge, the release Action opens a **Release PR** with
the accumulated changelog; merging that PR publishes `disponent-core` +
`disponent-cli` to crates.io and — with the fluessig-generated npm workspace
root — `@disponent/node` to npm, topologically ordered and tagged. Ruby (for
entl) stays on a side script until sampo grows a gem adapter.

## 6. Cost and risks (Q3)

- **Maturity / bus factor.** sampo is pre-1.0 with a single dominant maintainer
  (one author wrote roughly half of recent commits), ~206 stars, and breaking
  changes still landing (e.g. constraint handling changed in 0.14.0). Mitigation:
  it is MIT Rust one could fork, it is well-tested (~825 `#[test]` cases), and it
  is dogfooded — sampo releases itself with sampo.
- **Config lock-in.** A `.sampo/` directory plus a changeset discipline is a real
  commitment, and it is a **second mental model** alongside the fleet's
  conventional-commits habit. They coexist (a changeset is not a conventional
  commit), but the team carries two models.
- **CI coexistence.** Conventional-commits enforcement is orthogonal to sampo
  (fine). Housekeeping *wants* a changelog, so sampo is **synergistic** there.
  But sampo's Release-PR flow **force-pushes a `release/*` branch**, which must be
  squared with the branch protection the housekeeping audit expects.
- **Versus plain scripts.** For a 2–3 crate repo, per-ecosystem publish scripts
  (a `cargo publish` loop plus `npm publish`) are simpler and dependency-free —
  at the cost of changelog generation, coordinated version bumps, and
  dependency-graph ordering. sampo earns its keep once you want a real changelog
  plus a coordinated multi-ecosystem version story; below that it is overkill.

## 7. Recommendation

**Adopt sampo directly, per-repo.** It is a strong fit for the cargo / npm / PyPI
release problem, and the dry-run proved the core mechanics — fixed-group cascade
and a correct topological publish plan — actually work. **Have fluessig
contribute only a thin, opt-in scaffold generator** (npm workspace root +
`.sampo/config.toml` template + release workflow), **not** a `fluessig publish`
runtime wrapper, and build it **only once ≥2 consumers need it**. **Sequence the
prerequisites first:** publish fluessig itself to crates.io and switch consumers
to a versioned dependency, or nothing downstream can publish at all. **Keep
RubyGems on a side script** for entl (or contribute a RubyGems adapter upstream
to sampo).

## 8. Citations / evidence

**sampo source (quoted paths):**

- `crates/sampo-core/src/adapters.rs` — the adapter dispatch.
- `crates/sampo-core/src/adapters/cargo.rs:54` — the `cargo publish` adapter.
- `crates/sampo-core/src/adapters/pypi/pip.rs:257` — the `uv build` / `uv publish`
  adapter.
- `crates/sampo-core/src/config.rs:17-38` — the recognized ecosystem set
  (cargo / npm / hex / pypi / packagist — no Ruby).
- `crates/sampo/src/cli.rs` — the `sampo add` / `release` / `publish` CLI.

**Fleet manifests:**

- `fluessig/Cargo.toml` (`0.1.0`, unpublished),
  `fluessig/emitter/package.json` + `fluessig/typespec/package.json`
  (`private`, `0.0.0`).
- `disponent/Cargo.toml` (`0.1.0`),
  `disponent/crates/disponent-core/Cargo.toml` (the unversioned `fluessig` git
  dep), `disponent/crates/disponent-node/package.json` (`@disponent/node`,
  `0.0.0`).
- entl: workspace / `@x/node` / `entl` wheel all `0.0.0`; no Magnus gemspec yet.

**Related in-flight fluessig work:** `src/readme.rs` / PR #16
(README-multiplexing) — the codegen-of-text-artifacts pattern a release-scaffold
generator would follow.

**Prototype:** the full dry-run evidence (verbatim tool output, the `.sampo/`
config, the no-side-effects check) lives with this branch / PR; the example
config is at `notes/examples/sampo/disponent.config.toml`.
