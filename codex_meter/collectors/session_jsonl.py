"""Streaming parser for Codex rollout JSONL files.

The collector intentionally never reads message content, reasoning content, tool
arguments, shell commands, stdout, stderr, headers, or credentials.
"""

from __future__ import annotations

import hashlib
import json
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from codex_meter.models import (
    Confidence,
    LlmCallRecord,
    ParsedSession,
    Quality,
    SessionRecord,
    TokenUsage,
    ToolCallRecord,
    TurnRecord,
)
from codex_meter.pricing import PricingCatalog

from .base import Collector, CollectorCapabilities


_TOOL_BEGIN = {
    "exec_command_begin": "shell",
    "patch_apply_begin": "apply_patch",
    "mcp_tool_call_begin": "mcp",
    "web_search_begin": "web_search",
}
_TOOL_END = {
    "exec_command_end": "shell",
    "patch_apply_end": "apply_patch",
    "mcp_tool_call_end": "mcp",
    "web_search_end": "web_search",
}


class SessionJsonlCollector(Collector):
    def __init__(self, pricing: PricingCatalog | None = None) -> None:
        self.pricing = pricing or PricingCatalog.bundled()

    @property
    def name(self) -> str:
        return "session_jsonl"

    @property
    def capabilities(self) -> CollectorCapabilities:
        return CollectorCapabilities(
            sessions=True,
            turns=True,
            per_llm_call=True,
            token_usage=True,
            cache_write=True,
            latency=True,
            tools=True,
            exact_usage=False,
        )

    def collect_file(self, path: Path) -> ParsedSession:
        meta: dict[str, Any] | None = None
        started_at: str | None = None
        ended_at: str | None = None
        turns: dict[str, TurnRecord] = {}
        calls: list[LlmCallRecord] = []
        completed_tools: list[ToolCallRecord] = []
        open_tools: dict[str, tuple[str, str | None, str | None]] = {}
        active_turn: str | None = None
        latest_context: dict[str, Any] = {}
        previous_total = TokenUsage()
        have_total = False
        malformed = 0
        duplicate_usage = 0
        capabilities: set[str] = {"session_jsonl"}
        auth_mode = "unknown"
        exact_usage_waiting: Counter[tuple[str | None, TokenUsage]] = Counter()

        with path.open("r", encoding="utf-8", errors="replace") as handle:
            for line_number, raw_line in enumerate(handle, 1):
                try:
                    line = json.loads(raw_line)
                except (json.JSONDecodeError, UnicodeDecodeError):
                    malformed += 1
                    continue
                timestamp = _string(line.get("timestamp"))
                started_at = started_at or timestamp
                ended_at = timestamp or ended_at
                line_type = line.get("type")
                payload = line.get("payload") if isinstance(line.get("payload"), dict) else {}
                event_type = payload.get("type")

                if line_type == "session_meta" and meta is None:
                    meta = payload
                    continue

                if line_type == "turn_context":
                    turn_id = _string(payload.get("turn_id")) or active_turn
                    if turn_id:
                        active_turn = turn_id
                        latest_context = payload
                        turn = turns.setdefault(turn_id, TurnRecord(codex_turn_id=turn_id))
                        _apply_context(turn, payload)
                    continue

                if line_type != "event_msg":
                    continue

                if event_type in ("task_started", "turn_started"):
                    turn_id = _string(payload.get("turn_id"))
                    if not turn_id:
                        continue
                    active_turn = turn_id
                    turn = turns.setdefault(turn_id, TurnRecord(codex_turn_id=turn_id))
                    turn.started_at = timestamp or _epoch_to_iso(payload.get("started_at"))
                    turn.status = "running"
                    if latest_context.get("turn_id") == turn_id:
                        _apply_context(turn, latest_context)
                    capabilities.add("turn_timings")
                    continue

                if event_type in ("task_complete", "turn_complete", "turn_aborted"):
                    turn_id = _string(payload.get("turn_id")) or active_turn
                    if not turn_id:
                        continue
                    turn = turns.setdefault(turn_id, TurnRecord(codex_turn_id=turn_id))
                    turn.completed_at = timestamp or _epoch_to_iso(payload.get("completed_at"))
                    error = payload.get("error")
                    turn.status = "aborted" if event_type == "turn_aborted" else ("failed" if error else "completed")
                    turn.error_type = _error_type(error)
                    turn.e2e_ms = _nonnegative_int(payload.get("duration_ms"))
                    turn.ttft_ms = _nonnegative_int(payload.get("time_to_first_token_ms"))
                    active_turn = None if active_turn == turn_id else active_turn
                    continue

                if event_type == "thread_settings_applied":
                    settings = payload.get("thread_settings") or {}
                    if active_turn and active_turn in turns:
                        _apply_settings(turns[active_turn], settings)
                    continue

                if event_type == "context_compacted":
                    capabilities.add("compaction")
                    continue

                if event_type in ("raw_response_completed", "rawResponse/completed"):
                    usage = TokenUsage.from_mapping(payload.get("token_usage") or payload.get("usage"))
                    if usage.is_zero():
                        continue
                    turn_id = _string(payload.get("turn_id")) or active_turn
                    context = _context_for(turns, turn_id, latest_context)
                    response_id = _string(payload.get("response_id"))
                    call = self._make_call(
                        path,
                        line_number,
                        timestamp,
                        turn_id,
                        response_id,
                        usage,
                        context,
                        exact=True,
                        fingerprint_basis=response_id,
                    )
                    calls.append(call)
                    exact_usage_waiting[(turn_id, usage)] += 1
                    capabilities.add("exact_per_response_usage")
                    continue

                if event_type == "token_count":
                    info = payload.get("info")
                    if not isinstance(info, dict):
                        continue
                    rate_limits = payload.get("rate_limits") or {}
                    if rate_limits.get("plan_type"):
                        auth_mode = "chatgpt"
                    total = TokenUsage.from_mapping(info.get("total_token_usage"))
                    last = TokenUsage.from_mapping(info.get("last_token_usage"))
                    if have_total:
                        usage = total.delta(previous_total)
                        if usage is None:
                            # Accumulator reset/replay: last is safer than a negative delta.
                            usage = last
                    else:
                        usage = total if not total.is_zero() else last
                    if total != previous_total:
                        previous_total = total
                        have_total = True
                    if usage is None or usage.is_zero():
                        duplicate_usage += 1
                        continue
                    context = _context_for(turns, active_turn, latest_context)
                    key = (active_turn, usage)
                    if exact_usage_waiting[key]:
                        exact_usage_waiting[key] -= 1
                        duplicate_usage += 1
                        continue
                    calls.append(
                        self._make_call(
                            path,
                            line_number,
                            timestamp,
                            active_turn,
                            None,
                            usage,
                            context,
                            exact=False,
                            # Forks rewrite rollout timestamps while preserving event payloads.
                            # The cumulative + last snapshot is therefore the stable identity.
                            fingerprint_basis=_usage_identity(active_turn, total, last),
                        )
                    )
                    capabilities.add("token_usage")
                    if usage.cache_write_tokens:
                        capabilities.add("cache_write")
                    continue

                if event_type in _TOOL_BEGIN:
                    call_id = _string(payload.get("call_id") or payload.get("id"))
                    if call_id:
                        tool_turn = _string(payload.get("turn_id")) or active_turn
                        open_tools[call_id] = (_tool_name(event_type, payload), tool_turn, _ms_or_iso(payload.get("started_at_ms"), timestamp))
                    continue

                if event_type in _TOOL_END:
                    call_id = _string(payload.get("call_id") or payload.get("id"))
                    if not call_id:
                        continue
                    fallback = (_tool_name(event_type, payload), _string(payload.get("turn_id")) or active_turn, None)
                    tool_name, tool_turn, tool_started = open_tools.pop(call_id, fallback)
                    tool_completed = _ms_or_iso(payload.get("completed_at_ms"), timestamp)
                    duration = _duration_ms(payload.get("duration")) or _time_delta_ms(tool_started, tool_completed)
                    completed_tools.append(
                        ToolCallRecord(
                            call_id=call_id,
                            turn_id=tool_turn,
                            tool_name=tool_name,
                            started_at=tool_started,
                            completed_at=tool_completed,
                            duration_ms=duration,
                            success=_tool_success(payload),
                            exit_code=_optional_int(payload.get("exit_code")),
                        )
                    )
                    capabilities.add("tool_calls")

        if meta is None:
            raise ValueError(f"{path}: no session_meta record")
        session_id = _string(meta.get("session_id") or meta.get("id"))
        if not session_id:
            raise ValueError(f"{path}: session_meta has no id")
        git = meta.get("git") if isinstance(meta.get("git"), dict) else {}
        cwd = _string(meta.get("cwd"))
        session = SessionRecord(
            codex_thread_id=session_id,
            started_at=_string(meta.get("timestamp")) or started_at,
            ended_at=ended_at,
            cwd=cwd,
            project_name=Path(cwd).name if cwd else None,
            git_repo=_string(git.get("repository_url")),
            git_branch=_string(git.get("branch")),
            auth_mode=auth_mode,
            codex_version=_string(meta.get("cli_version")),
            provider=_string(meta.get("model_provider")),
            source_path=str(path.resolve()),
            parent_thread_id=_string(meta.get("parent_thread_id") or meta.get("forked_from_id"))
            or _nested_string(meta, "source", "subagent", "parent_thread_id"),
            agent_role=_string(meta.get("agent_role")) or _nested_string(meta, "source", "subagent", "agent_role"),
            agent_id=_string(meta.get("agent_id")) or _nested_string(meta, "source", "subagent", "agent_id"),
        )
        return ParsedSession(
            session=session,
            turns=turns,
            llm_calls=calls,
            tool_calls=completed_tools,
            malformed_lines=malformed,
            duplicate_usage_events=duplicate_usage,
            capability_names=capabilities,
        )

    def _make_call(
        self,
        path: Path,
        line_number: int,
        timestamp: str | None,
        turn_id: str | None,
        response_id: str | None,
        usage: TokenUsage,
        context: dict[str, Any],
        *,
        exact: bool,
        fingerprint_basis: str | None,
    ) -> LlmCallRecord:
        model = _string(context.get("model"))
        provider = _string(context.get("provider")) or "openai"
        fingerprint_value = fingerprint_basis or response_id or "|".join(
            str(value)
            for value in (
                timestamp,
                turn_id,
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.cache_write_tokens,
                usage.output_tokens,
                usage.reasoning_tokens,
                usage.total_tokens,
            )
        )
        fingerprint = hashlib.sha256(fingerprint_value.encode("utf-8")).hexdigest()
        price = self.pricing.resolve(model, provider, timestamp)
        cost = self.pricing.calculate(usage, price) if price else None
        quality = Quality(
            "app_server_raw_response" if exact else "session_jsonl_cumulative_delta",
            Confidence.EXACT if exact else Confidence.DERIVED,
            estimated=not exact,
        )
        return LlmCallRecord(
            event_fingerprint=fingerprint,
            turn_id=turn_id,
            response_id=response_id,
            completed_at=timestamp,
            model=model,
            actual_model=None,
            provider=provider,
            reasoning_effort=_string(context.get("effort") or context.get("reasoning_effort")),
            reasoning_mode=_string(context.get("reasoning_mode")),
            service_tier=_string(context.get("service_tier")),
            usage=usage,
            cost_usd=cost.total_usd if cost else None,
            pricing_version=cost.pricing_version if cost else None,
            quality=quality,
        )


def discover_rollouts(root: Path) -> list[Path]:
    if root.is_file():
        return [root]
    return sorted(root.rglob("rollout-*.jsonl"))


def _context_for(turns: dict[str, TurnRecord], turn_id: str | None, latest: dict[str, Any]) -> dict[str, Any]:
    if turn_id and turn_id in turns:
        turn = turns[turn_id]
        return {
            "model": turn.model,
            "effort": turn.reasoning_effort,
            "reasoning_mode": turn.reasoning_mode,
            "service_tier": turn.service_tier,
            "provider": latest.get("provider"),
        }
    return latest


def _apply_context(turn: TurnRecord, payload: dict[str, Any]) -> None:
    turn.model = _string(payload.get("model")) or turn.model
    turn.reasoning_effort = _string(payload.get("effort")) or turn.reasoning_effort
    turn.reasoning_mode = _string(payload.get("reasoning_mode")) or turn.reasoning_mode


def _apply_settings(turn: TurnRecord, settings: dict[str, Any]) -> None:
    turn.model = _string(settings.get("model")) or turn.model
    turn.reasoning_effort = _string(settings.get("reasoning_effort")) or turn.reasoning_effort
    turn.reasoning_mode = _string(settings.get("reasoning_mode")) or turn.reasoning_mode
    turn.service_tier = _string(settings.get("service_tier")) or turn.service_tier


def _tool_name(event_type: str, payload: dict[str, Any]) -> str:
    base = _TOOL_BEGIN.get(event_type) or _TOOL_END.get(event_type) or event_type
    if base == "mcp":
        invocation = payload.get("invocation") if isinstance(payload.get("invocation"), dict) else {}
        server = _string(invocation.get("server"))
        tool = _string(invocation.get("tool"))
        return "/".join(part for part in ("mcp", server, tool) if part)
    return base


def _usage_identity(turn_id: str | None, total: TokenUsage, last: TokenUsage) -> str:
    def values(usage: TokenUsage) -> tuple[int, int, int, int, int, int]:
        return (
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.cache_write_tokens,
            usage.output_tokens,
            usage.reasoning_tokens,
            usage.total_tokens,
        )

    return json.dumps((turn_id, values(total), values(last)), separators=(",", ":"))


def _tool_success(payload: dict[str, Any]) -> bool | None:
    if isinstance(payload.get("success"), bool):
        return payload["success"]
    if payload.get("exit_code") is not None:
        return _optional_int(payload.get("exit_code")) == 0
    status = _string(payload.get("status"))
    if status:
        return status in ("completed", "success")
    result = payload.get("result")
    if isinstance(result, dict):
        return "Err" not in result and "error" not in result
    return None


def _duration_ms(value: Any) -> int | None:
    if isinstance(value, dict):
        seconds = float(value.get("secs", value.get("seconds", 0)) or 0)
        nanos = float(value.get("nanos", value.get("nanoseconds", 0)) or 0)
        return max(0, round(seconds * 1000 + nanos / 1_000_000))
    if isinstance(value, (int, float)):
        return max(0, round(float(value) * 1000))
    if isinstance(value, str):
        try:
            return max(0, round(float(value) * 1000))
        except ValueError:
            return None
    return None


def _time_delta_ms(start: str | None, end: str | None) -> int | None:
    if not start or not end:
        return None
    try:
        first = datetime.fromisoformat(start.replace("Z", "+00:00"))
        last = datetime.fromisoformat(end.replace("Z", "+00:00"))
    except ValueError:
        return None
    return max(0, round((last - first).total_seconds() * 1000))


def _epoch_to_iso(value: Any) -> str | None:
    try:
        return datetime.fromtimestamp(int(value), tz=timezone.utc).isoformat().replace("+00:00", "Z")
    except (TypeError, ValueError, OSError):
        return None


def _ms_or_iso(milliseconds: Any, fallback: str | None) -> str | None:
    try:
        value = int(milliseconds)
    except (TypeError, ValueError):
        return fallback
    if value <= 0:
        return fallback
    return datetime.fromtimestamp(value / 1000, tz=timezone.utc).isoformat().replace("+00:00", "Z")


def _error_type(value: Any) -> str | None:
    if not isinstance(value, dict):
        return None
    return _string(value.get("codex_error_info") or value.get("type") or value.get("code")) or "error"


def _nested_string(value: dict[str, Any], *path: str) -> str | None:
    current: Any = value
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    return _string(current)


def _string(value: Any) -> str | None:
    return value if isinstance(value, str) and value else None


def _nonnegative_int(value: Any) -> int | None:
    parsed = _optional_int(value)
    return max(0, parsed) if parsed is not None else None


def _optional_int(value: Any) -> int | None:
    try:
        return int(value) if value is not None else None
    except (TypeError, ValueError):
        return None
