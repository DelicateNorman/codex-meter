from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from codex_meter.cli import _period_bounds, build_parser
from codex_meter.config import initialize_home, load_identity, update_account_identity
from codex_meter.models import LlmCallRecord, TokenUsage
from codex_meter.storage import Storage


class IdentityAndPeriodTests(unittest.TestCase):
    def test_account_tracking_is_opt_in_and_uses_manual_label(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            initialize_home(home)
            self.assertFalse(load_identity(home).account_tracking)
            identity = update_account_identity(home, enabled=True, label="work")
            self.assertTrue(identity.account_tracking)
            self.assertEqual(identity.account_label, "work")
            text = (home / "config.toml").read_text(encoding="utf-8")
            self.assertNotIn("token", text.lower())
            self.assertNotIn("email", text.lower())
            identity = update_account_identity(home, enabled=False, label=None)
            self.assertFalse(identity.account_tracking)
            self.assertIsNone(identity.account_label)

    def test_usage_is_scoped_to_os_user_and_optionally_account(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "meter.db"
            with Storage(database, owner_uid=1001, owner_username="alice", account_label="personal") as storage:
                storage.migrate()
                storage.insert_live_call("thread-personal", _call("p", "2026-08-12T01:00:00Z", 100))
                with storage.transaction() as connection:
                    connection.execute(
                        "UPDATE sessions SET project_name='alpha' WHERE codex_thread_id='thread-personal'"
                    )
            with Storage(database, owner_uid=1001, owner_username="alice", account_label="work") as storage:
                storage.migrate()
                storage.insert_live_call("thread-work", _call("w", "2026-08-13T01:00:00Z", 200))
                with storage.transaction() as connection:
                    connection.execute(
                        "UPDATE sessions SET project_name='beta' WHERE codex_thread_id='thread-work'"
                    )
                self.assertEqual(storage.overview()["total_tokens"], 300)
                self.assertEqual(storage.overview(account="personal")["total_tokens"], 100)
                self.assertEqual(storage.overview(account="work")["total_tokens"], 200)
                self.assertEqual(storage.overview(project="alpha")["total_tokens"], 100)
                self.assertEqual(storage.overview(project="beta")["total_tokens"], 200)
                self.assertEqual(
                    storage.overview(account="work", project="beta")["total_tokens"], 200,
                )
                self.assertEqual(storage.project_names(), ["beta", "alpha"])
                self.assertEqual(len(storage.usage_history("day", project="beta")), 1)
                self.assertEqual(len(storage.usage_history("day")), 2)
                labels = {row["account"] for row in storage.account_breakdown()}
                self.assertEqual(labels, {"personal", "work"})
            with Storage(database, owner_uid=1002, owner_username="bob") as storage:
                storage.migrate()
                self.assertEqual(storage.overview()["total_tokens"], 0)
                self.assertEqual(storage.sessions(), [])

    def test_period_bounds_cover_day_week_month_and_all(self) -> None:
        self.assertEqual(_period_bounds("day", "2026-08-12")[:2], ("2026-08-12", "2026-08-12"))
        self.assertEqual(_period_bounds("week", "2026-08-12")[:2], ("2026-08-10", "2026-08-16"))
        self.assertEqual(_period_bounds("month", "2026-02-12")[:2], ("2026-02-01", "2026-02-28"))
        self.assertEqual(_period_bounds("all", "2026-08-12")[:2], (None, None))

    def test_summary_and_history_accept_project_filters(self) -> None:
        parser = build_parser()
        summary = parser.parse_args(["summary", "--period", "month", "--project", "alpha"])
        history = parser.parse_args(["history", "--group", "week", "--project", "beta"])
        today = parser.parse_args(["today", "--project", "gamma"])
        self.assertEqual(summary.project, "alpha")
        self.assertEqual(history.project, "beta")
        self.assertEqual(today.project, "gamma")


def _call(fingerprint: str, completed_at: str, tokens: int) -> LlmCallRecord:
    return LlmCallRecord(
        event_fingerprint=fingerprint,
        turn_id=None,
        response_id=None,
        completed_at=completed_at,
        model="test-model",
        actual_model=None,
        provider="openai",
        reasoning_effort="medium",
        reasoning_mode=None,
        service_tier=None,
        usage=TokenUsage(input_tokens=tokens, total_tokens=tokens),
    )


if __name__ == "__main__":
    unittest.main()
