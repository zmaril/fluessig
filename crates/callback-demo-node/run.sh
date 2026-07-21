#!/usr/bin/env sh
# Build the callback-demo-node cdylib, expose it as a loadable node addon
# (rename the cdylib to `callback_demo_node.node`), run the JS consumer, and fail
# (nonzero) if the consumer fails. Touches nothing outside the cargo target + a
# scratch copy of the addon.
#
#   PROFILE   — debug (default) or release
set -eu

PROFILE="${PROFILE:-debug}"

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"

echo "== building callback-demo-node cdylib (profile: $PROFILE) =="
if [ "$PROFILE" = "release" ]; then
    ( cd "$ROOT" && cargo build -p callback-demo-node --release )
else
    ( cd "$ROOT" && cargo build -p callback-demo-node )
fi

LIBDIR="$ROOT/target/$PROFILE"
# The cdylib basename differs by platform (.so on Linux, .dylib on macOS).
if [ -f "$LIBDIR/libcallback_demo_node.so" ]; then
    SRC="$LIBDIR/libcallback_demo_node.so"
elif [ -f "$LIBDIR/libcallback_demo_node.dylib" ]; then
    SRC="$LIBDIR/libcallback_demo_node.dylib"
else
    echo "cdylib not found under $LIBDIR" >&2
    ls -l "$LIBDIR" >&2 || true
    exit 1
fi

# node loads a `.node` file; the napi addon is just the renamed cdylib.
ADDON="$LIBDIR/callback_demo_node.node"
cp "$SRC" "$ADDON"

echo "== running node consumer =="
CALLBACK_ADDON="$ADDON" node "$HERE/consumer.mjs"

echo "== callback-demo-node round-trip OK =="
