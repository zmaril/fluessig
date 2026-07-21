#!/usr/bin/env sh
# The compile-to-wasm32 gate for fluessig's `ApiType::Callback` + `Shape::
# Subscription` lowering on the wasm-bindgen backend.
#
# Unlike the cpp/java/node/python/ruby demos there is no in-repo runnable host
# harness (a browser/node wasm-bindgen-test runner is not part of the standard CI
# image). Instead, the PROOF that the generated callback + subscription binding is
# valid wasm-bindgen code is that the crate — the committed generated glue
# (src/generated.rs) plus the hand-written core (src/core_impl.rs) — COMPILES to
# `wasm32-unknown-unknown`. That is exactly how the wasm backend was validated
# before a runnable harness existed.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"

cd "$ROOT"

echo "== ensuring the wasm32-unknown-unknown target is installed =="
rustup target add wasm32-unknown-unknown

echo "== compiling callback-demo-wasm to wasm32-unknown-unknown =="
cargo build -p callback-demo-wasm --target wasm32-unknown-unknown

echo "== callback-demo-wasm compiled to wasm32 (callback + subscription binding is valid wasm-bindgen) =="
