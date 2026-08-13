import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

type Period = "day" | "week" | "month" | "all";

interface Overview {
  calls: number;
  sessions: number;
  turns: number;
  inputTokens: number;
  cachedInputTokens: number;
  cacheWriteTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  totalTokens: number;
  costUsd: number | null;
  unpricedCalls: number;
  historicalPriceEstimateCalls: number;
  avgTtftMs: number | null;
  avgE2eMs: number | null;
}

interface ModelUsage {
  model: string;
  effort: string;
  calls: number;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  totalTokens: number;
  costUsd: number | null;
  unpricedCalls: number;
}

interface HistoryBucket {
  periodStart: string;
  calls: number;
  sessions: number;
  turns: number;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  totalTokens: number;
  costUsd: number | null;
}

interface SessionSummary {
  codexThreadId: string;
  projectName: string | null;
  startedAt: string | null;
  endedAt: string | null;
  turns: number;
  calls: number;
  totalTokens: number;
  costUsd: number | null;
  cachedInputTokens: number;
  inputTokens: number;
}

interface DashboardSnapshot {
  generatedAt: string;
  period: Period;
  periodLabel: string;
  project: string | null;
  overview: Overview;
  models: ModelUsage[];
  history: HistoryBucket[];
  recentSessions: SessionSummary[];
  projects: string[];
  remoteCount: number;
  ownerUsername: string;
}

interface WeeklyQuota {
  limitId: string;
  name: string;
  usedPercent: number;
  resetsAt: number | null;
  windowMinutes: number;
  planType: string | null;
}

interface DesktopSettings {
  meterHome: string;
  databasePath: string;
  codexHome: string;
  sessionsPath: string;
  ownerUsername: string;
  accountTracking: boolean;
  accountLabel: string | null;
  remoteHosts: string[];
  privacySummary: string;
}

interface RefreshProgress {
  phase: string;
  message: string;
  host?: string;
  completedFiles?: number;
  totalFiles?: number;
  completedBytes?: number;
  totalBytes?: number;
}

interface RefreshOutcome {
  warnings: string[];
}

const app = document.querySelector<HTMLDivElement>("#app");
if (!app) throw new Error("App root is missing");
const appRoot: HTMLDivElement = app;

const state: {
  period: Period;
  project: string | null;
  snapshot: DashboardSnapshot | null;
  quotas: WeeklyQuota[] | null;
  settings: DesktopSettings | null;
  page: "overview" | "sessions" | "settings";
  busy: boolean;
  notice: string;
  error: string;
  quotaError: string;
} = {
  period: "day",
  project: null,
  snapshot: null,
  quotas: null,
  settings: null,
  page: "overview",
  busy: false,
  notice: "Opening your local usage database…",
  error: "",
  quotaError: "",
};

const icons = {
  overview: `<svg viewBox="0 0 24 24"><path d="M4 13h6V4H4v9Zm0 7h6v-5H4v5Zm10 0h6v-9h-6v9Zm0-16v5h6V4h-6Z"/></svg>`,
  sessions: `<svg viewBox="0 0 24 24"><path d="M4 5h16v11H7l-3 3V5Zm4 4h8M8 12h5"/></svg>`,
  settings: `<svg viewBox="0 0 24 24"><path d="M12 15.2a3.2 3.2 0 1 0 0-6.4 3.2 3.2 0 0 0 0 6.4Zm7-3.2 2-1.1-2-3.5-2.1.7a7 7 0 0 0-1.5-.9L15 5h-4l-.4 2.2a7 7 0 0 0-1.5.9L7 7.4l-2 3.5L7 12a7 7 0 0 0 0 1.8L5 15l2 3.5 2.1-.7a7 7 0 0 0 1.5.9L11 21h4l.4-2.2a7 7 0 0 0 1.5-.9l2.1.7 2-3.5-2-1.1a7 7 0 0 0 0-2Z"/></svg>`,
  refresh: `<svg viewBox="0 0 24 24"><path d="M20 6v5h-5M4 18v-5h5M18.5 9A7 7 0 0 0 6.8 6.8L4 11m16 2-2.8 4.2A7 7 0 0 1 5.5 15"/></svg>`,
};

function escapeHtml(value: unknown): string {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function number(value: number): string {
  const absolute = Math.abs(value);
  if (absolute >= 1e9) return `${(value / 1e9).toFixed(2)}B`;
  if (absolute >= 1e6) return `${(value / 1e6).toFixed(2)}M`;
  if (absolute >= 1e3) return `${(value / 1e3).toFixed(1)}K`;
  return new Intl.NumberFormat().format(value);
}

function money(value: number | null): string {
  return value == null ? "N/A" : `$${value.toFixed(2)}`;
}

function duration(value: number | null): string {
  if (value == null) return "N/A";
  if (value < 1000) return `${Math.round(value)} ms`;
  return `${(value / 1000).toFixed(2)} s`;
}

function percent(part: number, total: number): number {
  return total > 0 ? Math.min(100, Math.max(0, (part / total) * 100)) : 0;
}

function dateLabel(value: string | null): string {
  if (!value) return "Unknown";
  const date = new Date(value);
  return Number.isNaN(date.valueOf())
    ? value.slice(0, 16)
    : date.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

function shell(): string {
  const nav = (page: typeof state.page, label: string, icon: string) => `
    <button class="nav-item ${state.page === page ? "active" : ""}" data-page="${page}">
      ${icon}<span>${label}</span>
    </button>`;
  const pageTitle = state.page === "overview" ? "Overview" : state.page === "sessions" ? "Sessions" : "Settings";
  return `
    <div class="app-shell">
      <aside class="sidebar" data-tauri-drag-region>
        <div class="traffic-space" data-tauri-drag-region></div>
        <div class="brand" data-tauri-drag-region>
          <div class="brand-mark"><span></span><span></span><span></span></div>
          <div><strong>Codex Meter</strong><small>Usage at a glance</small></div>
        </div>
        <div class="nav-label">Workspace</div>
        <nav>
          ${nav("overview", "Overview", icons.overview)}
          ${nav("sessions", "Sessions", icons.sessions)}
          ${nav("settings", "Settings", icons.settings)}
        </nav>
        <div class="sidebar-spacer"></div>
        <div class="privacy-chip"><span></span><div><strong>Private by design</strong><small>Conversation content is never stored</small></div></div>
        <div class="version">Version 0.17.0 beta</div>
      </aside>
      <main class="main-area">
        <header class="topbar" data-tauri-drag-region>
          <div data-tauri-drag-region>
            <span class="eyebrow">Codex Meter</span>
            <h1>${pageTitle}</h1>
          </div>
          <div class="topbar-actions">
            <div class="source-pill"><span></span><div><strong>${state.snapshot ? escapeHtml(state.snapshot.ownerUsername) : "Local user"}</strong><small>${state.snapshot?.remoteCount ? `This Mac + ${state.snapshot.remoteCount} remote` : "This Mac"}</small></div></div>
            <button class="icon-button refresh-button ${state.busy ? "spinning" : ""}" title="Refresh usage" aria-label="Refresh usage" ${state.busy ? "disabled" : ""}>${icons.refresh}</button>
          </div>
        </header>
        <section class="page-content">
          ${state.error ? `<div class="banner error"><strong>Refresh needs attention</strong><span>${escapeHtml(state.error)}</span><button data-dismiss-error>Dismiss</button></div>` : ""}
          ${state.busy || state.notice ? `<div class="progress-line"><span class="progress-dot"></span><span>${escapeHtml(state.notice)}</span></div>` : ""}
          ${content()}
        </section>
      </main>
    </div>`;
}

function content(): string {
  if (!state.snapshot) return skeleton();
  if (state.page === "settings") return settingsView();
  if (state.page === "sessions") return sessionsView();
  return overviewView();
}

function skeleton(): string {
  return `<div class="skeleton-grid">${Array.from({ length: 8 }, (_, i) => `<div class="skeleton s${i}"></div>`).join("")}</div>`;
}

function overviewView(): string {
  const snapshot = state.snapshot!;
  const usage = snapshot.overview;
  const cache = percent(usage.cachedInputTokens, usage.inputTokens);
  const reasoning = percent(usage.reasoningTokens, usage.outputTokens);
  const periodButtons: Array<[Period, string]> = [["day", "Today"], ["week", "Week"], ["month", "Month"], ["all", "All time"]];
  const projectOptions = [`<option value="">All projects</option>`, ...snapshot.projects.map(project => `<option value="${escapeHtml(project)}" ${project === state.project ? "selected" : ""}>${escapeHtml(project)}</option>`)];
  return `
    <div class="toolbar-row">
      <div class="segmented">${periodButtons.map(([value, label]) => `<button data-period="${value}" class="${state.period === value ? "active" : ""}">${label}</button>`).join("")}</div>
      <label class="select-wrap"><span>Project</span><select id="project-select">${projectOptions.join("")}</select></label>
    </div>
    <div class="quota-section">
      <div class="section-heading"><div><span class="section-kicker">Live account</span><h2>Weekly allowance</h2><p>Independent of the date and project filters above</p></div></div>
      <div class="quota-grid">${quotaCards()}</div>
    </div>
    <div class="metric-grid">
      ${metricCard("Tokens", number(usage.totalTokens), `${number(usage.inputTokens)} input · ${number(usage.outputTokens)} output`, "cyan")}
      ${metricCard("API equivalent", money(usage.costUsd), costCoverage(usage), "violet")}
      ${metricCard("Cache efficiency", `${cache.toFixed(1)}%`, `${number(usage.cachedInputTokens)} cached input`, "blue")}
      ${metricCard("Calls", number(usage.calls), `${usage.sessions} sessions · ${usage.turns} turns`, "green")}
    </div>
    <div class="content-grid">
      <article class="panel chart-panel">
        <div class="panel-title"><div><h2>Usage trend</h2><p>Last ${snapshot.history.length} active days</p></div><span>${escapeHtml(snapshot.periodLabel)}</span></div>
        ${historyChart(snapshot.history)}
      </article>
      <article class="panel health-panel">
        <div class="panel-title"><div><h2>Response health</h2><p>Observed timing metadata</p></div></div>
        <div class="health-row"><span>First token</span><strong>${duration(usage.avgTtftMs)}</strong></div>
        <div class="health-row"><span>End to end</span><strong>${duration(usage.avgE2eMs)}</strong></div>
        <div class="ratio"><div><span>Cached input</span><strong>${cache.toFixed(1)}%</strong></div><div class="track"><i style="width:${cache}%"></i></div></div>
        <div class="ratio reasoning"><div><span>Reasoning / output</span><strong>${reasoning.toFixed(1)}%</strong></div><div class="track"><i style="width:${reasoning}%"></i></div></div>
      </article>
      <article class="panel model-panel">
        <div class="panel-title"><div><h2>Models</h2><p>Model and reasoning effort</p></div></div>
        ${modelTable(snapshot.models)}
      </article>
    </div>`;
}

function costCoverage(usage: Overview): string {
  const details: string[] = [];
  if (usage.unpricedCalls) details.push(`${usage.unpricedCalls} missing model prices`);
  if (usage.historicalPriceEstimateCalls) details.push(`${usage.historicalPriceEstimateCalls} historical estimates`);
  return details.length ? details.join(" · ") : "All calls use dated prices";
}

function metricCard(label: string, value: string, detail: string, color: string): string {
  return `<article class="metric-card ${color}"><i class="metric-accent"></i><span>${label}</span><strong>${value}</strong><small>${detail}</small></article>`;
}

function quotaCards(): string {
  if (state.quotas === null) return `<div class="quota-card loading"><div class="shimmer"></div><p>Loading live account limits…</p></div>`;
  if (!state.quotas.length) return `<div class="quota-card unavailable"><strong>Live limits unavailable</strong><p>${state.quotaError ? escapeHtml(state.quotaError) : "Run codex --version to confirm Codex CLI is available, then refresh."}</p></div>`;
  return state.quotas.map(quota => {
    const reset = quota.resetsAt ? new Date(quota.resetsAt * 1000).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }) : "Unknown";
    const remaining = Math.max(0, 100 - quota.usedPercent);
    return `<article class="quota-card">
      <div class="quota-head"><div><span class="status-dot"></span><div><small>7-day limit</small><strong>${escapeHtml(quota.name)}</strong></div></div><b>${remaining}% <small>left</small></b></div>
      <div class="quota-track"><i style="width:${quota.usedPercent}%"></i></div>
      <div class="quota-meta"><span>${quota.usedPercent}% used</span><span>Resets ${reset}</span></div>
    </article>`;
  }).join("");
}

function historyChart(rows: HistoryBucket[]): string {
  if (!rows.length) return `<div class="empty-state"><strong>No activity in this scope yet</strong><span>Refresh after using Codex to populate the chart.</span></div>`;
  const max = Math.max(...rows.map(row => row.totalTokens), 1);
  return `<div class="bar-chart">${rows.map(row => {
    const height = Math.max(4, Math.round((row.totalTokens / max) * 100));
    const label = new Date(`${row.periodStart}T00:00:00`).toLocaleDateString(undefined, { weekday: "short" });
    return `<div class="bar-column" title="${escapeHtml(row.periodStart)} · ${number(row.totalTokens)} tokens"><div class="bar-value">${number(row.totalTokens)}</div><div class="bar-wrap"><i style="height:${height}%"></i></div><span>${label}</span></div>`;
  }).join("")}</div>`;
}

function modelTable(rows: ModelUsage[]): string {
  if (!rows.length) return `<div class="empty-state compact"><strong>No model usage</strong><span>There are no calls in the selected scope.</span></div>`;
  return `<div class="table-wrap"><table><thead><tr><th>Model</th><th>Effort</th><th class="right">Calls</th><th class="right">Tokens</th><th class="right">Cache</th><th class="right">Cost</th></tr></thead><tbody>${rows.slice(0, 8).map(row => `<tr><td><span class="model-dot"></span>${escapeHtml(row.model)}</td><td><span class="pill">${escapeHtml(row.effort)}</span></td><td class="right">${number(row.calls)}</td><td class="right">${number(row.totalTokens)}</td><td class="right">${percent(row.cachedInputTokens, row.inputTokens).toFixed(1)}%</td><td class="right">${money(row.costUsd)}</td></tr>`).join("")}</tbody></table></div>`;
}

function sessionsView(): string {
  const sessions = state.snapshot!.recentSessions;
  return `<article class="panel sessions-panel"><div class="panel-title"><div><h2>Recent sessions</h2><p>The newest Codex sessions owned by this macOS user</p></div></div>${sessions.length ? `<div class="session-list">${sessions.map(session => {
    const cache = percent(session.cachedInputTokens, session.inputTokens);
    return `<div class="session-row"><div class="session-icon">${icons.sessions}</div><div class="session-main"><strong>${escapeHtml(session.projectName ?? "Unknown project")}</strong><span>${dateLabel(session.startedAt)} · ${session.turns} turns · ${session.calls} calls</span></div><div class="session-stat"><strong>${number(session.totalTokens)}</strong><span>tokens</span></div><div class="session-stat"><strong>${cache.toFixed(1)}%</strong><span>cache</span></div><div class="session-stat"><strong>${money(session.costUsd)}</strong><span>API equiv.</span></div></div>`;
  }).join("")}</div>` : `<div class="empty-state"><strong>No sessions imported</strong><span>Use Codex, then click Refresh.</span></div>`}</article>`;
}

function settingsView(): string {
  const settings = state.settings;
  if (!settings) return skeleton();
  return `<div class="settings-grid">
    <article class="panel settings-card"><div class="panel-title"><div><h2>Data locations</h2><p>Shared by the desktop app and CLI</p></div></div>
      ${settingRow("Meter home", settings.meterHome)}
      ${settingRow("Database", settings.databasePath)}
      ${settingRow("Codex home", settings.codexHome)}
      ${settingRow("Rollouts", settings.sessionsPath)}
    </article>
    <article class="panel settings-card"><div class="panel-title"><div><h2>Privacy</h2><p>Local-first by design</p></div><span class="safe-badge">Protected</span></div>
      <div class="privacy-note"><span class="shield">✓</span><p>${escapeHtml(settings.privacySummary)}</p></div>
      ${settingRow("OS user", settings.ownerUsername)}
      ${settingRow("Account labels", settings.accountTracking ? settings.accountLabel ?? "Enabled" : "Off by default")}
    </article>
    <article class="panel settings-card remote-card"><div class="panel-title"><div><h2>Remote servers</h2><p>SSH aliases whose metadata is shown on this Mac</p></div></div>
      <div class="remote-list">${settings.remoteHosts.length ? settings.remoteHosts.map(host => `<div class="remote-row"><span class="status-dot"></span><strong>${escapeHtml(host)}</strong><button data-remove-remote="${escapeHtml(host)}">Remove</button></div>`).join("") : `<div class="empty-inline">No remote servers configured.</div>`}</div>
      <form id="remote-form"><input name="host" autocomplete="off" placeholder="SSH alias, e.g. devbox" /><button type="submit">Add server</button></form>
      <small class="form-help">The alias must already work with <code>ssh alias</code>. Conversation content is not stored locally.</small>
    </article>
  </div>`;
}

function settingRow(label: string, value: string): string {
  return `<div class="setting-row"><span>${label}</span><code title="${escapeHtml(value)}">${escapeHtml(value)}</code></div>`;
}

function render(): void {
  appRoot.innerHTML = shell();
  bindEvents();
}

function bindEvents(): void {
  document.querySelectorAll<HTMLElement>("[data-page]").forEach(button => button.addEventListener("click", async () => {
    state.page = button.dataset.page as typeof state.page;
    render();
    if (state.page === "settings" && !state.settings) await loadSettings();
  }));
  document.querySelectorAll<HTMLElement>("[data-period]").forEach(button => button.addEventListener("click", async () => {
    state.period = button.dataset.period as Period;
    await loadDashboard();
  }));
  document.querySelector<HTMLSelectElement>("#project-select")?.addEventListener("change", async event => {
    const value = (event.target as HTMLSelectElement).value;
    state.project = value || null;
    await loadDashboard();
  });
  document.querySelector(".refresh-button")?.addEventListener("click", () => refreshAll());
  document.querySelector("[data-dismiss-error]")?.addEventListener("click", () => { state.error = ""; render(); });
  document.querySelector<HTMLFormElement>("#remote-form")?.addEventListener("submit", async event => {
    event.preventDefault();
    const data = new FormData(event.currentTarget as HTMLFormElement);
    const host = String(data.get("host") ?? "").trim();
    if (!host) return;
    state.busy = true; state.notice = `Testing SSH connection to ${host}…`; render();
    try {
      await invoke("add_remote", { host });
      await loadSettings(false);
      state.busy = false;
      state.notice = `${host} was added. Starting its first metadata sync…`;
      render();
      await refreshAll();
      return;
    } catch (error) { state.error = String(error); state.notice = ""; }
    state.busy = false;
    render();
  });
  document.querySelectorAll<HTMLElement>("[data-remove-remote]").forEach(button => button.addEventListener("click", async () => {
    const host = button.dataset.removeRemote!;
    try {
      await invoke("remove_remote", { host });
      await loadSettings(false);
      await loadDashboard(false);
    } catch (error) { state.error = String(error); }
    render();
  }));
}

async function loadDashboard(showProgress = true): Promise<void> {
  if (showProgress) { state.busy = true; state.notice = "Loading dashboard…"; render(); }
  try {
    state.snapshot = await invoke<DashboardSnapshot>("load_dashboard", { period: state.period, project: state.project });
    state.error = "";
  } catch (error) { state.error = String(error); }
  if (showProgress) { state.busy = false; state.notice = ""; render(); }
}

async function loadSettings(renderAfter = true): Promise<void> {
  try { state.settings = await invoke<DesktopSettings>("load_settings"); }
  catch (error) { state.error = String(error); }
  if (renderAfter) render();
}

async function loadQuotas(): Promise<void> {
  try {
    state.quotas = await invoke<WeeklyQuota[]>("load_quotas");
    state.quotaError = "";
  }
  catch (error) {
    state.quotas = [];
    state.quotaError = String(error);
  }
  render();
}

async function refreshAll(initial = false): Promise<void> {
  if (state.busy) return;
  state.busy = true;
  state.notice = initial ? "Scanning changed local Rollouts…" : "Refreshing local and remote metadata…";
  state.error = "";
  render();
  const quotaPromise = loadQuotas();
  try {
    const outcome = await invoke<RefreshOutcome>("refresh_all", { force: false });
    await Promise.all([loadDashboard(false), loadSettings(false), quotaPromise]);
    state.error = outcome.warnings.join(" · ");
    state.notice = outcome.warnings.length ? "Local usage is up to date; one or more remote sources need attention." : "Usage is up to date.";
    window.setTimeout(() => { if (!state.busy) { state.notice = ""; render(); } }, 2400);
  } catch (error) {
    await quotaPromise;
    state.error = String(error);
    state.notice = "";
  }
  state.busy = false;
  render();
}

await listen<RefreshProgress>("refresh-progress", event => {
  const progress = event.payload;
  state.notice = progress.message;
  render();
});

render();
await Promise.all([loadDashboard(false), loadSettings(false)]);
render();
void refreshAll(true);
