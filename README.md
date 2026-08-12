# Codex Meter

Codex Meter is a local-first usage and performance observability CLI for Codex. Version 0.11 adds a keyboard-driven interactive home screen with a Network response-performance view, imports rollout JSONL, receives Codex OTLP/HTTP JSON in real time, adapts App Server JSON-RPC, analyzes latency/cache/retries/compaction, provides daily/weekly/monthly/all-time reporting, and offers content-free network diagnostics and explicitly enabled local proxy modes.

It never imports prompts, model responses, reasoning content, shell commands, tool output, headers, cookies, or credentials.

## Platform support

The first public release officially supports **Linux** with Python 3.11 or newer. macOS and Windows are planned but are not yet tested or supported release targets.

## Install on Linux

Install the latest stable release for the current user without `sudo`:

```bash
curl -fsSL https://raw.githubusercontent.com/DelicateNorman/codex-meter/main/install.sh | sh
```

Then run:

```bash
codex-meter
```

The installer places the program under `~/.local/share/codex-meter` and the command under `~/.local/bin`. It does not modify the official `codex` command or delete existing data in `~/.codex-meter`. You can [read the installer](install.sh) before running it.

Stable versions and source archives are published on the [Releases page](https://github.com/DelicateNorman/codex-meter/releases). To install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/DelicateNorman/codex-meter/v0.11.2/install.sh | sh
```

## Build from source

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter
python3 -m venv .venv
source .venv/bin/activate
python -m pip install -e .
python -m unittest discover -v
codex-meter
```

See the complete [source-build guide](docs/build-from-source.md), including wheel creation.

## Quick usage

```bash
codex-meter
codex-meter today
codex-meter summary --period week
codex-meter history --group month
codex-meter doctor
```

Running `codex-meter` without a subcommand imports changed rollout files and opens an interactive dashboard. The menu sits below the report: use any arrow key to choose a view, then Enter or Space to open it. The Network view shows first-token latency, end-to-end time, and exact or clearly marked estimated output-token speed. Press `/` to open a command palette with short descriptions; Up/Down moves through the commands and automatically changes pages, Enter runs the selected command, and Esc closes the palette. While the palette is open all printable keys, including `q`, are command text. Back on the main screen, `r` refreshes and `q` quits. When output is redirected instead of attached to a terminal, the command prints today's overview for compatibility.

To preview the static UI without touching usage data:

```bash
codex-meter demo
```

Use `--no-color` before the subcommand for plain output:

```bash
codex-meter --no-color today
```

By default local state is created with user-only directory permissions:

```text
~/.codex-meter/
├── meter.db
├── config.toml
├── pricing.json
└── logs/
```

Override it with `CODEX_METER_HOME` or `--home`.

Each operating-system user gets a separate default database because `~` resolves to that user's home. Sessions also record the non-secret owner UID and username, and aggregate views stay scoped to that owner if a database is explicitly shared. Other Linux users' Codex homes are not scanned.

Account tracking is optional and disabled by default. It uses only a manually chosen local label and never reads Codex credentials, `auth.json`, tokens, or email addresses:

```bash
codex-meter account status
codex-meter account enable personal
codex-meter account set work
codex-meter account list
codex-meter summary --period month --account work
codex-meter account disable
```

Labels apply to future sessions. Existing history remains `Unassigned`; use `codex-meter account claim-unassigned LABEL` only when you explicitly know all unassigned history belongs to that label.

## Data flow

```text
~/.codex/sessions/**/rollout-*.jsonl
                    │
                    ▼
         SessionJsonlCollector
         (metadata only; fail-open)
                    │
                    ▼
             normalized records
 Session ── Turn ── LLM Call / Tool Call
                    │
          cumulative delta + semantic
          replay/fork deduplication
                    │
                    ▼
         SQLite (WAL) + Pricing Engine
                    │
          ┌─────────┼──────────┐
          ▼         ▼          ▼
       Overview   Models     Export
```

The canonical `MeterEvent` envelope and event payload types live in `codex_meter/models.py`; collection, pricing, storage, and UI do not depend on one another.

## Directory structure

```text
codex_meter/
├── models.py                 normalized events and data-quality metadata
├── collectors/
│   ├── base.py               source adapter contract/capabilities
│   └── session_jsonl.py      rollout collector and reconciliation
├── migrations/
│   └── 001_initial.sql       SQLite schema
├── data/
│   └── pricing.json          versioned price catalog
├── pricing.py                provider-aware cost calculator
├── storage.py                WAL persistence and aggregates
├── tui.py                    dark-blue overview/models rendering
├── interactive.py            keyboard navigation and slash commands
├── doctor.py                 runtime/schema capability detection
└── cli.py                    command entry point
tests/                        automated reconciliation/privacy tests
```

## Commands

```text
codex-meter                  open the interactive keyboard dashboard
codex-meter import [PATH]    import one rollout or a sessions tree
codex-meter today            daily overview
codex-meter summary          day/week/month/all-time overview
codex-meter history          usage since first use grouped by day/week/month
codex-meter account ...      optional manual account labels (off by default)
codex-meter models           Model × Reasoning Effort totals
codex-meter sessions         recent session totals
codex-meter doctor           capability and schema detection
codex-meter pricing          versioned price list
codex-meter export           JSON, JSONL, or CSV per-call export
codex-meter demo             deterministic dark-blue UI demo
codex-meter watch            live-refresh terminal dashboard
codex-meter statusline       one compact footer/status line
codex-meter perf             OTLP AVG/P50/P95 and TBT-derived TPS
codex-meter cache            reuse, savings, amplification, retry tax
codex-meter projects         usage and compaction by project
codex-meter providers        provider attribution
codex-meter agents           root/subagent attribution
codex-meter tools            success and AVG/P50/P95 tool timing
codex-meter waterfall TURN   per-turn LLM/tool timeline
codex-meter otel ...         local OTLP collector and config helper
codex-meter app-server ...   JSONL ingestion or transparent stdio proxy
codex-meter network ...      probe, passive packet metadata, saved flows
codex-meter proxy ...        CONNECT, reverse, or explicit TLS diagnostics
```

Examples:

```bash
codex-meter export --from 2026-08-01 --to 2026-08-12 --format csv --output usage.csv
codex-meter export --session 019ff... --format jsonl
codex-meter models --date 2026-08-12
codex-meter summary --period day
codex-meter summary --period week --date 2026-08-12
codex-meter summary --period month
codex-meter summary --period all
codex-meter history --group day
codex-meter history --group week
codex-meter history --group month
```

Exports contain usage metadata only.

## Database

The base migration is in [`codex_meter/migrations/001_initial.sql`](codex_meter/migrations/001_initial.sql), with live observability tables in `002_live_observability.sql` and local owner/account metadata in `003_local_identity.sql`. Core tables:

- `sessions`: Codex thread, project, Git, auth-mode, source, OS owner and optional manual account label.
- `turns`: real Codex turns, model/effort, status, usage totals, TTFT and E2E.
- `llm_calls`: one row per observed upstream completion/delta; this is the core fact table.
- `tool_calls`: paired shell, patch, MCP, and web-search timing metadata.
- `pricing_snapshots`: effective-dated, provider/model-specific price data.
- `import_files`: incremental import cursor and data-quality counters.
- `metric_points` / `telemetry_logs`: filtered OTel metric and structural-event metadata.
- `compactions`: context compaction count by thread/turn.
- `network_flows`: content-free probe, capture and proxy timing/byte aggregates.

Every metric row stores `data_source`, `confidence`, and `estimated`. Unknown data stays `NULL` and is displayed as `N/A`.

## JSONL reconciliation rules

Codex 0.146.1 emits both `total_token_usage` (cumulative) and `last_token_usage`. Codex Meter:

1. calculates non-negative deltas from cumulative snapshots;
2. ignores unchanged repeated snapshots;
3. falls back to `last_token_usage` when an accumulator resets;
4. fingerprints normalized semantic snapshots so fork/replay timestamp rewrites do not double count;
5. prefers `raw_response_completed` exact per-response usage when present;
6. preserves every newly observed call instead of collapsing usage to one turn total.

This behavior is covered by automated tests.

## Pricing

Pricing is data-driven in [`codex_meter/data/pricing.json`](codex_meter/data/pricing.json). The bundled 2026-08-12 catalog covers GPT-5.6 Sol/Terra/Luna, GPT-5.5, and GPT-5.4 mini. Unknown/internal models remain unpriced instead of receiving an invented value.

The calculator separates regular input, cached reads, cache writes, and output. Reasoning tokens are part of output tokens and are not charged twice. For ChatGPT-login sessions, the UI labels the result `API-EQUIV`, never actual spend.

## Current capability matrix

| Metric | Source | Quality |
|---|---|---|
| Session, project, Git | rollout `session_meta` | exact |
| Turn, model, raw effort | `turn_context` / task events | exact |
| Input, cache read/write, output, reasoning | cumulative TokenCount delta | derived |
| Per-response usage | `raw_response_completed`, when recorded | exact |
| Turn TTFT / E2E | task completion fields | exact when present |
| Shell/patch/MCP/web timing | paired rollout events | exact/derived |
| TTFM, inference/overhead, TBT/TPS | OTLP JSON | exact/OTLP bucket approximation |
| Exact response usage/waterfall | App Server raw response + lifecycle | exact when enabled |
| Network/TLS setup | socket probe / passive metadata / local proxy | exact at selected layer |
| Prompt/response content | intentionally excluded | never stored |

The `doctor` command invokes Codex's own experimental schema generator to detect App Server `rawResponse/completed` support without starting a session or reading credentials.

## Real-time OTel

Print a matching Codex configuration, copy the snippet into `~/.codex/config.toml`, then start the collector before Codex:

```bash
codex-meter otel config
codex-meter otel serve
```

The generated config sets all three OTLP/HTTP exporters to JSON on localhost and keeps `log_user_prompt = false`. The collector accepts `/v1/logs`, `/v1/metrics`, and `/v1/traces`; it stores only a small attribute allowlist. OTLP bodies, prompt fields, event bodies, arbitrary span attributes, auth metadata, tool arguments and output are discarded.

For App Server clients, replace `codex app-server --stdio` with:

```bash
codex-meter app-server proxy
```

The proxy relays JSON-RPC and adds `experimentalRawEvents = true` to `thread/start` only when the client did not specify it, enabling exact per-upstream-response usage. It derives only thread/turn/response IDs, usage, lifecycle timings, tool type/status/duration, reroutes, and compactions. Prompts and other request fields stay in memory and are not persisted. An existing JSONL stream can be imported with `codex-meter app-server ingest FILE`.

## Network diagnostics and packet capture

The default modes do not decrypt TLS:

```bash
codex-meter network probe api.openai.com
codex-meter network capture --host api.openai.com --host chatgpt.com --duration 15
codex-meter proxy tunnel --port 8899
```

`network capture` runs tcpdump without `-A`, `-X`, or `-w` and records only resolved destination, packet direction/count/length, and elapsed time. Linux packet capture permissions are still enforced by the OS. `proxy tunnel` is an HTTP CONNECT tunnel and keeps TLS opaque.

The reverse proxy is useful with Codex's current ChatGPT/WebSocket transport:

```bash
codex-meter proxy reverse --port 8900 --upstream https://chatgpt.com/backend-api/codex
codex -c 'openai_base_url="http://127.0.0.1:8900"'
```

It supports normal HTTP/SSE plus WebSocket Upgrade. HTTP bodies and WebSocket frames pass through memory only; only status/timing and byte counts are persisted. It also respects the machine's configured upstream proxy.

TLS termination/re-encryption is a separate, explicit diagnostic mode:

```bash
codex-meter proxy tls-init
codex-meter proxy tls --acknowledge-sensitive \
  --upstream https://chatgpt.com/backend-api/codex
```

This creates a 30-day local CA and localhost certificate under `~/.codex-meter/tls/`; private keys use mode `0600`. Trust only the CA certificate for the diagnostic window and remove that trust afterward. Even in this mode Codex Meter never persists headers, request/response bodies, SSE data, or WebSocket frames.

The exact 0.146.1 source/schema audit is recorded in [`docs/codex-0.146.1-schema.md`](docs/codex-0.146.1-schema.md). Official Codex OTel configuration and metric names are documented in [OpenAI's Advanced Configuration guide](https://developers.openai.com/codex/config-advanced/#observability-and-telemetry), and bundled prices are sourced from the [official model catalog](https://developers.openai.com/api/docs/models).

## Tests

```bash
python3 -m unittest discover -v
```

Tests cover cumulative-event duplication, fork/replay reconciliation, exact raw-response precedence, daily/weekly/monthly period boundaries, OS-user isolation, opt-in account labels, OTLP parsing/HTTP ingestion, App Server lifecycle usage, cache pricing, latency percentiles, tcpdump metadata parsing, CONNECT tunnels, HTTP/SSE and WebSocket reverse proxying, TLS termination/re-encryption, idempotent storage, privacy, and `N/A` rendering.

## Remaining work toward 1.0

- Add tested, native installation and terminal support for macOS and Windows.
- External `watch` and `statusline` are available now. A version-pinned upstream TUI patch for native `/meter` is included in [`integrations/`](integrations/); it has passed Rust compile, Clippy, command-order and snapshot tests but is not applied to the installed Codex binary automatically.
- Provider-specific deep timing adapters beyond OpenAI/ChatGPT remain future work; generic provider and root/subagent attribution are available now.
- Transparent OS-wide packet interception is intentionally not attempted; capture permissions and explicit proxy configuration remain visible to the user.

OTel and experimental App Server APIs remain behind adapters and capability detection. Missing metrics render as `Unknown`/`N/A`.
