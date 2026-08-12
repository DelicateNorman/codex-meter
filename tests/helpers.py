from __future__ import annotations

import json
from pathlib import Path
from typing import Any


def write_rollout(path: Path, events: list[dict[str, Any]]) -> Path:
    path.write_text("\n".join(json.dumps(event) for event in events) + "\n", encoding="utf-8")
    return path


def base_events() -> list[dict[str, Any]]:
    return [
        {
            "timestamp": "2026-08-12T00:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": "thread-1",
                "session_id": "thread-1",
                "timestamp": "2026-08-12T00:00:00Z",
                "cwd": "/work/project",
                "cli_version": "0.146.1",
                "model_provider": "openai",
                "git": {"branch": "main", "repository_url": "https://example.invalid/project.git"},
            },
        },
        {
            "timestamp": "2026-08-12T00:00:01Z",
            "type": "turn_context",
            "payload": {
                "turn_id": "turn-1",
                "cwd": "/work/project",
                "model": "gpt-5.6-sol",
                "effort": "high",
                "summary": "auto",
            },
        },
        {
            "timestamp": "2026-08-12T00:00:02Z",
            "type": "event_msg",
            "payload": {"type": "task_started", "turn_id": "turn-1", "started_at": 1786492802},
        },
    ]


def token_event(
    timestamp: str,
    total: tuple[int, int, int, int, int, int],
    last: tuple[int, int, int, int, int, int] | None = None,
) -> dict[str, Any]:
    last = last or total

    def usage(values: tuple[int, int, int, int, int, int]) -> dict[str, int]:
        return {
            "input_tokens": values[0],
            "cached_input_tokens": values[1],
            "cache_write_input_tokens": values[2],
            "output_tokens": values[3],
            "reasoning_output_tokens": values[4],
            "total_tokens": values[5],
        }

    return {
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": usage(total),
                "last_token_usage": usage(last),
                "model_context_window": 258400,
            },
            "rate_limits": {"plan_type": "pro"},
        },
    }


def complete_event(timestamp: str = "2026-08-12T00:00:10Z") -> dict[str, Any]:
    return {
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "task_complete",
            "turn_id": "turn-1",
            "duration_ms": 8000,
            "time_to_first_token_ms": 2200,
        },
    }
