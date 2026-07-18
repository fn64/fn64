#!/usr/bin/env bash
# Fetch a prebuilt n64-systemtest.z64 from lemmy-64/n64-systemtest's own CI
# ("Build ROM" GitHub Actions workflow) and verify its hash.
#
# n64-systemtest has no GitHub Releases as of 2026-07-18; its CI artifacts
# are the only prebuilt source. This script pulls the artifact from the most
# recent successful run of .github/workflows/build-rom.yml on `main` and
# checks it against a known-good sha256 (recorded the day this script was
# written -- update EXPECTED_SHA256 and EXPECTED_COMMIT if you deliberately
# want a newer build).
#
# Requires: gh (authenticated), unzip, shasum.
# Output: never written into the git tree -- pass an out-of-tree directory.
#
# Usage: tools/ares/fetch-systemtest.sh /path/to/scratch/dir

set -euo pipefail

REPO="lemmy-64/n64-systemtest"
EXPECTED_COMMIT="f2db2b92da9ddf281848f17c87b84c4aeea07c2f"
EXPECTED_SHA256="08a82f082fb50bb5e1256e9fec83383a878458801a8ff8dac78a548d9eeb14d1"

OUT_DIR="${1:?usage: fetch-systemtest.sh <out-of-tree-scratch-dir>}"
mkdir -p "$OUT_DIR"

echo "Looking up latest n64-systemtest-z64 artifact from $REPO CI..." >&2
ARTIFACT_JSON=$(gh api "repos/$REPO/actions/artifacts" --jq \
  '.artifacts | map(select(.name == "n64-systemtest-z64" and .expired == false)) | sort_by(.created_at) | last')

if [ -z "$ARTIFACT_JSON" ] || [ "$ARTIFACT_JSON" = "null" ]; then
  echo "error: no unexpired n64-systemtest-z64 CI artifact found. n64-systemtest" >&2
  echo "publishes no GitHub Releases -- if CI artifacts have all expired, you" >&2
  echo "must build from source (see README.md 'Build from source' section)." >&2
  exit 1
fi

ARTIFACT_ID=$(echo "$ARTIFACT_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
ZIP_PATH="$OUT_DIR/n64-systemtest-artifact.zip"

echo "Downloading artifact id=$ARTIFACT_ID ..." >&2
gh api "repos/$REPO/actions/artifacts/$ARTIFACT_ID/zip" > "$ZIP_PATH"

echo "Extracting ..." >&2
unzip -o -q "$ZIP_PATH" -d "$OUT_DIR"

ROM_PATH="$OUT_DIR/n64-systemtest.z64"
if [ ! -f "$ROM_PATH" ]; then
  echo "error: expected n64-systemtest.z64 not found after extraction" >&2
  exit 1
fi

ACTUAL_SHA256=$(shasum -a 256 "$ROM_PATH" | awk '{print $1}')
echo "sha256: $ACTUAL_SHA256"

if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
  echo "" >&2
  echo "WARNING: hash does not match the recorded known-good build." >&2
  echo "  expected (commit $EXPECTED_COMMIT): $EXPECTED_SHA256" >&2
  echo "  got:                                 $ACTUAL_SHA256" >&2
  echo "This likely means main has moved since this script was written." >&2
  echo "That is not necessarily bad -- just re-verify provenance (README.md)" >&2
  echo "and update EXPECTED_SHA256/EXPECTED_COMMIT once you've done so." >&2
  exit 2
fi

echo "OK: matches known-good build from commit $EXPECTED_COMMIT" >&2
echo "$ROM_PATH"
