//! Statistical helpers shared by reports and performance views.

use crate::models::TokenUsage;
use crate::pricing::PricingCatalog;
use std::collections::{BTreeMap, HashMap};

pub const PERFORMANCE_METRICS: &[&str] = &[
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
];

#[derive(Debug, Clone, Default)]
pub struct MetricSample {
    pub name: String,
    pub event_fingerprint: String,
    pub attributes_json: String,
    pub start_time_unix_nano: Option<String>,
    pub time_unix_nano: Option<String>,
    pub value: Option<f64>,
    pub point_sum: Option<f64>,
    pub point_count: Option<u64>,
    pub point_max: Option<f64>,
    pub explicit_bounds: Vec<f64>,
    pub bucket_counts: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PerformanceSummary {
    pub name: String,
    pub count: u64,
    pub average: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub tps: Option<f64>,
}

pub fn performance_summary(rows: &[MetricSample]) -> Vec<PerformanceSummary> {
    let mut latest: HashMap<(String, String, String), &MetricSample> = HashMap::new();
    for row in rows {
        let key = (
            row.name.clone(),
            row.attributes_json.clone(),
            row.start_time_unix_nano
                .clone()
                .unwrap_or_else(|| row.event_fingerprint.clone()),
        );
        if latest.get(&key).is_none_or(|previous| {
            row.time_unix_nano.as_deref().unwrap_or("")
                >= previous.time_unix_nano.as_deref().unwrap_or("")
        }) {
            latest.insert(key, row);
        }
    }
    let mut scalars: BTreeMap<String, Vec<(f64, u64)>> = BTreeMap::new();
    let mut buckets: BTreeMap<String, Vec<(f64, u64)>> = BTreeMap::new();
    for row in latest.values() {
        let count = row.point_count.unwrap_or(0);
        if let Some(sum) = row.point_sum.filter(|_| count > 0) {
            scalars
                .entry(row.name.clone())
                .or_default()
                .push((sum / count as f64, count));
        } else if let Some(value) = row.value {
            scalars
                .entry(row.name.clone())
                .or_default()
                .push((value, 1));
        }
        for (index, weight) in row.bucket_counts.iter().copied().enumerate() {
            if weight == 0 {
                continue;
            }
            let representative = row
                .explicit_bounds
                .get(index)
                .copied()
                .or(row.point_max)
                .or_else(|| row.explicit_bounds.last().copied())
                .or(row.value)
                .unwrap_or(0.0);
            buckets
                .entry(row.name.clone())
                .or_default()
                .push((representative, weight));
        }
    }
    let names: BTreeMap<_, _> = scalars
        .keys()
        .chain(buckets.keys())
        .map(|name| (name.clone(), ()))
        .collect();
    names
        .into_keys()
        .map(|name| {
            let weighted = scalars.get(&name).cloned().unwrap_or_default();
            let count: u64 = weighted.iter().map(|(_, weight)| *weight).sum();
            let average = (count > 0).then(|| {
                weighted
                    .iter()
                    .map(|(value, weight)| value * *weight as f64)
                    .sum::<f64>()
                    / count as f64
            });
            let distribution = buckets
                .get(&name)
                .filter(|values| !values.is_empty())
                .cloned()
                .unwrap_or_else(|| weighted.clone());
            PerformanceSummary {
                name: name.clone(),
                count: if count > 0 {
                    count
                } else {
                    distribution.iter().map(|(_, weight)| *weight).sum()
                },
                average,
                p50: weighted_percentile(&distribution, 0.5),
                p95: weighted_percentile(&distribution, 0.95),
                tps: average
                    .filter(|value| *value > 0.0 && name.ends_with("_tbt.duration_ms"))
                    .map(|value| 1000.0 / value),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct UsageSample {
    pub id: i64,
    pub turn_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub completed_at: Option<String>,
    pub usage: TokenUsage,
    pub retry_index: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheSummary {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub observed_cost_usd: f64,
    pub without_cache_usd: f64,
    pub savings_usd: f64,
    pub priced_calls: usize,
    pub unpriced_calls: usize,
    pub reuse_rate: f64,
}

pub fn cache_summary(rows: &[UsageSample], catalog: &PricingCatalog) -> CacheSummary {
    let mut output = CacheSummary::default();
    for row in rows {
        output.input_tokens += row.usage.input_tokens;
        output.cached_input_tokens += row.usage.cached_input_tokens;
        output.cache_write_tokens += row.usage.cache_write_tokens;
        if let Some(resolved) = catalog.resolve_for_estimate(
            row.model.as_deref(),
            row.provider.as_deref(),
            row.completed_at.as_deref(),
        ) {
            let cost = catalog.calculate(row.usage, resolved.price);
            output.observed_cost_usd += cost.total_usd;
            output.without_cache_usd += cost.without_cache_usd;
            output.savings_usd += cost.savings_usd;
            output.priced_calls += 1;
        } else {
            output.unpriced_calls += 1;
        }
    }
    if output.input_tokens > 0 {
        output.reuse_rate = output.cached_input_tokens as f64 / output.input_tokens as f64;
    }
    output
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContextRetrySummary {
    pub turns: usize,
    pub amplified_turns: usize,
    pub average_context_amplification: Option<f64>,
    pub max_context_amplification: Option<f64>,
    pub retry_calls: usize,
    pub retry_tokens: i64,
    pub retry_cost_usd: f64,
}

pub fn context_and_retry_summary(rows: &[UsageSample]) -> ContextRetrySummary {
    let mut grouped: HashMap<String, Vec<&UsageSample>> = HashMap::new();
    let mut output = ContextRetrySummary::default();
    for row in rows {
        grouped
            .entry(
                row.turn_id
                    .clone()
                    .unwrap_or_else(|| format!("call:{}", row.id)),
            )
            .or_default()
            .push(row);
        if row.retry_index > 0 {
            output.retry_calls += 1;
            output.retry_tokens += row.usage.total_tokens;
            output.retry_cost_usd += row.cost_usd.unwrap_or(0.0);
        }
    }
    output.turns = grouped.len();
    let mut amplification = Vec::new();
    for calls in grouped.values() {
        let inputs: Vec<_> = calls
            .iter()
            .map(|row| row.usage.input_tokens)
            .filter(|value| *value > 0)
            .collect();
        if inputs.len() > 1 {
            amplification.push(*inputs.last().unwrap() as f64 / inputs[0] as f64);
        }
    }
    output.amplified_turns = amplification.len();
    if !amplification.is_empty() {
        output.average_context_amplification =
            Some(amplification.iter().sum::<f64>() / amplification.len() as f64);
        output.max_context_amplification = amplification.into_iter().max_by(f64::total_cmp);
    }
    output
}

pub fn weighted_percentile(samples: &[(f64, u64)], percentile: f64) -> Option<f64> {
    let mut ordered: Vec<_> = samples
        .iter()
        .copied()
        .filter(|(_, weight)| *weight > 0)
        .collect();
    ordered.sort_by(|left, right| left.0.total_cmp(&right.0));
    let total: u64 = ordered.iter().map(|(_, weight)| *weight).sum();
    if total == 0 {
        return None;
    }
    let target = (percentile.clamp(0.0, 1.0) * total as f64).ceil() as u64;
    let mut accumulated = 0;
    for (value, weight) in ordered {
        accumulated += weight;
        if accumulated >= target.max(1) {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percentile_obeys_weights() {
        assert_eq!(
            weighted_percentile(&[(10.0, 1), (20.0, 3)], 0.5),
            Some(20.0)
        );
    }
    #[test]
    fn context_amplification_and_retry() {
        let rows = vec![
            UsageSample {
                id: 1,
                turn_id: Some("t".into()),
                usage: TokenUsage {
                    input_tokens: 100,
                    ..Default::default()
                },
                ..Default::default()
            },
            UsageSample {
                id: 2,
                turn_id: Some("t".into()),
                retry_index: 1,
                cost_usd: Some(0.2),
                usage: TokenUsage {
                    input_tokens: 250,
                    total_tokens: 270,
                    ..Default::default()
                },
                ..Default::default()
            },
        ];
        let summary = context_and_retry_summary(&rows);
        assert_eq!(summary.average_context_amplification, Some(2.5));
        assert_eq!(summary.retry_tokens, 270);
    }
}
