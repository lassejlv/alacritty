# Releases

Publish a **prerelease** in this repository from a tag containing the release
workflow. The Release workflow tests and builds macOS, signs and
notarizes the app, and uploads DMG/ZIP and SHA-256 checksums to that same release. Only after all jobs and uploads
succeed does it remove the prerelease flag and mark the release **Latest**.
A failed build leaves it as a prerelease. Rerun failed jobs after resolving the
failure. Publish releases in order; rerunning an older release can make it Latest.

The manual **Run workflow** action builds signed downloads as Actions artifacts
without creating or modifying a GitHub release. Use it to validate credentials
and packaging before publishing.

macOS downloads contain a universal Apple Silicon/Intel app with a macOS 27.0
minimum. The `xcode-27` runner provides the matching SDK and native ARM test host;
Intel is cross-compiled and included but not runtime-tested on that runner.
The app and DMG are Developer ID signed, notarized, stapled, and assessed with
Gatekeeper. The ZIP is made from the final stapled app.

## Apple credentials

Set repository variable `APPLE_TEAM_ID` and these Actions secrets:

- `APPLE_CERTIFICATE_P12_BASE64`: base64 password-protected Developer ID
  Application certificate **and private key**.
- `APPLE_CERTIFICATE_PASSWORD`: that P12's password.
- `APPLE_NOTARY_KEY_P8_BASE64`: base64 App Store Connect Team API private key.
- `APPLE_NOTARY_KEY_ID`: the API key ID.
- `APPLE_NOTARY_ISSUER_ID`: its team's issuer ID.

Credentials are imported into a temporary runner keychain added to the search
list. The workflow restores the original list and removes the keychain and
credentials even if signing fails. Missing credentials fail the release; there
is no ad hoc fallback. PR CI never receives these secrets.

Keep credentials and recovery exports outside the repository. Rotate secrets
in Settings → Secrets and variables → Actions when the certificate or API key
changes. Notarization submission IDs appear in the job log; inspect a pending
submission with `notarytool info` before submitting it again.
