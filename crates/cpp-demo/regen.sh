#!/usr/bin/env sh
# Regenerate the committed cpp-demo artifacts:
#   1. emit catalog.json from src/schema.rs (the `#[fluessig::export]` entity root),
#      then
#   2. run fluessig-gen to (re)write src/generated.rs + cpp_demo.h + cpp_demo.hpp
#      from catalog.json + the HAND-AUTHORED api.json.
#
# api.json is HAND-AUTHORED (not derive-emitted): it carries a `Ticker` interface
# whose `on_tick` is a `Shape::Subscription` op with an `ApiType::Callback` param,
# a shape the derive front end cannot yet spell (mirrors the callback-demo-node/py
# crates, which are also hand-authored). The Store surface in api.json still
# matches src/schema.rs's derive-authored ops.
#
# After a schema change, re-run this, then update src/core_impl.rs to match the
# regenerated `StoreCore` / `TickerCore` traits, and re-run ./run.sh. Do NOT
# hand-edit the generated files — they must be reproducible from here.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
SCHEMA_OUT="$(mktemp)"
trap 'rm -f "$SCHEMA_OUT"' EXIT

cd "$ROOT"

echo "== emitting catalog.json from the derive-authored entity root =="
cargo run -q --bin cpp-demo-emit-catalog > "$HERE/catalog.json"

echo "== generating the C ABI layer + headers with fluessig-gen =="
cargo run -q --bin fluessig-gen -- \
    "$HERE/catalog.json" "$SCHEMA_OUT" \
    --api "$HERE/api.json" \
    --cpp "$HERE/src/generated.rs" \
    --cpp-header "$HERE/cpp_demo.h" \
    --cpp-hpp "$HERE/cpp_demo.hpp" \
    --banner-note "straitjacket-allow-file:duplication — the generated per-op C ABI shims repeat a fixed marshalling shape by design."

echo "== regenerated cpp-demo artifacts =="
