//! SQLite schema compatibility, incremental imports, and aggregate queries.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use chrono::DateTime;
use rusqlite::{
    Connection, OptionalExtension, Transaction, params, params_from_iter, types::Value,
};

use crate::{
    models::{
        Confidence, LlmCallRecord, MetricPointRecord, NetworkFlowRecord, ParsedSession, Quality,
        TelemetryLogRecord, TokenUsage, ToolCallRecord,
    },
    pricing::PricingCatalog,
};

const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001",
        include_str!("../codex_meter/migrations/001_initial.sql"),
    ),
    (
        "002",
        include_str!("../codex_meter/migrations/002_live_observability.sql"),
    ),
    (
        "003",
        include_str!("../codex_meter/migrations/003_local_identity.sql"),
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMetadata {
    pub source_path: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
}

impl SourceMetadata {
    pub fn for_file(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        let modified = metadata
            .modified()
            .unwrap_or(UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(i64::MAX as u128) as i64;
        Ok(Self {
            source_path: path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .to_string_lossy()
                .into_owned(),
            size_bytes: metadata.len(),
            mtime_ns: modified,
        })
    }
}

/// `(inserted_turns, inserted_calls, inserted_tools)`, matching the Python API.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportStats(pub usize, pub usize, pub usize);

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Overview {
    pub calls: i64,
    pub sessions: i64,
    pub turns: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
    pub unpriced_calls: i64,
    pub avg_ttft_ms: Option<f64>,
    pub avg_e2e_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsage {
    pub model: String,
    pub effort: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
    pub unpriced_calls: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub codex_thread_id: String,
    pub project_name: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub turns: i64,
    pub calls: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
    pub cached_input_tokens: i64,
    pub input_tokens: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryBucket {
    pub period_start: String,
    pub calls: i64,
    pub sessions: i64,
    pub turns: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectUsage {
    pub project: String,
    pub sessions: i64,
    pub turns: i64,
    pub calls: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
    pub retry_tokens: i64,
    pub compactions: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UsageFilter<'a> {
    pub from_date: Option<&'a str>,
    pub to_date: Option<&'a str>,
    pub account: Option<&'a str>,
    pub project: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct LiveTurnUpdate<'a> {
    pub started_at: Option<&'a str>,
    pub completed_at: Option<&'a str>,
    pub status: &'a str,
    pub model: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
    pub service_tier: Option<&'a str>,
    pub ttft_ms: Option<i64>,
    pub ttfm_ms: Option<i64>,
    pub e2e_ms: Option<i64>,
    pub source: &'a str,
}

impl Default for LiveTurnUpdate<'_> {
    fn default() -> Self {
        Self {
            started_at: None,
            completed_at: None,
            status: "running",
            model: None,
            reasoning_effort: None,
            service_tier: None,
            ttft_ms: None,
            ttfm_ms: None,
            e2e_ms: None,
            source: "app_server",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LiveCallTimings<'a> {
    pub started_at: Option<&'a str>,
    pub first_event_at: Option<&'a str>,
    pub first_model_item_at: Option<&'a str>,
    pub ttft_ms: Option<i64>,
    pub ttfm_ms: Option<i64>,
    pub request_duration_ms: Option<i64>,
    pub transport: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsage {
    pub account: String,
    pub sessions: i64,
    pub calls: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
    pub first_used_at: Option<String>,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsage {
    pub provider: String,
    pub calls: i64,
    pub sessions: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentUsage {
    pub role: String,
    pub sessions: i64,
    pub turns: i64,
    pub calls: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExportCall {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub response_id: Option<String>,
    pub completed_at: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    #[serde(flatten)]
    pub usage: TokenUsage,
    pub cost_usd: Option<f64>,
    pub data_source: String,
    pub confidence: String,
    /// SQLite/Python compatible integer flag.  Keeping this as `0`/`1` also
    /// makes both JSON and CSV exports match the established Python format.
    pub estimated: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolUsage {
    pub tool_name: String,
    pub calls: i64,
    pub successes: i64,
    pub known_outcomes: i64,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<i64>,
    pub total_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResponsePerformance {
    pub completed_at: Option<String>,
    pub local_time: Option<String>,
    pub model: String,
    pub output_tokens: i64,
    pub ttft_ms: Option<i64>,
    pub e2e_ms: Option<i64>,
    pub exact_output_tps: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageCall {
    pub call: LlmCallRecord,
    pub project_name: Option<String>,
    pub codex_thread_id: String,
    pub codex_turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnWaterfall {
    pub codex_turn_id: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub status: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub ttft_ms: Option<i64>,
    pub ttfm_ms: Option<i64>,
    pub e2e_ms: Option<i64>,
    pub codex_thread_id: String,
    pub project_name: Option<String>,
    pub calls: Vec<LlmCallRecord>,
    pub tools: Vec<ToolCallRecord>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct TelemetryRetrySummary {
    pub attempts: i64,
    pub duration_ms: f64,
    pub failures: i64,
}

pub struct Storage {
    pub path: PathBuf,
    pub owner_uid: Option<i64>,
    pub owner_username: String,
    pub account_label: Option<String>,
    connection: Connection,
}

impl Storage {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::with_identity(path, current_uid(), current_username(), None)
    }

    pub fn with_identity(
        path: impl Into<PathBuf>,
        owner_uid: Option<i64>,
        owner_username: impl Into<String>,
        account_label: Option<String>,
    ) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }
        let connection =
            Connection::open(&path).with_context(|| format!("open database {}", path.display()))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;",
        )?;
        Ok(Self {
            path,
            owner_uid,
            owner_username: owner_username.into(),
            account_label: account_label.and_then(|value| {
                let value = value.trim().to_string();
                (!value.is_empty()).then_some(value)
            }),
            connection,
        })
    }

    pub fn close(self) -> Result<()> {
        self.connection
            .close()
            .map_err(|(_, error)| anyhow::Error::from(error))
    }

    pub fn migrate(&self) -> Result<()> {
        for (version, sql) in MIGRATIONS {
            let applied = if self.table_exists("schema_migrations")? {
                self.connection
                    .query_row(
                        "SELECT 1 FROM schema_migrations WHERE version=?1",
                        [version],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some()
            } else {
                false
            };
            if applied {
                continue;
            }
            self.connection
                .execute_batch(sql)
                .with_context(|| format!("apply database migration {version}"))?;
            self.connection.execute(
                "INSERT OR IGNORE INTO schema_migrations(version) VALUES (?1)",
                [version],
            )?;
        }
        if self.column_exists("sessions", "owner_username")? {
            self.connection.execute(
                "UPDATE sessions SET owner_uid=COALESCE(owner_uid, ?1), owner_username=COALESCE(owner_username, ?2)",
                params![self.owner_uid, self.owner_username],
            )?;
        }
        Ok(())
    }

    pub fn sync_pricing(&self, catalog: &PricingCatalog) -> Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        for price in &catalog.entries {
            transaction.execute(
                r#"INSERT OR IGNORE INTO pricing_snapshots(
                    model, provider, effective_from, input_per_million,
                    cached_input_per_million, cache_write_per_million,
                    output_per_million, long_context_threshold,
                    long_context_input_multiplier, long_context_output_multiplier,
                    pricing_version
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)"#,
                params![
                    price.model,
                    price.provider,
                    price.effective_from,
                    price.input_per_million,
                    price.cached_input_per_million,
                    price.cache_write_per_million,
                    price.output_per_million,
                    price.long_context_threshold,
                    price.long_context_input_multiplier,
                    price.long_context_output_multiplier,
                    price.version,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn file_is_current(&self, path: &Path) -> Result<bool> {
        let metadata = SourceMetadata::for_file(path)?;
        self.source_is_current(
            &metadata.source_path,
            metadata.size_bytes,
            metadata.mtime_ns,
        )
    }

    pub fn source_is_current(
        &self,
        source_path: &str,
        size_bytes: u64,
        mtime_ns: i64,
    ) -> Result<bool> {
        let row = self
            .connection
            .query_row(
                "SELECT size_bytes, mtime_ns FROM import_files WHERE source_path=?1",
                [source_path],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok(
            row.is_some_and(|(size, mtime)| {
                size == saturating_i64(size_bytes) && mtime == mtime_ns
            }),
        )
    }

    pub fn import_file(&self, parsed: &ParsedSession, path: &Path) -> Result<ImportStats> {
        self.import_session(parsed, SourceMetadata::for_file(path)?)
    }

    pub fn import_session(
        &self,
        parsed: &ParsedSession,
        metadata: SourceMetadata,
    ) -> Result<ImportStats> {
        let session = &parsed.session;
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            r#"INSERT INTO sessions(
                codex_thread_id, started_at, ended_at, cwd, project_name,
                git_repo, git_branch, auth_mode, codex_version, provider,
                source, source_path, parent_thread_id, agent_role, agent_id,
                owner_uid, owner_username, account_label
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'session_jsonl',?11,?12,?13,?14,?15,?16,?17)
            ON CONFLICT(codex_thread_id) DO UPDATE SET
                ended_at=excluded.ended_at,
                cwd=COALESCE(excluded.cwd, sessions.cwd),
                project_name=COALESCE(excluded.project_name, sessions.project_name),
                git_repo=COALESCE(excluded.git_repo, sessions.git_repo),
                git_branch=COALESCE(excluded.git_branch, sessions.git_branch),
                auth_mode=CASE WHEN excluded.auth_mode != 'unknown' THEN excluded.auth_mode ELSE sessions.auth_mode END,
                codex_version=COALESCE(excluded.codex_version, sessions.codex_version),
                provider=COALESCE(excluded.provider, sessions.provider),
                source_path=excluded.source_path,
                owner_uid=COALESCE(sessions.owner_uid, excluded.owner_uid),
                owner_username=COALESCE(sessions.owner_username, excluded.owner_username),
                account_label=sessions.account_label,
                updated_at=CURRENT_TIMESTAMP"#,
            params![
                session.codex_thread_id,
                session.started_at,
                session.ended_at,
                session.cwd,
                session.project_name,
                session.git_repo,
                session.git_branch,
                session.auth_mode,
                session.codex_version,
                session.provider,
                session.source_path,
                session.parent_thread_id,
                session.agent_role,
                session.agent_id,
                self.owner_uid,
                self.owner_username,
                self.account_label,
            ],
        )?;
        let session_id: i64 = transaction.query_row(
            "SELECT id FROM sessions WHERE codex_thread_id=?1",
            [&session.codex_thread_id],
            |row| row.get(0),
        )?;

        let mut stats = ImportStats::default();
        let mut turn_ids = HashMap::new();
        for turn in parsed.turns.values() {
            let existed = transaction
                .query_row(
                    "SELECT id FROM turns WHERE codex_turn_id=?1",
                    [&turn.codex_turn_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            transaction.execute(
                r#"INSERT INTO turns(
                    session_id, codex_turn_id, started_at, completed_at, status,
                    model, reasoning_effort, reasoning_mode, service_tier,
                    ttft_ms, ttfm_ms, e2e_ms, error_type,
                    data_source, confidence, estimated
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
                ON CONFLICT(codex_turn_id) DO UPDATE SET
                    completed_at=COALESCE(excluded.completed_at, turns.completed_at),
                    status=excluded.status,
                    model=COALESCE(excluded.model, turns.model),
                    reasoning_effort=COALESCE(excluded.reasoning_effort, turns.reasoning_effort),
                    reasoning_mode=COALESCE(excluded.reasoning_mode, turns.reasoning_mode),
                    service_tier=COALESCE(excluded.service_tier, turns.service_tier),
                    ttft_ms=COALESCE(excluded.ttft_ms, turns.ttft_ms),
                    ttfm_ms=COALESCE(excluded.ttfm_ms, turns.ttfm_ms),
                    e2e_ms=COALESCE(excluded.e2e_ms, turns.e2e_ms),
                    error_type=COALESCE(excluded.error_type, turns.error_type)"#,
                params![
                    session_id,
                    turn.codex_turn_id,
                    turn.started_at,
                    turn.completed_at,
                    turn.status,
                    turn.model,
                    turn.reasoning_effort,
                    turn.reasoning_mode,
                    turn.service_tier,
                    turn.ttft_ms,
                    turn.ttfm_ms,
                    turn.e2e_ms,
                    turn.error_type,
                    turn.quality.source,
                    turn.quality.confidence.as_str(),
                    i64::from(turn.quality.estimated),
                ],
            )?;
            stats.0 += usize::from(existed.is_none());
            let id = transaction.query_row(
                "SELECT id FROM turns WHERE codex_turn_id=?1",
                [&turn.codex_turn_id],
                |row| row.get::<_, i64>(0),
            )?;
            turn_ids.insert(turn.codex_turn_id.clone(), id);
        }

        for call in &parsed.llm_calls {
            let resolved_turn_id = call
                .turn_id
                .as_ref()
                .and_then(|turn_id| turn_ids.get(turn_id))
                .copied();
            // Forks copy the parent's rollout prefix. If the logical turn already
            // belongs to another session, retain that authoritative owner.
            let call_session_id = match resolved_turn_id {
                Some(turn_id) => transaction
                    .query_row(
                        "SELECT session_id FROM turns WHERE id=?1",
                        [turn_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .unwrap_or(session_id),
                None => session_id,
            };
            let inserted = transaction.execute(
                r#"INSERT OR IGNORE INTO llm_calls(
                    event_fingerprint, session_id, turn_id, response_id, completed_at,
                    model, actual_model, provider, reasoning_effort, reasoning_mode,
                    service_tier, input_tokens, cached_input_tokens, cache_write_tokens,
                    output_tokens, reasoning_tokens, total_tokens, retry_index, success,
                    error_type, cost_usd, pricing_version, data_source, confidence, estimated
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)"#,
                params![
                    call.event_fingerprint,
                    call_session_id,
                    resolved_turn_id,
                    call.response_id,
                    call.completed_at,
                    call.model,
                    call.actual_model,
                    call.provider,
                    call.reasoning_effort,
                    call.reasoning_mode,
                    call.service_tier,
                    call.usage.input_tokens,
                    call.usage.cached_input_tokens,
                    call.usage.cache_write_tokens,
                    call.usage.output_tokens,
                    call.usage.reasoning_tokens,
                    call.usage.total_tokens,
                    call.retry_index,
                    i64::from(call.success),
                    call.error_type,
                    call.cost_usd,
                    call.pricing_version,
                    call.quality.source,
                    call.quality.confidence.as_str(),
                    i64::from(call.quality.estimated),
                ],
            )?;
            stats.1 += inserted;
        }

        for tool in &parsed.tool_calls {
            let inserted = transaction.execute(
                r#"INSERT OR IGNORE INTO tool_calls(
                    source_call_id, session_id, turn_id, tool_name, started_at,
                    completed_at, duration_ms, success, exit_code,
                    data_source, confidence, estimated
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)"#,
                params![
                    tool.call_id,
                    session_id,
                    tool.turn_id
                        .as_ref()
                        .and_then(|turn_id| turn_ids.get(turn_id))
                        .copied(),
                    tool.tool_name,
                    tool.started_at,
                    tool.completed_at,
                    tool.duration_ms,
                    tool.success.map(i64::from),
                    tool.exit_code,
                    tool.quality.source,
                    tool.quality.confidence.as_str(),
                    i64::from(tool.quality.estimated),
                ],
            )?;
            stats.2 += inserted;
        }

        transaction.execute(
            r#"INSERT INTO import_files(
                source_path, size_bytes, mtime_ns, session_id,
                malformed_lines, duplicate_usage_events
            ) VALUES (?1,?2,?3,?4,?5,?6)
            ON CONFLICT(source_path) DO UPDATE SET
                size_bytes=excluded.size_bytes,
                mtime_ns=excluded.mtime_ns,
                session_id=excluded.session_id,
                imported_at=CURRENT_TIMESTAMP,
                malformed_lines=excluded.malformed_lines,
                duplicate_usage_events=excluded.duplicate_usage_events"#,
            params![
                metadata.source_path,
                saturating_i64(metadata.size_bytes),
                metadata.mtime_ns,
                session_id,
                parsed.malformed_lines,
                parsed.duplicate_usage_events,
            ],
        )?;
        refresh_turn_aggregates(&transaction, turn_ids.values().copied())?;
        transaction.commit()?;
        Ok(stats)
    }

    pub fn ensure_live_session(
        &self,
        thread_id: &str,
        started_at: Option<&str>,
        cwd: Option<&str>,
        _model: Option<&str>,
        source: &str,
    ) -> Result<i64> {
        let project_name = cwd.and_then(|cwd| {
            Path::new(cwd)
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
        });
        self.connection.execute(
            r#"INSERT INTO sessions(
                codex_thread_id,started_at,cwd,project_name,auth_mode,
                provider,source,source_path,owner_uid,owner_username,account_label
            ) VALUES (?1,?2,?3,?4,'unknown','openai',?5,?6,?7,?8,?9)
            ON CONFLICT(codex_thread_id) DO UPDATE SET
                started_at=COALESCE(sessions.started_at,excluded.started_at),
                cwd=COALESCE(excluded.cwd,sessions.cwd),
                project_name=COALESCE(excluded.project_name,sessions.project_name),
                owner_uid=COALESCE(sessions.owner_uid,excluded.owner_uid),
                owner_username=COALESCE(sessions.owner_username,excluded.owner_username),
                account_label=sessions.account_label,updated_at=CURRENT_TIMESTAMP"#,
            params![
                thread_id,
                started_at,
                cwd,
                project_name,
                source,
                format!("{source}://{thread_id}"),
                self.owner_uid,
                self.owner_username,
                self.account_label,
            ],
        )?;
        Ok(self.connection.query_row(
            "SELECT id FROM sessions WHERE codex_thread_id=?1",
            [thread_id],
            |row| row.get(0),
        )?)
    }

    pub fn upsert_live_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
        update: LiveTurnUpdate<'_>,
    ) -> Result<i64> {
        let session_id = self.ensure_live_session(
            thread_id,
            update.started_at,
            None,
            update.model,
            update.source,
        )?;
        self.connection.execute(
            r#"INSERT INTO turns(
                session_id,codex_turn_id,started_at,completed_at,status,model,
                reasoning_effort,service_tier,ttft_ms,ttfm_ms,e2e_ms,
                data_source,confidence,estimated
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'exact',0)
            ON CONFLICT(codex_turn_id) DO UPDATE SET
                started_at=COALESCE(turns.started_at,excluded.started_at),
                completed_at=COALESCE(excluded.completed_at,turns.completed_at),
                status=excluded.status,model=COALESCE(excluded.model,turns.model),
                reasoning_effort=COALESCE(excluded.reasoning_effort,turns.reasoning_effort),
                service_tier=COALESCE(excluded.service_tier,turns.service_tier),
                ttft_ms=COALESCE(excluded.ttft_ms,turns.ttft_ms),
                ttfm_ms=COALESCE(excluded.ttfm_ms,turns.ttfm_ms),
                e2e_ms=COALESCE(excluded.e2e_ms,turns.e2e_ms)"#,
            params![
                session_id,
                turn_id,
                update.started_at,
                update.completed_at,
                update.status,
                update.model,
                update.reasoning_effort,
                update.service_tier,
                update.ttft_ms,
                update.ttfm_ms,
                update.e2e_ms,
                update.source,
            ],
        )?;
        Ok(self.connection.query_row(
            "SELECT id FROM turns WHERE codex_turn_id=?1",
            [turn_id],
            |row| row.get(0),
        )?)
    }

    pub fn insert_live_call(
        &self,
        thread_id: &str,
        call: &LlmCallRecord,
        timing: LiveCallTimings<'_>,
    ) -> Result<bool> {
        let session_id = self.ensure_live_session(
            thread_id,
            timing.started_at,
            None,
            call.model.as_deref(),
            "app_server",
        )?;
        let turn_db_id = match call.turn_id.as_deref() {
            Some(turn_id) => Some(self.upsert_live_turn(
                thread_id,
                turn_id,
                LiveTurnUpdate {
                    started_at: timing.started_at,
                    ..Default::default()
                },
            )?),
            None => None,
        };
        let generation_ms = match (timing.first_model_item_at, call.completed_at.as_deref()) {
            (Some(start), Some(end)) => match (
                DateTime::parse_from_rfc3339(start),
                DateTime::parse_from_rfc3339(end),
            ) {
                (Ok(start), Ok(end)) => Some((end - start).num_milliseconds().max(0)),
                _ => None,
            },
            _ => None,
        };
        let output_tps = timing
            .request_duration_ms
            .filter(|value| *value > 0)
            .map(|milliseconds| call.usage.output_tokens as f64 / (milliseconds as f64 / 1_000.0));
        let inserted = self.connection.execute(
            r#"INSERT OR IGNORE INTO llm_calls(
                event_fingerprint,session_id,turn_id,response_id,started_at,
                first_event_at,first_model_item_at,completed_at,model,actual_model,
                provider,reasoning_effort,reasoning_mode,transport,service_tier,
                input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,
                reasoning_tokens,total_tokens,request_duration_ms,ttft_ms,ttfm_ms,
                generation_ms,output_tps,retry_index,success,error_type,cost_usd,
                pricing_version,data_source,confidence,estimated
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34)"#,
            params![
                call.event_fingerprint,
                session_id,
                turn_db_id,
                call.response_id,
                timing.started_at,
                timing.first_event_at,
                timing.first_model_item_at,
                call.completed_at,
                call.model,
                call.actual_model,
                call.provider,
                call.reasoning_effort,
                call.reasoning_mode,
                timing.transport,
                call.service_tier,
                call.usage.input_tokens,
                call.usage.cached_input_tokens,
                call.usage.cache_write_tokens,
                call.usage.output_tokens,
                call.usage.reasoning_tokens,
                call.usage.total_tokens,
                timing.request_duration_ms,
                timing.ttft_ms,
                timing.ttfm_ms,
                generation_ms,
                output_tps,
                call.retry_index,
                i64::from(call.success),
                call.error_type,
                call.cost_usd,
                call.pricing_version,
                call.quality.source,
                call.quality.confidence.as_str(),
                i64::from(call.quality.estimated),
            ],
        )?;
        if let Some(turn_id) = turn_db_id {
            let transaction = self.connection.unchecked_transaction()?;
            refresh_turn_aggregates(&transaction, [turn_id])?;
            transaction.commit()?;
        }
        Ok(inserted > 0)
    }

    pub fn update_live_turn_usage(
        &self,
        thread_id: &str,
        turn_id: &str,
        usage: TokenUsage,
    ) -> Result<()> {
        let turn_db_id = self.upsert_live_turn(thread_id, turn_id, LiveTurnUpdate::default())?;
        self.connection.execute(
            r#"UPDATE turns SET input_tokens=MAX(input_tokens,?1),
                cached_input_tokens=MAX(cached_input_tokens,?2),
                cache_write_tokens=MAX(cache_write_tokens,?3),
                output_tokens=MAX(output_tokens,?4),reasoning_tokens=MAX(reasoning_tokens,?5),
                total_tokens=MAX(total_tokens,?6) WHERE id=?7"#,
            params![
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.cache_write_tokens,
                usage.output_tokens,
                usage.reasoning_tokens,
                usage.total_tokens,
                turn_db_id,
            ],
        )?;
        Ok(())
    }

    pub fn update_actual_model(&self, turn_id: &str, actual_model: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE llm_calls SET actual_model=?1 WHERE turn_id=(SELECT id FROM turns WHERE codex_turn_id=?2)",
            params![actual_model, turn_id],
        )?;
        Ok(())
    }

    pub fn upsert_live_tool(
        &self,
        thread_id: &str,
        tool: &ToolCallRecord,
        source: &str,
    ) -> Result<()> {
        let session_id =
            self.ensure_live_session(thread_id, tool.started_at.as_deref(), None, None, source)?;
        let turn_db_id = match tool.turn_id.as_deref() {
            Some(turn_id) => Some(self.upsert_live_turn(
                thread_id,
                turn_id,
                LiveTurnUpdate {
                    started_at: tool.started_at.as_deref(),
                    source,
                    ..Default::default()
                },
            )?),
            None => None,
        };
        self.connection.execute(
            r#"INSERT INTO tool_calls(
                source_call_id,session_id,turn_id,tool_name,started_at,completed_at,
                duration_ms,success,exit_code,data_source,confidence,estimated
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'exact',0)
            ON CONFLICT(source_call_id) DO UPDATE SET
                completed_at=COALESCE(excluded.completed_at,tool_calls.completed_at),
                duration_ms=COALESCE(excluded.duration_ms,tool_calls.duration_ms),
                success=COALESCE(excluded.success,tool_calls.success),
                exit_code=COALESCE(excluded.exit_code,tool_calls.exit_code)"#,
            params![
                tool.call_id,
                session_id,
                turn_db_id,
                tool.tool_name,
                tool.started_at,
                tool.completed_at,
                tool.duration_ms,
                tool.success.map(i64::from),
                tool.exit_code,
                source,
            ],
        )?;
        if let Some(turn_id) = turn_db_id {
            let transaction = self.connection.unchecked_transaction()?;
            refresh_turn_aggregates(&transaction, [turn_id])?;
            transaction.commit()?;
        }
        Ok(())
    }

    pub fn insert_metric_points(&self, points: &[MetricPointRecord]) -> Result<usize> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut inserted = 0;
        for point in points {
            inserted += transaction.execute(
                r#"INSERT OR IGNORE INTO metric_points(
                    event_fingerprint,observed_at,name,kind,value,point_sum,point_count,
                    point_min,point_max,explicit_bounds_json,bucket_counts_json,attributes_json,
                    thread_id,turn_id,response_id,tool_name,start_time_unix_nano,time_unix_nano,
                    data_source,confidence,estimated
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)"#,
                params![
                    point.event_fingerprint, point.observed_at, point.name, point.kind,
                    point.value, point.point_sum, point.point_count, point.point_min, point.point_max,
                    serde_json::to_string(&point.explicit_bounds)?,
                    serde_json::to_string(&point.bucket_counts)?,
                    serde_json::to_string(&point.attributes)?,
                    point.thread_id, point.turn_id, point.response_id, point.tool_name,
                    point.start_time_unix_nano, point.time_unix_nano,
                    point.quality.source, point.quality.confidence.as_str(),
                    i64::from(point.quality.estimated),
                ],
            )?;
            apply_metric_point(&transaction, point)?;
        }
        transaction.commit()?;
        Ok(inserted)
    }

    pub fn insert_telemetry_logs(&self, records: &[TelemetryLogRecord]) -> Result<usize> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut inserted = 0;
        for record in records {
            inserted += transaction.execute(
                r#"INSERT OR IGNORE INTO telemetry_logs(
                    event_fingerprint,observed_at,event_name,severity,attributes_json,
                    thread_id,turn_id,response_id,item_id,tool_name,duration_ms,status,
                    success,data_source
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)"#,
                params![
                    record.event_fingerprint,
                    record.observed_at,
                    record.event_name,
                    record.severity,
                    serde_json::to_string(&record.attributes)?,
                    record.thread_id,
                    record.turn_id,
                    record.response_id,
                    record.item_id,
                    record.tool_name,
                    record.duration_ms,
                    record.status,
                    record.success.map(i64::from),
                    record.quality.source,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(inserted)
    }

    pub fn insert_compaction(
        &self,
        fingerprint: &str,
        thread_id: &str,
        turn_id: Option<&str>,
        occurred_at: Option<&str>,
        source: &str,
    ) -> Result<bool> {
        Ok(self.connection.execute(
            "INSERT OR IGNORE INTO compactions(event_fingerprint,thread_id,turn_id,occurred_at,data_source) VALUES (?1,?2,?3,?4,?5)",
            params![fingerprint, thread_id, turn_id, occurred_at, source],
        )? > 0)
    }

    pub fn insert_network_flow(&self, flow: &NetworkFlowRecord) -> Result<bool> {
        Ok(self.connection.execute(
            r#"INSERT OR IGNORE INTO network_flows(
                event_fingerprint,started_at,ended_at,mode,destination_host,destination_ip,
                destination_port,protocol,tls_version,alpn,http_status,request_bytes,response_bytes,
                packets_out,packets_in,dns_ms,tcp_ms,tls_ms,ttfb_ms,first_event_ms,first_output_ms,
                duration_ms,success,error_type,thread_id,turn_id,response_id,data_source,confidence
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29)"#,
            params![
                flow.event_fingerprint, flow.started_at, flow.ended_at, flow.mode,
                flow.destination_host, flow.destination_ip, flow.destination_port, flow.protocol,
                flow.tls_version, flow.alpn, flow.http_status, flow.request_bytes,
                flow.response_bytes, flow.packets_out, flow.packets_in, flow.dns_ms,
                flow.tcp_ms, flow.tls_ms, flow.ttfb_ms, flow.first_event_ms,
                flow.first_output_ms, flow.duration_ms, flow.success.map(i64::from),
                flow.error_type, flow.thread_id, flow.turn_id, flow.response_id,
                flow.data_source, flow.quality.confidence.as_str(),
            ],
        )? > 0)
    }

    pub fn overview(
        &self,
        day: Option<&str>,
        account: Option<&str>,
        project: Option<&str>,
    ) -> Result<Overview> {
        self.overview_range(UsageFilter {
            from_date: day,
            to_date: day,
            account,
            project,
        })
    }

    pub fn overview_range(&self, filter: UsageFilter<'_>) -> Result<Overview> {
        let (where_sql, values) = self.owned_filter("c.completed_at", filter);
        let sql = format!(
            r#"WITH filtered_calls AS (
                SELECT c.* FROM llm_calls c JOIN sessions s ON s.id=c.session_id {where_sql}
            ), selected_turns AS (
                SELECT DISTINCT turn_id FROM filtered_calls WHERE turn_id IS NOT NULL
            ), turn_metrics AS (
                SELECT AVG(t.ttft_ms) AS avg_ttft_ms, AVG(t.e2e_ms) AS avg_e2e_ms
                FROM turns t JOIN selected_turns s ON s.turn_id=t.id
            )
            SELECT COUNT(*), COUNT(DISTINCT c.session_id), COUNT(DISTINCT c.turn_id),
                   COALESCE(SUM(c.input_tokens),0), COALESCE(SUM(c.cached_input_tokens),0),
                   COALESCE(SUM(c.cache_write_tokens),0), COALESCE(SUM(c.output_tokens),0),
                   COALESCE(SUM(c.reasoning_tokens),0), COALESCE(SUM(c.total_tokens),0),
                   SUM(c.cost_usd), COALESCE(SUM(CASE WHEN c.cost_usd IS NULL THEN 1 ELSE 0 END),0),
                   tm.avg_ttft_ms, tm.avg_e2e_ms
            FROM filtered_calls c CROSS JOIN turn_metrics tm"#
        );
        self.connection
            .query_row(&sql, params_from_iter(values.iter()), |row| {
                Ok(Overview {
                    calls: row.get(0)?,
                    sessions: row.get(1)?,
                    turns: row.get(2)?,
                    input_tokens: row.get(3)?,
                    cached_input_tokens: row.get(4)?,
                    cache_write_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    reasoning_tokens: row.get(7)?,
                    total_tokens: row.get(8)?,
                    cost_usd: row.get(9)?,
                    unpriced_calls: row.get(10)?,
                    avg_ttft_ms: row.get(11)?,
                    avg_e2e_ms: row.get(12)?,
                })
            })
            .map_err(Into::into)
    }

    pub fn model_breakdown(&self, filter: UsageFilter<'_>) -> Result<Vec<ModelUsage>> {
        let (where_sql, values) = self.owned_filter("c.completed_at", filter);
        let sql = format!(
            r#"SELECT COALESCE(c.model,'Unknown'), COALESCE(c.reasoning_effort,'Unknown'),
                      COUNT(*), COALESCE(SUM(c.input_tokens),0),
                      COALESCE(SUM(c.cached_input_tokens),0), COALESCE(SUM(c.output_tokens),0),
                      COALESCE(SUM(c.reasoning_tokens),0), COALESCE(SUM(c.total_tokens),0),
                      SUM(c.cost_usd), SUM(CASE WHEN c.cost_usd IS NULL THEN 1 ELSE 0 END)
               FROM llm_calls c JOIN sessions s ON s.id=c.session_id
               {where_sql}
               GROUP BY c.model,c.reasoning_effort ORDER BY SUM(c.total_tokens) DESC"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(ModelUsage {
                model: row.get(0)?,
                effort: row.get(1)?,
                calls: row.get(2)?,
                input_tokens: row.get(3)?,
                cached_input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                reasoning_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                cost_usd: row.get(8)?,
                unpriced_calls: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn model_breakdown_range(&self, filter: UsageFilter<'_>) -> Result<Vec<ModelUsage>> {
        self.model_breakdown(filter)
    }

    pub fn recent_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        let (owner, owner_value) = self.owner_predicate("s");
        let sql = format!(
            r#"WITH turn_counts AS (
                    SELECT session_id,COUNT(*) turns FROM turns GROUP BY session_id
                ), call_totals AS (
                    SELECT session_id,COUNT(*) calls,SUM(total_tokens) total_tokens,
                           SUM(cost_usd) cost_usd,SUM(cached_input_tokens) cached_input_tokens,
                           SUM(input_tokens) input_tokens FROM llm_calls GROUP BY session_id
                )
                SELECT s.codex_thread_id,s.project_name,s.started_at,s.ended_at,
                       COALESCE(t.turns,0),COALESCE(c.calls,0),COALESCE(c.total_tokens,0),
                       c.cost_usd,COALESCE(c.cached_input_tokens,0),COALESCE(c.input_tokens,0)
                FROM sessions s LEFT JOIN turn_counts t ON t.session_id=s.id
                LEFT JOIN call_totals c ON c.session_id=s.id
                WHERE {owner} ORDER BY s.started_at DESC LIMIT ?2"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params![owner_value, limit.max(1) as i64], |row| {
            Ok(SessionSummary {
                codex_thread_id: row.get(0)?,
                project_name: row.get(1)?,
                started_at: row.get(2)?,
                ended_at: row.get(3)?,
                turns: row.get(4)?,
                calls: row.get(5)?,
                total_tokens: row.get(6)?,
                cost_usd: row.get(7)?,
                cached_input_tokens: row.get(8)?,
                input_tokens: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Alias matching the Python storage API.
    pub fn sessions(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        self.recent_sessions(limit)
    }

    pub fn account_breakdown(&self) -> Result<Vec<AccountUsage>> {
        let (where_sql, values) = self.owned_filter("c.completed_at", UsageFilter::default());
        let sql = format!(
            r#"SELECT COALESCE(s.account_label,'Unassigned'),COUNT(DISTINCT s.id),COUNT(*),
                      COALESCE(SUM(c.total_tokens),0),SUM(c.cost_usd),MIN(c.completed_at),MAX(c.completed_at)
               FROM llm_calls c JOIN sessions s ON s.id=c.session_id {where_sql}
               GROUP BY s.account_label ORDER BY SUM(c.total_tokens) DESC"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(AccountUsage {
                account: row.get(0)?,
                sessions: row.get(1)?,
                calls: row.get(2)?,
                total_tokens: row.get(3)?,
                cost_usd: row.get(4)?,
                first_used_at: row.get(5)?,
                last_used_at: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn usage_history(
        &self,
        group: &str,
        account: Option<&str>,
        project: Option<&str>,
    ) -> Result<Vec<HistoryBucket>> {
        let bucket = match group {
            "day" => "date(c.completed_at,'localtime')",
            "week" => "date(c.completed_at,'localtime','weekday 0','-6 days')",
            "month" => "strftime('%Y-%m-01',c.completed_at,'localtime')",
            _ => bail!("group must be day, week, or month"),
        };
        let (where_sql, values) = self.owned_filter(
            "c.completed_at",
            UsageFilter {
                account,
                project,
                ..Default::default()
            },
        );
        let sql = format!(
            r#"SELECT {bucket},COUNT(*),COUNT(DISTINCT c.session_id),COUNT(DISTINCT c.turn_id),
                      COALESCE(SUM(c.input_tokens),0),COALESCE(SUM(c.cached_input_tokens),0),
                      COALESCE(SUM(c.output_tokens),0),COALESCE(SUM(c.total_tokens),0),SUM(c.cost_usd)
               FROM llm_calls c JOIN sessions s ON s.id=c.session_id {where_sql}
               GROUP BY 1 ORDER BY 1"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(HistoryBucket {
                period_start: row.get(0)?,
                calls: row.get(1)?,
                sessions: row.get(2)?,
                turns: row.get(3)?,
                input_tokens: row.get(4)?,
                cached_input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                cost_usd: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn provider_breakdown(&self, day: Option<&str>) -> Result<Vec<ProviderUsage>> {
        let (where_sql, values) = self.owned_filter(
            "c.completed_at",
            UsageFilter {
                from_date: day,
                to_date: day,
                ..Default::default()
            },
        );
        let sql = format!(
            r#"SELECT COALESCE(c.provider,s.provider,'Unknown'),COUNT(*),COUNT(DISTINCT c.session_id),
                      COALESCE(SUM(c.input_tokens),0),COALESCE(SUM(c.cached_input_tokens),0),
                      COALESCE(SUM(c.output_tokens),0),COALESCE(SUM(c.total_tokens),0),SUM(c.cost_usd)
               FROM llm_calls c JOIN sessions s ON s.id=c.session_id {where_sql}
               GROUP BY COALESCE(c.provider,s.provider,'Unknown') ORDER BY SUM(c.total_tokens) DESC"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(ProviderUsage {
                provider: row.get(0)?,
                calls: row.get(1)?,
                sessions: row.get(2)?,
                input_tokens: row.get(3)?,
                cached_input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                total_tokens: row.get(6)?,
                cost_usd: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn agent_breakdown(&self, day: Option<&str>) -> Result<Vec<AgentUsage>> {
        let (where_sql, values) = self.owned_filter(
            "c.completed_at",
            UsageFilter {
                from_date: day,
                to_date: day,
                ..Default::default()
            },
        );
        let sql = format!(
            r#"SELECT COALESCE(s.agent_role,CASE WHEN s.parent_thread_id IS NULL THEN 'root' ELSE 'subagent' END),
                      COUNT(DISTINCT s.id),COUNT(DISTINCT c.turn_id),COUNT(*),
                      COALESCE(SUM(c.total_tokens),0),SUM(c.cost_usd)
               FROM llm_calls c JOIN sessions s ON s.id=c.session_id {where_sql}
               GROUP BY 1 ORDER BY SUM(c.total_tokens) DESC"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(AgentUsage {
                role: row.get(0)?,
                sessions: row.get(1)?,
                turns: row.get(2)?,
                calls: row.get(3)?,
                total_tokens: row.get(4)?,
                cost_usd: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn export_rows(
        &self,
        from_date: Option<&str>,
        to_date: Option<&str>,
        session: Option<&str>,
    ) -> Result<Vec<ExportCall>> {
        let mut clauses = Vec::new();
        let mut values = Vec::new();
        if let Some(uid) = self.owner_uid {
            clauses.push("s.owner_uid=?".to_string());
            values.push(Value::Integer(uid));
        } else {
            clauses.push("s.owner_username=?".to_string());
            values.push(Value::Text(self.owner_username.clone()));
        }
        if let Some(value) = from_date {
            clauses.push("date(c.completed_at,'localtime') >= date(?)".into());
            values.push(Value::Text(value.into()));
        }
        if let Some(value) = to_date {
            clauses.push("date(c.completed_at,'localtime') <= date(?)".into());
            values.push(Value::Text(value.into()));
        }
        if let Some(value) = session {
            clauses.push("s.codex_thread_id=?".into());
            values.push(Value::Text(value.into()));
        }
        let sql = format!(
            r#"SELECT s.codex_thread_id,t.codex_turn_id,c.response_id,c.completed_at,c.model,
                      c.reasoning_effort,c.input_tokens,c.cached_input_tokens,c.cache_write_tokens,
                      c.output_tokens,c.reasoning_tokens,c.total_tokens,c.cost_usd,c.data_source,
                      c.confidence,c.estimated
               FROM llm_calls c JOIN sessions s ON s.id=c.session_id
               LEFT JOIN turns t ON t.id=c.turn_id WHERE {} ORDER BY c.completed_at"#,
            clauses.join(" AND ")
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(ExportCall {
                session_id: row.get(0)?,
                turn_id: row.get(1)?,
                response_id: row.get(2)?,
                completed_at: row.get(3)?,
                model: row.get(4)?,
                reasoning_effort: row.get(5)?,
                usage: TokenUsage {
                    input_tokens: row.get(6)?,
                    cached_input_tokens: row.get(7)?,
                    cache_write_tokens: row.get(8)?,
                    output_tokens: row.get(9)?,
                    reasoning_tokens: row.get(10)?,
                    total_tokens: row.get(11)?,
                },
                cost_usd: row.get(12)?,
                data_source: row.get(13)?,
                confidence: row.get(14)?,
                estimated: row.get(15)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn metric_points(
        &self,
        day: Option<&str>,
        names: &[&str],
    ) -> Result<Vec<MetricPointRecord>> {
        let mut clauses = Vec::new();
        let mut values = Vec::new();
        if let Some(value) = day {
            clauses.push("date(observed_at,'localtime')=date(?)".to_string());
            values.push(Value::Text(value.into()));
        }
        if !names.is_empty() {
            clauses.push(format!("name IN ({})", vec!["?"; names.len()].join(",")));
            values.extend(names.iter().map(|value| Value::Text((*value).into())));
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let sql = format!(
            r#"SELECT event_fingerprint,observed_at,name,kind,value,point_sum,point_count,
                      point_min,point_max,explicit_bounds_json,bucket_counts_json,attributes_json,
                      thread_id,turn_id,response_id,tool_name,start_time_unix_nano,time_unix_nano,
                      data_source,confidence,estimated
               FROM metric_points {where_sql} ORDER BY observed_at,id"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), metric_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn usage_calls(&self, day: Option<&str>) -> Result<Vec<UsageCall>> {
        let (where_sql, values) = self.owned_filter(
            "c.completed_at",
            UsageFilter {
                from_date: day,
                to_date: day,
                ..Default::default()
            },
        );
        let sql = format!(
            r#"SELECT c.event_fingerprint,t.codex_turn_id,c.response_id,c.completed_at,c.model,
                      c.actual_model,c.provider,c.reasoning_effort,c.reasoning_mode,c.service_tier,
                      c.input_tokens,c.cached_input_tokens,c.cache_write_tokens,c.output_tokens,
                      c.reasoning_tokens,c.total_tokens,c.success,c.error_type,c.retry_index,
                      c.cost_usd,c.pricing_version,c.data_source,c.confidence,c.estimated,
                      s.project_name,s.codex_thread_id
               FROM llm_calls c JOIN sessions s ON s.id=c.session_id
               LEFT JOIN turns t ON t.id=c.turn_id {where_sql} ORDER BY c.completed_at,c.id"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(UsageCall {
                call: llm_call_from_row(row, 0)?,
                project_name: row.get(24)?,
                codex_thread_id: row.get(25)?,
                codex_turn_id: row.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn project_breakdown(&self, day: Option<&str>) -> Result<Vec<ProjectUsage>> {
        let (where_sql, values) = self.owned_filter(
            "c.completed_at",
            UsageFilter {
                from_date: day,
                to_date: day,
                ..Default::default()
            },
        );
        let sql = format!(
            r#"WITH compaction_counts AS (
                    SELECT thread_id,COUNT(*) compactions FROM compactions GROUP BY thread_id
                ), project_compactions AS (
                    SELECT s.project_name,SUM(COALESCE(x.compactions,0)) compactions
                    FROM sessions s LEFT JOIN compaction_counts x ON x.thread_id=s.codex_thread_id
                    GROUP BY s.project_name
                )
                SELECT COALESCE(s.project_name,'Unknown'),COUNT(DISTINCT s.id),
                       COUNT(DISTINCT c.turn_id),COUNT(*),COALESCE(SUM(c.input_tokens),0),
                       COALESCE(SUM(c.cached_input_tokens),0),COALESCE(SUM(c.output_tokens),0),
                       COALESCE(SUM(c.total_tokens),0),SUM(c.cost_usd),
                       COALESCE(SUM(CASE WHEN c.retry_index>0 THEN c.total_tokens ELSE 0 END),0),
                       COALESCE(MAX(pc.compactions),0)
                FROM llm_calls c JOIN sessions s ON s.id=c.session_id
                LEFT JOIN project_compactions pc ON pc.project_name IS s.project_name
                {where_sql} GROUP BY s.project_name ORDER BY SUM(c.total_tokens) DESC"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(ProjectUsage {
                project: row.get(0)?,
                sessions: row.get(1)?,
                turns: row.get(2)?,
                calls: row.get(3)?,
                input_tokens: row.get(4)?,
                cached_input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                cost_usd: row.get(8)?,
                retry_tokens: row.get(9)?,
                compactions: row.get(10)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn project_names(&self) -> Result<Vec<String>> {
        let (owner, owner_value) = self.owner_predicate("s");
        let sql = format!(
            r#"WITH project_activity AS (
                    SELECT s.project_name,COALESCE(c.completed_at,c.started_at,s.ended_at,s.started_at) used_at
                    FROM sessions s LEFT JOIN llm_calls c ON c.session_id=s.id WHERE {owner}
                )
                SELECT COALESCE(project_name,'Unknown'),MAX(used_at) last_used_at
                FROM project_activity GROUP BY project_name
                ORDER BY last_used_at DESC,1 COLLATE NOCASE"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map([owner_value], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn tool_breakdown(&self, day: Option<&str>) -> Result<Vec<ToolUsage>> {
        let (where_sql, values) = self.owned_filter(
            "t.completed_at",
            UsageFilter {
                from_date: day,
                to_date: day,
                ..Default::default()
            },
        );
        let sql = format!(
            r#"SELECT t.tool_name,COUNT(*),
                      SUM(CASE WHEN t.success=1 THEN 1 ELSE 0 END),
                      SUM(CASE WHEN t.success IS NOT NULL THEN 1 ELSE 0 END),
                      AVG(t.duration_ms),MAX(t.duration_ms),SUM(t.duration_ms)
               FROM tool_calls t JOIN sessions s ON s.id=t.session_id {where_sql}
               GROUP BY t.tool_name ORDER BY SUM(t.duration_ms) DESC"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(ToolUsage {
                tool_name: row.get(0)?,
                calls: row.get(1)?,
                successes: row.get(2)?,
                known_outcomes: row.get(3)?,
                avg_ms: row.get(4)?,
                max_ms: row.get(5)?,
                total_ms: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn tool_durations(&self, day: Option<&str>) -> Result<HashMap<String, Vec<i64>>> {
        let (where_sql, values) = self.owned_filter(
            "t.completed_at",
            UsageFilter {
                from_date: day,
                to_date: day,
                ..Default::default()
            },
        );
        let sql = format!(
            "SELECT t.tool_name,t.duration_ms FROM tool_calls t JOIN sessions s ON s.id=t.session_id {where_sql} AND t.duration_ms IS NOT NULL"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values.iter()))?;
        let mut output: HashMap<String, Vec<i64>> = HashMap::new();
        while let Some(row) = rows.next()? {
            output.entry(row.get(0)?).or_default().push(row.get(1)?);
        }
        Ok(output)
    }

    pub fn turn_waterfall(&self, turn_id: &str) -> Result<Option<TurnWaterfall>> {
        let (owner, owner_value) = self.owner_predicate("s");
        let owner = owner.replace("?1", "?2");
        let sql = format!(
            r#"SELECT t.codex_turn_id,t.started_at,t.completed_at,t.status,t.model,
                      t.reasoning_effort,t.ttft_ms,t.ttfm_ms,t.e2e_ms,
                      s.codex_thread_id,s.project_name,t.id
               FROM turns t JOIN sessions s ON s.id=t.session_id
               WHERE t.codex_turn_id=?1 AND {owner}"#
        );
        let detail = self
            .connection
            .query_row(&sql, params![turn_id, owner_value], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            })
            .optional()?;
        let Some((
            codex_turn_id,
            started_at,
            completed_at,
            status,
            model,
            reasoning_effort,
            ttft_ms,
            ttfm_ms,
            e2e_ms,
            codex_thread_id,
            project_name,
            db_id,
        )) = detail
        else {
            return Ok(None);
        };
        let mut call_statement = self.connection.prepare(
            r#"SELECT event_fingerprint,?2,response_id,completed_at,model,actual_model,provider,
                      reasoning_effort,reasoning_mode,service_tier,input_tokens,cached_input_tokens,
                      cache_write_tokens,output_tokens,reasoning_tokens,total_tokens,success,error_type,
                      retry_index,cost_usd,pricing_version,data_source,confidence,estimated
               FROM llm_calls WHERE turn_id=?1 ORDER BY COALESCE(started_at,completed_at),id"#,
        )?;
        let calls = call_statement
            .query_map(params![db_id, turn_id], |row| llm_call_from_row(row, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut tool_statement = self.connection.prepare(
            r#"SELECT source_call_id,?2,tool_name,started_at,completed_at,duration_ms,
                      success,exit_code,data_source,confidence,estimated
               FROM tool_calls WHERE turn_id=?1 ORDER BY COALESCE(started_at,completed_at),id"#,
        )?;
        let tools = tool_statement
            .query_map(params![db_id, turn_id], tool_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(TurnWaterfall {
            codex_turn_id,
            started_at,
            completed_at,
            status,
            model,
            reasoning_effort,
            ttft_ms,
            ttfm_ms,
            e2e_ms,
            codex_thread_id,
            project_name,
            calls,
            tools,
        }))
    }

    pub fn recent_network(
        &self,
        limit: usize,
        project: Option<&str>,
    ) -> Result<Vec<NetworkFlowRecord>> {
        let base_columns = "nf.event_fingerprint,nf.mode,nf.data_source,nf.started_at,nf.ended_at,nf.destination_host,nf.destination_ip,nf.destination_port,nf.protocol,nf.tls_version,nf.alpn,nf.http_status,nf.request_bytes,nf.response_bytes,nf.packets_out,nf.packets_in,nf.dns_ms,nf.tcp_ms,nf.tls_ms,nf.ttfb_ms,nf.first_event_ms,nf.first_output_ms,nf.duration_ms,nf.success,nf.error_type,nf.thread_id,nf.turn_id,nf.response_id,nf.confidence";
        let (sql, values) = if let Some(project) = project {
            let (owner, owner_value) = self.owner_predicate("s");
            (
                format!(
                    "SELECT {base_columns} FROM network_flows nf JOIN sessions s ON s.codex_thread_id=nf.thread_id WHERE {owner} AND COALESCE(s.project_name,'Unknown')=?2 ORDER BY COALESCE(nf.started_at,nf.created_at) DESC LIMIT ?3"
                ),
                vec![
                    owner_value,
                    Value::Text(project.into()),
                    Value::Integer(limit.max(1) as i64),
                ],
            )
        } else {
            (
                format!(
                    "SELECT {base_columns} FROM network_flows nf ORDER BY COALESCE(nf.started_at,nf.created_at) DESC LIMIT ?1"
                ),
                vec![Value::Integer(limit.max(1) as i64)],
            )
        };
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), network_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn response_performance_range(
        &self,
        from_date: Option<&str>,
        to_date: Option<&str>,
        project: Option<&str>,
    ) -> Result<Vec<ResponsePerformance>> {
        let (where_sql, values) = self.owned_filter(
            "t.completed_at",
            UsageFilter {
                from_date,
                to_date,
                project,
                account: None,
            },
        );
        let sql = format!(
            r#"SELECT t.completed_at,strftime('%H:%M:%S',t.completed_at,'localtime'),
                      COALESCE(t.model,'Unknown'),t.output_tokens,t.ttft_ms,t.e2e_ms,
                      (SELECT AVG(c.output_tps) FROM llm_calls c WHERE c.turn_id=t.id AND c.output_tps IS NOT NULL)
               FROM turns t JOIN sessions s ON s.id=t.session_id {where_sql}
               AND t.completed_at IS NOT NULL ORDER BY t.completed_at DESC"#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(ResponsePerformance {
                completed_at: row.get(0)?,
                local_time: row.get(1)?,
                model: row.get(2)?,
                output_tokens: row.get(3)?,
                ttft_ms: row.get(4)?,
                e2e_ms: row.get(5)?,
                exact_output_tps: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn telemetry_retry_summary(&self, day: Option<&str>) -> Result<TelemetryRetrySummary> {
        let mut sql = "SELECT attributes_json,duration_ms FROM telemetry_logs WHERE event_name='codex.api_request'".to_string();
        let mut values = Vec::new();
        if let Some(day) = day {
            sql.push_str(" AND date(observed_at,'localtime')=date(?)");
            values.push(Value::Text(day.into()));
        }
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values.iter()))?;
        let mut result = TelemetryRetrySummary::default();
        while let Some(row) = rows.next()? {
            let raw: String = row
                .get::<_, Option<String>>(0)?
                .unwrap_or_else(|| "{}".into());
            let attrs: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
            let attempt = attrs.get("attempt").and_then(json_i64).unwrap_or(0);
            if attempt > 0 {
                result.attempts += 1;
                result.duration_ms += row.get::<_, Option<f64>>(1)?.unwrap_or(0.0);
                let success = attrs
                    .get("success")
                    .map(json_string)
                    .unwrap_or_else(|| "true".into());
                result.failures += i64::from(!success.eq_ignore_ascii_case("true"));
            }
        }
        Ok(result)
    }

    pub fn claim_unassigned_account(&self, label: &str) -> Result<usize> {
        let label = label.trim();
        if label.is_empty() {
            bail!("account label cannot be empty");
        }
        let (predicate, owner) = self.owner_predicate("sessions");
        Ok(self.connection.execute(
            &format!(
                "UPDATE sessions SET account_label=?1 WHERE {} AND account_label IS NULL",
                predicate.replace("?1", "?2")
            ),
            params![label, owner],
        )?)
    }

    pub fn integrity_check(&self) -> Result<String> {
        Ok(self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?)
    }

    pub fn counts(&self) -> Result<HashMap<String, i64>> {
        let mut output = HashMap::new();
        for table in [
            "sessions",
            "turns",
            "llm_calls",
            "tool_calls",
            "pricing_snapshots",
            "metric_points",
            "telemetry_logs",
            "compactions",
            "network_flows",
        ] {
            let count =
                self.connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?;
            output.insert(table.to_string(), count);
        }
        Ok(output)
    }

    fn owned_filter(&self, column: &str, filter: UsageFilter<'_>) -> (String, Vec<Value>) {
        let mut clauses = Vec::new();
        let mut values = Vec::new();
        if let Some(uid) = self.owner_uid {
            clauses.push("s.owner_uid = ?".to_string());
            values.push(Value::Integer(uid));
        } else {
            clauses.push("s.owner_username = ?".to_string());
            values.push(Value::Text(self.owner_username.clone()));
        }
        if let Some(value) = filter.from_date {
            clauses.push(format!("date({column},'localtime') >= date(?)"));
            values.push(Value::Text(value.to_string()));
        }
        if let Some(value) = filter.to_date {
            clauses.push(format!("date({column},'localtime') <= date(?)"));
            values.push(Value::Text(value.to_string()));
        }
        match filter.account {
            Some("Unassigned") => clauses.push("s.account_label IS NULL".into()),
            Some(value) => {
                clauses.push("s.account_label = ?".into());
                values.push(Value::Text(value.to_string()));
            }
            None => {}
        }
        if let Some(value) = filter.project {
            clauses.push("COALESCE(s.project_name,'Unknown') = ?".into());
            values.push(Value::Text(value.to_string()));
        }
        (format!("WHERE {}", clauses.join(" AND ")), values)
    }

    fn owner_predicate(&self, alias: &str) -> (String, Value) {
        match self.owner_uid {
            Some(uid) => (format!("{alias}.owner_uid=?1"), Value::Integer(uid)),
            None => (
                format!("{alias}.owner_username=?1"),
                Value::Text(self.owner_username.clone()),
            ),
        }
    }

    fn table_exists(&self, table: &str) -> Result<bool> {
        Ok(self
            .connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn column_exists(&self, table: &str, column: &str) -> Result<bool> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
        for result in columns {
            if result? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn llm_call_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<LlmCallRecord> {
    let source: String = row.get(offset + 21)?;
    Ok(LlmCallRecord {
        event_fingerprint: row.get(offset)?,
        turn_id: row.get(offset + 1)?,
        response_id: row.get(offset + 2)?,
        completed_at: row.get(offset + 3)?,
        model: row.get(offset + 4)?,
        actual_model: row.get(offset + 5)?,
        provider: row.get(offset + 6)?,
        reasoning_effort: row.get(offset + 7)?,
        reasoning_mode: row.get(offset + 8)?,
        service_tier: row.get(offset + 9)?,
        usage: TokenUsage {
            input_tokens: row.get(offset + 10)?,
            cached_input_tokens: row.get(offset + 11)?,
            cache_write_tokens: row.get(offset + 12)?,
            output_tokens: row.get(offset + 13)?,
            reasoning_tokens: row.get(offset + 14)?,
            total_tokens: row.get(offset + 15)?,
        },
        success: row.get::<_, i64>(offset + 16)? != 0,
        error_type: row.get(offset + 17)?,
        retry_index: row.get(offset + 18)?,
        cost_usd: row.get(offset + 19)?,
        pricing_version: row.get(offset + 20)?,
        quality: Quality {
            source,
            confidence: confidence_from(&row.get::<_, String>(offset + 22)?),
            estimated: row.get::<_, i64>(offset + 23)? != 0,
        },
    })
}

fn tool_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ToolCallRecord> {
    let source: String = row.get(8)?;
    Ok(ToolCallRecord {
        call_id: row.get(0)?,
        turn_id: row.get(1)?,
        tool_name: row.get(2)?,
        started_at: row.get(3)?,
        completed_at: row.get(4)?,
        duration_ms: row.get(5)?,
        success: row.get::<_, Option<i64>>(6)?.map(|value| value != 0),
        exit_code: row.get(7)?,
        quality: Quality {
            source,
            confidence: confidence_from(&row.get::<_, String>(9)?),
            estimated: row.get::<_, i64>(10)? != 0,
        },
    })
}

fn metric_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MetricPointRecord> {
    let bounds: Option<String> = row.get(9)?;
    let buckets: Option<String> = row.get(10)?;
    let attributes: Option<String> = row.get(11)?;
    let source: String = row.get(18)?;
    Ok(MetricPointRecord {
        event_fingerprint: row.get(0)?,
        observed_at: row.get(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        value: row.get(4)?,
        point_sum: row.get(5)?,
        point_count: row.get(6)?,
        point_min: row.get(7)?,
        point_max: row.get(8)?,
        explicit_bounds: bounds
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_default(),
        bucket_counts: buckets
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_default(),
        attributes: attributes
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_default(),
        thread_id: row.get(12)?,
        turn_id: row.get(13)?,
        response_id: row.get(14)?,
        tool_name: row.get(15)?,
        start_time_unix_nano: row.get(16)?,
        time_unix_nano: row.get(17)?,
        quality: Quality {
            source,
            confidence: confidence_from(&row.get::<_, String>(19)?),
            estimated: row.get::<_, i64>(20)? != 0,
        },
    })
}

fn network_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NetworkFlowRecord> {
    let data_source: String = row.get(2)?;
    Ok(NetworkFlowRecord {
        event_fingerprint: row.get(0)?,
        mode: row.get(1)?,
        data_source: data_source.clone(),
        started_at: row.get(3)?,
        ended_at: row.get(4)?,
        destination_host: row.get(5)?,
        destination_ip: row.get(6)?,
        destination_port: row.get(7)?,
        protocol: row.get(8)?,
        tls_version: row.get(9)?,
        alpn: row.get(10)?,
        http_status: row.get(11)?,
        request_bytes: row.get(12)?,
        response_bytes: row.get(13)?,
        packets_out: row.get(14)?,
        packets_in: row.get(15)?,
        dns_ms: row.get(16)?,
        tcp_ms: row.get(17)?,
        tls_ms: row.get(18)?,
        ttfb_ms: row.get(19)?,
        first_event_ms: row.get(20)?,
        first_output_ms: row.get(21)?,
        duration_ms: row.get(22)?,
        success: row.get::<_, Option<i64>>(23)?.map(|value| value != 0),
        error_type: row.get(24)?,
        thread_id: row.get(25)?,
        turn_id: row.get(26)?,
        response_id: row.get(27)?,
        quality: Quality {
            source: data_source,
            confidence: confidence_from(&row.get::<_, String>(28)?),
            estimated: false,
        },
    })
}

fn confidence_from(value: &str) -> Confidence {
    match value {
        "exact" => Confidence::Exact,
        "derived" => Confidence::Derived,
        "estimated" => Confidence::Estimated,
        _ => Confidence::Unknown,
    }
}

fn json_i64(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn json_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Bool(value) => value.to_string(),
        other => other.to_string(),
    }
}

fn apply_metric_point(transaction: &Transaction<'_>, point: &MetricPointRecord) -> Result<()> {
    let value = point.value.or_else(|| {
        point
            .point_sum
            .zip(point.point_count.filter(|count| *count != 0))
            .map(|(sum, count)| sum / count as f64)
    });
    let Some(value) = value else { return Ok(()) };
    let turn_column = match point.name.as_str() {
        "codex.turn.e2e_duration_ms" => Some("e2e_ms"),
        "codex.turn.ttft.duration_ms" => Some("ttft_ms"),
        "codex.turn.ttfm.duration_ms" => Some("ttfm_ms"),
        _ => None,
    };
    if let (Some(column), Some(turn_id)) = (turn_column, point.turn_id.as_deref()) {
        transaction.execute(
            &format!("UPDATE turns SET {column}=COALESCE({column},?1) WHERE codex_turn_id=?2"),
            params![value.round() as i64, turn_id],
        )?;
    }
    let call_column = match point.name.as_str() {
        "codex.api_request.duration_ms" => Some("request_duration_ms"),
        "codex.responses_api_overhead.duration_ms" => Some("overhead_ms"),
        "codex.responses_api_inference_time.duration_ms" => Some("inference_ms"),
        "codex.responses_api_engine_iapi_ttft.duration_ms" => Some("ttfb_ms"),
        "codex.responses_api_engine_service_ttft.duration_ms" => Some("ttft_ms"),
        "codex.responses_api_engine_iapi_tbt.duration_ms"
        | "codex.responses_api_engine_service_tbt.duration_ms" => Some("avg_tbt_ms"),
        _ => None,
    };
    if let (Some(column), Some(response_id)) = (call_column, point.response_id.as_deref()) {
        transaction.execute(
            &format!("UPDATE llm_calls SET {column}=COALESCE({column},?1) WHERE response_id=?2"),
            params![value, response_id],
        )?;
    }
    if point.name == "codex.turn.token_usage" {
        if let Some(turn_id) = point.turn_id.as_deref() {
            let token_column = match point.attributes.get("token_type").map(String::as_str) {
                Some("input") => Some("input_tokens"),
                Some("cached_input") => Some("cached_input_tokens"),
                Some("cache_write_input") => Some("cache_write_tokens"),
                Some("output") => Some("output_tokens"),
                Some("reasoning_output") => Some("reasoning_tokens"),
                Some("total") => Some("total_tokens"),
                _ => None,
            };
            if let Some(column) = token_column {
                transaction.execute(
                    &format!("UPDATE turns SET {column}=MAX({column},?1) WHERE codex_turn_id=?2"),
                    params![value.round() as i64, turn_id],
                )?;
            }
        }
    }
    Ok(())
}

fn refresh_turn_aggregates(
    transaction: &Transaction<'_>,
    turn_ids: impl IntoIterator<Item = i64>,
) -> Result<()> {
    for turn_id in turn_ids {
        transaction.execute(
            r#"UPDATE turns SET
                input_tokens=COALESCE((SELECT SUM(input_tokens) FROM llm_calls WHERE turn_id=?1),0),
                cached_input_tokens=COALESCE((SELECT SUM(cached_input_tokens) FROM llm_calls WHERE turn_id=?1),0),
                cache_write_tokens=COALESCE((SELECT SUM(cache_write_tokens) FROM llm_calls WHERE turn_id=?1),0),
                output_tokens=COALESCE((SELECT SUM(output_tokens) FROM llm_calls WHERE turn_id=?1),0),
                reasoning_tokens=COALESCE((SELECT SUM(reasoning_tokens) FROM llm_calls WHERE turn_id=?1),0),
                total_tokens=COALESCE((SELECT SUM(total_tokens) FROM llm_calls WHERE turn_id=?1),0),
                cost_usd=(SELECT SUM(cost_usd) FROM llm_calls WHERE turn_id=?1),
                tool_time_ms=(SELECT SUM(duration_ms) FROM tool_calls WHERE turn_id=?1)
                WHERE id=?1"#,
            [turn_id],
        )?;
    }
    Ok(())
}

fn saturating_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[cfg(unix)]
fn current_uid() -> Option<i64> {
    // SAFETY: geteuid has no preconditions and does not retain pointers.
    Some(unsafe { libc::geteuid() as i64 })
}

#[cfg(not(unix))]
fn current_uid() -> Option<i64> {
    None
}

fn current_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tempfile::tempdir;

    use super::*;
    use crate::{collector::SessionCollector, pricing::PricingCatalog};

    fn fixture(thread: &str, turn: &str, timestamp: &str) -> String {
        format!(
            "{{\"timestamp\":\"{timestamp}\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread}\",\"cwd\":\"/work/project-a\"}}}}\n{{\"timestamp\":\"{timestamp}\",\"type\":\"turn_context\",\"payload\":{{\"turn_id\":\"{turn}\",\"model\":\"gpt-5.6-sol\",\"effort\":\"high\"}}}}\n{{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":10,\"total_tokens\":110}},\"last_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":10,\"total_tokens\":110}}}}}}}}\n"
        )
    }

    #[test]
    fn imports_into_python_schema_and_deduplicates_fork_replay() {
        let temp = tempdir().unwrap();
        let storage =
            Storage::with_identity(temp.path().join("meter.db"), Some(501), "tester", None)
                .unwrap();
        storage.migrate().unwrap();
        let catalog = PricingCatalog::bundled().unwrap();
        storage.sync_pricing(&catalog).unwrap();
        let collector = SessionCollector::new(&catalog);
        let first = collector
            .collect_reader(
                Cursor::new(fixture("thread-a", "turn-a", "2026-08-12T01:00:00Z")),
                "one",
            )
            .unwrap();
        let fork = collector
            .collect_reader(
                Cursor::new(fixture("thread-b", "turn-a", "2026-08-12T02:00:00Z")),
                "two",
            )
            .unwrap();
        let inserted = storage
            .import_session(
                &first,
                SourceMetadata {
                    source_path: "one".into(),
                    size_bytes: 10,
                    mtime_ns: 1,
                },
            )
            .unwrap();
        assert_eq!(inserted.1, 1);
        let replayed = storage
            .import_session(
                &fork,
                SourceMetadata {
                    source_path: "two".into(),
                    size_bytes: 10,
                    mtime_ns: 2,
                },
            )
            .unwrap();
        assert_eq!(replayed.1, 0);
        assert_eq!(storage.counts().unwrap()["llm_calls"], 1);
        assert_eq!(
            storage.overview(None, None, None).unwrap().total_tokens,
            110
        );
        assert_eq!(storage.project_names().unwrap(), vec!["project-a"]);
        assert_eq!(storage.integrity_check().unwrap(), "ok");
    }

    #[test]
    fn empty_database_aggregates_return_zero_instead_of_null_errors() {
        let temp = tempdir().unwrap();
        let storage =
            Storage::with_identity(temp.path().join("meter.db"), Some(501), "tester", None)
                .unwrap();
        storage.migrate().unwrap();
        let overview = storage.overview_range(UsageFilter::default()).unwrap();
        assert_eq!(overview.calls, 0);
        assert_eq!(overview.sessions, 0);
        assert_eq!(overview.turns, 0);
        assert_eq!(overview.total_tokens, 0);
        assert_eq!(overview.unpriced_calls, 0);
        assert_eq!(overview.cost_usd, None);
        assert!(
            storage
                .model_breakdown(UsageFilter::default())
                .unwrap()
                .is_empty()
        );
        assert!(storage.account_breakdown().unwrap().is_empty());
        assert!(storage.usage_history("day", None, None).unwrap().is_empty());
        assert!(storage.provider_breakdown(None).unwrap().is_empty());
        assert!(storage.agent_breakdown(None).unwrap().is_empty());
        assert!(storage.project_breakdown(None).unwrap().is_empty());
        assert!(storage.tool_breakdown(None).unwrap().is_empty());
    }

    #[test]
    fn live_and_observability_paths_round_trip_through_all_primary_queries() {
        let temp = tempdir().unwrap();
        let storage =
            Storage::with_identity(temp.path().join("meter.db"), Some(501), "tester", None)
                .unwrap();
        storage.migrate().unwrap();
        storage
            .ensure_live_session(
                "live-thread",
                Some("2026-08-12T01:00:00Z"),
                Some("/work/live-project"),
                Some("gpt-5.6-sol"),
                "app_server",
            )
            .unwrap();
        storage
            .upsert_live_turn(
                "live-thread",
                "live-turn",
                LiveTurnUpdate {
                    started_at: Some("2026-08-12T01:00:00Z"),
                    completed_at: Some("2026-08-12T01:00:02Z"),
                    status: "completed",
                    model: Some("gpt-5.6-sol"),
                    reasoning_effort: Some("high"),
                    ttft_ms: Some(100),
                    e2e_ms: Some(2_000),
                    ..Default::default()
                },
            )
            .unwrap();
        let call = LlmCallRecord {
            event_fingerprint: "live-call".into(),
            turn_id: Some("live-turn".into()),
            response_id: Some("live-response".into()),
            completed_at: Some("2026-08-12T01:00:02Z".into()),
            model: Some("gpt-5.6-sol".into()),
            actual_model: None,
            provider: Some("openai".into()),
            reasoning_effort: Some("high".into()),
            reasoning_mode: None,
            service_tier: None,
            usage: TokenUsage {
                input_tokens: 100,
                cached_input_tokens: 80,
                output_tokens: 10,
                total_tokens: 110,
                ..Default::default()
            },
            success: true,
            error_type: None,
            retry_index: 0,
            cost_usd: Some(0.001),
            pricing_version: Some("test".into()),
            quality: Quality::exact("app_server"),
        };
        assert!(
            storage
                .insert_live_call(
                    "live-thread",
                    &call,
                    LiveCallTimings {
                        started_at: Some("2026-08-12T01:00:00Z"),
                        first_event_at: Some("2026-08-12T01:00:00.050Z"),
                        first_model_item_at: Some("2026-08-12T01:00:00.100Z"),
                        request_duration_ms: Some(2_000),
                        ttft_ms: Some(100),
                        ..Default::default()
                    }
                )
                .unwrap()
        );
        storage
            .upsert_live_tool(
                "live-thread",
                &ToolCallRecord {
                    call_id: "tool-1".into(),
                    turn_id: Some("live-turn".into()),
                    tool_name: "shell".into(),
                    started_at: Some("2026-08-12T01:00:01Z".into()),
                    completed_at: Some("2026-08-12T01:00:02Z".into()),
                    duration_ms: Some(1_000),
                    success: Some(true),
                    exit_code: Some(0),
                    quality: Quality::exact("app_server"),
                },
                "app_server",
            )
            .unwrap();
        storage
            .insert_metric_points(&[MetricPointRecord {
                event_fingerprint: "metric-1".into(),
                observed_at: Some("2026-08-12T01:00:02Z".into()),
                name: "codex.api_request.duration_ms".into(),
                kind: "gauge".into(),
                value: Some(2_000.0),
                point_sum: None,
                point_count: None,
                point_min: None,
                point_max: None,
                explicit_bounds: vec![],
                bucket_counts: vec![],
                attributes: HashMap::new(),
                thread_id: Some("live-thread".into()),
                turn_id: Some("live-turn".into()),
                response_id: Some("live-response".into()),
                tool_name: None,
                start_time_unix_nano: None,
                time_unix_nano: None,
                quality: Quality::exact("otlp_http"),
            }])
            .unwrap();
        storage
            .insert_telemetry_logs(&[TelemetryLogRecord {
                event_fingerprint: "log-1".into(),
                observed_at: Some("2026-08-12T01:00:02Z".into()),
                event_name: "codex.api_request".into(),
                severity: None,
                attributes: HashMap::from([
                    ("attempt".into(), "1".into()),
                    ("success".into(), "false".into()),
                ]),
                thread_id: Some("live-thread".into()),
                turn_id: Some("live-turn".into()),
                response_id: None,
                item_id: None,
                tool_name: None,
                duration_ms: Some(12.0),
                status: None,
                success: Some(false),
                quality: Quality::exact("otlp_http"),
            }])
            .unwrap();
        assert!(
            storage
                .insert_compaction(
                    "compact-1",
                    "live-thread",
                    Some("live-turn"),
                    Some("2026-08-12T01:00:02Z"),
                    "app_server"
                )
                .unwrap()
        );
        assert!(
            storage
                .insert_network_flow(&NetworkFlowRecord {
                    event_fingerprint: "network-1".into(),
                    mode: "probe".into(),
                    data_source: "network".into(),
                    started_at: Some("2026-08-12T01:00:00Z".into()),
                    ended_at: Some("2026-08-12T01:00:01Z".into()),
                    destination_host: Some("example.invalid".into()),
                    destination_ip: None,
                    destination_port: Some(443),
                    protocol: Some("tcp".into()),
                    tls_version: None,
                    alpn: None,
                    http_status: None,
                    request_bytes: 0,
                    response_bytes: 0,
                    packets_out: 1,
                    packets_in: 1,
                    dns_ms: None,
                    tcp_ms: Some(10.0),
                    tls_ms: None,
                    ttfb_ms: None,
                    first_event_ms: None,
                    first_output_ms: None,
                    duration_ms: Some(1_000.0),
                    success: Some(true),
                    error_type: None,
                    thread_id: Some("live-thread".into()),
                    turn_id: Some("live-turn".into()),
                    response_id: None,
                    quality: Quality::exact("network"),
                })
                .unwrap()
        );

        assert_eq!(
            storage.account_breakdown().unwrap()[0].account,
            "Unassigned"
        );
        assert_eq!(storage.claim_unassigned_account("work").unwrap(), 1);
        assert_eq!(
            storage
                .provider_breakdown(Some("2026-08-12"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(storage.agent_breakdown(None).unwrap().len(), 1);
        assert_eq!(
            storage
                .export_rows(None, None, Some("live-thread"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            storage
                .metric_points(None, &["codex.api_request.duration_ms"])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(storage.usage_calls(None).unwrap().len(), 1);
        assert_eq!(storage.tool_breakdown(None).unwrap()[0].calls, 1);
        assert_eq!(storage.tool_durations(None).unwrap()["shell"], vec![1_000]);
        assert_eq!(
            storage
                .turn_waterfall("live-turn")
                .unwrap()
                .unwrap()
                .calls
                .len(),
            1
        );
        assert_eq!(
            storage
                .recent_network(10, Some("live-project"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            storage
                .response_performance_range(None, None, None)
                .unwrap()
                .len(),
            1
        );
        let retry = storage.telemetry_retry_summary(None).unwrap();
        assert_eq!(
            (retry.attempts, retry.failures, retry.duration_ms),
            (1, 1, 12.0)
        );
    }

    #[test]
    fn source_metadata_drives_incremental_state() {
        let temp = tempdir().unwrap();
        let storage =
            Storage::with_identity(temp.path().join("meter.db"), Some(501), "tester", None)
                .unwrap();
        storage.migrate().unwrap();
        let catalog = PricingCatalog::bundled().unwrap();
        let parsed = SessionCollector::new(&catalog)
            .collect_reader(
                Cursor::new(fixture("thread-a", "turn-a", "2026-08-12T01:00:00Z")),
                "remote",
            )
            .unwrap();
        storage
            .import_session(
                &parsed,
                SourceMetadata {
                    source_path: "ssh://host/rollout.jsonl".into(),
                    size_bytes: 55,
                    mtime_ns: 99,
                },
            )
            .unwrap();
        assert!(
            storage
                .source_is_current("ssh://host/rollout.jsonl", 55, 99)
                .unwrap()
        );
        assert!(
            !storage
                .source_is_current("ssh://host/rollout.jsonl", 56, 99)
                .unwrap()
        );
    }

    #[test]
    fn export_json_matches_python_flat_shape_and_integer_flag() {
        let row = ExportCall {
            session_id: "thread-a".into(),
            turn_id: Some("turn-a".into()),
            response_id: None,
            completed_at: Some("2026-08-12T01:00:00Z".into()),
            model: Some("gpt-5.6-sol".into()),
            reasoning_effort: Some("high".into()),
            usage: TokenUsage {
                input_tokens: 10,
                cached_input_tokens: 7,
                cache_write_tokens: 0,
                output_tokens: 3,
                reasoning_tokens: 1,
                total_tokens: 13,
            },
            cost_usd: Some(0.5),
            data_source: "session_jsonl_cumulative_delta".into(),
            confidence: "derived".into(),
            estimated: 1,
        };

        let value = serde_json::to_value(row).unwrap();
        assert_eq!(value["input_tokens"], 10);
        assert_eq!(value["cached_input_tokens"], 7);
        assert_eq!(value["estimated"], 1);
        assert!(value.get("usage").is_none());
    }

    #[test]
    fn prompt_response_and_tool_payload_never_reach_sqlite_or_wal() {
        let temp = tempdir().unwrap();
        let database = temp.path().join("meter.db");
        let secret = "NEVER-PERSIST-THIS-CONTENT-47d67d";
        let input = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"privacy-thread\"}}}}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{secret}\"}}}}\n{{\"timestamp\":\"2026-08-12T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":1,\"total_tokens\":1}},\"last_token_usage\":{{\"input_tokens\":1,\"total_tokens\":1}}}},\"tool_payload\":\"{secret}\"}}}}\n"
        );
        let catalog = PricingCatalog::bundled().unwrap();
        let parsed = SessionCollector::new(&catalog)
            .collect_reader(Cursor::new(input), "privacy")
            .unwrap();
        {
            let storage = Storage::with_identity(&database, Some(501), "tester", None).unwrap();
            storage.migrate().unwrap();
            storage
                .import_session(
                    &parsed,
                    SourceMetadata {
                        source_path: "privacy".into(),
                        size_bytes: 1,
                        mtime_ns: 1,
                    },
                )
                .unwrap();
        }
        for path in [database.clone(), database.with_extension("db-wal")] {
            if path.exists() {
                let bytes = fs::read(path).unwrap();
                assert!(!String::from_utf8_lossy(&bytes).contains(secret));
            }
        }
    }
}
