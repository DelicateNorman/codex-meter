//! OTLP/HTTP JSON metadata collector.

use crate::models::{MetricPointRecord, Quality, TelemetryLogRecord};
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, OnceLock};
use std::thread;

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricPoint {
    pub event_fingerprint: String,
    pub observed_at: Option<String>,
    pub name: String,
    pub kind: String,
    pub value: Option<f64>,
    pub point_sum: Option<f64>,
    pub point_count: Option<u64>,
    pub point_min: Option<f64>,
    pub point_max: Option<f64>,
    pub explicit_bounds: Vec<f64>,
    pub bucket_counts: Vec<u64>,
    pub attributes: BTreeMap<String, String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub response_id: Option<String>,
    pub tool_name: Option<String>,
    pub start_time_unix_nano: Option<String>,
    pub time_unix_nano: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetryLog {
    pub event_fingerprint: String,
    pub observed_at: Option<String>,
    pub event_name: String,
    pub severity: Option<String>,
    pub attributes: BTreeMap<String, String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub response_id: Option<String>,
    pub item_id: Option<String>,
    pub tool_name: Option<String>,
    pub duration_ms: Option<f64>,
    pub status: Option<String>,
    pub success: Option<bool>,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct OtlpBatch {
    pub metrics: Vec<MetricPoint>,
    pub logs: Vec<TelemetryLog>,
}

impl OtlpBatch {
    pub fn into_records(self) -> (Vec<MetricPointRecord>, Vec<TelemetryLogRecord>) {
        (
            self.metrics.into_iter().map(Into::into).collect(),
            self.logs.into_iter().map(Into::into).collect(),
        )
    }
}

impl From<MetricPoint> for MetricPointRecord {
    fn from(point: MetricPoint) -> Self {
        Self {
            event_fingerprint: point.event_fingerprint,
            observed_at: point.observed_at,
            name: point.name,
            kind: point.kind,
            value: point.value,
            point_sum: point.point_sum,
            point_count: point.point_count.map(bounded_i64),
            point_min: point.point_min,
            point_max: point.point_max,
            explicit_bounds: point.explicit_bounds,
            bucket_counts: point.bucket_counts.into_iter().map(bounded_i64).collect(),
            attributes: point.attributes.into_iter().collect(),
            thread_id: point.thread_id,
            turn_id: point.turn_id,
            response_id: point.response_id,
            tool_name: point.tool_name,
            start_time_unix_nano: point.start_time_unix_nano,
            time_unix_nano: point.time_unix_nano,
            quality: Quality::exact("otlp_http"),
        }
    }
}

impl From<TelemetryLog> for TelemetryLogRecord {
    fn from(record: TelemetryLog) -> Self {
        Self {
            event_fingerprint: record.event_fingerprint,
            observed_at: record.observed_at,
            event_name: record.event_name,
            severity: record.severity,
            attributes: record.attributes.into_iter().collect(),
            thread_id: record.thread_id,
            turn_id: record.turn_id,
            response_id: record.response_id,
            item_id: record.item_id,
            tool_name: record.tool_name,
            duration_ms: record.duration_ms,
            status: record.status,
            success: record.success,
            quality: Quality::exact(record.source),
        }
    }
}

fn bounded_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub type BatchHandler = Arc<dyn Fn(OtlpBatch) + Send + Sync + 'static>;

pub fn parse_metrics(document: &Value) -> Vec<MetricPoint> {
    let mut output = Vec::new();
    for resource in array(document, "resourceMetrics") {
        let resource_attributes = attributes(resource.pointer("/resource/attributes"));
        for scope in array(resource, "scopeMetrics") {
            for metric in array(scope, "metrics") {
                let name = string(metric.get("name"))
                    .chars()
                    .take(256)
                    .collect::<String>();
                if name.is_empty() {
                    continue;
                }
                for kind in [
                    "gauge",
                    "sum",
                    "histogram",
                    "exponentialHistogram",
                    "summary",
                ] {
                    let Some(container) = metric.get(kind).and_then(Value::as_object) else {
                        continue;
                    };
                    for point in container
                        .get("dataPoints")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let mut merged = resource_attributes.clone();
                        merged.extend(attributes(point.get("attributes")));
                        let attrs = safe_attributes(merged);
                        let time = string(point.get("timeUnixNano"));
                        let start = string(point.get("startTimeUnixNano"));
                        let normalized = serde_json::json!({
                            "name": name, "kind": kind, "time": time, "start": start, "attrs": attrs,
                            "value": number(point.get("asDouble").or_else(|| point.get("asInt"))),
                            "sum": number(point.get("sum")), "count": integer(point.get("count")),
                            "min": number(point.get("min")), "max": number(point.get("max")),
                            "bounds": point.get("explicitBounds").cloned().unwrap_or(Value::Array(vec![])),
                            "buckets": point.get("bucketCounts").cloned().unwrap_or(Value::Array(vec![])),
                        });
                        output.push(MetricPoint {
                            event_fingerprint: hash(&normalized),
                            observed_at: nanos_to_iso(&time),
                            name: name.clone(),
                            kind: kind.into(),
                            value: number(point.get("asDouble").or_else(|| point.get("asInt"))),
                            point_sum: number(point.get("sum")),
                            point_count: integer(point.get("count")),
                            point_min: number(point.get("min")),
                            point_max: number(point.get("max")),
                            explicit_bounds: numbers(point.get("explicitBounds")),
                            bucket_counts: integers(point.get("bucketCounts")),
                            thread_id: first(&attrs, &["thread.id", "conversation.id"]),
                            turn_id: attrs.get("turn.id").cloned(),
                            response_id: attrs.get("response.id").cloned(),
                            tool_name: first(&attrs, &["tool", "tool_name"]),
                            attributes: attrs,
                            start_time_unix_nano: (!start.is_empty()).then_some(start),
                            time_unix_nano: (!time.is_empty()).then_some(time),
                        });
                    }
                }
            }
        }
    }
    output
}

pub fn parse_logs(document: &Value) -> Vec<TelemetryLog> {
    let mut output = Vec::new();
    for resource in array(document, "resourceLogs") {
        let resource_attributes = attributes(resource.pointer("/resource/attributes"));
        for scope in array(resource, "scopeLogs") {
            for record in array(scope, "logRecords") {
                let mut merged = resource_attributes.clone();
                merged.extend(attributes(record.get("attributes")));
                let attrs = safe_attributes(merged);
                let body_name = record
                    .pointer("/body/stringValue")
                    .and_then(Value::as_str)
                    .filter(|value| {
                        value.starts_with("codex.") && value.len() <= 128 && !value.contains(' ')
                    });
                let name = attrs
                    .get("event.name")
                    .cloned()
                    .or_else(|| body_name.map(str::to_owned))
                    .unwrap_or_else(|| "otel.log".to_owned());
                output.push(log_record(
                    &name,
                    string(
                        record
                            .get("timeUnixNano")
                            .or_else(|| record.get("observedTimeUnixNano")),
                    ),
                    string(record.get("severityText")),
                    attrs,
                    "otlp_http",
                ));
            }
        }
    }
    output
}

pub fn parse_traces(document: &Value) -> Vec<TelemetryLog> {
    let mut output = Vec::new();
    for resource in array(document, "resourceSpans") {
        let resource_attributes = attributes(resource.pointer("/resource/attributes"));
        for scope in array(resource, "scopeSpans") {
            for span in array(scope, "spans") {
                let mut merged = resource_attributes.clone();
                merged.extend(attributes(span.get("attributes")));
                let mut attrs = safe_attributes(merged);
                if let (Some(start), Some(end)) = (
                    integer(span.get("startTimeUnixNano")),
                    integer(span.get("endTimeUnixNano")),
                ) {
                    if end >= start {
                        attrs.insert(
                            "duration_ms".into(),
                            format!("{:.6}", (end - start) as f64 / 1_000_000.0),
                        );
                    }
                }
                let raw = string(span.get("name"));
                let safe = !raw.is_empty()
                    && raw.len() <= 128
                    && raw
                        .chars()
                        .all(|c| c.is_alphanumeric() || "._:/- ".contains(c));
                let name = format!("span:{}", if safe { raw.as_str() } else { "otel.span" });
                output.push(log_record(
                    &name,
                    string(span.get("endTimeUnixNano")),
                    String::new(),
                    attrs,
                    "otlp_trace",
                ));
            }
        }
    }
    output
}

pub fn serve(bind: SocketAddr, token: Option<String>, handler: BatchHandler) -> Result<()> {
    let listener = TcpListener::bind(bind)?;
    for stream in listener.incoming() {
        let handler = Arc::clone(&handler);
        let token = token.clone();
        if let Ok(mut stream) = stream {
            thread::spawn(move || {
                let _ = handle_request(&mut stream, token.as_deref(), handler);
            });
        }
    }
    Ok(())
}

fn handle_request(
    stream: &mut TcpStream,
    token: Option<&str>,
    handler: BatchHandler,
) -> Result<()> {
    let (header, remainder) = read_http_header(stream)?;
    if !header.ends_with(b"\r\n\r\n") {
        return response(stream, 400, "{\"error\":\"incomplete HTTP header\"}");
    }
    let text = String::from_utf8_lossy(&header);
    let mut lines = text.lines();
    let first = lines.next().context("empty HTTP request")?;
    let mut request = first.split_whitespace();
    let method = request.next().unwrap_or("");
    let path = request.next().unwrap_or("");
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    if method == "GET" && matches!(path, "/healthz" | "/readyz") {
        return response(stream, 200, "{\"status\":\"ok\"}");
    }
    if method != "POST" {
        return response(stream, 404, "{\"error\":\"not found\"}");
    }
    if token.is_some_and(|token| {
        headers
            .get("authorization")
            .is_none_or(|value| value != &format!("Bearer {token}"))
    }) {
        return response(stream, 401, "{\"error\":\"unauthorized\"}");
    }
    let content_type = headers
        .get("content-type")
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    if !matches!(content_type.as_str(), "" | "application/json") {
        return response(
            stream,
            415,
            "{\"error\":\"configure Codex OTLP/HTTP with protocol = 'json'\"}",
        );
    }
    let length = match headers.get("content-length") {
        Some(value) => match value.parse::<usize>() {
            Ok(value) => value,
            Err(_) => return response(stream, 400, "{\"error\":\"invalid content length\"}"),
        },
        None => 0,
    };
    if length > MAX_BODY_BYTES {
        return response(stream, 413, "{\"error\":\"payload too large\"}");
    }
    let mut body = remainder;
    body.truncate(length);
    while body.len() < length {
        let mut chunk = vec![0u8; (length - body.len()).min(65_536)];
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            bail!("truncated OTLP body");
        }
        body.extend_from_slice(&chunk[..count]);
    }
    let body = if headers
        .get("content-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("gzip"))
    {
        let mut output = Vec::new();
        if GzDecoder::new(body.as_slice())
            .take((MAX_BODY_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .is_err()
        {
            return response(stream, 400, "{\"error\":\"invalid gzip body\"}");
        }
        if output.len() > MAX_BODY_BYTES {
            return response(stream, 413, "{\"error\":\"payload too large\"}");
        }
        output
    } else {
        body
    };
    let document: Value = match serde_json::from_slice(if body.is_empty() { b"{}" } else { &body })
    {
        Ok(Value::Object(document)) => Value::Object(document),
        Ok(_) => return response(stream, 400, "{\"error\":\"OTLP root must be an object\"}"),
        Err(_) => return response(stream, 400, "{\"error\":\"invalid OTLP JSON\"}"),
    };
    let batch = match path {
        "/v1/metrics" => OtlpBatch {
            metrics: parse_metrics(&document),
            logs: vec![],
        },
        "/v1/logs" => OtlpBatch {
            metrics: vec![],
            logs: parse_logs(&document),
        },
        "/v1/traces" => OtlpBatch {
            metrics: vec![],
            logs: parse_traces(&document),
        },
        _ => return response(stream, 404, "{\"error\":\"not found\"}"),
    };
    handler(batch);
    response(stream, 200, "{}")
}

fn read_http_header(stream: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..count]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            return Ok((data[..index + 4].to_vec(), data[index + 4..].to_vec()));
        }
        if data.len() > 65_536 {
            bail!("HTTP header too large");
        }
    }
    Ok((data, vec![]))
}

fn response(stream: &mut TcpStream, status: u16, body: &str) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        _ => "Bad Request",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

#[cfg(test)]
fn handle_request_bytes(request: &[u8], token: Option<&str>) -> Result<(String, Vec<OtlpBatch>)> {
    use std::net::Shutdown;

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received = Arc::clone(&batches);
    let token = token.map(str::to_owned);
    let worker = thread::spawn(move || -> Result<()> {
        let (mut stream, _) = listener.accept()?;
        handle_request(
            &mut stream,
            token.as_deref(),
            Arc::new(move |batch| received.lock().unwrap().push(batch)),
        )
    });
    let mut client = TcpStream::connect(address)?;
    client.write_all(request)?;
    client.shutdown(Shutdown::Write)?;
    let mut response = String::new();
    client.read_to_string(&mut response)?;
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("worker failed"))??;
    let batches = Arc::try_unwrap(batches)
        .map_err(|_| anyhow::anyhow!("batch handler still referenced"))?
        .into_inner()
        .map_err(|_| anyhow::anyhow!("batch lock poisoned"))?;
    Ok((response, batches))
}

fn log_record(
    name: &str,
    time: String,
    severity: String,
    attrs: BTreeMap<String, String>,
    source: &str,
) -> TelemetryLog {
    let normalized =
        serde_json::json!({"name": name, "time": time, "severity": severity, "attrs": attrs});
    TelemetryLog {
        event_fingerprint: hash(&normalized),
        observed_at: nanos_to_iso(&time),
        event_name: name.chars().take(256).collect(),
        severity: (!severity.is_empty()).then_some(severity.chars().take(32).collect()),
        thread_id: first(&attrs, &["thread.id", "conversation.id"]),
        turn_id: attrs.get("turn.id").cloned(),
        response_id: attrs.get("response.id").cloned(),
        item_id: first(&attrs, &["item.id", "call_id"]),
        tool_name: first(&attrs, &["tool", "tool_name"]),
        duration_ms: attrs.get("duration_ms").and_then(|v| v.parse().ok()),
        status: first(&attrs, &["status", "http.response.status_code"]),
        success: attrs
            .get("success")
            .map(|value| value.eq_ignore_ascii_case("true")),
        attributes: attrs,
        source: source.into(),
    }
}

fn safe_keys() -> &'static BTreeSet<&'static str> {
    static KEYS: OnceLock<BTreeSet<&str>> = OnceLock::new();
    KEYS.get_or_init(|| {
        [
            "event.name",
            "thread.id",
            "turn.id",
            "conversation.id",
            "response.id",
            "item.id",
            "call_id",
            "tool",
            "tool_name",
            "model",
            "actual_model",
            "codex.turn.reasoning_effort",
            "service_tier",
            "provider",
            "transport",
            "token_type",
            "status",
            "success",
            "attempt",
            "error.type",
            "http.response.status_code",
            "duration_ms",
            "endpoint",
            "mcp_server",
            "mcp_server_origin",
            "env",
            "originator",
            "session_source",
        ]
        .into_iter()
        .collect()
    })
}

fn attributes(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let key = item.get("key")?.as_str()?;
            let value = item.get("value")?;
            for kind in ["stringValue", "intValue", "doubleValue", "boolValue"] {
                if let Some(value) = value.get(kind) {
                    return Some((key.into(), string(Some(value))));
                }
            }
            None
        })
        .collect()
}
fn safe_attributes(values: BTreeMap<String, String>) -> BTreeMap<String, String> {
    values
        .into_iter()
        .filter(|(key, _)| safe_keys().contains(key.as_str()))
        .map(|(key, value)| (key, value.chars().take(256).collect()))
        .collect()
}
fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
fn string(value: Option<&Value>) -> String {
    value
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string().trim_matches('"').to_owned())
        })
        .unwrap_or_default()
}
fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
}
fn integer(value: Option<&Value>) -> Option<u64> {
    value.and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
}
fn numbers(value: Option<&Value>) -> Vec<f64> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| number(Some(v)))
        .collect()
}
fn integers(value: Option<&Value>) -> Vec<u64> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| integer(Some(v)))
        .collect()
}
fn first(attrs: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| attrs.get(*key).filter(|v| !v.is_empty()).cloned())
}
fn hash(value: &Value) -> String {
    let encoded = serde_json::to_string(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(python_ascii_json(&encoded)))
}
fn python_ascii_json(encoded: &str) -> String {
    let mut output = String::with_capacity(encoded.len());
    for character in encoded.chars() {
        if character.is_ascii() {
            output.push(character);
        } else {
            let codepoint = character as u32;
            if codepoint <= 0xffff {
                output.push_str(&format!("\\u{codepoint:04x}"));
            } else {
                let adjusted = codepoint - 0x1_0000;
                let high = 0xd800 + (adjusted >> 10);
                let low = 0xdc00 + (adjusted & 0x3ff);
                output.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
            }
        }
    }
    output
}
fn nanos_to_iso(value: &str) -> Option<String> {
    let nanos = value.parse::<i64>().ok()?;
    if nanos <= 0 {
        return None;
    }
    chrono::DateTime::from_timestamp(nanos / 1_000_000_000, (nanos % 1_000_000_000) as u32)
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discards_prompt_attributes() {
        let document = serde_json::json!({"resourceLogs":[{"scopeLogs":[{"logRecords":[{"timeUnixNano":"1786492800000000000","attributes":[{"key":"event.name","value":{"stringValue":"codex.test"}},{"key":"prompt","value":{"stringValue":"secret"}},{"key":"model","value":{"stringValue":"gpt-5"}}]}]}]}]});
        let logs = parse_logs(&document);
        assert_eq!(logs.len(), 1);
        assert_eq!(
            logs[0].attributes.get("model").map(String::as_str),
            Some("gpt-5")
        );
        assert!(!logs[0].attributes.contains_key("prompt"));
    }

    #[test]
    fn http_rejects_non_json_and_bad_auth_without_calling_handler() {
        let request = b"POST /v1/logs HTTP/1.1\r\nContent-Type: application/x-protobuf\r\nAuthorization: Bearer token\r\nContent-Length: 2\r\n\r\n{}";
        let (response, batches) = handle_request_bytes(request, Some("token")).unwrap();
        assert!(response.starts_with("HTTP/1.1 415"));
        assert!(batches.is_empty());

        let request = b"POST /v1/logs HTTP/1.1\r\nContent-Type: application/json\r\nAuthorization: Bearer wrong\r\nContent-Length: 2\r\n\r\n{}";
        let (response, batches) = handle_request_bytes(request, Some("token")).unwrap();
        assert!(response.starts_with("HTTP/1.1 401"));
        assert!(batches.is_empty());
    }

    #[test]
    fn http_ingests_traces_but_drops_span_attributes_with_content() {
        let body = serde_json::json!({"resourceSpans":[{"scopeSpans":[{"spans":[{
            "name":"codex.request", "startTimeUnixNano":"1786492800000000000",
            "endTimeUnixNano":"1786492801000000000", "attributes":[
                {"key":"model","value":{"stringValue":"gpt-5"}},
                {"key":"prompt","value":{"stringValue":"SECRET"}}
            ]
        }]}]}]})
        .to_string();
        let request = format!(
            "POST /v1/traces HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let (response, batches) = handle_request_bytes(request.as_bytes(), None).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert_eq!(batches.len(), 1);
        let debug = format!("{:?}", batches[0].logs);
        assert!(debug.contains("gpt-5"));
        assert!(!debug.contains("SECRET"));
    }

    #[test]
    fn parses_histogram_metadata_and_discards_unsafe_attributes() {
        let document = serde_json::json!({"resourceMetrics":[{
            "resource":{"attributes":[
                {"key":"model","value":{"stringValue":"gpt-5"}},
                {"key":"response.body","value":{"stringValue":"SECRET"}}
            ]},
            "scopeMetrics":[{"metrics":[{"name":"codex.latency","histogram":{"dataPoints":[{
                "timeUnixNano":"1786492800000000000", "count":"3", "sum":12.5,
                "min":1.0, "max":8.0, "explicitBounds":[2.0,5.0], "bucketCounts":[1,1,1]
            }]}}]}]
        }]});
        let metrics = parse_metrics(&document);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].point_count, Some(3));
        assert_eq!(metrics[0].bucket_counts, vec![1, 1, 1]);
        assert_eq!(
            metrics[0].attributes.get("model").map(String::as_str),
            Some("gpt-5")
        );
        assert!(!metrics[0].attributes.contains_key("response.body"));
        assert!(!format!("{metrics:?}").contains("SECRET"));

        let record: MetricPointRecord = metrics.into_iter().next().unwrap().into();
        assert_eq!(record.point_count, Some(3));
        assert_eq!(record.quality.source, "otlp_http");
    }

    #[test]
    fn fingerprints_match_python_sorted_ascii_json() {
        let value = serde_json::json!({"z":"模型","a":1});
        assert_eq!(
            hash(&value),
            "0e639773692e5ac64c135cbc563a3cac575a8d09f84079e37416e7b6363953c9"
        );
    }
}
