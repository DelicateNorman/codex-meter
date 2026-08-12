use codex_meter::desktop::{DashboardPeriod, DashboardSnapshot, DesktopSettings, MeterService};
use codex_meter::quota::WeeklyQuota;
use serde::Serialize;
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
}

fn emit(app: &AppHandle, progress: RefreshProgress) {
    let _ = app.emit("refresh-progress", progress);
}

#[tauri::command]
async fn load_dashboard(
    period: DashboardPeriod,
    project: Option<String>,
) -> CommandResult<DashboardSnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        MeterService::open_default()
            .and_then(|service| service.dashboard(period, project.as_deref()))
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
            return Ok(RefreshOutcome { warnings });
        }
        let mut storage = service
            .open_storage()
            .map_err(|error| format!("{error:#}"))?;
        for host in hosts {
            let progress_app = app.clone();
            let result = codex_meter::remote::sync_with_progress(
                &mut storage,
                &service.catalog,
                &host,
                force,
                move |progress| {
                    let percent = if progress.total_source_bytes == 0 {
                        100
                    } else {
                        progress.completed_source_bytes.saturating_mul(100)
                            / progress.total_source_bytes
                    };
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
            );
            if let Err(error) = result {
                let warning = format!("{host}: {error:#}");
                emit(
                    &app,
                    RefreshProgress {
                        phase: "remote-error",
                        message: format!(
                            "{host} could not be refreshed; local data is still ready"
                        ),
                        host: Some(host),
                        completed_files: None,
                        total_files: None,
                        completed_bytes: None,
                        total_bytes: None,
                    },
                );
                warnings.push(warning);
            }
        }
        storage.close().map_err(|error| format!("{error:#}"))?;
        Ok(RefreshOutcome { warnings })
    })
    .await
    .map_err(|error| error.to_string())?
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            load_dashboard,
            load_quotas,
            load_settings,
            refresh_all,
            add_remote,
            remove_remote,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Codex Meter Desktop");
}
