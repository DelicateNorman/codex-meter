"""Versioned, data-driven pricing calculations."""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime, timezone
from importlib.resources import files
from pathlib import Path

from .models import TokenUsage


@dataclass(frozen=True, slots=True)
class Price:
    model: str
    provider: str
    effective_from: str
    version: str
    input_per_million: float
    cached_input_per_million: float
    cache_write_per_million: float
    output_per_million: float
    long_context_threshold: int | None = None
    long_context_input_multiplier: float = 1.0
    long_context_output_multiplier: float = 1.0


@dataclass(frozen=True, slots=True)
class CostBreakdown:
    regular_input_usd: float
    cached_input_usd: float
    cache_write_usd: float
    output_usd: float
    total_usd: float
    without_cache_usd: float
    savings_usd: float
    pricing_version: str


class PricingCatalog:
    def __init__(self, entries: list[Price]) -> None:
        self.entries = entries

    @classmethod
    def bundled(cls, override_path: Path | None = None) -> "PricingCatalog":
        path = override_path or Path(str(files("codex_meter").joinpath("data/pricing.json")))
        data = json.loads(path.read_text(encoding="utf-8"))
        return cls([Price(**entry) for entry in data["prices"]])

    def resolve(self, model: str | None, provider: str | None, at: str | None) -> Price | None:
        if not model:
            return None
        provider = provider or "openai"
        aliases = {"gpt-5.6": "gpt-5.6-sol"}
        model = aliases.get(model, model)
        at_dt = _parse_time(at)
        candidates = [
            entry
            for entry in self.entries
            if entry.model == model
            and entry.provider == provider
            and _parse_time(entry.effective_from) <= at_dt
        ]
        return max(candidates, key=lambda entry: entry.effective_from, default=None)

    def calculate(self, usage: TokenUsage, price: Price) -> CostBreakdown:
        input_multiplier = 1.0
        output_multiplier = 1.0
        if price.long_context_threshold is not None and usage.input_tokens > price.long_context_threshold:
            input_multiplier = price.long_context_input_multiplier
            output_multiplier = price.long_context_output_multiplier

        scale = 1_000_000
        regular = usage.billable_regular_input_tokens * price.input_per_million * input_multiplier / scale
        cached = usage.cached_input_tokens * price.cached_input_per_million * input_multiplier / scale
        cache_write = usage.cache_write_tokens * price.cache_write_per_million * input_multiplier / scale
        # Reasoning tokens are already a subset of output_tokens and are never added again.
        output = usage.output_tokens * price.output_per_million * output_multiplier / scale
        total = regular + cached + cache_write + output
        without_cache = usage.input_tokens * price.input_per_million * input_multiplier / scale + output
        return CostBreakdown(
            regular_input_usd=regular,
            cached_input_usd=cached,
            cache_write_usd=cache_write,
            output_usd=output,
            total_usd=total,
            without_cache_usd=without_cache,
            savings_usd=without_cache - total,
            pricing_version=price.version,
        )


def _parse_time(value: str | None) -> datetime:
    if not value:
        return datetime.max.replace(tzinfo=timezone.utc)
    normalized = value.replace("Z", "+00:00")
    parsed = datetime.fromisoformat(normalized)
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed
