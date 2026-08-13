//! Structured application service used by the macOS desktop frontend.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::analytics::{UsageSample, cache_summary, context_and_retry_summary};
use crate::collector::{SessionCollector, discover_rollouts};
use crate::config::{self, LocalIdentity};
use crate::pricing::PricingCatalog;
use crate::storage::{
    HistoryBucket, ImportStats, ModelUsage, Overview, RemoteSourceStatus, ResponsePerformance,
    SessionSummary, Storage, UsageFilter,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DashboardPeriod {
    #[default]
    Day,
    Week,
    Month,
    All,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshot {
    pub generated_at: String,
    pub anchor_date: String,
    pub period: DashboardPeriod,
    pub period_label: String,
    pub project: Option<String>,
    pub account: Option<String>,
    pub overview: Overview,
    pub models: Vec<ModelUsage>,
    pub history: Vec<HistoryBucket>,
    pub weekly_history: Vec<HistoryBucket>,
    pub monthly_history: Vec<HistoryBucket>,
    pub recent_sessions: Vec<SessionSummary>,
    pub projects: Vec<String>,
    pub accounts: Vec<String>,
    pub remote_count: usize,
    pub owner_username: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheInsight {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub reuse_rate: f64,
    pub observed_cost_usd: f64,
    pub without_cache_usd: f64,
    pub savings_usd: f64,
    pub priced_calls: usize,
    pub unpriced_calls: usize,
    pub retry_calls: usize,
    pub retry_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceInsight {
    pub samples: usize,
    pub average_ttft_ms: Option<f64>,
    pub p95_ttft_ms: Option<f64>,
    pub average_e2e_ms: Option<f64>,
    pub p95_e2e_ms: Option<f64>,
    pub average_output_tps: Option<f64>,
    pub recent: Vec<ResponsePerformance>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInsight {
    pub started_at: Option<String>,
    pub mode: String,
    pub destination: String,
    pub duration_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
    pub request_bytes: i64,
    pub response_bytes: i64,
    pub success: Option<bool>,
    pub error_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsSnapshot {
    pub cache: CacheInsight,
    pub performance: PerformanceInsight,
    pub network: Vec<NetworkInsight>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshSummary {
    pub discovered_files: usize,
    pub imported_files: usize,
    pub skipped_files: usize,
    pub failed_files: usize,
    pub inserted_turns: usize,
    pub inserted_calls: usize,
    pub inserted_tools: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSettings {
    pub version: &'static str,
    pub pricing_catalog_version: String,
    pub pricing_source: String,
    pub meter_home: String,
    pub database_path: String,
    pub codex_home: String,
    pub sessions_path: String,
    pub owner_username: String,
    pub account_tracking: bool,
    pub account_label: Option<String>,
    pub remote_hosts: Vec<String>,
    pub remote_sources: Vec<RemoteSourceStatus>,
    pub privacy_summary: &'static str,
}

#[derive(Debug, Clone)]
pub struct MeterService {
    pub home: PathBuf,
    pub db_path: PathBuf,
    pub codex_home: PathBuf,
    pub identity: LocalIdentity,
    pub catalog: PricingCatalog,
}

impl MeterService {
    pub fn open_default() -> Result<Self> {
        Self::open(config::meter_home(None), config::codex_home())
    }

    pub fn open(home: PathBuf, codex_home: PathBuf) -> Result<Self> {
        config::initialize_home(&home)?;
        let identity = config::identity(&home);
        let catalog = PricingCatalog::load_with_bundled(&home.join("pricing.json"))?;
        let db_path = home.join("meter.db");
        let service = Self {
            home,
            db_path,
            codex_home,
            identity,
            catalog,
        };
        service.open_storage()?.close()?;
        Ok(service)
    }

    pub fn open_storage(&self) -> Result<Storage> {
        let storage = Storage::with_identity(
            &self.db_path,
            self.identity.uid.map(i64::from),
            &self.identity.username,
            self.identity.account_label.clone(),
        )?;
        storage.migrate()?;
        storage.sync_pricing(&self.catalog)?;
        storage.backfill_unpriced_calls(&self.catalog)?;
        Ok(storage)
    }

    pub fn dashboard(
        &self,
        period: DashboardPeriod,
        project: Option<&str>,
    ) -> Result<DashboardSnapshot> {
        self.dashboard_filtered(period, None, project, None)
    }

    pub fn dashboard_filtered(
        &self,
        period: DashboardPeriod,
        anchor: Option<&str>,
        project: Option<&str>,
        account: Option<&str>,
    ) -> Result<DashboardSnapshot> {
        let storage = self.open_storage()?;
        let anchor = parse_anchor(anchor)?;
        let (from, to, period_label) = period_bounds(period, anchor);
        let filter = UsageFilter {
            from_date: from.as_deref(),
            to_date: to.as_deref(),
            project,
            account,
        };
        let overview = storage.overview_range(filter)?;
        let models = storage.model_breakdown_range(filter)?;
        let history = storage.usage_history("day", account, project)?;
        let weekly_history = storage.usage_history("week", account, project)?;
        let monthly_history = storage.usage_history("month", account, project)?;
        let recent_sessions = storage.sessions_filtered(24, filter)?;
        let projects = storage.project_names()?;
        let accounts = storage.account_names()?;
        storage.close()?;
        Ok(DashboardSnapshot {
            generated_at: chrono::Utc::now().to_rfc3339(),
            anchor_date: anchor.to_string(),
            period,
            period_label,
            project: project.map(str::to_owned),
            account: account.map(str::to_owned),
            overview,
            models,
            history,
            weekly_history,
            monthly_history,
            recent_sessions,
            projects,
            accounts,
            remote_count: config::remote_hosts(&self.home).len(),
            owner_username: self.identity.username.clone(),
        })
    }

    pub fn insights(
        &self,
        period: DashboardPeriod,
        anchor: Option<&str>,
        project: Option<&str>,
        account: Option<&str>,
    ) -> Result<InsightsSnapshot> {
        let anchor = parse_anchor(anchor)?;
        let (from, to, _) = period_bounds(period, anchor);
        let filter = UsageFilter {
            from_date: from.as_deref(),
            to_date: to.as_deref(),
            project,
            account,
        };
        let storage = self.open_storage()?;
        let samples = storage
            .usage_calls_range(filter)?
            .into_iter()
            .map(|row| UsageSample {
                id: 0,
                turn_id: row.codex_turn_id,
                model: row.call.actual_model.or(row.call.model),
                provider: row.call.provider,
                completed_at: row.call.completed_at,
                usage: row.call.usage,
                retry_index: row.call.retry_index,
                cost_usd: row.call.cost_usd,
            })
            .collect::<Vec<_>>();
        let cache = cache_summary(&samples, &self.catalog);
        let retries = context_and_retry_summary(&samples);
        let performance_rows = storage.response_performance_filtered(filter)?;
        let performance = PerformanceInsight {
            samples: performance_rows.len(),
            average_ttft_ms: average(performance_rows.iter().filter_map(|row| row.ttft_ms)),
            p95_ttft_ms: percentile(performance_rows.iter().filter_map(|row| row.ttft_ms), 0.95),
            average_e2e_ms: average(performance_rows.iter().filter_map(|row| row.e2e_ms)),
            p95_e2e_ms: percentile(performance_rows.iter().filter_map(|row| row.e2e_ms), 0.95),
            average_output_tps: average_f64(
                performance_rows
                    .iter()
                    .filter_map(|row| row.exact_output_tps),
            ),
            recent: performance_rows.into_iter().take(12).collect(),
        };
        let network = storage
            .recent_network_filtered(16, filter)?
            .into_iter()
            .map(|row| NetworkInsight {
                started_at: row.started_at,
                mode: row.mode,
                destination: row
                    .destination_host
                    .or(row.destination_ip)
                    .unwrap_or_else(|| "Unknown destination".into()),
                duration_ms: row.duration_ms,
                ttfb_ms: row.ttfb_ms,
                request_bytes: row.request_bytes,
                response_bytes: row.response_bytes,
                success: row.success,
                error_type: row.error_type,
            })
            .collect();
        storage.close()?;
        Ok(InsightsSnapshot {
            cache: CacheInsight {
                input_tokens: cache.input_tokens,
                cached_input_tokens: cache.cached_input_tokens,
                cache_write_tokens: cache.cache_write_tokens,
                reuse_rate: cache.reuse_rate,
                observed_cost_usd: cache.observed_cost_usd,
                without_cache_usd: cache.without_cache_usd,
                savings_usd: cache.savings_usd,
                priced_calls: cache.priced_calls,
                unpriced_calls: cache.unpriced_calls,
                retry_calls: retries.retry_calls,
                retry_tokens: retries.retry_tokens,
            },
            performance,
            network,
        })
    }

    pub fn export_report(
        &self,
        period: DashboardPeriod,
        anchor: Option<&str>,
        project: Option<&str>,
        account: Option<&str>,
    ) -> Result<PathBuf> {
        let anchor = parse_anchor(anchor)?;
        let (from, to, _) = period_bounds(period, anchor);
        let storage = self.open_storage()?;
        let rows = storage.usage_calls_range(UsageFilter {
            from_date: from.as_deref(),
            to_date: to.as_deref(),
            project,
            account,
        })?;
        storage.close()?;
        let mut csv = String::from(
            "completed_at,project,model,effort,input_tokens,cached_input_tokens,output_tokens,total_tokens,cost_usd,pricing_version,data_source,confidence,estimated\n",
        );
        for row in rows {
            let call = row.call;
            let fields = [
                call.completed_at.unwrap_or_default(),
                row.project_name.unwrap_or_else(|| "Unknown".into()),
                call.actual_model
                    .or(call.model)
                    .unwrap_or_else(|| "Unknown".into()),
                call.reasoning_effort.unwrap_or_else(|| "Unknown".into()),
                call.usage.input_tokens.to_string(),
                call.usage.cached_input_tokens.to_string(),
                call.usage.output_tokens.to_string(),
                call.usage.total_tokens.to_string(),
                call.cost_usd
                    .map(|value| format!("{value:.8}"))
                    .unwrap_or_default(),
                call.pricing_version.unwrap_or_default(),
                call.quality.source,
                call.quality.confidence.as_str().into(),
                i32::from(call.quality.estimated).to_string(),
            ];
            csv.push_str(
                &fields
                    .into_iter()
                    .map(|value| csv_field(&value))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            csv.push('\n');
        }
        let directory = dirs::download_dir().unwrap_or_else(|| self.home.clone());
        fs::create_dir_all(&directory)
            .with_context(|| format!("create export directory {}", directory.display()))?;
        let filename = format!(
            "codex-meter-{}-{}.csv",
            match period {
                DashboardPeriod::Day => "day",
                DashboardPeriod::Week => "week",
                DashboardPeriod::Month => "month",
                DashboardPeriod::All => "all",
            },
            Local::now().format("%Y%m%d-%H%M%S")
        );
        let path = directory.join(filename);
        fs::write(&path, csv).with_context(|| format!("write export {}", path.display()))?;
        Ok(path)
    }

    pub fn refresh_local(&self, force: bool) -> Result<RefreshSummary> {
        let sessions_path = self.codex_home.join("sessions");
        let files = discover_rollouts(&sessions_path)?;
        let mut summary = RefreshSummary {
            discovered_files: files.len(),
            ..Default::default()
        };
        let storage = self.open_storage()?;
        let collector = SessionCollector::new(&self.catalog);
        for rollout in files {
            if !force && storage.file_is_current(&rollout)? {
                summary.skipped_files += 1;
                continue;
            }
            match collector
                .collect_file(&rollout)
                .and_then(|parsed| storage.import_file(&parsed, &rollout))
            {
                Ok(ImportStats(turns, calls, tools)) => {
                    summary.imported_files += 1;
                    summary.inserted_turns += turns;
                    summary.inserted_calls += calls;
                    summary.inserted_tools += tools;
                }
                Err(_) => summary.failed_files += 1,
            }
        }
        storage.close()?;
        Ok(summary)
    }

    pub fn settings(&self) -> DesktopSettings {
        let remote_hosts = config::remote_hosts(&self.home);
        let remote_sources = self
            .open_storage()
            .ok()
            .map(|storage| {
                remote_hosts
                    .iter()
                    .filter_map(|host| storage.remote_source_status(host).ok())
                    .collect()
            })
            .unwrap_or_default();
        DesktopSettings {
            version: env!("CARGO_PKG_VERSION"),
            pricing_catalog_version: self.catalog.catalog_version.clone(),
            pricing_source: self.catalog.source.clone(),
            meter_home: display_path(&self.home),
            database_path: display_path(&self.db_path),
            codex_home: display_path(&self.codex_home),
            sessions_path: display_path(&self.codex_home.join("sessions")),
            owner_username: self.identity.username.clone(),
            account_tracking: self.identity.account_tracking,
            account_label: self.identity.account_label.clone(),
            remote_hosts,
            remote_sources,
            privacy_summary: "Statistics metadata only. Prompts, responses, reasoning text, commands, tool output, headers, and credentials are not stored.",
        }
    }

    pub fn add_remote(&self, host: &str) -> Result<Vec<String>> {
        let host = config::validate_remote_host(host)?;
        let files =
            crate::remote::list(&host).with_context(|| format!("test SSH source {host}"))?;
        let mut hosts = config::remote_hosts(&self.home);
        if !hosts.contains(&host) {
            hosts.push(host);
            config::update_remote_hosts(&self.home, &hosts)?;
        }
        if files.is_empty() {
            // An empty source is valid; the connectivity check above is what
            // matters. Keep it configured for future Codex sessions.
        }
        Ok(config::remote_hosts(&self.home))
    }

    pub fn test_remote(&self, host: &str) -> Result<usize> {
        let host = config::validate_remote_host(host)?;
        let storage = self.open_storage()?;
        storage.record_remote_attempt(&host)?;
        match crate::remote::list(&host) {
            Ok(files) => {
                let previous = storage.remote_source_status(&host)?;
                storage.record_remote_success(
                    &host,
                    files.len(),
                    previous.imported_files.max(0) as usize,
                    previous.skipped_files.max(0) as usize,
                )?;
                storage.close()?;
                Ok(files.len())
            }
            Err(error) => {
                let _ = storage.record_remote_failure(&host, "connection");
                let _ = storage.close();
                Err(error)
            }
        }
    }

    pub fn update_pricing(&self) -> Result<String> {
        let catalog = crate::pricing::update_catalog(
            &self.home.join("pricing.json"),
            std::time::Duration::from_secs(15),
        )?;
        let storage = self.open_storage()?;
        storage.sync_pricing(&catalog)?;
        storage.backfill_unpriced_calls(&catalog)?;
        storage.close()?;
        Ok(catalog.catalog_version)
    }

    pub fn update_account_tracking(
        &self,
        enabled: bool,
        label: Option<&str>,
        claim_existing: bool,
    ) -> Result<DesktopSettings> {
        let label = label.map(str::trim).filter(|value| !value.is_empty());
        if enabled && label.is_none() {
            bail!("enter a short account label before enabling account tracking");
        }
        let identity = config::update_identity(&self.home, enabled, label)?;
        if enabled && claim_existing {
            let storage = Storage::with_identity(
                &self.db_path,
                identity.uid.map(i64::from),
                &identity.username,
                identity.account_label.clone(),
            )?;
            storage.migrate()?;
            storage.claim_unassigned_account(label.unwrap_or_default())?;
            storage.close()?;
        }
        MeterService::open(self.home.clone(), self.codex_home.clone())
            .map(|service| service.settings())
    }

    pub fn remove_remote(&self, host: &str) -> Result<Vec<String>> {
        let host = config::validate_remote_host(host)?;
        let mut hosts = config::remote_hosts(&self.home);
        hosts.retain(|item| item != &host);
        config::update_remote_hosts(&self.home, &hosts)
    }
}

fn parse_anchor(value: Option<&str>) -> Result<NaiveDate> {
    value
        .map(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d"))
        .transpose()
        .context("anchor date must use YYYY-MM-DD")
        .map(|value| value.unwrap_or_else(|| Local::now().date_naive()))
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn average(values: impl Iterator<Item = i64>) -> Option<f64> {
    average_f64(values.map(|value| value as f64))
}

fn average_f64(values: impl Iterator<Item = f64>) -> Option<f64> {
    let values = values.collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn percentile(values: impl Iterator<Item = i64>, percentile: f64) -> Option<f64> {
    let mut values = values.collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let index = ((values.len() - 1) as f64 * percentile).round() as usize;
    Some(values[index] as f64)
}

fn period_bounds(
    period: DashboardPeriod,
    anchor: NaiveDate,
) -> (Option<String>, Option<String>, String) {
    match period {
        DashboardPeriod::All => (None, None, "All time".into()),
        DashboardPeriod::Day => {
            let value = anchor.to_string();
            (Some(value.clone()), Some(value.clone()), "Today".into())
        }
        DashboardPeriod::Week => {
            let start =
                anchor - ChronoDuration::days(i64::from(anchor.weekday().num_days_from_monday()));
            let end = start + ChronoDuration::days(6);
            (
                Some(start.to_string()),
                Some(end.to_string()),
                format!("{start} – {end}"),
            )
        }
        DashboardPeriod::Month => {
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
                start.format("%B %Y").to_string(),
            )
        }
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_service_reuses_storage_and_preserves_privacy_defaults() {
        let meter = tempfile::tempdir().unwrap();
        let codex = tempfile::tempdir().unwrap();
        let service = MeterService::open(meter.path().into(), codex.path().into()).unwrap();
        let snapshot = service.dashboard(DashboardPeriod::Day, None).unwrap();
        assert_eq!(snapshot.overview.total_tokens, 0);
        assert_eq!(snapshot.owner_username, service.identity.username);
        let settings = service.settings();
        assert!(settings.privacy_summary.contains("Prompts"));
        let config = std::fs::read_to_string(meter.path().join("config.toml")).unwrap();
        assert!(config.contains("store_prompt = false"));
    }

    #[test]
    fn desktop_periods_follow_local_calendar_boundaries() {
        let anchor = NaiveDate::from_ymd_opt(2026, 8, 13).unwrap();
        assert_eq!(
            period_bounds(DashboardPeriod::Week, anchor).0.as_deref(),
            Some("2026-08-10")
        );
        assert_eq!(
            period_bounds(DashboardPeriod::Month, anchor).1.as_deref(),
            Some("2026-08-31")
        );
        assert_eq!(period_bounds(DashboardPeriod::All, anchor).0, None);
    }

    #[test]
    fn desktop_account_tracking_is_opt_in_and_requires_a_label() {
        let meter = tempfile::tempdir().unwrap();
        let codex = tempfile::tempdir().unwrap();
        let service = MeterService::open(meter.path().into(), codex.path().into()).unwrap();
        assert!(!service.settings().account_tracking);
        assert!(
            service
                .update_account_tracking(true, Some("  "), false)
                .unwrap_err()
                .to_string()
                .contains("account label")
        );
        let settings = service
            .update_account_tracking(true, Some("Work"), false)
            .unwrap();
        assert!(settings.account_tracking);
        assert_eq!(settings.account_label.as_deref(), Some("Work"));
        let settings = service.update_account_tracking(false, None, false).unwrap();
        assert!(!settings.account_tracking);
        assert_eq!(settings.account_label, None);
    }
}
