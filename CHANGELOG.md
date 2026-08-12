# Changelog

All notable changes to Codex Meter are documented here.

## [0.12.3] - 2026-08-12

- Add instant keyboard filtering to the interactive project picker.
- Support UTF-8 and project names containing spaces in picker input.
- Keep `q` as filter text inside the picker and retain Esc-to-cancel behavior.
- Restore the current project selection after clearing a search.
- Make Unicode project labels respect terminal display-column widths.

## [0.12.2] - 2026-08-12

- Turn the slash-command palette into a focused overlay and avoid redundant database queries while it is open.
- Add compact, non-overflowing navigation for terminals narrower than 80 columns.
- Adapt project-picker page size to short terminals.
- Preserve headline usage metrics when a short terminal clips the dashboard.
- Add automated layout coverage across terminal widths and heights.

## [0.12.1] - 2026-08-12

- Sort the interactive project picker by most recent activity.
- Use English descriptions throughout the slash-command palette.
- Keep dashboard frames visually closed when a short terminal clips their contents.
- Keep the bottom menu within the dashboard width and remove its redundant status row.

## [0.12.0] - 2026-08-12

- Add an interactive `Project` selector with `All projects` as the default.
- Apply the selected project to day, week, month, all-time, history, and Network views.
- Add `/project` to the slash-command palette.
- Add `--project` to `today`, `summary`, and `history` for scripts.
- Keep project and OS-user/account filters composable.

## [0.11.2] - 2026-08-12

- Add explicit CA certificate-signing key usage for Python 3.13 TLS verification.
- Keep the one-line installer pinned to the latest tested stable release.

## [0.11.1] - 2026-08-12

First public Linux release.

- Interactive day, week, month, all-time, and history views.
- Keyboard menu and searchable slash-command palette.
- Network/response view with TTFT, end-to-end latency, and clearly marked estimated Token throughput.
- Local rollout JSONL import with replay/fork deduplication.
- Per-OS-user storage and optional manual account labels.
- Content-free OTLP, App Server, and network diagnostic adapters.
- Versioned API-equivalent pricing with unknown prices shown as `N/A`.
- One-line, no-sudo Linux installer.

[0.12.3]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.12.3
[0.12.2]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.12.2
[0.12.1]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.12.1
[0.12.0]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.12.0
[0.11.2]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.11.2
[0.11.1]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.11.1
