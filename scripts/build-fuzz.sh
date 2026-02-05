#!/usr/bin/env bash
# Build forge with the `fuzz` profile and SanitizerCoverage instrumentation.
# This produces a binary that collects coverage from Tempo precompile Rust
# code during invariant/fuzz tests.
#
# Usage:
#   ./scripts/build-fuzz.sh
#
# Then run tests with:
#   /path/to/tempo-foundry/target/<target-triple>/fuzz/forge test --mt invariant
#
# In your project's foundry.toml, enable:
#   [invariant]
#   tempo_precompile_coverage = true
#   corpus_dir = "corpus/invariant"

set -euo pipefail

TARGET=$(rustc -vV | awk '/^host:/ { print $2 }')

# By explicitly passing --target, Cargo separates "target" from "host" builds.
# The [target.<triple>] rustflags then apply ONLY to the target compilation
# (the forge binary and its deps), NOT to build scripts or proc macros.
cargo build \
  --profile fuzz \
  --bin forge \
  --target "$TARGET" \
  --config "target.${TARGET}.rustflags=['-Cpasses=sancov-module', '-Cllvm-args=-sanitizer-coverage-level=3', '-Cllvm-args=-sanitizer-coverage-trace-pc-guard']" \
  "$@"

echo ""
echo "Built: target/${TARGET}/fuzz/forge"
