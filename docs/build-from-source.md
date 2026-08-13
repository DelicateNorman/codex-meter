# Build from source

Codex Meter 0.16 and later are native Rust applications. The frozen Python
v0.15 source is retained at tag `v0.15.0` and branch
[`legacy-python-v0.15`](https://github.com/DelicateNorman/codex-meter/tree/legacy-python-v0.15),
not duplicated on `main`.

## CLI

Install [Rust with rustup](https://rustup.rs/) (Rust 1.85 or newer), then run:

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter
cargo fmt --check
cargo test --all-targets --locked
cargo build --release --locked
./target/release/codex-meter --no-color demo
./target/release/codex-meter
```

On Windows PowerShell, run `target\release\codex-meter.exe`. No Python runtime
is required. The executable reuses `~/.codex-meter/config.toml`, `pricing.json`,
and `meter.db`, so building or replacing it does not clear existing history.

## macOS desktop app

Install current Node.js and stable Rust, then run:

```bash
cd desktop
npm ci
npm run build
npm run test:e2e
npm run tauri dev
```

Create an app bundle and DMG with:

```bash
npm run tauri -- build --bundles app,dmg
```

Artifacts are written below `desktop/src-tauri/target/release/bundle/`. A local
build is ad-hoc signed by default. Browser-downloaded public builds require an
Apple Developer ID signature and notarization to pass Gatekeeper without a
manual approval step.

## Platform notes

- Codex state comes from `$CODEX_HOME`, defaulting to `~/.codex`.
- Codex Meter state comes from `CODEX_METER_HOME`, defaulting to
  `~/.codex-meter`.
- Native Windows uses Win32 console input; Linux, macOS, and WSL use terminal
  input appropriate to those systems.
- When Codex runs inside WSL2, build or install Codex Meter inside the same
  distribution.
- Passive packet-metadata capture is optional on Linux/macOS and normally
  unavailable on native Windows. Rollout statistics and response timing still
  work without it.
- Explicit TLS diagnostics need `openssl`; all ordinary reporting works without
  it.
