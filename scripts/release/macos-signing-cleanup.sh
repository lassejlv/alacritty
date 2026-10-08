#!/usr/bin/env bash
set -euo pipefail
[[ -n "${APPLE_SIGNING_DIR:-}" ]] || exit 0
# Only remove the temporary directory created by the setup script.
[[ "$APPLE_SIGNING_DIR" == "$RUNNER_TEMP"/apple-signing.* ]] || exit 1
status=0
if [[ -f "$APPLE_SIGNING_DIR/original-keychains.txt" ]]; then
    keychains=()
    while IFS= read -r existing; do
        existing=${existing#*\"}
        existing=${existing%\"*}
        [[ -z "$existing" ]] || keychains+=("$existing")
    done < "$APPLE_SIGNING_DIR/original-keychains.txt"
    if (( ${#keychains[@]} )); then
        security list-keychains -d user -s "${keychains[@]}" || status=1
    fi
fi
if [[ -f "$APPLE_SIGNING_DIR/signing.keychain-db" ]]; then
    security delete-keychain "$APPLE_SIGNING_DIR/signing.keychain-db" || status=1
fi
rm -rf "$APPLE_SIGNING_DIR"
echo 'Temporary signing credentials removed.'
exit "$status"
