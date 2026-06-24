#!/usr/bin/env bash
# Full test suite for local development (Rust + web morph math).
set -euo pipefail

cd "$(dirname "$0")/.."

export RUST_TEST_THREADS=1

echo "==> cargo test"
cargo test -- --test-threads=1 "$@"

echo "==> node --test web/scenes-morph"
node --test web/scenes-morph.test.mjs
