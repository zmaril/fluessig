#!/usr/bin/env bash
# Stand up the fluessig dev environment: build the Rust workspace and install
# the emitter's npm deps, so `cargo test` and the emitter tests both run.
# Safe to run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "building the workspace…"
cargo build

echo
echo "installing emitter deps…"
(cd emitter && npm install)

echo
echo "dev environment ready:"
echo "  cargo test                       # the Rust engine + fixture tests"
echo "  (cd emitter && node test.mjs)    # the emitter"
