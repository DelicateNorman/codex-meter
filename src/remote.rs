//! Incremental SSH Rollout sources.

use crate::collector::SessionCollector;
use crate::config::validate_remote_host;
use crate::pricing::PricingCatalog;
use crate::storage::{SourceMetadata, Storage};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::{BufReader, Read, Seek, Write};
use std::path::{Component, Path};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const FILTER_SCRIPT: &str = r#"import io
import json
import re
import sys
import tarfile
from pathlib import Path, PurePosixPath

ROOT = Path.home() / ".codex" / "sessions"
TYPE_RE = re.compile(br'"type"\s*:\s*"([^"\\]+)"')
TIMESTAMP_RE = re.compile(br'"timestamp"\s*:\s*"([^"\\]+)"')
EVENTS = {
    "task_started", "turn_started", "task_complete", "turn_complete",
    "turn_aborted", "thread_settings_applied", "context_compacted",
    "raw_response_completed", "rawResponse/completed", "token_count",
    "exec_command_begin", "patch_apply_begin", "mcp_tool_call_begin",
    "web_search_begin", "exec_command_end", "patch_apply_end",
    "mcp_tool_call_end", "web_search_end",
}
TOKEN_KEYS = {
    "input_tokens", "inputTokens", "cached_input_tokens", "cachedInputTokens",
    "cache_write_input_tokens", "cacheWriteInputTokens", "cache_write_tokens",
    "output_tokens", "outputTokens", "reasoning_output_tokens",
    "reasoningOutputTokens", "total_tokens", "totalTokens",
}

def scalar(value):
    return value if isinstance(value, (str, int, float, bool)) or value is None else None

def picked(mapping, names):
    if not isinstance(mapping, dict):
        return {}
    return {name: scalar(mapping[name]) for name in names if name in mapping and scalar(mapping[name]) is not None}

def usage(value):
    return picked(value, TOKEN_KEYS)

def safe_payload(kind, payload):
    payload = payload if isinstance(payload, dict) else {}
    if kind == "session_meta":
        result = picked(payload, {
            "session_id", "id", "timestamp", "cwd", "cli_version",
            "model_provider", "parent_thread_id", "forked_from_id",
            "agent_role", "agent_id",
        })
        git = picked(payload.get("git"), {"repository_url", "branch"})
        if git:
            result["git"] = git
        source = payload.get("source")
        subagent = picked(source.get("subagent") if isinstance(source, dict) else None,
                          {"parent_thread_id", "agent_role", "agent_id"})
        if subagent:
            result["source"] = {"subagent": subagent}
        return result
    if kind == "turn_context":
        return picked(payload, {
            "model", "effort", "reasoning_effort", "reasoning_mode",
            "service_tier", "provider", "turn_id",
        })
    if kind in {"task_started", "turn_started"}:
        return picked(payload, {"type", "turn_id", "started_at"})
    if kind in {"task_complete", "turn_complete", "turn_aborted"}:
        result = picked(payload, {
            "type", "turn_id", "completed_at", "duration_ms",
            "time_to_first_token_ms",
        })
        error = payload.get("error")
        if error is not None:
            safe_error = picked(error, {"codex_error_info", "type", "code"})
            result["error"] = safe_error or {"type": "error"}
        return result
    if kind == "thread_settings_applied":
        return {
            "type": kind,
            "thread_settings": picked(payload.get("thread_settings"), {
                "model", "reasoning_effort", "reasoning_mode", "service_tier",
            }),
        }
    if kind == "context_compacted":
        return {"type": kind}
    if kind in {"raw_response_completed", "rawResponse/completed"}:
        result = picked(payload, {"type", "turn_id", "response_id"})
        if isinstance(payload.get("token_usage"), dict):
            result["token_usage"] = usage(payload["token_usage"])
        elif isinstance(payload.get("usage"), dict):
            result["usage"] = usage(payload["usage"])
        return result
    if kind == "token_count":
        info = payload.get("info") if isinstance(payload.get("info"), dict) else {}
        result = {
            "type": kind,
            "info": {
                "total_token_usage": usage(info.get("total_token_usage")),
                "last_token_usage": usage(info.get("last_token_usage")),
            },
        }
        limits = payload.get("rate_limits")
        if isinstance(limits, dict) and limits.get("plan_type") is not None:
            result["rate_limits"] = {"plan_type": scalar(limits.get("plan_type"))}
        return result
    result = picked(payload, {
        "type", "call_id", "id", "turn_id", "started_at_ms",
        "completed_at_ms", "success", "exit_code", "status",
    })
    duration = payload.get("duration")
    if isinstance(duration, dict):
        safe_duration = picked(duration, {"secs", "seconds", "nanos", "nanoseconds"})
        if safe_duration:
            result["duration"] = safe_duration
    elif scalar(duration) is not None:
        result["duration"] = scalar(duration)
    invocation = picked(payload.get("invocation"), {"server", "tool"})
    if invocation:
        result["invocation"] = invocation
    tool_result = payload.get("result")
    if isinstance(tool_result, dict):
        if "Err" in tool_result:
            result["result"] = {"Err": True}
        elif "error" in tool_result:
            result["result"] = {"error": True}
        else:
            result["result"] = {}
    return result

def filter_file(path):
    output = io.BytesIO()
    last_timestamp = None
    last_emitted_timestamp = None
    with path.open("rb") as stream:
        for raw in stream:
            timestamp_match = TIMESTAMP_RE.search(raw)
            if timestamp_match:
                last_timestamp = timestamp_match.group(1).decode("utf-8", "replace")
            types = TYPE_RE.findall(raw)
            if not types:
                continue
            outer = types[0].decode("utf-8", "replace")
            if outer not in {"session_meta", "turn_context", "event_msg"}:
                continue
            kind = outer
            if outer == "event_msg":
                if len(types) < 2:
                    continue
                kind = types[1].decode("utf-8", "replace")
                if kind not in EVENTS:
                    continue
            try:
                record = json.loads(raw)
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            timestamp = record.get("timestamp") if isinstance(record.get("timestamp"), str) else None
            envelope = {
                "timestamp": timestamp,
                "type": outer,
                "payload": safe_payload(kind, record.get("payload")),
            }
            output.write(json.dumps(envelope, ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n")
            last_emitted_timestamp = timestamp
    if last_timestamp and last_timestamp != last_emitted_timestamp:
        output.write(json.dumps({
            "timestamp": last_timestamp,
            "type": "codex_meter_end",
            "payload": {},
        }, separators=(",", ":")).encode("utf-8") + b"\n")
    return output.getvalue()

with tarfile.open(fileobj=sys.stdout.buffer, mode="w|gz", compresslevel=3) as archive:
    for argument in sys.argv[1:]:
        relative = PurePosixPath(argument)
        if (not argument or relative.is_absolute() or ".." in relative.parts
                or not relative.name.startswith("rollout-")
                or not relative.name.endswith(".jsonl")):
            continue
        path = ROOT.joinpath(*relative.parts)
        if not path.is_file() or path.is_symlink():
            continue
        data = filter_file(path)
        info = tarfile.TarInfo(relative.as_posix())
        info.size = len(data)
        info.mtime = int(path.stat().st_mtime)
        info.mode = 0o600
        archive.addfile(info, io.BytesIO(data))
"#;

const LIST_SCRIPT: &str = r#"set -eu
root="$HOME/.codex/sessions"
if [ ! -d "$root" ]; then
    echo "Codex session directory not found: $root" >&2
    exit 3
fi
find "$root" -type f -name 'rollout-*.jsonl' -print | while IFS= read -r file; do
    size=$(wc -c < "$file" | tr -d '[:space:]')
    if mtime=$(stat -c %Y "$file" 2>/dev/null); then :
    elif mtime=$(stat -f %m "$file" 2>/dev/null); then :
    else mtime=0
    fi
    relative=${file#"$root"/}
    printf '%s\t%s\t%s\n' "$size" "$mtime" "$relative"
done"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub host: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
}

impl RemoteFile {
    pub fn source_path(&self) -> String {
        format!(
            "ssh://{}/~/.codex/sessions/{}",
            self.host, self.relative_path
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncResult {
    pub host: String,
    pub discovered_files: usize,
    pub imported_files: usize,
    pub skipped_files: usize,
    pub failed_files: usize,
    pub inserted_turns: usize,
    pub inserted_calls: usize,
    pub inserted_tools: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncProgress {
    pub host: String,
    pub completed_files: usize,
    pub total_files: usize,
    pub skipped_files: usize,
    pub completed_source_bytes: u64,
    pub total_source_bytes: u64,
    pub server_filtered: bool,
}

pub fn list(host: &str) -> Result<Vec<RemoteFile>> {
    list_with_ssh(host, "ssh")
}

fn list_with_ssh(host: &str, ssh: &str) -> Result<Vec<RemoteFile>> {
    list_with_ssh_timeout(host, ssh, LIST_TIMEOUT)
}

fn list_with_ssh_timeout(host: &str, ssh: &str, timeout: Duration) -> Result<Vec<RemoteFile>> {
    let host = validate_remote_host(host)?;
    let command = format!("sh -c {}", shell_quote(LIST_SCRIPT));
    let mut ssh_command = Command::new(ssh);
    ssh_command.args(ssh_options()).arg(&host).arg(command);
    let (status, stdout_bytes, stderr_bytes) = command_output_with_timeout(
        &mut ssh_command,
        timeout,
        &format!("SSH connection to {host} timed out"),
        "OpenSSH client was not found; install `ssh` first",
    )?;
    if !status.success() {
        bail!(
            "{host}: {}",
            short_error(&String::from_utf8_lossy(&stderr_bytes))
        );
    }
    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&stdout_bytes).lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(size), Some(mtime), Some(relative)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(size_bytes), Ok(seconds), Ok(relative_path)) = (
            size.parse::<u64>(),
            mtime.parse::<i64>(),
            safe_relative_path(relative),
        ) else {
            continue;
        };
        files.push(RemoteFile {
            host: host.clone(),
            relative_path,
            size_bytes,
            mtime_ns: seconds.saturating_mul(1_000_000_000),
        });
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
    timeout_message: &str,
    not_found_message: &str,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    // Seekable anonymous files cannot fill and block the SSH child. They also
    // avoid leaving pipe-reader threads behind when a timed-out process has
    // descendants that inherited stdout/stderr.
    let mut stdout = tempfile::tempfile().context("could not buffer SSH stdout")?;
    let mut stderr = tempfile::tempfile().context("could not buffer SSH stderr")?;
    let mut child = command
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(not_found_message.to_owned())
            } else {
                anyhow::Error::new(error).context("could not start SSH client")
            }
        })?;
    let deadline = Instant::now() + timeout.max(Duration::from_millis(100));
    let timed_out = loop {
        if child.try_wait()?.is_some() {
            break false;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            break true;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let status = child.wait()?;
    if timed_out {
        bail!(timeout_message.to_owned());
    }
    stdout.rewind()?;
    stderr.rewind()?;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    stdout.read_to_end(&mut stdout_bytes)?;
    stderr.read_to_end(&mut stderr_bytes)?;
    Ok((status, stdout_bytes, stderr_bytes))
}

pub fn sync(
    storage: &mut Storage,
    catalog: &PricingCatalog,
    host: &str,
    force: bool,
) -> Result<SyncResult> {
    sync_with_progress(storage, catalog, host, force, |_| {})
}

pub fn sync_with_progress(
    storage: &mut Storage,
    catalog: &PricingCatalog,
    host: &str,
    force: bool,
    on_progress: impl FnMut(&SyncProgress),
) -> Result<SyncResult> {
    sync_with_progress_using(storage, catalog, host, force, "ssh", on_progress)
}

fn sync_with_progress_using(
    storage: &mut Storage,
    catalog: &PricingCatalog,
    host: &str,
    force: bool,
    ssh: &str,
    mut on_progress: impl FnMut(&SyncProgress),
) -> Result<SyncResult> {
    let files = list_with_ssh(host, ssh)?;
    let host = validate_remote_host(host)?;
    let mut result = SyncResult {
        host: host.clone(),
        discovered_files: files.len(),
        ..SyncResult::default()
    };
    let mut changed = Vec::new();
    for file in files {
        if !force
            && storage.source_is_current(&file.source_path(), file.size_bytes, file.mtime_ns)?
        {
            result.skipped_files += 1;
        } else {
            changed.push(file);
        }
    }
    let total_source_bytes = changed.iter().map(|file| file.size_bytes).sum();
    let server_filtered = !changed.is_empty() && remote_has_python(&host, ssh)?;
    let mut completed_files = 0;
    let mut completed_source_bytes = 0_u64;
    on_progress(&SyncProgress {
        host: host.clone(),
        completed_files,
        total_files: changed.len(),
        skipped_files: result.skipped_files,
        completed_source_bytes,
        total_source_bytes,
        server_filtered,
    });
    let collector = SessionCollector::new(catalog);
    let skipped_files = result.skipped_files;
    for batch in changed.chunks(64) {
        let mut file_completed = |file: &RemoteFile| {
            completed_files += 1;
            completed_source_bytes = completed_source_bytes.saturating_add(file.size_bytes);
            on_progress(&SyncProgress {
                host: host.clone(),
                completed_files,
                total_files: changed.len(),
                skipped_files,
                completed_source_bytes,
                total_source_bytes,
                server_filtered,
            });
        };
        import_batch(
            storage,
            &collector,
            batch,
            &mut result,
            ssh,
            server_filtered,
            &mut file_completed,
        )?;
    }
    Ok(result)
}

fn remote_has_python(host: &str, ssh: &str) -> Result<bool> {
    let host = validate_remote_host(host)?;
    let mut command = Command::new(ssh);
    command
        .args(ssh_options())
        .arg(&host)
        .arg("command -v python3 >/dev/null 2>&1");
    let (status, _, _) = command_output_with_timeout(
        &mut command,
        LIST_TIMEOUT,
        &format!("SSH connection to {host} timed out while checking metadata filtering"),
        "OpenSSH client was not found; install `ssh` first",
    )?;
    Ok(status.success())
}

fn import_batch(
    storage: &mut Storage,
    collector: &SessionCollector<'_>,
    files: &[RemoteFile],
    result: &mut SyncResult,
    ssh: &str,
    server_filtered: bool,
    on_file: &mut impl FnMut(&RemoteFile),
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let host = &files[0].host;
    let requested: HashMap<_, _> = files
        .iter()
        .map(|file| (file.relative_path.as_str(), file))
        .collect();
    let paths = files
        .iter()
        .map(|file| shell_quote(&file.relative_path))
        .collect::<Vec<_>>()
        .join(" ");
    let remote_command = if server_filtered {
        format!("python3 - {paths}")
    } else {
        format!("tar -C \"$HOME/.codex/sessions\" -cf - {paths}")
    };
    let mut command = Command::new(ssh);
    command
        .args(ssh_options())
        .arg(host)
        .arg(remote_command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if server_filtered {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .with_context(|| "OpenSSH client was not found; install `ssh` first")?;
    if server_filtered {
        child
            .stdin
            .take()
            .context("SSH did not expose the metadata-filter script input")?
            .write_all(FILTER_SCRIPT.as_bytes())?;
    }
    let stdout = child
        .stdout
        .take()
        .context("SSH did not expose its Rollout stream")?;
    let stream: Box<dyn Read> = if server_filtered {
        Box::new(flate2::read::GzDecoder::new(stdout))
    } else {
        Box::new(stdout)
    };
    let mut archive = tar::Archive::new(stream);
    let mut seen = std::collections::HashSet::new();
    for entry in archive.entries().context("read remote Rollout archive")? {
        let mut entry = entry.context("read remote Rollout entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().context("decode remote Rollout path")?;
        let Ok(relative) = safe_relative_path(&path.to_string_lossy()) else {
            continue;
        };
        let Some(file) = requested.get(relative.as_str()) else {
            continue;
        };
        seen.insert(relative.clone());
        let parsed = match collector.collect_reader(BufReader::new(&mut entry), file.source_path())
        {
            Ok(parsed) => parsed,
            Err(_) => {
                result.failed_files += 1;
                on_file(file);
                continue;
            }
        };
        let inserted = match storage.import_session(
            &parsed,
            SourceMetadata {
                source_path: file.source_path(),
                size_bytes: file.size_bytes,
                mtime_ns: file.mtime_ns,
            },
        ) {
            Ok(inserted) => inserted,
            Err(_) => {
                result.failed_files += 1;
                on_file(file);
                continue;
            }
        };
        result.imported_files += 1;
        result.inserted_turns += inserted.0;
        result.inserted_calls += inserted.1;
        result.inserted_tools += inserted.2;
        on_file(file);
    }
    drop(archive);
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "{host}: {}",
            short_error(&String::from_utf8_lossy(&output.stderr))
        );
    }
    for file in files {
        if !seen.contains(&file.relative_path) {
            result.failed_files += 1;
            on_file(file);
        }
    }
    Ok(())
}

fn ssh_options() -> [&'static str; 10] {
    [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        "-o",
        "ServerAliveInterval=10",
        "-o",
        "ServerAliveCountMax=3",
        "-T",
        "--",
    ]
}

fn safe_relative_path(value: &str) -> Result<String> {
    let normalized = value.strip_prefix("./").unwrap_or(value);
    let path = Path::new(normalized);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if normalized.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || normalized
            .chars()
            .any(|character| matches!(character, '\0' | '\n' | '\r' | '\t'))
        || !name.starts_with("rollout-")
        || !name.ends_with(".jsonl")
    {
        bail!("unsafe remote Rollout path");
    }
    Ok(normalized.to_owned())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn short_error(value: &str) -> String {
    let message = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if message.is_empty() {
        "remote operation failed".into()
    } else {
        message.chars().take(300).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::io::Cursor;

    #[test]
    fn rejects_archive_traversal_and_accepts_rollouts() {
        assert!(safe_relative_path("2026/08/rollout-one.jsonl").is_ok());
        assert!(safe_relative_path("../rollout-one.jsonl").is_err());
        assert!(safe_relative_path("2026/08/notes.jsonl").is_err());
    }

    #[test]
    fn shell_quotes_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }

    #[cfg(unix)]
    #[test]
    fn list_timeout_drains_pipes_and_reports_the_host() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf 'login banner\\n'; printf 'diagnostic\\n' >&2; exec sleep 5",
        ]);
        let error = command_output_with_timeout(
            &mut command,
            Duration::from_millis(100),
            "SSH connection to example-host timed out",
            "OpenSSH client was not found; install `ssh` first",
        )
        .unwrap_err()
        .to_string();
        assert_eq!(error, "SSH connection to example-host timed out");
    }

    #[test]
    fn only_not_found_is_reported_as_missing_ssh() {
        let mut command = Command::new("/definitely/not/a/codex-meter-command");
        let error = command_output_with_timeout(
            &mut command,
            Duration::from_secs(1),
            "timeout",
            "OpenSSH client was not found; install `ssh` first",
        )
        .unwrap_err()
        .to_string();
        assert_eq!(error, "OpenSSH client was not found; install `ssh` first");
    }

    #[cfg(unix)]
    #[test]
    fn server_filter_removes_content_and_preserves_collector_metrics() {
        let temporary = tempfile::tempdir().unwrap();
        let relative = "2026/08/12/rollout-filter-test.jsonl";
        let rollout = temporary.path().join(".codex/sessions").join(relative);
        std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        let secret = "REMOTE-PROMPT-MUST-NEVER-CROSS-SSH";
        let raw = format!(
            concat!(
                "{{\"timestamp\":\"2026-08-12T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"thread-1\",\"cwd\":\"/work/project-a\",\"git\":{{\"branch\":\"main\"}}}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:01Z\",\"type\":\"turn_context\",\"payload\":{{\"turn_id\":\"turn-1\",\"model\":\"gpt-5.6-sol\",\"effort\":\"high\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:02Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"turn_started\",\"turn_id\":\"turn-1\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:03Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{secret}\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:04Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":10,\"reasoning_output_tokens\":4,\"total_tokens\":110}},\"last_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":10,\"reasoning_output_tokens\":4,\"total_tokens\":110}}}}}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:05Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"exec_command_begin\",\"call_id\":\"tool-1\",\"turn_id\":\"turn-1\",\"command\":\"{secret}\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:06Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"exec_command_end\",\"call_id\":\"tool-1\",\"duration\":{{\"secs\":1,\"nanos\":500000000}},\"exit_code\":0,\"output\":\"{secret}\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:07Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"turn_complete\",\"turn_id\":\"turn-1\",\"duration_ms\":5000}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:08Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"agent_message\",\"message\":\"{secret}\"}}}}\n"
            ),
            secret = secret
        );
        std::fs::write(&rollout, &raw).unwrap();

        let mut child = Command::new("python3")
            .arg("-")
            .arg(relative)
            .env("HOME", temporary.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(FILTER_SCRIPT.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output
                .stdout
                .windows(secret.len())
                .any(|part| part == secret.as_bytes())
        );

        let mut archive =
            tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(output.stdout)));
        let mut entries = archive.entries().unwrap();
        let mut entry = entries.next().unwrap().unwrap();
        let mut filtered = String::new();
        entry.read_to_string(&mut filtered).unwrap();
        assert!(entries.next().is_none());
        assert!(!filtered.contains(secret));
        assert!(!filtered.contains("user_message"));
        assert!(!filtered.contains("agent_message"));
        assert!(!filtered.contains("\"command\":"));
        assert!(!filtered.contains("\"output\":"));
        assert!(filtered.len() < raw.len());

        let catalog = PricingCatalog::bundled().unwrap();
        let collector = SessionCollector::new(&catalog);
        let raw_session = collector
            .collect_reader(Cursor::new(raw.as_bytes()), "same-source")
            .unwrap();
        let filtered_session = collector
            .collect_reader(Cursor::new(filtered.as_bytes()), "same-source")
            .unwrap();
        assert_eq!(filtered_session, raw_session);
    }

    #[cfg(unix)]
    #[test]
    fn filtered_ssh_sync_is_incremental_private_and_reports_progress() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let relative = "2026/08/12/rollout-sync-test.jsonl";
        let rollout = temporary.path().join(".codex/sessions").join(relative);
        std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        let secret = "SSH-STREAM-MUST-NOT-CONTAIN-THIS-PROMPT";
        let raw = format!(
            concat!(
                "{{\"timestamp\":\"2026-08-12T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"thread-sync\",\"cwd\":\"/work/remote-project\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:01Z\",\"type\":\"turn_context\",\"payload\":{{\"turn_id\":\"turn-sync\",\"model\":\"gpt-5.6-sol\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:02Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{secret}\"}}}}\n",
                "{{\"timestamp\":\"2026-08-12T00:00:03Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":10,\"total_tokens\":110}},\"last_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":10,\"total_tokens\":110}}}}}}}}\n"
            ),
            secret = secret
        );
        std::fs::write(&rollout, raw).unwrap();

        let fake_ssh = temporary.path().join("fake-ssh");
        std::fs::write(
            &fake_ssh,
            format!(
                "#!/bin/sh\nexport HOME={}\nlast=\nfor argument do last=$argument; done\nexec sh -c \"$last\"\n",
                shell_quote(&temporary.path().to_string_lossy())
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o700)).unwrap();

        let catalog = PricingCatalog::bundled().unwrap();
        let mut storage = Storage::with_identity(
            temporary.path().join("meter/meter.db"),
            Some(501),
            "tester",
            None,
        )
        .unwrap();
        storage.migrate().unwrap();
        storage.sync_pricing(&catalog).unwrap();
        let mut updates = Vec::new();
        let result = sync_with_progress_using(
            &mut storage,
            &catalog,
            "devbox",
            false,
            fake_ssh.to_str().unwrap(),
            |progress| updates.push(progress.clone()),
        )
        .unwrap();
        assert_eq!(result.discovered_files, 1);
        assert_eq!(result.imported_files, 1);
        assert_eq!(result.inserted_calls, 1);
        assert_eq!(
            storage.overview(None, None, None).unwrap().total_tokens,
            110
        );
        assert_eq!(storage.project_names().unwrap(), ["remote-project"]);
        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|progress| progress.server_filtered));
        assert_eq!(updates[0].completed_files, 0);
        assert_eq!(updates[1].completed_files, 1);
        assert_eq!(
            updates[1].completed_source_bytes,
            updates[1].total_source_bytes
        );
        for entry in walkdir::WalkDir::new(temporary.path().join("meter")) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                let contents = std::fs::read(entry.path()).unwrap();
                assert!(
                    !contents
                        .windows(secret.len())
                        .any(|part| part == secret.as_bytes())
                );
            }
        }

        let replay = sync_with_progress_using(
            &mut storage,
            &catalog,
            "devbox",
            false,
            fake_ssh.to_str().unwrap(),
            |_| {},
        )
        .unwrap();
        assert_eq!(replay.imported_files, 0);
        assert_eq!(replay.skipped_files, 1);
    }
}
