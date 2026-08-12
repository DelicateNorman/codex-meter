CREATE TABLE IF NOT EXISTS metric_points (
    id INTEGER PRIMARY KEY,
    event_fingerprint TEXT NOT NULL UNIQUE,
    observed_at TEXT,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    value REAL,
    point_sum REAL,
    point_count INTEGER,
    point_min REAL,
    point_max REAL,
    explicit_bounds_json TEXT,
    bucket_counts_json TEXT,
    attributes_json TEXT NOT NULL DEFAULT '{}',
    thread_id TEXT,
    turn_id TEXT,
    response_id TEXT,
    tool_name TEXT,
    start_time_unix_nano TEXT,
    time_unix_nano TEXT,
    data_source TEXT NOT NULL DEFAULT 'otlp_http',
    confidence TEXT NOT NULL DEFAULT 'exact',
    estimated INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS telemetry_logs (
    id INTEGER PRIMARY KEY,
    event_fingerprint TEXT NOT NULL UNIQUE,
    observed_at TEXT,
    event_name TEXT NOT NULL,
    severity TEXT,
    attributes_json TEXT NOT NULL DEFAULT '{}',
    thread_id TEXT,
    turn_id TEXT,
    response_id TEXT,
    item_id TEXT,
    tool_name TEXT,
    duration_ms REAL,
    status TEXT,
    success INTEGER,
    data_source TEXT NOT NULL DEFAULT 'otlp_http',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS compactions (
    id INTEGER PRIMARY KEY,
    event_fingerprint TEXT NOT NULL UNIQUE,
    thread_id TEXT NOT NULL,
    turn_id TEXT,
    occurred_at TEXT,
    data_source TEXT NOT NULL,
    confidence TEXT NOT NULL DEFAULT 'exact'
);

CREATE TABLE IF NOT EXISTS network_flows (
    id INTEGER PRIMARY KEY,
    event_fingerprint TEXT NOT NULL UNIQUE,
    started_at TEXT,
    ended_at TEXT,
    mode TEXT NOT NULL,
    destination_host TEXT,
    destination_ip TEXT,
    destination_port INTEGER,
    protocol TEXT,
    tls_version TEXT,
    alpn TEXT,
    http_status INTEGER,
    request_bytes INTEGER NOT NULL DEFAULT 0,
    response_bytes INTEGER NOT NULL DEFAULT 0,
    packets_out INTEGER NOT NULL DEFAULT 0,
    packets_in INTEGER NOT NULL DEFAULT 0,
    dns_ms REAL,
    tcp_ms REAL,
    tls_ms REAL,
    ttfb_ms REAL,
    first_event_ms REAL,
    first_output_ms REAL,
    duration_ms REAL,
    success INTEGER,
    error_type TEXT,
    thread_id TEXT,
    turn_id TEXT,
    response_id TEXT,
    data_source TEXT NOT NULL,
    confidence TEXT NOT NULL DEFAULT 'exact',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_metric_name_time ON metric_points(name, observed_at);
CREATE INDEX IF NOT EXISTS idx_metric_turn ON metric_points(turn_id);
CREATE INDEX IF NOT EXISTS idx_telemetry_event_time ON telemetry_logs(event_name, observed_at);
CREATE INDEX IF NOT EXISTS idx_compactions_thread ON compactions(thread_id, occurred_at);
CREATE INDEX IF NOT EXISTS idx_network_time ON network_flows(started_at);
CREATE INDEX IF NOT EXISTS idx_network_destination ON network_flows(destination_host, destination_port);
