from __future__ import annotations

import unittest

from codex_meter.models import TokenUsage
from codex_meter.pricing import Price, PricingCatalog


class PricingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.catalog = PricingCatalog.bundled()
        self.price = self.catalog.resolve("gpt-5.6-sol", "openai", "2026-08-12T00:00:00Z")
        assert self.price is not None

    def test_reasoning_is_not_double_charged(self) -> None:
        usage = TokenUsage(input_tokens=0, output_tokens=1_000_000, reasoning_tokens=900_000, total_tokens=1_000_000)
        cost = self.catalog.calculate(usage, self.price)
        self.assertEqual(cost.output_usd, 30.0)
        self.assertEqual(cost.total_usd, 30.0)

    def test_cache_read_write_and_miss_have_separate_rates(self) -> None:
        usage = TokenUsage(
            input_tokens=200_000,
            cached_input_tokens=120_000,
            cache_write_tokens=20_000,
            output_tokens=0,
            total_tokens=200_000,
        )
        cost = self.catalog.calculate(usage, self.price)
        self.assertAlmostEqual(cost.regular_input_usd, 0.3)
        self.assertAlmostEqual(cost.cached_input_usd, 0.06)
        self.assertAlmostEqual(cost.cache_write_usd, 0.125)
        self.assertAlmostEqual(cost.total_usd, 0.485)
        self.assertAlmostEqual(cost.without_cache_usd, 1.0)
        self.assertAlmostEqual(cost.savings_usd, 0.515)

    def test_long_context_multipliers_apply_to_whole_call(self) -> None:
        usage = TokenUsage(input_tokens=300_000, output_tokens=100_000, total_tokens=400_000)
        cost = self.catalog.calculate(usage, self.price)
        self.assertAlmostEqual(cost.regular_input_usd, 3.0)
        self.assertAlmostEqual(cost.output_usd, 4.5)

    def test_unknown_model_returns_none(self) -> None:
        self.assertIsNone(self.catalog.resolve("private-model", "other", "2026-08-12T00:00:00Z"))


if __name__ == "__main__":
    unittest.main()
