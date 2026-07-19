#!/usr/bin/env bash
# Stand up the fluessig dev environment: build the Rust workspace, so
# `cargo test` runs. Safe to run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "building the workspace…"
cargo build

echo
echo "dev environment ready:"
echo "  cargo test                       # the Rust engine + fixture tests"
