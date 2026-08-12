//! Incremental SSH Rollout sources.

use crate::collector::SessionCollector;
use crate::config::validate_remote_host;
use crate::pricing::PricingCatalog;
use crate::storage::{SourceMetadata, Storage};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Component, Path};
use std::process::{Command, Stdio};

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

pub fn list(host: &str) -> Result<Vec<RemoteFile>> {
    list_with_ssh(host, "ssh")
}

fn list_with_ssh(host: &str, ssh: &str) -> Result<Vec<RemoteFile>> {
    let host = validate_remote_host(host)?;
    let command = format!("sh -c {}", shell_quote(LIST_SCRIPT));
    let output = Command::new(ssh)
        .args(ssh_options())
        .arg(&host)
        .arg(command)
        .output()
        .with_context(|| "OpenSSH client was not found; install `ssh` first")?;
    if !output.status.success() {
        bail!(
            "{host}: {}",
            short_error(&String::from_utf8_lossy(&output.stderr))
        );
    }
    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
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

pub fn sync(
    storage: &mut Storage,
    catalog: &PricingCatalog,
    host: &str,
    force: bool,
) -> Result<SyncResult> {
    let files = list(host)?;
    let host = validate_remote_host(host)?;
    let mut result = SyncResult {
        host,
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
    let collector = SessionCollector::new(catalog);
    for batch in changed.chunks(64) {
        import_batch(storage, &collector, batch, &mut result, "ssh")?;
    }
    Ok(result)
}

fn import_batch(
    storage: &mut Storage,
    collector: &SessionCollector<'_>,
    files: &[RemoteFile],
    result: &mut SyncResult,
    ssh: &str,
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
    let remote_command = format!("tar -C \"$HOME/.codex/sessions\" -cf - {paths}");
    let mut child = Command::new(ssh)
        .args(ssh_options())
        .arg(host)
        .arg(remote_command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| "OpenSSH client was not found; install `ssh` first")?;
    let stdout = child
        .stdout
        .take()
        .context("SSH did not expose its Rollout stream")?;
    let mut archive = tar::Archive::new(stdout);
    let mut seen = 0usize;
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
        seen += 1;
        let size = entry.header().size().unwrap_or(file.size_bytes);
        let mtime = entry.header().mtime().unwrap_or(0).min(i64::MAX as u64) as i64;
        let parsed = match collector.collect_reader(BufReader::new(&mut entry), file.source_path())
        {
            Ok(parsed) => parsed,
            Err(_) => {
                result.failed_files += 1;
                continue;
            }
        };
        let inserted = match storage.import_session(
            &parsed,
            SourceMetadata {
                source_path: file.source_path(),
                size_bytes: size,
                mtime_ns: mtime.saturating_mul(1_000_000_000),
            },
        ) {
            Ok(inserted) => inserted,
            Err(_) => {
                result.failed_files += 1;
                continue;
            }
        };
        result.imported_files += 1;
        result.inserted_turns += inserted.0;
        result.inserted_calls += inserted.1;
        result.inserted_tools += inserted.2;
    }
    drop(archive);
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "{host}: {}",
            short_error(&String::from_utf8_lossy(&output.stderr))
        );
    }
    result.failed_files += requested.len().saturating_sub(seen);
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
}
