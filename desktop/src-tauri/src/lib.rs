use codex_meter::desktop::{
    DashboardPeriod, DashboardSnapshot, DesktopSettings, InsightsSnapshot, MeterService,
};
use codex_meter::quota::WeeklyQuota;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};

type CommandResult<T> = Result<T, String>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshProgress {
    phase: &'static str,
    message: String,
    host: Option<String>,
    completed_files: Option<usize>,
    total_files: Option<usize>,
    completed_bytes: Option<u64>,
    total_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshOutcome {
    warnings: Vec<String>,
    cancelled: bool,
}

static CANCEL_REFRESH: AtomicBool = AtomicBool::new(false);

fn emit(app: &AppHandle, progress: RefreshProgress) {
    let _ = app.emit("refresh-progress", progress);
}

#[tauri::command]
async fn load_dashboard(
    period: DashboardPeriod,
    anchor: Option<String>,
    project: Option<String>,
    account: Option<String>,
) -> CommandResult<DashboardSnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        MeterService::open_default()
            .and_then(|service| {
                service.dashboard_filtered(
                    period,
                    anchor.as_deref(),
                    project.as_deref(),
                    account.as_deref(),
                )
            })
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn load_insights(
    period: DashboardPeriod,
    anchor: Option<String>,
    project: Option<String>,
    account: Option<String>,
) -> CommandResult<InsightsSnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        MeterService::open_default()
            .and_then(|service| {
                service.insights(
                    period,
                    anchor.as_deref(),
                    project.as_deref(),
                    account.as_deref(),
                )
            })
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn load_quotas() -> CommandResult<Vec<WeeklyQuota>> {
    tauri::async_runtime::spawn_blocking(|| {
        codex_meter::quota::read_default_weekly_quotas(std::time::Duration::from_secs(8))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn load_settings() -> CommandResult<DesktopSettings> {
    tauri::async_runtime::spawn_blocking(|| {
        MeterService::open_default()
            .map(|service| service.settings())
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn refresh_all(app: AppHandle, force: bool) -> CommandResult<RefreshOutcome> {
    CANCEL_REFRESH.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        let mut warnings = Vec::new();
        emit(
            &app,
            RefreshProgress {
                phase: "local",
                message: "Scanning changed Rollout files on this Mac…".into(),
                host: None,
                completed_files: None,
                total_files: None,
                completed_bytes: None,
                total_bytes: None,
            },
        );
        let service = MeterService::open_default().map_err(|error| format!("{error:#}"))?;
        let local = service
            .refresh_local(force)
            .map_err(|error| format!("{error:#}"))?;
        emit(
            &app,
            RefreshProgress {
                phase: "local",
                message: format!(
                    "Local history ready · {} imported · {} unchanged · {} skipped with errors",
                    local.imported_files, local.skipped_files, local.failed_files
                ),
                host: None,
                completed_files: Some(local.imported_files + local.skipped_files),
                total_files: Some(local.discovered_files),
                completed_bytes: None,
                total_bytes: None,
            },
        );
        if local.failed_files > 0 {
            warnings.push(format!(
                "{} local Rollout file(s) could not be imported",
                local.failed_files
            ));
        }

        let hosts = codex_meter::config::remote_hosts(&service.home);
        if hosts.is_empty() {
            return Ok(RefreshOutcome {
                warnings,
                cancelled: false,
            });
        }
        let results = std::thread::scope(|scope| {
            hosts
                .into_iter()
                .map(|host| {
                    let service = service.clone();
                    let progress_app = app.clone();
                    scope.spawn(move || {
                        let result = (|| -> Result<(), String> {
                            let mut storage = service
                                .open_storage()
                                .map_err(|error| format!("{error:#}"))?;
                            let sync = codex_meter::remote::sync_with_progress_until(
                                &mut storage,
                                &service.catalog,
                                &host,
                                force,
                                move |progress| {
                                    let percent = progress
                                        .completed_source_bytes
                                        .saturating_mul(100)
                                        .checked_div(progress.total_source_bytes)
                                        .unwrap_or(100);
                                    emit(
                                        &progress_app,
                                        RefreshProgress {
                                            phase: "remote",
                                            message: format!(
                                                "{} · metadata {} / {} · {}%",
                                                progress.host,
                                                progress.completed_files,
                                                progress.total_files,
                                                percent.min(100)
                                            ),
                                            host: Some(progress.host.clone()),
                                            completed_files: Some(progress.completed_files),
                                            total_files: Some(progress.total_files),
                                            completed_bytes: Some(progress.completed_source_bytes),
                                            total_bytes: Some(progress.total_source_bytes),
                                        },
                                    );
                                },
                                || CANCEL_REFRESH.load(Ordering::Relaxed),
                            )
                            .map_err(|error| format!("{error:#}"));
                            let close = storage.close().map_err(|error| format!("{error:#}"));
                            sync.and(close)
                        })();
                        (host, result)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join())
                .collect::<Vec<_>>()
        });
        for result in results {
            match result {
                Ok((_, Ok(()))) => {}
                Ok((host, Err(error))) if error.contains("cancelled") => {
                    emit(
                        &app,
                        RefreshProgress {
                            phase: "cancelled",
                            message: format!("{host} sync cancelled; existing data was preserved"),
                            host: Some(host),
                            completed_files: None,
                            total_files: None,
                            completed_bytes: None,
                            total_bytes: None,
                        },
                    );
                }
                Ok((host, Err(error))) => {
                    emit(
                        &app,
                        RefreshProgress {
                            phase: "remote-error",
                            message: format!(
                                "{host} could not be refreshed; local data is still ready"
                            ),
                            host: Some(host.clone()),
                            completed_files: None,
                            total_files: None,
                            completed_bytes: None,
                            total_bytes: None,
                        },
                    );
                    warnings.push(format!("{host}: {error}"));
                }
                Err(_) => warnings.push("A remote sync worker stopped unexpectedly".into()),
            }
        }
        Ok(RefreshOutcome {
            warnings,
            cancelled: CANCEL_REFRESH.load(Ordering::Relaxed),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn cancel_refresh() {
    CANCEL_REFRESH.store(true, Ordering::Relaxed);
}

#[tauri::command]
async fn add_remote(host: String) -> CommandResult<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        MeterService::open_default()
            .and_then(|service| service.add_remote(&host))
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn remove_remote(host: String) -> CommandResult<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        MeterService::open_default()
            .and_then(|service| service.remove_remote(&host))
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn test_remote(host: String) -> CommandResult<usize> {
    tauri::async_runtime::spawn_blocking(move || {
        MeterService::open_default()
            .and_then(|service| service.test_remote(&host))
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn refresh_remote(app: AppHandle, host: String) -> CommandResult<()> {
    CANCEL_REFRESH.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        let service = MeterService::open_default().map_err(|error| format!("{error:#}"))?;
        let mut storage = service
            .open_storage()
            .map_err(|error| format!("{error:#}"))?;
        let progress_app = app.clone();
        codex_meter::remote::sync_with_progress_until(
            &mut storage,
            &service.catalog,
            &host,
            false,
            move |progress| {
                let percent = progress
                    .completed_source_bytes
                    .saturating_mul(100)
                    .checked_div(progress.total_source_bytes)
                    .unwrap_or(100);
                emit(
                    &progress_app,
                    RefreshProgress {
                        phase: "remote",
                        message: format!(
                            "{} · metadata {} / {} · {}%",
                            progress.host,
                            progress.completed_files,
                            progress.total_files,
                            percent.min(100)
                        ),
                        host: Some(progress.host.clone()),
                        completed_files: Some(progress.completed_files),
                        total_files: Some(progress.total_files),
                        completed_bytes: Some(progress.completed_source_bytes),
                        total_bytes: Some(progress.total_source_bytes),
                    },
                );
            },
            || CANCEL_REFRESH.load(Ordering::Relaxed),
        )
        .map_err(|error| format!("{error:#}"))?;
        storage.close().map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn export_report(
    period: DashboardPeriod,
    anchor: Option<String>,
    project: Option<String>,
    account: Option<String>,
) -> CommandResult<String> {
    tauri::async_runtime::spawn_blocking(move || {
        MeterService::open_default()
            .and_then(|service| {
                service.export_report(
                    period,
                    anchor.as_deref(),
                    project.as_deref(),
                    account.as_deref(),
                )
            })
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn update_pricing() -> CommandResult<String> {
    tauri::async_runtime::spawn_blocking(move || {
        MeterService::open_default()
            .and_then(|service| service.update_pricing())
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn update_account_tracking(
    enabled: bool,
    label: Option<String>,
    claim_existing: bool,
) -> CommandResult<DesktopSettings> {
    tauri::async_runtime::spawn_blocking(move || {
        MeterService::open_default()
            .and_then(|service| {
                service.update_account_tracking(enabled, label.as_deref(), claim_existing)
            })
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            load_dashboard,
            load_insights,
            load_quotas,
            load_settings,
            refresh_all,
            cancel_refresh,
            add_remote,
            remove_remote,
            test_remote,
            refresh_remote,
            export_report,
            update_pricing,
            update_account_tracking,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Codex Meter Desktop");
}
