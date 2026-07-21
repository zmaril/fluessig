#!/usr/bin/env sh
# Regenerate the committed node binding from the hand-authored api.json +
# catalog.json:
#   run fluessig-gen to (re)write src/generated.rs (the napi surface).
#
# After an api.json change, re-run this, then update src/core_impl.rs to match the
# regenerated `TickerCore` trait, and re-run ./run.sh. Do NOT hand-edit the
# generated file — it must be reproducible from here.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
DDL_OUT="$(mktemp)"
trap 'rm -f "$DDL_OUT"' EXIT

cd "$ROOT"

echo "== generating the node napi surface with fluessig-gen =="
cargo run -q --bin fluessig-gen -- \
    "$HERE/catalog.json" "$DDL_OUT" \
    --api "$HERE/api.json" \
    --node "$HERE/src/generated.rs" \
    --banner-note "straitjacket-allow-file:duplication — the generated napi surface mirrors the sibling python demo's by design (one uniform core shape per backend)."

echo "== regenerated callback-demo-node artifacts =="
