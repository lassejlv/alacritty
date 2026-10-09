#!/usr/bin/env bash
set -euo pipefail
# Pin the upstream release and verify it before executing any bundled tools.
version=2.10.0
sha256=c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c
root="$(cd "$(dirname "$0")/../.." && pwd)"
destination="$root/target/sparkle/$version"
if [[ -f "$destination/.verified-$sha256" ]]; then
    exit 0
fi
mkdir -p "$root/target/sparkle"
work=$(mktemp -d "$root/target/sparkle/download.XXXXXX")
trap 'rm -rf "$work"' EXIT
curl --fail --location --retry 3 --proto '=https' --tlsv1.2 \
    "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz" \
    -o "$work/Sparkle.tar.xz"
printf '%s  %s\n' "$sha256" "$work/Sparkle.tar.xz" | shasum -a 256 --check
mkdir "$work/unpacked"
tar -xJf "$work/Sparkle.tar.xz" -C "$work/unpacked"
test -d "$work/unpacked/Sparkle.framework"
test -x "$work/unpacked/bin/generate_appcast"
rm -rf "$destination"
mv "$work/unpacked" "$destination"
touch "$destination/.verified-$sha256"
