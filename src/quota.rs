//! Live, account-wide Codex rate-limit reader.
//!
//! The app-server request is deliberately exposed both synchronously and through
//! [`spawn_weekly_quota_reader`].  Interactive callers should use the latter so a
//! slow Codex startup or an unavailable account never delays the first frame.

use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub const WEEK_MINUTES: i64 = 7 * 24 * 60;
const WEEK_TOLERANCE_MINUTES: i64 = 12 * 60;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeeklyQuota {
    pub limit_id: String,
    pub name: String,
    pub used_percent: u8,
    pub resets_at: Option<i64>,
    pub window_minutes: i64,
    pub plan_type: Option<String>,
}

impl WeeklyQuota {
    pub fn remaining_percent(&self) -> u8 {
        100_u8.saturating_sub(self.used_percent)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuotaUpdate {
    Loaded(Vec<WeeklyQuota>),
    Unavailable(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaUnavailable(pub String);

impl fmt::Display for QuotaUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for QuotaUnavailable {}

/// Ask the installed `codex app-server` for the current account limits.
pub fn read_default_weekly_quotas(timeout: Duration) -> Result<Vec<WeeklyQuota>, QuotaUnavailable> {
    let command = vec![
        OsString::from("codex"),
        OsString::from("app-server"),
        OsString::from("--stdio"),
    ];
    read_weekly_quotas(&command, timeout)
}

/// Read quota data using an arbitrary executable and argument vector.
///
/// This is public primarily so packaging probes and tests can exercise the
/// exact JSONL transport without replacing a user's `codex` binary.
pub fn read_weekly_quotas(
    command: &[OsString],
    timeout: Duration,
) -> Result<Vec<WeeklyQuota>, QuotaUnavailable> {
    let Some((program, arguments)) = command.split_first() else {
        return Err(QuotaUnavailable("quota command is empty".into()));
    };
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| QuotaUnavailable(format!("could not start Codex App Server: {error}")))?;

    let result = (|| {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            QuotaUnavailable("Codex App Server did not open its stdio transport".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            QuotaUnavailable("Codex App Server did not open its stdio transport".into())
        })?;

        let requests = [
            json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "codex-meter",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": {"experimentalApi": true},
                },
            }),
            json!({"method": "initialized"}),
            json!({"id": 2, "method": "account/rateLimits/read", "params": null}),
        ];
        for request in requests {
            serde_json::to_writer(&mut stdin, &request).map_err(|error| {
                QuotaUnavailable(format!("could not write Codex rate-limit request: {error}"))
            })?;
            stdin.write_all(b"\n").map_err(|error| {
                QuotaUnavailable(format!("could not write Codex rate-limit request: {error}"))
            })?;
        }
        stdin.flush().map_err(|error| {
            QuotaUnavailable(format!("could not write Codex rate-limit request: {error}"))
        })?;

        let (sender, receiver) = mpsc::channel::<Option<String>>();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(Some(line)).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = sender.send(None);
        });

        let deadline = Instant::now() + timeout.max(Duration::from_millis(100));
        let response = loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break Err(QuotaUnavailable(
                    "timed out waiting for Codex rate limits".into(),
                ));
            };
            match receiver.recv_timeout(remaining) {
                Ok(Some(line)) => {
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if message.get("id").and_then(Value::as_i64) != Some(2) {
                        continue;
                    }
                    if message.get("error").is_some_and(|value| !value.is_null()) {
                        break Err(QuotaUnavailable(
                            "Codex App Server rejected the rate-limit request".into(),
                        ));
                    }
                    let Some(result) = message.get("result").and_then(Value::as_object) else {
                        break Err(QuotaUnavailable(
                            "Codex App Server returned an invalid rate-limit response".into(),
                        ));
                    };
                    break Ok(extract_weekly_quotas(result));
                }
                Ok(None) => {
                    break Err(QuotaUnavailable(
                        "Codex App Server closed before returning rate limits".into(),
                    ));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    break Err(QuotaUnavailable(
                        "timed out waiting for Codex rate limits".into(),
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(QuotaUnavailable(
                        "Codex App Server closed before returning rate limits".into(),
                    ));
                }
            }
        };

        drop(stdin);
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        response
    })();

    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

/// Launch a quota read on a worker thread. Receiving from the returned channel
/// is optional; dropping it is safe and never holds the process open.
pub fn spawn_weekly_quota_reader(timeout: Duration) -> Receiver<QuotaUpdate> {
    spawn_weekly_quota_reader_with(
        vec![
            OsString::from("codex"),
            OsString::from("app-server"),
            OsString::from("--stdio"),
        ],
        timeout,
    )
}

pub fn spawn_weekly_quota_reader_with(
    command: Vec<OsString>,
    timeout: Duration,
) -> Receiver<QuotaUpdate> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let update = match read_weekly_quotas(&command, timeout) {
            Ok(quotas) => QuotaUpdate::Loaded(quotas),
            Err(error) => QuotaUpdate::Unavailable(error.to_string()),
        };
        let _ = sender.send(update);
    });
    receiver
}

/// Extract every approximately-seven-day bucket from an app-server result.
pub fn extract_weekly_quotas(result: &Map<String, Value>) -> Vec<WeeklyQuota> {
    let mut snapshots: Vec<&Map<String, Value>> = result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(Map::values)
        .filter_map(Value::as_object)
        .collect();

    if let Some(legacy) = result.get("rateLimits").and_then(Value::as_object) {
        let legacy_id = string_value(legacy.get("limitId")).unwrap_or_else(|| "codex".into());
        let already_present = snapshots.iter().any(|snapshot| {
            string_value(snapshot.get("limitId")).unwrap_or_else(|| "codex".into()) == legacy_id
        });
        if !already_present {
            snapshots.push(legacy);
        }
    }

    let mut seen = HashSet::new();
    let mut quotas = Vec::new();
    for snapshot in snapshots {
        let limit_id = string_value(snapshot.get("limitId")).unwrap_or_else(|| "codex".into());
        if seen.contains(&limit_id) {
            continue;
        }
        let weekly = ["primary", "secondary"]
            .into_iter()
            .filter_map(|key| snapshot.get(key).and_then(Value::as_object))
            .filter(|window| is_week_window(window.get("windowDurationMins")))
            .min_by_key(|window| (integer(window.get("windowDurationMins")) - WEEK_MINUTES).abs());
        let Some(window) = weekly else { continue };
        let used_percent = integer(window.get("usedPercent")).clamp(0, 100) as u8;
        let name = string_value(snapshot.get("limitName"))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                if limit_id == "codex" {
                    "Codex".into()
                } else {
                    limit_id.clone()
                }
            });
        quotas.push(WeeklyQuota {
            limit_id: limit_id.clone(),
            name: name.trim().to_owned(),
            used_percent,
            resets_at: optional_integer(window.get("resetsAt")),
            window_minutes: integer(window.get("windowDurationMins")),
            plan_type: string_value(snapshot.get("planType")),
        });
        seen.insert(limit_id);
    }
    quotas.sort_by_key(|quota| (quota.limit_id != "codex", quota.name.to_lowercase()));
    quotas
}

fn is_week_window(value: Option<&Value>) -> bool {
    let minutes = integer(value);
    (WEEK_MINUTES - WEEK_TOLERANCE_MINUTES..=WEEK_MINUTES + WEEK_TOLERANCE_MINUTES)
        .contains(&minutes)
}

fn integer(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .unwrap_or(0),
        Some(Value::String(value)) => value.parse().unwrap_or(0),
        Some(Value::Bool(value)) => i64::from(*value),
        _ => 0,
    }
}

fn optional_integer(value: Option<&Value>) -> Option<i64> {
    let value = integer(value);
    (value != 0).then_some(value)
}

fn string_value(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => Some(value.clone()),
        Some(value) if !value.is_null() => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_all_weekly_buckets_and_places_codex_first() {
        let value = json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 99, "windowDurationMins": 300},
                "secondary": {"usedPercent": 15, "windowDurationMins": WEEK_MINUTES,
                              "resetsAt": 1234},
            },
            "rateLimitsByLimitId": {
                "spark": {
                    "limitId": "codex_bengalfox",
                    "limitName": "GPT-5.3-Codex-Spark",
                    "primary": {"usedPercent": 0, "windowDurationMins": WEEK_MINUTES,
                                "resetsAt": 5678},
                },
                "codex": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 15, "windowDurationMins": WEEK_MINUTES,
                                "resetsAt": 1234},
                },
            },
        });
        let quotas = extract_weekly_quotas(value.as_object().unwrap());
        assert_eq!(quotas.len(), 2);
        assert_eq!(quotas[0].limit_id, "codex");
        assert_eq!(quotas[0].remaining_percent(), 85);
        assert_eq!(quotas[1].name, "GPT-5.3-Codex-Spark");
    }

    #[test]
    fn ignores_malformed_windows_and_clamps_percentage() {
        let malformed = json!({"rateLimits": {
            "limitId": "codex",
            "primary": {"usedPercent": 20, "windowDurationMins": 300},
            "secondary": {"usedPercent": "bad", "windowDurationMins": null}
        }});
        assert!(extract_weekly_quotas(malformed.as_object().unwrap()).is_empty());

        let full = json!({"rateLimits": {
            "limitId": "codex",
            "primary": {"usedPercent": 150, "windowDurationMins": WEEK_MINUTES}
        }});
        let quotas = extract_weekly_quotas(full.as_object().unwrap());
        assert_eq!(quotas[0].used_percent, 100);
        assert_eq!(quotas[0].remaining_percent(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn reads_batched_jsonl_responses() {
        let script = r#"
for ignored in 1 2 3; do IFS= read -r line; done
printf '%s\n' '{"id":1,"result":{}}'
printf '%s\n' '{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":42,"windowDurationMins":10080,"resetsAt":1234}}}}'
"#;
        let command = vec![
            OsString::from("sh"),
            OsString::from("-c"),
            OsString::from(script),
        ];
        let quotas = read_weekly_quotas(&command, Duration::from_secs(2)).unwrap();
        assert_eq!(quotas.len(), 1);
        assert_eq!(quotas[0].used_percent, 42);
    }

    #[cfg(unix)]
    #[test]
    fn background_reader_returns_before_slow_quota_finishes() {
        let script = r#"
for ignored in 1 2 3; do IFS= read -r line; done
sleep 0.2
printf '%s\n' '{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":1,"windowDurationMins":10080}}}}'
"#;
        let started = Instant::now();
        let receiver = spawn_weekly_quota_reader_with(
            vec![
                OsString::from("sh"),
                OsString::from("-c"),
                OsString::from(script),
            ],
            Duration::from_secs(2),
        );
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            QuotaUpdate::Loaded(_)
        ));
    }
}
