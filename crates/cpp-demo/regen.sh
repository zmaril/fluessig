#!/usr/bin/env sh
# Regenerate the committed cpp-demo artifacts from the derive-authored schema:
#   1. emit catalog.json + api.json from src/schema.rs (the `#[fluessig::export]`
#      op surface), then
#   2. run fluessig-gen to (re)write src/generated.rs + cpp_demo.h + cpp_demo.hpp.
#
# After a schema change, re-run this, then update src/core_impl.rs to match the
# regenerated `StoreCore` trait, and re-run ./run.sh. Do NOT hand-edit the
# generated files — they must be reproducible from here.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
SCHEMA_OUT="$(mktemp)"
trap 'rm -f "$SCHEMA_OUT"' EXIT

cd "$ROOT"

echo "== emitting catalog.json / api.json from the derive-authored schema =="
cargo run -q --bin cpp-demo-emit-catalog > "$HERE/catalog.json"
cargo run -q --bin cpp-demo-emit-api > "$HERE/api.json"

echo "== generating the C ABI layer + headers with fluessig-gen =="
cargo run -q --bin fluessig-gen -- \
    "$HERE/catalog.json" "$SCHEMA_OUT" \
    --api "$HERE/api.json" \
    --cpp "$HERE/src/generated.rs" \
    --cpp-header "$HERE/cpp_demo.h" \
    --cpp-hpp "$HERE/cpp_demo.hpp" \
    --banner-note "straitjacket-allow-file:duplication — the generated per-op C ABI shims repeat a fixed marshalling shape by design."

echo "== regenerated cpp-demo artifacts =="
