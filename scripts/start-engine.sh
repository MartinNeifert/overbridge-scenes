#!/usr/bin/env bash
# Start Overbridge Engine if not already running.
set -euo pipefail

ENGINE="/Applications/Elektron/Overbridge Engine.app"

if pgrep -f "Overbridge Engine" >/dev/null 2>&1; then
  echo "Overbridge Engine already running"
  exit 0
fi

if [[ ! -d "$ENGINE" ]]; then
  echo "Error: Overbridge Engine not found at $ENGINE"
  exit 1
fi

open -a "$ENGINE"
echo "Started Overbridge Engine — waiting for initialization..."
sleep 3

if pgrep -f "Overbridge Engine" >/dev/null 2>&1; then
  echo "Overbridge Engine is running"
else
  echo "Warning: Overbridge Engine may not have started"
fi
