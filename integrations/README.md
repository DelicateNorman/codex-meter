# Codex TUI integration

Codex 0.146.1 has no external custom-slash-command registration API. The included patch adds a native `/meter` command to the upstream TUI without replacing an installed Codex binary.

Apply it to the matching `rust-v0.146.1` source tree:

```bash
git apply /absolute/path/to/codex-stats/integrations/codex-0.146.1-meter-command.patch
cargo build --release -p codex-cli
```

`/meter` invokes `codex-meter --no-color today --refresh` and renders the result in TUI history. It uses `codex-meter` from `PATH`; set `CODEX_METER_BIN` to an absolute executable path when needed.

The patch was verified against `rust-v0.146.1` with Rust 1.97.1: `cargo check -p codex-tui`, repository formatting, `just fix -p codex-tui`, the affected slash-command ordering test, and an accepted `/meter` popup snapshot all pass. An initial wider upstream TUI run completed 3,252 tests: 3,229 passed, one affected ordering test was then updated and passed, and the remaining 22 failures were pre-existing snapshots comparing the source tag's `v0.146.1` against fixtures expecting `v0.0.0`.

The patch is intentionally not applied to the user's installed binary automatically. Building and replacing Codex changes upstream executable provenance and remains an explicit user action.
