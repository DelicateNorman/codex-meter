//! Versioned, data-driven API-equivalent pricing.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;

use crate::models::TokenUsage;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Price {
    pub model: String,
    pub provider: String,
    pub effective_from: String,
    pub version: String,
    pub input_per_million: f64,
    pub cached_input_per_million: f64,
    pub cache_write_per_million: f64,
    pub output_per_million: f64,
    pub long_context_threshold: Option<i64>,
    #[serde(default = "one")]
    pub long_context_input_multiplier: f64,
    #[serde(default = "one")]
    pub long_context_output_multiplier: f64,
}

const fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostBreakdown {
    pub regular_input_usd: f64,
    pub cached_input_usd: f64,
    pub cache_write_usd: f64,
    pub output_usd: f64,
    pub total_usd: f64,
    pub without_cache_usd: f64,
    pub savings_usd: f64,
    pub pricing_version: String,
}

#[derive(Debug, Clone, Default)]
pub struct PricingCatalog {
    pub entries: Vec<Price>,
}

#[derive(Deserialize)]
struct PriceFile {
    prices: Vec<Price>,
}

impl PricingCatalog {
    pub fn new(entries: Vec<Price>) -> Self {
        Self { entries }
    }

    /// Load the catalog compiled into the standalone binary.
    pub fn bundled() -> Result<Self> {
        Self::from_json(include_str!("../codex_meter/data/pricing.json"))
            .context("parse bundled pricing catalog")
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read pricing catalog {}", path.display()))?;
        Self::from_json(&text).with_context(|| format!("parse pricing catalog {}", path.display()))
    }

    pub fn from_json(value: &str) -> Result<Self> {
        let parsed: PriceFile = serde_json::from_str(value)?;
        Ok(Self::new(parsed.prices))
    }

    pub fn resolve<'a>(
        &'a self,
        model: Option<&str>,
        provider: Option<&str>,
        at: Option<&str>,
    ) -> Option<&'a Price> {
        let model = match model? {
            "gpt-5.6" => "gpt-5.6-sol",
            value => value,
        };
        let provider = provider.unwrap_or("openai");
        let at = at.and_then(parse_time);
        self.entries
            .iter()
            .filter(|entry| {
                if entry.model != model || entry.provider != provider {
                    return false;
                }
                match (parse_time(&entry.effective_from), at) {
                    (Some(effective), Some(at)) => effective <= at,
                    (Some(_), None) => true,
                    (None, _) => false,
                }
            })
            .max_by(|left, right| left.effective_from.cmp(&right.effective_from))
    }

    pub fn calculate(&self, usage: TokenUsage, price: &Price) -> CostBreakdown {
        let (input_multiplier, output_multiplier) = match price.long_context_threshold {
            Some(threshold) if usage.input_tokens > threshold => (
                price.long_context_input_multiplier,
                price.long_context_output_multiplier,
            ),
            _ => (1.0, 1.0),
        };
        let scale = 1_000_000.0;
        let regular = usage.billable_regular_input_tokens() as f64
            * price.input_per_million
            * input_multiplier
            / scale;
        let cached =
            usage.cached_input_tokens as f64 * price.cached_input_per_million * input_multiplier
                / scale;
        let cache_write =
            usage.cache_write_tokens as f64 * price.cache_write_per_million * input_multiplier
                / scale;
        // Reasoning tokens are already part of output_tokens.
        let output =
            usage.output_tokens as f64 * price.output_per_million * output_multiplier / scale;
        let total = regular + cached + cache_write + output;
        let without_cache =
            usage.input_tokens as f64 * price.input_per_million * input_multiplier / scale + output;
        CostBreakdown {
            regular_input_usd: regular,
            cached_input_usd: cached,
            cache_write_usd: cache_write,
            output_usd: output,
            total_usd: total,
            without_cache_usd: without_cache,
            savings_usd: without_cache - total,
            pricing_version: price.version.clone(),
        }
    }
}

fn parse_time(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_alias_and_calculates_cache_savings() {
        let catalog = PricingCatalog::bundled().unwrap();
        let price = catalog
            .resolve(Some("gpt-5.6"), None, Some("2026-08-12T00:00:00Z"))
            .unwrap();
        assert_eq!(price.model, "gpt-5.6-sol");
        let usage = TokenUsage {
            input_tokens: 200_000,
            cached_input_tokens: 160_000,
            output_tokens: 20_000,
            reasoning_tokens: 10_000,
            total_tokens: 220_000,
            ..Default::default()
        };
        let cost = catalog.calculate(usage, price);
        assert!((cost.total_usd - 0.88).abs() < 1e-9);
        assert!((cost.savings_usd - 0.72).abs() < 1e-9);
    }

    #[test]
    fn long_context_multipliers_match_python() {
        let catalog = PricingCatalog::bundled().unwrap();
        let price = catalog
            .resolve(Some("gpt-5.6-sol"), Some("openai"), None)
            .unwrap();
        let usage = TokenUsage {
            input_tokens: 300_000,
            output_tokens: 100_000,
            ..Default::default()
        };
        let cost = catalog.calculate(usage, price);
        assert!((cost.total_usd - 7.5).abs() < 1e-9);
    }
}
