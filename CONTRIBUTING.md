# Contributing

Codex Meter targets Linux, macOS, and native Windows. The current implementation
uses Rust 1.85 or newer. The frozen Python v0.15 reference lives on the
`legacy-python-v0.15` branch instead of being duplicated on `main`. Bug reports,
privacy reviews, documentation fixes, and focused pull requests are welcome.

## Development setup

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
```

For desktop changes:

```bash
cd desktop
npm ci
npm run build
npm run test:e2e
```

CI runs the Rust suite on Linux, macOS arm64/x86_64, and Windows; desktop
interaction, accessibility, and screenshot regression run in a browser harness.

Please keep collection content-free: prompts, responses, reasoning text, commands, tool output, headers, cookies, credentials, and authentication files must never be persisted.

Before opening a pull request, run the full test suite and describe any user-visible behavior changes. Platform changes should preserve native keyboard navigation and include platform-specific installation or packaging verification.
