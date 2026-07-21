#!/usr/bin/env sh
# Build the callback-demo-ruby cdylib, expose it as a loadable Ruby extension
# (rename the cdylib to `callback_demo_ruby.so`, matching the magnus
# `Init_callback_demo_ruby` symbol), run the Ruby consumer, and fail (nonzero) if
# the consumer fails. Touches nothing outside the cargo target + a scratch copy of
# the extension.
#
#   PROFILE   — debug (default) or release
set -eu

PROFILE="${PROFILE:-debug}"

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"

echo "== building callback-demo-ruby cdylib (profile: $PROFILE) =="
if [ "$PROFILE" = "release" ]; then
    ( cd "$ROOT" && cargo build -p callback-demo-ruby --release )
else
    ( cd "$ROOT" && cargo build -p callback-demo-ruby )
fi

LIBDIR="$ROOT/target/$PROFILE"
# The cdylib basename differs by platform (.so on Linux, .bundle on macOS).
if [ -f "$LIBDIR/libcallback_demo_ruby.so" ]; then
    SRC="$LIBDIR/libcallback_demo_ruby.so"
elif [ -f "$LIBDIR/libcallback_demo_ruby.dylib" ]; then
    SRC="$LIBDIR/libcallback_demo_ruby.dylib"
else
    echo "cdylib not found under $LIBDIR" >&2
    ls -l "$LIBDIR" >&2 || true
    exit 1
fi

# Ruby `require` loads a `.so`; the magnus extension is just the renamed cdylib
# whose `Init_callback_demo_ruby` symbol matches the require name.
STAGE="$ROOT/target/callback-demo-ruby-run"
mkdir -p "$STAGE"
cp "$SRC" "$STAGE/callback_demo_ruby.so"

echo "== running ruby consumer =="
ruby -I "$STAGE" "$HERE/consumer.rb"

echo "== callback-demo-ruby round-trip OK =="
