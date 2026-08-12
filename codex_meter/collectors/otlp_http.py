"""Minimal, privacy-filtered OTLP/HTTP JSON collector for Codex telemetry."""

from __future__ import annotations

import gzip
import hashlib
import json
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Iterable

from codex_meter.models import MetricPointRecord, Quality, TelemetryLogRecord
from codex_meter.storage import Storage


MAX_BODY_BYTES = 8 * 1024 * 1024
SAFE_ATTRIBUTE_KEYS = {
    "event.name", "thread.id", "turn.id", "conversation.id", "response.id",
    "item.id", "call_id", "tool", "tool_name", "model", "actual_model",
    "codex.turn.reasoning_effort", "service_tier", "provider", "transport",
    "token_type", "status", "success", "attempt", "error.type",
    "http.response.status_code", "duration_ms", "endpoint", "mcp_server",
    "mcp_server_origin", "env", "originator", "session_source",
}


def parse_metrics(document: dict[str, Any]) -> list[MetricPointRecord]:
    output: list[MetricPointRecord] = []
    for resource in document.get("resourceMetrics", []):
        resource_attrs = _attributes(resource.get("resource", {}).get("attributes", []))
        for scope in resource.get("scopeMetrics", []):
            for metric in scope.get("metrics", []):
                name = str(metric.get("name") or "")[:256]
                if not name:
                    continue
                for kind in ("gauge", "sum", "histogram", "exponentialHistogram", "summary"):
                    container = metric.get(kind)
                    if not isinstance(container, dict):
                        continue
                    for point in container.get("dataPoints", []):
                        attrs = _safe_attributes(resource_attrs | _attributes(point.get("attributes", [])))
                        normalized = {
                            "name": name,
                            "kind": kind,
                            "time": str(point.get("timeUnixNano") or ""),
                            "start": str(point.get("startTimeUnixNano") or ""),
                            "attrs": attrs,
                            "value": _number(point.get("asDouble", point.get("asInt"))),
                            "sum": _number(point.get("sum")),
                            "count": _integer(point.get("count")),
                            "min": _number(point.get("min")),
                            "max": _number(point.get("max")),
                            "bounds": point.get("explicitBounds") or [],
                            "buckets": point.get("bucketCounts") or [],
                        }
                        output.append(
                            MetricPointRecord(
                                event_fingerprint=_fingerprint(normalized),
                                observed_at=_nanos_to_iso(normalized["time"]),
                                name=name,
                                kind=kind,
                                value=normalized["value"],
                                point_sum=normalized["sum"],
                                point_count=normalized["count"],
                                point_min=normalized["min"],
                                point_max=normalized["max"],
                                explicit_bounds=tuple(float(value) for value in normalized["bounds"]),
                                bucket_counts=tuple(int(value) for value in normalized["buckets"]),
                                attributes=attrs,
                                thread_id=_first(attrs, "thread.id", "conversation.id"),
                                turn_id=attrs.get("turn.id"),
                                response_id=attrs.get("response.id"),
                                tool_name=_first(attrs, "tool", "tool_name"),
                                start_time_unix_nano=normalized["start"] or None,
                                time_unix_nano=normalized["time"] or None,
                                quality=Quality("otlp_http"),
                            )
                        )
    return output


def parse_logs(document: dict[str, Any]) -> list[TelemetryLogRecord]:
    output: list[TelemetryLogRecord] = []
    for resource in document.get("resourceLogs", []):
        resource_attrs = _attributes(resource.get("resource", {}).get("attributes", []))
        for scope in resource.get("scopeLogs", []):
            for record in scope.get("logRecords", []):
                attrs = _safe_attributes(resource_attrs | _attributes(record.get("attributes", [])))
                event_name = attrs.get("event.name") or _safe_body_event_name(record.get("body"))
                if not event_name:
                    event_name = "otel.log"
                normalized = {
                    "name": event_name,
                    "time": str(record.get("timeUnixNano") or record.get("observedTimeUnixNano") or ""),
                    "severity": str(record.get("severityText") or ""),
                    "attrs": attrs,
                }
                output.append(_log_record(normalized))
    return output


def parse_traces(document: dict[str, Any]) -> list[TelemetryLogRecord]:
    output: list[TelemetryLogRecord] = []
    for resource in document.get("resourceSpans", []):
        resource_attrs = _attributes(resource.get("resource", {}).get("attributes", []))
        for scope in resource.get("scopeSpans", []):
            for span in scope.get("spans", []):
                attrs = _safe_attributes(resource_attrs | _attributes(span.get("attributes", [])))
                raw_name = str(span.get("name") or "")
                name = raw_name[:128] if _safe_span_name(raw_name) else "otel.span"
                start = _integer(span.get("startTimeUnixNano"))
                end = _integer(span.get("endTimeUnixNano"))
                if start is not None and end is not None and end >= start:
                    attrs["duration_ms"] = f"{(end - start) / 1_000_000:.6f}"
                normalized = {
                    "name": f"span:{name}",
                    "time": str(span.get("endTimeUnixNano") or ""),
                    "severity": "",
                    "attrs": attrs,
                }
                output.append(_log_record(normalized, source="otlp_trace"))
    return output


class OtlpServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, address: tuple[str, int], storage: Storage, token: str | None = None) -> None:
        self.storage = storage
        self.token = token
        super().__init__(address, OtlpRequestHandler)


class OtlpRequestHandler(BaseHTTPRequestHandler):
    server: OtlpServer

    def do_GET(self) -> None:  # noqa: N802
        if self.path in ("/healthz", "/readyz"):
            self._respond(200, {"status": "ok"})
        else:
            self._respond(404, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        if self.server.token and self.headers.get("Authorization") != f"Bearer {self.server.token}":
            self._respond(401, {"error": "unauthorized"})
            return
        content_type = self.headers.get("Content-Type", "").split(";", 1)[0].strip().lower()
        if content_type not in ("application/json", ""):
            self._respond(415, {"error": "configure Codex OTLP/HTTP with protocol = 'json'"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            self._respond(400, {"error": "invalid content length"})
            return
        if length < 0 or length > MAX_BODY_BYTES:
            self._respond(413, {"error": "payload too large"})
            return
        body = self.rfile.read(length)
        if self.headers.get("Content-Encoding", "").lower() == "gzip":
            try:
                body = gzip.decompress(body)
            except OSError:
                self._respond(400, {"error": "invalid gzip body"})
                return
        try:
            document = json.loads(body or b"{}")
        except json.JSONDecodeError:
            self._respond(400, {"error": "invalid OTLP JSON"})
            return
        if not isinstance(document, dict):
            self._respond(400, {"error": "OTLP root must be an object"})
            return
        if self.path == "/v1/metrics":
            self.server.storage.insert_metric_points(parse_metrics(document))
        elif self.path == "/v1/logs":
            self.server.storage.insert_telemetry_logs(parse_logs(document))
        elif self.path == "/v1/traces":
            self.server.storage.insert_telemetry_logs(parse_traces(document))
        else:
            self._respond(404, {"error": "not found"})
            return
        self._respond(200, {})

    def log_message(self, _format: str, *_args: object) -> None:
        # Deliberately avoid access logs: they can contain query strings or auth metadata.
        return

    def _respond(self, status: int, payload: dict[str, Any]) -> None:
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def _log_record(normalized: dict[str, Any], source: str = "otlp_http") -> TelemetryLogRecord:
    attrs = normalized["attrs"]
    success_value = attrs.get("success")
    success = None if success_value is None else success_value.lower() == "true"
    return TelemetryLogRecord(
        event_fingerprint=_fingerprint(normalized),
        observed_at=_nanos_to_iso(normalized["time"]),
        event_name=normalized["name"][:256],
        severity=normalized["severity"][:32] or None,
        attributes=attrs,
        thread_id=_first(attrs, "thread.id", "conversation.id"),
        turn_id=attrs.get("turn.id"),
        response_id=attrs.get("response.id"),
        item_id=_first(attrs, "item.id", "call_id"),
        tool_name=_first(attrs, "tool", "tool_name"),
        duration_ms=_number(attrs.get("duration_ms")),
        status=_first(attrs, "status", "http.response.status_code"),
        success=success,
        quality=Quality(source),
    )


def _attributes(values: Iterable[dict[str, Any]]) -> dict[str, str]:
    output: dict[str, str] = {}
    for item in values:
        key = str(item.get("key") or "")
        value = _any_value(item.get("value"))
        if key and value is not None:
            output[key] = value
    return output


def _safe_attributes(values: dict[str, str]) -> dict[str, str]:
    return {key: value[:256] for key, value in values.items() if key in SAFE_ATTRIBUTE_KEYS}


def _any_value(value: object) -> str | None:
    if not isinstance(value, dict):
        return None
    for key in ("stringValue", "intValue", "doubleValue", "boolValue"):
        if key in value:
            raw = value[key]
            return str(raw).lower() if isinstance(raw, bool) else str(raw)
    return None


def _safe_body_event_name(value: object) -> str | None:
    body = _any_value(value)
    return body if body and body.startswith("codex.") and len(body) <= 128 and " " not in body else None


def _safe_span_name(value: str) -> bool:
    return bool(value) and len(value) <= 128 and all(char.isalnum() or char in "._:/- " for char in value)


def _number(value: object) -> float | None:
    try:
        return float(value) if value is not None else None
    except (TypeError, ValueError):
        return None


def _integer(value: object) -> int | None:
    try:
        return int(value) if value is not None else None
    except (TypeError, ValueError):
        return None


def _nanos_to_iso(value: object) -> str | None:
    nanos = _integer(value)
    if nanos is None or nanos <= 0:
        return None
    return datetime.fromtimestamp(nanos / 1_000_000_000, timezone.utc).isoformat().replace("+00:00", "Z")


def _fingerprint(value: object) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _first(values: dict[str, str], *keys: str) -> str | None:
    return next((values[key] for key in keys if values.get(key)), None)
