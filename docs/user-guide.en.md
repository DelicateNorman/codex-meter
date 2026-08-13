# Codex Meter User Guide

Codex Meter is a standalone companion for Codex CLI. It reads the Codex usage history owned by the current operating-system user and helps you understand tokens, projects, models, cache efficiency, latency, weekly account limits, and API-equivalent cost.

It does not replace the official `codex` command, and it does not clear or modify your existing Codex sessions.

![Codex Meter dashboard](assets/dashboard.png)

## 1. Install

### Linux and macOS

Open a terminal and paste this one-line installer:

```bash
curl -fsSL https://raw.githubusercontent.com/DelicateNorman/codex-meter/v0.17.0-beta.1/install.sh | sh
```

Then run:

```bash
codex-meter
```

The program is installed for the current user under `~/.local/bin`; `sudo` is not required.

### Windows PowerShell

Open PowerShell and paste:

```powershell
irm https://raw.githubusercontent.com/DelicateNorman/codex-meter/v0.17.0-beta.1/install.ps1 | iex
```

Open a new terminal after installation, then run:

```powershell
codex-meter
```

The Windows build is installed under:

```text
%LOCALAPPDATA%\Programs\CodexMeter\bin
```

### Verify the installation

```bash
codex-meter --version
```

For this release, the result should be:

```text
codex-meter 0.17.0-beta.1
```

If the shell cannot find the command, close the terminal and open a new one. On Linux and macOS, you can also check:

```bash
command -v codex-meter
```

A normal result looks like:

```text
/home/your-name/.local/bin/codex-meter
```

## 2. First launch

Run:

```bash
codex-meter
```

Codex Meter will:

1. Scan new or changed Rollout files under the current user's `~/.codex/sessions` directory.
2. Store normalized statistics—without conversation content—in its local database.
3. Open the keyboard-driven interactive dashboard.

Files that were already imported and have not changed are skipped. Later launches therefore do not need to parse your entire history again.

Weekly account limits load in the background. The dashboard may briefly show:

```text
ACCOUNT WEEKLY LIMITS  Loading…
```

When Codex returns the limits, the dashboard updates the bars automatically; no key press is required.

## 3. Navigate the dashboard

The main menu appears at the bottom of the terminal.

| Key | Action |
|---|---|
| `↑` `↓` `←` `→` | Move through the bottom menu |
| `Enter` or `Space` | Open the selected view |
| `/` | Open the searchable command palette |
| `Esc` | Close the command palette, Project picker, or Help |
| `r` | Refresh local records, configured SSH sources, and weekly limits |
| `q` | Quit from the main screen |
| `Ctrl+C` | Quit the program |

### Why does `q` sometimes not quit?

Inside the `/` command palette and the Project search field, `q` is treated as text so that commands and project names may contain that letter.

Press `Esc` to return to the main screen, then press `q` to quit.

### The `/` command palette

After pressing `/`, type to search or use the arrow keys to choose a command:

```text
/today
/week
/month
/all
/history day
/history week
/history month
/network
/project
/refresh
/help
/quit
```

Each command includes a short English description. Press `Enter` to run it or `Esc` to return.

## 4. What each view measures

### Today

Usage for the current local calendar date.

### Week

Usage during the current local calendar week, from Monday through Sunday.

### Month

Usage during the current local calendar month.

### All time

Usage from the first imported Codex record through now.

### Daily, Weekly, and Monthly history

Usage grouped by calendar day, week, or month. These views are useful for tracking longer-term changes.

### Network

Performance information that Codex Meter has been able to collect, including:

- TTFT: time from request start to the first output token;
- E2E: total elapsed time from request start to completion;
- Output TPS: output tokens per second;
- recent network connections, byte counts, status, and latency.

Not every Codex record contains every timing field. If the source data is insufficient, a speed or latency is shown as `N/A` or `estimated` instead of being invented.

### Project

Changes the project scope. The default is `All projects`, meaning all Codex projects owned by the current OS user.

Projects are ordered by most recent activity. Type to filter the list, use the arrow keys to move, press `Enter` to apply the selection, or press `Esc` to cancel.

A project name is the final directory component of the Rollout working directory. Paths with the same final directory name are intentionally grouped as one project.

## 5. Weekly account limits versus the Week report

These two measurements are easy to confuse, but they come from different sources and use different time windows.

### ACCOUNT WEEKLY LIMITS

These are the real seven-day limits returned by the currently active Codex account. For example:

```text
Codex · 82% left · reset Aug 18 11:42
Used  ███████░░░░░░░░░░░░░░░░░░░░░░░░░ 18%
```

- `18%` is the portion already used during this allowance period.
- `82% left` is the remaining portion.
- `reset` is the reset time displayed in your local time zone.
- If the account returns multiple limit buckets, Codex Meter shows each one separately—for example, Codex and GPT-5.3-Codex-Spark.

The account limit does not change when you select Day, Week, Month, or a particular project.

### Week

The Week report is calculated from imported history for the current Monday-to-Sunday calendar week. It is an analysis view, not an account allowance.

In short:

- the green bars at the top answer “How much of the account's seven-day limit remains?”;
- the Week view answers “How much activity was recorded during this local calendar week?”

The numbers are not expected to match.

## 6. Understand token metrics

### Headline metrics

| Field | Meaning |
|---|---|
| `TOKENS` | Total tokens in the selected scope |
| `API-EQUIV` | Estimated equivalent cost at public API prices; not a subscription bill |
| `CACHE` | Cached input as a percentage of all input |
| `CALLS` | Number of identified model calls |
| `Input` | Input tokens |
| `Output` | Output tokens |
| `Reasoning` | The portion of output classified as reasoning tokens |
| `Cache read` | Input tokens reused from cache |
| `Cache miss` | Input tokens not served from cache |
| `Cache write` | Tokens written to cache |

### The three token bars

- `Input total` is the total number of input tokens; it deliberately omits a meaningless 100% label.
- `Cached input` is cached input divided by all input.
- `Reasoning out` is reasoning tokens divided by all output.

For example:

```text
Cached input  96.7% of input
Reasoning out 29.7% of output
```

The two percentages use different denominators and should not be compared directly.

### Why does a price show `N/A`?

`N/A` is classified instead of silently omitted:

- **No published price**: an internal, review, or new model has no price in the
  current catalog;
- **Missing model metadata**: the original record does not identify a model;
- **Historical estimate**: the call predates the earliest known price, so Codex
  Meter includes it using that earliest price and marks it as estimated.

Calls and tokens are always counted. Only the first two categories are excluded
from cost. Check or update the checksum-verified catalog with:

```bash
codex-meter pricing
codex-meter pricing --update
```

## 7. Filter by project

The Project menu is usually the easiest method in the interactive dashboard. You can also select a project on the command line:

```bash
codex-meter today --project codex-stats
codex-meter summary --period week --project codex-stats
codex-meter summary --period month --project codex-stats
codex-meter history --group month --project codex-stats
```

List project totals with:

```bash
codex-meter projects
```

Without `--project`, reports always include all projects.

## 8. Include a remote server used from your Mac

When Codex Desktop opens a project over SSH, Codex runs on that server and normally writes its Rollouts to the server's `~/.codex/sessions`. A Mac-only local scan therefore cannot see those tokens.

Starting with version 0.15.0, an SSH host can be configured as another history source. Codex Meter does not need to be installed on the remote server. The Mac must be able to connect through SSH. Python 3 on the remote host provides the privacy filter. If it is unavailable, Codex Meter stops with an actionable message instead of transferring raw Rollouts.

First verify that the SSH alias used by Codex Desktop also works in the Mac terminal:

```bash
ssh devbox
```

After leaving the remote shell, add the source from your Mac:

```bash
codex-meter remote add devbox
codex-meter
```

`devbox` should be a `Host` alias from `~/.ssh/config`, not an arbitrary shell command. Codex Meter checks both the connection and the remote Rollout directory before saving the source, then performs the first sync.

On later launches, the local dashboard appears immediately while remote history syncs in the background. The title and source status look similar to:

```text
CODEX METER  ● LOCAL + 1 REMOTE
REMOTE SOURCES  devbox synced · 2 updated
```

After syncing, remote activity is merged into Day, Week, Month, All time, history, project, model, and performance reports. Pressing `r` refreshes local history, every configured remote source, and the weekly account limits.

Manage remote sources with:

```bash
codex-meter remote list
codex-meter remote test devbox
codex-meter remote sync
codex-meter remote sync devbox
codex-meter remote remove devbox
```

The first sync reads existing history and displays per-file/source-byte progress.
Later syncs process only new or changed Rollouts. Filtering happens on the
server and only gzip-compressed token/model/timing/project metadata crosses SSH;
prompts, responses, reasoning, commands, and tool output do not. A remote host
without Python 3 is rejected with an actionable message instead of using a raw
transfer fallback.

`remote remove` stops future synchronization but intentionally keeps statistics that were already imported.

If setup fails, first resolve login, host-key, or key-agent problems by running `ssh devbox` normally. Background synchronization uses non-interactive SSH and never waits behind the dashboard for a password prompt.

## 9. Filter by optional account label

Account labels are disabled by default. Codex Meter never derives them from an email address or authentication file; you assign a local name manually.

Check the current status:

```bash
codex-meter account status
```

Enable labels and select one:

```bash
codex-meter account enable personal
```

Switch the active label later:

```bash
codex-meter account set work
```

List known labels:

```bash
codex-meter account list
```

Filter a report:

```bash
codex-meter summary --period month --account work
```

Disable account labels:

```bash
codex-meter account disable
```

Labels primarily apply to sessions imported after a label is selected. Existing unlabeled history remains `Unassigned`. Only if you are certain that every unassigned session belongs to the same account should you run:

```bash
codex-meter account claim-unassigned personal
```

## 10. Query without the interactive dashboard

### One day

```bash
codex-meter summary --period day --date 2026-08-12
```

### One week

`--date` may be any date within the desired week:

```bash
codex-meter summary --period week --date 2026-08-12
```

### One month

```bash
codex-meter summary --period month --date 2026-08-12
```

### All history

```bash
codex-meter summary --period all
```

### Plain-text output

The global `--no-color` option must appear before the subcommand:

```bash
codex-meter --no-color summary --period week
```

## 11. Export statistics

Export CSV:

```bash
codex-meter export \
  --from 2026-08-01 \
  --to 2026-08-12 \
  --format csv \
  --output usage.csv
```

Export JSONL:

```bash
codex-meter export --format jsonl --output usage.jsonl
```

Export one session:

```bash
codex-meter export --session SESSION_ID --format json
```

Exports contain usage and performance metadata. They do not include prompts, response text, reasoning text, or tool output.

## 12. Network and packet-metadata diagnostics

### View saved network records

```bash
codex-meter network show
```

### Test DNS, TCP, and TLS connection setup

```bash
codex-meter network probe api.openai.com
```

### Passively capture packet sizes and directions

```bash
codex-meter network capture \
  --host api.openai.com \
  --host chatgpt.com \
  --duration 15
```

This mode does not use `tcpdump -A`, `-X`, or `-w`. It stores only:

- destination;
- transfer direction;
- packet counts and lengths;
- elapsed time.

Packet payloads are not saved, although the operating system may still require permission to run tcpdump. Linux and macOS select a common capture interface automatically. Passive capture on Windows requires a compatible tcpdump environment.

### CONNECT proxy without TLS decryption

```bash
codex-meter proxy tunnel --port 8899
```

### HTTP/WebSocket reverse proxy

```bash
codex-meter proxy reverse \
  --port 8900 \
  --upstream https://chatgpt.com/backend-api/codex
```

Requests and responses are forwarded in memory. Only status, timing, and byte counts are stored.

### Explicit TLS termination

This is a separate advanced diagnostic mode. Enable it only when you deliberately need it:

```bash
codex-meter proxy tls-init
codex-meter proxy tls --acknowledge-sensitive \
  --upstream https://chatgpt.com/backend-api/codex
```

Codex Meter creates a short-lived local CA and certificates under `~/.codex-meter/tls`. Trust that CA only for the diagnostic window, and remove its trust from the operating system afterward. Even in this mode, Codex Meter does not write request headers, bodies, SSE data, or WebSocket frames to its database.

## 13. Optional live performance sources

Rollout history is sufficient for normal usage reporting. These optional sources can provide more precise latency, throughput, or lifecycle information.

### OTLP

Generate a Codex configuration snippet:

```bash
codex-meter otel config
```

Copy the output into `~/.codex/config.toml`, then start the collector before launching Codex:

```bash
codex-meter otel serve
```

The OTLP collector keeps only a small allowlist of statistical fields. It does not store prompts, arbitrary event bodies, or unapproved attributes.

### App Server proxy

```bash
codex-meter app-server proxy
```

Import an existing App Server JSONL stream:

```bash
codex-meter app-server ingest FILE
```

App Server data can add exact per-response usage, Turn lifecycles, tool types and timing, reroutes, and compactions. Request content remains in memory and is not persisted.

## 14. Data location and ownership

The default data directory is:

```text
~/.codex-meter/
├── meter.db
├── config.toml
├── pricing.json
└── logs/
```

`meter.db` is the primary statistics database.

For local history, Codex Meter reads only the current user's:

```text
~/.codex/sessions/
```

It does not scan the home directories of other operating-system users. Because `~/.codex-meter` is also resolved inside the current user's home, each OS user gets a separate database by default. Configured SSH sources are imported into the database of the local OS user who added them.

### Back up the database

Close Codex Meter, then copy the database:

```bash
cp ~/.codex-meter/meter.db ~/codex-meter-backup.db
```

### Use a different data directory

For one invocation:

```bash
codex-meter --home /path/to/meter-data
```

Or with an environment variable:

```bash
export CODEX_METER_HOME=/path/to/meter-data
```

## 15. Update and uninstall

### Update

Find the newest version on the [Releases page](https://github.com/DelicateNorman/codex-meter/releases), then rerun its one-line installer.

The installer replaces only the program files. It does not remove `~/.codex-meter/meter.db`.

### Remove the program but keep statistics

Linux and macOS:

```bash
rm ~/.local/bin/codex-meter
```

Windows PowerShell:

```powershell
Remove-Item "$env:LOCALAPPDATA\Programs\CodexMeter\bin\codex-meter.exe"
```

These commands leave the database intact. If you reinstall later, the existing statistics remain available.

Delete `~/.codex-meter` manually only when you are certain that you no longer need any history or configuration stored by Codex Meter.

## 16. Troubleshooting

### Weekly limits are missing

1. Check the installed version:

   ```bash
   codex-meter --version
   ```

2. Allow `Loading…` to update; limit retrieval does not block the dashboard.
3. Press `r` to retry.
4. Read the specific `Unavailable` reason shown in the dashboard.
5. Confirm that the official `codex` command runs normally and that its current account is signed in.

### New history does not appear

Complete one real request in Codex, return to Codex Meter, and press `r`. You can also import explicitly:

```bash
codex-meter import ~/.codex/sessions
```

For a remote source, verify and synchronize it directly:

```bash
codex-meter remote test devbox
codex-meter remote sync devbox
```

### An older version still opens

```bash
command -v codex-meter
codex-meter --version
```

If the command path is not the current user's `~/.local/bin/codex-meter`, another executable with the same name appears earlier on `PATH`.

### The terminal layout is incomplete

Increase terminal height to show more model rows. The headline summary and weekly account limits have priority when vertical space is limited, and narrow terminals switch to a compact layout automatically.

### Report a data problem

Run:

```bash
codex-meter doctor
```

When filing an issue, include:

- `codex-meter --version`;
- your operating system;
- the output of `codex-meter doctor`;
- a screenshot that does not contain private information.

Do not upload `auth.json`, access tokens, complete Rollouts, prompts, or response content.

## 17. Run from source

Install stable Rust 1.85 or newer:

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter
cargo test --all-targets --locked
cargo build --release --locked
./target/release/codex-meter
```

Python is not required. On Windows run
`target\release\codex-meter.exe`. See [Build from source](build-from-source.md)
for the desktop build and platform-specific notes.

## 18. Privacy principles

Codex Meter's central rule is: collect statistical metadata, not content.

It does not persist:

- prompts, model responses, or reasoning text;
- shell commands, tool arguments, or tool output;
- HTTP headers, cookies, or authentication data;
- SSE payloads or WebSocket frames;
- Codex account email addresses or `auth.json`.

The database does keep the identifiers required for safe deduplication, token counters, model and reasoning-effort names, timestamps, project and Git metadata, status, byte counts, and available timing measurements.

Every collector—including local Rollouts, SSH streams, OTLP, App Server, packet metadata, and proxy diagnostics—must follow the same content-exclusion policy. If you discover a path that may store content or credentials, stop using that feature and open an issue.
