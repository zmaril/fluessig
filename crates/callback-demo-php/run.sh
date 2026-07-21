#!/usr/bin/env sh
# Build the callback-demo-php cdylib, load it as a PHP extension (via
# `php -d extension=<path>`), run the PHP consumer, and fail (nonzero) if the
# consumer fails. Touches nothing outside the cargo target. ext-php-rs's build
# script needs the PHP dev headers (`php-config` on PATH).
#
#   PROFILE   — debug (default) or release
set -eu

PROFILE="${PROFILE:-debug}"

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"

echo "== building callback-demo-php cdylib (profile: $PROFILE) =="
if [ "$PROFILE" = "release" ]; then
    ( cd "$ROOT" && cargo build -p callback-demo-php --release )
else
    ( cd "$ROOT" && cargo build -p callback-demo-php )
fi

LIBDIR="$ROOT/target/$PROFILE"
# The cdylib basename differs by platform (.so on Linux, .dylib on macOS).
if [ -f "$LIBDIR/libcallback_demo_php.so" ]; then
    SRC="$LIBDIR/libcallback_demo_php.so"
elif [ -f "$LIBDIR/libcallback_demo_php.dylib" ]; then
    SRC="$LIBDIR/libcallback_demo_php.dylib"
else
    echo "cdylib not found under $LIBDIR" >&2
    ls -l "$LIBDIR" >&2 || true
    exit 1
fi

echo "== running php consumer =="
# ext-php-rs' #[php_module] exports `get_module`; PHP loads the cdylib directly.
php -d extension="$SRC" "$HERE/consumer.php"

echo "== callback-demo-php round-trip OK =="
