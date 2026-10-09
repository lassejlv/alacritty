#!/usr/bin/env bash
set -euo pipefail
app=${1:?Pass the app bundle path}
root="$(cd "$(dirname "$0")/../.." && pwd)"
bash "$root/scripts/release/fetch-sparkle.sh"
framework="$app/Contents/Frameworks/Sparkle.framework"
mkdir -p "$app/Contents/Frameworks"
rm -rf "$framework"
ditto "$root/target/sparkle/2.10.0/Sparkle.framework" "$framework"
python3 "$root/scripts/release/macos-version.py" "$app"
# These helpers are nested code and must be signed before the framework and app.
for architecture in arm64 x86_64; do
    lipo "$framework/Versions/B/Sparkle" -verify_arch "$architecture"
done
