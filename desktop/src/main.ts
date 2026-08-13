import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import "./styles.css";

declare global {
  interface Window {
    __CODEX_METER_TEST__?: {
      invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
      listen?: <T>(event: string, handler: (event: { payload: T }) => void) => Promise<() => void>;
    };
  }
}

function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return window.__CODEX_METER_TEST__
    ? window.__CODEX_METER_TEST__.invoke<T>(command, args)
    : tauriInvoke<T>(command, args);
}

function listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<() => void> {
  if (window.__CODEX_METER_TEST__?.listen) return window.__CODEX_METER_TEST__.listen(event, handler);
  return tauriListen<T>(event, received => handler({ payload: received.payload }));
}

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
  missingModelCalls: number;
  unpublishedPriceCalls: number;
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
  anchorDate: string;
  period: Period;
  periodLabel: string;
  project: string | null;
  account: string | null;
  overview: Overview;
  models: ModelUsage[];
  history: HistoryBucket[];
  weeklyHistory: HistoryBucket[];
  monthlyHistory: HistoryBucket[];
  recentSessions: SessionSummary[];
  projects: string[];
  accounts: string[];
  remoteCount: number;
  ownerUsername: string;
}

interface CacheInsight {
  inputTokens: number;
  cachedInputTokens: number;
  cacheWriteTokens: number;
  reuseRate: number;
  observedCostUsd: number;
  withoutCacheUsd: number;
  savingsUsd: number;
  pricedCalls: number;
  unpricedCalls: number;
  retryCalls: number;
  retryTokens: number;
}

interface ResponsePerformance {
  completedAt: string | null;
  localTime: string | null;
  model: string;
  outputTokens: number;
  ttftMs: number | null;
  e2eMs: number | null;
  exactOutputTps: number | null;
}

interface PerformanceInsight {
  samples: number;
  averageTtftMs: number | null;
  p95TtftMs: number | null;
  averageE2eMs: number | null;
  p95E2eMs: number | null;
  averageOutputTps: number | null;
  recent: ResponsePerformance[];
}

interface NetworkInsight {
  startedAt: string | null;
  mode: string;
  destination: string;
  durationMs: number | null;
  ttfbMs: number | null;
  requestBytes: number;
  responseBytes: number;
  success: boolean | null;
  errorType: string | null;
}

interface InsightsSnapshot {
  cache: CacheInsight;
  performance: PerformanceInsight;
  network: NetworkInsight[];
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
  version: string;
  pricingCatalogVersion: string;
  pricingSource: string;
  meterHome: string;
  databasePath: string;
  codexHome: string;
  sessionsPath: string;
  ownerUsername: string;
  accountTracking: boolean;
  accountLabel: string | null;
  remoteHosts: string[];
  remoteSources: Array<{ host: string; lastAttemptAt: string | null; lastSuccessAt: string | null; lastErrorKind: string | null; discoveredFiles: number; importedFiles: number; skippedFiles: number }>;
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
  cancelled: boolean;
}

const app = document.querySelector<HTMLDivElement>("#app");
if (!app) throw new Error("App root is missing");
const appRoot: HTMLDivElement = app;

const state: {
  period: Period;
  anchor: string;
  project: string | null;
  account: string | null;
  snapshot: DashboardSnapshot | null;
  insights: InsightsSnapshot | null;
  quotas: WeeklyQuota[] | null;
  settings: DesktopSettings | null;
  page: "overview" | "history" | "insights" | "sessions" | "settings";
  historyGroup: "day" | "week" | "month";
  busy: boolean;
  notice: string;
  error: string;
  quotaError: string;
} = {
  period: "day",
  anchor: new Date().toISOString().slice(0, 10),
  project: null,
  account: null,
  snapshot: null,
  insights: null,
  quotas: null,
  settings: null,
  page: "overview",
  historyGroup: "day",
  busy: false,
  notice: "Opening your local usage database…",
  error: "",
  quotaError: "",
};

const icons = {
  overview: `<svg viewBox="0 0 24 24"><path d="M4 13h6V4H4v9Zm0 7h6v-5H4v5Zm10 0h6v-9h-6v9Zm0-16v5h6V4h-6Z"/></svg>`,
  sessions: `<svg viewBox="0 0 24 24"><path d="M4 5h16v11H7l-3 3V5Zm4 4h8M8 12h5"/></svg>`,
  history: `<svg viewBox="0 0 24 24"><path d="M4 18V9m5 9V5m5 13v-7m5 7V3M3 21h18"/></svg>`,
  insights: `<svg viewBox="0 0 24 24"><path d="M4 15l4-4 4 3 7-8M4 20h16"/></svg>`,
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
  const pageTitles = { overview: "Overview", history: "History", insights: "Insights", sessions: "Sessions", settings: "Settings" } as const;
  const pageTitle = pageTitles[state.page];
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
          ${nav("history", "History", icons.history)}
          ${nav("insights", "Insights", icons.insights)}
          ${nav("sessions", "Sessions", icons.sessions)}
          ${nav("settings", "Settings", icons.settings)}
        </nav>
        <div class="sidebar-spacer"></div>
        <div class="privacy-chip"><span></span><div><strong>Private by design</strong><small>Conversation content is never stored</small></div></div>
        <div class="version">Version ${escapeHtml(state.settings?.version ?? "0.17 beta")}</div>
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
          ${state.busy || state.notice ? `<div class="progress-line"><span class="progress-dot"></span><span>${escapeHtml(state.notice)}</span>${state.busy && state.notice.includes("metadata") ? `<button data-cancel-refresh>Cancel</button>` : ""}</div>` : ""}
          ${content()}
        </section>
      </main>
    </div>`;
}

function content(): string {
  if (!state.snapshot) return skeleton();
  if (state.page === "settings") return settingsView();
  if (state.page === "sessions") return sessionsView();
  if (state.page === "history") return historyView();
  if (state.page === "insights") return insightsView();
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
  return `
    ${scopeToolbar()}
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
        ${historyChart(snapshot.history.slice(-14))}
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

function scopeToolbar(includePeriod = true, includeDate = true): string {
  const snapshot = state.snapshot!;
  const periodButtons: Array<[Period, string]> = [["day", "Day"], ["week", "Week"], ["month", "Month"], ["all", "All time"]];
  const projectOptions = [`<option value="">All projects</option>`, ...snapshot.projects.map(project => `<option value="${escapeHtml(project)}" ${project === state.project ? "selected" : ""}>${escapeHtml(project)}</option>`)];
  const accountOptions = [`<option value="">All account labels</option>`, ...snapshot.accounts.map(account => `<option value="${escapeHtml(account)}" ${account === state.account ? "selected" : ""}>${escapeHtml(account)}</option>`)];
  return `<div class="toolbar-row scope-toolbar">
    ${includePeriod ? `<div class="segmented">${periodButtons.map(([value, label]) => `<button data-period="${value}" class="${state.period === value ? "active" : ""}">${label}</button>`).join("")}</div>` : ""}
    ${includeDate ? `<div class="date-stepper ${state.period === "all" ? "disabled" : ""}">
      <button data-date-step="-1" aria-label="Previous period" ${state.period === "all" ? "disabled" : ""}>‹</button>
      <input id="anchor-date" type="date" value="${escapeHtml(state.anchor)}" ${state.period === "all" ? "disabled" : ""} aria-label="Report date" />
      <button data-date-step="1" aria-label="Next period" ${state.period === "all" ? "disabled" : ""}>›</button>
    </div>` : ""}
    <label class="select-wrap"><span>Project</span><select id="project-select">${projectOptions.join("")}</select></label>
    <label class="select-wrap"><span>Account</span><select id="account-select">${accountOptions.join("")}</select></label>
    <button class="export-button" data-export title="Export this filtered report">Export</button>
  </div>`;
}

function costCoverage(usage: Overview): string {
  const details: string[] = [];
  if (usage.unpublishedPriceCalls) details.push(`${usage.unpublishedPriceCalls} without a published price`);
  if (usage.missingModelCalls) details.push(`${usage.missingModelCalls} missing model metadata`);
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

function historyView(): string {
  const snapshot = state.snapshot!;
  const rows = state.historyGroup === "day" ? snapshot.history : state.historyGroup === "week" ? snapshot.weeklyHistory : snapshot.monthlyHistory;
  const shown = rows.slice(-18);
  return `<div class="history-page">
    ${scopeToolbar(false, false)}
    <article class="panel history-panel">
      <div class="panel-title"><div><h2>Usage over time</h2><p>Calendar buckets since first use · newest activity last</p></div>
        <div class="segmented compact-segmented">${(["day", "week", "month"] as const).map(group => `<button data-history-group="${group}" class="${state.historyGroup === group ? "active" : ""}">${group.charAt(0).toUpperCase() + group.slice(1)}</button>`).join("")}</div>
      </div>
      ${historyChart(shown)}
      ${rows.length ? `<div class="table-wrap history-table"><table><thead><tr><th>Period</th><th class="right">Sessions</th><th class="right">Calls</th><th class="right">Tokens</th><th class="right">Cache</th><th class="right">API equivalent</th></tr></thead><tbody>${[...rows].reverse().map(row => `<tr><td>${escapeHtml(row.periodStart)}</td><td class="right">${number(row.sessions)}</td><td class="right">${number(row.calls)}</td><td class="right">${number(row.totalTokens)}</td><td class="right">${percent(row.cachedInputTokens, row.inputTokens).toFixed(1)}%</td><td class="right">${money(row.costUsd)}</td></tr>`).join("")}</tbody></table></div>` : ""}
    </article>
  </div>`;
}

function insightsView(): string {
  if (!state.insights) return `${scopeToolbar()}${skeleton()}`;
  const { cache, performance, network } = state.insights;
  const reuse = cache.reuseRate * 100;
  return `<div class="insights-page">
    ${scopeToolbar()}
    <div class="metric-grid insight-metrics">
      ${metricCard("Cache reuse", `${reuse.toFixed(1)}%`, `${number(cache.cachedInputTokens)} reused input`, "blue")}
      ${metricCard("Estimated savings", money(cache.savingsUsd), `${money(cache.withoutCacheUsd)} without cache`, "green")}
      ${metricCard("First token P95", duration(performance.p95TtftMs), `${duration(performance.averageTtftMs)} average`, "cyan")}
      ${metricCard("Output speed", performance.averageOutputTps == null ? "N/A" : `${performance.averageOutputTps.toFixed(1)} tok/s`, `${performance.samples} timed responses`, "violet")}
    </div>
    <div class="content-grid insight-grid">
      <article class="panel health-panel"><div class="panel-title"><div><h2>Performance</h2><p>Observed response timing for this scope</p></div></div>
        <div class="health-row"><span>First token average</span><strong>${duration(performance.averageTtftMs)}</strong></div>
        <div class="health-row"><span>First token P95</span><strong>${duration(performance.p95TtftMs)}</strong></div>
        <div class="health-row"><span>End to end average</span><strong>${duration(performance.averageE2eMs)}</strong></div>
        <div class="health-row"><span>End to end P95</span><strong>${duration(performance.p95E2eMs)}</strong></div>
      </article>
      <article class="panel health-panel"><div class="panel-title"><div><h2>Cache & retries</h2><p>API-equivalent impact</p></div></div>
        <div class="health-row"><span>Observed equivalent</span><strong>${money(cache.observedCostUsd)}</strong></div>
        <div class="health-row"><span>Without cached input</span><strong>${money(cache.withoutCacheUsd)}</strong></div>
        <div class="health-row"><span>Retry calls</span><strong>${number(cache.retryCalls)}</strong></div>
        <div class="health-row"><span>Retry tokens</span><strong>${number(cache.retryTokens)}</strong></div>
      </article>
      <article class="panel network-panel"><div class="panel-title"><div><h2>Network observations</h2><p>Metadata only · packet contents are never stored</p></div><span>${network.length} recent</span></div>
        ${network.length ? `<div class="table-wrap"><table><thead><tr><th>Destination</th><th>Mode</th><th class="right">TTFB</th><th class="right">Duration</th><th class="right">Transfer</th><th>Status</th></tr></thead><tbody>${network.map(row => `<tr><td>${escapeHtml(row.destination)}</td><td><span class="pill">${escapeHtml(row.mode)}</span></td><td class="right">${duration(row.ttfbMs)}</td><td class="right">${duration(row.durationMs)}</td><td class="right">${bytes(row.requestBytes + row.responseBytes)}</td><td><span class="status-label ${row.success === false ? "failed" : row.success === true ? "ok" : "unknown"}">${row.success === false ? escapeHtml(row.errorType ?? "Failed") : row.success === true ? "OK" : "Unknown"}</span></td></tr>`).join("")}</tbody></table></div>` : `<div class="empty-state compact"><strong>No network metadata yet</strong><span>Use the CLI Network diagnostics to record DNS, TCP, TLS and TTFB timing.</span></div>`}
      </article>
    </div>
  </div>`;
}

function bytes(value: number): string {
  if (value >= 1e9) return `${(value / 1e9).toFixed(1)} GB`;
  if (value >= 1e6) return `${(value / 1e6).toFixed(1)} MB`;
  if (value >= 1e3) return `${(value / 1e3).toFixed(1)} KB`;
  return `${value} B`;
}

function sessionsView(): string {
  const sessions = state.snapshot!.recentSessions;
  return `<div class="sessions-page">${scopeToolbar()}<article class="panel sessions-panel"><div class="panel-title"><div><h2>Recent sessions</h2><p>The newest sessions in the selected date, project and account scope</p></div></div>${sessions.length ? `<div class="session-list">${sessions.map(session => {
    const cache = percent(session.cachedInputTokens, session.inputTokens);
    return `<div class="session-row"><div class="session-icon">${icons.sessions}</div><div class="session-main"><strong>${escapeHtml(session.projectName ?? "Unknown project")}</strong><span>${dateLabel(session.startedAt)} · ${session.turns} turns · ${session.calls} calls</span></div><div class="session-stat"><strong>${number(session.totalTokens)}</strong><span>tokens</span></div><div class="session-stat"><strong>${cache.toFixed(1)}%</strong><span>cache</span></div><div class="session-stat"><strong>${money(session.costUsd)}</strong><span>API equiv.</span></div></div>`;
  }).join("")}</div>` : `<div class="empty-state"><strong>No sessions in this scope</strong><span>Change the filters or use Codex, then click Refresh.</span></div>`}</article></div>`;
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
      <div class="setting-row"><span>Pricing</span><div class="pricing-setting"><code title="${escapeHtml(settings.pricingSource)}">${escapeHtml(settings.pricingCatalogVersion)}</code><button data-update-pricing>Check for updates</button></div></div>
    </article>
    <article class="panel settings-card"><div class="panel-title"><div><h2>Privacy</h2><p>Local-first by design</p></div><span class="safe-badge">Protected</span></div>
      <div class="privacy-note"><span class="shield">✓</span><p>${escapeHtml(settings.privacySummary)}</p></div>
      ${settingRow("OS user", settings.ownerUsername)}
      <form id="account-form" class="account-form">
        <label class="toggle-row"><span><strong>Account labels</strong><small>Optional manual labels; Codex credentials are never read</small></span><input name="enabled" type="checkbox" ${settings.accountTracking ? "checked" : ""} /></label>
        <label><span>Label for new sessions</span><input name="label" autocomplete="off" maxlength="64" value="${escapeHtml(settings.accountLabel ?? "")}" placeholder="e.g. Work" /></label>
        <label class="claim-row"><input name="claimExisting" type="checkbox" /><span>Label existing unassigned sessions too</span></label>
        <button type="submit">Save account settings</button>
      </form>
    </article>
    <article class="panel settings-card remote-card"><div class="panel-title"><div><h2>Remote servers</h2><p>SSH aliases whose metadata is shown on this Mac</p></div></div>
      <div class="remote-list">${settings.remoteHosts.length ? settings.remoteHosts.map(host => {
        const source = settings.remoteSources.find(item => item.host === host);
        const detail = source?.lastErrorKind ? `Needs attention · ${source.lastErrorKind}` : source?.lastSuccessAt ? `${source.discoveredFiles} files · synced ${dateLabel(source.lastSuccessAt)}` : "Not synced yet";
        return `<div class="remote-row"><span class="status-dot ${source?.lastSuccessAt && !source.lastErrorKind ? "ready" : source?.lastErrorKind ? "failed" : ""}"></span><div class="remote-main"><strong>${escapeHtml(host)}</strong><small>${escapeHtml(detail)}</small></div><div class="remote-actions"><button data-test-remote="${escapeHtml(host)}">Test</button><button data-sync-remote="${escapeHtml(host)}">Sync</button><button data-remove-remote="${escapeHtml(host)}">Remove</button></div></div>`;
      }).join("") : `<div class="empty-inline">No remote servers configured.</div>`}</div>
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
    if (state.page === "insights" && !state.insights) await loadInsights();
  }));
  document.querySelectorAll<HTMLElement>("[data-period]").forEach(button => button.addEventListener("click", async () => {
    state.period = button.dataset.period as Period;
    await reloadScope();
  }));
  document.querySelector<HTMLSelectElement>("#project-select")?.addEventListener("change", async event => {
    const value = (event.target as HTMLSelectElement).value;
    state.project = value || null;
    await reloadScope();
  });
  document.querySelector<HTMLSelectElement>("#account-select")?.addEventListener("change", async event => {
    const value = (event.target as HTMLSelectElement).value;
    state.account = value || null;
    await reloadScope();
  });
  document.querySelector<HTMLInputElement>("#anchor-date")?.addEventListener("change", async event => {
    state.anchor = (event.target as HTMLInputElement).value || state.anchor;
    await reloadScope();
  });
  document.querySelectorAll<HTMLElement>("[data-date-step]").forEach(button => button.addEventListener("click", async () => {
    stepAnchor(Number(button.dataset.dateStep));
    await reloadScope();
  }));
  document.querySelectorAll<HTMLElement>("[data-history-group]").forEach(button => button.addEventListener("click", () => {
    state.historyGroup = button.dataset.historyGroup as typeof state.historyGroup;
    render();
  }));
  document.querySelector<HTMLElement>("[data-export]")?.addEventListener("click", exportReport);
  document.querySelector(".refresh-button")?.addEventListener("click", () => refreshAll());
  document.querySelector("[data-cancel-refresh]")?.addEventListener("click", async () => {
    await invoke("cancel_refresh");
    state.notice = "Cancelling remote sync safely…";
    render();
  });
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
  document.querySelectorAll<HTMLElement>("[data-test-remote]").forEach(button => button.addEventListener("click", async () => {
    const host = button.dataset.testRemote!;
    state.busy = true; state.notice = `Testing ${host}…`; render();
    try {
      const files = await invoke<number>("test_remote", { host });
      state.notice = `${host} is online · ${files} Rollout files found`;
      state.error = "";
    } catch (error) { state.error = String(error); state.notice = ""; }
    state.busy = false; render();
  }));
  document.querySelector<HTMLElement>("[data-update-pricing]")?.addEventListener("click", async () => {
    state.busy = true; state.notice = "Downloading and verifying the pricing catalog…"; render();
    try {
      const version = await invoke<string>("update_pricing");
      await Promise.all([loadDashboard(false), loadSettings(false)]);
      state.notice = `Pricing catalog updated to ${version}`;
      state.error = "";
    } catch (error) { state.error = String(error); state.notice = ""; }
    state.busy = false; render();
  });
  document.querySelector<HTMLFormElement>("#account-form")?.addEventListener("submit", async event => {
    event.preventDefault();
    const form = new FormData(event.currentTarget as HTMLFormElement);
    const enabled = form.get("enabled") === "on";
    const label = String(form.get("label") ?? "").trim() || null;
    const claimExisting = form.get("claimExisting") === "on";
    state.busy = true; state.notice = "Saving private account-label settings…"; render();
    try {
      state.settings = await invoke<DesktopSettings>("update_account_tracking", { enabled, label, claimExisting });
      state.account = null;
      await loadDashboard(false);
      state.notice = enabled ? "Account labels enabled for new sessions" : "Account labels disabled";
      state.error = "";
    } catch (error) { state.error = String(error); state.notice = ""; }
    state.busy = false; render();
  });
  document.querySelectorAll<HTMLElement>("[data-sync-remote]").forEach(button => button.addEventListener("click", async () => {
    const host = button.dataset.syncRemote!;
    state.busy = true; state.notice = `Syncing ${host} metadata…`; render();
    try {
      await invoke("refresh_remote", { host });
      await Promise.all([loadDashboard(false), loadSettings(false), loadInsights(false)]);
      state.notice = `${host} is up to date`;
      state.error = "";
    } catch (error) { state.error = String(error); state.notice = ""; }
    state.busy = false; render();
  }));
}

function stepAnchor(direction: number): void {
  const date = new Date(`${state.anchor}T12:00:00`);
  if (state.period === "month") date.setMonth(date.getMonth() + direction);
  else date.setDate(date.getDate() + direction * (state.period === "week" ? 7 : 1));
  state.anchor = date.toISOString().slice(0, 10);
}

async function reloadScope(): Promise<void> {
  state.insights = null;
  await Promise.all([loadDashboard(), state.page === "insights" ? loadInsights(false) : Promise.resolve()]);
}

async function exportReport(): Promise<void> {
  state.busy = true; state.notice = "Exporting filtered metadata…"; render();
  try {
    const path = await invoke<string>("export_report", { period: state.period, anchor: state.anchor, project: state.project, account: state.account });
    state.notice = `Exported to ${path}`;
    state.error = "";
  } catch (error) { state.error = String(error); state.notice = ""; }
  state.busy = false; render();
}

async function loadDashboard(showProgress = true): Promise<void> {
  if (showProgress) { state.busy = true; state.notice = "Loading dashboard…"; render(); }
  try {
    state.snapshot = await invoke<DashboardSnapshot>("load_dashboard", { period: state.period, anchor: state.anchor, project: state.project, account: state.account });
    state.error = "";
  } catch (error) { state.error = String(error); }
  if (showProgress) { state.busy = false; state.notice = ""; render(); }
}

async function loadInsights(renderAfter = true): Promise<void> {
  try {
    state.insights = await invoke<InsightsSnapshot>("load_insights", { period: state.period, anchor: state.anchor, project: state.project, account: state.account });
    state.error = "";
  } catch (error) { state.error = String(error); }
  if (renderAfter) render();
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
    await Promise.all([loadDashboard(false), loadSettings(false), state.page === "insights" ? loadInsights(false) : Promise.resolve(), quotaPromise]);
    state.error = outcome.warnings.join(" · ");
    state.notice = outcome.cancelled ? "Remote sync cancelled; existing data was preserved." : outcome.warnings.length ? "Local usage is up to date; one or more remote sources need attention." : "Usage is up to date.";
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

window.addEventListener("keydown", event => {
  if (!event.metaKey || event.altKey || event.ctrlKey) return;
  if (event.key.toLowerCase() === "r") {
    event.preventDefault();
    void refreshAll();
    return;
  }
  if (event.key === ",") {
    event.preventDefault();
    state.page = "settings";
    render();
    if (!state.settings) void loadSettings();
    return;
  }
  const pages = { "1": "overview", "2": "history", "3": "insights" } as const;
  const page = pages[event.key as keyof typeof pages];
  if (page) {
    event.preventDefault();
    state.page = page;
    render();
    if (page === "insights" && !state.insights) void loadInsights();
  }
});

render();
await Promise.all([loadDashboard(false), loadSettings(false)]);
render();
void refreshAll(true);
