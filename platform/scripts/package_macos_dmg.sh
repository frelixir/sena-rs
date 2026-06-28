#!/usr/bin/env bash
set -euo pipefail

# package_macos_dmg.sh
#
# Creates a DMG from dist/macos/Sena.app using hdiutil.


ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_PATH="${ROOT_DIR}/dist/macos/Sena.app"
OUT_DIR="${ROOT_DIR}/dist/macos"
DMG_NAME="${DMG_NAME:-Sena}"
DMG_PATH="${OUT_DIR}/${DMG_NAME}.dmg"
STAGING="${OUT_DIR}/_dmg_staging"

[[ -d "${APP_PATH}" ]] || { echo "ERROR: Missing ${APP_PATH}. Run package_macos_app.sh first."; exit 1; }

rm -rf "${STAGING}"
mkdir -p "${STAGING}"

# Put app at DMG root
cp -R "${APP_PATH}" "${STAGING}/Sena.app"

# Add /Applications link at DMG root (drag-to-install target)
ln -sf /Applications "${STAGING}/Applications"

rm -f "${DMG_PATH}"
hdiutil create -volname "${DMG_NAME}" -srcfolder "${STAGING}" -ov -format UDZO "${DMG_PATH}"

rm -rf "${STAGING}"
echo "[macos] OK: ${DMG_PATH}"
