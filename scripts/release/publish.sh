#!/usr/bin/env bash
set -euo pipefail
: "${GH_TOKEN:?}" "${GITHUB_REPOSITORY:?}" "${RELEASE_ID:?}" "${RELEASE_TAG:?}"
[[ "$RELEASE_ID" =~ ^[0-9]+$ ]] || exit 1
cd dist
for asset in Alacritty-macos-universal.zip Alacritty-macos-universal.dmg appcast.xml SHA256SUMS-macos.txt; do
    [[ -s "$asset" ]] || { echo "Missing release asset: $asset" >&2; exit 1; }
done
sha256sum --check SHA256SUMS-macos.txt
# Validate the immutable release ID before uploading anything to its tag.
release=$(gh api "repos/$GITHUB_REPOSITORY/releases/$RELEASE_ID")
jq -e --arg tag "$RELEASE_TAG" '.tag_name == $tag and .draft == false and .prerelease == true' <<< "$release" >/dev/null
gh release upload "$RELEASE_TAG" ./* --repo "$GITHUB_REPOSITORY" --clobber
# Check GitHub's uploaded asset sizes and SHA-256 digests before promotion.
gh api "repos/$GITHUB_REPOSITORY/releases/$RELEASE_ID" > ../release-upload.json
python3 - <<'PY'
import hashlib, json, pathlib
assets = {a['name']: a for a in json.loads(pathlib.Path('../release-upload.json').read_text())['assets']}
for path in pathlib.Path('.').iterdir():
    asset = assets[path.name]
    assert asset['state'] == 'uploaded' and asset['size'] == path.stat().st_size, path.name
    assert asset['digest'] == 'sha256:' + hashlib.sha256(path.read_bytes()).hexdigest(), path.name
print('All uploaded release assets verified.')
PY
gh api --method PATCH "repos/$GITHUB_REPOSITORY/releases/$RELEASE_ID" \
    -F prerelease=false -f make_latest=true --silent
[[ $(gh api "repos/$GITHUB_REPOSITORY/releases/latest" --jq .id) == "$RELEASE_ID" ]]
echo "Release $RELEASE_TAG is now stable and Latest."
