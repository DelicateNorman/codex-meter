"""Privacy-preserving adapter and transparent stdio proxy for Codex App Server."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import threading
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, BinaryIO, TextIO

from codex_meter.models import Confidence, LlmCallRecord, Quality, TokenUsage
from codex_meter.pricing import PricingCatalog
from codex_meter.storage import Storage


class AppServerAdapter:
    def __init__(self, storage: Storage, catalog: PricingCatalog | None = None) -> None:
        self.storage = storage
        self.catalog = catalog
        self.turn_started: dict[str, str] = {}
        self.item_started: dict[str, str] = {}
        self.turn_thread: dict[str, str] = {}
        self.turn_model: dict[str, str] = {}
        self.turn_effort: dict[str, str] = {}
        self.turn_first_event: dict[str, str] = {}
        self.turn_first_message: dict[str, str] = {}
        self.call_phase_started: dict[str, str] = {}
        self.pending_requests: dict[str, tuple[str, dict[str, Any]]] = {}
        self.thread_totals: dict[str, TokenUsage] = {}

    def ingest(self, message: dict[str, Any], *, direction: str = "server") -> int:
        """Ingest one JSON-RPC envelope. Payload text is never copied into a record."""
        observed_at = _event_time(message)
        method = str(message.get("method") or "")
        params = message.get("params") if isinstance(message.get("params"), dict) else {}
        if direction == "client" and method:
            request_id = message.get("id")
            if request_id is not None and method in ("thread/start", "thread/resume", "thread/fork", "turn/start"):
                self.pending_requests[str(request_id)] = (method, _safe_request_context(params))
            return 0
        if not method and "id" in message:
            self._handle_response(str(message["id"]), message.get("result"), observed_at)
            return 0
        if method == "thread/started":
            thread = params.get("thread") if isinstance(params.get("thread"), dict) else params
            thread_id = str(thread.get("id") or params.get("threadId") or "")
            if thread_id:
                self.storage.ensure_live_session(
                    thread_id,
                    started_at=_timestamp_value(thread.get("createdAt")) or observed_at,
                    cwd=_safe_path(thread.get("cwd")),
                    model=_safe_identifier(thread.get("model")),
                )
                return 1
        elif method == "turn/started":
            turn = params.get("turn") if isinstance(params.get("turn"), dict) else {}
            turn_id = str(turn.get("id") or params.get("turnId") or "")
            thread_id = str(params.get("threadId") or turn.get("threadId") or self.turn_thread.get(turn_id) or "")
            if turn_id and thread_id:
                self.turn_started[turn_id] = observed_at
                self.call_phase_started[turn_id] = observed_at
                self.turn_thread[turn_id] = thread_id
                self.storage.upsert_live_turn(
                    thread_id, turn_id, started_at=observed_at, status="running",
                    model=self.turn_model.get(turn_id), reasoning_effort=self.turn_effort.get(turn_id),
                )
                return 1
        elif method == "turn/completed":
            turn = params.get("turn") if isinstance(params.get("turn"), dict) else {}
            turn_id = str(turn.get("id") or params.get("turnId") or "")
            thread_id = str(params.get("threadId") or turn.get("threadId") or self.turn_thread.get(turn_id) or "")
            if turn_id and thread_id:
                self.storage.upsert_live_turn(
                    thread_id, turn_id, started_at=self.turn_started.get(turn_id),
                    completed_at=observed_at, status=_turn_status(turn.get("status")),
                    model=self.turn_model.get(turn_id), reasoning_effort=self.turn_effort.get(turn_id),
                    e2e_ms=_duration_ms(self.turn_started.get(turn_id), observed_at),
                )
                return 1
        elif method == "rawResponse/completed":
            return self._raw_response(params, observed_at)
        elif method == "thread/tokenUsage/updated":
            return self._token_usage(params)
        elif method in ("item/started", "item/completed"):
            return self._item(method, params, observed_at)
        elif method in ("thread/compacted", "compacted"):
            return self._compaction(params, method, observed_at)
        elif method == "model/rerouted":
            turn_id = str(params.get("turnId") or "")
            actual_model = _safe_identifier(params.get("toModel"))
            if turn_id and actual_model:
                self.storage.update_actual_model(turn_id, actual_model)
                return 1
        elif method == "item/agentMessage/delta":
            turn_id = str(params.get("turnId") or "")
            if turn_id and turn_id not in self.turn_first_message:
                self.turn_first_message[turn_id] = observed_at
                thread_id = str(params.get("threadId") or self.turn_thread.get(turn_id) or "")
                if thread_id:
                    self.storage.upsert_live_turn(
                        thread_id, turn_id,
                        ttfm_ms=_duration_ms(self.turn_started.get(turn_id), observed_at),
                    )
                return 1
        return 0

    def _handle_response(self, request_id: str, result: object, observed_at: str) -> None:
        pending = self.pending_requests.pop(request_id, None)
        if pending is None or not isinstance(result, dict):
            return
        method, context = pending
        if method.startswith("thread/"):
            thread = result.get("thread") if isinstance(result.get("thread"), dict) else result
            thread_id = str(thread.get("id") or "")
            if thread_id:
                self.storage.ensure_live_session(
                    thread_id, started_at=observed_at, cwd=context.get("cwd"), model=context.get("model"),
                )
        elif method == "turn/start":
            turn = result.get("turn") if isinstance(result.get("turn"), dict) else result
            turn_id = str(turn.get("id") or "")
            thread_id = str(context.get("thread_id") or "")
            if turn_id and thread_id:
                self.turn_thread[turn_id] = thread_id
                if context.get("model"):
                    self.turn_model[turn_id] = str(context["model"])
                if context.get("reasoning_effort"):
                    self.turn_effort[turn_id] = str(context["reasoning_effort"])
                self.storage.upsert_live_turn(
                    thread_id, turn_id, started_at=observed_at, model=self.turn_model.get(turn_id),
                    reasoning_effort=self.turn_effort.get(turn_id),
                )

    def _raw_response(self, params: dict[str, Any], observed_at: str) -> int:
        thread_id = str(params.get("threadId") or "")
        turn_id = str(params.get("turnId") or "")
        response_id = str(params.get("responseId") or "")
        usage_value = params.get("usage")
        if not thread_id or not response_id or not isinstance(usage_value, dict):
            return 0
        usage = TokenUsage.from_mapping(usage_value)
        model = self.turn_model.get(turn_id)
        price = self.catalog.resolve(model, "openai", observed_at) if self.catalog else None
        cost = self.catalog.calculate(usage, price) if self.catalog and price else None
        fingerprint = _fingerprint(("raw", thread_id, turn_id, response_id, usage_value))
        call = LlmCallRecord(
            event_fingerprint=fingerprint,
            turn_id=turn_id or None,
            response_id=response_id,
            completed_at=observed_at,
            model=model,
            actual_model=None,
            provider="openai",
            reasoning_effort=self.turn_effort.get(turn_id),
            reasoning_mode=None,
            service_tier=None,
            usage=usage,
            cost_usd=cost.total_usd if cost else None,
            pricing_version=cost.pricing_version if cost else None,
            quality=Quality("app_server", Confidence.EXACT, False),
        )
        first_event = self.turn_first_event.get(turn_id)
        first_message = self.turn_first_message.get(turn_id)
        call_started = self.call_phase_started.get(turn_id)
        inserted = int(
            self.storage.insert_live_call(
                thread_id, call, started_at=call_started,
                first_event_at=first_event, first_model_item_at=first_message,
                ttft_ms=_duration_ms(call_started, first_event),
                ttfm_ms=_duration_ms(call_started, first_message),
                request_duration_ms=_duration_ms(call_started, observed_at),
                transport="app_server",
            )
        )
        self.call_phase_started[turn_id] = observed_at
        self.turn_first_event.pop(turn_id, None)
        self.turn_first_message.pop(turn_id, None)
        return inserted

    def _token_usage(self, params: dict[str, Any]) -> int:
        thread_id = str(params.get("threadId") or "")
        turn_id = str(params.get("turnId") or "")
        token_usage = params.get("tokenUsage") if isinstance(params.get("tokenUsage"), dict) else {}
        total = TokenUsage.from_mapping(token_usage.get("total") if isinstance(token_usage.get("total"), dict) else None)
        if not thread_id or not turn_id or total.is_zero():
            return 0
        previous = self.thread_totals.get(thread_id, TokenUsage())
        delta = total.delta(previous)
        self.thread_totals[thread_id] = total
        if delta is None or delta.is_zero():
            return 0
        self.storage.update_live_turn_usage(thread_id, turn_id, delta)
        return 1

    def _item(self, method: str, params: dict[str, Any], observed_at: str) -> int:
        item = params.get("item") if isinstance(params.get("item"), dict) else {}
        item_id = str(item.get("id") or params.get("itemId") or "")
        turn_id = str(params.get("turnId") or item.get("turnId") or "")
        thread_id = str(params.get("threadId") or item.get("threadId") or self.turn_thread.get(turn_id) or "")
        if turn_id and turn_id not in self.turn_first_event:
            self.turn_first_event[turn_id] = observed_at
            if thread_id:
                self.storage.upsert_live_turn(
                    thread_id, turn_id,
                    ttft_ms=_duration_ms(self.turn_started.get(turn_id), observed_at),
                )
        item_type = str(item.get("type") or "")
        if item_type == "contextCompaction" and method == "item/completed" and thread_id:
            return self._compaction({"threadId": thread_id, "turnId": turn_id, "itemId": item_id}, item_type, observed_at)
        tool_name = _tool_name(item_type, item)
        if not tool_name or not item_id or not thread_id:
            return 0
        if method == "item/started":
            self.item_started[item_id] = observed_at
            self.storage.upsert_live_tool(
                thread_id, turn_id or None, item_id, tool_name,
                started_at=observed_at, completed_at=None, duration_ms=None, success=None,
            )
        else:
            status = str(item.get("status") or "")
            duration = _integer(item.get("durationMs")) or _duration_ms(self.item_started.get(item_id), observed_at)
            self.storage.upsert_live_tool(
                thread_id, turn_id or None, item_id, tool_name,
                started_at=self.item_started.get(item_id), completed_at=observed_at,
                duration_ms=duration, success=status in ("completed", "success"),
                exit_code=_integer(item.get("exitCode")),
            )
            if turn_id:
                self.call_phase_started[turn_id] = observed_at
        return 1

    def _compaction(self, params: dict[str, Any], method: str, observed_at: str) -> int:
        thread_id = str(params.get("threadId") or "")
        turn_id = str(params.get("turnId") or "") or None
        if not thread_id:
            return 0
        fingerprint = _fingerprint(("compaction", thread_id, turn_id, params.get("itemId") or method, observed_at))
        return int(self.storage.insert_compaction(fingerprint, thread_id, turn_id, observed_at, "app_server"))


def ingest_stream(stream: TextIO, adapter: AppServerAdapter, *, direction: str = "server") -> tuple[int, int]:
    ingested = malformed = 0
    for line in stream:
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            malformed += 1
            continue
        if isinstance(value, dict):
            ingested += adapter.ingest(value, direction=direction)
    return ingested, malformed


def proxy_stdio(storage: Storage, catalog: PricingCatalog, command: list[str] | None = None) -> int:
    command = command or ["codex", "app-server", "--stdio"]
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=None)
    assert process.stdin is not None and process.stdout is not None
    adapter = AppServerAdapter(storage, catalog)

    def client_to_server() -> None:
        _pump(sys.stdin.buffer, process.stdin, adapter, "client")
        try:
            process.stdin.close()
        except OSError:
            pass

    thread = threading.Thread(target=client_to_server, name="codex-meter-app-server-input", daemon=True)
    thread.start()
    _pump(process.stdout, sys.stdout.buffer, adapter, "server")
    thread.join(timeout=1)
    return process.wait()


def _pump(source: BinaryIO, target: BinaryIO, adapter: AppServerAdapter, direction: str) -> None:
    for line in iter(source.readline, b""):
        try:
            value = json.loads(line)
            if isinstance(value, dict):
                adapter.ingest(value, direction=direction)
                if direction == "client" and value.get("method") == "thread/start":
                    params = value.get("params")
                    if isinstance(params, dict) and "experimentalRawEvents" not in params:
                        # This is the one deliberate protocol augmentation made by the
                        # observability proxy. It enables exact per-upstream-response
                        # usage; prompts and other request fields remain memory-only.
                        value = dict(value)
                        value["params"] = dict(params)
                        value["params"]["experimentalRawEvents"] = True
                        line = (json.dumps(value, separators=(",", ":"), ensure_ascii=False) + "\n").encode()
        except (json.JSONDecodeError, UnicodeDecodeError):
            pass
        target.write(line)
        target.flush()


def _tool_name(item_type: str, item: dict[str, Any]) -> str | None:
    if item_type == "commandExecution":
        return "command"
    if item_type == "fileChange":
        return "apply_patch"
    if item_type == "mcpToolCall":
        server = _safe_identifier(item.get("server")) or "unknown"
        tool = _safe_identifier(item.get("tool")) or "unknown"
        return f"mcp:{server}:{tool}"[:256]
    if item_type == "collabToolCall":
        return f"collab:{_safe_identifier(item.get('tool')) or 'unknown'}"
    return {
        "webSearch": "web_search", "imageView": "view_image", "sleep": "sleep",
    }.get(item_type)


def _safe_request_context(params: dict[str, Any]) -> dict[str, Any]:
    settings = params.get("settings") if isinstance(params.get("settings"), dict) else {}
    return {
        "thread_id": _safe_identifier(params.get("threadId")),
        "cwd": _safe_path(params.get("cwd")),
        "model": _safe_identifier(params.get("model")),
        "reasoning_effort": _safe_identifier(params.get("effort") or settings.get("reasoningEffort")),
    }


def _safe_identifier(value: object) -> str | None:
    if not isinstance(value, str) or not value or len(value) > 256:
        return None
    return value if all(char.isalnum() or char in "._:/-@" for char in value) else None


def _safe_path(value: object) -> str | None:
    if not isinstance(value, str) or not value or len(value) > 4096 or "\x00" in value:
        return None
    return str(Path(value))


def _event_time(message: dict[str, Any]) -> str:
    emitted = message.get("emittedAtMs")
    try:
        if emitted is not None:
            return datetime.fromtimestamp(float(emitted) / 1000, timezone.utc).isoformat().replace("+00:00", "Z")
    except (TypeError, ValueError, OSError):
        pass
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _timestamp_value(value: object) -> str | None:
    try:
        if value is not None:
            numeric = float(value)
            if numeric > 10_000_000_000:
                numeric /= 1000
            return datetime.fromtimestamp(numeric, timezone.utc).isoformat().replace("+00:00", "Z")
    except (TypeError, ValueError, OSError):
        return None
    return None


def _duration_ms(start: str | None, end: str | None) -> int | None:
    if not start or not end:
        return None
    try:
        first = datetime.fromisoformat(start.replace("Z", "+00:00"))
        last = datetime.fromisoformat(end.replace("Z", "+00:00"))
        return max(0, round((last - first).total_seconds() * 1000))
    except ValueError:
        return None


def _turn_status(value: object) -> str:
    return {"inProgress": "running", "completed": "completed", "interrupted": "interrupted", "failed": "failed"}.get(str(value), str(value or "completed"))


def _integer(value: object) -> int | None:
    try:
        return int(value) if value is not None else None
    except (TypeError, ValueError):
        return None


def _fingerprint(value: object) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), default=str).encode()).hexdigest()
