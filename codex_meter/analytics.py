"""Higher-level analysis over normalized usage and OTLP metric points."""

from __future__ import annotations

import json
import math
from collections import defaultdict
from collections.abc import Iterable, Mapping
from typing import Any

from .models import TokenUsage
from .pricing import PricingCatalog


PERFORMANCE_METRICS = (
    "codex.api_request.duration_ms",
    "codex.responses_api_overhead.duration_ms",
    "codex.responses_api_inference_time.duration_ms",
    "codex.responses_api_engine_iapi_ttft.duration_ms",
    "codex.responses_api_engine_service_ttft.duration_ms",
    "codex.responses_api_engine_iapi_tbt.duration_ms",
    "codex.responses_api_engine_service_tbt.duration_ms",
    "codex.turn.e2e_duration_ms",
    "codex.turn.ttft.duration_ms",
    "codex.turn.ttfm.duration_ms",
    "codex.tool.call.duration_ms",
)


def performance_summary(rows: Iterable[Mapping[str, Any]]) -> list[dict[str, Any]]:
    """Return count/average/approximate P50/P95 from latest OTLP series snapshots."""
    latest: dict[tuple[str, str, str], Mapping[str, Any]] = {}
    scalars: dict[str, list[tuple[float, int]]] = defaultdict(list)
    buckets: dict[str, list[tuple[float, int]]] = defaultdict(list)
    for row in rows:
        key = (
            str(row["name"]),
            str(row["attributes_json"] or "{}"),
            str(row["start_time_unix_nano"] or row["event_fingerprint"]),
        )
        previous = latest.get(key)
        if previous is None or str(row["time_unix_nano"] or "") >= str(previous["time_unix_nano"] or ""):
            latest[key] = row

    for row in latest.values():
        name = str(row["name"])
        count = int(row["point_count"] or 0)
        total = row["point_sum"]
        bounds = _json_numbers(row["explicit_bounds_json"])
        counts = [int(value) for value in _json_numbers(row["bucket_counts_json"])]
        if count and total is not None:
            scalars[name].append((float(total) / count, count))
        elif row["value"] is not None:
            scalars[name].append((float(row["value"]), 1))
        if counts:
            for index, weight in enumerate(counts):
                if not weight:
                    continue
                if index < len(bounds):
                    representative = bounds[index]
                elif bounds:
                    representative = float(row["point_max"] or bounds[-1])
                else:
                    representative = float(row["point_max"] or row["value"] or 0)
                buckets[name].append((representative, weight))

    output: list[dict[str, Any]] = []
    for name in sorted(set(scalars) | set(buckets)):
        weighted = scalars.get(name, [])
        sample_count = sum(weight for _, weight in weighted)
        average = (
            sum(value * weight for value, weight in weighted) / sample_count
            if sample_count else None
        )
        distribution = buckets.get(name) or weighted
        output.append(
            {
                "name": name,
                "count": sample_count or sum(weight for _, weight in distribution),
                "avg": average,
                "p50": weighted_percentile(distribution, 0.50),
                "p95": weighted_percentile(distribution, 0.95),
                "tps": (
                    1000 / average
                    if average and name.endswith("_tbt.duration_ms") and average > 0 else None
                ),
            }
        )
    return output


def cache_summary(rows: Iterable[Mapping[str, Any]], catalog: PricingCatalog) -> dict[str, Any]:
    totals = {
        "input_tokens": 0,
        "cached_input_tokens": 0,
        "cache_write_tokens": 0,
        "observed_cost_usd": 0.0,
        "without_cache_usd": 0.0,
        "savings_usd": 0.0,
        "priced_calls": 0,
        "unpriced_calls": 0,
    }
    for row in rows:
        usage = TokenUsage(
            input_tokens=int(row["input_tokens"] or 0),
            cached_input_tokens=int(row["cached_input_tokens"] or 0),
            cache_write_tokens=int(row["cache_write_tokens"] or 0),
            output_tokens=int(row["output_tokens"] or 0),
            reasoning_tokens=int(row["reasoning_tokens"] or 0),
            total_tokens=int(row["total_tokens"] or 0),
        )
        totals["input_tokens"] += usage.input_tokens
        totals["cached_input_tokens"] += usage.cached_input_tokens
        totals["cache_write_tokens"] += usage.cache_write_tokens
        price = catalog.resolve(row["model"], row["provider"], row["completed_at"])
        if price is None:
            totals["unpriced_calls"] += 1
            continue
        cost = catalog.calculate(usage, price)
        totals["observed_cost_usd"] += cost.total_usd
        totals["without_cache_usd"] += cost.without_cache_usd
        totals["savings_usd"] += cost.savings_usd
        totals["priced_calls"] += 1
    input_tokens = totals["input_tokens"]
    totals["reuse_rate"] = totals["cached_input_tokens"] / input_tokens if input_tokens else 0.0
    return totals


def context_and_retry_summary(rows: Iterable[Mapping[str, Any]]) -> dict[str, Any]:
    by_turn: dict[str, list[Mapping[str, Any]]] = defaultdict(list)
    retry_tokens = retry_calls = 0
    retry_cost = 0.0
    for row in rows:
        turn_id = str(row["codex_turn_id"] or f"call:{row['id']}")
        by_turn[turn_id].append(row)
        if int(row["retry_index"] or 0) > 0:
            retry_calls += 1
            retry_tokens += int(row["total_tokens"] or 0)
            retry_cost += float(row["cost_usd"] or 0)
    amplification: list[float] = []
    for calls in by_turn.values():
        inputs = [int(row["input_tokens"] or 0) for row in calls if int(row["input_tokens"] or 0) > 0]
        if len(inputs) > 1 and inputs[0]:
            amplification.append(inputs[-1] / inputs[0])
    return {
        "turns": len(by_turn),
        "amplified_turns": len(amplification),
        "average_context_amplification": sum(amplification) / len(amplification) if amplification else None,
        "max_context_amplification": max(amplification, default=None),
        "retry_calls": retry_calls,
        "retry_tokens": retry_tokens,
        "retry_cost_usd": retry_cost,
    }


def weighted_percentile(values: Iterable[tuple[float, int]], percentile: float) -> float | None:
    ordered = sorted((float(value), int(weight)) for value, weight in values if weight > 0)
    total = sum(weight for _, weight in ordered)
    if not total:
        return None
    target = max(1, math.ceil(total * percentile))
    seen = 0
    for value, weight in ordered:
        seen += weight
        if seen >= target:
            return value
    return ordered[-1][0]


def _json_numbers(value: object) -> list[float]:
    try:
        parsed = json.loads(str(value or "[]"))
    except (json.JSONDecodeError, TypeError):
        return []
    return [float(item) for item in parsed] if isinstance(parsed, list) else []
