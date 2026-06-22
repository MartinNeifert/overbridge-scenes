#!/usr/bin/env bash
# Full setup: copy plugins, build host, verify install.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "=== Overbridge Host Setup ==="

bash scripts/copy-plugins.sh

if [[ -d "/Applications/Elektron/Overbridge Engine.app" ]]; then
  mkdir -p vendor
  if [[ ! -d "vendor/Overbridge Engine.app" ]]; then
    echo "Copying Overbridge Engine reference to vendor/ ..."
    cp -R "/Applications/Elektron/Overbridge Engine.app" vendor/
  fi
fi

echo ""
echo "Building ob-host ..."
export PATH="/opt/homebrew/bin:$PATH"
export CARGO_TARGET_DIR="$ROOT/target"
cargo build --release

echo ""
echo "Setup complete. Run with:"
echo "  cd $ROOT"
echo "  ./scripts/start-engine.sh"
echo "  ./target/release/ob-host --plugin Digitakt"
echo ""
echo "Control surface: http://127.0.0.1:7780/"
