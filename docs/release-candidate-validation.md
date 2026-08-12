# Rust 0.16 release validation

Validation date: 2026-08-13
Candidate branch: `main`
Validated beta commit: `81a0292ae56a475f3920b34bff4459be8c4c58ed`

## Outcome

The Rust candidate passed the automated release rehearsal on Linux x86_64,
macOS arm64, macOS x86_64, and Windows x86_64. The release workflow and normal
Rust workflow were both green:

- [release installation and artifact run](https://github.com/DelicateNorman/codex-meter/actions/runs/31606754285)
- [format, Clippy, tests, differential fixture, and command-smoke run](https://github.com/DelicateNorman/codex-meter/actions/runs/31606754311)

The candidate was subsequently published as the public
[v0.16.0-beta.1 Rust prerelease](https://github.com/DelicateNorman/codex-meter/releases/tag/v0.16.0-beta.1).
Its [tagged release run](https://github.com/DelicateNorman/codex-meter/actions/runs/31610921094)
passed the same four-platform build and acceptance matrix, published only the
four native Rust executables plus `SHA256SUMS`, and then repeated one-line
installation and rollback from the public URLs on all four platforms.

The macOS Terminal row-drift regression reported against beta.1 was corrected
in [v0.16.0-beta.2](https://github.com/DelicateNorman/codex-meter/releases/tag/v0.16.0-beta.2).
Its [tagged release run](https://github.com/DelicateNorman/codex-meter/actions/runs/31612267447)
passed the complete four-platform build, native-terminal, installation,
checksum, history-preservation, rollback, publication, and public-URL matrix.

Remote-sync progress and remote-side metadata filtering were added in
[v0.16.0-beta.3](https://github.com/DelicateNorman/codex-meter/releases/tag/v0.16.0-beta.3).
Its [tagged release run](https://github.com/DelicateNorman/codex-meter/actions/runs/31615733928)
passed the same complete matrix for all four platforms, including installation
from the final public download URLs.

## Per-platform release rehearsal

Each native runner performed the following operations with its own executable:

1. Build the optimized Rust executable and run `--version` and `demo`.
2. Generate a platform `SHA256SUMS` entry.
3. Download the matching executable from the real public v0.15.0 release.
4. Seed a real v0.15 database and preservation canary with that executable.
5. Install the Rust candidate through the one-line script entry point.
6. Verify that the installed file is the candidate and `.previous` is v0.15.
7. Append a byte to a copied candidate and confirm checksum rejection leaves the installed executable unchanged.
8. Roll back and verify that v0.15 is restored while the Rust file becomes `.previous`.
9. Upgrade again; run summary, history, export, and doctor against the v0.15 database; verify SQLite integrity, row counts, and usage aggregates remain unchanged.
10. Compare an exact SHA-256 manifest of every Meter-home file before and after installation and rollback.

The four files were assembled into one artifact with a combined checksum file.
That artifact was downloaded again on the development host; all four checksums,
file formats, names, and the Linux executable self-check passed.

## Native terminal input

Linux and both macOS runners used native pseudo-terminals. Windows used a native
ConPTY session. Each release executable was opened interactively at 120×30 and
verified the following sequence:

- every raw-mode screen row uses CRLF and therefore returns to column zero;
- the dashboard selects Today;
- Right selects Week;
- `/` opens the English command palette;
- `q` is entered as text and does not quit;
- `Esc` returns to the main dashboard;
- `q` then exits successfully.

The CRLF assertion covers the exact beta.1 failure: the old bare-LF renderer
could contain all expected text while drawing each next row at the previous
row's ending column in macOS Terminal. This is a real automated terminal session
on each operating system. It is not a manual recording from Terminal.app or
Windows Terminal.

## Real SSH synchronization

A configured external OpenSSH alias was tested with a temporary metadata-only
Rollout. Discovery found one file, the first sync imported one session, one turn,
one call, and 110 tokens, and the second sync skipped the unchanged file. Removing
the source preserved the imported statistics. No raw Rollout file was written to
the local Meter home. The exact temporary remote file and its test directory were
removed after verification.

Beta.3 additionally runs an allowlist filter on the SSH host when Python 3 is
available. Prompts, responses, reasoning text, commands, tool output, headers,
and secrets are excluded before transmission. An end-to-end fake-SSH test
confirms that content canaries never enter the local database or WAL and that a
repeat sync skips unchanged sources. A 60-file live development dataset was
also compared through the full parser and filtered path: more than 21,000 CSV
rows matched exactly, while 678.1 MiB of raw Rollouts produced a 2,273,557-byte
compressed metadata stream (about 99.7% smaller). The same test verifies file,
byte, and percentage progress callbacks from start through completion.

## Live history preservation

The public beta.1 was installed and rolled back in an isolated bin directory
while `CODEX_METER_HOME` pointed to the development user's real
`~/.codex-meter`. SHA-256 hashes for every file under that directory matched
exactly before installation, after upgrade, and after rollback. The installed
candidate hash matched the public Linux asset. The real
`~/.local/bin/codex-meter` was not replaced during that specific check.

Beta.2 repeated the same exact-manifest preservation check on all four release
runners using seeded v0.15 histories. A development-host check against the live
home was deliberately not forced while existing `codex-meter` processes were
active; the hotfix changes only terminal row separators and no storage path.

Beta.3 again passed exact-manifest history preservation during install,
upgrade, checksum rejection, and rollback on every release runner. Its remote
filter parity benchmark used isolated Meter homes and did not modify the live
history.

## Post-release follow-up checks

- Record a short manual interaction in macOS Terminal and Windows Terminal; add a WSL smoke check.
- Run privileged live `tcpdump` capture on an approved machine. Parser behavior and safe failure are already automated.

These manual recordings and the optional privileged capture remain worthwhile
supplementary evidence. For v0.16.0, the formal release gate is the completed
four-platform native terminal, privacy, installer, history-preservation,
differential, real-SSH, and public-download matrix documented above.
