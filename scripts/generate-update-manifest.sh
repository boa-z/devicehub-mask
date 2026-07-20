#!/usr/bin/env bash
set -euo pipefail

: "${UPDATE_VERSION:?UPDATE_VERSION is required}"
: "${UPDATE_ARCHIVE:?UPDATE_ARCHIVE is required}"
: "${RELEASE_BASE_URL:?RELEASE_BASE_URL is required}"

SIGNATURE_FILE="${SIGNATURE_FILE:-${UPDATE_ARCHIVE}.sig}"
OUTPUT="${OUTPUT:-dist/latest.json}"
NOTES="${NOTES:-Automated DeviceHub Mask nightly update.}"

test -f "$UPDATE_ARCHIVE"
test -f "$SIGNATURE_FILE"
mkdir -p "$(dirname "$OUTPUT")"

ARCHIVE_NAME="$(basename "$UPDATE_ARCHIVE")"
SIGNATURE="$(tr -d '\r\n' < "$SIGNATURE_FILE")"
DOWNLOAD_URL="${RELEASE_BASE_URL}/${ARCHIVE_NAME}"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

jq -n \
  --arg version "$UPDATE_VERSION" \
  --arg notes "$NOTES" \
  --arg pub_date "$PUB_DATE" \
  --arg signature "$SIGNATURE" \
  --arg url "$DOWNLOAD_URL" \
  '{
    version: $version,
    notes: $notes,
    pub_date: $pub_date,
    platforms: {
      "darwin-aarch64": { signature: $signature, url: $url },
      "darwin-x86_64": { signature: $signature, url: $url }
    }
  }' > "$OUTPUT"

echo "$OUTPUT"
