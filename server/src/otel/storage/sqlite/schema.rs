//! SQLite schema definitions

/// Initial schema version
pub const SCHEMA_VERSION: i32 = 1;

/// Initial schema SQL
pub const INITIAL_SCHEMA: &str = r#"
-- Schema version table
CREATE TABLE IF NOT EXISTS schema_version (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL,
    applied_at INTEGER NOT NULL,
    description TEXT
);

-- Migration history
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at INTEGER NOT NULL,
    checksum TEXT NOT NULL,
    execution_time_ms INTEGER,
    success INTEGER NOT NULL DEFAULT 1
);

-- Trace summary table
CREATE TABLE IF NOT EXISTS traces (
    trace_id TEXT PRIMARY KEY,
    root_span_id TEXT,
    service_name TEXT NOT NULL,
    detected_framework TEXT NOT NULL,
    span_count INTEGER NOT NULL DEFAULT 0,
    start_time_ns INTEGER NOT NULL,
    end_time_ns INTEGER,
    duration_ns INTEGER,
    total_input_tokens INTEGER,
    total_output_tokens INTEGER,
    total_tokens INTEGER,
    has_errors INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_traces_service ON traces(service_name);
CREATE INDEX IF NOT EXISTS idx_traces_framework ON traces(detected_framework);
CREATE INDEX IF NOT EXISTS idx_traces_start ON traces(start_time_ns);
CREATE INDEX IF NOT EXISTS idx_traces_created ON traces(created_at);

-- Span index table (for fast queries)
CREATE TABLE IF NOT EXISTS spans (
    span_id TEXT PRIMARY KEY,
    trace_id TEXT NOT NULL,
    parent_span_id TEXT,
    span_name TEXT NOT NULL,
    service_name TEXT NOT NULL,
    detected_framework TEXT NOT NULL,
    detected_category TEXT,
    gen_ai_agent_name TEXT,
    gen_ai_tool_name TEXT,
    gen_ai_request_model TEXT,
    start_time_ns INTEGER NOT NULL,
    end_time_ns INTEGER,
    duration_ns INTEGER,
    status_code INTEGER NOT NULL DEFAULT 0,
    usage_input_tokens INTEGER,
    usage_output_tokens INTEGER,
    parquet_file TEXT,
    FOREIGN KEY (trace_id) REFERENCES traces(trace_id)
);

CREATE INDEX IF NOT EXISTS idx_spans_trace ON spans(trace_id);
CREATE INDEX IF NOT EXISTS idx_spans_parent ON spans(parent_span_id);
CREATE INDEX IF NOT EXISTS idx_spans_service ON spans(service_name);
CREATE INDEX IF NOT EXISTS idx_spans_framework ON spans(detected_framework);
CREATE INDEX IF NOT EXISTS idx_spans_category ON spans(detected_category);
CREATE INDEX IF NOT EXISTS idx_spans_agent ON spans(gen_ai_agent_name);
CREATE INDEX IF NOT EXISTS idx_spans_tool ON spans(gen_ai_tool_name);
CREATE INDEX IF NOT EXISTS idx_spans_model ON spans(gen_ai_request_model);
CREATE INDEX IF NOT EXISTS idx_spans_start ON spans(start_time_ns);

-- Span events index
CREATE TABLE IF NOT EXISTS span_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    span_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    event_name TEXT NOT NULL,
    event_time_ns INTEGER NOT NULL,
    FOREIGN KEY (span_id) REFERENCES spans(span_id)
);

CREATE INDEX IF NOT EXISTS idx_events_span ON span_events(span_id);
CREATE INDEX IF NOT EXISTS idx_events_trace ON span_events(trace_id);
CREATE INDEX IF NOT EXISTS idx_events_name ON span_events(event_name);

-- Parquet file tracking
CREATE TABLE IF NOT EXISTS parquet_files (
    file_path TEXT PRIMARY KEY,
    date_partition TEXT NOT NULL,
    span_count INTEGER NOT NULL,
    file_size_bytes INTEGER NOT NULL,
    min_start_time_ns INTEGER NOT NULL,
    max_end_time_ns INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_files_date ON parquet_files(date_partition);
CREATE INDEX IF NOT EXISTS idx_files_created ON parquet_files(created_at);

-- Storage statistics
CREATE TABLE IF NOT EXISTS storage_stats (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    total_traces INTEGER NOT NULL DEFAULT 0,
    total_spans INTEGER NOT NULL DEFAULT 0,
    total_parquet_bytes INTEGER NOT NULL DEFAULT 0,
    total_parquet_files INTEGER NOT NULL DEFAULT 0,
    last_updated INTEGER NOT NULL
);
"#;
