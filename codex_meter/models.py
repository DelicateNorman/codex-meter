"""Normalized, source-independent meter event models.

No payload text is represented here by design. Collectors normalize only metadata,
usage, timings, and status into these records.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Iterator


class Confidence(str, Enum):
    EXACT = "exact"
    DERIVED = "derived"
    ESTIMATED = "estimated"
    UNKNOWN = "unknown"


class MeterEventKind(str, Enum):
    SESSION_UPSERT = "session_upsert"
    TURN_UPSERT = "turn_upsert"
    LLM_CALL_COMPLETED = "llm_call_completed"
    TOOL_CALL_COMPLETED = "tool_call_completed"
    METRIC_POINT = "metric_point"
    TELEMETRY_LOG = "telemetry_log"
    NETWORK_FLOW = "network_flow"


@dataclass(frozen=True, slots=True)
class Quality:
    source: str
    confidence: Confidence = Confidence.EXACT
    estimated: bool = False


@dataclass(frozen=True, slots=True)
class TokenUsage:
    input_tokens: int = 0
    cached_input_tokens: int = 0
    cache_write_tokens: int = 0
    output_tokens: int = 0
    reasoning_tokens: int = 0
    total_tokens: int = 0

    @classmethod
    def from_mapping(cls, value: dict[str, Any] | None) -> "TokenUsage":
        value = value or {}
        return cls(
            input_tokens=max(0, int(value.get("input_tokens", value.get("inputTokens", 0)) or 0)),
            cached_input_tokens=max(0, int(value.get("cached_input_tokens", value.get("cachedInputTokens", 0)) or 0)),
            cache_write_tokens=max(
                0,
                int(
                    value.get(
                        "cache_write_input_tokens",
                        value.get("cacheWriteInputTokens", value.get("cache_write_tokens", 0)),
                    )
                    or 0
                ),
            ),
            output_tokens=max(0, int(value.get("output_tokens", value.get("outputTokens", 0)) or 0)),
            reasoning_tokens=max(
                0,
                int(value.get("reasoning_output_tokens", value.get("reasoningOutputTokens", 0)) or 0),
            ),
            total_tokens=max(0, int(value.get("total_tokens", value.get("totalTokens", 0)) or 0)),
        )

    def delta(self, previous: "TokenUsage") -> "TokenUsage | None":
        values = (
            self.input_tokens - previous.input_tokens,
            self.cached_input_tokens - previous.cached_input_tokens,
            self.cache_write_tokens - previous.cache_write_tokens,
            self.output_tokens - previous.output_tokens,
            self.reasoning_tokens - previous.reasoning_tokens,
            self.total_tokens - previous.total_tokens,
        )
        if any(value < 0 for value in values):
            return None
        return TokenUsage(*values)

    @property
    def cache_miss_tokens(self) -> int:
        return max(0, self.input_tokens - self.cached_input_tokens - self.cache_write_tokens)

    @property
    def billable_regular_input_tokens(self) -> int:
        return self.cache_miss_tokens

    def is_zero(self) -> bool:
        return not any(
            (
                self.input_tokens,
                self.cached_input_tokens,
                self.cache_write_tokens,
                self.output_tokens,
                self.reasoning_tokens,
                self.total_tokens,
            )
        )


@dataclass(slots=True)
class SessionRecord:
    codex_thread_id: str
    started_at: str | None
    ended_at: str | None
    cwd: str | None
    project_name: str | None
    git_repo: str | None
    git_branch: str | None
    auth_mode: str
    codex_version: str | None
    provider: str | None
    source_path: str
    parent_thread_id: str | None = None
    agent_role: str | None = None
    agent_id: str | None = None


@dataclass(slots=True)
class TurnRecord:
    codex_turn_id: str
    started_at: str | None = None
    completed_at: str | None = None
    status: str = "running"
    model: str | None = None
    reasoning_effort: str | None = None
    reasoning_mode: str | None = None
    service_tier: str | None = None
    ttft_ms: int | None = None
    ttfm_ms: int | None = None
    e2e_ms: int | None = None
    error_type: str | None = None
    quality: Quality = field(default_factory=lambda: Quality("session_jsonl"))


@dataclass(slots=True)
class LlmCallRecord:
    event_fingerprint: str
    turn_id: str | None
    response_id: str | None
    completed_at: str | None
    model: str | None
    actual_model: str | None
    provider: str | None
    reasoning_effort: str | None
    reasoning_mode: str | None
    service_tier: str | None
    usage: TokenUsage
    success: bool = True
    error_type: str | None = None
    retry_index: int = 0
    cost_usd: float | None = None
    pricing_version: str | None = None
    quality: Quality = field(default_factory=lambda: Quality("session_jsonl", Confidence.DERIVED, True))


@dataclass(slots=True)
class ToolCallRecord:
    call_id: str
    turn_id: str | None
    tool_name: str
    started_at: str | None
    completed_at: str | None
    duration_ms: int | None
    success: bool | None
    exit_code: int | None
    quality: Quality = field(default_factory=lambda: Quality("session_jsonl"))


@dataclass(slots=True)
class MetricPointRecord:
    event_fingerprint: str
    observed_at: str | None
    name: str
    kind: str
    value: float | None = None
    point_sum: float | None = None
    point_count: int | None = None
    point_min: float | None = None
    point_max: float | None = None
    explicit_bounds: tuple[float, ...] = ()
    bucket_counts: tuple[int, ...] = ()
    attributes: dict[str, str] = field(default_factory=dict)
    thread_id: str | None = None
    turn_id: str | None = None
    response_id: str | None = None
    tool_name: str | None = None
    start_time_unix_nano: str | None = None
    time_unix_nano: str | None = None
    quality: Quality = field(default_factory=lambda: Quality("otlp_http"))


@dataclass(slots=True)
class TelemetryLogRecord:
    event_fingerprint: str
    observed_at: str | None
    event_name: str
    severity: str | None = None
    attributes: dict[str, str] = field(default_factory=dict)
    thread_id: str | None = None
    turn_id: str | None = None
    response_id: str | None = None
    item_id: str | None = None
    tool_name: str | None = None
    duration_ms: float | None = None
    status: str | None = None
    success: bool | None = None
    quality: Quality = field(default_factory=lambda: Quality("otlp_http"))


@dataclass(slots=True)
class NetworkFlowRecord:
    event_fingerprint: str
    mode: str
    data_source: str
    started_at: str | None = None
    ended_at: str | None = None
    destination_host: str | None = None
    destination_ip: str | None = None
    destination_port: int | None = None
    protocol: str | None = None
    tls_version: str | None = None
    alpn: str | None = None
    http_status: int | None = None
    request_bytes: int = 0
    response_bytes: int = 0
    packets_out: int = 0
    packets_in: int = 0
    dns_ms: float | None = None
    tcp_ms: float | None = None
    tls_ms: float | None = None
    ttfb_ms: float | None = None
    first_event_ms: float | None = None
    first_output_ms: float | None = None
    duration_ms: float | None = None
    success: bool | None = None
    error_type: str | None = None
    thread_id: str | None = None
    turn_id: str | None = None
    response_id: str | None = None
    quality: Quality = field(default_factory=lambda: Quality("network"))


MeterPayload = (
    SessionRecord
    | TurnRecord
    | LlmCallRecord
    | ToolCallRecord
    | MetricPointRecord
    | TelemetryLogRecord
    | NetworkFlowRecord
)


@dataclass(frozen=True, slots=True)
class MeterEvent:
    """Canonical event envelope emitted by every collector adapter."""

    kind: MeterEventKind
    payload: MeterPayload
    occurred_at: str | None
    quality: Quality


@dataclass(slots=True)
class ParsedSession:
    session: SessionRecord
    turns: dict[str, TurnRecord] = field(default_factory=dict)
    llm_calls: list[LlmCallRecord] = field(default_factory=list)
    tool_calls: list[ToolCallRecord] = field(default_factory=list)
    malformed_lines: int = 0
    duplicate_usage_events: int = 0
    capability_names: set[str] = field(default_factory=set)

    def events(self) -> Iterator[MeterEvent]:
        yield MeterEvent(
            MeterEventKind.SESSION_UPSERT,
            self.session,
            self.session.started_at,
            Quality("session_jsonl"),
        )
        for turn in self.turns.values():
            yield MeterEvent(MeterEventKind.TURN_UPSERT, turn, turn.completed_at or turn.started_at, turn.quality)
        for call in self.llm_calls:
            yield MeterEvent(MeterEventKind.LLM_CALL_COMPLETED, call, call.completed_at, call.quality)
        for tool in self.tool_calls:
            yield MeterEvent(MeterEventKind.TOOL_CALL_COMPLETED, tool, tool.completed_at, tool.quality)
