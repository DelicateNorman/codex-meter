from __future__ import annotations

import argparse
import hashlib
import json
import os
import sqlite3
import subprocess
from pathlib import Path

TABLES = (
    "sessions",
    "turns",
    "llm_calls",
    "tool_calls",
    "import_files",
    "metric_points",
    "telemetry_logs",
    "network_flows",
    "compactions",
)


def file_manifest(home: Path) -> dict[str, str]:
    manifest: dict[str, str] = {}
    for path in sorted(home.rglob("*")):
        if path.is_file():
            manifest[path.relative_to(home).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return manifest


def database_snapshot(home: Path) -> dict[str, object]:
    database = (home / "meter.db").resolve()
    connection = sqlite3.connect(f"file:{database.as_posix()}?mode=ro", uri=True)
    try:
        integrity = connection.execute("PRAGMA integrity_check").fetchone()[0]
        available = {
            row[0]
            for row in connection.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            )
        }
        counts = {
            table: connection.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()[0]
            for table in TABLES
            if table in available
        }
        usage = connection.execute(
            """SELECT COUNT(*), COALESCE(SUM(input_tokens), 0),
                      COALESCE(SUM(cached_input_tokens), 0),
                      COALESCE(SUM(output_tokens), 0),
                      COALESCE(SUM(reasoning_tokens), 0),
                      COALESCE(SUM(total_tokens), 0)
               FROM llm_calls"""
        ).fetchone()
        return {"integrity": integrity, "counts": counts, "usage": list(usage)}
    finally:
        connection.close()


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def verify(path: Path, actual: object) -> None:
    expected = json.loads(path.read_text(encoding="utf-8"))
    if actual != expected:
        raise SystemExit(
            "release history guard mismatch\n"
            f"expected={json.dumps(expected, sort_keys=True)}\n"
            f"actual={json.dumps(actual, sort_keys=True)}"
        )


def seed(binary: Path, home: Path, sessions: Path) -> None:
    home.mkdir(parents=True, exist_ok=True)
    sessions.mkdir(parents=True, exist_ok=True)
    usage = {
        "input_tokens": 100,
        "cached_input_tokens": 60,
        "cache_write_input_tokens": 0,
        "output_tokens": 10,
        "reasoning_output_tokens": 4,
        "total_tokens": 110,
    }
    events = [
        {
            "timestamp": "2026-08-12T00:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": "release-guard-thread",
                "cwd": "/work/release-guard",
                "cli_version": "0.146.1",
                "model_provider": "openai",
            },
        },
        {
            "timestamp": "2026-08-12T00:00:01Z",
            "type": "turn_context",
            "payload": {
                "turn_id": "release-guard-turn",
                "model": "gpt-5.6-sol",
                "effort": "high",
            },
        },
        {
            "timestamp": "2026-08-12T00:00:02Z",
            "type": "event_msg",
            "payload": {
                "type": "task_started",
                "turn_id": "release-guard-turn",
                "started_at": 1786492802,
            },
        },
        {
            "timestamp": "2026-08-12T00:00:04Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": usage,
                    "last_token_usage": usage,
                    "model_context_window": 258400,
                },
                "rate_limits": {"plan_type": "pro"},
            },
        },
        {
            "timestamp": "2026-08-12T00:00:10Z",
            "type": "event_msg",
            "payload": {
                "type": "task_complete",
                "turn_id": "release-guard-turn",
                "duration_ms": 8000,
                "time_to_first_token_ms": 2200,
            },
        },
    ]
    rollout = sessions / "rollout-release-guard.jsonl"
    rollout.write_text(
        "\n".join(json.dumps(event) for event in events) + "\n",
        encoding="utf-8",
    )
    environment = os.environ.copy()
    environment["NO_COLOR"] = "1"
    subprocess.run(
        [str(binary.resolve()), "--home", str(home), "import", str(sessions)],
        check=True,
        env=environment,
        stdout=subprocess.DEVNULL,
    )
    (home / "install-preservation-canary.txt").write_text(
        "CODEX_METER_HISTORY_MUST_SURVIVE\n", encoding="utf-8"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    seed_parser = subparsers.add_parser("seed")
    seed_parser.add_argument("--binary", type=Path, required=True)
    seed_parser.add_argument("--home", type=Path, required=True)
    seed_parser.add_argument("--sessions", type=Path, required=True)

    for name in ("manifest", "database"):
        command = subparsers.add_parser(name)
        command.add_argument("--home", type=Path, required=True)
        command.add_argument("--output", type=Path)
        command.add_argument("--expect", type=Path)

    args = parser.parse_args()
    if args.command == "seed":
        seed(args.binary, args.home, args.sessions)
        return
    value = file_manifest(args.home) if args.command == "manifest" else database_snapshot(args.home)
    if args.expect:
        verify(args.expect, value)
    if args.output:
        write_json(args.output, value)
    elif not args.expect:
        print(json.dumps(value, sort_keys=True))


if __name__ == "__main__":
    main()
