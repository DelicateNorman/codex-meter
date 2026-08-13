//! Streaming, content-excluding Codex Rollout JSONL collector.
//!
//! Every line is discarded immediately after a small metadata whitelist is
//! extracted.  Message bodies, reasoning, commands, tool payloads and output
//! are never represented by a Rust domain type and therefore cannot reach the
//! database layer.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{
    models::{
        Confidence, LlmCallRecord, ParsedSession, Quality, SessionRecord, TokenUsage,
        ToolCallRecord, TurnRecord,
    },
    pricing::PricingCatalog,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectorCapabilities {
    pub sessions: bool,
    pub turns: bool,
    pub per_llm_call: bool,
    pub token_usage: bool,
    pub cache_write: bool,
    pub latency: bool,
    pub tools: bool,
    pub exact_usage: bool,
}

#[derive(Debug, Default, Clone)]
struct Context {
    model: Option<String>,
    effort: Option<String>,
    reasoning_mode: Option<String>,
    service_tier: Option<String>,
    provider: Option<String>,
    turn_id: Option<String>,
}

#[derive(Debug, Default)]
struct SessionMeta {
    id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    repository_url: Option<String>,
    branch: Option<String>,
    cli_version: Option<String>,
    model_provider: Option<String>,
    parent_thread_id: Option<String>,
    agent_role: Option<String>,
    agent_id: Option<String>,
}

pub struct SessionCollector<'a> {
    pricing: &'a PricingCatalog,
}

impl<'a> SessionCollector<'a> {
    pub fn new(pricing: &'a PricingCatalog) -> Self {
        Self { pricing }
    }

    pub const fn capabilities(&self) -> CollectorCapabilities {
        CollectorCapabilities {
            sessions: true,
            turns: true,
            per_llm_call: true,
            token_usage: true,
            cache_write: true,
            latency: true,
            tools: true,
            exact_usage: false,
        }
    }

    pub fn collect_file(&self, path: &Path) -> Result<ParsedSession> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let source_path = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .into_owned();
        self.collect_reader(BufReader::new(file), source_path)
    }

    pub fn collect_reader<R: BufRead>(
        &self,
        mut reader: R,
        source_path: impl Into<String>,
    ) -> Result<ParsedSession> {
        let source_path = source_path.into();
        let mut meta: Option<SessionMeta> = None;
        let mut started_at = None;
        let mut ended_at = None;
        let mut turns: HashMap<String, TurnRecord> = HashMap::new();
        let mut calls = Vec::new();
        let mut completed_tools = Vec::new();
        let mut open_tools: HashMap<String, (String, Option<String>, Option<String>)> =
            HashMap::new();
        let mut active_turn: Option<String> = None;
        let mut latest_context = Context::default();
        let mut previous_total = TokenUsage::default();
        let mut have_total = false;
        let mut malformed_lines = 0_i64;
        let mut duplicate_usage_events = 0_i64;
        let mut capability_names = HashSet::from(["session_jsonl".to_string()]);
        let mut auth_mode = "unknown".to_string();
        let mut exact_usage_waiting: HashMap<(Option<String>, TokenUsage), usize> = HashMap::new();

        let mut raw_line = Vec::new();
        loop {
            raw_line.clear();
            let bytes = reader.read_until(b'\n', &mut raw_line)?;
            if bytes == 0 {
                break;
            }
            let line: Value = match serde_json::from_str(&String::from_utf8_lossy(&raw_line)) {
                Ok(value) => value,
                Err(_) => {
                    malformed_lines += 1;
                    continue;
                }
            };
            let Some(line) = line.as_object() else {
                malformed_lines += 1;
                continue;
            };
            let timestamp = string(line.get("timestamp"));
            if started_at.is_none() {
                started_at.clone_from(&timestamp);
            }
            if timestamp.is_some() {
                ended_at.clone_from(&timestamp);
            }
            let line_type = line.get("type").and_then(Value::as_str);
            let payload = line.get("payload").and_then(Value::as_object);
            let event_type = payload
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str);

            if line_type == Some("session_meta") && meta.is_none() {
                meta = Some(parse_meta(payload));
                continue;
            }

            if line_type == Some("turn_context") {
                let context = parse_context(payload);
                let turn_id = context.turn_id.clone().or_else(|| active_turn.clone());
                if let Some(turn_id) = turn_id {
                    active_turn = Some(turn_id.clone());
                    latest_context = context;
                    let turn = turns
                        .entry(turn_id.clone())
                        .or_insert_with(|| TurnRecord::new(turn_id));
                    apply_context(turn, &latest_context);
                }
                continue;
            }

            if line_type != Some("event_msg") {
                continue;
            }
            let empty = Map::new();
            let payload = payload.unwrap_or(&empty);

            match event_type {
                Some("task_started" | "turn_started") => {
                    let Some(turn_id) = string(payload.get("turn_id")) else {
                        continue;
                    };
                    active_turn = Some(turn_id.clone());
                    let turn = turns
                        .entry(turn_id.clone())
                        .or_insert_with(|| TurnRecord::new(turn_id.clone()));
                    turn.started_at = timestamp
                        .clone()
                        .or_else(|| epoch_to_iso(payload.get("started_at")));
                    turn.status = "running".into();
                    if latest_context.turn_id.as_deref() == Some(&turn_id) {
                        apply_context(turn, &latest_context);
                    }
                    capability_names.insert("turn_timings".into());
                }
                Some("task_complete" | "turn_complete" | "turn_aborted") => {
                    let Some(turn_id) =
                        string(payload.get("turn_id")).or_else(|| active_turn.clone())
                    else {
                        continue;
                    };
                    let turn = turns
                        .entry(turn_id.clone())
                        .or_insert_with(|| TurnRecord::new(turn_id.clone()));
                    turn.completed_at = timestamp
                        .clone()
                        .or_else(|| epoch_to_iso(payload.get("completed_at")));
                    let error = payload.get("error");
                    turn.status = if event_type == Some("turn_aborted") {
                        "aborted"
                    } else if error.is_some_and(|value| !value.is_null()) {
                        "failed"
                    } else {
                        "completed"
                    }
                    .into();
                    turn.error_type = error_type(error);
                    turn.e2e_ms = nonnegative_int(payload.get("duration_ms"));
                    turn.ttft_ms = nonnegative_int(payload.get("time_to_first_token_ms"));
                    if active_turn.as_deref() == Some(&turn_id) {
                        active_turn = None;
                    }
                }
                Some("thread_settings_applied") => {
                    if let Some(turn_id) = active_turn.as_deref() {
                        if let Some(turn) = turns.get_mut(turn_id) {
                            apply_settings(
                                turn,
                                payload.get("thread_settings").and_then(Value::as_object),
                            );
                        }
                    }
                }
                Some("context_compacted") => {
                    capability_names.insert("compaction".into());
                }
                Some("raw_response_completed" | "rawResponse/completed") => {
                    let usage = token_usage(
                        payload
                            .get("token_usage")
                            .filter(|value| !json_falsy(value))
                            .or_else(|| payload.get("usage")),
                    );
                    if usage.is_zero() {
                        continue;
                    }
                    let turn_id = string(payload.get("turn_id")).or_else(|| active_turn.clone());
                    let context = context_for(&turns, turn_id.as_deref(), &latest_context);
                    let response_id = string(payload.get("response_id"));
                    calls.push(self.make_call(
                        timestamp.clone(),
                        turn_id.clone(),
                        response_id.clone(),
                        usage,
                        &context,
                        true,
                        response_id,
                    ));
                    *exact_usage_waiting.entry((turn_id, usage)).or_default() += 1;
                    capability_names.insert("exact_per_response_usage".into());
                }
                Some("token_count") => {
                    let Some(info) = payload.get("info").and_then(Value::as_object) else {
                        continue;
                    };
                    if payload
                        .get("rate_limits")
                        .and_then(Value::as_object)
                        .and_then(|value| value.get("plan_type"))
                        .is_some_and(|value| !value.is_null())
                    {
                        auth_mode = "chatgpt".into();
                    }
                    let total = token_usage(info.get("total_token_usage"));
                    let last = token_usage(info.get("last_token_usage"));
                    let usage = if have_total {
                        total.delta(previous_total).unwrap_or(last)
                    } else if total.is_zero() {
                        last
                    } else {
                        total
                    };
                    if total != previous_total {
                        previous_total = total;
                        have_total = true;
                    }
                    if usage.is_zero() {
                        duplicate_usage_events += 1;
                        continue;
                    }
                    let key = (active_turn.clone(), usage);
                    if let Some(waiting) = exact_usage_waiting.get_mut(&key) {
                        if *waiting > 0 {
                            *waiting -= 1;
                            duplicate_usage_events += 1;
                            continue;
                        }
                    }
                    let context = context_for(&turns, active_turn.as_deref(), &latest_context);
                    calls.push(self.make_call(
                        timestamp.clone(),
                        active_turn.clone(),
                        None,
                        usage,
                        &context,
                        false,
                        Some(usage_identity(active_turn.as_deref(), total, last)),
                    ));
                    capability_names.insert("token_usage".into());
                    if usage.cache_write_tokens != 0 {
                        capability_names.insert("cache_write".into());
                    }
                }
                Some(event) if tool_begin(event).is_some() => {
                    let call_id =
                        string(payload.get("call_id")).or_else(|| string(payload.get("id")));
                    if let Some(call_id) = call_id {
                        let turn_id =
                            string(payload.get("turn_id")).or_else(|| active_turn.clone());
                        let started = ms_or_iso(payload.get("started_at_ms"), timestamp.clone());
                        open_tools.insert(call_id, (tool_name(event, payload), turn_id, started));
                    }
                }
                Some(event) if tool_end(event).is_some() => {
                    let Some(call_id) =
                        string(payload.get("call_id")).or_else(|| string(payload.get("id")))
                    else {
                        continue;
                    };
                    let fallback = (
                        tool_name(event, payload),
                        string(payload.get("turn_id")).or_else(|| active_turn.clone()),
                        None,
                    );
                    let (name, turn_id, tool_started) =
                        open_tools.remove(&call_id).unwrap_or(fallback);
                    let completed = ms_or_iso(payload.get("completed_at_ms"), timestamp.clone());
                    let duration = duration_ms(payload.get("duration"))
                        .or_else(|| time_delta_ms(tool_started.as_deref(), completed.as_deref()));
                    completed_tools.push(ToolCallRecord {
                        call_id,
                        turn_id,
                        tool_name: name,
                        started_at: tool_started,
                        completed_at: completed,
                        duration_ms: duration,
                        success: tool_success(payload),
                        exit_code: optional_int(payload.get("exit_code")),
                        quality: Quality::exact("session_jsonl"),
                    });
                    capability_names.insert("tool_calls".into());
                }
                _ => {}
            }
        }

        let Some(meta) = meta else {
            bail!("{source_path}: no session_meta record");
        };
        let Some(session_id) = meta.id else {
            bail!("{source_path}: session_meta has no id");
        };
        let project_name = meta.cwd.as_deref().and_then(|cwd| {
            Path::new(cwd)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
        });
        Ok(ParsedSession {
            session: SessionRecord {
                codex_thread_id: session_id,
                started_at: meta.timestamp.or(started_at),
                ended_at,
                cwd: meta.cwd,
                project_name,
                git_repo: meta.repository_url,
                git_branch: meta.branch,
                auth_mode,
                codex_version: meta.cli_version,
                provider: meta.model_provider,
                source_path,
                parent_thread_id: meta.parent_thread_id,
                agent_role: meta.agent_role,
                agent_id: meta.agent_id,
            },
            turns,
            llm_calls: calls,
            tool_calls: completed_tools,
            malformed_lines,
            duplicate_usage_events,
            capability_names,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn make_call(
        &self,
        timestamp: Option<String>,
        turn_id: Option<String>,
        response_id: Option<String>,
        usage: TokenUsage,
        context: &Context,
        exact: bool,
        fingerprint_basis: Option<String>,
    ) -> LlmCallRecord {
        let provider = context.provider.as_deref().unwrap_or("openai");
        let fingerprint_value = fingerprint_basis.unwrap_or_else(|| {
            [
                py_string(timestamp.as_deref()),
                py_string(turn_id.as_deref()),
                usage.input_tokens.to_string(),
                usage.cached_input_tokens.to_string(),
                usage.cache_write_tokens.to_string(),
                usage.output_tokens.to_string(),
                usage.reasoning_tokens.to_string(),
                usage.total_tokens.to_string(),
            ]
            .join("|")
        });
        let event_fingerprint = format!("{:x}", Sha256::digest(fingerprint_value.as_bytes()));
        let resolved_price = self.pricing.resolve_for_estimate(
            context.model.as_deref(),
            Some(provider),
            timestamp.as_deref(),
        );
        let historical_price_estimate =
            resolved_price.is_some_and(|resolution| resolution.historical_estimate);
        let cost = resolved_price.map(|resolution| self.pricing.calculate(usage, resolution.price));
        let pricing_version = resolved_price.map(|resolution| resolution.version());
        LlmCallRecord {
            event_fingerprint,
            turn_id,
            response_id,
            completed_at: timestamp,
            model: context.model.clone(),
            actual_model: None,
            provider: Some(provider.to_string()),
            reasoning_effort: context.effort.clone(),
            reasoning_mode: context.reasoning_mode.clone(),
            service_tier: context.service_tier.clone(),
            usage,
            success: true,
            error_type: None,
            retry_index: 0,
            cost_usd: cost.as_ref().map(|value| value.total_usd),
            pricing_version,
            quality: Quality {
                source: if exact {
                    "app_server_raw_response"
                } else {
                    "session_jsonl_cumulative_delta"
                }
                .into(),
                confidence: if exact {
                    Confidence::Exact
                } else {
                    Confidence::Derived
                },
                estimated: !exact || historical_price_estimate,
            },
        }
    }
}

pub fn discover_rollouts(root: &Path) -> Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    let mut paths = Vec::new();
    if !root.exists() {
        return Ok(paths);
    }
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {}", root.display()))?;
        if entry.file_type().is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        {
            paths.push(entry.into_path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn parse_meta(payload: Option<&Map<String, Value>>) -> SessionMeta {
    let Some(payload) = payload else {
        return SessionMeta::default();
    };
    let git = payload.get("git").and_then(Value::as_object);
    let subagent = payload
        .get("source")
        .and_then(Value::as_object)
        .and_then(|value| value.get("subagent"))
        .and_then(Value::as_object);
    SessionMeta {
        id: string(payload.get("session_id")).or_else(|| string(payload.get("id"))),
        timestamp: string(payload.get("timestamp")),
        cwd: string(payload.get("cwd")),
        repository_url: git.and_then(|value| string(value.get("repository_url"))),
        branch: git.and_then(|value| string(value.get("branch"))),
        cli_version: string(payload.get("cli_version")),
        model_provider: string(payload.get("model_provider")),
        parent_thread_id: string(payload.get("parent_thread_id"))
            .or_else(|| string(payload.get("forked_from_id")))
            .or_else(|| subagent.and_then(|value| string(value.get("parent_thread_id")))),
        agent_role: string(payload.get("agent_role"))
            .or_else(|| subagent.and_then(|value| string(value.get("agent_role")))),
        agent_id: string(payload.get("agent_id"))
            .or_else(|| subagent.and_then(|value| string(value.get("agent_id")))),
    }
}

fn parse_context(payload: Option<&Map<String, Value>>) -> Context {
    let Some(payload) = payload else {
        return Context::default();
    };
    Context {
        model: string(payload.get("model")),
        effort: string(payload.get("effort")).or_else(|| string(payload.get("reasoning_effort"))),
        reasoning_mode: string(payload.get("reasoning_mode")),
        service_tier: string(payload.get("service_tier")),
        provider: string(payload.get("provider")),
        turn_id: string(payload.get("turn_id")),
    }
}

fn apply_context(turn: &mut TurnRecord, context: &Context) {
    if context.model.is_some() {
        turn.model.clone_from(&context.model);
    }
    if context.effort.is_some() {
        turn.reasoning_effort.clone_from(&context.effort);
    }
    if context.reasoning_mode.is_some() {
        turn.reasoning_mode.clone_from(&context.reasoning_mode);
    }
}

fn apply_settings(turn: &mut TurnRecord, settings: Option<&Map<String, Value>>) {
    let Some(settings) = settings else { return };
    if let Some(value) = string(settings.get("model")) {
        turn.model = Some(value);
    }
    if let Some(value) = string(settings.get("reasoning_effort")) {
        turn.reasoning_effort = Some(value);
    }
    if let Some(value) = string(settings.get("reasoning_mode")) {
        turn.reasoning_mode = Some(value);
    }
    if let Some(value) = string(settings.get("service_tier")) {
        turn.service_tier = Some(value);
    }
}

fn context_for(
    turns: &HashMap<String, TurnRecord>,
    turn_id: Option<&str>,
    latest: &Context,
) -> Context {
    if let Some(turn) = turn_id.and_then(|id| turns.get(id)) {
        Context {
            model: turn.model.clone(),
            effort: turn.reasoning_effort.clone(),
            reasoning_mode: turn.reasoning_mode.clone(),
            service_tier: turn.service_tier.clone(),
            provider: latest.provider.clone(),
            turn_id: turn_id.map(str::to_string),
        }
    } else {
        latest.clone()
    }
}

fn token_usage(value: Option<&Value>) -> TokenUsage {
    let Some(value) = value.and_then(Value::as_object) else {
        return TokenUsage::default();
    };
    TokenUsage {
        input_tokens: mapping_int(value, &["input_tokens", "inputTokens"]),
        cached_input_tokens: mapping_int(value, &["cached_input_tokens", "cachedInputTokens"]),
        cache_write_tokens: mapping_int(
            value,
            &[
                "cache_write_input_tokens",
                "cacheWriteInputTokens",
                "cache_write_tokens",
            ],
        ),
        output_tokens: mapping_int(value, &["output_tokens", "outputTokens"]),
        reasoning_tokens: mapping_int(value, &["reasoning_output_tokens", "reasoningOutputTokens"]),
        total_tokens: mapping_int(value, &["total_tokens", "totalTokens"]),
    }
}

fn mapping_int(value: &Map<String, Value>, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| optional_int(value.get(*key)))
        .unwrap_or(0)
        .max(0)
}

fn string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn json_falsy(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(value) => !value,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        Value::Number(value) => value.as_f64() == Some(0.0),
    }
}

fn optional_int(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| number.as_f64().map(|value| value as i64)),
        Value::String(value) => value.parse::<f64>().ok().map(|value| value as i64),
        Value::Bool(value) => Some(i64::from(*value)),
        _ => None,
    }
}

fn nonnegative_int(value: Option<&Value>) -> Option<i64> {
    optional_int(value).map(|value| value.max(0))
}

fn epoch_to_iso(value: Option<&Value>) -> Option<String> {
    let seconds = optional_int(value)?;
    Utc.timestamp_opt(seconds, 0)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn ms_or_iso(value: Option<&Value>, fallback: Option<String>) -> Option<String> {
    let Some(milliseconds) = optional_int(value).filter(|value| *value > 0) else {
        return fallback;
    };
    Utc.timestamp_millis_opt(milliseconds)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
        .or(fallback)
}

fn duration_ms(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    let milliseconds = if let Some(value) = value.as_object() {
        let seconds = numeric(value.get("secs").or_else(|| value.get("seconds"))).unwrap_or(0.0);
        let nanos = numeric(value.get("nanos").or_else(|| value.get("nanoseconds"))).unwrap_or(0.0);
        seconds * 1_000.0 + nanos / 1_000_000.0
    } else {
        numeric(Some(value))? * 1_000.0
    };
    Some(milliseconds.round().max(0.0) as i64)
}

fn numeric(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn time_delta_ms(start: Option<&str>, end: Option<&str>) -> Option<i64> {
    let start = DateTime::parse_from_rfc3339(start?).ok()?;
    let end = DateTime::parse_from_rfc3339(end?).ok()?;
    Some((end - start).num_milliseconds().max(0))
}

fn error_type(value: Option<&Value>) -> Option<String> {
    let value = value?.as_object()?;
    string(
        value
            .get("codex_error_info")
            .or_else(|| value.get("type"))
            .or_else(|| value.get("code")),
    )
    .or_else(|| Some("error".into()))
}

fn tool_begin(value: &str) -> Option<&'static str> {
    match value {
        "exec_command_begin" => Some("shell"),
        "patch_apply_begin" => Some("apply_patch"),
        "mcp_tool_call_begin" => Some("mcp"),
        "web_search_begin" => Some("web_search"),
        _ => None,
    }
}

fn tool_end(value: &str) -> Option<&'static str> {
    match value {
        "exec_command_end" => Some("shell"),
        "patch_apply_end" => Some("apply_patch"),
        "mcp_tool_call_end" => Some("mcp"),
        "web_search_end" => Some("web_search"),
        _ => None,
    }
}

fn tool_name(event: &str, payload: &Map<String, Value>) -> String {
    let base = tool_begin(event)
        .or_else(|| tool_end(event))
        .unwrap_or(event);
    if base != "mcp" {
        return base.to_string();
    }
    let invocation = payload.get("invocation").and_then(Value::as_object);
    [
        Some("mcp".to_string()),
        invocation.and_then(|value| string(value.get("server"))),
        invocation.and_then(|value| string(value.get("tool"))),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("/")
}

fn tool_success(payload: &Map<String, Value>) -> Option<bool> {
    if let Some(value) = payload.get("success").and_then(Value::as_bool) {
        return Some(value);
    }
    if payload
        .get("exit_code")
        .is_some_and(|value| !value.is_null())
    {
        return optional_int(payload.get("exit_code")).map(|value| value == 0);
    }
    if let Some(status) = payload.get("status").and_then(Value::as_str) {
        return Some(matches!(status, "completed" | "success"));
    }
    payload
        .get("result")
        .and_then(Value::as_object)
        .map(|result| !result.contains_key("Err") && !result.contains_key("error"))
}

fn usage_identity(turn_id: Option<&str>, total: TokenUsage, last: TokenUsage) -> String {
    let values = |usage: TokenUsage| {
        [
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.cache_write_tokens,
            usage.output_tokens,
            usage.reasoning_tokens,
            usage.total_tokens,
        ]
    };
    serde_json::to_string(&(turn_id, values(total), values(last)))
        .expect("serializable usage identity")
}

fn py_string(value: Option<&str>) -> String {
    value.unwrap_or("None").to_string()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::json;

    use super::*;

    fn catalog() -> PricingCatalog {
        PricingCatalog::bundled().unwrap()
    }

    #[test]
    fn parses_only_metadata_and_suppresses_exact_usage_duplicate() {
        let secret = "TOP-SECRET-PROMPT";
        let input = format!(
            concat!(
                "{{\"timestamp\":\"2026-08-12T01:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"thread-1\",\"cwd\":\"/work/demo\",\"model_provider\":\"openai\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T01:00:01Z\",\"type\":\"turn_context\",\"payload\":{{\"turn_id\":\"turn-1\",\"model\":\"gpt-5.6-sol\",\"effort\":\"high\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T01:00:02Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"content\":\"{}\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T01:00:03Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"raw_response_completed\",\"turn_id\":\"turn-1\",\"response_id\":\"resp-1\",\"token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":10,\"total_tokens\":110}}}}}}\n",
                "{{\"timestamp\":\"2026-08-12T01:00:04Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":10,\"total_tokens\":110}},\"last_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":10,\"total_tokens\":110}}}}}}}}\n"
            ),
            secret
        );
        let parsed = SessionCollector::new(&catalog())
            .collect_reader(Cursor::new(input), "memory://fixture")
            .unwrap();
        assert_eq!(parsed.session.project_name.as_deref(), Some("demo"));
        assert_eq!(parsed.llm_calls.len(), 1);
        assert_eq!(parsed.duplicate_usage_events, 1);
        assert!(!format!("{parsed:?}").contains(secret));
    }

    #[test]
    fn cumulative_identity_is_stable_across_replayed_timestamps() {
        fn rollout(timestamp: &str) -> String {
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"t\"}}}}\n{{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":2,\"total_tokens\":3}},\"last_token_usage\":{{\"input_tokens\":2,\"total_tokens\":3}}}}}}}}\n"
            )
        }
        let catalog = catalog();
        let collector = SessionCollector::new(&catalog);
        let a = collector
            .collect_reader(Cursor::new(rollout("2026-01-01T00:00:00Z")), "a")
            .unwrap();
        let b = collector
            .collect_reader(Cursor::new(rollout("2026-02-01T00:00:00Z")), "b")
            .unwrap();
        assert_eq!(
            a.llm_calls[0].event_fingerprint,
            b.llm_calls[0].event_fingerprint
        );
    }

    #[test]
    fn invalid_utf8_in_ignored_content_is_lossy_not_fatal() {
        let mut input = br#"{"type":"session_meta","payload":{"id":"thread"}}
{"type":"response_item","payload":{"content":""#
            .to_vec();
        input.push(0xff);
        input.extend_from_slice(br#""}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1,"total_tokens":1},"last_token_usage":{"input_tokens":1,"total_tokens":1}}}}
"#);
        let catalog = catalog();
        let parsed = SessionCollector::new(&catalog)
            .collect_reader(Cursor::new(input), "invalid-utf8")
            .unwrap();
        assert_eq!(parsed.llm_calls.len(), 1);
        assert!(!format!("{parsed:?}").contains('\u{fffd}'));
    }

    #[test]
    fn cumulative_deltas_tools_timings_and_malformed_lines_match_python() {
        let usage = |input, cached, write, output, reasoning, total| {
            json!({
                "input_tokens": input,
                "cached_input_tokens": cached,
                "cache_write_input_tokens": write,
                "output_tokens": output,
                "reasoning_output_tokens": reasoning,
                "total_tokens": total,
            })
        };
        let token = |timestamp: &str, total: Value, last: Value| {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {"total_token_usage": total, "last_token_usage": last},
                    "rate_limits": {"plan_type": "pro"},
                }
            })
        };
        let first = usage(100, 60, 0, 10, 4, 110);
        let last = usage(150, 100, 10, 20, 8, 170);
        let events = vec![
            json!({"timestamp":"2026-08-12T00:00:00Z","type":"session_meta","payload":{"id":"thread-1","cwd":"/work/project"}}),
            json!({"timestamp":"2026-08-12T00:00:01Z","type":"turn_context","payload":{"turn_id":"turn-1","model":"gpt-5.6-sol","effort":"high"}}),
            json!({"timestamp":"2026-08-12T00:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}),
            token("2026-08-12T00:00:03Z", first.clone(), first.clone()),
            token("2026-08-12T00:00:04Z", first.clone(), first),
            token(
                "2026-08-12T00:00:05Z",
                usage(250, 160, 10, 30, 12, 280),
                last,
            ),
            json!({"timestamp":"2026-08-12T00:00:06Z","type":"event_msg","payload":{"type":"exec_command_begin","call_id":"tool-1","turn_id":"turn-1","started_at_ms":1786492806000_i64,"command":"must-not-survive"}}),
            json!({"timestamp":"2026-08-12T00:00:07Z","type":"event_msg","payload":{"type":"exec_command_end","call_id":"tool-1","completed_at_ms":1786492807000_i64,"exit_code":0,"stdout":"must-not-survive"}}),
            json!({"timestamp":"2026-08-12T00:00:10Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1","duration_ms":8000,"time_to_first_token_ms":2200}}),
        ];
        let mut input = events
            .into_iter()
            .map(|event| event.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        input.push_str("\n{broken\n");
        let catalog = catalog();
        let parsed = SessionCollector::new(&catalog)
            .collect_reader(Cursor::new(input), "fixture")
            .unwrap();
        assert_eq!(parsed.llm_calls.len(), 2);
        assert_eq!(
            parsed
                .llm_calls
                .iter()
                .map(|call| call.usage.input_tokens)
                .sum::<i64>(),
            250
        );
        assert_eq!(
            parsed
                .llm_calls
                .iter()
                .map(|call| call.usage.cached_input_tokens)
                .sum::<i64>(),
            160
        );
        assert_eq!(parsed.duplicate_usage_events, 1);
        assert_eq!(parsed.malformed_lines, 1);
        assert_eq!(parsed.session.auth_mode, "chatgpt");
        assert_eq!(parsed.turns["turn-1"].ttft_ms, Some(2_200));
        assert_eq!(parsed.tool_calls[0].duration_ms, Some(1_000));
        assert!(!format!("{parsed:?}").contains("must-not-survive"));
    }
}
