# Codex 0.146.1 schema verification

This MVP was implemented against the locally installed `codex-cli 0.146.1` and the matching upstream tag `rust-v0.146.1`. It also generated the local experimental App Server JSON schema with:

```bash
codex app-server generate-json-schema --experimental --out <temporary-directory>
```

No request/prompt fields were inferred from names. The relevant verified sources are:

- `codex-rs/protocol/src/protocol.rs`: `TokenUsage`, `TokenUsageInfo`, `TokenCountEvent`, `TurnStartedEvent`, `TurnCompleteEvent`, `TurnContextItem`, `RawResponseCompletedEvent`, and rollout item tags.
- `codex-rs/app-server-protocol/src/protocol/v2/thread.rs`: `ThreadTokenUsageUpdatedNotification`, `RawResponseCompletedNotification`, and camelCase wire usage.
- `codex-rs/app-server/src/bespoke_event_handling.rs`: conversion from core token/raw-response events into App Server notifications.
- `codex-rs/rollout/src/recorder.rs`: first `session_meta` is canonical; later `session_meta` records may be copied fork history.
- `codex-rs/tui/src/token_usage.rs`: cumulative totals and last/context usage have different semantics.

## Verified rollout mappings

| Meter field | Codex rollout field |
|---|---|
| Thread ID | first `session_meta.payload.session_id` (legacy fallback: `id`) |
| Turn ID | `task_started.payload.turn_id` / `turn_context.payload.turn_id` |
| Model | `turn_context.payload.model` |
| Reasoning effort | `turn_context.payload.effort` |
| Service tier | `thread_settings_applied.payload.thread_settings.service_tier` |
| Cumulative usage | `token_count.payload.info.total_token_usage` |
| Latest usage | `token_count.payload.info.last_token_usage` |
| Cache write | `cache_write_input_tokens` |
| Turn TTFT | `task_complete.payload.time_to_first_token_ms` |
| Turn E2E | `task_complete.payload.duration_ms` |

The installed rollout corpus demonstrated both missing legacy zero-valued `cache_write_input_tokens` fields and newer explicit zero values. Fingerprinting normalizes both forms before fork/replay reconciliation.

## Verified App Server mappings

The generated v2 schema contains:

```text
thread/tokenUsage/updated
turn/started
turn/completed
rawResponse/completed   (experimental/internal)
```

`RawResponseCompletedNotification` retains `threadId`, `turnId`, `responseId`, and an optional one-completion `TokenUsageBreakdown`. The upstream source explicitly distinguishes this exact per-completion usage from accumulated/estimated/replayed `TokenCountEvent` data.

## Quality rules derived from the schema

- `rawResponse/completed` usage is `exact`.
- JSONL cumulative differences are `derived` and marked `estimated=true` at call level.
- Missing TTFM, generation timing, response IDs, transport, or TPS remain `NULL`.
- The first `session_meta` is canonical. Copied fork history is reconciled through normalized semantic usage fingerprints and existing turn ownership.

## Verified OTel and transport mappings

Codex 0.146.1 accepts OTLP/HTTP `binary` and `json`; Codex Meter deliberately uses `json` so the collector stays dependency-free and testable. It recognizes the official metric names for API duration, Responses API inference/overhead, engine IAPI/service TTFT/TBT, Turn TTFT/TTFM/E2E/token usage, and tool duration.

The current ChatGPT-login transport uses WebSocket Upgrade on the built-in OpenAI provider. A real end-to-end run through `openai_base_url=http://127.0.0.1:<port>` verified that the local reverse proxy forwards `/models` plus `/responses` WebSocket traffic. Frames remain opaque to storage; only status, timing, and byte counts are retained.
