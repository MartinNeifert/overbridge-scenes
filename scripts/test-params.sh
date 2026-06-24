#!/usr/bin/env bash
# Run the full test suite (same as `cargo test` in this repo).
set -euo pipefail

cd "$(dirname "$0")/.."
cargo test "$@"

if [[ "${1:-}" == "--live" ]]; then
  PORT="${OB_PORT:-7781}"
  echo "==> live HTTP smoke on port ${PORT}"
  RUST_LOG=warn cargo run --release -- \
    --fake-plugin \
    --control-only \
    --no-engine \
    --port "${PORT}" &
  HOST_PID=$!
  trap 'kill "${HOST_PID}" 2>/dev/null || true' EXIT

  BASE="http://127.0.0.1:${PORT}"
  for _ in $(seq 1 50); do
    curl -sf "${BASE}/api/status" >/dev/null && break
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
