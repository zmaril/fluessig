#!/usr/bin/env sh
# Build the cpp-demo cdylib, compile the C and C++ consumers against the
# COMMITTED headers, link the cdylib, run both, and fail (nonzero) if either
# consumer fails. Works under both gcc/g++ and clang/clang++ (the CI matrix sets
# CC/CXX per leg). Touches nothing outside a scratch temp dir + the cargo target.
#
#   CC / CXX  — the C / C++ compilers (default: cc / c++)
#   PROFILE   — debug (default) or release
set -eu

CC="${CC:-cc}"
CXX="${CXX:-c++}"
PROFILE="${PROFILE:-debug}"

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "== building cpp-demo cdylib (profile: $PROFILE, CC=$CC CXX=$CXX) =="
if [ "$PROFILE" = "release" ]; then
    ( cd "$ROOT" && cargo build -p cpp-demo --release )
else
    ( cd "$ROOT" && cargo build -p cpp-demo )
fi

LIBDIR="$ROOT/target/$PROFILE"
# The cdylib basename differs by platform (.so on Linux, .dylib on macOS).
if [ -f "$LIBDIR/libcpp_demo.so" ]; then
    :
elif [ -f "$LIBDIR/libcpp_demo.dylib" ]; then
    :
else
    echo "cdylib not found under $LIBDIR" >&2
    ls -l "$LIBDIR" >&2 || true
    exit 1
fi

echo "== compiling C consumer with $CC =="
"$CC" -std=c11 -Wall -Wextra -I "$HERE" "$HERE/consumer.c" \
    -L "$LIBDIR" -lcpp_demo -Wl,-rpath,"$LIBDIR" -o "$WORK/consumer_c"

echo "== compiling C++ consumer with $CXX =="
"$CXX" -std=c++17 -Wall -Wextra -I "$HERE" "$HERE/consumer.cpp" \
    -L "$LIBDIR" -lcpp_demo -Wl,-rpath,"$LIBDIR" -o "$WORK/consumer_cpp"

echo "== running C consumer =="
LD_LIBRARY_PATH="$LIBDIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$WORK/consumer_c"

echo "== running C++ consumer =="
LD_LIBRARY_PATH="$LIBDIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$WORK/consumer_cpp"

echo "== cpp-demo round-trip OK ($CC / $CXX) =="
