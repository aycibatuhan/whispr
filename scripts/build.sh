#!/bin/bash
# Build whispr.app (debug) with optional stable code signing.
#
# Stable signing keeps macOS permission grants (mic, accessibility, input
# monitoring) across rebuilds. The script auto-detects an Apple Development
# certificate from the keychain; if none is present it falls back to adhoc
# signing (works, but macOS re-prompts for permissions on every rebuild).
#
# Usage:  scripts/build.sh
set -euo pipefail

[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
export CMAKE_POLICY_VERSION_MINIMUM=3.5

IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null | grep "Apple Development" | grep -oE '[0-9A-F]{40}' | head -1 || true)
if [ -n "${IDENTITY:-}" ]; then
  echo "Signing with identity: $IDENTITY"
  export APPLE_SIGNING_IDENTITY="$IDENTITY"
else
  echo "No Apple Development certificate found — building with adhoc signing."
  echo "Permission grants (mic/accessibility) will reset on every rebuild."
fi

cd "$(dirname "$0")/.."
npx tauri build --debug --bundles app
