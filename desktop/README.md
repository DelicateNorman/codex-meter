# Codex Meter Desktop

The desktop app is a macOS-first Tauri 2 frontend over the same Rust collector,
privacy filters, pricing catalog, SQLite schema, and SSH synchronizer used by
the `codex-meter` CLI. Both apps share `~/.codex-meter`; neither stores prompt,
response, reasoning, command, tool-output, header, or credential content.

## Develop on macOS

Install the current Node.js LTS and stable Rust toolchain, then run:

```bash
cd desktop
npm ci
npm run tauri dev
```

## Build an app and DMG

```bash
cd desktop
npm ci
npm run tauri build
```

Artifacts are written below `desktop/src-tauri/target/release/bundle/`. The
GitHub workflow builds separate Apple Silicon and Intel artifacts. Public
distribution still requires an Apple Developer ID certificate and notarization;
unsigned CI builds are intended for project testing only.
