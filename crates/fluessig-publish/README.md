# fluessig-publish

A **dumb, explicit, commit-style-agnostic** publishing tool.

It publishes a package to a registry using an **explicit version** you pass in
and **explicit inputs** — no changesets, no commit-message parsing, no version
inference, ever. It never requires you to follow any commit style. Registry
adapters shell out to the **native tool** for each registry and inherit that
tool's own auth; this crate handles no tokens.

**Dry-run is the default.** A real publish happens only when you pass
`--confirm`.

This crate is deliberately self-contained: its only dependencies are
third-party crates (no dependency on the `fluessig` library or any
catalog/codegen machinery), so it can be lifted into its own repo later.

## CLI

```
fluessig publish
  --to <crates|npm|pypi|gems>   target registry (required)
  --path <DIR>                  package dir where the manifest lives (required;
                                relative to --repo when --ref is set)
  --version <V>                 explicit version to stamp in (required — no inference)
  --ref <SHA|TAG>               optional: publish from this git ref, checked out
                                into an ISOLATED worktree of --repo
  --repo <DIR>                  git repo root for --ref (default ".")
  --package <NAME>              optional label for messages
  --artifact <PATH>             repeatable: prebuilt outputs to include (.node, wheel, .gem)
  --readme <PATH>               optional: dropped into the package as README.md
  --manifest <PATH>             optional: explicit manifest path override
  --confirm                     actually publish. WITHOUT it, everything is a DRY RUN.
```

### The safe staging model

Nothing in your source tree is ever mutated. On each run the tool:

1. Resolves the source package dir. With `--ref`, it does
   `git worktree add --detach` into a throwaway temp dir and reads the package
   from there.
2. Copies that package into a fresh temp **staging** dir.
3. In staging only: stamps the version → drops the readme (if any) as
   `README.md` → places artifacts (if any) → runs the registry dry-run (or the
   real publish with `--confirm`).
4. Removes the git worktree and lets the temp dirs drop.
5. Prints a clear summary of what was stamped, what command ran, and whether it
   was a dry run.

## Adapters

| Registry | Native tool         | Version stamped in         | Dry-run                          |
|----------|---------------------|----------------------------|----------------------------------|
| `crates` | `cargo`             | `[package].version` (TOML) | `cargo publish --dry-run`        |
| `npm`    | `npm`               | `"version"` (package.json) | `npm publish --dry-run`          |
| `pypi`   | `uv`                | `[project].version` (TOML) | `uv publish --dry-run`           |
| `gems`   | `gem`               | `.version` line (gemspec)  | **none exists** — see below      |

- **crates** — `cargo publish --dry-run --allow-dirty --no-verify`. With
  `--confirm`: `cargo publish --allow-dirty`.
- **npm** — `npm publish --dry-run` in staging. With `--confirm`: `npm publish`.
  Version is set by editing `package.json` directly (not `npm version`).
- **pypi** — if you pass `--artifact`s they are placed in `staging/dist/`;
  otherwise `uv build` produces `dist/*`. Then `uv publish --dry-run` validates
  the artifacts against the index without uploading. With `--confirm`:
  `uv publish`.
- **gems** — if you pass a prebuilt `.gem` via `--artifact` it is used;
  otherwise `gem build <gemspec>` produces one (which validates the gemspec).

## Honest capability edges

- **gems has no registry dry-run.** There is no `gem push --dry-run`. In dry-run
  mode the tool builds the `.gem` (validating the gemspec — the closest gems
  gets to a dry-run) and prints exactly what a real publish would run, e.g.:

  ```
  gems: no registry dry-run exists. Built toygem-1.2.3.gem (gemspec validated). Nothing was pushed.
  Would run: `gem push toygem-1.2.3.gem`
  Re-run with --confirm to actually push.
  ```

  It does **not** fake a dry-run.

- **Workspace-inherited versions (first pass).** For `crates`, if
  `[package].version` is absent or set to `{ workspace = true }`, stamping bails
  with an honest error — a standalone package copy can't stamp a version that
  lives in the workspace root. Set the version in the workspace root instead.
  The same idea applies to a `dynamic` version in a pyproject: the pypi adapter
  bails rather than guess.

## Verifying it

`tests/verify_dry_run.sh` builds the binary, creates four minimal standalone toy
fixtures (a crate, an npm package, a pyproject/hatchling project, a gem), and
runs each through a real dry run. The Rust unit tests (`cargo test -p
fluessig-publish`) cover version stamping and the recursive copy helper with no
network or external tools.
