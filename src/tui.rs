//! Dark-blue, dependency-light terminal reports.

use crate::quota::WeeklyQuota;
use chrono::{Local, TimeZone};
use unicode_width::UnicodeWidthChar;

pub const RESET: &str = "\x1b[0m";
pub const BG: &str = "\x1b[48;2;7;17;31m";
pub const PANEL: &str = "\x1b[48;2;11;31;51m";
pub const BLUE: &str = "\x1b[38;2;10;132;255m";
pub const CYAN: &str = "\x1b[38;2;56;189;248m";
pub const LIGHT: &str = "\x1b[38;2;234;242;255m";
pub const MUTED: &str = "\x1b[38;2;147;164;184m";
pub const GREEN: &str = "\x1b[38;2;56;217;150m";
pub const YELLOW: &str = "\x1b[38;2;247;201;72m";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
    pub unpriced_calls: i64,
    pub calls: i64,
    pub sessions: i64,
    pub turns: i64,
    pub avg_ttft_ms: Option<f64>,
    pub avg_e2e_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelRow {
    pub model: String,
    pub effort: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryRow {
    pub period_start: String,
    pub sessions: i64,
    pub turns: i64,
    pub calls: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkRow {
    pub local_time: String,
    pub model: String,
    pub output_tokens: i64,
    pub ttft_ms: Option<f64>,
    pub e2e_ms: Option<f64>,
    pub exact_output_tps: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowRow {
    pub destination_host: Option<String>,
    pub destination_ip: Option<String>,
    pub success: Option<bool>,
    pub error_type: Option<String>,
    pub dns_ms: Option<f64>,
    pub tcp_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct OverviewOptions<'a> {
    pub period: &'a str,
    pub color: bool,
    pub width: usize,
    /// `None` hides the account section; `Some(&[])` shows its message.
    pub weekly_quotas: Option<&'a [WeeklyQuota]>,
    pub quota_message: Option<&'a str>,
    pub source_label: &'a str,
    pub source_message: Option<&'a str>,
}

impl<'a> OverviewOptions<'a> {
    pub fn new(period: &'a str, width: usize, color: bool) -> Self {
        Self {
            period,
            color,
            width,
            weekly_quotas: None,
            quota_message: None,
            source_label: "LOCAL",
            source_message: None,
        }
    }
}

pub fn render_overview(
    overview: &Overview,
    models: &[ModelRow],
    options: &OverviewOptions<'_>,
) -> String {
    let width = options.width.clamp(40, 132);
    let inner = width - 2;
    let mut lines = Vec::new();
    let border = |left: char, middle: char, right: char| {
        styled(
            &format!("{left}{}{right}", middle.to_string().repeat(inner - 1)),
            BLUE,
            false,
            options.color,
        )
    };

    lines.push(border('╭', '─', '╮'));
    lines.push(frame(
        &format!(
            "CODEX METER  ● {}  │  {}",
            options.source_label, options.period
        ),
        inner,
        CYAN,
        false,
        options.color,
    ));
    lines.push(border('├', '─', '┤'));

    // A loading/error message must occupy the quota section even before a
    // worker has produced its first `Vec<WeeklyQuota>`.  Non-interactive
    // reports still omit the section by leaving both values as `None`.
    if options.weekly_quotas.is_some() || options.quota_message.is_some() {
        let quotas = options.weekly_quotas.unwrap_or(&[]);
        if quotas.is_empty() {
            lines.push(frame(
                options.quota_message.unwrap_or(
                    "ACCOUNT WEEKLY LIMITS  No seven-day quota was provided for this account",
                ),
                inner,
                YELLOW,
                true,
                options.color,
            ));
        } else {
            lines.push(frame(
                "ACCOUNT WEEKLY LIMITS",
                inner,
                MUTED,
                true,
                options.color,
            ));
            let quota_bar_width = width.saturating_sub(34).clamp(16, 40);
            for quota in quotas {
                let style = if quota.remaining_percent() > 25 {
                    GREEN
                } else {
                    YELLOW
                };
                for line in weekly_quota_lines(quota, quota_bar_width) {
                    lines.push(frame(&line, inner, style, true, options.color));
                }
            }
        }
        lines.push(border('├', '─', '┤'));
    }

    if let Some(message) = options.source_message.filter(|message| !message.is_empty()) {
        lines.push(frame(message, inner, MUTED, true, options.color));
        lines.push(border('├', '─', '┤'));
    }

    let input = overview.input_tokens;
    let cached = overview.cached_input_tokens;
    let cache_write = overview.cache_write_tokens;
    let cache_miss = (input - cached - cache_write).max(0);
    let output = overview.output_tokens;
    let reasoning = overview.reasoning_tokens;
    let hit_rate = percent(cached, input);
    let reasoning_rate = percent(reasoning, output);
    lines.push(frame(
        &format!(
            "TOKENS  {:>9}    API-EQUIV  {:>10}    CACHE  {:>5.1}%    CALLS  {:>5}",
            tokens(overview.total_tokens),
            money(overview.cost_usd),
            hit_rate,
            overview.calls,
        ),
        inner,
        LIGHT,
        true,
        options.color,
    ));
    lines.push(frame(
        &format!(
            "Input {}  ·  Output {}  ·  Reasoning {} ({reasoning_rate:.1}%)",
            tokens(input),
            tokens(output),
            tokens(reasoning),
        ),
        inner,
        MUTED,
        true,
        options.color,
    ));
    lines.push(frame(
        &format!(
            "Cache read {}  ·  Cache miss {}  ·  Cache write {}",
            tokens(cached),
            tokens(cache_miss),
            tokens(cache_write),
        ),
        inner,
        MUTED,
        true,
        options.color,
    ));
    lines.push(frame(
        &format!(
            "Sessions {}  ·  Turns {}  ·  Avg TTFT {}  ·  Avg E2E {}",
            overview.sessions,
            overview.turns,
            duration(overview.avg_ttft_ms),
            duration(overview.avg_e2e_ms),
        ),
        inner,
        MUTED,
        true,
        options.color,
    ));
    if overview.unpriced_calls != 0 {
        lines.push(frame(
            &format!(
                "△ {} call(s) have unknown pricing; cost excludes them",
                overview.unpriced_calls
            ),
            inner,
            YELLOW,
            true,
            options.color,
        ));
    }

    lines.push(border('├', '─', '┤'));
    let bar_width = width.saturating_sub(56).clamp(16, 48);
    lines.push(frame(
        &bar_line("Input total", input, input, bar_width, "total input"),
        inner,
        CYAN,
        false,
        options.color,
    ));
    lines.push(frame(
        &bar_line(
            "Cached input",
            cached,
            input,
            bar_width,
            &format!("{hit_rate:.1}% of input"),
        ),
        inner,
        BLUE,
        false,
        options.color,
    ));
    lines.push(frame(
        &bar_line(
            "Reasoning out",
            reasoning,
            output.max(1),
            bar_width,
            &format!("{reasoning_rate:.1}% of output"),
        ),
        inner,
        CYAN,
        false,
        options.color,
    ));
    lines.push(border('├', '─', '┤'));
    lines.push(frame(
        &format!(
            "{:<25} {:<9} {:>6} {:>10} {:>7} {:>10} {:>10}",
            "MODEL", "EFFORT", "CALLS", "TOKENS", "CACHE", "REASON", "COST"
        ),
        inner,
        MUTED,
        false,
        options.color,
    ));
    if models.is_empty() {
        lines.push(frame(
            "No Codex usage found for this period or project.",
            inner,
            MUTED,
            false,
            options.color,
        ));
        lines.push(frame(
            "Use Codex normally, then press r here to refresh.",
            inner,
            YELLOW,
            false,
            options.color,
        ));
    }
    for row in models.iter().take(8) {
        let model = fit_display(
            if row.model.is_empty() {
                "Unknown"
            } else {
                &row.model
            },
            25,
        );
        let effort = fit_display(
            if row.effort.is_empty() {
                "Unknown"
            } else {
                &row.effort
            },
            9,
        );
        lines.push(frame(
            &format!(
                "{model:<25} {effort:<9} {:>6} {:>10} {:>6.1}% {:>10} {:>10}",
                row.calls,
                tokens(row.total_tokens),
                percent(row.cached_input_tokens, row.input_tokens),
                tokens(row.reasoning_tokens),
                money(row.cost_usd),
            ),
            inner,
            LIGHT,
            false,
            options.color,
        ));
    }
    lines.push(border('╰', '─', '╯'));
    lines.join("\n")
}

pub fn render_models(rows: &[ModelRow], color: bool) -> String {
    let mut output = vec!["Model / Effort usage".to_owned(), String::new()];
    output.push(format!(
        "{:<28} {:<10} {:>7} {:>12} {:>12} {:>12} {:>12} {:>11}",
        "MODEL", "EFFORT", "CALLS", "INPUT", "CACHED", "OUTPUT", "REASON", "COST"
    ));
    output.push("─".repeat(112));
    for row in rows {
        output.push(format!(
            "{:<28} {:<10} {:>7} {:>12} {:>12} {:>12} {:>12} {:>11}",
            fit_display(&row.model, 28),
            fit_display(&row.effort, 10),
            row.calls,
            tokens(row.input_tokens),
            tokens(row.cached_input_tokens),
            tokens(row.output_tokens),
            tokens(row.reasoning_tokens),
            money(row.cost_usd),
        ));
    }
    let text = output.join("\n");
    if color {
        format!("{BG}{LIGHT}{text}{RESET}")
    } else {
        text
    }
}

pub fn render_history(
    rows: &[HistoryRow],
    group: &str,
    username: &str,
    project: Option<&str>,
    color: bool,
) -> String {
    let mut title = format!("Usage history by {group} · OS user {username}");
    if let Some(project) = project {
        title.push_str(&format!(" · project {project}"));
    }
    let mut output = vec![title, String::new()];
    output.push(format!(
        "{:<14} {:>6} {:>7} {:>7} {:>13} {:>7} {:>12}",
        "PERIOD", "SESS", "TURNS", "CALLS", "TOKENS", "CACHE", "COST"
    ));
    output.push("─".repeat(72));
    for row in rows {
        output.push(format!(
            "{:<14} {:>6} {:>7} {:>7} {:>13} {:>6.1}% {:>12}",
            if row.period_start.is_empty() {
                "Unknown"
            } else {
                &row.period_start
            },
            row.sessions,
            row.turns,
            row.calls,
            tokens(row.total_tokens),
            percent(row.cached_input_tokens, row.input_tokens),
            money(row.cost_usd),
        ));
    }
    if rows.is_empty() {
        output.push("No imported usage.".into());
    }
    let text = output.join("\n");
    if color {
        format!("{BG}{LIGHT}{text}{RESET}")
    } else {
        text
    }
}

#[derive(Clone, Debug)]
pub struct NetworkOptions<'a> {
    pub period: &'a str,
    pub username: &'a str,
    pub project: Option<&'a str>,
    pub color: bool,
    pub width: usize,
}

pub fn render_network(
    rows: &[NetworkRow],
    flows: &[FlowRow],
    options: &NetworkOptions<'_>,
) -> String {
    let width = options.width.clamp(80, 132);
    let inner = width - 2;
    let mut lines = Vec::new();
    let border = |left: char, middle: char, right: char| {
        styled(
            &format!("{left}{}{right}", middle.to_string().repeat(inner - 1)),
            BLUE,
            false,
            options.color,
        )
    };
    let ttft: Vec<f64> = rows.iter().filter_map(|row| row.ttft_ms).collect();
    let e2e: Vec<f64> = rows.iter().filter_map(|row| row.e2e_ms).collect();
    let rates: Vec<Option<f64>> = rows.iter().map(output_rate).collect();
    let usable_rates: Vec<f64> = rates.iter().flatten().copied().collect();
    let exact_rates = rows
        .iter()
        .filter(|row| row.exact_output_tps.is_some())
        .count();

    lines.push(border('╭', '─', '╮'));
    let mut title = format!(
        "NETWORK & RESPONSE  ● LOCAL  │  {}  │  OS user {}",
        options.period, options.username
    );
    if let Some(project) = options.project {
        title.push_str(&format!("  │  project {project}"));
    }
    lines.push(frame(&title, inner, CYAN, false, options.color));
    lines.push(border('├', '─', '┤'));
    lines.push(frame(
        &format!(
            "Samples {} turns  ·  timed {}  ·  speed {} ({} exact, {} estimated)",
            rows.len(),
            e2e.len(),
            usable_rates.len(),
            exact_rates,
            usable_rates.len().saturating_sub(exact_rates),
        ),
        inner,
        LIGHT,
        true,
        options.color,
    ));
    lines.push(frame(
        &timing_summary("First token", &ttft),
        inner,
        MUTED,
        true,
        options.color,
    ));
    lines.push(frame(
        &timing_summary("Complete", &e2e),
        inner,
        MUTED,
        true,
        options.color,
    ));
    lines.push(frame(
        &rate_summary("Output speed", &usable_rates),
        inner,
        MUTED,
        true,
        options.color,
    ));
    lines.push(border('├', '─', '┤'));
    lines.push(frame(
        &format!(
            "{:<10} {:<25} {:>9} {:>10} {:>10} {:>9}",
            "TIME", "MODEL", "OUTPUT", "FIRST", "COMPLETE", "TOK/S"
        ),
        inner,
        MUTED,
        false,
        options.color,
    ));
    for (row, rate) in rows.iter().zip(rates).take(8) {
        let rate = rate.map_or_else(
            || "N/A".into(),
            |rate| {
                format!(
                    "{rate:.1}{}",
                    if row.exact_output_tps.is_some() {
                        ""
                    } else {
                        "*"
                    }
                )
            },
        );
        lines.push(frame(
            &format!(
                "{:<10} {:<25} {:>9} {:>10} {:>10} {:>9}",
                fit_display(
                    if row.local_time.is_empty() {
                        "N/A"
                    } else {
                        &row.local_time
                    },
                    10
                ),
                fit_display(
                    if row.model.is_empty() {
                        "Unknown"
                    } else {
                        &row.model
                    },
                    25
                ),
                tokens(row.output_tokens),
                duration(row.ttft_ms),
                duration(row.e2e_ms),
                rate,
            ),
            inner,
            LIGHT,
            false,
            options.color,
        ));
    }
    if rows.is_empty() {
        lines.push(frame(
            "No response timing samples for today. Press r to refresh local records.",
            inner,
            YELLOW,
            false,
            options.color,
        ));
    }
    lines.push(border('├', '─', '┤'));
    if let Some(flow) = flows.first() {
        let destination = flow
            .destination_host
            .as_deref()
            .or(flow.destination_ip.as_deref())
            .unwrap_or("Unknown");
        let status = match flow.success {
            None => "UNKNOWN".to_owned(),
            Some(true) => "OK".to_owned(),
            Some(false) => format!(
                "FAILED ({})",
                flow.error_type.as_deref().unwrap_or("connection error")
            ),
        };
        lines.push(frame(
            &format!(
                "Latest connection · {status} · {destination}  DNS {}  TCP {}  TLS {}  TTFB {}",
                duration(flow.dns_ms),
                duration(flow.tcp_ms),
                duration(flow.tls_ms),
                duration(flow.ttfb_ms),
            ),
            inner,
            CYAN,
            false,
            options.color,
        ));
    } else {
        lines.push(frame(
            "Connection setup · no probe/capture samples yet; response timing above still works.",
            inner,
            MUTED,
            false,
            options.color,
        ));
    }
    lines.push(frame(
        "* Estimated speed = output tokens / (complete time - first-token time); tool time may be included.",
        inner,
        YELLOW,
        false,
        options.color,
    ));
    lines.push(border('╰', '─', '╯'));
    lines.join("\n")
}

pub fn output_rate(row: &NetworkRow) -> Option<f64> {
    if let Some(exact) = row.exact_output_tps.filter(|value| *value >= 0.0) {
        return Some(exact);
    }
    let (Some(ttft), Some(e2e)) = (row.ttft_ms, row.e2e_ms) else {
        return None;
    };
    if row.output_tokens <= 0 || e2e <= ttft {
        return None;
    }
    Some(row.output_tokens as f64 * 1000.0 / (e2e - ttft))
}

fn timing_summary(label: &str, values: &[f64]) -> String {
    if values.is_empty() {
        return format!("{label:<14} AVG N/A  ·  P50 N/A  ·  P95 N/A");
    }
    format!(
        "{label:<14} AVG {}  ·  P50 {}  ·  P95 {}",
        duration(Some(values.iter().sum::<f64>() / values.len() as f64)),
        duration(percentile(values, 0.50)),
        duration(percentile(values, 0.95)),
    )
}

fn rate_summary(label: &str, values: &[f64]) -> String {
    if values.is_empty() {
        return format!("{label:<14} AVG N/A  ·  P50 N/A  ·  P95 N/A");
    }
    format!(
        "{label:<14} AVG {:.1} tok/s  ·  P50 {:.1}  ·  P95 {:.1}",
        values.iter().sum::<f64>() / values.len() as f64,
        percentile(values, 0.50).unwrap_or_default(),
        percentile(values, 0.95).unwrap_or_default(),
    )
}

fn percentile(values: &[f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    // Python round uses ties-to-even; preserve the reference renderer's
    // nearest-rank choice for two/even-sized samples.
    let index = ((values.len() - 1) as f64 * fraction).round_ties_even() as usize;
    values.get(index).copied()
}

fn weekly_quota_lines(quota: &WeeklyQuota, bar_width: usize) -> [String; 2] {
    let used = if quota.used_percent > 0 {
        ((bar_width as f64 * f64::from(quota.used_percent) / 100.0).round_ties_even() as usize)
            .max(1)
    } else {
        0
    }
    .min(bar_width);
    let bar = format!("{}{}", "█".repeat(used), "░".repeat(bar_width - used));
    [
        format!(
            "{}  ·  {}% left  ·  reset {}",
            fit_display(&quota.name, 28),
            quota.remaining_percent(),
            reset_time(quota.resets_at),
        ),
        format!("Used  {bar}  {:>3}%", quota.used_percent),
    ]
}

fn reset_time(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(|timestamp| Local.timestamp_opt(timestamp, 0).single())
        .map(|timestamp| timestamp.format("%b %d %H:%M").to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn bar_line(label: &str, value: i64, maximum: i64, width: usize, meaning: &str) -> String {
    let ratio = if maximum == 0 {
        0.0
    } else {
        (value as f64 / maximum as f64).clamp(0.0, 1.0)
    };
    let filled = (width as f64 * ratio).round_ties_even() as usize;
    format!(
        "{label:<14} {}{}  {:>9}  {meaning}",
        "█".repeat(filled),
        "░".repeat(width - filled),
        tokens(value),
    )
}

fn percent(numerator: i64, denominator: i64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64 * 100.0
    }
}

pub fn tokens(value: i64) -> String {
    let absolute = value.unsigned_abs();
    if absolute >= 1_000_000_000 {
        format!("{:.2}B", value as f64 / 1_000_000_000.0)
    } else if absolute >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1_000_000.0)
    } else if absolute >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

pub fn money(value: Option<f64>) -> String {
    value.map_or_else(|| "N/A".into(), |value| format!("${value:.2} eq"))
}

pub fn duration(value: Option<f64>) -> String {
    value.map_or_else(
        || "N/A".into(),
        |milliseconds| {
            if milliseconds < 1000.0 {
                format!("{milliseconds:.0}ms")
            } else {
                format!("{:.2}s", milliseconds / 1000.0)
            }
        },
    )
}

pub fn fit_display(value: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    for character in value.chars() {
        let cell_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + cell_width > width {
            break;
        }
        result.push(character);
        used += cell_width;
    }
    result
}

pub fn display_width(value: &str) -> usize {
    value
        .chars()
        .map(|character| UnicodeWidthChar::width(character).unwrap_or(0))
        .sum()
}

fn frame(text: &str, inner: usize, style: &str, panel: bool, color: bool) -> String {
    let clipped = fit_display(text, inner.saturating_sub(2));
    let padding = " ".repeat(inner.saturating_sub(2 + display_width(&clipped)));
    styled(&format!("│ {clipped}{padding} │"), style, panel, color)
}

fn styled(text: &str, style: &str, panel: bool, color: bool) -> String {
    if color {
        format!("{}{style}{text}{RESET}", if panel { PANEL } else { BG })
    } else {
        text.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::WEEK_MINUTES;

    fn options<'a>(period: &'a str, width: usize) -> OverviewOptions<'a> {
        OverviewOptions::new(period, width, false)
    }

    #[test]
    fn quota_bars_and_explanatory_ratios_match_python_ui() {
        let overview = Overview {
            input_tokens: 1_000,
            cached_input_tokens: 800,
            output_tokens: 100,
            reasoning_tokens: 25,
            ..Overview::default()
        };
        let quotas = [WeeklyQuota {
            limit_id: "codex".into(),
            name: "Codex".into(),
            used_percent: 15,
            resets_at: None,
            window_minutes: WEEK_MINUTES,
            plan_type: None,
        }];
        let mut options = options("TODAY", 80);
        options.weekly_quotas = Some(&quotas);
        let rendered = render_overview(&overview, &[], &options);
        assert!(rendered.contains("ACCOUNT WEEKLY LIMITS"));
        assert!(rendered.contains("85% left"));
        assert!(rendered.contains("Used  ██"));
        assert!(rendered.contains("80.0% of input"));
        assert!(rendered.contains("25.0% of output"));
        assert!(!rendered.contains("100.0%"));
        assert!(rendered.lines().all(|line| display_width(line) <= 80));
    }

    #[test]
    fn zero_and_full_quota_bars_are_unambiguous() {
        let quotas = [
            WeeklyQuota {
                limit_id: "free".into(),
                name: "Unused".into(),
                used_percent: 0,
                resets_at: None,
                window_minutes: WEEK_MINUTES,
                plan_type: None,
            },
            WeeklyQuota {
                limit_id: "full".into(),
                name: "Exhausted".into(),
                used_percent: 100,
                resets_at: None,
                window_minutes: WEEK_MINUTES,
                plan_type: None,
            },
        ];
        let mut options = options("TODAY", 80);
        options.weekly_quotas = Some(&quotas);
        let rendered = render_overview(&Overview::default(), &[], &options);
        assert!(rendered.contains("Used  ░░░░░░░░░░░░░░░░"));
        assert!(rendered.contains("Used  ████████████████"));
        assert!(rendered.contains("0% left"));
    }

    #[test]
    fn loading_message_shows_before_worker_returns_quota_rows() {
        let mut options = options("TODAY", 80);
        options.quota_message = Some("ACCOUNT WEEKLY LIMITS  Loading…");
        let rendered = render_overview(&Overview::default(), &[], &options);
        assert!(rendered.contains("ACCOUNT WEEKLY LIMITS  Loading…"));
    }

    #[test]
    fn remote_message_follows_weekly_limits() {
        let quotas = [WeeklyQuota {
            limit_id: "codex".into(),
            name: "Codex".into(),
            used_percent: 18,
            resets_at: None,
            window_minutes: WEEK_MINUTES,
            plan_type: None,
        }];
        let mut options = options("TODAY", 100);
        options.weekly_quotas = Some(&quotas);
        options.source_label = "LOCAL + 1 REMOTE";
        options.source_message = Some("REMOTE SOURCES  devbox synced · 1 updated");
        let rendered = render_overview(&Overview::default(), &[], &options);
        assert!(rendered.contains("CODEX METER  ● LOCAL + 1 REMOTE"));
        assert!(rendered.find("ACCOUNT WEEKLY LIMITS") < rendered.find("REMOTE SOURCES"));
    }

    #[test]
    fn empty_metrics_use_na_and_unicode_is_clipped_by_cells() {
        let rendered = render_overview(
            &Overview::default(),
            &[],
            &options("项目".repeat(40).as_str(), 80),
        );
        assert!(rendered.contains("N/A"));
        assert!(rendered.contains("No Codex usage found"));
        assert!(rendered.lines().all(|line| display_width(line) <= 80));
        assert_eq!(display_width(&fit_display("中文项目", 5)), 4);
    }

    #[test]
    fn network_estimates_output_speed_after_first_token() {
        let rows = [NetworkRow {
            local_time: "12:34:56".into(),
            model: "gpt-test".into(),
            output_tokens: 200,
            ttft_ms: Some(1000.0),
            e2e_ms: Some(5000.0),
            exact_output_tps: None,
        }];
        let rendered = render_network(
            &rows,
            &[],
            &NetworkOptions {
                period: "DAY",
                username: "test-user",
                project: None,
                color: false,
                width: 100,
            },
        );
        assert!(rendered.contains("NETWORK & RESPONSE"));
        assert!(rendered.contains("First token"));
        assert!(rendered.contains("50.0*"));
        assert!(rendered.contains("no probe/capture samples"));
    }

    #[test]
    fn python_ties_to_even_rounding_is_preserved() {
        assert_eq!(percentile(&[10.0, 20.0], 0.5), Some(10.0));
        let quota = WeeklyQuota {
            limit_id: "codex".into(),
            name: "Codex".into(),
            used_percent: 15,
            resets_at: None,
            window_minutes: WEEK_MINUTES,
            plan_type: None,
        };
        assert_eq!(weekly_quota_lines(&quota, 30)[1].matches('█').count(), 4);
        assert_eq!(
            bar_line("Cached input", 25, 100, 10, "25.0% of input")
                .matches('█')
                .count(),
            2
        );
    }

    #[test]
    fn history_and_failed_network_flow_keep_unknown_values_explicit() {
        let history = render_history(
            &[HistoryRow {
                period_start: "2026-08-12".into(),
                sessions: 2,
                turns: 4,
                calls: 5,
                input_tokens: 100,
                cached_input_tokens: 50,
                total_tokens: 120,
                cost_usd: None,
            }],
            "day",
            "tester",
            Some("demo"),
            false,
        );
        assert!(history.contains("tester"));
        assert!(history.contains("project demo"));
        assert!(history.contains("50.0%"));
        assert!(history.contains("N/A"));

        let network = render_network(
            &[],
            &[FlowRow {
                destination_host: Some("api.openai.com".into()),
                success: Some(false),
                error_type: Some("timeout".into()),
                ..FlowRow::default()
            }],
            &NetworkOptions {
                period: "DAY",
                username: "tester",
                project: Some("demo"),
                color: false,
                width: 100,
            },
        );
        assert!(network.contains("FAILED (timeout)"));
        assert!(network.contains("api.openai.com"));
        assert!(network.contains("AVG N/A"));
        assert!(network.contains("project demo"));
    }

    #[test]
    fn color_mode_uses_palette_and_plain_mode_has_no_escapes() {
        let plain = render_overview(
            &Overview::default(),
            &[],
            &OverviewOptions::new("TODAY", 80, false),
        );
        let colored = render_overview(
            &Overview::default(),
            &[],
            &OverviewOptions::new("TODAY", 80, true),
        );
        assert!(!plain.contains(''));
        for expected in [BG, PANEL, BLUE, CYAN, LIGHT, MUTED, YELLOW, RESET] {
            assert!(
                colored.contains(expected),
                "missing palette entry {expected:?}"
            );
        }
    }

    #[test]
    fn overview_warns_for_unpriced_calls_and_limits_model_rows_to_eight() {
        let overview = Overview {
            unpriced_calls: 3,
            ..Overview::default()
        };
        let models = (0..10)
            .map(|index| ModelRow {
                model: format!("model-{index}"),
                ..ModelRow::default()
            })
            .collect::<Vec<_>>();
        let rendered = render_overview(
            &overview,
            &models,
            &OverviewOptions::new("TODAY", 100, false),
        );
        assert!(rendered.contains("3 call(s) have unknown pricing"));
        assert!(rendered.contains("model-7"));
        assert!(!rendered.contains("model-8"));
    }
}
