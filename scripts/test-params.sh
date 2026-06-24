#!/usr/bin/env bash
# Back-compat wrapper: full suite by default, optional live smoke with --live.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ "${1:-}" == "--live" ]]; then
  shift
  cargo build --bin ob-host
  TARGET_DIR="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])')"
  BIN="${TARGET_DIR}/debug/ob-host"
  PORT="${OB_TEST_PORT:-3848}"
  export OB_FAKE_PLUGIN=1
  "$BIN" --fake-plugin --port "$PORT" &
  PID=$!
  cleanup() { kill "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true; }
  trap cleanup EXIT
  for _ in $(seq 1 50); do
    if curl -sf "http://127.0.0.1:${PORT}/api/status" >/dev/null 2>&1; then
      break
    fi
    sleep 0.1
  done
  curl -sf "http://127.0.0.1:${PORT}/api/status" | grep -q 'OB Test Host'
  curl -sf -X POST "http://127.0.0.1:${PORT}/api/parameters/0" \
    -H 'Content-Type: application/json' -d '{"value":0.5}' | grep -q '"index":0'
  echo "live smoke ok (port ${PORT})"
else
  exec "${ROOT}/scripts/test.sh" "$@"
fi
