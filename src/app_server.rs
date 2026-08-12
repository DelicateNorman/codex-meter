//! Privacy-preserving Codex App Server adapter and transparent stdio proxy.

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
}

impl AppUsage {
    fn from_value(value: Option<&Value>) -> Self {
        let value = value.and_then(Value::as_object);
        let integer = |snake: &str, camel: &str| {
            value
                .and_then(|map| map.get(snake).or_else(|| map.get(camel)))
                .and_then(as_u64)
                .unwrap_or(0)
        };
        Self {
            input_tokens: integer("input_tokens", "inputTokens"),
            cached_input_tokens: integer("cached_input_tokens", "cachedInputTokens"),
            cache_write_tokens: integer("cache_write_input_tokens", "cacheWriteInputTokens"),
            output_tokens: integer("output_tokens", "outputTokens"),
            reasoning_tokens: integer("reasoning_output_tokens", "reasoningOutputTokens"),
            total_tokens: integer("total_tokens", "totalTokens"),
        }
    }
    fn delta(self, previous: Self) -> Option<Self> {
        Some(Self {
            input_tokens: self.input_tokens.checked_sub(previous.input_tokens)?,
            cached_input_tokens: self
                .cached_input_tokens
                .checked_sub(previous.cached_input_tokens)?,
            cache_write_tokens: self
                .cache_write_tokens
                .checked_sub(previous.cache_write_tokens)?,
            output_tokens: self.output_tokens.checked_sub(previous.output_tokens)?,
            reasoning_tokens: self
                .reasoning_tokens
                .checked_sub(previous.reasoning_tokens)?,
            total_tokens: self.total_tokens.checked_sub(previous.total_tokens)?,
        })
    }
    fn is_zero(self) -> bool {
        self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    Session {
        thread_id: String,
        started_at: String,
        cwd: Option<String>,
        model: Option<String>,
    },
    Turn {
        thread_id: String,
        turn_id: String,
        started_at: Option<String>,
        completed_at: Option<String>,
        status: String,
        model: Option<String>,
        effort: Option<String>,
        ttft_ms: Option<u64>,
        ttfm_ms: Option<u64>,
        e2e_ms: Option<u64>,
    },
    Call {
        fingerprint: String,
        thread_id: String,
        turn_id: Option<String>,
        response_id: String,
        completed_at: String,
        model: Option<String>,
        effort: Option<String>,
        usage: AppUsage,
        started_at: Option<String>,
        first_event_at: Option<String>,
        first_message_at: Option<String>,
    },
    Tool {
        thread_id: String,
        turn_id: Option<String>,
        call_id: String,
        tool_name: String,
        started_at: Option<String>,
        completed_at: Option<String>,
        duration_ms: Option<u64>,
        success: Option<bool>,
        exit_code: Option<i64>,
    },
    Compaction {
        fingerprint: String,
        thread_id: String,
        turn_id: Option<String>,
        occurred_at: String,
    },
    ActualModel {
        turn_id: String,
        model: String,
    },
    TurnUsage {
        thread_id: String,
        turn_id: String,
        usage: AppUsage,
    },
}

#[derive(Default)]
pub struct Adapter {
    turn_started: HashMap<String, String>,
    item_started: HashMap<String, String>,
    turn_thread: HashMap<String, String>,
    turn_model: HashMap<String, String>,
    turn_effort: HashMap<String, String>,
    turn_first_event: HashMap<String, String>,
    turn_first_message: HashMap<String, String>,
    call_phase_started: HashMap<String, String>,
    pending: HashMap<String, (String, SafeContext)>,
    thread_totals: HashMap<String, AppUsage>,
}

#[derive(Debug, Clone, Default)]
struct SafeContext {
    thread_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

impl Adapter {
    pub fn ingest(&mut self, message: &Value, direction: Direction) -> Vec<LiveEvent> {
        let Some(envelope) = message.as_object() else {
            return vec![];
        };
        let observed = event_time(envelope);
        let method = envelope.get("method").and_then(Value::as_str).unwrap_or("");
        let params = envelope
            .get("params")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if direction == Direction::Client && !method.is_empty() {
            if matches!(
                method,
                "thread/start" | "thread/resume" | "thread/fork" | "turn/start"
            ) {
                if let Some(id) = envelope.get("id") {
                    self.pending
                        .insert(value_string(id), (method.into(), safe_context(&params)));
                }
            }
            return vec![];
        }
        if method.is_empty() && envelope.contains_key("id") {
            return self.handle_response(
                &value_string(&envelope["id"]),
                envelope.get("result"),
                &observed,
            );
        }
        match method {
            "thread/started" => {
                let thread = params
                    .get("thread")
                    .and_then(Value::as_object)
                    .unwrap_or(&params);
                let id = get_string(thread, "id").or_else(|| get_string(&params, "threadId"));
                id.map(|thread_id| {
                    vec![LiveEvent::Session {
                        thread_id,
                        started_at: timestamp(thread.get("createdAt")).unwrap_or(observed),
                        cwd: safe_path(thread.get("cwd")),
                        model: safe_identifier(thread.get("model")),
                    }]
                })
                .unwrap_or_default()
            }
            "turn/started" => {
                let turn = params
                    .get("turn")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let id = get_string(&turn, "id").or_else(|| get_string(&params, "turnId"));
                let thread = get_string(&params, "threadId")
                    .or_else(|| get_string(&turn, "threadId"))
                    .or_else(|| id.as_ref().and_then(|id| self.turn_thread.get(id).cloned()));
                match (id, thread) {
                    (Some(turn_id), Some(thread_id)) => {
                        self.turn_started.insert(turn_id.clone(), observed.clone());
                        self.call_phase_started
                            .insert(turn_id.clone(), observed.clone());
                        self.turn_thread.insert(turn_id.clone(), thread_id.clone());
                        vec![self.turn_event(
                            thread_id,
                            turn_id,
                            Some(observed),
                            None,
                            "running",
                            None,
                            None,
                            None,
                        )]
                    }
                    _ => vec![],
                }
            }
            "turn/completed" => {
                let turn = params
                    .get("turn")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let id = get_string(&turn, "id").or_else(|| get_string(&params, "turnId"));
                let thread = get_string(&params, "threadId")
                    .or_else(|| get_string(&turn, "threadId"))
                    .or_else(|| id.as_ref().and_then(|id| self.turn_thread.get(id).cloned()));
                match (id, thread) {
                    (Some(turn_id), Some(thread_id)) => {
                        let started = self.turn_started.get(&turn_id).cloned();
                        let e2e = duration(started.as_deref(), Some(&observed));
                        vec![self.turn_event(
                            thread_id,
                            turn_id,
                            started,
                            Some(observed),
                            &turn_status(turn.get("status")),
                            None,
                            None,
                            e2e,
                        )]
                    }
                    _ => vec![],
                }
            }
            "rawResponse/completed" => self.raw_response(&params, &observed).into_iter().collect(),
            "thread/tokenUsage/updated" => self.token_usage(&params).into_iter().collect(),
            "item/started" | "item/completed" => self.item(method, &params, &observed),
            "thread/compacted" | "compacted" => self
                .compaction(&params, method, &observed)
                .into_iter()
                .collect(),
            "model/rerouted" => match (
                get_string(&params, "turnId"),
                safe_identifier(params.get("toModel")),
            ) {
                (Some(turn_id), Some(model)) => vec![LiveEvent::ActualModel { turn_id, model }],
                _ => vec![],
            },
            "item/agentMessage/delta" => {
                let Some(turn_id) = get_string(&params, "turnId") else {
                    return vec![];
                };
                if self.turn_first_message.contains_key(&turn_id) {
                    return vec![];
                };
                self.turn_first_message
                    .insert(turn_id.clone(), observed.clone());
                let Some(thread_id) = get_string(&params, "threadId")
                    .or_else(|| self.turn_thread.get(&turn_id).cloned())
                else {
                    return vec![];
                };
                let ttfm = duration(
                    self.turn_started.get(&turn_id).map(String::as_str),
                    Some(&observed),
                );
                vec![self.turn_event(thread_id, turn_id, None, None, "running", None, ttfm, None)]
            }
            _ => vec![],
        }
    }

    fn handle_response(
        &mut self,
        id: &str,
        result: Option<&Value>,
        observed: &str,
    ) -> Vec<LiveEvent> {
        let Some((method, context)) = self.pending.remove(id) else {
            return vec![];
        };
        let Some(result) = result.and_then(Value::as_object) else {
            return vec![];
        };
        if method.starts_with("thread/") {
            let thread = result
                .get("thread")
                .and_then(Value::as_object)
                .unwrap_or(result);
            get_string(thread, "id")
                .map(|thread_id| {
                    vec![LiveEvent::Session {
                        thread_id,
                        started_at: observed.into(),
                        cwd: context.cwd,
                        model: context.model,
                    }]
                })
                .unwrap_or_default()
        } else if method == "turn/start" {
            let turn = result
                .get("turn")
                .and_then(Value::as_object)
                .unwrap_or(result);
            let (Some(turn_id), Some(thread_id)) = (get_string(turn, "id"), context.thread_id)
            else {
                return vec![];
            };
            self.turn_thread.insert(turn_id.clone(), thread_id.clone());
            if let Some(model) = context.model {
                self.turn_model.insert(turn_id.clone(), model);
            }
            if let Some(effort) = context.effort {
                self.turn_effort.insert(turn_id.clone(), effort);
            }
            vec![self.turn_event(
                thread_id,
                turn_id,
                Some(observed.into()),
                None,
                "running",
                None,
                None,
                None,
            )]
        } else {
            vec![]
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn turn_event(
        &self,
        thread_id: String,
        turn_id: String,
        started_at: Option<String>,
        completed_at: Option<String>,
        status: &str,
        ttft_ms: Option<u64>,
        ttfm_ms: Option<u64>,
        e2e_ms: Option<u64>,
    ) -> LiveEvent {
        LiveEvent::Turn {
            model: self.turn_model.get(&turn_id).cloned(),
            effort: self.turn_effort.get(&turn_id).cloned(),
            thread_id,
            turn_id,
            started_at,
            completed_at,
            status: status.into(),
            ttft_ms,
            ttfm_ms,
            e2e_ms,
        }
    }
    fn raw_response(&mut self, p: &Map<String, Value>, observed: &str) -> Option<LiveEvent> {
        let thread_id = get_string(p, "threadId")?;
        let turn_id = get_string(p, "turnId");
        let response_id = get_string(p, "responseId")?;
        let usage_value = p.get("usage")?.as_object()?;
        let usage = AppUsage::from_value(p.get("usage"));
        let basis = serde_json::json!(["raw", thread_id, turn_id, response_id, usage_value]);
        let fingerprint = metadata_fingerprint(&basis)?;
        let started_at = turn_id
            .as_ref()
            .and_then(|id| self.call_phase_started.get(id).cloned());
        let first_event_at = turn_id
            .as_ref()
            .and_then(|id| self.turn_first_event.remove(id));
        let first_message_at = turn_id
            .as_ref()
            .and_then(|id| self.turn_first_message.remove(id));
        let model = turn_id
            .as_ref()
            .and_then(|id| self.turn_model.get(id).cloned());
        let effort = turn_id
            .as_ref()
            .and_then(|id| self.turn_effort.get(id).cloned());
        if let Some(id) = &turn_id {
            self.call_phase_started.insert(id.clone(), observed.into());
        }
        Some(LiveEvent::Call {
            fingerprint,
            thread_id,
            turn_id,
            response_id,
            completed_at: observed.into(),
            model,
            effort,
            usage,
            started_at,
            first_event_at,
            first_message_at,
        })
    }
    fn token_usage(&mut self, p: &Map<String, Value>) -> Option<LiveEvent> {
        let thread_id = get_string(p, "threadId")?;
        let turn_id = get_string(p, "turnId")?;
        let total = AppUsage::from_value(p.get("tokenUsage").and_then(|usage| usage.get("total")));
        if total.is_zero() {
            return None;
        };
        let previous = self
            .thread_totals
            .insert(thread_id.clone(), total)
            .unwrap_or_default();
        let delta = total.delta(previous)?;
        (!delta.is_zero()).then_some(LiveEvent::TurnUsage {
            thread_id,
            turn_id,
            usage: delta,
        })
    }
    fn item(&mut self, method: &str, p: &Map<String, Value>, observed: &str) -> Vec<LiveEvent> {
        let item = p
            .get("item")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let item_id = get_string(&item, "id").or_else(|| get_string(p, "itemId"));
        let turn_id = get_string(p, "turnId").or_else(|| get_string(&item, "turnId"));
        let thread_id = get_string(p, "threadId")
            .or_else(|| get_string(&item, "threadId"))
            .or_else(|| {
                turn_id
                    .as_ref()
                    .and_then(|id| self.turn_thread.get(id).cloned())
            });
        let mut output = vec![];
        if let Some(id) = &turn_id {
            if !self.turn_first_event.contains_key(id) {
                self.turn_first_event.insert(id.clone(), observed.into());
                if let Some(thread) = &thread_id {
                    let ttft = duration(
                        self.turn_started.get(id).map(String::as_str),
                        Some(observed),
                    );
                    output.push(self.turn_event(
                        thread.clone(),
                        id.clone(),
                        None,
                        None,
                        "running",
                        ttft,
                        None,
                        None,
                    ));
                }
            }
        }
        let item_type = get_string(&item, "type").unwrap_or_default();
        if item_type == "contextCompaction" && method == "item/completed" {
            if let Some(event) = self.compaction(p, &item_type, observed) {
                output.push(event);
            }
            return output;
        }
        let Some(tool_name) = tool_name(&item_type, &item) else {
            return output;
        };
        let (Some(call_id), Some(thread_id)) = (item_id, thread_id) else {
            return output;
        };
        if method == "item/started" {
            self.item_started.insert(call_id.clone(), observed.into());
            output.push(LiveEvent::Tool {
                thread_id,
                turn_id,
                call_id,
                tool_name,
                started_at: Some(observed.into()),
                completed_at: None,
                duration_ms: None,
                success: None,
                exit_code: None,
            });
        } else {
            let started = self.item_started.get(&call_id).cloned();
            let status = get_string(&item, "status").unwrap_or_default();
            let duration_ms = item
                .get("durationMs")
                .and_then(as_u64)
                .or_else(|| duration(started.as_deref(), Some(observed)));
            let exit_code = item.get("exitCode").and_then(as_i64);
            if let Some(id) = &turn_id {
                self.call_phase_started.insert(id.clone(), observed.into());
            }
            output.push(LiveEvent::Tool {
                thread_id,
                turn_id,
                call_id,
                tool_name,
                started_at: started,
                completed_at: Some(observed.into()),
                duration_ms,
                success: Some(matches!(status.as_str(), "completed" | "success")),
                exit_code,
            });
        }
        output
    }
    fn compaction(
        &self,
        p: &Map<String, Value>,
        method: &str,
        observed: &str,
    ) -> Option<LiveEvent> {
        let thread_id = get_string(p, "threadId")?;
        let turn_id = get_string(p, "turnId");
        let basis = serde_json::json!([
            "compaction",
            thread_id,
            turn_id,
            p.get("itemId")
                .map(value_string)
                .unwrap_or_else(|| method.into()),
            observed
        ]);
        Some(LiveEvent::Compaction {
            fingerprint: metadata_fingerprint(&basis)?,
            thread_id,
            turn_id,
            occurred_at: observed.into(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Client,
    Server,
}
pub fn ingest_stream<R: BufRead>(
    reader: R,
    adapter: &mut Adapter,
    direction: Direction,
) -> (Vec<LiveEvent>, usize) {
    let mut events = vec![];
    let mut malformed = 0;
    for line in reader.lines() {
        match line.ok().and_then(|line| serde_json::from_str(&line).ok()) {
            Some(value) => events.extend(adapter.ingest(&value, direction)),
            None => malformed += 1,
        }
    }
    (events, malformed)
}

pub fn proxy_stdio(
    command: &[String],
    sink: Arc<Mutex<dyn FnMut(LiveEvent) + Send>>,
) -> Result<i32> {
    let mut child =
        crate::process_command::command(command.first().context("empty App Server command")?)
            .args(&command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
    let mut child_input = child.stdin.take().unwrap();
    let child_output = child.stdout.take().unwrap();
    let sink_input = Arc::clone(&sink);
    let adapter = Arc::new(Mutex::new(Adapter::default()));
    let input_adapter = Arc::clone(&adapter);
    let _input = thread::spawn(move || {
        pump(
            std::io::stdin().lock(),
            &mut child_input,
            Direction::Client,
            sink_input,
            input_adapter,
            true,
        )
    });
    pump(
        BufReader::new(child_output),
        &mut std::io::stdout().lock(),
        Direction::Server,
        sink,
        adapter,
        false,
    )?;
    Ok(child.wait()?.code().unwrap_or(1))
}
fn pump<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    direction: Direction,
    sink: Arc<Mutex<dyn FnMut(LiveEvent) + Send>>,
    adapter: Arc<Mutex<Adapter>>,
    augment: bool,
) -> Result<()> {
    for line in reader.lines() {
        let line = line?;
        let mut bytes = line.into_bytes();
        if let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) {
            let events = adapter
                .lock()
                .map_err(|_| anyhow::anyhow!("App Server adapter lock poisoned"))?
                .ingest(&value, direction);
            if !events.is_empty() {
                let mut handler = sink
                    .lock()
                    .map_err(|_| anyhow::anyhow!("App Server event sink lock poisoned"))?;
                for event in events {
                    handler(event);
                }
            }
            if augment
                && direction == Direction::Client
                && value.get("method").and_then(Value::as_str) == Some("thread/start")
            {
                if let Some(params) = value.get_mut("params").and_then(Value::as_object_mut) {
                    params
                        .entry("experimentalRawEvents")
                        .or_insert(Value::Bool(true));
                    bytes = serde_json::to_vec(&value)?;
                }
            }
        }
        writer.write_all(&bytes)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    Ok(())
}

fn safe_context(p: &Map<String, Value>) -> SafeContext {
    let settings = p.get("settings").and_then(Value::as_object);
    SafeContext {
        thread_id: safe_identifier(p.get("threadId")),
        cwd: safe_path(p.get("cwd")),
        model: safe_identifier(p.get("model")),
        effort: safe_identifier(
            p.get("effort")
                .or_else(|| settings.and_then(|s| s.get("reasoningEffort"))),
        ),
    }
}
fn safe_identifier(v: Option<&Value>) -> Option<String> {
    let value = v?.as_str()?;
    (!value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || "._:/-@".contains(c)))
    .then(|| value.into())
}
fn safe_path(v: Option<&Value>) -> Option<String> {
    let value = v?.as_str()?;
    (!value.is_empty() && value.len() <= 4096 && !value.contains('\0'))
        .then(|| std::path::Path::new(value).to_string_lossy().into_owned())
}
fn get_string(p: &Map<String, Value>, key: &str) -> Option<String> {
    p.get(key).and_then(metadata_scalar)
}
fn value_string(v: &Value) -> String {
    metadata_scalar(v).unwrap_or_default()
}
fn metadata_scalar(v: &Value) -> Option<String> {
    let value = match v {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        _ => return None,
    };
    (!value.is_empty() && value.len() <= 256 && !value.contains(['\r', '\n', '\0']))
        .then_some(value)
}
fn as_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}
fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}
fn event_time(m: &Map<String, Value>) -> String {
    m.get("emittedAtMs")
        .and_then(numeric)
        .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms as i64))
        .map(|d| d.to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_else(|| Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true))
}
fn timestamp(v: Option<&Value>) -> Option<String> {
    let mut numeric = numeric(v?)?;
    if numeric > 10_000_000_000.0 {
        numeric /= 1000.0
    }
    chrono::DateTime::from_timestamp_millis((numeric * 1000.0) as i64)
        .map(|d| d.to_rfc3339_opts(SecondsFormat::Millis, true))
}
fn duration(start: Option<&str>, end: Option<&str>) -> Option<u64> {
    let (start, end) = (start?, end?);
    let a = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let b = chrono::DateTime::parse_from_rfc3339(end).ok()?;
    Some((b - a).num_milliseconds().max(0) as u64)
}
fn turn_status(v: Option<&Value>) -> String {
    match v.and_then(Value::as_str).unwrap_or("completed") {
        "inProgress" => "running".into(),
        "" => "completed".into(),
        value => value.chars().take(64).collect(),
    }
}
fn numeric(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}
fn metadata_fingerprint(value: &Value) -> Option<String> {
    let encoded = serde_json::to_string(value).ok()?;
    let mut ascii = String::with_capacity(encoded.len());
    for character in encoded.chars() {
        if character.is_ascii() {
            ascii.push(character);
            continue;
        }
        let codepoint = character as u32;
        if codepoint <= 0xffff {
            ascii.push_str(&format!("\\u{codepoint:04x}"));
        } else {
            let adjusted = codepoint - 0x1_0000;
            ascii.push_str(&format!(
                "\\u{:04x}\\u{:04x}",
                0xd800 + (adjusted >> 10),
                0xdc00 + (adjusted & 0x3ff)
            ));
        }
    }
    Some(format!("{:x}", Sha256::digest(ascii)))
}
fn tool_name(t: &str, item: &Map<String, Value>) -> Option<String> {
    match t {
        "commandExecution" => Some("command".into()),
        "fileChange" => Some("apply_patch".into()),
        "webSearch" => Some("web_search".into()),
        "imageView" => Some("view_image".into()),
        "sleep" => Some("sleep".into()),
        "mcpToolCall" => Some(
            format!(
                "mcp:{}:{}",
                safe_identifier(item.get("server")).unwrap_or_else(|| "unknown".into()),
                safe_identifier(item.get("tool")).unwrap_or_else(|| "unknown".into())
            )
            .chars()
            .take(256)
            .collect(),
        ),
        "collabToolCall" => Some(
            format!(
                "collab:{}",
                safe_identifier(item.get("tool")).unwrap_or_else(|| "unknown".into())
            )
            .chars()
            .take(256)
            .collect(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_never_enters_event() {
        let mut a = Adapter::default();
        let value = serde_json::json!({"method":"thread/started","params":{"thread":{"id":"t1","cwd":"/tmp/p","prompt":"SECRET"}}});
        let events = a.ingest(&value, Direction::Server);
        let encoded = format!("{events:?}");
        assert!(!encoded.contains("SECRET"));
        assert!(encoded.contains("t1"));
    }

    #[test]
    fn client_request_context_is_correlated_with_server_response() {
        let mut adapter = Adapter::default();
        let request = serde_json::json!({
            "id": 7,
            "method": "thread/start",
            "params": {"cwd": "/tmp/project", "model": "gpt-5", "prompt": "SECRET"}
        });
        assert!(adapter.ingest(&request, Direction::Client).is_empty());
        let response = serde_json::json!({"id": 7, "result": {"thread": {"id": "thread-1"}}});
        let events = adapter.ingest(&response, Direction::Server);
        assert_eq!(events.len(), 1);
        let debug = format!("{events:?}");
        assert!(debug.contains("thread-1"));
        assert!(debug.contains("/tmp/project"));
        assert!(!debug.contains("SECRET"));
    }

    #[test]
    fn usage_updates_emit_only_monotonic_deltas() {
        let mut adapter = Adapter::default();
        let first = serde_json::json!({
            "method": "thread/tokenUsage/updated",
            "params": {"threadId": "t", "turnId": "u", "tokenUsage": {"total": {
                "inputTokens": 10, "outputTokens": 2, "totalTokens": 12
            }}}
        });
        let second = serde_json::json!({
            "method": "thread/tokenUsage/updated",
            "params": {"threadId": "t", "turnId": "u", "tokenUsage": {"total": {
                "inputTokens": 15, "outputTokens": 4, "totalTokens": 19
            }}}
        });
        let first_events = adapter.ingest(&first, Direction::Server);
        let second_events = adapter.ingest(&second, Direction::Server);
        assert!(matches!(
            &first_events[0],
            LiveEvent::TurnUsage { usage, .. } if usage.total_tokens == 12
        ));
        assert!(matches!(
            &second_events[0],
            LiveEvent::TurnUsage { usage, .. }
                if usage.input_tokens == 5 && usage.output_tokens == 2 && usage.total_tokens == 7
        ));
    }

    #[test]
    fn object_shaped_ids_are_rejected_instead_of_serialized() {
        let mut adapter = Adapter::default();
        let message = serde_json::json!({
            "method": "thread/started",
            "params": {"thread": {"id": {"secret": "DO_NOT_STORE"}}}
        });
        assert!(adapter.ingest(&message, Direction::Server).is_empty());
    }
}
