# Release signing

Codex Meter can build unsigned preview artifacts without private credentials.
Stable release tags deliberately fail unless both macOS and Windows signing are
configured, so an unsigned desktop build cannot accidentally be presented as a
normal stable release.

## macOS Developer ID and notarization

Apple notarization requires a paid Apple Developer Program membership. Create a
`Developer ID Application` certificate, export it as a password-protected
PKCS#12 (`.p12`) file, and configure these GitHub Actions secrets:

- `APPLE_CERTIFICATE`: the base64-encoded `.p12` contents
- `APPLE_CERTIFICATE_PASSWORD`: the export password
- `APPLE_ID`: the Apple account used for notarization
- `APPLE_PASSWORD`: an app-specific password for that account
- `APPLE_TEAM_ID`: the ten-character developer team identifier

The release workflow imports the certificate into a temporary keychain, asks
Tauri to sign and notarize the app, then requires both `stapler validate` and
Gatekeeper's `spctl` assessment to pass. The temporary runner is discarded after
the job; certificate material is never committed to the repository.

See [Tauri's macOS signing guide](https://v2.tauri.app/distribute/sign/macos/)
for certificate creation and Apple account details.

## Windows Authenticode

Export the Windows code-signing certificate and private key as a
password-protected `.pfx`, then configure:

- `WINDOWS_CERTIFICATE`: the base64-encoded `.pfx` contents
- `WINDOWS_CERTIFICATE_PASSWORD`: the export password

The release workflow signs the executable with SHA-256, adds a trusted timestamp,
and runs `signtool verify /pa /v` before checksums and release artifacts are
created. See [Tauri's Windows signing guide](https://v2.tauri.app/distribute/sign/windows/)
for certificate options and SmartScreen considerations.

## Preview behavior

Pre-release tags such as `v0.17.0-beta.1` may still produce an ad-hoc-signed
macOS preview and an unsigned Windows executable when secrets are absent. The
workflow emits a warning and the documentation must continue to describe those
artifacts as previews. Stable tags such as `v0.17.0` will stop instead.
