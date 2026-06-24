#!/usr/bin/env bash
# Headless parameter e2e against the in-process fake plugin.
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> cargo test (in-process fake plugin e2e)"
# Global VST notifiers are process-wide; run serially to avoid cross-test races.
cargo test --test e2e_params -- --test-threads=1

if [[ "${1:-}" == "--live" ]]; then
  PORT="${OB_PORT:-7781}"
  echo "==> live HTTP harness on port ${PORT}"
  RUST_LOG=warn cargo run --release -- \
    --fake-plugin \
    --control-only \
    --no-engine \
    --port "${PORT}" &
  HOST_PID=$!
  trap 'kill "${HOST_PID}" 2>/dev/null || true' EXIT

  BASE="http://127.0.0.1:${PORT}"
  for _ in $(seq 1 50); do
    if curl -sf "${BASE}/api/status" >/dev/null; then
      break
    fi
    sleep 0.1
  done

  COUNT=$(curl -sf "${BASE}/api/status" | jq -r .parameter_count)
  [[ "${COUNT}" == "9" ]] || { echo "expected 9 parameters, got ${COUNT}"; exit 1; }

  curl -sf -X POST "${BASE}/api/parameters/0" \
    -H 'Content-Type: application/json' \
    -d '{"value":0.75}' >/dev/null

  VAL=$(curl -sf "${BASE}/api/parameters/0" | jq -r .value)
  awk "BEGIN { exit !(${VAL} > 0.74 && ${VAL} < 0.76) }"

  echo "OK (live)"
fi
