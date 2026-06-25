#!/usr/bin/env bash
# Build the OB Scenes Remote VST3 bundle (macOS target recommended for Ableton).
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> cargo build -p ob-remote-vst --release"
cargo build -p ob-remote-vst --release

OUT_DIR="target/bundled"
PLUGIN_NAME="OB Scenes Remote"
BUNDLE_NAME="OB Scenes Remote.vst3"
LIB_NAME="libob_remote_vst.so"

case "$(uname -s)" in
  Darwin)
    LIB_NAME="libob_remote_vst.dylib"
    ;;
  Linux)
    LIB_NAME="libob_remote_vst.so"
    ;;
  *)
    echo "Unsupported platform for VST3 bundling: $(uname -s)" >&2
    exit 1
    ;;
esac

mkdir -p "${OUT_DIR}/${BUNDLE_NAME}/Contents/x86_64-linux-gnu" \
         "${OUT_DIR}/${BUNDLE_NAME}/Contents/MacOS" 2>/dev/null || true

if [[ "$(uname -s)" == "Darwin" ]]; then
  DEST="${OUT_DIR}/${BUNDLE_NAME}/Contents/MacOS/${PLUGIN_NAME}"
else
  DEST="${OUT_DIR}/${BUNDLE_NAME}/Contents/x86_64-linux-gnu/${PLUGIN_NAME}"
  mkdir -p "$(dirname "$DEST")"
fi

cp "target/release/${LIB_NAME}" "$DEST"

cat > "${OUT_DIR}/${BUNDLE_NAME}/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>English</string>
  <key>CFBundleExecutable</key>
  <string>OB Scenes Remote</string>
  <key>CFBundleIdentifier</key>
  <string>com.overbridge.scenes.remote</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>OB Scenes Remote</string>
  <key>CFBundlePackageType</key>
  <string>BNDL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleSignature</key>
  <string>????</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
</dict>
</plist>
PLIST

echo "Built: ${OUT_DIR}/${BUNDLE_NAME}"
echo "Install: cp -R \"${OUT_DIR}/${BUNDLE_NAME}\" ~/Library/Audio/Plug-Ins/VST3/"
