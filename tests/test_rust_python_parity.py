from __future__ import annotations

import csv
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from .helpers import base_events, complete_event, token_event, write_rollout


class RustPythonParityTests(unittest.TestCase):
    """Frozen cross-implementation acceptance fixture.

    Stable Python-only jobs skip this test. Rust CI sets CODEX_METER_RUST_BIN
    after building the candidate executable.
    """

    @unittest.skipUnless(os.environ.get("CODEX_METER_RUST_BIN"), "Rust candidate not built")
    def test_rollout_database_and_exports_match_python(self) -> None:
        binary = Path(os.environ["CODEX_METER_RUST_BIN"]).resolve()
        self.assertTrue(binary.is_file(), binary)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sessions = root / "sessions"
            sessions.mkdir()
            events = base_events() + [
                token_event("2026-08-12T00:00:04Z", (100, 60, 0, 10, 4, 110)),
                token_event("2026-08-12T00:00:05Z", (100, 60, 0, 10, 4, 110)),
                token_event(
                    "2026-08-12T00:00:06Z",
                    (250, 160, 10, 30, 12, 280),
                    (150, 100, 10, 20, 8, 170),
                ),
                complete_event(),
            ]
            write_rollout(sessions / "rollout-parity.jsonl", events)
            python_home = root / "python-home"
            rust_home = root / "rust-home"

            self._run(
                [sys.executable, "-m", "codex_meter", "--home", str(python_home), "import", str(sessions)]
            )
            self._run([str(binary), "--home", str(rust_home), "import", str(sessions)])

            queries = {
                "counts": """SELECT
                    (SELECT COUNT(*) FROM sessions),
                    (SELECT COUNT(*) FROM turns),
                    (SELECT COUNT(*) FROM llm_calls),
                    (SELECT COUNT(*) FROM tool_calls),
                    (SELECT COUNT(*) FROM import_files)""",
                "sessions": """SELECT codex_thread_id,started_at,ended_at,cwd,project_name,
                    git_repo,git_branch,auth_mode,codex_version,provider,parent_thread_id,
                    agent_role,agent_id FROM sessions ORDER BY codex_thread_id""",
                "turns": """SELECT codex_turn_id,started_at,completed_at,status,model,
                    reasoning_effort,reasoning_mode,service_tier,input_tokens,cached_input_tokens,
                    cache_write_tokens,output_tokens,reasoning_tokens,total_tokens,ttft_ms,ttfm_ms,
                    e2e_ms,error_type,data_source,confidence,estimated FROM turns ORDER BY codex_turn_id""",
                "calls": """SELECT event_fingerprint,response_id,completed_at,model,actual_model,
                    provider,reasoning_effort,reasoning_mode,service_tier,input_tokens,
                    cached_input_tokens,cache_write_tokens,output_tokens,reasoning_tokens,
                    total_tokens,retry_index,success,error_type,cost_usd,pricing_version,
                    data_source,confidence,estimated FROM llm_calls ORDER BY event_fingerprint""",
                "tools": """SELECT source_call_id,tool_name,started_at,completed_at,duration_ms,
                    success,exit_code,data_source,confidence,estimated FROM tool_calls
                    ORDER BY source_call_id""",
            }
            left = sqlite3.connect(python_home / "meter.db")
            right = sqlite3.connect(rust_home / "meter.db")
            for name, query in queries.items():
                with self.subTest(query=name):
                    self.assertEqual(left.execute(query).fetchall(), right.execute(query).fetchall())

            for output_format in ("json", "csv"):
                python_output = self._run(
                    [sys.executable, "-m", "codex_meter", "--home", str(python_home), "export", "--format", output_format]
                ).stdout
                rust_output = self._run(
                    [str(binary), "--home", str(rust_home), "export", "--format", output_format]
                ).stdout
                if output_format == "json":
                    self.assertEqual(json.loads(python_output), json.loads(rust_output))
                else:
                    self.assertEqual(
                        list(csv.DictReader(python_output.splitlines())),
                        list(csv.DictReader(rust_output.splitlines())),
                    )

    @staticmethod
    def _run(command: list[str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run(command, check=True, text=True, capture_output=True)


if __name__ == "__main__":
    unittest.main()

