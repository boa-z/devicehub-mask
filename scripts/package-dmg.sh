#!/usr/bin/env bash
set -euo pipefail

: "${APP_VERSION:?APP_VERSION is required}"
: "${BUILD_NUMBER:?BUILD_NUMBER is required}"

if [[ ! "$BUILD_NUMBER" =~ ^[0-9]+$ ]]; then
  echo "BUILD_NUMBER must be numeric" >&2
  exit 2
fi

APP_PATH="${APP_PATH:-src-tauri/target/release/bundle/macos/DeviceHub Mask.app}"
ARTIFACT="${ARTIFACT:-devicehub-mask_${APP_VERSION}+${BUILD_NUMBER}.dmg}"
BUNDLE_VERSION="${BUNDLE_VERSION:-$APP_VERSION}"
DIST_DIR="${DIST_DIR:-dist}"
STAGING_DIR="${DIST_DIR}/dmg"
STAGED_APP="${STAGING_DIR}/DeviceHub Mask.app"

test -d "$APP_PATH"
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"
ditto "$APP_PATH" "$STAGED_APP"
ln -s /Applications "${STAGING_DIR}/Applications"

PLIST_PATH="${STAGED_APP}/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${BUNDLE_VERSION}" "$PLIST_PATH"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${BUILD_NUMBER}" "$PLIST_PATH"

# Stamping Info.plist invalidates Tauri's bundle signature. Re-sign the complete
# staged app and verify that its executable and sealed resources agree.
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}"
if [[ "$CODESIGN_IDENTITY" == "-" ]]; then
  codesign --force --deep --sign - "$STAGED_APP"
else
  codesign --force --deep --options runtime --sign "$CODESIGN_IDENTITY" "$STAGED_APP"
fi
codesign --verify --deep --strict --verbose=2 "$STAGED_APP"

hdiutil create \
  -volname "DeviceHub Mask ${APP_VERSION}+${BUILD_NUMBER}" \
  -srcfolder "$STAGING_DIR" \
  -ov \
  -format UDZO \
  "${DIST_DIR}/${ARTIFACT}"
shasum -a 256 "${DIST_DIR}/${ARTIFACT}" > "${DIST_DIR}/${ARTIFACT}.sha256"

echo "${DIST_DIR}/${ARTIFACT}"
