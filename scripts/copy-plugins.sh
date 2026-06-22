#!/usr/bin/env bash
# Copy Elektron Overbridge VST3 plugins from system install into project.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="/Library/Audio/Plug-Ins/VST3/Elektron"
DEST="$ROOT/plugins"

if [[ ! -d "$SRC" ]]; then
  echo "Error: Elektron VST3 directory not found at $SRC"
  echo "Install Overbridge from https://www.elektron.se/support-downloads/overbridge"
  exit 1
fi

mkdir -p "$DEST"
echo "Copying VST3 plugins from $SRC to $DEST ..."
cp -R "$SRC/"* "$DEST/"
echo "Copied plugins:"
ls -1 "$DEST"
