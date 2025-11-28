//! Centralized SQLite schema definitions
//!
//! All tables are defined here. Schema version 1 is the initial version.
//! Future migrations will be added to migrations.rs.

/// Current schema version
pub const SCHEMA_VERSION: i32 = 1;

/// Complete schema SQL (version 1)
pub const SCHEMA: &str = r#"
-- Infrastructure: Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL,
    applied_at INTEGER NOT NULL,
    description TEXT
);

-- Infrastructure: Migration history
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at INTEGER NOT NULL,
    checksum TEXT NOT NULL,
    execution_time_ms INTEGER,
    success INTEGER NOT NULL DEFAULT 1
);

-- OTEL: Sessions table (aggregates data across traces)
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    user_id TEXT,
    service_name TEXT,
    trace_count INTEGER NOT NULL DEFAULT 0,
    span_count INTEGER NOT NULL DEFAULT 0,
    total_input_tokens INTEGER,
    total_output_tokens INTEGER,
    total_tokens INTEGER,
    has_errors INTEGER NOT NULL DEFAULT 0,
    first_seen_ns INTEGER NOT NULL,
    last_seen_ns INTEGER NOT NULL,
    deleted_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_service ON sessions(service_name);
CREATE INDEX IF NOT EXISTS idx_sessions_first_seen ON sessions(first_seen_ns DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_last_seen ON sessions(last_seen_ns DESC);

-- OTEL: Trace summary table
CREATE TABLE IF NOT EXISTS traces (
    trace_id TEXT PRIMARY KEY,
    session_id TEXT,
    root_span_id TEXT,
    root_span_name TEXT,
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
    deleted_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);

CREATE INDEX IF NOT EXISTS idx_traces_session ON traces(session_id);
CREATE INDEX IF NOT EXISTS idx_traces_service ON traces(service_name);
CREATE INDEX IF NOT EXISTS idx_traces_framework ON traces(detected_framework);
CREATE INDEX IF NOT EXISTS idx_traces_start ON traces(start_time_ns DESC);
CREATE INDEX IF NOT EXISTS idx_traces_created ON traces(created_at);
CREATE INDEX IF NOT EXISTS idx_traces_time_errors ON traces(start_time_ns DESC, has_errors);

-- OTEL: Span index table (for fast queries)
CREATE TABLE IF NOT EXISTS spans (
    span_id TEXT PRIMARY KEY,
    trace_id TEXT NOT NULL,
    session_id TEXT,
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
    FOREIGN KEY (trace_id) REFERENCES traces(trace_id),
    FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);

CREATE INDEX IF NOT EXISTS idx_spans_trace ON spans(trace_id);
CREATE INDEX IF NOT EXISTS idx_spans_session ON spans(session_id);
CREATE INDEX IF NOT EXISTS idx_spans_parent ON spans(parent_span_id);
CREATE INDEX IF NOT EXISTS idx_spans_service ON spans(service_name);
CREATE INDEX IF NOT EXISTS idx_spans_framework ON spans(detected_framework);
CREATE INDEX IF NOT EXISTS idx_spans_category ON spans(detected_category);
CREATE INDEX IF NOT EXISTS idx_spans_agent ON spans(gen_ai_agent_name);
CREATE INDEX IF NOT EXISTS idx_spans_tool ON spans(gen_ai_tool_name);
CREATE INDEX IF NOT EXISTS idx_spans_model ON spans(gen_ai_request_model);
CREATE INDEX IF NOT EXISTS idx_spans_start ON spans(start_time_ns);
CREATE INDEX IF NOT EXISTS idx_spans_parquet ON spans(parquet_file);

-- OTEL: Span events index
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

-- OTEL: Parquet file tracking
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

-- OTEL: Storage statistics
CREATE TABLE IF NOT EXISTS storage_stats (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    total_traces INTEGER NOT NULL DEFAULT 0,
    total_spans INTEGER NOT NULL DEFAULT 0,
    total_parquet_bytes INTEGER NOT NULL DEFAULT 0,
    total_parquet_files INTEGER NOT NULL DEFAULT 0,
    last_updated INTEGER NOT NULL
);

-- OTEL EAV: Registered attribute keys (for discovery and validation)
CREATE TABLE IF NOT EXISTS attribute_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key_name TEXT NOT NULL,
    key_type TEXT NOT NULL DEFAULT 'string',
    entity_type TEXT NOT NULL,
    indexed INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    UNIQUE(key_name, entity_type)
);

CREATE INDEX IF NOT EXISTS idx_attr_keys_entity ON attribute_keys(entity_type, indexed);

-- OTEL EAV: Trace-level attributes
CREATE TABLE IF NOT EXISTS trace_attributes (
    trace_id TEXT NOT NULL,
    key_id INTEGER NOT NULL,
    value_str TEXT,
    value_num REAL,
    PRIMARY KEY (trace_id, key_id),
    FOREIGN KEY (trace_id) REFERENCES traces(trace_id) ON DELETE CASCADE,
    FOREIGN KEY (key_id) REFERENCES attribute_keys(id)
);

CREATE INDEX IF NOT EXISTS idx_trace_attr_str ON trace_attributes(key_id, value_str)
    WHERE value_str IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_trace_attr_num ON trace_attributes(key_id, value_num)
    WHERE value_num IS NOT NULL;

-- OTEL EAV: Span-level attributes
CREATE TABLE IF NOT EXISTS span_attributes (
    span_id TEXT NOT NULL,
    key_id INTEGER NOT NULL,
    value_str TEXT,
    value_num REAL,
    PRIMARY KEY (span_id, key_id),
    FOREIGN KEY (span_id) REFERENCES spans(span_id) ON DELETE CASCADE,
    FOREIGN KEY (key_id) REFERENCES attribute_keys(id)
);

CREATE INDEX IF NOT EXISTS idx_span_attr_str ON span_attributes(key_id, value_str)
    WHERE value_str IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_span_attr_num ON span_attributes(key_id, value_num)
    WHERE value_num IS NOT NULL;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_version_is_positive() {
        let version = SCHEMA_VERSION;
        assert!(version > 0);
    }

    #[test]
    fn test_schema_is_not_empty() {
        let schema = SCHEMA;
        assert!(!schema.is_empty());
        assert!(schema.len() > 1000); // Schema should be substantial
    }

    #[test]
    fn test_schema_contains_required_tables() {
        let required_tables = [
            "schema_version",
            "schema_migrations",
            "sessions",
            "traces",
            "spans",
            "span_events",
            "parquet_files",
            "storage_stats",
            "attribute_keys",
            "trace_attributes",
            "span_attributes",
        ];

        for table in required_tables {
            assert!(
                SCHEMA.contains(&format!("CREATE TABLE IF NOT EXISTS {}", table)),
                "Schema missing table: {}",
                table
            );
        }
    }

    #[test]
    fn test_schema_contains_required_indexes() {
        let required_indexes = [
            "idx_traces_service",
            "idx_traces_start",
            "idx_spans_trace",
            "idx_spans_parent",
            "idx_files_date",
        ];

        for index in required_indexes {
            assert!(SCHEMA.contains(index), "Schema missing index: {}", index);
        }
    }

    #[test]
    fn test_schema_has_foreign_keys() {
        assert!(SCHEMA.contains("FOREIGN KEY"));
        assert!(SCHEMA.contains("ON DELETE CASCADE"));
    }
}
