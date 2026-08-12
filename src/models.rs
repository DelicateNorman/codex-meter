//! Normalized, metadata-only domain records.
//!
//! Deliberately absent: prompts, responses, reasoning text, tool arguments,
//! command lines, stdout/stderr, HTTP bodies/headers, and credentials.  Keeping
//! those concepts out of the type system makes accidental persistence harder.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confidence {
    Exact,
    Derived,
    Estimated,
    Unknown,
}

impl Confidence {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Derived => "derived",
            Self::Estimated => "estimated",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quality {
    pub source: String,
    pub confidence: Confidence,
    pub estimated: bool,
}

impl Quality {
    pub fn exact(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            confidence: Confidence::Exact,
            estimated: false,
        }
    }

    pub fn derived(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            confidence: Confidence::Derived,
            estimated: true,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
}

impl TokenUsage {
    pub fn delta(self, previous: Self) -> Option<Self> {
        let result = Self {
            input_tokens: self.input_tokens - previous.input_tokens,
            cached_input_tokens: self.cached_input_tokens - previous.cached_input_tokens,
            cache_write_tokens: self.cache_write_tokens - previous.cache_write_tokens,
            output_tokens: self.output_tokens - previous.output_tokens,
            reasoning_tokens: self.reasoning_tokens - previous.reasoning_tokens,
            total_tokens: self.total_tokens - previous.total_tokens,
        };
        (result.input_tokens >= 0
            && result.cached_input_tokens >= 0
            && result.cache_write_tokens >= 0
            && result.output_tokens >= 0
            && result.reasoning_tokens >= 0
            && result.total_tokens >= 0)
            .then_some(result)
    }

    pub fn cache_miss_tokens(self) -> i64 {
        (self.input_tokens - self.cached_input_tokens - self.cache_write_tokens).max(0)
    }

    pub fn billable_regular_input_tokens(self) -> i64 {
        self.cache_miss_tokens()
    }

    pub fn is_zero(self) -> bool {
        self.input_tokens == 0
            && self.cached_input_tokens == 0
            && self.cache_write_tokens == 0
            && self.output_tokens == 0
            && self.reasoning_tokens == 0
            && self.total_tokens == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub codex_thread_id: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub cwd: Option<String>,
    pub project_name: Option<String>,
    pub git_repo: Option<String>,
    pub git_branch: Option<String>,
    pub auth_mode: String,
    pub codex_version: Option<String>,
    pub provider: Option<String>,
    pub source_path: String,
    pub parent_thread_id: Option<String>,
    pub agent_role: Option<String>,
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRecord {
    pub codex_turn_id: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub status: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_mode: Option<String>,
    pub service_tier: Option<String>,
    pub ttft_ms: Option<i64>,
    pub ttfm_ms: Option<i64>,
    pub e2e_ms: Option<i64>,
    pub error_type: Option<String>,
    pub quality: Quality,
}

impl TurnRecord {
    pub fn new(codex_turn_id: impl Into<String>) -> Self {
        Self {
            codex_turn_id: codex_turn_id.into(),
            started_at: None,
            completed_at: None,
            status: "running".into(),
            model: None,
            reasoning_effort: None,
            reasoning_mode: None,
            service_tier: None,
            ttft_ms: None,
            ttfm_ms: None,
            e2e_ms: None,
            error_type: None,
            quality: Quality::exact("session_jsonl"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmCallRecord {
    pub event_fingerprint: String,
    pub turn_id: Option<String>,
    pub response_id: Option<String>,
    pub completed_at: Option<String>,
    pub model: Option<String>,
    pub actual_model: Option<String>,
    pub provider: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_mode: Option<String>,
    pub service_tier: Option<String>,
    pub usage: TokenUsage,
    pub success: bool,
    pub error_type: Option<String>,
    pub retry_index: i64,
    pub cost_usd: Option<f64>,
    pub pricing_version: Option<String>,
    pub quality: Quality,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRecord {
    pub call_id: String,
    pub turn_id: Option<String>,
    pub tool_name: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub success: Option<bool>,
    pub exit_code: Option<i64>,
    pub quality: Quality,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricPointRecord {
    pub event_fingerprint: String,
    pub observed_at: Option<String>,
    pub name: String,
    pub kind: String,
    pub value: Option<f64>,
    pub point_sum: Option<f64>,
    pub point_count: Option<i64>,
    pub point_min: Option<f64>,
    pub point_max: Option<f64>,
    pub explicit_bounds: Vec<f64>,
    pub bucket_counts: Vec<i64>,
    pub attributes: HashMap<String, String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub response_id: Option<String>,
    pub tool_name: Option<String>,
    pub start_time_unix_nano: Option<String>,
    pub time_unix_nano: Option<String>,
    pub quality: Quality,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryLogRecord {
    pub event_fingerprint: String,
    pub observed_at: Option<String>,
    pub event_name: String,
    pub severity: Option<String>,
    pub attributes: HashMap<String, String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub response_id: Option<String>,
    pub item_id: Option<String>,
    pub tool_name: Option<String>,
    pub duration_ms: Option<f64>,
    pub status: Option<String>,
    pub success: Option<bool>,
    pub quality: Quality,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetworkFlowRecord {
    pub event_fingerprint: String,
    pub mode: String,
    pub data_source: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub destination_host: Option<String>,
    pub destination_ip: Option<String>,
    pub destination_port: Option<i64>,
    pub protocol: Option<String>,
    pub tls_version: Option<String>,
    pub alpn: Option<String>,
    pub http_status: Option<i64>,
    pub request_bytes: i64,
    pub response_bytes: i64,
    pub packets_out: i64,
    pub packets_in: i64,
    pub dns_ms: Option<f64>,
    pub tcp_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
    pub first_event_ms: Option<f64>,
    pub first_output_ms: Option<f64>,
    pub duration_ms: Option<f64>,
    pub success: Option<bool>,
    pub error_type: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub response_id: Option<String>,
    pub quality: Quality,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSession {
    pub session: SessionRecord,
    pub turns: HashMap<String, TurnRecord>,
    pub llm_calls: Vec<LlmCallRecord>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub malformed_lines: i64,
    pub duplicate_usage_events: i64,
    pub capability_names: HashSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_delta_rejects_accumulator_reset() {
        let old = TokenUsage {
            input_tokens: 20,
            total_tokens: 25,
            ..Default::default()
        };
        let new = TokenUsage {
            input_tokens: 10,
            total_tokens: 12,
            ..Default::default()
        };
        assert_eq!(new.delta(old), None);
    }

    #[test]
    fn cache_miss_excludes_read_and_write_tokens() {
        let usage = TokenUsage {
            input_tokens: 100,
            cached_input_tokens: 60,
            cache_write_tokens: 10,
            ..Default::default()
        };
        assert_eq!(usage.cache_miss_tokens(), 30);
    }
}
