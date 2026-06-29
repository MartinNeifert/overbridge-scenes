#!/usr/bin/env bash
# Download and install yabridge prebuilt binaries to ~/.local/share/yabridge
set -euo pipefail

VERSION="${YABRIDGE_VERSION:-5.1.1}"
DEST="${HOME}/.local/share"
INSTALL_DIR="${DEST}/yabridge"
ARCHIVE="yabridge-${VERSION}.tar.gz"
URL="https://github.com/robbert-vdh/yabridge/releases/download/${VERSION}/${ARCHIVE}"
CACHE="${HOME}/.cache/yabridge/${ARCHIVE}"

mkdir -p "${HOME}/.cache/yabridge" "$DEST"

if [[ -x "${INSTALL_DIR}/yabridgectl" ]]; then
  echo "yabridge already installed at ${INSTALL_DIR}/yabridgectl"
  "${INSTALL_DIR}/yabridgectl" --version 2>/dev/null || true
  exit 0
fi

echo "Downloading yabridge ${VERSION} ..."
if command -v curl >/dev/null; then
  curl -fL "$URL" -o "$CACHE"
elif command -v wget >/dev/null; then
  wget -qO "$CACHE" "$URL"
else
  echo "curl or wget required to download yabridge" >&2
  exit 1
fi

echo "Extracting to ${DEST} ..."
tar -C "$DEST" -xaf "$CACHE"
chmod +x "${INSTALL_DIR}/yabridgectl" "${INSTALL_DIR}/yabridge-host.exe" 2>/dev/null || true

if [[ ! -x "${INSTALL_DIR}/yabridgectl" ]]; then
  echo "Install failed — expected ${INSTALL_DIR}/yabridgectl" >&2
  exit 1
fi

echo "Installed: ${INSTALL_DIR}/yabridgectl"
"${INSTALL_DIR}/yabridgectl" --version 2>/dev/null || true
