#!/usr/bin/env bash
set -euo pipefail
[[ -n "${GITHUB_ENV:-}" && -n "${RUNNER_TEMP:-}" ]] || { echo 'GitHub Actions is required.' >&2; exit 1; }
[[ "${APPLE_TEAM_ID:-}" =~ ^[A-Z0-9]{10}$ ]] || { echo 'Set APPLE_TEAM_ID.' >&2; exit 1; }
for name in APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_NOTARY_KEY_P8_BASE64 APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID; do
    [[ -n "${!name:-}" ]] || { echo "Missing $name." >&2; exit 1; }
done
umask 077
signing_dir=$(mktemp -d "$RUNNER_TEMP/apple-signing.XXXXXX")
# Register cleanup before decoding or importing anything.
printf 'APPLE_SIGNING_DIR=%s\n' "$signing_dir" >> "$GITHUB_ENV"
keychain="$signing_dir/signing.keychain-db"
security list-keychains -d user > "$signing_dir/original-keychains.txt"
printf '%s' "$APPLE_CERTIFICATE_P12_BASE64" | base64 -D > "$signing_dir/developer-id.p12"
printf '%s' "$APPLE_NOTARY_KEY_P8_BASE64" | base64 -D > "$signing_dir/notary-key.p8"
/usr/bin/openssl pkey -in "$signing_dir/notary-key.p8" -noout >/dev/null
password=$(/usr/bin/openssl rand -hex 32)
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$signing_dir/developer-id.p12" -k "$keychain" \
    -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" >/dev/null
keychains=("$keychain")
while IFS= read -r existing; do
    existing=${existing#*\"}
    existing=${existing%\"*}
    [[ -z "$existing" ]] || keychains+=("$existing")
done < "$signing_dir/original-keychains.txt"
security list-keychains -d user -s "${keychains[@]}"
identity=$(security find-identity -v -p codesigning "$keychain" |
    awk -v team="($APPLE_TEAM_ID)" '$0 ~ /Developer ID Application:/ && index($0, team) {print $2}')
[[ "$identity" =~ ^[A-Fa-f0-9]{40}$ ]] || { echo 'Expected exactly one matching Developer ID Application identity.' >&2; exit 1; }
printf 'APPLE_SIGN_IDENTITY=%s\nAPPLE_SIGNING_KEYCHAIN=%s\nAPPLE_NOTARY_KEY=%s\n' \
    "$identity" "$keychain" "$signing_dir/notary-key.p8" >> "$GITHUB_ENV"
printf 'APPLE_TEAM_ID=%s\nAPPLE_NOTARY_KEY_ID=%s\nAPPLE_NOTARY_ISSUER_ID=%s\n' \
    "$APPLE_TEAM_ID" "$APPLE_NOTARY_KEY_ID" "$APPLE_NOTARY_ISSUER_ID" >> "$GITHUB_ENV"
echo "Developer ID signing is ready for team $APPLE_TEAM_ID."
