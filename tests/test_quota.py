from __future__ import annotations

import sys
import unittest

from codex_meter.quota import WEEK_MINUTES, extract_weekly_quotas, read_weekly_quotas


class QuotaTests(unittest.TestCase):
    def test_reads_batched_json_rpc_responses_from_stdio(self) -> None:
        script = """
import json, sys
for _ in range(3):
    sys.stdin.readline()
print(json.dumps({'id': 1, 'result': {}}))
print(json.dumps({'id': 2, 'result': {'rateLimits': {
    'limitId': 'codex',
    'primary': {'usedPercent': 42, 'windowDurationMins': 10080, 'resetsAt': 1234},
}}}), flush=True)
"""

        quotas = read_weekly_quotas((sys.executable, "-c", script), timeout=2)

        self.assertEqual(len(quotas), 1)
        self.assertEqual(quotas[0].used_percent, 42)

    def test_extracts_all_weekly_buckets_and_prefers_main_codex_first(self) -> None:
        result = {
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 99, "windowDurationMins": 300},
                "secondary": {"usedPercent": 15, "windowDurationMins": WEEK_MINUTES, "resetsAt": 1234},
                "planType": "pro",
            },
            "rateLimitsByLimitId": {
                "codex_bengalfox": {
                    "limitId": "codex_bengalfox",
                    "limitName": "GPT-5.3-Codex-Spark",
                    "primary": {"usedPercent": 0, "windowDurationMins": WEEK_MINUTES, "resetsAt": 5678},
                    "planType": "pro",
                },
                "codex": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 15, "windowDurationMins": WEEK_MINUTES, "resetsAt": 1234},
                    "planType": "pro",
                },
            },
        }

        quotas = extract_weekly_quotas(result)

        self.assertEqual([quota.limit_id for quota in quotas], ["codex", "codex_bengalfox"])
        self.assertEqual(quotas[0].name, "Codex")
        self.assertEqual(quotas[0].used_percent, 15)
        self.assertEqual(quotas[0].remaining_percent, 85)
        self.assertEqual(quotas[1].name, "GPT-5.3-Codex-Spark")

    def test_ignores_nonweekly_and_malformed_windows(self) -> None:
        result = {
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 20, "windowDurationMins": 300},
                "secondary": {"usedPercent": "bad", "windowDurationMins": None},
            }
        }
        self.assertEqual(extract_weekly_quotas(result), ())

    def test_clamps_backend_percentage_for_safe_remaining_display(self) -> None:
        result = {
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 150, "windowDurationMins": WEEK_MINUTES},
            }
        }
        quota = extract_weekly_quotas(result)[0]
        self.assertEqual(quota.used_percent, 100)
        self.assertEqual(quota.remaining_percent, 0)


if __name__ == "__main__":
    unittest.main()
