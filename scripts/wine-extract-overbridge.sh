#!/usr/bin/env bash
# Extract Windows Overbridge VST3 bundles from the MSI without running the installer.
# Use when msiexec fails under Wine (e.g. CheckForDevicesAction / USB checks).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MSI="${OB_OVERBRIDGE_MSI:-$HOME/Downloads/Elektron_Overbridge_2.25.7/Elektron Overbridge Installer 64bit 2.25.7 .msi}"
WINEPREFIX="${WINEPREFIX:-$HOME/.wine-overbridge}"
VST3_DEST="$WINEPREFIX/drive_c/Program Files/Common Files/VST3"
EXTRACT="${TMPDIR:-/tmp}/overbridge-msi-extract-$$"

if [[ ! -f "$MSI" ]]; then
  echo "MSI not found: $MSI" >&2
  exit 1
fi
command -v 7z >/dev/null || { echo "7z not found — install p7zip-full" >&2; exit 1; }

cleanup() { rm -rf "$EXTRACT"; }
trap cleanup EXIT

echo "Extracting $MSI ..."
mkdir -p "$EXTRACT"
7z x -o"$EXTRACT" "$MSI" -y >/dev/null
for cab in "$EXTRACT"/cab*.cab; do
  [[ -f "$cab" ]] || continue
  dir="$EXTRACT/$(basename "$cab" .cab)"
  mkdir -p "$dir"
  7z x -o"$dir" "$cab" -y >/dev/null
done

# cab_file:source_name:bundle_name
PLUGINS=(
  "cab1:DigitaktVst3_64bit:Digitakt.vst3"
  "cab1:DigitaktIIVst3_64bit:Digitakt II.vst3"
  "cab1:DigitoneIIVst3_64bit:Digitone II.vst3"
  "cab1:AnalogRytmVst3_64bit:Analog Rytm MKII.vst3"
  "cab1:AnalogKeysVst3_64bit:Analog Keys MKII.vst3"
  "cab1:AnalogHeatVst3_64bit:Analog Heat MKII.vst3"
  "cab1:AnalogHeatFXVst3_64bit:Analog Heat +FX.vst3"
  "cab1:AnalogFourVst3_64bit:Analog Four MKII.vst3"
  "cab2:DigitoneVst3_64bit:Digitone.vst3"
  "cab3:SyntaktVst3_64bit:Syntakt.vst3"
)

deploy_vst3() {
  local cab_dir="$1" src_name="$2" bundle="$3"
  local src="$EXTRACT/$cab_dir/$src_name"
  local stem="${bundle%.vst3}"
  local dest="$VST3_DEST/$bundle/Contents/x86_64-win"
  if [[ ! -f "$src" ]]; then
    echo "  skip $bundle (not in MSI)"
    return 0
  fi
  mkdir -p "$dest"
  cp -f "$src" "$dest/$stem.vst3"
  echo "  $bundle"
}

mkdir -p "$VST3_DEST"
echo "Deploying VST3 bundles to:"
echo "  $VST3_DEST"
count=0
for entry in "${PLUGINS[@]}"; do
  IFS=: read -r cab src bundle <<<"$entry"
  if deploy_vst3 "$cab" "$src" "$bundle"; then
    count=$((count + 1))
  fi
done

# Optional: Engine binary for manual testing (won't run USB drivers under Wine).
ENGINE_SRC="$EXTRACT/cab3/OverbridgeEngineEXE"
if [[ -f "$ENGINE_SRC" ]]; then
  ENGINE_DEST="$WINEPREFIX/drive_c/Program Files/Elektron/Overbridge Engine"
  mkdir -p "$ENGINE_DEST"
  cp -f "$ENGINE_SRC" "$ENGINE_DEST/Overbridge Engine.exe"
  echo "  Overbridge Engine.exe (USB drivers not installed — lab use only)"
fi

echo ""
echo "Extracted $count VST3 bundle(s)."
echo "Next: yabridgectl add \"\$WINEPREFIX/drive_c/Program Files/Common Files/VST3/Digitakt.vst3\""
echo "      mise run yabridge:sync"
