#!/usr/bin/env bash
set -euo pipefail
: "${SPARKLE_PRIVATE_KEY:?Missing Sparkle signing key}" "${GITHUB_REPOSITORY:?}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
tools="$root/target/sparkle/2.10.0/bin"
python3 "$root/scripts/release/verify-sparkle-key.py" target/release/osx/Alacritty.app
work=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/alacritty-appcast.XXXXXX")
trap 'rm -rf "$work"' EXIT
# Use a tag-specific URL, never /latest for the archive: the signed feed must
# continue to describe exactly the same bytes when a newer release is published.
tag=${RELEASE_TAG:-validation}
cp dist/Alacritty-macos-universal.zip "$work/"
printf '%s' "$SPARKLE_PRIVATE_KEY" | "$tools/generate_appcast" \
    --ed-key-file - --maximum-deltas 0 \
    --download-url-prefix "https://github.com/$GITHUB_REPOSITORY/releases/download/$tag/" \
    --link "https://github.com/$GITHUB_REPOSITORY" "$work"
test -s "$work/appcast.xml"
printf '%s' "$SPARKLE_PRIVATE_KEY" | "$tools/sign_update" \
    --ed-key-file - --verify "$work/appcast.xml"
cp "$work/appcast.xml" dist/appcast.xml
(cd dist && shasum -a 256 Alacritty-macos-universal.dmg Alacritty-macos-universal.zip appcast.xml > SHA256SUMS-macos.txt)
