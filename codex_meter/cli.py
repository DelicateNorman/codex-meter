from __future__ import annotations

import argparse
import csv
import json
import os
import sys
import threading
import time
from datetime import date, timedelta
from pathlib import Path
from typing import Iterable, Mapping, Sequence

from . import __version__
from .collectors.session_jsonl import SessionJsonlCollector, discover_rollouts
from .config import (
    initialize_home,
    load_identity,
    load_remote_hosts,
    update_account_identity,
    update_remote_hosts,
    validate_remote_host,
)
from .interactive import run_interactive
from .pricing import PricingCatalog
from .quota import QuotaUnavailable, WeeklyQuota, read_weekly_quotas
from .storage import Storage
from .tui import render_history, render_models, render_network, render_overview


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="codex-meter", description="Local-first Codex usage observability")
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    parser.add_argument("--home", type=Path, default=_meter_home(), help="data directory (default: ~/.codex-meter)")
    parser.add_argument("--db", type=Path, help="SQLite path (default: <home>/meter.db)")
    parser.add_argument("--no-color", action="store_true", help="disable ANSI colors")
    sub = parser.add_subparsers(dest="command")

    import_parser = sub.add_parser("import", help="import Codex rollout JSONL history")
    import_parser.add_argument("path", nargs="?", type=Path, default=_codex_home() / "sessions")
    import_parser.add_argument("--force", action="store_true", help="parse unchanged files again (events remain deduplicated)")

    today = sub.add_parser("today", help="show today's overview")
    today.add_argument("--refresh", action="store_true", help="import changed rollout files first")
    today.add_argument("--account", help="optional configured account label")
    today.add_argument("--project", help="optional project directory name")

    summary = sub.add_parser("summary", help="show day, week, month, or all-time overview")
    summary.add_argument("--period", choices=("day", "week", "month", "all"), default="day")
    summary.add_argument("--date", help="anchor local date in YYYY-MM-DD (default: today)")
    summary.add_argument("--account", help="optional configured account label")
    summary.add_argument("--project", help="optional project directory name")
    summary.add_argument("--refresh", action="store_true", help="import changed rollout files first")

    history = sub.add_parser("history", help="group all usage since first use by day, week, or month")
    history.add_argument("--group", choices=("day", "week", "month"), default="day")
    history.add_argument("--account", help="optional configured account label")
    history.add_argument("--project", help="optional project directory name")
    history.add_argument("--refresh", action="store_true", help="import changed rollout files first")

    account = sub.add_parser("account", help="optional manual account labels (disabled by default)")
    account_sub = account.add_subparsers(dest="account_command", required=True)
    account_sub.add_parser("status", help="show account tracking state")
    account_enable = account_sub.add_parser("enable", help="enable tracking for future sessions")
    account_enable.add_argument("label", help="non-secret label such as personal or work")
    account_set = account_sub.add_parser("set", help="switch the label used for future sessions")
    account_set.add_argument("label", help="non-secret label such as personal or work")
    account_sub.add_parser("disable", help="stop labeling future sessions")
    account_sub.add_parser("list", help="show labels already present in local history")
    account_claim = account_sub.add_parser("claim-unassigned", help="explicitly label all unassigned history for this OS user")
    account_claim.add_argument("label")

    remote = sub.add_parser("remote", help="aggregate Codex history from SSH hosts")
    remote_sub = remote.add_subparsers(dest="remote_command", required=True)
    remote_sub.add_parser("list", help="show configured SSH hosts")
    remote_add = remote_sub.add_parser("add", help="add and verify an SSH config alias")
    remote_add.add_argument("host", help="host alias from ~/.ssh/config")
    remote_remove = remote_sub.add_parser("remove", help="stop syncing an SSH host")
    remote_remove.add_argument("host", help="configured SSH alias")
    remote_sync = remote_sub.add_parser("sync", help="sync all configured hosts, or one host")
    remote_sync.add_argument("host", nargs="?", help="optional SSH alias")
    remote_sync.add_argument("--force", action="store_true", help="parse unchanged remote files again")
    remote_test = remote_sub.add_parser("test", help="test access to remote Codex history")
    remote_test.add_argument("host", help="SSH alias to test")

    models = sub.add_parser("models", help="show Model × Reasoning Effort aggregates")
    models.add_argument("--date", help="local date in YYYY-MM-DD; omit for all time")

    sessions = sub.add_parser("sessions", help="show recent sessions")
    sessions.add_argument("--limit", type=int, default=20)

    perf = sub.add_parser("perf", help="show OTLP latency P50/P95 and throughput inputs")
    perf.add_argument("--date", help="local date in YYYY-MM-DD; omit for all time")

    cache = sub.add_parser("cache", help="show cache reuse, savings, context amplification, and retry tax")
    cache.add_argument("--date", help="local date in YYYY-MM-DD; omit for all time")

    projects = sub.add_parser("projects", help="show per-project usage and compactions")
    projects.add_argument("--date", help="local date in YYYY-MM-DD; omit for all time")

    providers = sub.add_parser("providers", help="show provider usage attribution")
    providers.add_argument("--date", help="local date in YYYY-MM-DD; omit for all time")

    agents = sub.add_parser("agents", help="show root/subagent usage attribution")
    agents.add_argument("--date", help="local date in YYYY-MM-DD; omit for all time")

    tools = sub.add_parser("tools", help="show tool timing and success aggregates")
    tools.add_argument("--date", help="local date in YYYY-MM-DD; omit for all time")

    waterfall = sub.add_parser("waterfall", help="show calls and tools for one Codex turn")
    waterfall.add_argument("turn_id")

    watch = sub.add_parser("watch", help="refresh rollout history and redraw the live dashboard")
    watch.add_argument("--interval", type=float, default=2.0)
    watch.add_argument("--iterations", type=int, help="stop after N redraws (useful for scripts/tests)")

    sub.add_parser("statusline", help="print one compact usage line for shell/footer integrations")

    otel = sub.add_parser("otel", help="localhost OTLP/HTTP JSON collector")
    otel_sub = otel.add_subparsers(dest="otel_command", required=True)
    otel_serve = otel_sub.add_parser("serve", help="serve /v1/logs, /v1/metrics, and /v1/traces")
    otel_serve.add_argument("--bind", default="127.0.0.1")
    otel_serve.add_argument("--port", type=int, default=4318)
    otel_serve.add_argument("--token", help="optional bearer token")
    otel_config = otel_sub.add_parser("config", help="print matching Codex config.toml snippet")
    otel_config.add_argument("--host", default="127.0.0.1")
    otel_config.add_argument("--port", type=int, default=4318)

    app_server = sub.add_parser("app-server", help="ingest or transparently proxy Codex App Server JSONL")
    app_sub = app_server.add_subparsers(dest="app_command", required=True)
    app_ingest = app_sub.add_parser("ingest", help="ingest a captured server JSONL stream")
    app_ingest.add_argument("path", help="JSONL path or - for stdin")
    app_proxy = app_sub.add_parser("proxy", help="stdio proxy; point an App Server client at this command")
    app_proxy.add_argument("server_command", nargs=argparse.REMAINDER, help="optional command after --")

    network = sub.add_parser("network", help="content-free network diagnostics")
    network_sub = network.add_subparsers(dest="network_command", required=True)
    network_probe = network_sub.add_parser("probe", help="measure DNS/TCP/TLS setup")
    network_probe.add_argument("host", nargs="?", default="api.openai.com")
    network_probe.add_argument("--port", type=int, default=443)
    network_capture = network_sub.add_parser("capture", help="capture only packet direction/length with tcpdump")
    network_capture.add_argument("--host", action="append", default=[])
    network_capture.add_argument("--port", type=int, default=443)
    network_capture.add_argument("--interface", help="capture interface (default: auto-detect)")
    network_capture.add_argument("--duration", type=float, default=15.0)
    network_capture.add_argument("--packet-limit", type=int, default=5000)
    network_show = network_sub.add_parser("show", help="show recent saved network flows")
    network_show.add_argument("--limit", type=int, default=30)

    proxy = sub.add_parser("proxy", help="run local metadata or explicit TLS diagnostic proxies")
    proxy_sub = proxy.add_subparsers(dest="proxy_command", required=True)
    tunnel = proxy_sub.add_parser("tunnel", help="HTTP CONNECT tunnel; TLS content stays opaque")
    tunnel.add_argument("--bind", default="127.0.0.1")
    tunnel.add_argument("--port", type=int, default=8899)
    reverse = proxy_sub.add_parser("reverse", help="HTTP reverse proxy with SSE timing only")
    reverse.add_argument("--bind", default="127.0.0.1")
    reverse.add_argument("--port", type=int, default=8900)
    reverse.add_argument("--upstream", default="https://api.openai.com")
    tls_init = proxy_sub.add_parser("tls-init", help="create a 30-day local diagnostic CA and localhost cert")
    tls_init.add_argument("--directory", type=Path)
    tls = proxy_sub.add_parser("tls", help="explicit HTTPS termination/re-encryption diagnostic mode")
    tls.add_argument("--bind", default="127.0.0.1")
    tls.add_argument("--port", type=int, default=8901)
    tls.add_argument("--upstream", default="https://api.openai.com")
    tls.add_argument("--directory", type=Path)
    tls.add_argument("--acknowledge-sensitive", action="store_true", help="confirm local TLS termination is intentional")

    export = sub.add_parser("export", help="export per-call metrics without payloads")
    export.add_argument("--from", dest="from_date")
    export.add_argument("--to", dest="to_date")
    export.add_argument("--session")
    export.add_argument("--format", choices=("json", "jsonl", "csv"), default="json")
    export.add_argument("--output", type=Path)

    sub.add_parser("doctor", help="detect available Codex data sources and schema capabilities")
    sub.add_parser("pricing", help="list versioned pricing catalog")
    sub.add_parser("demo", help="render a deterministic TUI demo")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    _configure_windows_stdio()
    args = build_parser().parse_args(argv)
    if args.command == "demo":
        print(_demo(not args.no_color))
        return 0

    initialize_home(args.home)
    if args.command == "account" and args.account_command in ("enable", "set", "disable"):
        if args.account_command == "disable":
            identity = update_account_identity(args.home, enabled=False, label=None)
        else:
            identity = update_account_identity(args.home, enabled=True, label=args.label)
        print(_identity_status(identity))
        return 0
    identity = load_identity(args.home)
    remote_hosts = load_remote_hosts(args.home)
    db_path = args.db or args.home / "meter.db"
    catalog = PricingCatalog.bundled(args.home / "pricing.json" if (args.home / "pricing.json").exists() else None)
    with Storage(
        db_path,
        owner_uid=identity.uid,
        owner_username=identity.username,
        account_label=identity.account_label,
    ) as storage:
        storage.migrate()
        storage.sync_pricing(catalog)

        if args.command == "import":
            return _import(storage, catalog, args.path, args.force)
        if args.command == "doctor":
            return _doctor(storage)
        if args.command == "pricing":
            return _pricing(catalog)
        if args.command == "account":
            return _account(storage, identity, args)
        if args.command == "remote":
            return _remote(storage, catalog, args.home, args)
        if args.command == "models":
            rows = storage.model_breakdown(args.date)
            print(render_models(rows, color=not args.no_color and sys.stdout.isatty()))
            return 0
        if args.command == "perf":
            return _perf(storage, args.date)
        if args.command == "cache":
            return _cache(storage, catalog, args.date)
        if args.command == "projects":
            return _projects(storage, args.date)
        if args.command == "providers":
            return _providers(storage, args.date)
        if args.command == "agents":
            return _agents(storage, args.date)
        if args.command == "tools":
            return _tools(storage, args.date)
        if args.command == "waterfall":
            return _waterfall(storage, args.turn_id)
        if args.command == "watch":
            return _watch(storage, catalog, args, not args.no_color, remote_hosts)
        if args.command == "statusline":
            return _statusline(storage)
        if args.command == "otel":
            return _otel(storage, args)
        if args.command == "app-server":
            return _app_server(storage, catalog, args)
        if args.command == "network":
            return _network(storage, args)
        if args.command == "proxy":
            return _proxy(storage, args, args.home)
        if args.command == "sessions":
            return _sessions(storage, args.limit)
        if args.command == "export":
            return _export(storage, args)

        if args.command in ("summary", "history") and args.refresh:
            _import(storage, catalog, _codex_home() / "sessions", force=False, quiet=True)
            _sync_remotes(storage, catalog, remote_hosts, quiet=True)
        if args.command == "history":
            return _history(storage, args.group, args.account, args.project, remote_hosts)
        if args.command == "summary":
            return _summary(
                storage, args.period, args.date, args.account, args.project, not args.no_color,
                remote_hosts,
            )

        # The default interactive dashboard and `today` both reflect changed local history.
        if args.command is None or getattr(args, "refresh", False):
            _import(storage, catalog, _codex_home() / "sessions", force=False, quiet=True)
        if args.command == "today" and args.refresh:
            _sync_remotes(storage, catalog, remote_hosts, quiet=True)
        if args.command is None and sys.stdin.isatty() and sys.stdout.isatty():
            return _interactive_dashboard(
                storage,
                catalog,
                remote_hosts=remote_hosts,
                color=not args.no_color,
            )
        if args.command is None:
            _sync_remotes(storage, catalog, remote_hosts, quiet=True)
        selected_day = date.today().isoformat()
        account_filter = getattr(args, "account", None)
        project_filter = getattr(args, "project", None)
        overview = dict(storage.overview(
            selected_day, account=account_filter, project=project_filter,
        ))
        rows = [dict(row) for row in storage.model_breakdown(
            selected_day, account=account_filter, project=project_filter,
        )]
        today_label = f"TODAY · {selected_day}"
        if project_filter:
            today_label += f" · PROJECT {project_filter}"
        print(
            render_overview(
                overview,
                rows,
                period=today_label,
                color=not args.no_color and sys.stdout.isatty(),
                source_label=(f"LOCAL + {len(remote_hosts)} REMOTE" if remote_hosts else "LOCAL"),
            )
        )
        return 0


def _import(
    storage: Storage,
    catalog: PricingCatalog,
    path: Path,
    force: bool,
    quiet: bool = False,
) -> int:
    files = discover_rollouts(path)
    collector = SessionJsonlCollector(catalog)
    parsed_count = skipped = failed = turns = calls = tools = malformed = duplicates = 0
    for rollout in files:
        if not force and storage.file_is_current(rollout):
            skipped += 1
            continue
        try:
            parsed = collector.collect_file(rollout)
            inserted = storage.import_session(parsed, rollout)
        except (OSError, ValueError, json.JSONDecodeError) as error:
            failed += 1
            if not quiet:
                print(f"warning: {rollout}: {error}", file=sys.stderr)
            continue
        parsed_count += 1
        turns += inserted[0]
        calls += inserted[1]
        tools += inserted[2]
        malformed += parsed.malformed_lines
        duplicates += parsed.duplicate_usage_events
    if not quiet:
        print(
            f"Imported {parsed_count} file(s), skipped {skipped}, failed {failed}; "
            f"{turns} turns, {calls} LLM calls, {tools} tools; "
            f"ignored {duplicates} duplicate usage event(s), {malformed} malformed line(s)."
        )
    return 1 if failed else 0


def _remote(
    storage: Storage,
    catalog: PricingCatalog,
    home: Path,
    args: argparse.Namespace,
) -> int:
    from .remote import RemoteError, list_remote_rollouts

    command = args.remote_command
    configured = list(load_remote_hosts(home))
    if command == "list":
        if not configured:
            print("No remote sources configured. Add one with: codex-meter remote add <ssh-alias>")
            return 0
        print("REMOTE SSH SOURCE")
        for host in configured:
            print(f"  {host}")
        return 0

    if command == "sync" and args.host is None:
        if not configured:
            print("No remote sources configured. Add one with: codex-meter remote add <ssh-alias>")
            return 0
        return _sync_remotes(storage, catalog, configured, force=args.force, quiet=False)

    try:
        host = validate_remote_host(args.host)
    except ValueError as error:
        print(str(error), file=sys.stderr)
        return 2

    if command == "remove":
        if host not in configured:
            print(f"Remote source {host!r} is not configured.", file=sys.stderr)
            return 1
        configured.remove(host)
        update_remote_hosts(home, configured)
        print(f"Removed remote source {host}. Previously imported metrics remain in history.")
        return 0

    if command in ("add", "test"):
        try:
            files = list_remote_rollouts(host)
        except RemoteError as error:
            print(f"Remote check failed: {error}", file=sys.stderr)
            print(f"Tip: first confirm that `ssh {host}` works without an interactive password prompt.", file=sys.stderr)
            return 1
        if command == "test":
            print(f"Connected to {host}; found {len(files)} Codex Rollout file(s).")
            return 0
        if host not in configured:
            configured.append(host)
            update_remote_hosts(home, configured)
        print(f"Added remote source {host}; found {len(files)} Codex Rollout file(s).")
        print("Syncing metadata now; raw prompts and responses will not be saved locally.")

    return _sync_remotes(
        storage,
        catalog,
        (host,),
        force=bool(getattr(args, "force", False)),
        quiet=False,
    )


def _sync_remotes(
    storage: Storage,
    catalog: PricingCatalog,
    hosts: Sequence[str],
    *,
    force: bool = False,
    quiet: bool = False,
) -> int:
    from .remote import RemoteError, sync_remote_rollouts

    failed = 0
    for host in hosts:
        try:
            result = sync_remote_rollouts(storage, catalog, host, force=force)
        except (RemoteError, OSError, ValueError) as error:
            failed += 1
            if not quiet:
                print(f"Remote sync failed: {error}", file=sys.stderr)
            continue
        if not quiet:
            print(
                f"Synced {host}: imported {result.imported_files}, "
                f"unchanged {result.skipped_files}, failed {result.failed_files}; "
                f"{result.inserted_turns} turns, {result.inserted_calls} LLM calls."
            )
        failed += int(result.failed_files > 0)
    return 1 if failed else 0


def _doctor(storage: Storage) -> int:
    from .doctor import run_doctor

    print("Codex Meter Doctor\n")
    status_symbols = {"yes": "✓", "no": "✗", "disabled": "○", "unknown": "?", "experimental": "△"}
    for check in run_doctor(_codex_home(), storage):
        suffix = f"  {check.detail}" if check.detail else ""
        print(f"{check.name:<28} {status_symbols.get(check.status, '?')} {check.status}{suffix}")
    counts = storage.counts()
    print(f"\nDatabase: {storage.path}")
    print("Rows: " + ", ".join(f"{name}={value}" for name, value in counts.items()))
    print("Privacy: prompts/responses/tool output/headers are never imported")
    return 0


def _pricing(catalog: PricingCatalog) -> int:
    print(f"{'MODEL':<22} {'EFFECTIVE':<22} {'INPUT':>9} {'CACHED':>9} {'WRITE':>9} {'OUTPUT':>9}  VERSION")
    for item in catalog.entries:
        print(
            f"{item.model:<22} {item.effective_from:<22} {item.input_per_million:>9.3f} "
            f"{item.cached_input_per_million:>9.3f} {item.cache_write_per_million:>9.3f} "
            f"{item.output_per_million:>9.3f}  {item.version}"
        )
    print("USD per 1M tokens. Reasoning tokens are included in output and are not double-counted.")
    return 0


def _sessions(storage: Storage, limit: int) -> int:
    print(f"{'SESSION':<16} {'PROJECT':<24} {'STARTED':<21} {'TURNS':>6} {'CALLS':>6} {'TOKENS':>12} {'CACHE':>7} {'COST':>11}")
    for row in storage.sessions(max(1, limit)):
        input_tokens = int(row["input_tokens"] or 0)
        hit = int(row["cached_input_tokens"] or 0) / input_tokens * 100 if input_tokens else 0
        cost = "N/A" if row["cost_usd"] is None else f"${float(row['cost_usd']):.2f} eq"
        print(
            f"{row['codex_thread_id'][:15]:<16} {str(row['project_name'] or 'Unknown')[:24]:<24} "
            f"{str(row['started_at'] or 'Unknown')[:20]:<21} {int(row['turns'] or 0):>6} "
            f"{int(row['calls'] or 0):>6} {int(row['total_tokens'] or 0):>12,} {hit:>6.1f}% {cost:>11}"
        )
    return 0


def _summary(
    storage: Storage,
    period: str,
    anchor_text: str | None,
    account: str | None,
    project: str | None,
    color: bool,
    remote_hosts: Sequence[str] = (),
) -> int:
    try:
        start, end, label = _period_bounds(period, anchor_text)
    except ValueError as error:
        print(f"invalid date: {error}", file=sys.stderr)
        return 2
    overview = dict(storage.overview_range(start, end, account=account, project=project))
    rows = [dict(row) for row in storage.model_breakdown_range(
        start, end, account=account, project=project,
    )]
    if account:
        label += f" · ACCOUNT {account}"
    if project:
        label += f" · PROJECT {project}"
    print(render_overview(
        overview, rows, period=label,
        color=color and sys.stdout.isatty(),
        source_label=(f"LOCAL + {len(remote_hosts)} REMOTE" if remote_hosts else "LOCAL"),
    ))
    return 0


def _history(
    storage: Storage,
    group: str,
    account: str | None,
    project: str | None,
    remote_hosts: Sequence[str] = (),
) -> int:
    scope = f"Usage history by {group} · OS user {storage.owner_username}"
    if remote_hosts:
        scope += f" · local + {len(remote_hosts)} remote"
    if account:
        scope += f" · account {account}"
    if project:
        scope += f" · project {project}"
    print(scope)
    print(f"{'PERIOD START':<14} {'SESS':>7} {'TURNS':>7} {'CALLS':>7} {'INPUT':>14} {'CACHED':>14} {'OUTPUT':>12} {'TOKENS':>14} {'COST':>12}")
    rows = storage.usage_history(group, account=account, project=project)
    for row in rows:
        cost = "N/A" if row["cost_usd"] is None else f"${float(row['cost_usd']):.2f} eq"
        print(
            f"{str(row['period_start'] or 'Unknown'):<14} {int(row['sessions'] or 0):>7} "
            f"{int(row['turns'] or 0):>7} {int(row['calls'] or 0):>7} "
            f"{int(row['input_tokens'] or 0):>14,} {int(row['cached_input_tokens'] or 0):>14,} "
            f"{int(row['output_tokens'] or 0):>12,} {int(row['total_tokens'] or 0):>14,} {cost:>12}"
        )
    if not rows:
        print("No imported usage.")
    return 0


def _interactive_dashboard(
    storage: Storage,
    catalog: PricingCatalog,
    *,
    remote_hosts: Sequence[str] = (),
    color: bool,
) -> int:
    weekly_quotas: tuple[WeeklyQuota, ...] = ()
    quota_message: str | None = "ACCOUNT WEEKLY LIMITS  Loading…"
    quota_loading = False
    remote_message: str | None = "REMOTE SOURCES  Syncing…" if remote_hosts else None
    remote_loading = False
    quota_lock = threading.Lock()
    remote_lock = threading.Lock()
    dashboard_changed = threading.Event()

    def load_quota() -> None:
        nonlocal weekly_quotas, quota_message, quota_loading
        try:
            loaded = read_weekly_quotas()
        except QuotaUnavailable as error:
            with quota_lock:
                weekly_quotas = ()
                quota_message = f"ACCOUNT WEEKLY LIMITS  Unavailable · {error} · press r to retry"
                quota_loading = False
        else:
            with quota_lock:
                weekly_quotas = loaded
                quota_message = None
                quota_loading = False
        dashboard_changed.set()

    def start_quota_refresh(*, notify: bool = True) -> None:
        nonlocal quota_message, quota_loading
        with quota_lock:
            if quota_loading:
                return
            quota_loading = True
            quota_message = "ACCOUNT WEEKLY LIMITS  Loading…"
        if notify:
            dashboard_changed.set()
        threading.Thread(
            target=load_quota,
            name="codex-meter-weekly-quota",
            daemon=True,
        ).start()

    def load_remotes() -> None:
        nonlocal remote_message, remote_loading
        from .remote import RemoteError, sync_remote_rollouts

        imported = 0
        errors: list[str] = []
        try:
            with Storage(
                storage.path,
                owner_uid=storage.owner_uid,
                owner_username=storage.owner_username,
                account_label=storage.account_label,
            ) as remote_storage:
                for host in remote_hosts:
                    try:
                        result = sync_remote_rollouts(remote_storage, catalog, host)
                    except (RemoteError, OSError, ValueError) as error:
                        errors.append(f"{host}: {error}")
                    else:
                        imported += result.imported_files
                        if result.failed_files:
                            errors.append(f"{host}: {result.failed_files} file(s) failed")
        except (OSError, ValueError) as error:
            errors.append(str(error))
        finally:
            with remote_lock:
                if errors:
                    remote_message = "REMOTE SOURCES  " + " · ".join(errors) + " · press r to retry"
                else:
                    names = ", ".join(remote_hosts)
                    remote_message = f"REMOTE SOURCES  {names} synced · {imported} updated"
                remote_loading = False
            dashboard_changed.set()

    def start_remote_refresh(*, notify: bool = True) -> None:
        nonlocal remote_message, remote_loading
        if not remote_hosts:
            return
        with remote_lock:
            if remote_loading:
                return
            remote_loading = True
            remote_message = "REMOTE SOURCES  Syncing " + ", ".join(remote_hosts) + "…"
        if notify:
            dashboard_changed.set()
        threading.Thread(
            target=load_remotes,
            name="codex-meter-remote-sync",
            daemon=True,
        ).start()

    def consume_dashboard_update() -> bool:
        if not dashboard_changed.is_set():
            return False
        dashboard_changed.clear()
        return True

    start_quota_refresh(notify=False)
    start_remote_refresh(notify=False)

    def content(
        view: str,
        width: int,
        use_color: bool,
        project: str | None,
    ) -> str:
        if view == "network":
            start, end, label = _period_bounds("day", None)
            rows = [dict(row) for row in storage.response_performance_range(
                start, end, project=project,
            )]
            flows = [dict(row) for row in storage.recent_network(5, project=project)]
            return render_network(
                rows,
                flows,
                period=label,
                username=storage.owner_username,
                project=project,
                color=use_color,
                width=width,
            )

        if view.startswith("history_"):
            group = view.removeprefix("history_")
            rows = [dict(row) for row in storage.usage_history(group, project=project)]
            return render_history(
                rows,
                group=group,
                username=storage.owner_username,
                project=project,
                color=use_color,
            )

        period = {"today": "day", "week": "week", "month": "month", "all": "all"}.get(view, "day")
        start, end, label = _period_bounds(period, None)
        overview = dict(storage.overview_range(start, end, project=project))
        rows = [dict(row) for row in storage.model_breakdown_range(
            start, end, project=project,
        )]
        if project:
            label += f" · PROJECT {project}"
        with quota_lock:
            current_quotas = weekly_quotas
            current_quota_message = quota_message
        with remote_lock:
            current_remote_message = remote_message
        return render_overview(
            overview,
            rows,
            period=label,
            color=use_color,
            width=width,
            weekly_quotas=current_quotas,
            quota_message=current_quota_message,
            source_label=(
                f"LOCAL + {len(remote_hosts)} REMOTE"
                if remote_hosts else "LOCAL"
            ),
            source_message=current_remote_message,
        )

    def refresh() -> None:
        result = _import(storage, catalog, _codex_home() / "sessions", force=False, quiet=True)
        start_quota_refresh()
        start_remote_refresh()
        if result:
            raise OSError("one or more rollout files could not be imported")

    return run_interactive(
        content,
        refresh,
        storage.project_names,
        consume_dashboard_update,
        color=color,
    )


def _account(storage: Storage, identity: object, args: argparse.Namespace) -> int:
    if args.account_command == "status":
        print(_identity_status(identity))
        print("Account labels are manual metadata; Codex credentials and auth files are never read.")
        return 0
    if args.account_command == "claim-unassigned":
        try:
            count = storage.claim_unassigned_account(args.label)
        except ValueError as error:
            print(str(error), file=sys.stderr)
            return 2
        print(f"Assigned {count} existing session(s) for OS user {storage.owner_username} to account {args.label!r}.")
        return 0
    if args.account_command == "list":
        print(f"{'ACCOUNT':<24} {'SESSIONS':>9} {'CALLS':>9} {'TOKENS':>14} {'FIRST':<21} {'LAST':<21} {'COST':>12}")
        for row in storage.account_breakdown():
            cost = "N/A" if row["cost_usd"] is None else f"${float(row['cost_usd']):.2f} eq"
            print(
                f"{str(row['account'])[:24]:<24} {int(row['sessions'] or 0):>9} "
                f"{int(row['calls'] or 0):>9} {int(row['total_tokens'] or 0):>14,} "
                f"{str(row['first_used_at'] or 'N/A')[:20]:<21} {str(row['last_used_at'] or 'N/A')[:20]:<21} {cost:>12}"
            )
        return 0
    return 2


def _identity_status(identity: object) -> str:
    enabled = bool(getattr(identity, "account_tracking", False))
    label = getattr(identity, "account_label", None)
    uid = getattr(identity, "uid", None)
    username = getattr(identity, "username", "unknown")
    state = f"enabled · current label {label!r}" if enabled and label else "disabled"
    return f"OS user: {username} (uid {uid if uid is not None else 'N/A'})\nAccount tracking: {state}"


def _period_bounds(period: str, anchor_text: str | None) -> tuple[str | None, str | None, str]:
    anchor = date.fromisoformat(anchor_text) if anchor_text else date.today()
    if period == "all":
        return None, None, "ALL TIME · SINCE FIRST USE"
    if period == "day":
        value = anchor.isoformat()
        return value, value, f"DAY · {value}"
    if period == "week":
        start = anchor - timedelta(days=anchor.weekday())
        end = start + timedelta(days=6)
        return start.isoformat(), end.isoformat(), f"WEEK · {start.isoformat()} → {end.isoformat()}"
    if period == "month":
        start = anchor.replace(day=1)
        if start.month == 12:
            next_month = start.replace(year=start.year + 1, month=1)
        else:
            next_month = start.replace(month=start.month + 1)
        end = next_month - timedelta(days=1)
        return start.isoformat(), end.isoformat(), f"MONTH · {start:%Y-%m}"
    raise ValueError(f"unknown period {period!r}")


def _perf(storage: Storage, selected_date: str | None) -> int:
    from .analytics import PERFORMANCE_METRICS, performance_summary

    rows = performance_summary(storage.metric_points(selected_date, PERFORMANCE_METRICS))
    print(f"{'METRIC':<58} {'COUNT':>8} {'AVG':>12} {'P50':>12} {'P95':>12} {'TPS':>9}")
    for row in rows:
        tps_text = "N/A" if row["tps"] is None else f"{row['tps']:.2f}"
        print(
            f"{row['name']:<58} {int(row['count'] or 0):>8} "
            f"{_milliseconds(row['avg']):>12} {_milliseconds(row['p50']):>12} {_milliseconds(row['p95']):>12} "
            f"{tps_text:>9}"
        )
    if not rows:
        print("No OTLP performance points. Run `codex-meter otel config`, apply it, then `codex-meter otel serve`.")
    print("Histogram percentiles are bucket approximations; AVG uses the exported sum/count.")
    return 0


def _cache(storage: Storage, catalog: PricingCatalog, selected_date: str | None) -> int:
    from .analytics import cache_summary, context_and_retry_summary

    rows = storage.usage_calls(selected_date)
    cache = cache_summary(rows, catalog)
    context = context_and_retry_summary(rows)
    telemetry_retry = storage.telemetry_retry_summary(selected_date)
    print("Cache and efficiency")
    print(f"Input tokens          {cache['input_tokens']:>14,}")
    print(f"Cached input          {cache['cached_input_tokens']:>14,}")
    print(f"Cache write           {cache['cache_write_tokens']:>14,}")
    print(f"Reuse rate            {cache['reuse_rate'] * 100:>13.1f}%")
    print(f"API-equiv cost        ${cache['observed_cost_usd']:>13.4f}")
    print(f"Without cache         ${cache['without_cache_usd']:>13.4f}")
    print(f"Estimated savings     ${cache['savings_usd']:>13.4f}")
    amplification = context['average_context_amplification']
    maximum = context['max_context_amplification']
    print(f"Context amplification {('N/A' if amplification is None else f'{amplification:.2f}x'):>14}")
    print(f"Maximum amplification {('N/A' if maximum is None else f'{maximum:.2f}x'):>14}")
    print(f"Explicit retry calls  {context['retry_calls']:>14,}")
    print(f"Retry tokens          {context['retry_tokens']:>14,}")
    print(f"Retry API-equiv       ${context['retry_cost_usd']:>13.4f}")
    print(f"OTel retry attempts   {int(telemetry_retry['attempts']):>14,}")
    print(f"OTel retry time       {_milliseconds(telemetry_retry['duration_ms']):>14}")
    if cache["unpriced_calls"]:
        print(f"Unpriced calls: {cache['unpriced_calls']} (excluded from cost/savings).")
    return 0


def _projects(storage: Storage, selected_date: str | None) -> int:
    print(f"{'PROJECT':<30} {'SESS':>5} {'TURN':>5} {'CALL':>5} {'TOKENS':>13} {'CACHE':>7} {'RETRY':>11} {'COMPACT':>7} {'COST':>11}")
    for row in storage.project_breakdown(selected_date):
        input_tokens = int(row["input_tokens"] or 0)
        reuse = int(row["cached_input_tokens"] or 0) / input_tokens * 100 if input_tokens else 0
        cost = "N/A" if row["cost_usd"] is None else f"${float(row['cost_usd']):.2f} eq"
        print(
            f"{str(row['project'])[:30]:<30} {int(row['sessions']):>5} {int(row['turns']):>5} "
            f"{int(row['calls']):>5} {int(row['total_tokens'] or 0):>13,} {reuse:>6.1f}% "
            f"{int(row['retry_tokens'] or 0):>11,} {int(row['compactions'] or 0):>7} {cost:>11}"
        )
    return 0


def _providers(storage: Storage, selected_date: str | None) -> int:
    print(f"{'PROVIDER':<24} {'SESSIONS':>9} {'CALLS':>8} {'TOKENS':>14} {'CACHE':>8} {'COST':>12}")
    for row in storage.provider_breakdown(selected_date):
        input_tokens = int(row["input_tokens"] or 0)
        reuse = int(row["cached_input_tokens"] or 0) / input_tokens * 100 if input_tokens else 0
        cost = "N/A" if row["cost_usd"] is None else f"${float(row['cost_usd']):.2f} eq"
        print(f"{str(row['provider'])[:24]:<24} {int(row['sessions']):>9} {int(row['calls']):>8} {int(row['total_tokens'] or 0):>14,} {reuse:>7.1f}% {cost:>12}")
    return 0


def _agents(storage: Storage, selected_date: str | None) -> int:
    print(f"{'ROLE':<24} {'SESSIONS':>9} {'TURNS':>8} {'CALLS':>8} {'TOKENS':>14} {'COST':>12}")
    for row in storage.agent_breakdown(selected_date):
        cost = "N/A" if row["cost_usd"] is None else f"${float(row['cost_usd']):.2f} eq"
        print(f"{str(row['role'])[:24]:<24} {int(row['sessions']):>9} {int(row['turns']):>8} {int(row['calls']):>8} {int(row['total_tokens'] or 0):>14,} {cost:>12}")
    return 0


def _tools(storage: Storage, selected_date: str | None) -> int:
    from .analytics import weighted_percentile

    durations = storage.tool_durations(selected_date)
    print(f"{'TOOL':<42} {'CALLS':>7} {'SUCCESS':>9} {'AVG':>10} {'P50':>10} {'P95':>10} {'TOTAL':>11}")
    for row in storage.tool_breakdown(selected_date):
        calls = int(row["calls"] or 0)
        known = int(row["known_outcomes"] or 0)
        success_text = f"{int(row['successes'] or 0) / known * 100:.1f}%" if known else "N/A"
        weighted = [(value, 1) for value in durations.get(str(row["tool_name"]), [])]
        print(
            f"{str(row['tool_name'])[:42]:<42} {calls:>7} {success_text:>9} "
            f"{_milliseconds(row['avg_ms']):>10} {_milliseconds(weighted_percentile(weighted, .50)):>10} "
            f"{_milliseconds(weighted_percentile(weighted, .95)):>10} {_milliseconds(row['total_ms']):>11}"
        )
    return 0


def _waterfall(storage: Storage, turn_id: str) -> int:
    turn, calls, tools = storage.turn_waterfall(turn_id)
    if turn is None:
        print(f"Turn not found: {turn_id}", file=sys.stderr)
        return 1
    print(f"Turn {turn_id} · {turn['project_name'] or 'Unknown'} · {turn['status']}")
    print(f"TTFT={_milliseconds(turn['ttft_ms'])}  TTFM={_milliseconds(turn['ttfm_ms'])}  E2E={_milliseconds(turn['e2e_ms'])}")
    events: list[tuple[str, str, object, object, str]] = []
    for row in calls:
        events.append((str(row["started_at"] or row["completed_at"] or ""), "LLM", row["started_at"], row["completed_at"], str(row["response_id"] or row["model"] or "unknown")))
    for row in tools:
        events.append((str(row["started_at"] or row["completed_at"] or ""), "TOOL", row["started_at"], row["completed_at"], str(row["tool_name"])))
    for _, kind, started, completed, label in sorted(events):
        print(f"{kind:<5} {str(started or 'N/A'):<27} → {str(completed or 'N/A'):<27} {label}")
    return 0


def _watch(
    storage: Storage,
    catalog: PricingCatalog,
    args: argparse.Namespace,
    color: bool,
    remote_hosts: Sequence[str] = (),
) -> int:
    iteration = 0
    try:
        while args.iterations is None or iteration < max(0, args.iterations):
            _import(storage, catalog, _codex_home() / "sessions", force=False, quiet=True)
            _sync_remotes(storage, catalog, remote_hosts, quiet=True)
            selected_day = date.today().isoformat()
            if sys.stdout.isatty() and iteration:
                print("\033[2J\033[H", end="")
            print(render_overview(
                dict(storage.overview(selected_day)),
                [dict(row) for row in storage.model_breakdown(selected_day)],
                period=f"LIVE · {selected_day}", color=color and sys.stdout.isatty(),
                source_label=(f"LOCAL + {len(remote_hosts)} REMOTE" if remote_hosts else "LOCAL"),
            ))
            iteration += 1
            if args.iterations is None or iteration < args.iterations:
                time.sleep(max(0.1, args.interval))
    except KeyboardInterrupt:
        pass
    return 0


def _statusline(storage: Storage) -> int:
    row = storage.overview(date.today().isoformat())
    input_tokens = int(row["input_tokens"] or 0)
    cached = int(row["cached_input_tokens"] or 0)
    reuse = cached / input_tokens * 100 if input_tokens else 0
    cost = "N/A" if row["cost_usd"] is None else f"${float(row['cost_usd']):.2f}eq"
    print(
        f"Codex {int(row['total_tokens'] or 0):,} tok · cache {reuse:.0f}% · "
        f"{int(row['calls'] or 0)} calls · {cost} · TTFT {_milliseconds(row['avg_ttft_ms'])}"
    )
    return 0


def _otel(storage: Storage, args: argparse.Namespace) -> int:
    from .collectors.otlp_http import OtlpServer

    if args.otel_command == "config":
        base = f"http://{args.host}:{args.port}"
        print("[otel]")
        print("log_user_prompt = false")
        print(f'exporter = {{ otlp-http = {{ endpoint = "{base}/v1/logs", protocol = "json" }} }}')
        print(f'trace_exporter = {{ otlp-http = {{ endpoint = "{base}/v1/traces", protocol = "json" }} }}')
        print(f'metrics_exporter = {{ otlp-http = {{ endpoint = "{base}/v1/metrics", protocol = "json" }} }}')
        return 0
    server = OtlpServer((args.bind, args.port), storage, args.token)
    print(f"OTLP collector listening on http://{args.bind}:{server.server_address[1]} (JSON; metadata only)", file=sys.stderr)
    return _serve(server)


def _app_server(storage: Storage, catalog: PricingCatalog, args: argparse.Namespace) -> int:
    from .collectors.app_server import AppServerAdapter, ingest_stream, proxy_stdio

    if args.app_command == "ingest":
        adapter = AppServerAdapter(storage, catalog)
        if args.path == "-":
            ingested, malformed = ingest_stream(sys.stdin, adapter)
        else:
            with Path(args.path).open("r", encoding="utf-8", errors="replace") as stream:
                ingested, malformed = ingest_stream(stream, adapter)
        print(f"Ingested {ingested} structural event(s); ignored {malformed} malformed line(s).")
        return 0
    command = list(args.server_command)
    if command[:1] == ["--"]:
        command = command[1:]
    return proxy_stdio(storage, catalog, command or None)


def _network(storage: Storage, args: argparse.Namespace) -> int:
    from .network import capture_metadata, probe_endpoint

    if args.network_command == "probe":
        flow = probe_endpoint(args.host, args.port)
        storage.insert_network_flow(flow)
        print(
            f"{flow.destination_host}:{flow.destination_port} ip={flow.destination_ip or 'N/A'} "
            f"DNS={_milliseconds(flow.dns_ms)} TCP={_milliseconds(flow.tcp_ms)} "
            f"TLS={_milliseconds(flow.tls_ms)} version={flow.tls_version or 'N/A'} "
            f"ALPN={flow.alpn or 'N/A'} success={flow.success}"
        )
        return 0 if flow.success else 1
    if args.network_command == "capture":
        hosts = args.host or ["api.openai.com", "chatgpt.com"]
        flows, error = capture_metadata(
            hosts, interface=args.interface, port=args.port,
            duration=args.duration, packet_limit=args.packet_limit,
        )
        for flow in flows:
            storage.insert_network_flow(flow)
            print(f"{flow.destination_host} {flow.destination_ip} out={flow.packets_out}/{flow.request_bytes}B in={flow.packets_in}/{flow.response_bytes}B duration={_milliseconds(flow.duration_ms)}")
        if error:
            print(f"capture error: {error}", file=sys.stderr)
            return 1
        print(f"Saved {len(flows)} content-free flow aggregate(s).")
        return 0
    for row in storage.recent_network(args.limit):
        print(
            f"{str(row['started_at'] or 'N/A'):<27} {str(row['mode']):<18} "
            f"{str(row['destination_host'] or row['destination_ip'] or 'N/A'):<28} "
            f"out={int(row['request_bytes'] or 0):>9,}B in={int(row['response_bytes'] or 0):>9,}B "
            f"ttfb={_milliseconds(row['ttfb_ms'])} status={row['http_status'] or 'N/A'}"
        )
    return 0


def _proxy(storage: Storage, args: argparse.Namespace, home: Path) -> int:
    from .network import TunnelProxyServer
    from .proxy import ReverseProxyServer, initialize_tls_material, wrap_server_tls

    if args.proxy_command == "tunnel":
        server = TunnelProxyServer((args.bind, args.port), storage)
        print(f"CONNECT proxy on http://{args.bind}:{server.server_address[1]} (TLS opaque; metadata only)", file=sys.stderr)
        return _serve(server)
    if args.proxy_command == "tls-init":
        paths = initialize_tls_material(args.directory or home / "tls")
        print(f"CA certificate: {paths['ca_cert']}")
        print(f"Leaf certificate: {paths['leaf_cert']}")
        print("Private keys are mode 0600. Trust only the CA certificate, and remove trust after diagnostics.")
        return 0
    if args.proxy_command == "tls" and not args.acknowledge_sensitive:
        print("Refusing TLS termination without --acknowledge-sensitive.", file=sys.stderr)
        return 2
    try:
        server = ReverseProxyServer((args.bind, args.port), storage, args.upstream)
    except ValueError as error:
        print(f"proxy configuration error: {error}", file=sys.stderr)
        return 2
    scheme = "http"
    if args.proxy_command == "tls":
        paths = initialize_tls_material(args.directory or home / "tls")
        wrap_server_tls(server, paths["leaf_cert"], paths["leaf_key"])
        scheme = "https"
        print(f"Trust CA for this diagnostic only: {paths['ca_cert']}", file=sys.stderr)
    print(f"Reverse proxy on {scheme}://{args.bind}:{server.server_address[1]} → {args.upstream}; bodies are not persisted", file=sys.stderr)
    return _serve(server)


def _serve(server: object) -> int:
    try:
        server.serve_forever()  # type: ignore[attr-defined]
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()  # type: ignore[attr-defined]
    return 0


def _milliseconds(value: object) -> str:
    if value is None:
        return "N/A"
    numeric = float(value)
    return f"{numeric:.0f}ms" if numeric < 1000 else f"{numeric / 1000:.2f}s"


def _export(storage: Storage, args: argparse.Namespace) -> int:
    rows = [dict(row) for row in storage.export_rows(args.from_date, args.to_date, args.session)]
    if args.format == "json":
        text = json.dumps(rows, ensure_ascii=False, indent=2)
    elif args.format == "jsonl":
        text = "\n".join(json.dumps(row, ensure_ascii=False) for row in rows)
    else:
        import io

        output = io.StringIO()
        fieldnames = list(rows[0]) if rows else [
            "session_id", "turn_id", "response_id", "completed_at", "model", "reasoning_effort",
            "input_tokens", "cached_input_tokens", "cache_write_tokens", "output_tokens",
            "reasoning_tokens", "total_tokens", "cost_usd", "data_source", "confidence", "estimated",
        ]
        writer = csv.DictWriter(output, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)
        text = output.getvalue().rstrip("\r\n")
    if args.output:
        args.output.write_text(text + "\n", encoding="utf-8")
        print(f"Exported {len(rows)} call(s) to {args.output}", file=sys.stderr)
    else:
        print(text)
    return 0


def _demo(color: bool) -> str:
    overview: Mapping[str, object] = {
        "calls": 31,
        "sessions": 3,
        "turns": 12,
        "input_tokens": 163_200,
        "cached_input_tokens": 127_600,
        "cache_write_tokens": 8_200,
        "output_tokens": 21_100,
        "reasoning_tokens": 19_400,
        "total_tokens": 184_300,
        "cost_usd": 0.84,
        "unpriced_calls": 0,
        "avg_ttft_ms": 2810,
        "avg_e2e_ms": 18420,
    }
    models: list[Mapping[str, object]] = [
        {"model": "gpt-5.6-sol", "effort": "medium", "calls": 18, "input_tokens": 90000, "cached_input_tokens": 73800, "reasoning_tokens": 8200, "total_tokens": 101200, "cost_usd": 0.41},
        {"model": "gpt-5.6-sol", "effort": "high", "calls": 9, "input_tokens": 54200, "cached_input_tokens": 40100, "reasoning_tokens": 8100, "total_tokens": 61700, "cost_usd": 0.35},
        {"model": "gpt-5.6-sol", "effort": "xhigh", "calls": 4, "input_tokens": 19000, "cached_input_tokens": 10000, "reasoning_tokens": 3100, "total_tokens": 21400, "cost_usd": 0.08},
    ]
    return render_overview(overview, models, period="DEMO", color=color, width=110)


def _meter_home() -> Path:
    return Path(os.environ.get("CODEX_METER_HOME", Path.home() / ".codex-meter"))


def _codex_home() -> Path:
    return Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))


def _configure_windows_stdio() -> None:
    """Use UTF-8 consistently in native Windows terminals and redirected CI output."""
    if os.name != "nt":
        return
    try:
        import ctypes

        ctypes.windll.kernel32.SetConsoleOutputCP(65001)
        ctypes.windll.kernel32.SetConsoleCP(65001)
    except (AttributeError, OSError):
        pass
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            try:
                reconfigure(encoding="utf-8", errors="replace")
            except (OSError, ValueError):
                pass
