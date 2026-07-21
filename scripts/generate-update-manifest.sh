#!/usr/bin/env bash
set -euo pipefail

: "${UPDATE_VERSION:?UPDATE_VERSION is required}"

DIST_DIR="${DIST_DIR:-dist}"
OUTPUT="${OUTPUT:-${DIST_DIR}/latest.json}"
NOTES="${NOTES:-Automated DeviceHub Mask nightly update.}"
PUB_DATE="${PUB_DATE:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"

shopt -s nullglob
FRAGMENTS=("${DIST_DIR}"/update-fragment-*.json)

# Unsigned builds still publish native installers, but must not advertise an
# unverifiable in-app update payload.
if [[ "${#FRAGMENTS[@]}" -eq 0 ]]; then
  echo "No signed updater fragments found; latest.json will not be generated"
  exit 0
fi

jq -s \
  --arg version "$UPDATE_VERSION" \
  --arg notes "$NOTES" \
  --arg pub_date "$PUB_DATE" \
  '{
    version: $version,
    notes: $notes,
    pub_date: $pub_date,
    platforms: (map(.platforms) | add)
  }' "${FRAGMENTS[@]}" > "$OUTPUT"

test "$(jq '.platforms | length' "$OUTPUT")" -gt 0
echo "$OUTPUT"
