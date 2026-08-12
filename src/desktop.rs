//! Structured application service used by the macOS desktop frontend.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::collector::{SessionCollector, discover_rollouts};
use crate::config::{self, LocalIdentity};
use crate::pricing::PricingCatalog;
use crate::storage::{
    HistoryBucket, ImportStats, ModelUsage, Overview, SessionSummary, Storage, UsageFilter,
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
    pub period: DashboardPeriod,
    pub period_label: String,
    pub project: Option<String>,
    pub overview: Overview,
    pub models: Vec<ModelUsage>,
    pub history: Vec<HistoryBucket>,
    pub recent_sessions: Vec<SessionSummary>,
    pub projects: Vec<String>,
    pub remote_count: usize,
    pub owner_username: String,
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
    pub meter_home: String,
    pub database_path: String,
    pub codex_home: String,
    pub sessions_path: String,
    pub owner_username: String,
    pub account_tracking: bool,
    pub account_label: Option<String>,
    pub remote_hosts: Vec<String>,
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
        let catalog = PricingCatalog::from_path(&home.join("pricing.json"))
            .or_else(|_| PricingCatalog::bundled())?;
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
        Ok(storage)
    }

    pub fn dashboard(
        &self,
        period: DashboardPeriod,
        project: Option<&str>,
    ) -> Result<DashboardSnapshot> {
        let storage = self.open_storage()?;
        let (from, to, period_label) = period_bounds(period, Local::now().date_naive());
        let filter = UsageFilter {
            from_date: from.as_deref(),
            to_date: to.as_deref(),
            project,
            ..Default::default()
        };
        let overview = storage.overview_range(filter)?;
        let models = storage.model_breakdown_range(filter)?;
        let history = storage.usage_history("day", None, project)?;
        let recent_sessions = storage.sessions(8)?;
        let projects = storage.project_names()?;
        storage.close()?;
        Ok(DashboardSnapshot {
            generated_at: chrono::Utc::now().to_rfc3339(),
            period,
            period_label,
            project: project.map(str::to_owned),
            overview,
            models,
            history: history
                .into_iter()
                .rev()
                .take(14)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            recent_sessions,
            projects,
            remote_count: config::remote_hosts(&self.home).len(),
            owner_username: self.identity.username.clone(),
        })
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
        DesktopSettings {
            meter_home: display_path(&self.home),
            database_path: display_path(&self.db_path),
            codex_home: display_path(&self.codex_home),
            sessions_path: display_path(&self.codex_home.join("sessions")),
            owner_username: self.identity.username.clone(),
            account_tracking: self.identity.account_tracking,
            account_label: self.identity.account_label.clone(),
            remote_hosts: config::remote_hosts(&self.home),
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

    pub fn remove_remote(&self, host: &str) -> Result<Vec<String>> {
        let host = config::validate_remote_host(host)?;
        let mut hosts = config::remote_hosts(&self.home);
        hosts.retain(|item| item != &host);
        config::update_remote_hosts(&self.home, &hosts)
    }
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
}
