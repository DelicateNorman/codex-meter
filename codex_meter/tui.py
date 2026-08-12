"""Dependency-free, dark-blue terminal dashboard."""

from __future__ import annotations

import os
import shutil
import sys
from typing import Mapping, Sequence


RESET = "\x1b[0m"
BG = "\x1b[48;2;7;17;31m"
PANEL = "\x1b[48;2;11;31;51m"
BLUE = "\x1b[38;2;10;132;255m"
CYAN = "\x1b[38;2;56;189;248m"
LIGHT = "\x1b[38;2;234;242;255m"
MUTED = "\x1b[38;2;147;164;184m"
GREEN = "\x1b[38;2;56;217;150m"
YELLOW = "\x1b[38;2;247;201;72m"


def render_overview(
    overview: Mapping[str, object],
    models: Sequence[Mapping[str, object]],
    *,
    period: str,
    color: bool | None = None,
    width: int | None = None,
) -> str:
    color = _supports_color() if color is None else color
    width = max(80, min(width or shutil.get_terminal_size((110, 30)).columns, 132))
    inner = width - 2
    lines: list[str] = []

    def styled(text: str, style: str = LIGHT, panel: bool = False) -> str:
        if not color:
            return text
        return f"{PANEL if panel else BG}{style}{text}{RESET}"

    def frame(text: str = "", style: str = LIGHT, panel: bool = False) -> str:
        clipped = text[: inner - 2]
        plain = f"│ {clipped:<{inner - 2}} │"
        return styled(plain, style, panel)

    lines.append(styled("╭" + "─" * (inner - 1) + "╮", BLUE))
    title = f"CODEX METER  ● LOCAL  │  {period}"
    lines.append(frame(title, CYAN))
    lines.append(styled("├" + "─" * (inner - 1) + "┤", BLUE))

    input_tokens = _int(overview.get("input_tokens"))
    cached = _int(overview.get("cached_input_tokens"))
    cache_write = _int(overview.get("cache_write_tokens"))
    cache_miss = max(0, input_tokens - cached - cache_write)
    output = _int(overview.get("output_tokens"))
    reasoning = _int(overview.get("reasoning_tokens"))
    total = _int(overview.get("total_tokens"))
    hit_rate = cached / input_tokens * 100 if input_tokens else 0
    reasoning_rate = reasoning / output * 100 if output else 0
    cost = overview.get("cost_usd")
    unpriced = _int(overview.get("unpriced_calls"))

    metrics = (
        f"TOKENS  {_tokens(total):>9}    "
        f"API-EQUIV  {_money(cost):>10}    "
        f"CACHE  {hit_rate:>5.1f}%    "
        f"CALLS  {_int(overview.get('calls')):>5}"
    )
    lines.append(frame(metrics, LIGHT, True))
    subtitle = (
        f"Input {_tokens(input_tokens)}  ·  Output {_tokens(output)}  ·  "
        f"Reasoning {_tokens(reasoning)} ({reasoning_rate:.1f}%)"
    )
    lines.append(frame(subtitle, MUTED, True))
    lines.append(
        frame(
            f"Cache read {_tokens(cached)}  ·  Cache miss {_tokens(cache_miss)}  ·  Cache write {_tokens(cache_write)}",
            MUTED,
            True,
        )
    )
    latency = (
        f"Sessions {_int(overview.get('sessions'))}  ·  Turns {_int(overview.get('turns'))}  ·  "
        f"Avg TTFT {_duration(overview.get('avg_ttft_ms'))}  ·  "
        f"Avg E2E {_duration(overview.get('avg_e2e_ms'))}"
    )
    lines.append(frame(latency, MUTED, True))
    if unpriced:
        lines.append(frame(f"△ {unpriced} call(s) have unknown pricing; cost excludes them", YELLOW, True))

    lines.append(styled("├" + "─" * (inner - 1) + "┤", BLUE))
    bar_width = max(20, min(48, width - 38))
    lines.append(frame(_bar_line("Input", input_tokens, input_tokens, bar_width), CYAN))
    lines.append(frame(_bar_line("Cached", cached, input_tokens, bar_width), BLUE))
    lines.append(frame(_bar_line("Reasoning", reasoning, max(output, 1), bar_width), CYAN))
    lines.append(styled("├" + "─" * (inner - 1) + "┤", BLUE))
    header = f"{'MODEL':<25} {'EFFORT':<9} {'CALLS':>6} {'TOKENS':>10} {'CACHE':>7} {'REASON':>10} {'COST':>10}"
    lines.append(frame(header, MUTED))
    if not models:
        lines.append(frame("No imported usage for this period.", MUTED))
    for row in models[:8]:
        row_input = _int(row.get("input_tokens"))
        row_cached = _int(row.get("cached_input_tokens"))
        row_cache = row_cached / row_input * 100 if row_input else 0
        body = (
            f"{str(row.get('model') or 'Unknown')[:25]:<25} "
            f"{str(row.get('effort') or 'Unknown')[:9]:<9} "
            f"{_int(row.get('calls')):>6} {_tokens(_int(row.get('total_tokens'))):>10} "
            f"{row_cache:>6.1f}% {_tokens(_int(row.get('reasoning_tokens'))):>10} "
            f"{_money(row.get('cost_usd')):>10}"
        )
        lines.append(frame(body, LIGHT))
    lines.append(styled("╰" + "─" * (inner - 1) + "╯", BLUE))
    return "\n".join(lines)


def render_models(rows: Sequence[Mapping[str, object]], *, color: bool | None = None) -> str:
    color = _supports_color() if color is None else color
    output = ["Model / Effort usage", ""]
    output.append(f"{'MODEL':<28} {'EFFORT':<10} {'CALLS':>7} {'INPUT':>12} {'CACHED':>12} {'OUTPUT':>12} {'REASON':>12} {'COST':>11}")
    output.append("─" * 112)
    for row in rows:
        output.append(
            f"{str(row['model'])[:28]:<28} {str(row['effort'])[:10]:<10} "
            f"{_int(row['calls']):>7} {_tokens(_int(row['input_tokens'])):>12} "
            f"{_tokens(_int(row['cached_input_tokens'])):>12} {_tokens(_int(row['output_tokens'])):>12} "
            f"{_tokens(_int(row['reasoning_tokens'])):>12} {_money(row['cost_usd']):>11}"
        )
    text = "\n".join(output)
    return f"{BG}{LIGHT}{text}{RESET}" if color else text


def render_history(
    rows: Sequence[Mapping[str, object]],
    *,
    group: str,
    username: str,
    project: str | None = None,
    color: bool | None = None,
) -> str:
    color = _supports_color() if color is None else color
    title = f"Usage history by {group} · OS user {username}"
    if project:
        title += f" · project {project}"
    output = [title, ""]
    output.append(
        f"{'PERIOD':<14} {'SESS':>6} {'TURNS':>7} {'CALLS':>7} "
        f"{'TOKENS':>13} {'CACHE':>7} {'COST':>12}"
    )
    output.append("─" * 72)
    for row in rows:
        input_tokens = _int(row.get("input_tokens"))
        cached = _int(row.get("cached_input_tokens"))
        cache_rate = cached / input_tokens * 100 if input_tokens else 0
        output.append(
            f"{str(row.get('period_start') or 'Unknown'):<14} "
            f"{_int(row.get('sessions')):>6} {_int(row.get('turns')):>7} "
            f"{_int(row.get('calls')):>7} {_tokens(_int(row.get('total_tokens'))):>13} "
            f"{cache_rate:>6.1f}% {_money(row.get('cost_usd')):>12}"
        )
    if not rows:
        output.append("No imported usage.")
    text = "\n".join(output)
    return f"{BG}{LIGHT}{text}{RESET}" if color else text


def render_network(
    rows: Sequence[Mapping[str, object]],
    flows: Sequence[Mapping[str, object]],
    *,
    period: str,
    username: str,
    project: str | None = None,
    color: bool | None = None,
    width: int | None = None,
) -> str:
    """Render response timings and content-free connection diagnostics."""
    color = _supports_color() if color is None else color
    width = max(80, min(width or shutil.get_terminal_size((110, 30)).columns, 132))
    inner = width - 2
    lines: list[str] = []

    def styled(text: str, style: str = LIGHT, panel: bool = False) -> str:
        if not color:
            return text
        return f"{PANEL if panel else BG}{style}{text}{RESET}"

    def frame(text: str = "", style: str = LIGHT, panel: bool = False) -> str:
        clipped = text[: inner - 2]
        return styled(f"│ {clipped:<{inner - 2}} │", style, panel)

    ttft = [float(row["ttft_ms"]) for row in rows if row.get("ttft_ms") is not None]
    e2e = [float(row["e2e_ms"]) for row in rows if row.get("e2e_ms") is not None]
    rates = [_output_rate(row) for row in rows]
    usable_rates = [rate for rate in rates if rate is not None]
    exact_rates = sum(row.get("exact_output_tps") is not None for row in rows)

    lines.append(styled("╭" + "─" * (inner - 1) + "╮", BLUE))
    title = f"NETWORK & RESPONSE  ● LOCAL  │  {period}  │  OS user {username}"
    if project:
        title += f"  │  project {project}"
    lines.append(frame(title, CYAN))
    lines.append(styled("├" + "─" * (inner - 1) + "┤", BLUE))
    lines.append(frame(
        f"Samples {len(rows)} turns  ·  timed {len(e2e)}  ·  speed {len(usable_rates)} "
        f"({exact_rates} exact, {len(usable_rates) - exact_rates} estimated)",
        LIGHT,
        True,
    ))
    lines.append(frame(_timing_summary("First token", ttft), MUTED, True))
    lines.append(frame(_timing_summary("Complete", e2e), MUTED, True))
    lines.append(frame(_rate_summary("Output speed", usable_rates), MUTED, True))
    lines.append(styled("├" + "─" * (inner - 1) + "┤", BLUE))
    lines.append(frame(f"{'TIME':<10} {'MODEL':<25} {'OUTPUT':>9} {'FIRST':>10} {'COMPLETE':>10} {'TOK/S':>9}", MUTED))
    for row, rate in list(zip(rows, rates))[:8]:
        rate_text = "N/A" if rate is None else f"{rate:.1f}{'' if row.get('exact_output_tps') is not None else '*'}"
        lines.append(frame(
            f"{str(row.get('local_time') or 'N/A')[:10]:<10} "
            f"{str(row.get('model') or 'Unknown')[:25]:<25} "
            f"{_tokens(_int(row.get('output_tokens'))):>9} "
            f"{_duration(row.get('ttft_ms')):>10} {_duration(row.get('e2e_ms')):>10} {rate_text:>9}",
            LIGHT,
        ))
    if not rows:
        lines.append(frame("No response timing samples for today. Press r to refresh local records.", YELLOW))
    lines.append(styled("├" + "─" * (inner - 1) + "┤", BLUE))
    if flows:
        flow = flows[0]
        destination = str(flow.get("destination_host") or flow.get("destination_ip") or "Unknown")
        if flow.get("success") is None:
            status = "UNKNOWN"
        elif bool(flow.get("success")):
            status = "OK"
        else:
            status = f"FAILED ({flow.get('error_type') or 'connection error'})"
        lines.append(frame(
            f"Latest connection · {status} · {destination}  DNS {_duration(flow.get('dns_ms'))}  "
            f"TCP {_duration(flow.get('tcp_ms'))}  TLS {_duration(flow.get('tls_ms'))}  "
            f"TTFB {_duration(flow.get('ttfb_ms'))}",
            CYAN,
        ))
    else:
        lines.append(frame("Connection setup · no probe/capture samples yet; response timing above still works.", MUTED))
    lines.append(frame("* Estimated speed = output tokens / (complete time - first-token time); tool time may be included.", YELLOW))
    lines.append(styled("╰" + "─" * (inner - 1) + "╯", BLUE))
    return "\n".join(lines)


def _output_rate(row: Mapping[str, object]) -> float | None:
    exact = row.get("exact_output_tps")
    if exact is not None and float(exact) >= 0:
        return float(exact)
    ttft = row.get("ttft_ms")
    e2e = row.get("e2e_ms")
    output = _int(row.get("output_tokens"))
    if ttft is None or e2e is None or output <= 0 or float(e2e) <= float(ttft):
        return None
    return output * 1000 / (float(e2e) - float(ttft))


def _percentile(values: Sequence[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = round((len(ordered) - 1) * fraction)
    return ordered[index]


def _timing_summary(label: str, values: Sequence[float]) -> str:
    if not values:
        return f"{label:<14} AVG N/A  ·  P50 N/A  ·  P95 N/A"
    return (
        f"{label:<14} AVG {_duration(sum(values) / len(values))}  ·  "
        f"P50 {_duration(_percentile(values, 0.50))}  ·  P95 {_duration(_percentile(values, 0.95))}"
    )


def _rate_summary(label: str, values: Sequence[float]) -> str:
    if not values:
        return f"{label:<14} AVG N/A  ·  P50 N/A  ·  P95 N/A"
    return (
        f"{label:<14} AVG {sum(values) / len(values):.1f} tok/s  ·  "
        f"P50 {_percentile(values, 0.50):.1f}  ·  P95 {_percentile(values, 0.95):.1f}"
    )


def _bar_line(label: str, value: int, maximum: int, width: int) -> str:
    ratio = min(1.0, value / maximum) if maximum else 0
    filled = round(width * ratio)
    return f"{label:<10} {'█' * filled}{'░' * (width - filled)}  {_tokens(value):>9}  {ratio * 100:5.1f}%"


def _tokens(value: int) -> str:
    absolute = abs(value)
    if absolute >= 1_000_000_000:
        return f"{value / 1_000_000_000:.2f}B"
    if absolute >= 1_000_000:
        return f"{value / 1_000_000:.2f}M"
    if absolute >= 1_000:
        return f"{value / 1_000:.1f}K"
    return str(value)


def _money(value: object) -> str:
    if value is None:
        return "N/A"
    return f"${float(value):.2f} eq"


def _duration(value: object) -> str:
    if value is None:
        return "N/A"
    milliseconds = float(value)
    return f"{milliseconds:.0f}ms" if milliseconds < 1000 else f"{milliseconds / 1000:.2f}s"


def _int(value: object) -> int:
    return int(value or 0)


def _supports_color() -> bool:
    return sys.stdout.isatty() and os.environ.get("NO_COLOR") is None
