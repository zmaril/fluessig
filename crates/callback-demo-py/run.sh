#!/usr/bin/env sh
# Build the callback-demo-py cdylib, expose it as an importable python extension
# module (rename the cdylib to `callback_demo.so` in a scratch dir), run the
# Python consumer, and fail (nonzero) if the consumer fails. Touches nothing
# outside the cargo target + a scratch temp dir.
#
#   PYTHON    — the python interpreter (default: python3)
#   PROFILE   — debug (default) or release
set -eu

PYTHON="${PYTHON:-python3}"
PROFILE="${PROFILE:-debug}"

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# pyo3 needs to know which interpreter to build against (headers + abi).
PYO3_PYTHON="$(command -v "$PYTHON")"
export PYO3_PYTHON

echo "== building callback-demo-py cdylib (profile: $PROFILE, PYO3_PYTHON=$PYO3_PYTHON) =="
if [ "$PROFILE" = "release" ]; then
    ( cd "$ROOT" && cargo build -p callback-demo-py --release )
else
    ( cd "$ROOT" && cargo build -p callback-demo-py )
fi

LIBDIR="$ROOT/target/$PROFILE"
# The cdylib basename differs by platform (.so on Linux, .dylib on macOS).
if [ -f "$LIBDIR/libcallback_demo.so" ]; then
    SRC="$LIBDIR/libcallback_demo.so"
elif [ -f "$LIBDIR/libcallback_demo.dylib" ]; then
    SRC="$LIBDIR/libcallback_demo.dylib"
else
    echo "cdylib not found under $LIBDIR" >&2
    ls -l "$LIBDIR" >&2 || true
    exit 1
fi

# python imports `callback_demo` from a file literally named `callback_demo.so`.
cp "$SRC" "$WORK/callback_demo.so"

echo "== running python consumer =="
CALLBACK_MODULE_DIR="$WORK" "$PYTHON" "$HERE/consumer.py"

echo "== callback-demo-py round-trip OK =="
