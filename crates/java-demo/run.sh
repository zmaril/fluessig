#!/usr/bin/env bash
# End-to-end Java (JNI) round-trip proof:
#
#   java-demo-schema  ──emit──▶  catalog.json + api.json
#        │
#        ▼  fluessig-gen --java
#   src/generated.rs  +  java/fluessig/*.java   (REGENERATED here — never hand-copied)
#        │
#        ▼  cargo build -p java-demo
#   libfluessig.so   (staged onto java.library.path)
#        │
#        ▼  javac generated + Main.java ; java Main
#   asserted, order-sensitive output
#
# Exits non-zero on any build failure or output mismatch. Touches only scratch
# dirs under target/ plus the committed generated artifacts (which must match).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
cd "$ROOT"

GEN_IN="$ROOT/target/java-demo-gen"
STAGE="$ROOT/target/java-demo-run"
OUT="$STAGE/out"
mkdir -p "$GEN_IN" "$STAGE" "$OUT"

echo "== 1. build fluessig-gen + emit the schema =="
cargo build --release -p fluessig >/dev/null
cargo run  --release -q -p java-demo-schema --bin emit-catalog > "$GEN_IN/catalog.json"

# api.json is HAND-AUTHORED (not derive-emitted): the derive front end cannot yet
# spell a callback param, so the `Ticker` interface (a `Shape::Subscription`
# `on_tick` with a `Callback<(int32)>` listener) lives in the committed
# crates/java-demo/api.json — mirroring cpp-demo. Its Store surface still matches
# java-demo-schema's derive-authored ops (kept in sync by hand).

echo "== 2. regenerate the JNI glue + .java from catalog + the hand-authored api.json =="
"$ROOT/target/release/fluessig-gen" \
  "$GEN_IN/catalog.json" "$GEN_IN/schema-throwaway.rs" \
  --api "$HERE/api.json" \
  --java "$HERE/src/generated.rs" \
  --java-src-out "$HERE/java"

echo "== 3. build the cdylib and stage libfluessig.so =="
cargo build -p java-demo >/dev/null
# Cargo names the artifact after the crate (libjava_demo.so); the generated Java
# loads `fluessig` → stage a copy as libfluessig.so on java.library.path.
cp "$ROOT/target/debug/libjava_demo.so" "$STAGE/libfluessig.so"

echo "== 4. javac the generated classes + Main.java =="
javac -d "$OUT" "$HERE"/java/fluessig/*.java "$HERE/Main.java"

echo "== 5. run Main against the real cdylib =="
ACTUAL="$(java -Djava.library.path="$STAGE" -cp "$OUT" Main)"

read -r -d '' EXPECTED <<'EOF' || true
version=store-1.0
checked(abc)=103
count(stream)=6
item 1 alpha
item 2 beta
item 3 gamma
stream-closed
throw-ok: boom requested for key boom
ticks-before-unsub=[0, 1]
ticks-after-unsub=[0, 1]
callback-ok: Java Consumer fired [0, 1] from Rust, silent after unsubscribe
EOF

echo "---- actual ----"
echo "$ACTUAL"
echo "----------------"

if [ "$ACTUAL" != "$EXPECTED" ]; then
  echo "FAIL: output did not match expected:" >&2
  diff <(printf '%s\n' "$EXPECTED") <(printf '%s\n' "$ACTUAL") >&2 || true
  exit 1
fi

echo "PASS: sync + infallible + async + stream + throw + callback/subscription all round-tripped."
