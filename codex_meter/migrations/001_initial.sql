PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY,
    codex_thread_id TEXT NOT NULL UNIQUE,
    started_at TEXT,
    ended_at TEXT,
    cwd TEXT,
    project_name TEXT,
    git_repo TEXT,
    git_branch TEXT,
    auth_mode TEXT NOT NULL DEFAULT 'unknown',
    codex_version TEXT,
    provider TEXT,
    source TEXT NOT NULL,
    source_path TEXT NOT NULL,
    parent_thread_id TEXT,
    agent_role TEXT,
    agent_id TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS turns (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    codex_turn_id TEXT NOT NULL UNIQUE,
    started_at TEXT,
    completed_at TEXT,
    status TEXT NOT NULL,
    model TEXT,
    reasoning_effort TEXT,
    reasoning_mode TEXT,
    service_tier TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL,
    ttft_ms INTEGER,
    ttfm_ms INTEGER,
    e2e_ms INTEGER,
    tool_time_ms INTEGER,
    model_time_ms INTEGER,
    error_type TEXT,
    data_source TEXT NOT NULL,
    confidence TEXT NOT NULL,
    estimated INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS llm_calls (
    id INTEGER PRIMARY KEY,
    event_fingerprint TEXT NOT NULL UNIQUE,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id INTEGER REFERENCES turns(id) ON DELETE SET NULL,
    response_id TEXT UNIQUE,
    started_at TEXT,
    first_event_at TEXT,
    first_model_item_at TEXT,
    first_visible_token_at TEXT,
    last_token_at TEXT,
    completed_at TEXT,
    model TEXT,
    actual_model TEXT,
    provider TEXT,
    reasoning_effort TEXT,
    reasoning_mode TEXT,
    transport TEXT,
    service_tier TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    request_duration_ms INTEGER,
    ttfb_ms INTEGER,
    ttfm_ms INTEGER,
    ttft_ms INTEGER,
    generation_ms INTEGER,
    inference_ms INTEGER,
    overhead_ms INTEGER,
    avg_tbt_ms REAL,
    output_tps REAL,
    visible_output_tps REAL,
    retry_index INTEGER NOT NULL DEFAULT 0,
    success INTEGER NOT NULL DEFAULT 1,
    error_type TEXT,
    cost_usd REAL,
    pricing_version TEXT,
    data_source TEXT NOT NULL,
    confidence TEXT NOT NULL,
    estimated INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS tool_calls (
    id INTEGER PRIMARY KEY,
    source_call_id TEXT NOT NULL UNIQUE,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id INTEGER REFERENCES turns(id) ON DELETE SET NULL,
    llm_call_id INTEGER REFERENCES llm_calls(id) ON DELETE SET NULL,
    tool_name TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    duration_ms INTEGER,
    success INTEGER,
    exit_code INTEGER,
    data_source TEXT NOT NULL,
    confidence TEXT NOT NULL,
    estimated INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS pricing_snapshots (
    id INTEGER PRIMARY KEY,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    effective_from TEXT NOT NULL,
    input_per_million REAL NOT NULL,
    cached_input_per_million REAL NOT NULL,
    cache_write_per_million REAL NOT NULL,
    output_per_million REAL NOT NULL,
    long_context_threshold INTEGER,
    long_context_input_multiplier REAL NOT NULL DEFAULT 1,
    long_context_output_multiplier REAL NOT NULL DEFAULT 1,
    currency TEXT NOT NULL DEFAULT 'USD',
    pricing_version TEXT NOT NULL,
    UNIQUE(model, provider, effective_from, pricing_version)
);

CREATE TABLE IF NOT EXISTS import_files (
    source_path TEXT PRIMARY KEY,
    size_bytes INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id) ON DELETE SET NULL,
    imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    malformed_lines INTEGER NOT NULL DEFAULT 0,
    duplicate_usage_events INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_sessions_started_at ON sessions(started_at);
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_name);
CREATE INDEX IF NOT EXISTS idx_turns_session ON turns(session_id);
CREATE INDEX IF NOT EXISTS idx_turns_completed_at ON turns(completed_at);
CREATE INDEX IF NOT EXISTS idx_calls_session ON llm_calls(session_id);
CREATE INDEX IF NOT EXISTS idx_calls_turn ON llm_calls(turn_id);
CREATE INDEX IF NOT EXISTS idx_calls_completed_at ON llm_calls(completed_at);
CREATE INDEX IF NOT EXISTS idx_calls_model_effort ON llm_calls(model, reasoning_effort);
CREATE INDEX IF NOT EXISTS idx_tools_turn ON tool_calls(turn_id);
