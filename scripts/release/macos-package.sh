#!/usr/bin/env bash
set -euo pipefail
for name in APPLE_SIGN_IDENTITY APPLE_SIGNING_KEYCHAIN APPLE_TEAM_ID APPLE_NOTARY_KEY APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID RUNNER_TEMP; do
    [[ -n "${!name:-}" ]] || { echo "Missing $name." >&2; exit 1; }
done
app=target/release/osx/Alacritty.app
binary="$app/Contents/MacOS/alacritty"
[[ -x "$binary" ]] || { echo 'Build the universal app first.' >&2; exit 1; }
for architecture in arm64 x86_64; do
    lipo "$binary" -verify_arch "$architecture"
done
work=$(mktemp -d "$RUNNER_TEMP/alacritty-package.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir -p dist
sign=(--force --sign "$APPLE_SIGN_IDENTITY" --keychain "$APPLE_SIGNING_KEYCHAIN" --timestamp)
# Alacritty contains one Mach-O executable and no bundled runtime/frameworks.
codesign "${sign[@]}" --options runtime "$binary"
codesign "${sign[@]}" --options runtime "$app"
codesign --verify --deep --strict --verbose=2 "$app"
metadata=$(codesign --display --verbose=4 "$app" 2>&1)
printf '%s\n' "$metadata"
printf '%s\n' "$metadata" | grep -F "TeamIdentifier=$APPLE_TEAM_ID"
printf '%s\n' "$metadata" | grep -E 'flags=.*runtime'
printf '%s\n' "$metadata" | grep '^Timestamp='

notarize() {
    local file=$1 result=$2 id status
    # Submit once, then wait on that exact submission (never resubmit on timeout).
    xcrun notarytool submit "$file" --key "$APPLE_NOTARY_KEY" \
        --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID" \
        --output-format json > "$result"
    id=$(jq -er '.id' "$result")
    echo "Notarization submission: $id"
    if ! xcrun notarytool wait "$id" --key "$APPLE_NOTARY_KEY" \
        --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID" \
        --timeout 30m --output-format json > "$result"; then
        xcrun notarytool info "$id" --key "$APPLE_NOTARY_KEY" \
            --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID" \
            --output-format json > "$result"
    fi
    status=$(jq -er '.status' "$result")
    if [[ "$status" != Accepted ]]; then
        echo "Notarization $id: $status" >&2
        if [[ "$status" == Invalid || "$status" == Rejected ]]; then
            xcrun notarytool log "$id" --key "$APPLE_NOTARY_KEY" \
                --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID"
        fi
        return 1
    fi
}

ditto -c -k --keepParent "$app" "$work/notarize.zip"
notarize "$work/notarize.zip" "$work/app-result.json"
xcrun stapler staple "$app"
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=2 "$app"
# Package only the stapled app, then notarize and staple the DMG separately.
mkdir "$work/dmg"
ditto "$app" "$work/dmg/Alacritty.app"
ln -s /Applications "$work/dmg/Applications"
dmg=dist/Alacritty-macos-universal.dmg
hdiutil create -volname Alacritty -fs HFS+ -srcfolder "$work/dmg" -ov -format UDZO "$dmg"
codesign "${sign[@]}" "$dmg"
notarize "$dmg" "$work/dmg-result.json"
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
codesign --verify --strict --verbose=2 "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
ditto -c -k --keepParent "$app" dist/Alacritty-macos-universal.zip
(cd dist && shasum -a 256 Alacritty-macos-universal.dmg Alacritty-macos-universal.zip > SHA256SUMS-macos.txt)
