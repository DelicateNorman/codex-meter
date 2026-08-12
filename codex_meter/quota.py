"""Read account-wide Codex rate-limit windows without accessing credentials."""

from __future__ import annotations

import json
import queue
import subprocess
import threading
import time
from dataclasses import dataclass
from typing import Mapping, Sequence

from . import __version__


WEEK_MINUTES = 7 * 24 * 60


class QuotaUnavailable(RuntimeError):
    """Raised when Codex cannot provide its current rate-limit snapshot."""


@dataclass(frozen=True, slots=True)
class WeeklyQuota:
    limit_id: str
    name: str
    used_percent: int
    resets_at: int | None
    window_minutes: int
    plan_type: str | None = None

    @property
    def remaining_percent(self) -> int:
        return max(0, 100 - self.used_percent)


def read_weekly_quotas(
    command: Sequence[str] = ("codex", "app-server", "--stdio"),
    *,
    timeout: float = 8.0,
) -> tuple[WeeklyQuota, ...]:
    """Ask the installed Codex App Server for current account quota metadata."""

    reader: threading.Thread | None = None
    try:
        process = subprocess.Popen(
            list(command),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
    except OSError as error:
        raise QuotaUnavailable(f"could not start Codex App Server: {error}") from error

    try:
        if process.stdin is None or process.stdout is None:
            raise QuotaUnavailable("Codex App Server did not open its stdio transport")
        requests = (
            {
                "id": 1,
                "method": "initialize",
                "params": {
                    "clientInfo": {"name": "codex-meter", "version": __version__},
                    "capabilities": {"experimentalApi": True},
                },
            },
            {"method": "initialized"},
            {"id": 2, "method": "account/rateLimits/read", "params": None},
        )
        for request in requests:
            process.stdin.write(
                (json.dumps(request, separators=(",", ":")) + "\n").encode()
            )
        process.stdin.flush()

        responses: queue.Queue[bytes | None] = queue.Queue()

        def read_responses() -> None:
            assert process.stdout is not None
            try:
                for line in iter(process.stdout.readline, b""):
                    responses.put(line)
            finally:
                responses.put(None)

        reader = threading.Thread(
            target=read_responses,
            name="codex-meter-quota-reader",
            daemon=True,
        )
        reader.start()
        deadline = time.monotonic() + max(0.1, timeout)
        while time.monotonic() < deadline:
            remaining = max(0.0, deadline - time.monotonic())
            try:
                line = responses.get(timeout=remaining)
            except queue.Empty:
                break
            if line is None:
                break
            try:
                message = json.loads(line)
            except (json.JSONDecodeError, UnicodeDecodeError):
                continue
            if not isinstance(message, dict) or message.get("id") != 2:
                continue
            if message.get("error") is not None:
                raise QuotaUnavailable("Codex App Server rejected the rate-limit request")
            result = message.get("result")
            if not isinstance(result, dict):
                raise QuotaUnavailable("Codex App Server returned an invalid rate-limit response")
            return extract_weekly_quotas(result)
        raise QuotaUnavailable("timed out waiting for Codex rate limits")
    except (OSError, ValueError) as error:
        raise QuotaUnavailable(f"could not read Codex rate limits: {error}") from error
    finally:
        if process.stdin is not None:
            try:
                process.stdin.close()
            except OSError:
                pass
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=1)
        if reader is not None:
            reader.join(timeout=1)
        if process.stdout is not None:
            process.stdout.close()


def extract_weekly_quotas(result: Mapping[str, object]) -> tuple[WeeklyQuota, ...]:
    """Extract seven-day windows from a rateLimits/read result."""

    snapshots: list[Mapping[str, object]] = []
    by_limit = result.get("rateLimitsByLimitId")
    if isinstance(by_limit, dict):
        snapshots.extend(value for value in by_limit.values() if isinstance(value, dict))
    legacy = result.get("rateLimits")
    if isinstance(legacy, dict):
        legacy_id = str(legacy.get("limitId") or "codex")
        if not any(str(item.get("limitId") or "codex") == legacy_id for item in snapshots):
            snapshots.append(legacy)

    quotas: list[WeeklyQuota] = []
    seen: set[str] = set()
    for snapshot in snapshots:
        limit_id = str(snapshot.get("limitId") or "codex")
        if limit_id in seen:
            continue
        windows = [
            value for key in ("primary", "secondary")
            if isinstance((value := snapshot.get(key)), dict)
        ]
        weekly = [
            window for window in windows
            if _is_week_window(window.get("windowDurationMins"))
        ]
        if not weekly:
            continue
        window = min(
            weekly,
            key=lambda item: abs(_integer(item.get("windowDurationMins")) - WEEK_MINUTES),
        )
        used = max(0, min(100, _integer(window.get("usedPercent"))))
        supplied_name = snapshot.get("limitName")
        name = str(supplied_name).strip() if supplied_name else (
            "Codex" if limit_id == "codex" else limit_id
        )
        quotas.append(WeeklyQuota(
            limit_id=limit_id,
            name=name,
            used_percent=used,
            resets_at=_optional_integer(window.get("resetsAt")),
            window_minutes=_integer(window.get("windowDurationMins")),
            plan_type=str(snapshot["planType"]) if snapshot.get("planType") else None,
        ))
        seen.add(limit_id)
    return tuple(sorted(quotas, key=lambda quota: (quota.limit_id != "codex", quota.name.casefold())))


def _is_week_window(value: object) -> bool:
    minutes = _integer(value)
    return WEEK_MINUTES - 12 * 60 <= minutes <= WEEK_MINUTES + 12 * 60


def _integer(value: object) -> int:
    try:
        return int(value or 0)
    except (TypeError, ValueError):
        return 0


def _optional_integer(value: object) -> int | None:
    if value is None:
        return None
    parsed = _integer(value)
    return parsed or None
