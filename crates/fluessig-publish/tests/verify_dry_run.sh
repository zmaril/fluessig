#!/usr/bin/env bash
# Dry-run verification for fluessig-publish against toy fixtures.
#
# Builds the binary, creates 4 minimal standalone toy packages (a crate, an npm
# package, a pyproject project, and a gem), and runs `fluessig publish` in
# DRY-RUN mode (no --confirm) against each. Nothing is ever published.
set -u

# Resolve the fluessig repo root (this script lives in crates/fluessig-publish/tests).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

echo "### building the binary (cargo build --release -p fluessig-publish)"
( cd "$REPO_ROOT" && cargo build --release -p fluessig-publish ) || {
    echo "BUILD FAILED"; exit 1;
}
BIN="$REPO_ROOT/target/release/fluessig"
echo "binary: $BIN"
echo

WORK="$(mktemp -d)"
echo "fixtures under: $WORK"
echo

# ---------------------------------------------------------------------------
# 1. crates.io fixture
# ---------------------------------------------------------------------------
CRATE="$WORK/toycrate"
cargo new --lib "$CRATE" --name toycrate >/dev/null 2>&1
# `cargo new` inside a workspace-less temp dir makes a standalone crate.

# ---------------------------------------------------------------------------
# 2. npm fixture
# ---------------------------------------------------------------------------
NPM="$WORK/toynpm"
mkdir -p "$NPM"
cat > "$NPM/package.json" <<'JSON'
{
  "name": "toynpm-fluessig-demo",
  "version": "0.0.0",
  "description": "toy package for fluessig-publish dry-run verification",
  "license": "MIT",
  "main": "index.js"
}
JSON
cat > "$NPM/index.js" <<'JS'
module.exports = () => "hello from toynpm";
JS

# ---------------------------------------------------------------------------
# 3. pypi fixture (hatchling). The src dir name must match the normalized
#    project name so the wheel builds.
# ---------------------------------------------------------------------------
PYPI="$WORK/toypypi"
mkdir -p "$PYPI/src/toypypi_fluessig_demo"
cat > "$PYPI/pyproject.toml" <<'TOML'
[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[project]
name = "toypypi-fluessig-demo"
version = "0.0.0"
description = "toy package for fluessig-publish dry-run verification"
requires-python = ">=3.8"
TOML
cat > "$PYPI/src/toypypi_fluessig_demo/__init__.py" <<'PY'
def hello() -> str:
    return "hello from toypypi"
PY

# ---------------------------------------------------------------------------
# 4. gems fixture
# ---------------------------------------------------------------------------
GEMS="$WORK/toygem"
mkdir -p "$GEMS/lib"
cat > "$GEMS/toygem.gemspec" <<'RUBY'
Gem::Specification.new do |spec|
  spec.name = "toygem_fluessig_demo"
  spec.version = "0.0.0"
  spec.summary = "toy gem for fluessig-publish dry-run verification"
  spec.authors = ["fluessig"]
  spec.files = ["lib/toygem.rb"]
  spec.license = "MIT"
end
RUBY
cat > "$GEMS/lib/toygem.rb" <<'RUBY'
module Toygem
  def self.hello = "hello from toygem"
end
RUBY

run_case() {
    local title="$1"; shift
    echo "==================================================================="
    echo "### $title"
    echo "### \$ $BIN $*"
    echo "-------------------------------------------------------------------"
    "$BIN" "$@"
    local rc=$?
    echo "-------------------------------------------------------------------"
    echo "### exit code: $rc"
    echo
    return $rc
}

overall=0

run_case "crates.io dry-run" publish --to crates --path "$CRATE" --version 1.2.3 --package toycrate || overall=1
run_case "npm dry-run"       publish --to npm    --path "$NPM"   --version 1.2.3 --package toynpm  || overall=1
run_case "pypi dry-run"      publish --to pypi   --path "$PYPI"  --version 1.2.3 --package toypypi || overall=1
# gems has NO registry dry-run: it builds the .gem and prints the honest message.
# It returns exit 0 (validation succeeded), staying a stub.
run_case "gems dry-run (no real dry-run exists)" publish --to gems --path "$GEMS" --version 1.2.3 --package toygem || overall=1

echo "==================================================================="
if [ "$overall" -eq 0 ]; then
    echo "ALL FOUR DRY-RUNS BEHAVED CORRECTLY."
else
    echo "SOME CASES FAILED — see above."
fi
rm -rf "$WORK"
exit "$overall"
