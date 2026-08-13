#!/usr/bin/env python3
"""Repeatable release-binary performance smoke test.

The fixture contains metadata only. It measures first import, cached report
startup, incremental Rollout scanning, and peak resident memory on Linux.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import threading
import time


def rollout(index: int, calls: int) -> str:
    thread = f"perf-thread-{index}"
    turn = f"perf-turn-{index}"
    lines = [
        json.dumps({"timestamp": "2026-08-14T01:00:00Z", "type": "session_meta", "payload": {"id": thread, "cwd": f"/work/project-{index % 8}"}}, separators=(",", ":")),
        json.dumps({"timestamp": "2026-08-14T01:00:01Z", "type": "turn_context", "payload": {"turn_id": turn, "model": "gpt-5.6-sol", "effort": "medium"}}, separators=(",", ":")),
    ]
    for call in range(1, calls + 1):
        usage = {"input_tokens": call * 4_000, "cached_input_tokens": call * 3_600, "output_tokens": call * 80, "total_tokens": call * 4_080}
        lines.append(json.dumps({"timestamp": f"2026-08-14T01:{call:02d}:00Z", "type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": usage, "last_token_usage": usage}}}, separators=(",", ":")))
    return "\n".join(lines) + "\n"


def run_timed(command: list[str]) -> tuple[float, int]:
    started = time.perf_counter()
    process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    peak_kib = 0

    def sample() -> None:
        nonlocal peak_kib
        status = Path(f"/proc/{process.pid}/status")
        while process.poll() is None:
            try:
                for line in status.read_text(encoding="utf-8").splitlines():
                    if line.startswith("VmRSS:"):
                        peak_kib = max(peak_kib, int(line.split()[1]))
                        break
            except (FileNotFoundError, ProcessLookupError):
                pass
            time.sleep(0.002)

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    _, stderr = process.communicate()
    sampler.join(timeout=0.1)
    elapsed = time.perf_counter() - started
    if process.returncode:
        raise RuntimeError(f"{' '.join(command)} failed: {stderr.strip()}")
    return elapsed, peak_kib


def median_runs(command: list[str], count: int = 5) -> tuple[float, int]:
    samples = [run_timed(command) for _ in range(count)]
    return statistics.median(value[0] for value in samples), max(value[1] for value in samples)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--files", type=int, default=250)
    parser.add_argument("--calls", type=int, default=20)
    parser.add_argument("--enforce", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="codex-meter-perf-") as root_text:
        root = Path(root_text)
        sessions = root / "sessions"
        meter = root / "meter"
        sessions.mkdir()
        for index in range(args.files):
            (sessions / f"rollout-{index:04d}.jsonl").write_text(rollout(index, args.calls), encoding="utf-8")

        binary = str(args.binary.resolve())
        imported, import_rss = run_timed([binary, "--home", str(meter), "import", str(sessions)])
        summary, summary_rss = median_runs([binary, "--home", str(meter), "--no-color", "summary", "--period", "all"])
        incremental, incremental_rss = median_runs([binary, "--home", str(meter), "import", str(sessions)])
        result = {
            "fixture_files": args.files,
            "fixture_calls": args.files * args.calls,
            "cold_import_seconds": round(imported, 4),
            "cached_summary_p50_seconds": round(summary, 4),
            "incremental_scan_p50_seconds": round(incremental, 4),
            "peak_rss_mib": round(max(import_rss, summary_rss, incremental_rss) / 1024, 2),
        }
        rendered = json.dumps(result, indent=2, sort_keys=True)
        print(rendered)
        if args.output:
            args.output.write_text(rendered + "\n", encoding="utf-8")
        if args.enforce:
            budgets = {
                "cold_import_seconds": 10.0,
                "cached_summary_p50_seconds": 1.0,
                "incremental_scan_p50_seconds": 2.0,
                "peak_rss_mib": 192.0,
            }
            failures = [f"{name}={result[name]} exceeds {limit}" for name, limit in budgets.items() if result[name] > limit]
            if failures:
                raise SystemExit("Performance budget failed: " + "; ".join(failures))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
