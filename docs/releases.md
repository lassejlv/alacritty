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

## macOS automatic updates

Packaged apps include Sparkle 2.10.0 for Intel and Apple Silicon. Sparkle checks
every six hours, and **Alacritty → Check for Updates…** checks immediately.
The native updater offers downloads and asks before installing and relaunching;
automatic installation is disabled to avoid interrupting terminal sessions.
Quit or finish running commands before choosing to install and relaunch.
Standalone Cargo binaries do not initialize an updater.

Release tags must be `vMAJOR.MINOR.PATCH` (or `MAJOR.MINOR.PATCH`), e.g. `v0.18.0`.
Publish the GitHub release with the prerelease checkbox enabled. The workflow
stamps that version into both bundle version fields before signing, signs the
final ZIP and appcast with Ed25519, and attaches `appcast.xml` alongside the
downloads. The feed lives at
`https://github.com/lassejlv/alacritty/releases/latest/download/appcast.xml`;
its archive URL always includes the specific release tag. Promoting the release
to Latest switches the feed only after all uploaded assets have been verified.
Sparkle verifies the feed and archive before extracting the update. Do not edit
the generated appcast after signing it or replace published release archives.

The first build with Sparkle must be installed manually. Older Alacritty builds
without an updater cannot acquire this feature automatically. Before the first
release, manual update checks report that the feed is unavailable.

Manual workflow builds use bundle build number `0` and a validation-only archive
URL. They upload signed artifacts without making an update available to users.
Local `make app` / `make app-universal` fetch the pinned Sparkle distribution,
verify its SHA-256, and embed it with symlinks intact. Plain `cargo build` stays
independent of the downloaded framework.

Set the **`SPARKLE_PRIVATE_KEY`** Actions secret to the exported base64 Ed25519
seed from Sparkle's `generate_keys`. The matching public key is committed in the
app's Info.plist. The release script checks that the private key matches this
public key before generating the feed. Keep the recovery export outside the
repo with restrictive permissions; never regenerate this key for each release.
Sparkle's framework, updater app, and XPC helpers are Developer ID signed from
the inside out before signing/notarizing the outer app.

References: [Sparkle setup](https://sparkle-project.org/documentation/),
[update behavior](https://sparkle-project.org/documentation/customization/),
[publishing updates](https://sparkle-project.org/documentation/publishing/).

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
