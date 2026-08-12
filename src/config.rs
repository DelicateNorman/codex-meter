//! Per-user configuration and identity handling.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = r#"# Codex Meter local-first configuration
[privacy]
store_prompt = false
store_response = false
store_tool_output = false
store_headers = false
diagnostic_payload_logging = false

[retention]
raw_days = 7
call_days = 90

[collector]
batch_size = 500
fail_open = true

[network]
store_payloads = false
store_headers = false
passive_capture = false
tls_diagnostic = false

[identity]
account_tracking = false
account_label = ""

[remotes]
hosts = []
"#;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub identity: IdentityConfig,
    #[serde(default)]
    pub remotes: RemoteConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdentityConfig {
    #[serde(default)]
    pub account_tracking: bool,
    #[serde(default)]
    pub account_label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteConfig {
    #[serde(default)]
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIdentity {
    pub uid: Option<u32>,
    pub username: String,
    pub account_tracking: bool,
    pub account_label: Option<String>,
}

pub fn meter_home(explicit: Option<PathBuf>) -> PathBuf {
    explicit
        .or_else(|| std::env::var_os("CODEX_METER_HOME").map(PathBuf::from))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex-meter")
        })
}

pub fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        })
}

pub fn initialize_home(home: &Path) -> Result<()> {
    fs::create_dir_all(home).with_context(|| format!("create {}", home.display()))?;
    fs::create_dir_all(home.join("logs"))?;
    set_private_directory(home);
    set_private_directory(&home.join("logs"));
    let config = home.join("config.toml");
    if !config.exists() {
        fs::write(&config, DEFAULT_CONFIG)?;
    }
    let pricing = home.join("pricing.json");
    if !pricing.exists() {
        fs::write(pricing, include_bytes!("../codex_meter/data/pricing.json"))?;
    }
    Ok(())
}

pub fn load(home: &Path) -> ConfigFile {
    fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn identity(home: &Path) -> LocalIdentity {
    let config = load(home).identity;
    let label = config
        .account_label
        .trim()
        .to_owned()
        .filter(|_| config.account_tracking);
    LocalIdentity {
        uid: local_uid(),
        username: local_username(),
        account_tracking: config.account_tracking,
        account_label: label,
    }
}

pub fn update_identity(home: &Path, enabled: bool, label: Option<&str>) -> Result<LocalIdentity> {
    initialize_home(home)?;
    let cleaned = label.unwrap_or_default().trim();
    if cleaned
        .chars()
        .any(|character| matches!(character, '\n' | '\r' | '\0'))
    {
        bail!("account label must be a single line");
    }
    update_section(
        &home.join("config.toml"),
        "identity",
        &format!(
            "[identity]\naccount_tracking = {}\naccount_label = {}\n",
            enabled,
            serde_json::to_string(cleaned)?
        ),
    )?;
    Ok(identity(home))
}

pub fn remote_hosts(home: &Path) -> Vec<String> {
    let mut seen = BTreeSet::new();
    load(home)
        .remotes
        .hosts
        .into_iter()
        .filter_map(|host| validate_remote_host(&host).ok())
        .filter(|host| seen.insert(host.clone()))
        .collect()
}

pub fn update_remote_hosts(home: &Path, hosts: &[String]) -> Result<Vec<String>> {
    initialize_home(home)?;
    let mut normalized = Vec::new();
    for host in hosts {
        let host = validate_remote_host(host)?;
        if !normalized.contains(&host) {
            normalized.push(host);
        }
    }
    update_section(
        &home.join("config.toml"),
        "remotes",
        &format!(
            "[remotes]\nhosts = {}\n",
            serde_json::to_string(&normalized)?
        ),
    )?;
    Ok(normalized)
}

pub fn validate_remote_host(value: &str) -> Result<String> {
    let host = value.trim();
    if host.is_empty()
        || host.len() > 128
        || !host
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        || !host.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        bail!(
            "remote host must be an SSH config alias containing only letters, numbers, dots, underscores, and hyphens"
        );
    }
    Ok(host.to_owned())
}

fn update_section(path: &Path, name: &str, replacement: &str) -> Result<()> {
    let text = fs::read_to_string(path)?;
    let header = format!("[{name}]");
    let mut output = Vec::new();
    let mut inserted = false;
    let mut skipping = false;
    for line in text.lines() {
        let is_header = line.starts_with('[') && line.ends_with(']');
        if line.trim() == header {
            if !inserted {
                output.extend(replacement.trim_end().lines().map(str::to_owned));
                inserted = true;
            }
            skipping = true;
            continue;
        }
        if skipping && is_header {
            skipping = false;
        }
        if !skipping {
            output.push(line.to_owned());
        }
    }
    if !inserted {
        if output.last().is_some_and(|line| !line.is_empty()) {
            output.push(String::new());
        }
        output.extend(replacement.trim_end().lines().map(str::to_owned));
    }
    fs::write(path, output.join("\n") + "\n")?;
    Ok(())
}

fn local_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

#[cfg(unix)]
fn local_uid() -> Option<u32> {
    // SAFETY: getuid has no preconditions and cannot fail.
    Some(unsafe { libc::getuid() })
}

#[cfg(not(unix))]
fn local_uid() -> Option<u32> {
    None
}

#[cfg(unix)]
fn set_private_directory(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_private_directory(_: &Path) {}

trait NonEmptyString {
    fn filter(self, predicate: impl FnOnce(&str) -> bool) -> Option<String>;
}

impl NonEmptyString for String {
    fn filter(self, predicate: impl FnOnce(&str) -> bool) -> Option<String> {
        predicate(&self).then_some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_sections_and_preserves_privacy() {
        let root = tempfile::tempdir().unwrap();
        initialize_home(root.path()).unwrap();
        update_identity(root.path(), true, Some("work")).unwrap();
        update_remote_hosts(root.path(), &["devbox".into(), "devbox".into()]).unwrap();
        let text = fs::read_to_string(root.path().join("config.toml")).unwrap();
        assert!(text.contains("[privacy]"));
        assert_eq!(identity(root.path()).account_label.as_deref(), Some("work"));
        assert_eq!(remote_hosts(root.path()), vec!["devbox"]);
    }

    #[test]
    fn rejects_command_like_host_names() {
        assert!(validate_remote_host("dev-box.example").is_ok());
        assert!(validate_remote_host("-oProxy=x").is_err());
        assert!(validate_remote_host("user@host").is_err());
        assert!(validate_remote_host("host;touch x").is_err());
    }
}
