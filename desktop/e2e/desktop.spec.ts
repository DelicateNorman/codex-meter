import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

async function installMockBridge(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const commands: Array<{ command: string; args?: Record<string, unknown> }> = [];
    Object.assign(window, { __CODEX_METER_COMMANDS__: commands });
    const dashboard = (args: Record<string, unknown> = {}) => ({
      generatedAt: "2026-08-14T12:00:00Z",
      anchorDate: String(args.anchor ?? "2026-08-14"),
      period: String(args.period ?? "day"),
      periodLabel: args.period === "month" ? "August 2026" : "Today",
      project: args.project ?? null,
      account: args.account ?? null,
      overview: {
        calls: 2648, sessions: 8, turns: 184, inputTokens: 342730000,
        cachedInputTokens: 332110000, cacheWriteTokens: 0, outputTokens: 1130000,
        reasoningTokens: 310600, totalTokens: 343870000, costUsd: 243.85,
        unpricedCalls: 97, missingModelCalls: 1, unpublishedPriceCalls: 96,
        historicalPriceEstimateCalls: 61, avgTtftMs: 5710, avgE2eMs: 192910,
      },
      models: [
        { model: "gpt-5.6-sol", effort: "high", calls: 1467, inputTokens: 188110000, cachedInputTokens: 182500000, outputTokens: 1130000, reasoningTokens: 177200, totalTokens: 189240000, costUsd: 137.98, unpricedCalls: 0 },
        { model: "gpt-5.6-sol", effort: "xhigh", calls: 1037, inputTokens: 136200000, cachedInputTokens: 132380000, outputTokens: 310000, reasoningTokens: 112000, totalTokens: 136510000, costUsd: 99.06, unpricedCalls: 0 },
      ],
      history: [
        { periodStart: "2026-08-11", calls: 650, sessions: 3, turns: 42, inputTokens: 70000000, cachedInputTokens: 65000000, outputTokens: 250000, totalTokens: 70250000, costUsd: 48.2 },
        { periodStart: "2026-08-12", calls: 900, sessions: 4, turns: 58, inputTokens: 120000000, cachedInputTokens: 116000000, outputTokens: 400000, totalTokens: 120400000, costUsd: 86.4 },
        { periodStart: "2026-08-13", calls: 1098, sessions: 5, turns: 84, inputTokens: 152730000, cachedInputTokens: 151110000, outputTokens: 480000, totalTokens: 153210000, costUsd: 109.25 },
      ],
      weeklyHistory: [{ periodStart: "2026-08-10", calls: 2648, sessions: 8, turns: 184, inputTokens: 342730000, cachedInputTokens: 332110000, outputTokens: 1130000, totalTokens: 343870000, costUsd: 243.85 }],
      monthlyHistory: [{ periodStart: "2026-08-01", calls: 2648, sessions: 8, turns: 184, inputTokens: 342730000, cachedInputTokens: 332110000, outputTokens: 1130000, totalTokens: 343870000, costUsd: 243.85 }],
      recentSessions: [{ codexThreadId: "thread-1", projectName: "codex-meter", startedAt: "2026-08-14T10:00:00Z", endedAt: "2026-08-14T10:03:00Z", turns: 12, calls: 52, totalTokens: 7200000, costUsd: 5.24, cachedInputTokens: 6800000, inputTokens: 7000000 }],
      projects: ["codex-meter", "research-notes"], accounts: ["Work", "Unassigned"],
      remoteCount: 1, ownerUsername: "zhangxinlang",
    });
    const settings = {
      version: "0.17.0-beta.1", pricingCatalogVersion: "openai-2026-08-14",
      pricingSource: "Official OpenAI model documentation", meterHome: "/Users/demo/.codex-meter",
      databasePath: "/Users/demo/.codex-meter/meter.db", codexHome: "/Users/demo/.codex",
      sessionsPath: "/Users/demo/.codex/sessions", ownerUsername: "zhangxinlang",
      accountTracking: false, accountLabel: null, remoteHosts: ["devbox"],
      remoteSources: [{ host: "devbox", lastAttemptAt: "2026-08-14T11:00:00Z", lastSuccessAt: "2026-08-14T11:00:00Z", lastErrorKind: null, discoveredFiles: 60, importedFiles: 2, skippedFiles: 58 }],
      privacySummary: "Statistics metadata only. Prompts, responses, reasoning text, commands, tool output, headers, and credentials are not stored.",
    };
    window.__CODEX_METER_TEST__ = {
      listen: async () => () => undefined,
      invoke: async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
        commands.push({ command, args });
        if (command === "load_dashboard") return dashboard(args) as T;
        if (command === "load_settings") return settings as T;
        if (command === "load_quotas") return [
          { limitId: "codex", name: "Codex", usedPercent: 32, resetsAt: 1786761600, windowMinutes: 10080, planType: "plus" },
          { limitId: "spark", name: "GPT-5.3-Codex-Spark", usedPercent: 0, resetsAt: 1786848000, windowMinutes: 10080, planType: "plus" },
        ] as T;
        if (command === "load_insights") return {
          cache: { inputTokens: 342730000, cachedInputTokens: 332110000, cacheWriteTokens: 0, reuseRate: .969, observedCostUsd: 243.85, withoutCacheUsd: 612.2, savingsUsd: 368.35, pricedCalls: 2551, unpricedCalls: 97, retryCalls: 18, retryTokens: 1200000 },
          performance: { samples: 184, averageTtftMs: 5710, p95TtftMs: 12500, averageE2eMs: 192910, p95E2eMs: 320000, averageOutputTps: 42.5, recent: [] },
          network: [{ startedAt: "2026-08-14T10:00:00Z", mode: "proxy", destination: "api.openai.com", durationMs: 6250, ttfbMs: 840, requestBytes: 1200, responseBytes: 48200, success: true, errorType: null }],
        } as T;
        if (command === "refresh_all") return { warnings: [], cancelled: false } as T;
        if (command === "export_report") return "/Users/demo/Downloads/codex-meter.csv" as T;
        if (command === "update_account_tracking") return { ...settings, accountTracking: Boolean(args?.enabled), accountLabel: args?.label ?? null } as T;
        return undefined as T;
      },
    };
  });
}

async function openReady(page: Page): Promise<void> {
  await installMockBridge(page);
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Weekly allowance" })).toBeVisible();
  await expect(page.locator(".progress-line")).toHaveCount(0, { timeout: 5_000 });
}

test("desktop navigation, filters, shortcuts, and account opt-in", async ({ page }) => {
  await openReady(page);
  await page.getByRole("button", { name: "History" }).click();
  await expect(page.getByRole("heading", { name: "Usage over time" })).toBeVisible();
  await page.keyboard.press("Meta+3");
  await expect(page.getByRole("heading", { name: "Network observations" })).toBeVisible();
  await page.getByRole("button", { name: "Month" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __CODEX_METER_COMMANDS__: Array<{ command: string; args?: Record<string, unknown> }> }).__CODEX_METER_COMMANDS__.some(call => call.command === "load_dashboard" && call.args?.period === "month"))).toBe(true);
  await page.keyboard.press("Meta+,");
  await expect(page.getByRole("heading", { name: "Privacy" })).toBeVisible();
  await page.getByLabel("Account labels").check();
  await page.getByLabel("Label for new sessions").fill("Work");
  await page.getByRole("button", { name: "Save account settings" }).click();
  await expect(page.getByText("Account labels enabled for new sessions")).toBeVisible();
});

for (const colorScheme of ["light", "dark"] as const) {
  test(`${colorScheme} appearance and accessibility`, async ({ page }) => {
    await page.emulateMedia({ colorScheme });
    await openReady(page);
    const results = await new AxeBuilder({ page }).analyze();
    const severe = results.violations.filter(result => ["serious", "critical"].includes(result.impact ?? ""));
    expect(severe, severe.map(result => `${result.id}: ${result.help}`).join("\n")).toEqual([]);
    await expect(page).toHaveScreenshot(`overview-${colorScheme}.png`, {
      animations: "disabled",
      caret: "hide",
      maxDiffPixelRatio: 0.01,
    });
  });
}
