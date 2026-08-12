from __future__ import annotations

import sqlite3
import tempfile
import unittest
from pathlib import Path

from codex_meter.collectors.session_jsonl import SessionJsonlCollector
from codex_meter.models import MeterEventKind
from codex_meter.pricing import PricingCatalog
from codex_meter.storage import Storage

from .helpers import base_events, complete_event, token_event, write_rollout


class SessionCollectorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.collector = SessionJsonlCollector()

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_cumulative_events_are_deduplicated_and_delta_detected(self) -> None:
        events = base_events() + [
            token_event("2026-08-12T00:00:04Z", (100, 60, 0, 10, 4, 110)),
            token_event("2026-08-12T00:00:05Z", (100, 60, 0, 10, 4, 110)),
            token_event("2026-08-12T00:00:06Z", (250, 160, 10, 30, 12, 280), (150, 100, 10, 20, 8, 170)),
            complete_event(),
        ]
        parsed = self.collector.collect_file(write_rollout(self.root / "rollout-test.jsonl", events))

        self.assertEqual(len(parsed.llm_calls), 2)
        self.assertEqual(sum(call.usage.input_tokens for call in parsed.llm_calls), 250)
        self.assertEqual(sum(call.usage.cached_input_tokens for call in parsed.llm_calls), 160)
        self.assertEqual(sum(call.usage.output_tokens for call in parsed.llm_calls), 30)
        self.assertEqual(parsed.duplicate_usage_events, 1)
        self.assertEqual(parsed.session.auth_mode, "chatgpt")
        self.assertEqual(parsed.turns["turn-1"].reasoning_effort, "high")
        self.assertEqual(parsed.turns["turn-1"].ttft_ms, 2200)
        kinds = [event.kind for event in parsed.events()]
        self.assertEqual(kinds.count(MeterEventKind.LLM_CALL_COMPLETED), 2)
        self.assertIn(MeterEventKind.SESSION_UPSERT, kinds)

    def test_raw_response_usage_wins_over_matching_cumulative_delta(self) -> None:
        raw = {
            "timestamp": "2026-08-12T00:00:04Z",
            "type": "event_msg",
            "payload": {
                "type": "raw_response_completed",
                "turn_id": "turn-1",
                "response_id": "resp-1",
                "token_usage": {
                    "input_tokens": 100,
                    "cached_input_tokens": 80,
                    "cache_write_input_tokens": 0,
                    "output_tokens": 10,
                    "reasoning_output_tokens": 4,
                    "total_tokens": 110,
                },
            },
        }
        events = base_events() + [raw, token_event("2026-08-12T00:00:05Z", (100, 80, 0, 10, 4, 110)), complete_event()]
        parsed = self.collector.collect_file(write_rollout(self.root / "rollout-raw.jsonl", events))

        self.assertEqual(len(parsed.llm_calls), 1)
        self.assertEqual(parsed.llm_calls[0].response_id, "resp-1")
        self.assertEqual(parsed.llm_calls[0].quality.confidence.value, "exact")
        self.assertFalse(parsed.llm_calls[0].quality.estimated)

    def test_accumulator_reset_falls_back_to_last_usage(self) -> None:
        events = base_events() + [
            token_event("2026-08-12T00:00:04Z", (200, 100, 0, 20, 5, 220)),
            token_event("2026-08-12T00:00:05Z", (50, 20, 0, 6, 2, 56), (50, 20, 0, 6, 2, 56)),
            complete_event(),
        ]
        parsed = self.collector.collect_file(write_rollout(self.root / "rollout-reset.jsonl", events))
        self.assertEqual([call.usage.input_tokens for call in parsed.llm_calls], [200, 50])

    def test_malformed_lines_fail_open(self) -> None:
        path = write_rollout(
            self.root / "rollout-malformed.jsonl",
            base_events() + [token_event("2026-08-12T00:00:04Z", (10, 0, 0, 2, 0, 12)), complete_event()],
        )
        with path.open("a", encoding="utf-8") as handle:
            handle.write("{broken\n")
        parsed = self.collector.collect_file(path)
        self.assertEqual(parsed.malformed_lines, 1)
        self.assertEqual(len(parsed.llm_calls), 1)

    def test_storage_import_is_idempotent(self) -> None:
        path = write_rollout(
            self.root / "rollout-storage.jsonl",
            base_events() + [token_event("2026-08-12T00:00:04Z", (100, 80, 0, 10, 4, 110)), complete_event()],
        )
        parsed = self.collector.collect_file(path)
        with Storage(self.root / "meter.db") as storage:
            storage.migrate()
            storage.sync_pricing(PricingCatalog.bundled())
            storage.import_session(parsed, path)
            storage.import_session(parsed, path)
            counts = storage.counts()
            overview = storage.overview("2026-08-12")
            session_row = storage.sessions(1)[0]
        self.assertEqual(counts["sessions"], 1)
        self.assertEqual(counts["turns"], 1)
        self.assertEqual(counts["llm_calls"], 1)
        self.assertEqual(overview["input_tokens"], 100)
        self.assertEqual(session_row["calls"], 1)
        self.assertEqual(session_row["total_tokens"], 110)

    def test_fork_replay_with_rewritten_timestamps_does_not_duplicate_call(self) -> None:
        parent_path = write_rollout(
            self.root / "rollout-parent.jsonl",
            base_events() + [token_event("2026-08-12T00:00:04Z", (100, 80, 0, 10, 4, 110)), complete_event()],
        )
        child_events = base_events()
        child_events[0]["payload"]["id"] = "thread-2"
        child_events[0]["payload"]["session_id"] = "thread-2"
        child_events[0]["payload"]["forked_from_id"] = "thread-1"
        replayed_token = token_event("2026-08-12T01:00:04Z", (100, 80, 0, 10, 4, 110))
        # A newer Codex version may serialize a formerly absent zero-valued field.
        replayed_token["payload"]["info"]["total_token_usage"].pop("cache_write_input_tokens")
        replayed_token["payload"]["info"]["last_token_usage"].pop("cache_write_input_tokens")
        child_events += [replayed_token, complete_event("2026-08-12T01:00:10Z")]
        child_path = write_rollout(self.root / "rollout-child.jsonl", child_events)
        parent = self.collector.collect_file(parent_path)
        child = self.collector.collect_file(child_path)

        self.assertEqual(parent.llm_calls[0].event_fingerprint, child.llm_calls[0].event_fingerprint)
        with Storage(self.root / "fork.db") as storage:
            storage.migrate()
            storage.import_session(parent, parent_path)
            storage.import_session(child, child_path)
            counts = storage.counts()
        self.assertEqual(counts["sessions"], 2)
        self.assertEqual(counts["llm_calls"], 1)

    def test_collector_never_persists_payload_text(self) -> None:
        events = base_events() + [
            {
                "timestamp": "2026-08-12T00:00:03Z",
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "TOP SECRET PROMPT"},
            },
            token_event("2026-08-12T00:00:04Z", (10, 0, 0, 2, 0, 12)),
            complete_event(),
        ]
        path = write_rollout(self.root / "rollout-privacy.jsonl", events)
        parsed = self.collector.collect_file(path)
        database = self.root / "privacy.db"
        with Storage(database) as storage:
            storage.migrate()
            storage.import_session(parsed, path)
        self.assertNotIn(b"TOP SECRET PROMPT", database.read_bytes())


if __name__ == "__main__":
    unittest.main()
