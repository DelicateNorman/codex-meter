"""SQLite persistence and aggregate queries."""

from __future__ import annotations

import json
import getpass
import os
import sqlite3
import threading
from contextlib import contextmanager
from importlib.resources import files
from pathlib import Path
from typing import Iterator

from .models import (
    LlmCallRecord,
    MetricPointRecord,
    NetworkFlowRecord,
    ParsedSession,
    TelemetryLogRecord,
    TokenUsage,
)
from .pricing import PricingCatalog


class Storage:
    def __init__(
        self,
        path: Path,
        *,
        owner_uid: int | None = None,
        owner_username: str | None = None,
        account_label: str | None = None,
    ) -> None:
        self.path = path
        self.owner_uid = (os.getuid() if hasattr(os, "getuid") else None) if owner_uid is None else owner_uid
        self.owner_username = owner_username or getpass.getuser() or "unknown"
        self.account_label = account_label.strip() if account_label and account_label.strip() else None
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        try:
            self.path.parent.chmod(0o700)
        except OSError:
            pass
        self.connection = sqlite3.connect(path, check_same_thread=False)
        self._lock = threading.RLock()
        self.connection.row_factory = sqlite3.Row
        self.connection.execute("PRAGMA foreign_keys = ON")
        self.connection.execute("PRAGMA journal_mode = WAL")
        self.connection.execute("PRAGMA synchronous = NORMAL")

    def close(self) -> None:
        self.connection.close()

    def __enter__(self) -> "Storage":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    @contextmanager
    def transaction(self) -> Iterator[sqlite3.Connection]:
        with self._lock:
            with self.connection:
                yield self.connection

    def migrate(self) -> None:
        migration_dir = files("codex_meter").joinpath("migrations")
        for resource in sorted(migration_dir.iterdir(), key=lambda item: item.name):
            if not resource.name.endswith(".sql"):
                continue
            version = resource.name.split("_", 1)[0]
            exists = self.connection.execute(
                "SELECT 1 FROM schema_migrations WHERE version = ?", (version,)
            ).fetchone() if self._table_exists("schema_migrations") else None
            if exists:
                continue
            self.connection.executescript(resource.read_text(encoding="utf-8"))
            self.connection.execute("INSERT OR IGNORE INTO schema_migrations(version) VALUES (?)", (version,))
            self.connection.commit()
        if self._column_exists("sessions", "owner_username"):
            with self.connection:
                self.connection.execute(
                    "UPDATE sessions SET owner_uid=COALESCE(owner_uid, ?), "
                    "owner_username=COALESCE(owner_username, ?)",
                    (self.owner_uid, self.owner_username),
                )

    def sync_pricing(self, catalog: PricingCatalog) -> None:
        with self.transaction() as db:
            for price in catalog.entries:
                db.execute(
                    """
                    INSERT OR IGNORE INTO pricing_snapshots(
                        model, provider, effective_from, input_per_million,
                        cached_input_per_million, cache_write_per_million,
                        output_per_million, long_context_threshold,
                        long_context_input_multiplier, long_context_output_multiplier,
                        pricing_version
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        price.model,
                        price.provider,
                        price.effective_from,
                        price.input_per_million,
                        price.cached_input_per_million,
                        price.cache_write_per_million,
                        price.output_per_million,
                        price.long_context_threshold,
                        price.long_context_input_multiplier,
                        price.long_context_output_multiplier,
                        price.version,
                    ),
                )

    def file_is_current(self, path: Path) -> bool:
        stat = path.stat()
        return self.source_is_current(str(path.resolve()), stat.st_size, stat.st_mtime_ns)

    def source_is_current(self, source_path: str, size_bytes: int, mtime_ns: int) -> bool:
        row = self.connection.execute(
            "SELECT size_bytes, mtime_ns FROM import_files WHERE source_path = ?",
            (source_path,),
        ).fetchone()
        return bool(row and row["size_bytes"] == size_bytes and row["mtime_ns"] == mtime_ns)

    def import_session(
        self,
        parsed: ParsedSession,
        path: Path | None = None,
        *,
        source_path: str | None = None,
        size_bytes: int | None = None,
        mtime_ns: int | None = None,
    ) -> tuple[int, int, int]:
        session = parsed.session
        if path is not None:
            stat = path.stat()
            resolved_source = str(path.resolve())
            resolved_size = stat.st_size
            resolved_mtime = stat.st_mtime_ns
        else:
            if source_path is None or size_bytes is None or mtime_ns is None:
                raise ValueError("stream imports require source_path, size_bytes, and mtime_ns")
            resolved_source = source_path
            resolved_size = size_bytes
            resolved_mtime = mtime_ns
        inserted_calls = 0
        inserted_turns = 0
        inserted_tools = 0
        with self.transaction() as db:
            db.execute(
                """
                INSERT INTO sessions(
                    codex_thread_id, started_at, ended_at, cwd, project_name,
                    git_repo, git_branch, auth_mode, codex_version, provider,
                    source, source_path, parent_thread_id, agent_role, agent_id,
                    owner_uid, owner_username, account_label
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'session_jsonl', ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(codex_thread_id) DO UPDATE SET
                    ended_at=excluded.ended_at,
                    cwd=COALESCE(excluded.cwd, sessions.cwd),
                    project_name=COALESCE(excluded.project_name, sessions.project_name),
                    git_repo=COALESCE(excluded.git_repo, sessions.git_repo),
                    git_branch=COALESCE(excluded.git_branch, sessions.git_branch),
                    auth_mode=CASE WHEN excluded.auth_mode != 'unknown' THEN excluded.auth_mode ELSE sessions.auth_mode END,
                    codex_version=COALESCE(excluded.codex_version, sessions.codex_version),
                    provider=COALESCE(excluded.provider, sessions.provider),
                    source_path=excluded.source_path,
                    owner_uid=COALESCE(sessions.owner_uid, excluded.owner_uid),
                    owner_username=COALESCE(sessions.owner_username, excluded.owner_username),
                    account_label=sessions.account_label,
                    updated_at=CURRENT_TIMESTAMP
                """,
                (
                    session.codex_thread_id,
                    session.started_at,
                    session.ended_at,
                    session.cwd,
                    session.project_name,
                    session.git_repo,
                    session.git_branch,
                    session.auth_mode,
                    session.codex_version,
                    session.provider,
                    session.source_path,
                    session.parent_thread_id,
                    session.agent_role,
                    session.agent_id,
                    self.owner_uid,
                    self.owner_username,
                    self.account_label,
                ),
            )
            session_id = int(
                db.execute("SELECT id FROM sessions WHERE codex_thread_id = ?", (session.codex_thread_id,)).fetchone()[0]
            )
            turn_ids: dict[str, int] = {}
            for turn in parsed.turns.values():
                existed = db.execute(
                    "SELECT id FROM turns WHERE codex_turn_id = ?", (turn.codex_turn_id,)
                ).fetchone()
                db.execute(
                    """
                    INSERT INTO turns(
                        session_id, codex_turn_id, started_at, completed_at, status,
                        model, reasoning_effort, reasoning_mode, service_tier,
                        ttft_ms, ttfm_ms, e2e_ms, error_type,
                        data_source, confidence, estimated
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(codex_turn_id) DO UPDATE SET
                        completed_at=COALESCE(excluded.completed_at, turns.completed_at),
                        status=excluded.status,
                        model=COALESCE(excluded.model, turns.model),
                        reasoning_effort=COALESCE(excluded.reasoning_effort, turns.reasoning_effort),
                        reasoning_mode=COALESCE(excluded.reasoning_mode, turns.reasoning_mode),
                        service_tier=COALESCE(excluded.service_tier, turns.service_tier),
                        ttft_ms=COALESCE(excluded.ttft_ms, turns.ttft_ms),
                        ttfm_ms=COALESCE(excluded.ttfm_ms, turns.ttfm_ms),
                        e2e_ms=COALESCE(excluded.e2e_ms, turns.e2e_ms),
                        error_type=COALESCE(excluded.error_type, turns.error_type)
                    """,
                    (
                        session_id,
                        turn.codex_turn_id,
                        turn.started_at,
                        turn.completed_at,
                        turn.status,
                        turn.model,
                        turn.reasoning_effort,
                        turn.reasoning_mode,
                        turn.service_tier,
                        turn.ttft_ms,
                        turn.ttfm_ms,
                        turn.e2e_ms,
                        turn.error_type,
                        turn.quality.source,
                        turn.quality.confidence.value,
                        int(turn.quality.estimated),
                    ),
                )
                inserted_turns += int(existed is None)
                turn_row = db.execute(
                    "SELECT id FROM turns WHERE codex_turn_id = ?", (turn.codex_turn_id,)
                ).fetchone()
                turn_ids[turn.codex_turn_id] = int(turn_row[0])

            for call in parsed.llm_calls:
                resolved_turn_id = turn_ids.get(call.turn_id or "")
                # Fork rollout prefixes are copies of parent history. When a copied
                # turn already has an authoritative owner, keep the call with that
                # owner even if this semantic event has not been seen before.
                call_session_id = session_id
                if resolved_turn_id is not None:
                    owner = db.execute("SELECT session_id FROM turns WHERE id = ?", (resolved_turn_id,)).fetchone()
                    if owner:
                        call_session_id = int(owner[0])
                before = db.total_changes
                db.execute(
                    """
                    INSERT OR IGNORE INTO llm_calls(
                        event_fingerprint, session_id, turn_id, response_id, completed_at,
                        model, actual_model, provider, reasoning_effort, reasoning_mode,
                        service_tier, input_tokens, cached_input_tokens, cache_write_tokens,
                        output_tokens, reasoning_tokens, total_tokens, retry_index, success,
                        error_type, cost_usd, pricing_version, data_source, confidence, estimated
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        call.event_fingerprint,
                        call_session_id,
                        resolved_turn_id,
                        call.response_id,
                        call.completed_at,
                        call.model,
                        call.actual_model,
                        call.provider,
                        call.reasoning_effort,
                        call.reasoning_mode,
                        call.service_tier,
                        call.usage.input_tokens,
                        call.usage.cached_input_tokens,
                        call.usage.cache_write_tokens,
                        call.usage.output_tokens,
                        call.usage.reasoning_tokens,
                        call.usage.total_tokens,
                        call.retry_index,
                        int(call.success),
                        call.error_type,
                        call.cost_usd,
                        call.pricing_version,
                        call.quality.source,
                        call.quality.confidence.value,
                        int(call.quality.estimated),
                    ),
                )
                inserted_calls += int(db.total_changes > before)

            for tool in parsed.tool_calls:
                before = db.total_changes
                db.execute(
                    """
                    INSERT OR IGNORE INTO tool_calls(
                        source_call_id, session_id, turn_id, tool_name, started_at,
                        completed_at, duration_ms, success, exit_code,
                        data_source, confidence, estimated
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        tool.call_id,
                        session_id,
                        turn_ids.get(tool.turn_id or ""),
                        tool.tool_name,
                        tool.started_at,
                        tool.completed_at,
                        tool.duration_ms,
                        None if tool.success is None else int(tool.success),
                        tool.exit_code,
                        tool.quality.source,
                        tool.quality.confidence.value,
                        int(tool.quality.estimated),
                    ),
                )
                inserted_tools += int(db.total_changes > before)

            db.execute(
                """
                INSERT INTO import_files(
                    source_path, size_bytes, mtime_ns, session_id,
                    malformed_lines, duplicate_usage_events
                ) VALUES (?, ?, ?, ?, ?, ?)
                ON CONFLICT(source_path) DO UPDATE SET
                    size_bytes=excluded.size_bytes,
                    mtime_ns=excluded.mtime_ns,
                    session_id=excluded.session_id,
                    imported_at=CURRENT_TIMESTAMP,
                    malformed_lines=excluded.malformed_lines,
                    duplicate_usage_events=excluded.duplicate_usage_events
                """,
                (
                    resolved_source,
                    resolved_size,
                    resolved_mtime,
                    session_id,
                    parsed.malformed_lines,
                    parsed.duplicate_usage_events,
                ),
            )
            self._refresh_turn_aggregates(db, turn_ids.values())
        return inserted_turns, inserted_calls, inserted_tools

    def ensure_live_session(
        self,
        thread_id: str,
        *,
        started_at: str | None = None,
        cwd: str | None = None,
        model: str | None = None,
        source: str = "app_server",
    ) -> int:
        """Create the minimal session needed to correlate a live signal."""
        project_name = Path(cwd).name if cwd else None
        with self.transaction() as db:
            db.execute(
                """
                INSERT INTO sessions(
                    codex_thread_id, started_at, cwd, project_name, auth_mode,
                    provider, source, source_path, owner_uid, owner_username,
                    account_label
                ) VALUES (?, ?, ?, ?, 'unknown', 'openai', ?, ?, ?, ?, ?)
                ON CONFLICT(codex_thread_id) DO UPDATE SET
                    started_at=COALESCE(sessions.started_at, excluded.started_at),
                    cwd=COALESCE(excluded.cwd, sessions.cwd),
                    project_name=COALESCE(excluded.project_name, sessions.project_name),
                    owner_uid=COALESCE(sessions.owner_uid, excluded.owner_uid),
                    owner_username=COALESCE(sessions.owner_username, excluded.owner_username),
                    account_label=sessions.account_label,
                    updated_at=CURRENT_TIMESTAMP
                """,
                (
                    thread_id, started_at, cwd, project_name, source,
                    f"{source}://{thread_id}", self.owner_uid,
                    self.owner_username, self.account_label,
                ),
            )
            return int(db.execute("SELECT id FROM sessions WHERE codex_thread_id=?", (thread_id,)).fetchone()[0])

    def upsert_live_turn(
        self,
        thread_id: str,
        turn_id: str,
        *,
        started_at: str | None = None,
        completed_at: str | None = None,
        status: str = "running",
        model: str | None = None,
        reasoning_effort: str | None = None,
        service_tier: str | None = None,
        ttft_ms: int | None = None,
        ttfm_ms: int | None = None,
        e2e_ms: int | None = None,
        source: str = "app_server",
    ) -> int:
        session_id = self.ensure_live_session(thread_id, started_at=started_at, model=model, source=source)
        with self.transaction() as db:
            db.execute(
                """
                INSERT INTO turns(
                    session_id, codex_turn_id, started_at, completed_at, status,
                    model, reasoning_effort, service_tier, ttft_ms, ttfm_ms,
                    e2e_ms, data_source, confidence, estimated
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'exact', 0)
                ON CONFLICT(codex_turn_id) DO UPDATE SET
                    started_at=COALESCE(turns.started_at, excluded.started_at),
                    completed_at=COALESCE(excluded.completed_at, turns.completed_at),
                    status=excluded.status,
                    model=COALESCE(excluded.model, turns.model),
                    reasoning_effort=COALESCE(excluded.reasoning_effort, turns.reasoning_effort),
                    service_tier=COALESCE(excluded.service_tier, turns.service_tier),
                    ttft_ms=COALESCE(excluded.ttft_ms, turns.ttft_ms),
                    ttfm_ms=COALESCE(excluded.ttfm_ms, turns.ttfm_ms),
                    e2e_ms=COALESCE(excluded.e2e_ms, turns.e2e_ms)
                """,
                (
                    session_id, turn_id, started_at, completed_at, status, model,
                    reasoning_effort, service_tier, ttft_ms, ttfm_ms, e2e_ms, source,
                ),
            )
            return int(db.execute("SELECT id FROM turns WHERE codex_turn_id=?", (turn_id,)).fetchone()[0])

    def insert_live_call(
        self,
        thread_id: str,
        call: LlmCallRecord,
        *,
        started_at: str | None = None,
        first_event_at: str | None = None,
        first_model_item_at: str | None = None,
        ttft_ms: int | None = None,
        ttfm_ms: int | None = None,
        request_duration_ms: int | None = None,
        transport: str | None = None,
    ) -> bool:
        session_id = self.ensure_live_session(thread_id, started_at=started_at)
        turn_db_id = None
        if call.turn_id:
            turn_db_id = self.upsert_live_turn(thread_id, call.turn_id, started_at=started_at)
        with self.transaction() as db:
            generation_ms = None
            if first_model_item_at and call.completed_at:
                from datetime import datetime
                try:
                    generation_ms = max(0, round((
                        datetime.fromisoformat(call.completed_at.replace("Z", "+00:00"))
                        - datetime.fromisoformat(first_model_item_at.replace("Z", "+00:00"))
                    ).total_seconds() * 1000))
                except ValueError:
                    generation_ms = None
            output_tps = (
                call.usage.output_tokens / (request_duration_ms / 1000)
                if request_duration_ms and request_duration_ms > 0 else None
            )
            before = db.total_changes
            db.execute(
                """
                INSERT OR IGNORE INTO llm_calls(
                    event_fingerprint, session_id, turn_id, response_id, started_at,
                    first_event_at, first_model_item_at, completed_at, model, actual_model,
                    provider, reasoning_effort, reasoning_mode, transport, service_tier,
                    input_tokens, cached_input_tokens, cache_write_tokens, output_tokens,
                    reasoning_tokens, total_tokens, request_duration_ms, ttft_ms, ttfm_ms,
                    generation_ms, output_tps, retry_index, success, error_type, cost_usd, pricing_version,
                    data_source, confidence, estimated
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    call.event_fingerprint, session_id, turn_db_id, call.response_id, started_at,
                    first_event_at, first_model_item_at, call.completed_at, call.model,
                    call.actual_model, call.provider, call.reasoning_effort, call.reasoning_mode,
                    transport, call.service_tier, call.usage.input_tokens,
                    call.usage.cached_input_tokens, call.usage.cache_write_tokens,
                    call.usage.output_tokens, call.usage.reasoning_tokens,
                    call.usage.total_tokens, request_duration_ms, ttft_ms, ttfm_ms,
                    generation_ms, output_tps,
                    call.retry_index, int(call.success), call.error_type, call.cost_usd,
                    call.pricing_version, call.quality.source, call.quality.confidence.value,
                    int(call.quality.estimated),
                ),
            )
            inserted = db.total_changes > before
            if turn_db_id is not None:
                self._refresh_turn_aggregates(db, (turn_db_id,))
            return inserted

    def update_live_turn_usage(self, thread_id: str, turn_id: str, usage: TokenUsage) -> None:
        turn_db_id = self.upsert_live_turn(thread_id, turn_id)
        with self.transaction() as db:
            db.execute(
                """
                UPDATE turns SET
                    input_tokens=MAX(input_tokens, ?),
                    cached_input_tokens=MAX(cached_input_tokens, ?),
                    cache_write_tokens=MAX(cache_write_tokens, ?),
                    output_tokens=MAX(output_tokens, ?),
                    reasoning_tokens=MAX(reasoning_tokens, ?),
                    total_tokens=MAX(total_tokens, ?)
                WHERE id=?
                """,
                (
                    usage.input_tokens, usage.cached_input_tokens, usage.cache_write_tokens,
                    usage.output_tokens, usage.reasoning_tokens, usage.total_tokens, turn_db_id,
                ),
            )

    def update_actual_model(self, turn_id: str, actual_model: str) -> None:
        with self.transaction() as db:
            db.execute("UPDATE llm_calls SET actual_model=? WHERE turn_id=(SELECT id FROM turns WHERE codex_turn_id=?)", (actual_model, turn_id))

    def upsert_live_tool(
        self,
        thread_id: str,
        turn_id: str | None,
        call_id: str,
        tool_name: str,
        *,
        started_at: str | None,
        completed_at: str | None,
        duration_ms: int | None,
        success: bool | None,
        exit_code: int | None = None,
        source: str = "app_server",
    ) -> None:
        session_id = self.ensure_live_session(thread_id, started_at=started_at, source=source)
        turn_db_id = self.upsert_live_turn(thread_id, turn_id, started_at=started_at, source=source) if turn_id else None
        with self.transaction() as db:
            db.execute(
                """
                INSERT INTO tool_calls(
                    source_call_id, session_id, turn_id, tool_name, started_at,
                    completed_at, duration_ms, success, exit_code,
                    data_source, confidence, estimated
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'exact', 0)
                ON CONFLICT(source_call_id) DO UPDATE SET
                    completed_at=COALESCE(excluded.completed_at, tool_calls.completed_at),
                    duration_ms=COALESCE(excluded.duration_ms, tool_calls.duration_ms),
                    success=COALESCE(excluded.success, tool_calls.success),
                    exit_code=COALESCE(excluded.exit_code, tool_calls.exit_code)
                """,
                (
                    call_id, session_id, turn_db_id, tool_name, started_at, completed_at,
                    duration_ms, None if success is None else int(success), exit_code, source,
                ),
            )
            if turn_db_id is not None:
                self._refresh_turn_aggregates(db, (turn_db_id,))

    def insert_metric_points(self, points: list[MetricPointRecord]) -> int:
        inserted = 0
        with self.transaction() as db:
            for point in points:
                before = db.total_changes
                db.execute(
                    """
                    INSERT OR IGNORE INTO metric_points(
                        event_fingerprint, observed_at, name, kind, value, point_sum,
                        point_count, point_min, point_max, explicit_bounds_json,
                        bucket_counts_json, attributes_json, thread_id, turn_id,
                        response_id, tool_name, start_time_unix_nano, time_unix_nano,
                        data_source, confidence, estimated
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        point.event_fingerprint, point.observed_at, point.name, point.kind,
                        point.value, point.point_sum, point.point_count, point.point_min,
                        point.point_max, json.dumps(point.explicit_bounds, separators=(",", ":")),
                        json.dumps(point.bucket_counts, separators=(",", ":")),
                        json.dumps(point.attributes, sort_keys=True, separators=(",", ":")),
                        point.thread_id, point.turn_id, point.response_id, point.tool_name,
                        point.start_time_unix_nano, point.time_unix_nano,
                        point.quality.source, point.quality.confidence.value,
                        int(point.quality.estimated),
                    ),
                )
                inserted += int(db.total_changes > before)
                self._apply_metric_point(db, point)
        return inserted

    def insert_telemetry_logs(self, records: list[TelemetryLogRecord]) -> int:
        inserted = 0
        with self.transaction() as db:
            for record in records:
                before = db.total_changes
                db.execute(
                    """
                    INSERT OR IGNORE INTO telemetry_logs(
                        event_fingerprint, observed_at, event_name, severity,
                        attributes_json, thread_id, turn_id, response_id, item_id,
                        tool_name, duration_ms, status, success, data_source
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        record.event_fingerprint, record.observed_at, record.event_name,
                        record.severity,
                        json.dumps(record.attributes, sort_keys=True, separators=(",", ":")),
                        record.thread_id, record.turn_id, record.response_id, record.item_id,
                        record.tool_name, record.duration_ms, record.status,
                        None if record.success is None else int(record.success),
                        record.quality.source,
                    ),
                )
                inserted += int(db.total_changes > before)
        return inserted

    def insert_compaction(
        self, fingerprint: str, thread_id: str, turn_id: str | None,
        occurred_at: str | None, source: str,
    ) -> bool:
        self.ensure_live_session(thread_id, started_at=occurred_at, source=source)
        with self.transaction() as db:
            before = db.total_changes
            db.execute(
                """INSERT OR IGNORE INTO compactions(
                       event_fingerprint, thread_id, turn_id, occurred_at, data_source
                   ) VALUES (?, ?, ?, ?, ?)""",
                (fingerprint, thread_id, turn_id, occurred_at, source),
            )
            return db.total_changes > before

    def insert_network_flow(self, flow: NetworkFlowRecord) -> bool:
        with self.transaction() as db:
            before = db.total_changes
            db.execute(
                """
                INSERT OR IGNORE INTO network_flows(
                    event_fingerprint, started_at, ended_at, mode, destination_host,
                    destination_ip, destination_port, protocol, tls_version, alpn,
                    http_status, request_bytes, response_bytes, packets_out, packets_in,
                    dns_ms, tcp_ms, tls_ms, ttfb_ms, first_event_ms, first_output_ms,
                    duration_ms, success, error_type, thread_id, turn_id, response_id,
                    data_source, confidence
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    flow.event_fingerprint, flow.started_at, flow.ended_at, flow.mode,
                    flow.destination_host, flow.destination_ip, flow.destination_port,
                    flow.protocol, flow.tls_version, flow.alpn, flow.http_status,
                    flow.request_bytes, flow.response_bytes, flow.packets_out,
                    flow.packets_in, flow.dns_ms, flow.tcp_ms, flow.tls_ms,
                    flow.ttfb_ms, flow.first_event_ms, flow.first_output_ms,
                    flow.duration_ms, None if flow.success is None else int(flow.success),
                    flow.error_type, flow.thread_id, flow.turn_id, flow.response_id,
                    flow.data_source, flow.quality.confidence.value,
                ),
            )
            return db.total_changes > before

    def overview(
        self,
        day: str | None = None,
        *,
        account: str | None = None,
        project: str | None = None,
    ) -> sqlite3.Row:
        return self.overview_range(day, day, account=account, project=project)

    def overview_range(
        self,
        from_date: str | None = None,
        to_date: str | None = None,
        *,
        account: str | None = None,
        project: str | None = None,
    ) -> sqlite3.Row:
        where, params = self._owned_call_filter(
            "c.completed_at", from_date=from_date, to_date=to_date,
            account=account, project=project,
        )
        return self.connection.execute(
            f"""
            WITH filtered_calls AS (
                SELECT c.* FROM llm_calls c
                JOIN sessions s ON s.id=c.session_id
                {where}
            ), selected_turns AS (
                SELECT DISTINCT turn_id FROM filtered_calls WHERE turn_id IS NOT NULL
            ), turn_metrics AS (
                SELECT AVG(t.ttft_ms) AS avg_ttft_ms, AVG(t.e2e_ms) AS avg_e2e_ms
                FROM turns t JOIN selected_turns s ON s.turn_id = t.id
            )
            SELECT
                COUNT(*) AS calls,
                COUNT(DISTINCT c.session_id) AS sessions,
                COUNT(DISTINCT c.turn_id) AS turns,
                COALESCE(SUM(c.input_tokens), 0) AS input_tokens,
                COALESCE(SUM(c.cached_input_tokens), 0) AS cached_input_tokens,
                COALESCE(SUM(c.cache_write_tokens), 0) AS cache_write_tokens,
                COALESCE(SUM(c.output_tokens), 0) AS output_tokens,
                COALESCE(SUM(c.reasoning_tokens), 0) AS reasoning_tokens,
                COALESCE(SUM(c.total_tokens), 0) AS total_tokens,
                SUM(c.cost_usd) AS cost_usd,
                SUM(CASE WHEN c.cost_usd IS NULL THEN 1 ELSE 0 END) AS unpriced_calls,
                tm.avg_ttft_ms AS avg_ttft_ms,
                tm.avg_e2e_ms AS avg_e2e_ms
            FROM filtered_calls c
            CROSS JOIN turn_metrics tm
            """,
            params,
        ).fetchone()

    def model_breakdown(
        self,
        day: str | None = None,
        *,
        account: str | None = None,
        project: str | None = None,
    ) -> list[sqlite3.Row]:
        return self.model_breakdown_range(day, day, account=account, project=project)

    def model_breakdown_range(
        self,
        from_date: str | None = None,
        to_date: str | None = None,
        *,
        account: str | None = None,
        project: str | None = None,
    ) -> list[sqlite3.Row]:
        where, params = self._owned_call_filter(
            "c.completed_at", from_date=from_date, to_date=to_date,
            account=account, project=project,
        )
        return list(
            self.connection.execute(
                f"""
                SELECT
                    COALESCE(c.model, 'Unknown') AS model,
                    COALESCE(c.reasoning_effort, 'Unknown') AS effort,
                    COUNT(*) AS calls,
                    SUM(c.input_tokens) AS input_tokens,
                    SUM(c.cached_input_tokens) AS cached_input_tokens,
                    SUM(c.output_tokens) AS output_tokens,
                    SUM(c.reasoning_tokens) AS reasoning_tokens,
                    SUM(c.total_tokens) AS total_tokens,
                    SUM(c.cost_usd) AS cost_usd,
                    SUM(CASE WHEN c.cost_usd IS NULL THEN 1 ELSE 0 END) AS unpriced_calls
                FROM llm_calls c JOIN sessions s ON s.id=c.session_id
                {where}
                GROUP BY c.model, c.reasoning_effort
                ORDER BY SUM(c.total_tokens) DESC
                """,
                params,
            )
        )

    def sessions(self, limit: int = 20) -> list[sqlite3.Row]:
        return list(
            self.connection.execute(
                """
                WITH turn_counts AS (
                    SELECT session_id, COUNT(*) AS turns
                    FROM turns
                    GROUP BY session_id
                ), call_totals AS (
                    SELECT session_id, COUNT(*) AS calls,
                           SUM(total_tokens) AS total_tokens,
                           SUM(cost_usd) AS cost_usd,
                           SUM(cached_input_tokens) AS cached_input_tokens,
                           SUM(input_tokens) AS input_tokens
                    FROM llm_calls
                    GROUP BY session_id
                )
                SELECT s.codex_thread_id, s.project_name, s.started_at, s.ended_at,
                       COALESCE(t.turns, 0) AS turns,
                       COALESCE(c.calls, 0) AS calls,
                       COALESCE(c.total_tokens, 0) AS total_tokens,
                       c.cost_usd,
                       COALESCE(c.cached_input_tokens, 0) AS cached_input_tokens,
                       COALESCE(c.input_tokens, 0) AS input_tokens
                FROM sessions s
                LEFT JOIN turn_counts t ON t.session_id = s.id
                LEFT JOIN call_totals c ON c.session_id = s.id
                WHERE ((? IS NOT NULL AND s.owner_uid=?) OR (? IS NULL AND s.owner_username=?))
                ORDER BY s.started_at DESC
                LIMIT ?
                """,
                (self.owner_uid, self.owner_uid, self.owner_uid, self.owner_username, limit),
            )
        )

    def account_breakdown(self) -> list[sqlite3.Row]:
        where, params = self._owned_call_filter("c.completed_at")
        return list(self.connection.execute(
            f"""
            SELECT COALESCE(s.account_label, 'Unassigned') AS account,
                   COUNT(DISTINCT s.id) AS sessions,
                   COUNT(*) AS calls,
                   COALESCE(SUM(c.total_tokens), 0) AS total_tokens,
                   SUM(c.cost_usd) AS cost_usd,
                   MIN(c.completed_at) AS first_used_at,
                   MAX(c.completed_at) AS last_used_at
            FROM llm_calls c JOIN sessions s ON s.id=c.session_id
            {where}
            GROUP BY s.account_label
            ORDER BY total_tokens DESC
            """, params,
        ))

    def claim_unassigned_account(self, label: str) -> int:
        cleaned = label.strip()
        if not cleaned:
            raise ValueError("account label cannot be empty")
        with self.transaction() as db:
            before = db.total_changes
            if self.owner_uid is not None:
                db.execute(
                    "UPDATE sessions SET account_label=? WHERE owner_uid=? AND account_label IS NULL",
                    (cleaned, self.owner_uid),
                )
            else:
                db.execute(
                    "UPDATE sessions SET account_label=? WHERE owner_username=? AND account_label IS NULL",
                    (cleaned, self.owner_username),
                )
            return db.total_changes - before

    def usage_history(
        self,
        group: str,
        *,
        account: str | None = None,
        project: str | None = None,
    ) -> list[sqlite3.Row]:
        bucket = {
            "day": "date(c.completed_at, 'localtime')",
            "week": "date(c.completed_at, 'localtime', 'weekday 0', '-6 days')",
            "month": "strftime('%Y-%m-01', c.completed_at, 'localtime')",
        }.get(group)
        if bucket is None:
            raise ValueError("group must be day, week, or month")
        where, params = self._owned_call_filter(
            "c.completed_at", account=account, project=project,
        )
        return list(self.connection.execute(
            f"""
            SELECT {bucket} AS period_start,
                   COUNT(*) AS calls,
                   COUNT(DISTINCT c.session_id) AS sessions,
                   COUNT(DISTINCT c.turn_id) AS turns,
                   COALESCE(SUM(c.input_tokens), 0) AS input_tokens,
                   COALESCE(SUM(c.cached_input_tokens), 0) AS cached_input_tokens,
                   COALESCE(SUM(c.output_tokens), 0) AS output_tokens,
                   COALESCE(SUM(c.total_tokens), 0) AS total_tokens,
                   SUM(c.cost_usd) AS cost_usd
            FROM llm_calls c JOIN sessions s ON s.id=c.session_id
            {where}
            GROUP BY period_start
            ORDER BY period_start
            """, params,
        ))

    def provider_breakdown(self, day: str | None = None) -> list[sqlite3.Row]:
        where, params = self._owned_call_filter(
            "c.completed_at", from_date=day, to_date=day,
        )
        return list(self.connection.execute(
            f"""
            SELECT COALESCE(c.provider, s.provider, 'Unknown') AS provider,
                   COUNT(*) AS calls, COUNT(DISTINCT c.session_id) AS sessions,
                   SUM(c.input_tokens) AS input_tokens,
                   SUM(c.cached_input_tokens) AS cached_input_tokens,
                   SUM(c.output_tokens) AS output_tokens,
                   SUM(c.total_tokens) AS total_tokens,
                   SUM(c.cost_usd) AS cost_usd
            FROM llm_calls c JOIN sessions s ON s.id=c.session_id
            {where}
            GROUP BY COALESCE(c.provider, s.provider, 'Unknown')
            ORDER BY total_tokens DESC
            """, params,
        ))

    def agent_breakdown(self, day: str | None = None) -> list[sqlite3.Row]:
        where, params = self._owned_call_filter(
            "c.completed_at", from_date=day, to_date=day,
        )
        return list(self.connection.execute(
            f"""
            SELECT COALESCE(s.agent_role,
                       CASE WHEN s.parent_thread_id IS NULL THEN 'root' ELSE 'subagent' END) AS role,
                   COUNT(DISTINCT s.id) AS sessions,
                   COUNT(DISTINCT c.turn_id) AS turns,
                   COUNT(*) AS calls,
                   SUM(c.total_tokens) AS total_tokens,
                   SUM(c.cost_usd) AS cost_usd
            FROM llm_calls c JOIN sessions s ON s.id=c.session_id
            {where}
            GROUP BY role ORDER BY total_tokens DESC
            """, params,
        ))

    def export_rows(self, from_date: str | None, to_date: str | None, session: str | None) -> list[sqlite3.Row]:
        clauses: list[str] = []
        params: list[object] = []
        if self.owner_uid is not None:
            clauses.append("s.owner_uid = ?")
            params.append(self.owner_uid)
        else:
            clauses.append("s.owner_username = ?")
            params.append(self.owner_username)
        if from_date:
            clauses.append("date(c.completed_at, 'localtime') >= date(?)")
            params.append(from_date)
        if to_date:
            clauses.append("date(c.completed_at, 'localtime') <= date(?)")
            params.append(to_date)
        if session:
            clauses.append("s.codex_thread_id = ?")
            params.append(session)
        where = "WHERE " + " AND ".join(clauses) if clauses else ""
        return list(
            self.connection.execute(
                f"""
                SELECT s.codex_thread_id AS session_id, t.codex_turn_id AS turn_id,
                       c.response_id, c.completed_at, c.model, c.reasoning_effort,
                       c.input_tokens, c.cached_input_tokens, c.cache_write_tokens,
                       c.output_tokens, c.reasoning_tokens, c.total_tokens,
                       c.cost_usd, c.data_source, c.confidence, c.estimated
                FROM llm_calls c
                JOIN sessions s ON s.id = c.session_id
                LEFT JOIN turns t ON t.id = c.turn_id
                {where}
                ORDER BY c.completed_at
                """,
                params,
            )
        )

    def metric_points(self, day: str | None = None, names: tuple[str, ...] = ()) -> list[sqlite3.Row]:
        clauses: list[str] = []
        params: list[object] = []
        if day:
            clauses.append("date(observed_at, 'localtime') = date(?)")
            params.append(day)
        if names:
            clauses.append("name IN (" + ",".join("?" for _ in names) + ")")
            params.extend(names)
        where = "WHERE " + " AND ".join(clauses) if clauses else ""
        return list(
            self.connection.execute(
                f"SELECT * FROM metric_points {where} ORDER BY observed_at, id", params
            )
        )

    def usage_calls(self, day: str | None = None) -> list[sqlite3.Row]:
        where, params = self._owned_call_filter(
            "c.completed_at", from_date=day, to_date=day,
        )
        return list(
            self.connection.execute(
                f"""
                SELECT c.*, s.project_name, s.codex_thread_id, t.codex_turn_id
                FROM llm_calls c
                JOIN sessions s ON s.id=c.session_id
                LEFT JOIN turns t ON t.id=c.turn_id
                {where}
                ORDER BY c.completed_at, c.id
                """,
                params,
            )
        )

    def project_breakdown(self, day: str | None = None) -> list[sqlite3.Row]:
        where, params = self._owned_call_filter(
            "c.completed_at", from_date=day, to_date=day,
        )
        return list(
            self.connection.execute(
                f"""
                WITH compaction_counts AS (
                    SELECT thread_id, COUNT(*) AS compactions
                    FROM compactions GROUP BY thread_id
                ), project_compactions AS (
                    SELECT s.project_name, SUM(COALESCE(x.compactions, 0)) AS compactions
                    FROM sessions s
                    LEFT JOIN compaction_counts x ON x.thread_id=s.codex_thread_id
                    GROUP BY s.project_name
                )
                SELECT COALESCE(s.project_name, 'Unknown') AS project,
                       COUNT(DISTINCT s.id) AS sessions,
                       COUNT(DISTINCT c.turn_id) AS turns,
                       COUNT(*) AS calls,
                       SUM(c.input_tokens) AS input_tokens,
                       SUM(c.cached_input_tokens) AS cached_input_tokens,
                       SUM(c.output_tokens) AS output_tokens,
                       SUM(c.total_tokens) AS total_tokens,
                       SUM(c.cost_usd) AS cost_usd,
                       SUM(CASE WHEN c.retry_index > 0 THEN c.total_tokens ELSE 0 END) AS retry_tokens,
                       COALESCE(MAX(pc.compactions), 0) AS compactions
                FROM llm_calls c
                JOIN sessions s ON s.id=c.session_id
                LEFT JOIN project_compactions pc ON pc.project_name IS s.project_name
                {where}
                GROUP BY s.project_name
                ORDER BY total_tokens DESC
                """,
                params,
            )
        )

    def tool_breakdown(self, day: str | None = None) -> list[sqlite3.Row]:
        where, params = self._owned_call_filter(
            "t.completed_at", from_date=day, to_date=day,
        )
        return list(
            self.connection.execute(
                f"""
                SELECT tool_name, COUNT(*) AS calls,
                       SUM(CASE WHEN success=1 THEN 1 ELSE 0 END) AS successes,
                       SUM(CASE WHEN success IS NOT NULL THEN 1 ELSE 0 END) AS known_outcomes,
                       AVG(duration_ms) AS avg_ms, MAX(duration_ms) AS max_ms,
                       SUM(duration_ms) AS total_ms
                FROM tool_calls t JOIN sessions s ON s.id=t.session_id
                {where}
                GROUP BY t.tool_name ORDER BY total_ms DESC
                """,
                params,
            )
        )

    def tool_durations(self, day: str | None = None) -> dict[str, list[int]]:
        where, params = self._owned_call_filter(
            "t.completed_at", from_date=day, to_date=day,
        )
        rows = self.connection.execute(
            f"""SELECT t.tool_name, t.duration_ms FROM tool_calls t
                JOIN sessions s ON s.id=t.session_id
                {where} AND t.duration_ms IS NOT NULL""",
            params,
        )
        output: dict[str, list[int]] = {}
        for row in rows:
            output.setdefault(str(row["tool_name"]), []).append(int(row["duration_ms"]))
        return output

    def turn_waterfall(self, turn_id: str) -> tuple[sqlite3.Row | None, list[sqlite3.Row], list[sqlite3.Row]]:
        turn = self.connection.execute(
            """SELECT t.*, s.codex_thread_id, s.project_name
               FROM turns t JOIN sessions s ON s.id=t.session_id
               WHERE t.codex_turn_id=?
                 AND ((? IS NOT NULL AND s.owner_uid=?) OR (? IS NULL AND s.owner_username=?))""",
            (turn_id, self.owner_uid, self.owner_uid, self.owner_uid, self.owner_username),
        ).fetchone()
        if turn is None:
            return None, [], []
        calls = list(
            self.connection.execute(
                "SELECT * FROM llm_calls WHERE turn_id=(SELECT id FROM turns WHERE codex_turn_id=?) ORDER BY COALESCE(started_at, completed_at), id",
                (turn_id,),
            )
        )
        tools = list(
            self.connection.execute(
                "SELECT * FROM tool_calls WHERE turn_id=(SELECT id FROM turns WHERE codex_turn_id=?) ORDER BY COALESCE(started_at, completed_at), id",
                (turn_id,),
            )
        )
        return turn, calls, tools

    def project_names(self) -> list[str]:
        if self.owner_uid is not None:
            owner_clause = "s.owner_uid=?"
            params: tuple[object, ...] = (self.owner_uid,)
        else:
            owner_clause = "s.owner_username=?"
            params = (self.owner_username,)
        rows = self.connection.execute(
            f"""
            WITH project_activity AS (
                SELECT s.project_name,
                       COALESCE(c.completed_at, c.started_at, s.ended_at, s.started_at) AS used_at
                FROM sessions s
                LEFT JOIN llm_calls c ON c.session_id=s.id
                WHERE {owner_clause}
            )
            SELECT COALESCE(project_name, 'Unknown') AS project,
                   MAX(used_at) AS last_used_at
            FROM project_activity
            GROUP BY project_name
            ORDER BY last_used_at DESC, project COLLATE NOCASE
            """,
            params,
        )
        return [str(row["project"]) for row in rows]

    def recent_network(
        self,
        limit: int = 30,
        *,
        project: str | None = None,
    ) -> list[sqlite3.Row]:
        if project is not None:
            if self.owner_uid is not None:
                owner_clause = "s.owner_uid=?"
                params: tuple[object, ...] = (self.owner_uid, project, max(1, limit))
            else:
                owner_clause = "s.owner_username=?"
                params = (self.owner_username, project, max(1, limit))
            return list(
                self.connection.execute(
                    f"""
                    SELECT nf.* FROM network_flows nf
                    JOIN sessions s ON s.codex_thread_id=nf.thread_id
                    WHERE {owner_clause} AND COALESCE(s.project_name, 'Unknown')=?
                    ORDER BY COALESCE(nf.started_at, nf.created_at) DESC LIMIT ?
                    """,
                    params,
                )
            )
        return list(
            self.connection.execute(
                "SELECT * FROM network_flows ORDER BY COALESCE(started_at, created_at) DESC LIMIT ?",
                (max(1, limit),),
            )
        )

    def response_performance_range(
        self,
        from_date: str | None = None,
        to_date: str | None = None,
        *,
        project: str | None = None,
    ) -> list[sqlite3.Row]:
        """Return content-free turn timing samples for the current OS user."""
        where, params = self._owned_call_filter(
            "t.completed_at", from_date=from_date, to_date=to_date, project=project,
        )
        return list(
            self.connection.execute(
                f"""
                SELECT
                    t.completed_at,
                    strftime('%H:%M:%S', t.completed_at, 'localtime') AS local_time,
                    COALESCE(t.model, 'Unknown') AS model,
                    t.output_tokens,
                    t.ttft_ms,
                    t.e2e_ms,
                    (
                        SELECT AVG(c.output_tps)
                        FROM llm_calls c
                        WHERE c.turn_id=t.id AND c.output_tps IS NOT NULL
                    ) AS exact_output_tps
                FROM turns t JOIN sessions s ON s.id=t.session_id
                {where}
                AND t.completed_at IS NOT NULL
                ORDER BY t.completed_at DESC
                """,
                params,
            )
        )

    def telemetry_retry_summary(self, day: str | None = None) -> dict[str, float | int]:
        clauses = ["event_name='codex.api_request'"]
        params: list[object] = []
        if day:
            clauses.append("date(observed_at, 'localtime')=date(?)")
            params.append(day)
        rows = self.connection.execute(
            "SELECT attributes_json, duration_ms FROM telemetry_logs WHERE " + " AND ".join(clauses),
            params,
        )
        attempts = duration_ms = failures = 0
        for row in rows:
            try:
                attrs = json.loads(row["attributes_json"] or "{}")
                attempt = int(attrs.get("attempt", 0))
            except (json.JSONDecodeError, TypeError, ValueError):
                attempt = 0
                attrs = {}
            if attempt > 0:
                attempts += 1
                duration_ms += float(row["duration_ms"] or 0)
                failures += int(str(attrs.get("success", "true")).lower() != "true")
        return {"attempts": attempts, "duration_ms": duration_ms, "failures": failures}

    def integrity_check(self) -> str:
        return str(self.connection.execute("PRAGMA integrity_check").fetchone()[0])

    def counts(self) -> dict[str, int]:
        return {
            table: int(self.connection.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0])
            for table in (
                "sessions", "turns", "llm_calls", "tool_calls", "pricing_snapshots",
                "metric_points", "telemetry_logs", "compactions", "network_flows",
            )
        }

    def _table_exists(self, table: str) -> bool:
        return bool(
            self.connection.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (table,)
            ).fetchone()
        )

    def _column_exists(self, table: str, column: str) -> bool:
        return any(row[1] == column for row in self.connection.execute(f"PRAGMA table_info({table})"))

    def _owned_call_filter(
        self,
        column: str,
        *,
        from_date: str | None = None,
        to_date: str | None = None,
        account: str | None = None,
        project: str | None = None,
    ) -> tuple[str, tuple[object, ...]]:
        clauses: list[str] = []
        params: list[object] = []
        if self.owner_uid is not None:
            clauses.append("s.owner_uid = ?")
            params.append(self.owner_uid)
        else:
            clauses.append("s.owner_username = ?")
            params.append(self.owner_username)
        if from_date:
            clauses.append(f"date({column}, 'localtime') >= date(?)")
            params.append(from_date)
        if to_date:
            clauses.append(f"date({column}, 'localtime') <= date(?)")
            params.append(to_date)
        if account == "Unassigned":
            clauses.append("s.account_label IS NULL")
        elif account:
            clauses.append("s.account_label = ?")
            params.append(account)
        if project is not None:
            clauses.append("COALESCE(s.project_name, 'Unknown') = ?")
            params.append(project)
        return "WHERE " + " AND ".join(clauses), tuple(params)

    @staticmethod
    def _apply_metric_point(db: sqlite3.Connection, point: MetricPointRecord) -> None:
        value = point.value
        if value is None and point.point_sum is not None and point.point_count:
            value = point.point_sum / point.point_count
        if value is None:
            return
        turn_columns = {
            "codex.turn.e2e_duration_ms": "e2e_ms",
            "codex.turn.ttft.duration_ms": "ttft_ms",
            "codex.turn.ttfm.duration_ms": "ttfm_ms",
        }
        column = turn_columns.get(point.name)
        if column and point.turn_id:
            db.execute(
                f"UPDATE turns SET {column}=COALESCE({column}, ?) WHERE codex_turn_id=?",
                (round(value), point.turn_id),
            )
        call_columns = {
            "codex.api_request.duration_ms": "request_duration_ms",
            "codex.responses_api_overhead.duration_ms": "overhead_ms",
            "codex.responses_api_inference_time.duration_ms": "inference_ms",
            "codex.responses_api_engine_iapi_ttft.duration_ms": "ttfb_ms",
            "codex.responses_api_engine_service_ttft.duration_ms": "ttft_ms",
            "codex.responses_api_engine_iapi_tbt.duration_ms": "avg_tbt_ms",
            "codex.responses_api_engine_service_tbt.duration_ms": "avg_tbt_ms",
        }
        call_column = call_columns.get(point.name)
        if call_column and point.response_id:
            db.execute(
                f"UPDATE llm_calls SET {call_column}=COALESCE({call_column}, ?) WHERE response_id=?",
                (value, point.response_id),
            )
        if point.name == "codex.turn.token_usage" and point.turn_id:
            token_columns = {
                "input": "input_tokens",
                "cached_input": "cached_input_tokens",
                "cache_write_input": "cache_write_tokens",
                "output": "output_tokens",
                "reasoning_output": "reasoning_tokens",
                "total": "total_tokens",
            }
            token_column = token_columns.get(point.attributes.get("token_type", ""))
            if token_column:
                db.execute(
                    f"UPDATE turns SET {token_column}=MAX({token_column}, ?) WHERE codex_turn_id=?",
                    (round(value), point.turn_id),
                )

    @staticmethod
    def _refresh_turn_aggregates(db: sqlite3.Connection, turn_ids: object) -> None:
        for turn_id in turn_ids:
            db.execute(
                """
                UPDATE turns SET
                    input_tokens=COALESCE((SELECT SUM(input_tokens) FROM llm_calls WHERE turn_id=?), 0),
                    cached_input_tokens=COALESCE((SELECT SUM(cached_input_tokens) FROM llm_calls WHERE turn_id=?), 0),
                    cache_write_tokens=COALESCE((SELECT SUM(cache_write_tokens) FROM llm_calls WHERE turn_id=?), 0),
                    output_tokens=COALESCE((SELECT SUM(output_tokens) FROM llm_calls WHERE turn_id=?), 0),
                    reasoning_tokens=COALESCE((SELECT SUM(reasoning_tokens) FROM llm_calls WHERE turn_id=?), 0),
                    total_tokens=COALESCE((SELECT SUM(total_tokens) FROM llm_calls WHERE turn_id=?), 0),
                    cost_usd=(SELECT SUM(cost_usd) FROM llm_calls WHERE turn_id=?),
                    tool_time_ms=(SELECT SUM(duration_ms) FROM tool_calls WHERE turn_id=?)
                WHERE id=?
                """,
                (turn_id, turn_id, turn_id, turn_id, turn_id, turn_id, turn_id, turn_id, turn_id),
            )

    @staticmethod
    def _date_filter(day: str | None, column: str) -> tuple[str, tuple[object, ...]]:
        if day is None:
            return "", ()
        return f"WHERE date({column}, 'localtime') = date(?)", (day,)
