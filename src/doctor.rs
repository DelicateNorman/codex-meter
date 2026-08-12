//! Data-source capability diagnostics.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub status: &'static str,
    pub detail: String,
}

pub fn run(codex_home: &Path, sqlite_integrity: &str) -> Vec<Check> {
    let codex_version = command_output("codex", &["--version"], Duration::from_secs(10));
    let sessions = codex_home.join("sessions");
    let rollouts: Vec<_> = WalkDir::new(&sessions)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.file_name().to_string_lossy().starts_with("rollout-")
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
        })
        .map(|entry| entry.into_path())
        .collect();
    let capabilities = rollout_capabilities(&rollouts);
    let otel = otel_exporter(codex_home);
    let app_server =
        command_output("codex", &["app-server", "--help"], Duration::from_secs(10)).is_some();
    let raw_response = app_server && app_server_has_raw_response("codex");
    vec![
        check(
            "Codex version",
            codex_version.is_some(),
            codex_version.unwrap_or_else(|| "not found".into()),
        ),
        check(
            "Session JSONL",
            !rollouts.is_empty(),
            format!("{} file(s)", rollouts.len()),
        ),
        capability("Reasoning usage", "reasoning_output_tokens", &capabilities),
        capability("Cached input", "cached_input_tokens", &capabilities),
        capability("Cache write", "cache_write_input_tokens", &capabilities),
        capability("Turn timings", "time_to_first_token_ms", &capabilities),
        Check {
            name: "OpenTelemetry",
            status: if otel == "none" { "disabled" } else { "yes" },
            detail: otel,
        },
        check(
            "App Server",
            app_server,
            if app_server { "experimental CLI" } else { "" }.into(),
        ),
        Check {
            name: "Raw response events",
            status: if raw_response {
                "experimental"
            } else {
                "unknown"
            },
            detail: String::new(),
        },
        Check {
            name: "OTLP HTTP JSON collector",
            status: "yes",
            detail: "logs + metrics + traces".into(),
        },
        check(
            "Passive packet metadata",
            command_exists("tcpdump"),
            executable_detail("tcpdump"),
        ),
        Check {
            name: "CONNECT/reverse proxy",
            status: "yes",
            detail: "content-free persistence".into(),
        },
        Check {
            name: "TLS diagnostic",
            status: if command_exists("openssl") {
                "disabled"
            } else {
                "no"
            },
            detail: if command_exists("openssl") {
                "explicit opt-in".into()
            } else {
                "openssl not found".into()
            },
        },
        Check {
            name: "SQLite",
            status: if sqlite_integrity == "ok" {
                "yes"
            } else {
                "no"
            },
            detail: sqlite_integrity.into(),
        },
    ]
}

fn check(name: &'static str, yes: bool, detail: String) -> Check {
    Check {
        name,
        status: if yes { "yes" } else { "no" },
        detail,
    }
}

fn capability(name: &'static str, key: &str, found: &BTreeSet<String>) -> Check {
    Check {
        name,
        status: if found.contains(key) {
            "yes"
        } else {
            "unknown"
        },
        detail: String::new(),
    }
}

fn rollout_capabilities(paths: &[std::path::PathBuf]) -> BTreeSet<String> {
    let keys = [
        "reasoning_output_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "time_to_first_token_ms",
    ];
    let mut output = BTreeSet::new();
    let mut sorted = paths.to_vec();
    sorted.sort_by(|left, right| right.cmp(left));
    for path in sorted.iter().take(10) {
        let Ok(file) = File::open(path) else {
            continue;
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains("token_count") && !line.contains("time_to_first_token_ms") {
                continue;
            }
            for key in keys {
                if line.contains(key) {
                    output.insert(key.to_owned());
                }
            }
        }
        if output.len() == keys.len() {
            break;
        }
    }
    output
}

fn otel_exporter(codex_home: &Path) -> String {
    let Some(parsed) = fs::read_to_string(codex_home.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
    else {
        return "none".into();
    };
    let Some(exporter) = parsed.get("otel").and_then(|otel| otel.get("exporter")) else {
        return "none".into();
    };
    if let Some(name) = exporter.as_str() {
        return name.into();
    }
    if exporter.get("otlp-http").is_some() {
        "otlp-http".into()
    } else if exporter.get("otlp-grpc").is_some() {
        "otlp-grpc".into()
    } else {
        "configured".into()
    }
}

fn command_exists(program: &str) -> bool {
    command_output(program, &["--version"], Duration::from_secs(5)).is_some()
        || command_output(program, &["-h"], Duration::from_secs(5)).is_some()
}

fn executable_detail(program: &str) -> String {
    find_command(program)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{program} not found"))
}

fn command_output(program: &str, arguments: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().ok()? {
            if !status.success() {
                return None;
            }
            let mut output = Vec::new();
            use std::io::Read;
            child.stdout.take()?.read_to_end(&mut output).ok()?;
            return Some(String::from_utf8_lossy(&output).trim().to_owned());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn app_server_has_raw_response(codex: &str) -> bool {
    let Ok(directory) = tempfile::Builder::new()
        .prefix("codex-meter-schema-")
        .tempdir()
    else {
        return false;
    };
    if !command_success(
        codex,
        &[
            "app-server",
            "generate-json-schema",
            "--experimental",
            "--out",
            directory.path().to_str().unwrap_or(""),
        ],
        Duration::from_secs(20),
    ) {
        return false;
    }
    let path = directory
        .path()
        .join("v2")
        .join("RawResponseCompletedNotification.json");
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value.get("title").and_then(str_value).map(str::to_owned))
        .is_some_and(|title| title == "RawResponseCompletedNotification")
}

fn str_value(value: &serde_json::Value) -> Option<&str> {
    value.as_str()
}

fn command_success(program: &str, arguments: &[&str], timeout: Duration) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn find_command(program: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_owned()
    };
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(&file))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exporter_name_never_exposes_headers() {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("config.toml"),
            "[otel]\nexporter = { otlp-http = { endpoint = 'http://localhost', headers = { Authorization = 'SECRET' } } }\n",
        )
        .unwrap();
        let value = otel_exporter(home.path());
        assert_eq!(value, "otlp-http");
        assert!(!value.contains("SECRET"));
    }

    #[test]
    fn rollout_scan_reports_only_known_capabilities() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout-test.jsonl");
        fs::write(
            &path,
            "{\"prompt\":\"SECRET\"}\n{\"type\":\"token_count\",\"cached_input_tokens\":7}\n",
        )
        .unwrap();
        let capabilities = rollout_capabilities(&[path]);
        assert!(capabilities.contains("cached_input_tokens"));
        assert_eq!(capabilities.len(), 1);
    }
}
