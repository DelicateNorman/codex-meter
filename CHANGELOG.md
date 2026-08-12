# Changelog

All notable changes to Codex Meter are documented here.

## [0.16.0-beta.3] - 2026-08-12

### Added

- Show live per-file and source-byte progress for foreground and background SSH
  synchronization.

### Changed

- Filter Rollouts on Python 3-capable remote hosts and send only a
  gzip-compressed metadata allowlist over SSH; prompts, responses, reasoning,
  commands, and tool output no longer cross the connection.
- Preserve a clearly labelled legacy in-memory transfer fallback for remote
  hosts without Python 3.

### Verified

- The development host's live 60-file, 678 MiB Rollout set became a 2.17 MiB
  compressed metadata stream (99.7% smaller) and completed the new server-filter
  path in about three seconds on a local transport simulation; all 21,000+
  exported call rows matched a full local parse exactly.

## [0.16.0-beta.2] - 2026-08-12

### Fixed

- Render interactive rows with explicit CRLF line endings in raw mode, fixing
  severe horizontal row drift in macOS Terminal.
- Extend native PTY acceptance coverage so bare line feeds fail before release.

## [0.16.0-beta.1] - 2026-08-12

- Rewrite the application runtime in Rust while retaining the v0.15 command surface, SQLite schema, configuration, pricing, privacy rules, and reports.
- Keep interactive first paint non-blocking while weekly limits and SSH sources update in background workers.
- Port Rollout, quota, OTLP, App Server, network capture, CONNECT, reverse proxy, TLS diagnostics, export, and doctor functionality.
- Add a Linux/macOS/Windows Rust CI matrix, release-candidate artifact workflow, and automated Python/Rust database/export differential test.
- Make checksum-verified upgrades transactional, retain the previous executable for rollback, and leave the Meter history directory outside the install transaction.
- Exercise one-line install, v0.15 upgrade, tamper rejection, rollback, history preservation, and native terminal input on all four release targets.
- Validate the complete local 0.63 GiB Rollout history: 60 files, 1,712 turns, 20,643 calls, and 1,926 tools matched Python exactly.
- Reduce the optimized Linux executable to about 5.7 MiB and improve no-op startup from roughly 90 ms to below the timer's 10 ms resolution on the development machine.

## [0.15.0] - 2026-08-12

- Add configurable SSH sources for Codex Desktop and CLI sessions executed on remote hosts.
- Stream remote Rollouts directly into the metadata-only collector without saving raw conversations locally.
- Incrementally transfer only changed remote files and deduplicate sessions/calls across local and remote sources.
- Open the dashboard immediately while remote history and weekly quotas refresh in parallel.
- Merge remote usage into day/week/month/all-time, history, project, model, and performance views.
- Add `remote add`, `list`, `test`, `sync`, and `remove` commands with safe SSH-alias validation.

## [0.14.2] - 2026-08-12

- Open the interactive dashboard immediately and load live weekly quotas in the background.
- Redraw quota bars automatically when Codex responds, without waiting for a key press.
- Keep the dashboard and weekly quota rows visible in terminals narrower than 80 columns.
- Lazy-load diagnostic modules so ordinary startup imports less code and uses less memory.
- Strip and optimize standalone release bundles while smoke-testing every optional command.

## [0.14.1] - 2026-08-12

- Render each live seven-day account quota as a prominent usage bar at the top of the dashboard.
- Keep weekly quotas visible when a short terminal clips lower dashboard sections.
- Show the concrete App Server error when live quotas are unavailable.
- Replace the ambiguous token percentages with `total input`, `% of input`, and `% of output` labels.
- Add width and clipping regressions for 80-, 100-, and 132-column terminal layouts.

## [0.14.0] - 2026-08-12

- Add native interactive keyboard handling and ANSI console setup for Windows.
- Replace POSIX pipe polling in the live quota reader with a cross-platform reader.
- Add standalone Linux, macOS arm64/x86_64, and Windows x86_64 release binaries.
- Add checksum-verifying one-line installers for Linux/macOS and Windows PowerShell.
- Auto-detect tcpdump interfaces so passive metadata capture works with macOS interface names.
- Test source installs on Linux, macOS arm64/x86_64, and Windows in GitHub Actions.
- Add an automated tagged-release workflow that builds, smoke-tests, checksums, and publishes every platform asset.

## [0.13.0] - 2026-08-12

- Show live account weekly limits, used/remaining percentages, and local reset times in the interactive overview.
- Keep account limits independent from local day/week/month/project filters.
- Display separate backend quota buckets, including named model-specific limits, without reading or storing credentials.
- Refresh quota snapshots together with local rollout history and fail gracefully when Codex does not provide them.

## [0.12.4] - 2026-08-12

- Show refresh progress before scanning local Codex records and confirm completion afterward.
- Add actionable guidance when no usage exists for the selected period or project.
- Return the menu cursor to the active view when Help closes.
- Close Help before a keyboard refresh and clarify modal `q` behavior.
- Extend Help with project-filter controls.

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

[0.16.0-beta.3]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.16.0-beta.3
[0.16.0-beta.2]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.16.0-beta.2
[0.16.0-beta.1]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.16.0-beta.1
[0.15.0]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.15.0
[0.14.2]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.14.2
[0.14.1]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.14.1
[0.14.0]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.14.0
[0.13.0]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.13.0
[0.12.4]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.12.4
[0.12.3]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.12.3
[0.12.2]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.12.2
[0.12.1]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.12.1
[0.12.0]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.12.0
[0.11.2]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.11.2
[0.11.1]: https://github.com/DelicateNorman/codex-meter/releases/tag/v0.11.1
