use anyhow::{Context, Result, bail};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use std::cell::RefCell;
use std::fs;
use std::io::{self, BufReader, IsTerminal, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::analytics::{
    MetricSample, PERFORMANCE_METRICS, UsageSample, cache_summary, context_and_retry_summary,
    performance_summary, weighted_percentile,
};
use crate::app_server::{self, AppUsage, Direction, LiveEvent};
use crate::collector::{SessionCollector, discover_rollouts};
use crate::config::{self, LocalIdentity};
use crate::interactive::{InteractiveCallbacks, View, run_interactive};
use crate::models::{LlmCallRecord, Quality, TokenUsage, ToolCallRecord};
use crate::network;
use crate::otlp;
use crate::pricing::PricingCatalog;
use crate::quota::{QuotaUpdate, WeeklyQuota};
use crate::storage::{
    ExportCall, ImportStats, LiveCallTimings, LiveTurnUpdate, Storage, UsageFilter,
};
use crate::tui::{
    self, FlowRow, HistoryRow, ModelRow, NetworkOptions, NetworkRow, OverviewOptions,
};

#[derive(Debug)]
struct ExitError {
    code: i32,
    message: String,
}

impl std::fmt::Display for ExitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ExitError {}

pub fn error_exit_code(error: &anyhow::Error) -> i32 {
    error
        .downcast_ref::<ExitError>()
        .map_or(1, |error| error.code)
}

fn exit_error(code: i32, message: impl Into<String>) -> anyhow::Error {
    ExitError {
        code,
        message: message.into(),
    }
    .into()
}

#[derive(Debug, Parser)]
#[command(
    name = "codex-meter",
    version,
    about = "Local-first Codex usage observability"
)]
pub struct Args {
    /// Data directory (default: ~/.codex-meter).
    #[arg(long, global = true)]
    pub home: Option<PathBuf>,
    /// SQLite path (default: <home>/meter.db).
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,
    /// Disable ANSI colors.
    #[arg(long, global = true)]
    pub no_color: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Import Codex rollout JSONL history.
    Import {
        path: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    /// Show today's overview.
    Today(FilterArgs),
    /// Show day, week, month, or all-time overview.
    Summary(SummaryArgs),
    /// Group all usage since first use by day, week, or month.
    History(HistoryArgs),
    /// Optional manual account labels (disabled by default).
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Aggregate Codex history from SSH hosts.
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
    /// Show Model × Reasoning Effort aggregates.
    Models(DateArgs),
    /// Show recent sessions.
    Sessions {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show OTLP latency P50/P95 and throughput inputs.
    Perf(DateArgs),
    /// Show cache reuse, savings, context amplification, and retry tax.
    Cache(DateArgs),
    /// Show per-project usage and compactions.
    Projects(DateArgs),
    /// Show provider usage attribution.
    Providers(DateArgs),
    /// Show root/subagent usage attribution.
    Agents(DateArgs),
    /// Show tool timing and success aggregates.
    Tools(DateArgs),
    /// Show calls and tools for one Codex turn.
    Waterfall { turn_id: String },
    /// Refresh rollout history and redraw the live dashboard.
    Watch {
        #[arg(long, default_value_t = 2.0)]
        interval: f64,
        #[arg(long)]
        iterations: Option<usize>,
    },
    /// Print one compact usage line for shell/footer integrations.
    Statusline,
    /// Local OTLP/HTTP JSON collector.
    Otel {
        #[command(subcommand)]
        command: OtelCommand,
    },
    /// Ingest or transparently proxy Codex App Server JSONL.
    AppServer {
        #[command(subcommand)]
        command: AppServerCommand,
    },
    /// Content-free network diagnostics.
    Network {
        #[command(subcommand)]
        command: NetworkCommand,
    },
    /// Run local metadata or explicit TLS diagnostic proxies.
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    /// Export per-call metrics without payloads.
    Export(ExportArgs),
    /// Detect available Codex data sources and schema capabilities.
    Doctor,
    /// List the versioned pricing catalog.
    Pricing,
    /// Render a deterministic dashboard preview.
    Demo,
}

#[derive(Debug, Clone, ClapArgs)]
pub struct FilterArgs {
    #[arg(long)]
    pub refresh: bool,
    #[arg(long)]
    pub account: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Debug, Clone, ClapArgs)]
pub struct SummaryArgs {
    #[arg(long, value_enum, default_value_t = Period::Day)]
    pub period: Period,
    #[arg(long)]
    pub date: Option<String>,
    #[arg(long)]
    pub account: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Debug, Clone, ClapArgs)]
pub struct HistoryArgs {
    #[arg(long, value_enum, default_value_t = Group::Day)]
    pub group: Group,
    #[arg(long)]
    pub account: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
    All,
}
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Group {
    Day,
    Week,
    Month,
}

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    Status,
    Enable { label: String },
    Set { label: String },
    Disable,
    List,
    ClaimUnassigned { label: String },
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    List,
    Add {
        host: String,
    },
    Remove {
        host: String,
    },
    Sync {
        host: Option<String>,
        #[arg(long)]
        force: bool,
    },
    Test {
        host: String,
    },
}

#[derive(Debug, Clone, ClapArgs)]
pub struct DateArgs {
    #[arg(long)]
    pub date: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum OtelCommand {
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 4318)]
        port: u16,
        #[arg(long)]
        token: Option<String>,
    },
    Config {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 4318)]
        port: u16,
    },
}

#[derive(Debug, Subcommand)]
pub enum AppServerCommand {
    Ingest {
        path: String,
    },
    Proxy {
        #[arg(last = true)]
        server_command: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum NetworkCommand {
    Probe {
        #[arg(default_value = "api.openai.com")]
        host: String,
        #[arg(long, default_value_t = 443)]
        port: u16,
    },
    Capture {
        #[arg(long = "host")]
        hosts: Vec<String>,
        #[arg(long, default_value_t = 443)]
        port: u16,
        #[arg(long)]
        interface: Option<String>,
        #[arg(long, default_value_t = 15.0)]
        duration: f64,
        #[arg(long, default_value_t = 5000)]
        packet_limit: usize,
    },
    Show {
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProxyCommand {
    Tunnel {
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 8899)]
        port: u16,
    },
    Reverse {
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 8900)]
        port: u16,
        #[arg(long, default_value = "https://api.openai.com")]
        upstream: String,
    },
    TlsInit {
        #[arg(long)]
        directory: Option<PathBuf>,
    },
    Tls {
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 8901)]
        port: u16,
        #[arg(long, default_value = "https://api.openai.com")]
        upstream: String,
        #[arg(long)]
        directory: Option<PathBuf>,
        #[arg(long)]
        acknowledge_sensitive: bool,
    },
}

#[derive(Debug, ClapArgs)]
pub struct ExportArgs {
    #[arg(long = "from")]
    pub from_date: Option<String>,
    #[arg(long = "to")]
    pub to_date: Option<String>,
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long, value_enum, default_value_t = ExportFormat::Json)]
    pub format: ExportFormat,
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ExportFormat {
    Json,
    Jsonl,
    Csv,
}

pub fn run() -> Result<()> {
    let args = Args::parse();
    dispatch(args)
}

fn dispatch(args: Args) -> Result<()> {
    let Args {
        home,
        db,
        no_color,
        command,
    } = args;
    if matches!(command, Some(Command::Demo)) {
        println!(
            "{}",
            demo(!no_color && std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal())
        );
        return Ok(());
    }

    let home = config::meter_home(home);
    config::initialize_home(&home)?;
    if let Some(Command::Account { command }) = &command {
        match command {
            AccountCommand::Enable { label } | AccountCommand::Set { label } => {
                print_identity(&config::update_identity(&home, true, Some(label))?);
                return Ok(());
            }
            AccountCommand::Disable => {
                print_identity(&config::update_identity(&home, false, None)?);
                return Ok(());
            }
            _ => {}
        }
    }
    let identity = config::identity(&home);
    let remotes = config::remote_hosts(&home);
    let db_path = db.unwrap_or_else(|| home.join("meter.db"));
    let catalog = PricingCatalog::from_path(&home.join("pricing.json"))
        .or_else(|_| PricingCatalog::bundled())?;
    let mut storage = open_storage(&db_path, &identity, &catalog)?;
    let color = !no_color && std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal();

    match command {
        Some(Command::Import { path, force }) => {
            import_rollouts(
                &storage,
                &catalog,
                path.as_deref()
                    .unwrap_or(&config::codex_home().join("sessions")),
                force,
                false,
            )?;
        }
        Some(Command::Doctor) => print_doctor(&storage)?,
        Some(Command::Pricing) => print_pricing(&catalog),
        Some(Command::Account { command }) => account_command(&storage, &identity, command)?,
        Some(Command::Remote { command }) => {
            remote_command(&mut storage, &catalog, &home, command)?
        }
        Some(Command::Models(args)) => print_models(&storage, args.date.as_deref(), color)?,
        Some(Command::Sessions { limit }) => print_sessions(&storage, limit)?,
        Some(Command::Perf(args)) => print_perf(&storage, args.date.as_deref())?,
        Some(Command::Cache(args)) => print_cache(&storage, &catalog, args.date.as_deref())?,
        Some(Command::Projects(args)) => print_projects(&storage, args.date.as_deref())?,
        Some(Command::Providers(args)) => print_providers(&storage, args.date.as_deref())?,
        Some(Command::Agents(args)) => print_agents(&storage, args.date.as_deref())?,
        Some(Command::Tools(args)) => print_tools(&storage, args.date.as_deref())?,
        Some(Command::Waterfall { turn_id }) => print_waterfall(&storage, &turn_id)?,
        Some(Command::Watch {
            interval,
            iterations,
        }) => watch(
            &mut storage,
            &catalog,
            &remotes,
            interval,
            iterations,
            color,
        )?,
        Some(Command::Statusline) => print_statusline(&storage)?,
        Some(Command::Otel { command }) => otel_command(&db_path, &identity, &catalog, command)?,
        Some(Command::AppServer { command }) => {
            app_server_command(&db_path, &identity, &catalog, command)?
        }
        Some(Command::Network { command }) => network_command(&storage, command)?,
        Some(Command::Proxy { command }) => {
            proxy_command(&db_path, &identity, &catalog, &home, command)?
        }
        Some(Command::Export(args)) => export(&storage, &args)?,
        Some(Command::History(args)) => {
            if args.refresh {
                refresh_sources(&mut storage, &catalog, &remotes, true)?;
            }
            print_history(
                &storage,
                args.group,
                args.account.as_deref(),
                args.project.as_deref(),
                color,
            )?;
        }
        Some(Command::Summary(args)) => {
            if args.refresh {
                refresh_sources(&mut storage, &catalog, &remotes, true)?;
            }
            print_summary(&storage, &remotes, &args, color, None, None)?;
        }
        Some(Command::Today(args)) => {
            import_rollouts(
                &storage,
                &catalog,
                &config::codex_home().join("sessions"),
                false,
                true,
            )?;
            if args.refresh {
                sync_remotes(&mut storage, &catalog, &remotes, false, true)?;
            }
            print_today(
                &storage,
                &remotes,
                args.account.as_deref(),
                args.project.as_deref(),
                color,
            )?;
        }
        None => {
            import_rollouts(
                &storage,
                &catalog,
                &config::codex_home().join("sessions"),
                false,
                true,
            )?;
            if io::stdin().is_terminal() && io::stdout().is_terminal() {
                interactive(storage, catalog, remotes, color)?;
            } else {
                sync_remotes(&mut storage, &catalog, &remotes, false, true)?;
                print_today(&storage, &remotes, None, None, color)?;
            }
        }
        Some(Command::Demo) => unreachable!(),
    }
    Ok(())
}

fn open_storage(
    path: &Path,
    identity: &LocalIdentity,
    catalog: &PricingCatalog,
) -> Result<Storage> {
    let storage = Storage::with_identity(
        path,
        identity.uid.map(i64::from),
        &identity.username,
        identity.account_label.clone(),
    )?;
    storage.migrate()?;
    storage.sync_pricing(catalog)?;
    storage.backfill_unpriced_calls(catalog)?;
    Ok(storage)
}

fn import_rollouts(
    storage: &Storage,
    catalog: &PricingCatalog,
    path: &Path,
    force: bool,
    quiet: bool,
) -> Result<ImportStats> {
    let files = discover_rollouts(path)?;
    let collector = SessionCollector::new(catalog);
    let mut parsed_count = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut stats = ImportStats::default();
    let mut malformed = 0;
    let mut duplicates = 0;
    for rollout in files {
        if !force && storage.file_is_current(&rollout)? {
            skipped += 1;
            continue;
        }
        match collector.collect_file(&rollout).and_then(|parsed| {
            malformed += parsed.malformed_lines;
            duplicates += parsed.duplicate_usage_events;
            storage.import_file(&parsed, &rollout)
        }) {
            Ok(inserted) => {
                parsed_count += 1;
                stats.0 += inserted.0;
                stats.1 += inserted.1;
                stats.2 += inserted.2;
            }
            Err(error) => {
                failed += 1;
                if !quiet {
                    eprintln!("warning: {}: {error:#}", rollout.display());
                }
            }
        }
    }
    if !quiet {
        println!(
            "Imported {parsed_count} file(s), skipped {skipped}, failed {failed}; {} turns, {} LLM calls, {} tools; ignored {duplicates} duplicate usage event(s), {malformed} malformed line(s).",
            stats.0, stats.1, stats.2
        );
    }
    if failed > 0 {
        bail!("{failed} Rollout file(s) could not be imported");
    }
    Ok(stats)
}

fn refresh_sources(
    storage: &mut Storage,
    catalog: &PricingCatalog,
    remotes: &[String],
    quiet: bool,
) -> Result<()> {
    import_rollouts(
        storage,
        catalog,
        &config::codex_home().join("sessions"),
        false,
        quiet,
    )?;
    sync_remotes(storage, catalog, remotes, false, quiet)
}

fn sync_remotes(
    storage: &mut Storage,
    catalog: &PricingCatalog,
    hosts: &[String],
    force: bool,
    quiet: bool,
) -> Result<()> {
    let mut failed = Vec::new();
    for host in hosts {
        let progress_terminal = io::stderr().is_terminal();
        let mut progress_printed = false;
        let mut last_completed = usize::MAX;
        let sync = if quiet {
            crate::remote::sync(storage, catalog, host, force)
        } else {
            crate::remote::sync_with_progress(storage, catalog, host, force, |progress| {
                if progress.total_files == 0 || progress.completed_files == last_completed {
                    return;
                }
                last_completed = progress.completed_files;
                progress_printed = true;
                let percent = if progress.total_source_bytes == 0 {
                    100
                } else {
                    progress
                        .completed_source_bytes
                        .saturating_mul(100)
                        .checked_div(progress.total_source_bytes)
                        .unwrap_or(0)
                        .min(100)
                };
                let mode = if progress.server_filtered {
                    "server metadata scan"
                } else {
                    "legacy full transfer"
                };
                let message = format!(
                    "{}: {mode} {}/{} · {percent:>3}% · {} / {} source scanned",
                    progress.host,
                    progress.completed_files,
                    progress.total_files,
                    human_bytes(progress.completed_source_bytes),
                    human_bytes(progress.total_source_bytes),
                );
                if progress_terminal {
                    eprint!("\r\x1b[2K{message}");
                    let _ = io::stderr().flush();
                } else {
                    eprintln!("{message}");
                }
            })
        };
        if progress_terminal && progress_printed {
            eprintln!();
        }
        match sync {
            Ok(result) if !quiet => println!(
                "{}: discovered {}, imported {}, skipped {}, failed {}; {} turns, {} LLM calls, {} tools.",
                result.host,
                result.discovered_files,
                result.imported_files,
                result.skipped_files,
                result.failed_files,
                result.inserted_turns,
                result.inserted_calls,
                result.inserted_tools
            ),
            Ok(_) => {}
            Err(error) => {
                failed.push(format!("{host}: {error:#}"));
                if !quiet {
                    eprintln!("warning: {host}: {error:#}");
                }
            }
        }
    }
    if !failed.is_empty() && !quiet {
        bail!("{} remote host(s) failed", failed.len());
    }
    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn account_command(
    storage: &Storage,
    identity: &LocalIdentity,
    command: AccountCommand,
) -> Result<()> {
    match command {
        AccountCommand::Status => {
            print_identity(identity);
            println!(
                "Account labels are manual metadata; Codex credentials and auth files are never read."
            );
        }
        AccountCommand::List => {
            println!(
                "{:<24} {:>9} {:>9} {:>14} {:<21} {:<21} {:>12}",
                "ACCOUNT", "SESSIONS", "CALLS", "TOKENS", "FIRST", "LAST", "COST"
            );
            for row in storage.account_breakdown()? {
                println!(
                    "{:<24} {:>9} {:>9} {:>14} {:<21} {:<21} {:>12}",
                    truncate(&row.account, 24),
                    row.sessions,
                    row.calls,
                    row.total_tokens,
                    truncate(row.first_used_at.as_deref().unwrap_or("N/A"), 20),
                    truncate(row.last_used_at.as_deref().unwrap_or("N/A"), 20),
                    tui::money(row.cost_usd),
                );
            }
        }
        AccountCommand::ClaimUnassigned { label } => {
            let label = validated_account_label(&label)?;
            let count = storage.claim_unassigned_account(label)?;
            println!(
                "Assigned {count} existing session(s) for OS user {} to account {label:?}.",
                storage.owner_username
            );
        }
        AccountCommand::Enable { .. } | AccountCommand::Set { .. } | AccountCommand::Disable => {
            unreachable!()
        }
    }
    Ok(())
}

fn print_identity(identity: &LocalIdentity) {
    let state = match (&identity.account_label, identity.account_tracking) {
        (Some(label), true) => format!("enabled · current label {label:?}"),
        _ => "disabled".into(),
    };
    println!(
        "OS user: {} (uid {})\nAccount tracking: {state}",
        identity.username,
        identity
            .uid
            .map_or_else(|| "N/A".into(), |value| value.to_string())
    );
}

fn remote_command(
    storage: &mut Storage,
    catalog: &PricingCatalog,
    home: &Path,
    command: RemoteCommand,
) -> Result<()> {
    match command {
        RemoteCommand::List => {
            let hosts = config::remote_hosts(home);
            if hosts.is_empty() {
                println!(
                    "No remote sources configured. Add one with: codex-meter remote add <ssh-alias>"
                );
            } else {
                for host in hosts {
                    println!("{host}");
                }
            }
        }
        RemoteCommand::Add { host } => {
            let host = validated_remote_host(&host)?;
            let files = crate::remote::list(&host)?;
            let mut hosts = config::remote_hosts(home);
            if !hosts.contains(&host) {
                hosts.push(host.clone());
                config::update_remote_hosts(home, &hosts)?;
            }
            println!(
                "Added remote source {host}; found {} Codex Rollout file(s).",
                files.len()
            );
            println!("Syncing metadata now; raw prompts and responses will not be saved locally.");
            sync_remotes(storage, catalog, &[host], false, false)?;
        }
        RemoteCommand::Remove { host } => {
            let host = validated_remote_host(&host)?;
            let mut hosts = config::remote_hosts(home);
            let original = hosts.len();
            hosts.retain(|item| item != &host);
            config::update_remote_hosts(home, &hosts)?;
            if hosts.len() == original {
                return Err(exit_error(
                    1,
                    format!("Remote source {host:?} is not configured."),
                ));
            } else {
                println!("Removed {host}. Existing aggregate history was retained.");
            }
        }
        RemoteCommand::Sync { host, force } => {
            let hosts = match host {
                Some(host) => vec![validated_remote_host(&host)?],
                None => config::remote_hosts(home),
            };
            sync_remotes(storage, catalog, &hosts, force, false)?;
        }
        RemoteCommand::Test { host } => {
            let host = validated_remote_host(&host)?;
            let files = crate::remote::list(&host)?;
            println!(
                "{host}: SSH access OK; {} Rollout file(s) found.",
                files.len()
            );
        }
    }
    Ok(())
}

fn validated_remote_host(value: &str) -> Result<String> {
    config::validate_remote_host(value).map_err(|error| exit_error(2, error.to_string()))
}

fn validated_account_label(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() {
        Err(exit_error(2, "account label cannot be empty"))
    } else {
        Ok(value)
    }
}

fn period_bounds(
    period: Period,
    anchor: Option<&str>,
) -> Result<(Option<String>, Option<String>, String)> {
    let anchor = match anchor {
        Some(value) => NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|error| {
            exit_error(
                2,
                format!("invalid date {value:?}; expected YYYY-MM-DD: {error}"),
            )
        })?,
        None => Local::now().date_naive(),
    };
    Ok(match period {
        Period::All => (None, None, "ALL TIME · SINCE FIRST USE".into()),
        Period::Day => {
            let value = anchor.to_string();
            (
                Some(value.clone()),
                Some(value.clone()),
                format!("DAY · {value}"),
            )
        }
        Period::Week => {
            let start =
                anchor - ChronoDuration::days(i64::from(anchor.weekday().num_days_from_monday()));
            let end = start + ChronoDuration::days(6);
            (
                Some(start.to_string()),
                Some(end.to_string()),
                format!("WEEK · {start} → {end}"),
            )
        }
        Period::Month => {
            let start = anchor.with_day(1).expect("day one is valid");
            let (year, month) = if start.month() == 12 {
                (start.year() + 1, 1)
            } else {
                (start.year(), start.month() + 1)
            };
            let end = NaiveDate::from_ymd_opt(year, month, 1).expect("valid next month")
                - ChronoDuration::days(1);
            (
                Some(start.to_string()),
                Some(end.to_string()),
                format!("MONTH · {}", start.format("%Y-%m")),
            )
        }
    })
}

fn print_today(
    storage: &Storage,
    remotes: &[String],
    account: Option<&str>,
    project: Option<&str>,
    color: bool,
) -> Result<()> {
    let args = SummaryArgs {
        period: Period::Day,
        date: None,
        account: account.map(str::to_owned),
        project: project.map(str::to_owned),
        refresh: false,
    };
    print_summary(storage, remotes, &args, color, None, None)
}

fn print_summary(
    storage: &Storage,
    remotes: &[String],
    args: &SummaryArgs,
    color: bool,
    quotas: Option<&[WeeklyQuota]>,
    quota_message: Option<&str>,
) -> Result<()> {
    let (from, to, mut label) = period_bounds(args.period, args.date.as_deref())?;
    if let Some(account) = &args.account {
        label.push_str(&format!(" · ACCOUNT {account}"));
    }
    if let Some(project) = &args.project {
        label.push_str(&format!(" · PROJECT {project}"));
    }
    let filter = UsageFilter {
        from_date: from.as_deref(),
        to_date: to.as_deref(),
        account: args.account.as_deref(),
        project: args.project.as_deref(),
    };
    let overview = storage.overview_range(filter)?;
    let models = storage.model_breakdown_range(filter)?;
    let source = if remotes.is_empty() {
        "LOCAL".into()
    } else {
        format!("LOCAL + {} REMOTE", remotes.len())
    };
    let mut options = OverviewOptions::new(&label, terminal_width(), color);
    options.source_label = &source;
    options.weekly_quotas = quotas;
    options.quota_message = quota_message;
    println!(
        "{}",
        tui::render_overview(
            &overview.into(),
            &models.into_iter().map(Into::into).collect::<Vec<_>>(),
            &options
        )
    );
    Ok(())
}

fn print_history(
    storage: &Storage,
    group: Group,
    account: Option<&str>,
    project: Option<&str>,
    color: bool,
) -> Result<()> {
    let group = match group {
        Group::Day => "day",
        Group::Week => "week",
        Group::Month => "month",
    };
    let rows: Vec<HistoryRow> = storage
        .usage_history(group, account, project)?
        .into_iter()
        .map(Into::into)
        .collect();
    println!(
        "{}",
        tui::render_history(&rows, group, &storage.owner_username, project, color)
    );
    Ok(())
}

fn print_models(storage: &Storage, date: Option<&str>, color: bool) -> Result<()> {
    let filter = UsageFilter {
        from_date: date,
        to_date: date,
        ..Default::default()
    };
    let rows = storage
        .model_breakdown(filter)?
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>();
    println!("{}", tui::render_models(&rows, color));
    Ok(())
}

fn print_sessions(storage: &Storage, limit: usize) -> Result<()> {
    println!(
        "{:<16} {:<24} {:<21} {:>6} {:>6} {:>12} {:>7} {:>11}",
        "SESSION", "PROJECT", "STARTED", "TURNS", "CALLS", "TOKENS", "CACHE", "COST"
    );
    for row in storage.sessions(limit.max(1))? {
        let cache = percent(row.cached_input_tokens, row.input_tokens);
        println!(
            "{:<16} {:<24} {:<21} {:>6} {:>6} {:>12} {:>6.1}% {:>11}",
            truncate(&row.codex_thread_id, 15),
            truncate(row.project_name.as_deref().unwrap_or("Unknown"), 24),
            truncate(row.started_at.as_deref().unwrap_or("Unknown"), 20),
            row.turns,
            row.calls,
            row.total_tokens,
            cache,
            tui::money(row.cost_usd)
        );
    }
    Ok(())
}

impl From<crate::storage::Overview> for tui::Overview {
    fn from(row: crate::storage::Overview) -> Self {
        Self {
            input_tokens: row.input_tokens,
            cached_input_tokens: row.cached_input_tokens,
            cache_write_tokens: row.cache_write_tokens,
            output_tokens: row.output_tokens,
            reasoning_tokens: row.reasoning_tokens,
            total_tokens: row.total_tokens,
            cost_usd: row.cost_usd,
            unpriced_calls: row.unpriced_calls,
            historical_price_estimate_calls: row.historical_price_estimate_calls,
            calls: row.calls,
            sessions: row.sessions,
            turns: row.turns,
            avg_ttft_ms: row.avg_ttft_ms,
            avg_e2e_ms: row.avg_e2e_ms,
        }
    }
}

impl From<crate::storage::ModelUsage> for ModelRow {
    fn from(row: crate::storage::ModelUsage) -> Self {
        Self {
            model: row.model,
            effort: row.effort,
            calls: row.calls,
            input_tokens: row.input_tokens,
            cached_input_tokens: row.cached_input_tokens,
            output_tokens: row.output_tokens,
            reasoning_tokens: row.reasoning_tokens,
            total_tokens: row.total_tokens,
            cost_usd: row.cost_usd,
        }
    }
}

impl From<crate::storage::HistoryBucket> for HistoryRow {
    fn from(row: crate::storage::HistoryBucket) -> Self {
        Self {
            period_start: row.period_start,
            sessions: row.sessions,
            turns: row.turns,
            calls: row.calls,
            input_tokens: row.input_tokens,
            cached_input_tokens: row.cached_input_tokens,
            total_tokens: row.total_tokens,
            cost_usd: row.cost_usd,
        }
    }
}

fn print_perf(storage: &Storage, date: Option<&str>) -> Result<()> {
    let samples = storage
        .metric_points(date, PERFORMANCE_METRICS)?
        .into_iter()
        .map(|row| MetricSample {
            name: row.name,
            event_fingerprint: row.event_fingerprint,
            attributes_json: serde_json::to_string(&row.attributes).unwrap_or_default(),
            start_time_unix_nano: row.start_time_unix_nano,
            time_unix_nano: row.time_unix_nano,
            value: row.value,
            point_sum: row.point_sum,
            point_count: row.point_count.and_then(|value| u64::try_from(value).ok()),
            point_max: row.point_max,
            explicit_bounds: row.explicit_bounds,
            bucket_counts: row
                .bucket_counts
                .into_iter()
                .filter_map(|value| u64::try_from(value).ok())
                .collect(),
        })
        .collect::<Vec<_>>();
    println!(
        "{:<58} {:>8} {:>12} {:>12} {:>12} {:>9}",
        "METRIC", "COUNT", "AVG", "P50", "P95", "TPS"
    );
    let rows = performance_summary(&samples);
    for row in &rows {
        println!(
            "{:<58} {:>8} {:>12} {:>12} {:>12} {:>9}",
            truncate(&row.name, 58),
            row.count,
            milliseconds(row.average),
            milliseconds(row.p50),
            milliseconds(row.p95),
            row.tps
                .map_or_else(|| "N/A".into(), |value| format!("{value:.2}"))
        );
    }
    if rows.is_empty() {
        println!(
            "No OTLP performance points. Run `codex-meter otel config`, apply it, then `codex-meter otel serve`."
        );
    }
    println!("Histogram percentiles are bucket approximations; AVG uses the exported sum/count.");
    Ok(())
}

fn usage_samples(storage: &Storage, date: Option<&str>) -> Result<Vec<UsageSample>> {
    Ok(storage
        .usage_calls(date)?
        .into_iter()
        .enumerate()
        .map(|(index, row)| UsageSample {
            id: index as i64,
            turn_id: row.codex_turn_id,
            model: row.call.model,
            provider: row.call.provider,
            completed_at: row.call.completed_at,
            usage: row.call.usage,
            retry_index: row.call.retry_index,
            cost_usd: row.call.cost_usd,
        })
        .collect())
}

fn print_cache(storage: &Storage, catalog: &PricingCatalog, date: Option<&str>) -> Result<()> {
    let samples = usage_samples(storage, date)?;
    let cache = cache_summary(&samples, catalog);
    let context = context_and_retry_summary(&samples);
    let telemetry = storage.telemetry_retry_summary(date)?;
    println!("Cache and efficiency");
    println!("Input tokens          {:>14}", cache.input_tokens);
    println!("Cached input          {:>14}", cache.cached_input_tokens);
    println!("Cache write           {:>14}", cache.cache_write_tokens);
    println!("Reuse rate            {:>13.1}%", cache.reuse_rate * 100.0);
    println!("API-equiv cost        ${:>13.4}", cache.observed_cost_usd);
    println!("Without cache         ${:>13.4}", cache.without_cache_usd);
    println!("Estimated savings     ${:>13.4}", cache.savings_usd);
    println!(
        "Context amplification {:>14}",
        optional_ratio(context.average_context_amplification)
    );
    println!(
        "Maximum amplification {:>14}",
        optional_ratio(context.max_context_amplification)
    );
    println!("Explicit retry calls  {:>14}", context.retry_calls);
    println!("Retry tokens          {:>14}", context.retry_tokens);
    println!("Retry API-equiv       ${:>13.4}", context.retry_cost_usd);
    println!("OTel retry attempts   {:>14}", telemetry.attempts);
    println!(
        "OTel retry time       {:>14}",
        milliseconds(Some(telemetry.duration_ms))
    );
    if cache.unpriced_calls > 0 {
        println!(
            "Unpriced calls: {} (excluded from cost/savings).",
            cache.unpriced_calls
        );
    }
    Ok(())
}

fn print_projects(storage: &Storage, date: Option<&str>) -> Result<()> {
    println!(
        "{:<30} {:>5} {:>5} {:>5} {:>13} {:>7} {:>11} {:>7} {:>11}",
        "PROJECT", "SESS", "TURN", "CALL", "TOKENS", "CACHE", "RETRY", "COMPACT", "COST"
    );
    for row in storage.project_breakdown(date)? {
        println!(
            "{:<30} {:>5} {:>5} {:>5} {:>13} {:>6.1}% {:>11} {:>7} {:>11}",
            truncate(&row.project, 30),
            row.sessions,
            row.turns,
            row.calls,
            row.total_tokens,
            percent(row.cached_input_tokens, row.input_tokens),
            row.retry_tokens,
            row.compactions,
            tui::money(row.cost_usd)
        );
    }
    Ok(())
}

fn print_providers(storage: &Storage, date: Option<&str>) -> Result<()> {
    println!(
        "{:<24} {:>9} {:>8} {:>14} {:>8} {:>12}",
        "PROVIDER", "SESSIONS", "CALLS", "TOKENS", "CACHE", "COST"
    );
    for row in storage.provider_breakdown(date)? {
        println!(
            "{:<24} {:>9} {:>8} {:>14} {:>7.1}% {:>12}",
            truncate(&row.provider, 24),
            row.sessions,
            row.calls,
            row.total_tokens,
            percent(row.cached_input_tokens, row.input_tokens),
            tui::money(row.cost_usd)
        );
    }
    Ok(())
}

fn print_agents(storage: &Storage, date: Option<&str>) -> Result<()> {
    println!(
        "{:<24} {:>9} {:>8} {:>8} {:>14} {:>12}",
        "ROLE", "SESSIONS", "TURNS", "CALLS", "TOKENS", "COST"
    );
    for row in storage.agent_breakdown(date)? {
        println!(
            "{:<24} {:>9} {:>8} {:>8} {:>14} {:>12}",
            truncate(&row.role, 24),
            row.sessions,
            row.turns,
            row.calls,
            row.total_tokens,
            tui::money(row.cost_usd)
        );
    }
    Ok(())
}

fn print_tools(storage: &Storage, date: Option<&str>) -> Result<()> {
    let durations = storage.tool_durations(date)?;
    println!(
        "{:<42} {:>7} {:>9} {:>10} {:>10} {:>10} {:>11}",
        "TOOL", "CALLS", "SUCCESS", "AVG", "P50", "P95", "TOTAL"
    );
    for row in storage.tool_breakdown(date)? {
        let success = if row.known_outcomes > 0 {
            format!(
                "{:.1}%",
                row.successes as f64 / row.known_outcomes as f64 * 100.0
            )
        } else {
            "N/A".into()
        };
        let weighted = durations
            .get(&row.tool_name)
            .into_iter()
            .flatten()
            .map(|value| (*value as f64, 1))
            .collect::<Vec<_>>();
        println!(
            "{:<42} {:>7} {:>9} {:>10} {:>10} {:>10} {:>11}",
            truncate(&row.tool_name, 42),
            row.calls,
            success,
            milliseconds(row.avg_ms),
            milliseconds(weighted_percentile(&weighted, 0.5)),
            milliseconds(weighted_percentile(&weighted, 0.95)),
            milliseconds(row.total_ms.map(|value| value as f64))
        );
    }
    Ok(())
}

fn print_waterfall(storage: &Storage, turn_id: &str) -> Result<()> {
    let Some(row) = storage.turn_waterfall(turn_id)? else {
        bail!("Turn not found: {turn_id}");
    };
    println!(
        "Turn {turn_id} · {} · {}",
        row.project_name.as_deref().unwrap_or("Unknown"),
        row.status
    );
    println!(
        "TTFT={}  TTFM={}  E2E={}",
        milliseconds(row.ttft_ms.map(|v| v as f64)),
        milliseconds(row.ttfm_ms.map(|v| v as f64)),
        milliseconds(row.e2e_ms.map(|v| v as f64))
    );
    let mut events = Vec::new();
    for call in row.calls {
        events.push((
            call.completed_at.clone().unwrap_or_default(),
            "LLM",
            call.completed_at.clone(),
            call.completed_at,
            call.response_id
                .or(call.model)
                .unwrap_or_else(|| "unknown".into()),
        ));
    }
    for tool in row.tools {
        events.push((
            tool.started_at
                .clone()
                .or(tool.completed_at.clone())
                .unwrap_or_default(),
            "TOOL",
            tool.started_at,
            tool.completed_at,
            tool.tool_name,
        ));
    }
    events.sort_by(|left, right| left.0.cmp(&right.0));
    for (_, kind, started, completed, label) in events {
        println!(
            "{kind:<5} {:<27} → {:<27} {label}",
            started.as_deref().unwrap_or("N/A"),
            completed.as_deref().unwrap_or("N/A")
        );
    }
    Ok(())
}

fn print_statusline(storage: &Storage) -> Result<()> {
    let today = Local::now().date_naive().to_string();
    let row = storage.overview(Some(&today), None, None)?;
    println!(
        "Codex {} tok · cache {:.0}% · {} calls · {} · TTFT {}",
        row.total_tokens,
        percent(row.cached_input_tokens, row.input_tokens),
        row.calls,
        tui::money(row.cost_usd).replace(' ', ""),
        milliseconds(row.avg_ttft_ms)
    );
    Ok(())
}

fn print_doctor(storage: &Storage) -> Result<()> {
    println!("Codex Meter Doctor\n");
    let integrity = storage.integrity_check()?;
    for check in crate::doctor::run(&config::codex_home(), &integrity) {
        let symbol = match check.status {
            "yes" => "✓",
            "no" => "✗",
            "disabled" => "○",
            "experimental" => "△",
            _ => "?",
        };
        let detail = if check.detail.is_empty() {
            String::new()
        } else {
            format!("  {}", check.detail)
        };
        println!("{:<28} {symbol} {}{detail}", check.name, check.status);
    }
    let mut counts = storage.counts()?.into_iter().collect::<Vec<_>>();
    counts.sort_by(|left, right| left.0.cmp(&right.0));
    println!("\nDatabase: {}", storage.path.display());
    println!(
        "Rows: {}",
        counts
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("Privacy: prompts/responses/tool output/headers are never imported");
    Ok(())
}

fn print_pricing(catalog: &PricingCatalog) {
    println!(
        "{:<22} {:<22} {:>9} {:>9} {:>9} {:>9}  VERSION",
        "MODEL", "EFFECTIVE", "INPUT", "CACHED", "WRITE", "OUTPUT"
    );
    for item in &catalog.entries {
        println!(
            "{:<22} {:<22} {:>9.3} {:>9.3} {:>9.3} {:>9.3}  {}",
            item.model,
            item.effective_from,
            item.input_per_million,
            item.cached_input_per_million,
            item.cache_write_per_million,
            item.output_per_million,
            item.version
        );
    }
    println!(
        "USD per 1M tokens. Reasoning tokens are included in output and are not double-counted."
    );
}

fn watch(
    storage: &mut Storage,
    catalog: &PricingCatalog,
    remotes: &[String],
    interval: f64,
    iterations: Option<usize>,
    color: bool,
) -> Result<()> {
    let mut iteration = 0;
    while iterations.is_none_or(|limit| iteration < limit) {
        refresh_sources(storage, catalog, remotes, true)?;
        if io::stdout().is_terminal() && iteration > 0 {
            print!("\x1b[2J\x1b[H");
        }
        print_today(storage, remotes, None, None, color)?;
        io::stdout().flush()?;
        iteration += 1;
        if iterations.is_none_or(|limit| iteration < limit) {
            thread::sleep(Duration::from_secs_f64(interval.max(0.1)));
        }
    }
    Ok(())
}

fn otel_command(
    db_path: &Path,
    identity: &LocalIdentity,
    catalog: &PricingCatalog,
    command: OtelCommand,
) -> Result<()> {
    match command {
        OtelCommand::Config { host, port } => {
            let base = format!("http://{host}:{port}");
            println!("[otel]\nlog_user_prompt = false");
            println!(
                "exporter = {{ otlp-http = {{ endpoint = \"{base}/v1/logs\", protocol = \"json\" }} }}"
            );
            println!(
                "trace_exporter = {{ otlp-http = {{ endpoint = \"{base}/v1/traces\", protocol = \"json\" }} }}"
            );
            println!(
                "metrics_exporter = {{ otlp-http = {{ endpoint = \"{base}/v1/metrics\", protocol = \"json\" }} }}"
            );
        }
        OtelCommand::Serve { bind, port, token } => {
            let address: SocketAddr = format!("{bind}:{port}")
                .parse()
                .context("invalid OTLP bind address")?;
            eprintln!("OTLP collector listening on http://{address} (JSON; metadata only)");
            let path = db_path.to_path_buf();
            let identity = identity.clone();
            let catalog = catalog.clone();
            let handler: otlp::BatchHandler = Arc::new(move |batch| {
                let result = (|| -> Result<()> {
                    let storage = open_storage(&path, &identity, &catalog)?;
                    let (metrics, logs) = batch.into_records();
                    storage.insert_metric_points(&metrics)?;
                    storage.insert_telemetry_logs(&logs)?;
                    Ok(())
                })();
                if let Err(error) = result {
                    eprintln!("OTLP persistence warning: {error:#}");
                }
            });
            otlp::serve(address, token, handler)?;
        }
    }
    Ok(())
}

fn app_server_command(
    db_path: &Path,
    identity: &LocalIdentity,
    catalog: &PricingCatalog,
    command: AppServerCommand,
) -> Result<()> {
    match command {
        AppServerCommand::Ingest { path } => {
            let mut adapter = app_server::Adapter::default();
            let (events, malformed) = if path == "-" {
                app_server::ingest_stream(
                    BufReader::new(io::stdin()),
                    &mut adapter,
                    Direction::Server,
                )
            } else {
                let file = fs::File::open(&path).with_context(|| format!("open {path}"))?;
                app_server::ingest_stream(BufReader::new(file), &mut adapter, Direction::Server)
            };
            let storage = open_storage(db_path, identity, catalog)?;
            let count = events.len();
            for event in events {
                persist_live_event(&storage, catalog, event)?;
            }
            println!(
                "Ingested {count} structural event(s); ignored {malformed} malformed line(s)."
            );
        }
        AppServerCommand::Proxy { mut server_command } => {
            if server_command.first().is_some_and(|item| item == "--") {
                server_command.remove(0);
            }
            if server_command.is_empty() {
                server_command = vec!["codex".into(), "app-server".into(), "--stdio".into()];
            }
            let storage = Arc::new(Mutex::new(open_storage(db_path, identity, catalog)?));
            let event_storage = Arc::clone(&storage);
            let catalog = catalog.clone();
            let sink = Arc::new(Mutex::new(move |event| {
                let result = event_storage
                    .lock()
                    .map_err(|_| anyhow::anyhow!("App Server storage lock poisoned"))
                    .and_then(|storage| persist_live_event(&storage, &catalog, event));
                if let Err(error) = result {
                    eprintln!("App Server persistence warning: {error:#}");
                }
            }));
            let status = app_server::proxy_stdio(&server_command, sink)?;
            if status != 0 {
                return Err(exit_error(
                    status,
                    format!("Codex App Server exited with status {status}"),
                ));
            }
        }
    }
    Ok(())
}

fn persist_live_event(storage: &Storage, catalog: &PricingCatalog, event: LiveEvent) -> Result<()> {
    match event {
        LiveEvent::Session {
            thread_id,
            started_at,
            cwd,
            model,
        } => {
            storage.ensure_live_session(
                &thread_id,
                Some(&started_at),
                cwd.as_deref(),
                model.as_deref(),
                "app_server",
            )?;
        }
        LiveEvent::Turn {
            thread_id,
            turn_id,
            started_at,
            completed_at,
            status,
            model,
            effort,
            ttft_ms,
            ttfm_ms,
            e2e_ms,
        } => {
            storage.upsert_live_turn(
                &thread_id,
                &turn_id,
                LiveTurnUpdate {
                    started_at: started_at.as_deref(),
                    completed_at: completed_at.as_deref(),
                    status: &status,
                    model: model.as_deref(),
                    reasoning_effort: effort.as_deref(),
                    ttft_ms: bounded_i64(ttft_ms),
                    ttfm_ms: bounded_i64(ttfm_ms),
                    e2e_ms: bounded_i64(e2e_ms),
                    source: "app_server",
                    ..Default::default()
                },
            )?;
        }
        LiveEvent::Call {
            fingerprint,
            thread_id,
            turn_id,
            response_id,
            completed_at,
            model,
            effort,
            usage,
            started_at,
            first_event_at,
            first_message_at,
        } => {
            let usage = token_usage(usage);
            let price = catalog.resolve(model.as_deref(), Some("openai"), Some(&completed_at));
            let (cost_usd, pricing_version) = price
                .map(|price| {
                    let cost = catalog.calculate(usage, price);
                    (Some(cost.total_usd), Some(cost.pricing_version))
                })
                .unwrap_or((None, None));
            let call = LlmCallRecord {
                event_fingerprint: fingerprint,
                turn_id,
                response_id: Some(response_id),
                completed_at: Some(completed_at),
                model,
                actual_model: None,
                provider: Some("openai".into()),
                reasoning_effort: effort,
                reasoning_mode: None,
                service_tier: None,
                usage,
                success: true,
                error_type: None,
                retry_index: 0,
                cost_usd,
                pricing_version,
                quality: Quality::exact("app_server"),
            };
            let ttft_ms = time_delta_ms(started_at.as_deref(), first_event_at.as_deref());
            let ttfm_ms = time_delta_ms(started_at.as_deref(), first_message_at.as_deref());
            let request_duration_ms =
                time_delta_ms(started_at.as_deref(), call.completed_at.as_deref());
            storage.insert_live_call(
                &thread_id,
                &call,
                LiveCallTimings {
                    started_at: started_at.as_deref(),
                    first_event_at: first_event_at.as_deref(),
                    first_model_item_at: first_message_at.as_deref(),
                    ttft_ms,
                    ttfm_ms,
                    request_duration_ms,
                    transport: Some("app_server"),
                },
            )?;
        }
        LiveEvent::Tool {
            thread_id,
            turn_id,
            call_id,
            tool_name,
            started_at,
            completed_at,
            duration_ms,
            success,
            exit_code,
        } => {
            storage.upsert_live_tool(
                &thread_id,
                &ToolCallRecord {
                    call_id,
                    turn_id,
                    tool_name,
                    started_at,
                    completed_at,
                    duration_ms: bounded_i64(duration_ms),
                    success,
                    exit_code,
                    quality: Quality::exact("app_server"),
                },
                "app_server",
            )?;
        }
        LiveEvent::Compaction {
            fingerprint,
            thread_id,
            turn_id,
            occurred_at,
        } => {
            storage.insert_compaction(
                &fingerprint,
                &thread_id,
                turn_id.as_deref(),
                Some(&occurred_at),
                "app_server",
            )?;
        }
        LiveEvent::ActualModel { turn_id, model } => {
            storage.update_actual_model(&turn_id, &model)?
        }
        LiveEvent::TurnUsage {
            thread_id,
            turn_id,
            usage,
        } => storage.update_live_turn_usage(&thread_id, &turn_id, token_usage(usage))?,
    }
    Ok(())
}

fn token_usage(usage: AppUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: bounded_i64(Some(usage.input_tokens)).unwrap_or(i64::MAX),
        cached_input_tokens: bounded_i64(Some(usage.cached_input_tokens)).unwrap_or(i64::MAX),
        cache_write_tokens: bounded_i64(Some(usage.cache_write_tokens)).unwrap_or(i64::MAX),
        output_tokens: bounded_i64(Some(usage.output_tokens)).unwrap_or(i64::MAX),
        reasoning_tokens: bounded_i64(Some(usage.reasoning_tokens)).unwrap_or(i64::MAX),
        total_tokens: bounded_i64(Some(usage.total_tokens)).unwrap_or(i64::MAX),
    }
}

fn network_command(storage: &Storage, command: NetworkCommand) -> Result<()> {
    match command {
        NetworkCommand::Probe { host, port } => {
            let flow = network::probe_endpoint(&host, port, Duration::from_secs(15));
            let success = flow.success == Some(true);
            println!(
                "{}:{} ip={} DNS={} TCP={} TLS={} version={} ALPN={} success={}",
                host,
                port,
                flow.destination_ip.as_deref().unwrap_or("N/A"),
                milliseconds(flow.dns_ms),
                milliseconds(flow.tcp_ms),
                milliseconds(flow.tls_ms),
                flow.tls_version.as_deref().unwrap_or("N/A"),
                flow.alpn.as_deref().unwrap_or("N/A"),
                success
            );
            storage.insert_network_flow(&flow.into())?;
            if !success {
                bail!("network probe failed")
            }
        }
        NetworkCommand::Capture {
            mut hosts,
            port,
            interface,
            duration,
            packet_limit,
        } => {
            if hosts.is_empty() {
                hosts = vec!["api.openai.com".into(), "chatgpt.com".into()];
            }
            let flows = network::capture_metadata(
                &hosts,
                interface.as_deref(),
                port,
                Duration::from_secs_f64(duration.max(0.1)),
                packet_limit,
            )?;
            let count = flows.len();
            for flow in flows {
                println!(
                    "{} {} out={}/{}B in={}/{}B duration={}",
                    flow.destination_host.as_deref().unwrap_or("N/A"),
                    flow.destination_ip.as_deref().unwrap_or("N/A"),
                    flow.packets_out,
                    flow.request_bytes,
                    flow.packets_in,
                    flow.response_bytes,
                    milliseconds(flow.duration_ms)
                );
                storage.insert_network_flow(&flow.into())?;
            }
            println!("Saved {count} content-free flow aggregate(s).");
        }
        NetworkCommand::Show { limit } => {
            for row in storage.recent_network(limit, None)? {
                println!(
                    "{:<27} {:<18} {:<28} out={:>9}B in={:>9}B ttfb={} status={}",
                    row.started_at.as_deref().unwrap_or("N/A"),
                    row.mode,
                    row.destination_host
                        .as_deref()
                        .or(row.destination_ip.as_deref())
                        .unwrap_or("N/A"),
                    row.request_bytes,
                    row.response_bytes,
                    milliseconds(row.ttfb_ms),
                    row.http_status
                        .map_or_else(|| "N/A".into(), |value| value.to_string())
                );
            }
        }
    }
    Ok(())
}

fn proxy_command(
    db_path: &Path,
    identity: &LocalIdentity,
    catalog: &PricingCatalog,
    home: &Path,
    command: ProxyCommand,
) -> Result<()> {
    let handler = flow_handler(db_path, identity, catalog);
    match command {
        ProxyCommand::Tunnel { bind, port } => {
            let address: SocketAddr = format!("{bind}:{port}")
                .parse()
                .context("invalid proxy bind address")?;
            eprintln!("CONNECT proxy on http://{address} (TLS opaque; metadata only)");
            crate::proxy::serve_tunnel(address, handler)?;
        }
        ProxyCommand::Reverse {
            bind,
            port,
            upstream,
        } => {
            let address: SocketAddr = format!("{bind}:{port}")
                .parse()
                .context("invalid proxy bind address")?;
            eprintln!("Reverse proxy on http://{address} → {upstream}; bodies are not persisted");
            crate::proxy::serve_reverse(address, &upstream, handler)?;
        }
        ProxyCommand::TlsInit { directory } => {
            let material = crate::proxy::initialize_tls_material(
                directory.as_deref().unwrap_or(&home.join("tls")),
            )?;
            println!("CA certificate: {}", material.ca_cert.display());
            println!("Leaf certificate: {}", material.leaf_cert.display());
            println!(
                "Private keys are mode 0600. Trust only the CA certificate, and remove trust after diagnostics."
            );
        }
        ProxyCommand::Tls {
            bind,
            port,
            upstream,
            directory,
            acknowledge_sensitive,
        } => {
            if !acknowledge_sensitive {
                return Err(exit_error(
                    2,
                    "Refusing TLS termination without --acknowledge-sensitive",
                ));
            }
            let address: SocketAddr = format!("{bind}:{port}")
                .parse()
                .context("invalid proxy bind address")?;
            let material = crate::proxy::initialize_tls_material(
                directory.as_deref().unwrap_or(&home.join("tls")),
            )?;
            let tls = crate::proxy::load_tls_server_config(&material)?;
            eprintln!(
                "Trust CA for this diagnostic only: {}",
                material.ca_cert.display()
            );
            eprintln!("Reverse proxy on https://{address} → {upstream}; bodies are not persisted");
            crate::proxy::serve_reverse_with_tls(address, &upstream, Some(tls), handler)?;
        }
    }
    Ok(())
}

fn flow_handler(
    db_path: &Path,
    identity: &LocalIdentity,
    catalog: &PricingCatalog,
) -> crate::proxy::FlowHandler {
    let path = db_path.to_path_buf();
    let identity = identity.clone();
    let catalog = catalog.clone();
    Arc::new(move |flow| {
        let result = open_storage(&path, &identity, &catalog)
            .and_then(|storage| storage.insert_network_flow(&flow.into()).map(|_| ()));
        if let Err(error) = result {
            eprintln!("network metadata persistence warning: {error:#}");
        }
    })
}

fn export(storage: &Storage, args: &ExportArgs) -> Result<()> {
    let rows = storage.export_rows(
        args.from_date.as_deref(),
        args.to_date.as_deref(),
        args.session.as_deref(),
    )?;
    let text = match args.format {
        ExportFormat::Json => serde_json::to_string_pretty(&rows)?,
        ExportFormat::Jsonl => rows
            .iter()
            .map(serde_json::to_string)
            .collect::<serde_json::Result<Vec<_>>>()?
            .join("\n"),
        ExportFormat::Csv => export_csv(&rows),
    };
    if let Some(path) = &args.output {
        fs::write(path, format!("{text}\n"))
            .with_context(|| format!("write {}", path.display()))?;
        eprintln!("Exported {} call(s) to {}", rows.len(), path.display());
    } else {
        println!("{text}");
    }
    Ok(())
}

fn export_csv(rows: &[ExportCall]) -> String {
    let mut lines = vec!["session_id,turn_id,response_id,completed_at,model,reasoning_effort,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,reasoning_tokens,total_tokens,cost_usd,data_source,confidence,estimated".into()];
    for row in rows {
        lines.push(
            [
                csv(row.session_id.as_str()),
                csv_opt(row.turn_id.as_deref()),
                csv_opt(row.response_id.as_deref()),
                csv_opt(row.completed_at.as_deref()),
                csv_opt(row.model.as_deref()),
                csv_opt(row.reasoning_effort.as_deref()),
                row.usage.input_tokens.to_string(),
                row.usage.cached_input_tokens.to_string(),
                row.usage.cache_write_tokens.to_string(),
                row.usage.output_tokens.to_string(),
                row.usage.reasoning_tokens.to_string(),
                row.usage.total_tokens.to_string(),
                row.cost_usd
                    .map_or_else(String::new, |value| value.to_string()),
                csv(&row.data_source),
                csv(&row.confidence),
                row.estimated.to_string(),
            ]
            .join(","),
        );
    }
    lines.join("\n")
}

fn interactive(
    storage: Storage,
    catalog: PricingCatalog,
    remotes: Vec<String>,
    color: bool,
) -> Result<()> {
    let storage = Rc::new(RefCell::new(storage));
    let quotas = Rc::new(RefCell::new(None::<Vec<WeeklyQuota>>));
    let quota_message = Rc::new(RefCell::new(Some(
        "Loading account weekly limits…".to_owned(),
    )));
    let quota_receiver = Rc::new(RefCell::new(Some(crate::quota::spawn_weekly_quota_reader(
        Duration::from_secs(4),
    ))));
    let remote_message = Rc::new(RefCell::new((!remotes.is_empty()).then(|| {
        format!(
            "Syncing {} remote source(s) in the background…",
            remotes.len()
        )
    })));
    let worker_identity = LocalIdentity {
        uid: storage
            .borrow()
            .owner_uid
            .and_then(|value| u32::try_from(value).ok()),
        username: storage.borrow().owner_username.clone(),
        account_tracking: storage.borrow().account_label.is_some(),
        account_label: storage.borrow().account_label.clone(),
    };
    let remote_receiver = Rc::new(RefCell::new((!remotes.is_empty()).then(|| {
        spawn_remote_worker(
            storage.borrow().path.clone(),
            worker_identity.clone(),
            catalog.clone(),
            remotes.clone(),
        )
    })));
    let refresh_storage = Rc::clone(&storage);
    let refresh_catalog = catalog.clone();
    let refresh_remotes = remotes.clone();
    let refresh_identity = worker_identity.clone();
    let refresh_quota_receiver = Rc::clone(&quota_receiver);
    let refresh_remote_receiver = Rc::clone(&remote_receiver);
    let refresh_quota_message = Rc::clone(&quota_message);
    let refresh_remote_message = Rc::clone(&remote_message);
    let mut refresh = move || {
        import_rollouts(
            &refresh_storage.borrow(),
            &refresh_catalog,
            &config::codex_home().join("sessions"),
            false,
            true,
        )
        .map_err(|error| format!("{error:#}"))?;
        // Do not stack account/app-server or SSH processes when a user presses
        // r repeatedly while the previous background refresh is still live.
        let quota_idle = refresh_quota_receiver.borrow().is_none();
        if quota_idle {
            *refresh_quota_message.borrow_mut() = Some("Loading account weekly limits…".into());
            *refresh_quota_receiver.borrow_mut() = Some(crate::quota::spawn_weekly_quota_reader(
                Duration::from_secs(4),
            ));
        }
        let remotes_idle = refresh_remote_receiver.borrow().is_none();
        if !refresh_remotes.is_empty() && remotes_idle {
            *refresh_remote_message.borrow_mut() = Some(format!(
                "Syncing {} remote source(s) in the background…",
                refresh_remotes.len()
            ));
            *refresh_remote_receiver.borrow_mut() = Some(spawn_remote_worker(
                refresh_storage.borrow().path.clone(),
                refresh_identity.clone(),
                refresh_catalog.clone(),
                refresh_remotes.clone(),
            ));
        }
        Ok(())
    };
    let project_storage = Rc::clone(&storage);
    let mut list_projects = move || project_storage.borrow().project_names().unwrap_or_default();
    let content_storage = Rc::clone(&storage);
    let content_quotas = Rc::clone(&quotas);
    let content_quota_message = Rc::clone(&quota_message);
    let content_remote_message = Rc::clone(&remote_message);
    let source = if remotes.is_empty() {
        "LOCAL".into()
    } else {
        format!("LOCAL + {} REMOTE", remotes.len())
    };
    let username = storage.borrow().owner_username.clone();
    let mut render_content = move |view: View, width: usize, color: bool, project: Option<&str>| {
        render_view(
            &content_storage.borrow(),
            view,
            width,
            color,
            project,
            &source,
            &username,
            content_quotas.borrow().as_deref(),
            content_quota_message.borrow().as_deref(),
            content_remote_message.borrow().as_deref(),
        )
        .unwrap_or_else(|error| format!("Unable to render view: {error:#}"))
    };
    let poll_quotas = Rc::clone(&quotas);
    let poll_message = Rc::clone(&quota_message);
    let poll_quota_receiver = Rc::clone(&quota_receiver);
    let poll_remote_receiver = Rc::clone(&remote_receiver);
    let poll_remote_message = Rc::clone(&remote_message);
    let mut poll_updates = move || {
        let mut changed = false;
        let quota_update = poll_quota_receiver
            .borrow()
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        match quota_update {
            Some(QuotaUpdate::Loaded(rows)) => {
                *poll_quotas.borrow_mut() = Some(rows);
                *poll_message.borrow_mut() = None;
                *poll_quota_receiver.borrow_mut() = None;
                changed = true;
            }
            Some(QuotaUpdate::Unavailable(message)) => {
                *poll_quotas.borrow_mut() = Some(Vec::new());
                *poll_message.borrow_mut() = Some(format!(
                    "Account weekly limits unavailable: {message} · press r to retry"
                ));
                *poll_quota_receiver.borrow_mut() = None;
                changed = true;
            }
            None => {}
        }
        let remote_update = poll_remote_receiver.borrow().as_ref().and_then(|receiver| {
            let mut latest = None;
            while let Ok(update) = receiver.try_recv() {
                latest = Some(update);
            }
            latest
        });
        if let Some(message) = remote_update {
            *poll_remote_message.borrow_mut() = Some(message.text);
            if message.finished {
                *poll_remote_receiver.borrow_mut() = None;
            }
            changed = true;
        }
        changed
    };
    let mut callbacks = InteractiveCallbacks {
        render_content: &mut render_content,
        refresh: &mut refresh,
        list_projects: &mut list_projects,
        poll_updates: &mut poll_updates,
    };
    run_interactive(&mut callbacks, color)
        .map_err(Into::into)
        .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn render_view(
    storage: &Storage,
    view: View,
    width: usize,
    color: bool,
    project: Option<&str>,
    source: &str,
    username: &str,
    quotas: Option<&[WeeklyQuota]>,
    quota_message: Option<&str>,
    remote_message: Option<&str>,
) -> Result<String> {
    if matches!(
        view,
        View::HistoryDay | View::HistoryWeek | View::HistoryMonth
    ) {
        let group = match view {
            View::HistoryDay => "day",
            View::HistoryWeek => "week",
            _ => "month",
        };
        let rows = storage
            .usage_history(group, None, project)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>();
        return Ok(tui::render_history(&rows, group, username, project, color));
    }
    let period = match view {
        View::Today => Period::Day,
        View::Week => Period::Week,
        View::Month => Period::Month,
        View::All => Period::All,
        View::Network => {
            let today = Local::now().date_naive().to_string();
            let rows = storage
                .response_performance_range(Some(&today), Some(&today), project)?
                .into_iter()
                .map(|row| NetworkRow {
                    local_time: row.local_time.unwrap_or_else(|| "N/A".into()),
                    model: row.model,
                    output_tokens: row.output_tokens,
                    ttft_ms: row.ttft_ms.map(|value| value as f64),
                    e2e_ms: row.e2e_ms.map(|value| value as f64),
                    exact_output_tps: row.exact_output_tps,
                })
                .collect::<Vec<_>>();
            let flows = storage
                .recent_network(8, project)?
                .into_iter()
                .map(|row| FlowRow {
                    destination_host: row.destination_host,
                    destination_ip: row.destination_ip,
                    success: row.success,
                    error_type: row.error_type,
                    dns_ms: row.dns_ms,
                    tcp_ms: row.tcp_ms,
                    tls_ms: row.tls_ms,
                    ttfb_ms: row.ttfb_ms,
                })
                .collect::<Vec<_>>();
            return Ok(tui::render_network(
                &rows,
                &flows,
                &NetworkOptions {
                    period: &format!("DAY · {today}"),
                    username,
                    project,
                    color,
                    width,
                },
            ));
        }
        _ => unreachable!(),
    };
    let (from, to, mut label) = period_bounds(period, None)?;
    if let Some(project) = project {
        label.push_str(&format!(" · PROJECT {project}"));
    }
    let filter = UsageFilter {
        from_date: from.as_deref(),
        to_date: to.as_deref(),
        project,
        account: None,
    };
    let overview = storage.overview_range(filter)?;
    let models = storage
        .model_breakdown_range(filter)?
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>();
    let mut options = OverviewOptions::new(&label, width, color);
    options.source_label = source;
    options.weekly_quotas = quotas;
    options.quota_message = quota_message;
    options.source_message = remote_message;
    Ok(tui::render_overview(&overview.into(), &models, &options))
}

struct RemoteWorkerUpdate {
    text: String,
    finished: bool,
}

fn spawn_remote_worker(
    path: PathBuf,
    identity: LocalIdentity,
    catalog: PricingCatalog,
    hosts: Vec<String>,
) -> std::sync::mpsc::Receiver<RemoteWorkerUpdate> {
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let result = (|| -> Result<(usize, Vec<String>)> {
            let mut storage = open_storage(&path, &identity, &catalog)?;
            let mut imported = 0;
            let mut failures = Vec::new();
            for host in &hosts {
                let progress_sender = sender.clone();
                match crate::remote::sync_with_progress(
                    &mut storage,
                    &catalog,
                    host,
                    false,
                    |progress| {
                        if progress.total_files == 0 {
                            return;
                        }
                        let percent = if progress.total_source_bytes == 0 {
                            100
                        } else {
                            progress
                                .completed_source_bytes
                                .saturating_mul(100)
                                .checked_div(progress.total_source_bytes)
                                .unwrap_or(0)
                                .min(100)
                        };
                        let mode = if progress.server_filtered {
                            "server metadata"
                        } else {
                            "legacy transfer"
                        };
                        let _ = progress_sender.send(RemoteWorkerUpdate {
                            text: format!(
                                "{} · {mode} {}/{} · {percent}%",
                                progress.host, progress.completed_files, progress.total_files
                            ),
                            finished: false,
                        });
                    },
                ) {
                    Ok(row) => {
                        imported += row.imported_files;
                        if row.failed_files > 0 {
                            failures.push(format!(
                                "{host}: {} remote Rollout file(s) failed",
                                row.failed_files
                            ));
                        }
                    }
                    Err(error) => failures.push(format!("{host}: {error:#}")),
                }
            }
            Ok((imported, failures))
        })();
        let message = match result {
            Ok((imported, failures)) if failures.is_empty() => {
                format!("Remote sync complete · {imported} changed file(s)")
            }
            Ok((imported, failures)) => {
                format!(
                    "Remote sync partial · {imported} imported · {} · press r to retry",
                    failures.join(" · ")
                )
            }
            Err(error) => format!("Remote sync failed: {error:#} · press r to retry"),
        };
        let _ = sender.send(RemoteWorkerUpdate {
            text: message,
            finished: true,
        });
    });
    receiver
}

fn demo(color: bool) -> String {
    let overview = tui::Overview {
        calls: 31,
        sessions: 3,
        turns: 12,
        input_tokens: 163_200,
        cached_input_tokens: 127_600,
        cache_write_tokens: 8_200,
        output_tokens: 21_100,
        reasoning_tokens: 19_400,
        total_tokens: 184_300,
        cost_usd: Some(0.84),
        avg_ttft_ms: Some(420.0),
        avg_e2e_ms: Some(11_700.0),
        ..Default::default()
    };
    let models = vec![ModelRow {
        model: "gpt-5.6-sol".into(),
        effort: "high".into(),
        calls: 31,
        input_tokens: 163_200,
        cached_input_tokens: 127_600,
        output_tokens: 21_100,
        reasoning_tokens: 19_400,
        total_tokens: 184_300,
        cost_usd: Some(0.84),
    }];
    tui::render_overview(
        &overview,
        &models,
        &OverviewOptions::new("DEMO · LOCAL ONLY", 110, color),
    )
}

fn terminal_width() -> usize {
    crossterm::terminal::size().map_or(110, |(width, _)| usize::from(width))
}

fn milliseconds(value: Option<f64>) -> String {
    value.map_or_else(
        || "N/A".into(),
        |value| {
            if value < 1000.0 {
                format!("{value:.0}ms")
            } else {
                format!("{:.2}s", value / 1000.0)
            }
        },
    )
}

fn optional_ratio(value: Option<f64>) -> String {
    value.map_or_else(|| "N/A".into(), |value| format!("{value:.2}x"))
}

fn percent(numerator: i64, denominator: i64) -> f64 {
    if denominator > 0 {
        numerator as f64 / denominator as f64 * 100.0
    } else {
        0.0
    }
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn bounded_i64(value: Option<u64>) -> Option<i64> {
    value.map(|value| value.min(i64::MAX as u64) as i64)
}

fn time_delta_ms(start: Option<&str>, end: Option<&str>) -> Option<i64> {
    let start = chrono::DateTime::parse_from_rfc3339(start?).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(end?).ok()?;
    Some((end - start).num_milliseconds().max(0))
}

fn csv_opt(value: Option<&str>) -> String {
    csv(value.unwrap_or(""))
}

fn csv(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn exposes_every_python_top_level_command() {
        let command = Args::command();
        let names: Vec<_> = command
            .get_subcommands()
            .map(|item| item.get_name())
            .collect();
        for expected in [
            "import",
            "today",
            "summary",
            "history",
            "account",
            "remote",
            "models",
            "sessions",
            "perf",
            "cache",
            "projects",
            "providers",
            "agents",
            "tools",
            "waterfall",
            "watch",
            "statusline",
            "otel",
            "app-server",
            "network",
            "proxy",
            "export",
            "doctor",
            "pricing",
            "demo",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn invalid_remote_alias_and_empty_account_label_are_usage_errors() {
        for error in [
            validated_remote_host("bad@host").unwrap_err(),
            validated_account_label("   ").unwrap_err(),
        ] {
            assert_eq!(error_exit_code(&error), 2);
        }
        assert_eq!(validated_account_label("  work  ").unwrap(), "work");
    }
}
