use super::*;

// --- Helper functions ---

pub(super) fn row_to_trace(row: &Row<'_>) -> Result<TraceRow, DuckdbError> {
    let start_time_micros: i64 = row.get(2)?;
    let end_time_micros: Option<i64> = row.get(3)?;
    let tags_json: Option<String> = row.get(21)?;
    let metadata_json: Option<String> = row.get(23)?;

    Ok(TraceRow {
        trace_id: row.get(0)?,
        trace_name: row.get(1)?,
        start_time: micros_to_datetime(start_time_micros),
        end_time: end_time_micros.map(micros_to_datetime),
        duration_ms: row.get(4)?,
        session_id: row.get(5)?,
        user_id: row.get(6)?,
        environment: row.get(7)?,
        span_count: row.get(8)?,
        input_tokens: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
        output_tokens: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
        total_tokens: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
        cache_read_tokens: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
        cache_write_tokens: row.get::<_, Option<i64>>(13)?.unwrap_or(0),
        reasoning_tokens: row.get::<_, Option<i64>>(14)?.unwrap_or(0),
        input_cost: row.get::<_, Option<f64>>(15)?.unwrap_or(0.0),
        output_cost: row.get::<_, Option<f64>>(16)?.unwrap_or(0.0),
        cache_read_cost: row.get::<_, Option<f64>>(17)?.unwrap_or(0.0),
        cache_write_cost: row.get::<_, Option<f64>>(18)?.unwrap_or(0.0),
        reasoning_cost: row.get::<_, Option<f64>>(19)?.unwrap_or(0.0),
        total_cost: row.get::<_, Option<f64>>(20)?.unwrap_or(0.0),
        tags: parse_tags(&tags_json),
        observation_count: row.get(22)?,
        metadata: metadata_json,
        input_preview: row.get(24)?,
        output_preview: row.get(25)?,
        has_error: row.get::<_, Option<bool>>(26)?.unwrap_or(false),
    })
}

pub(super) fn duckdb_values(values: &[analytics::QueryValue]) -> Vec<&dyn duckdb::ToSql> {
    values
        .iter()
        .map(|value| match value {
            analytics::QueryValue::String(value) => value as &dyn duckdb::ToSql,
            analytics::QueryValue::Int64(value) => value as &dyn duckdb::ToSql,
            analytics::QueryValue::Float64(value) => value as &dyn duckdb::ToSql,
        })
        .collect()
}

pub(super) fn execute_count_values(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<u64, DuckdbError> {
    let values = duckdb_values(query.params());
    let count: i64 = conn.query_row(query.sql(), values.as_slice(), |row| row.get(0))?;
    Ok(count as u64)
}

pub(super) fn execute_trace_query_values(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<TraceRow>, DuckdbError> {
    let mut stmt = conn.prepare(query.sql())?;
    let values = duckdb_values(query.params());
    let mut query_rows = stmt.query(values.as_slice())?;
    let mut rows = Vec::new();
    while let Some(row) = query_rows.next()? {
        rows.push(row_to_trace(row)?);
    }
    Ok(rows)
}

pub(super) fn execute_span_query_values(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<SpanRow>, DuckdbError> {
    let mut stmt = conn.prepare(query.sql())?;
    let values = duckdb_values(query.params());
    let mut query_rows = stmt.query(values.as_slice())?;
    let mut rows = Vec::new();
    while let Some(row) = query_rows.next()? {
        rows.push(row_to_span(row)?);
    }
    Ok(rows)
}

pub(super) fn row_to_span(row: &Row<'_>) -> Result<SpanRow, DuckdbError> {
    let start_time_micros: i64 = row.get(9)?;
    let end_time_micros: Option<i64> = row.get(10)?;
    let ingested_at_micros: i64 = row.get(35)?;

    Ok(SpanRow {
        trace_id: row.get(0)?,
        span_id: row.get(1)?,
        parent_span_id: row.get(2)?,
        span_name: row.get(3)?,
        span_kind: row.get(4)?,
        span_category: row.get(5)?,
        observation_type: row.get(6)?,
        framework: row.get(7)?,
        status_code: row.get(8)?,
        timestamp_start: micros_to_datetime(start_time_micros),
        timestamp_end: end_time_micros.map(micros_to_datetime),
        duration_ms: row.get(11)?,
        environment: row.get(12)?,
        session_id: row.get(13)?,
        user_id: row.get(14)?,
        gen_ai_system: row.get(15)?,
        gen_ai_request_model: row.get(16)?,
        gen_ai_agent_name: row.get(17)?,
        gen_ai_finish_reasons: row
            .get::<_, Option<String>>(18)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        gen_ai_usage_input_tokens: row.get::<_, Option<i64>>(19)?.unwrap_or(0),
        gen_ai_usage_output_tokens: row.get::<_, Option<i64>>(20)?.unwrap_or(0),
        gen_ai_usage_total_tokens: row.get::<_, Option<i64>>(21)?.unwrap_or(0),
        gen_ai_usage_cache_read_tokens: row.get::<_, Option<i64>>(22)?.unwrap_or(0),
        gen_ai_usage_cache_write_tokens: row.get::<_, Option<i64>>(23)?.unwrap_or(0),
        gen_ai_usage_reasoning_tokens: row.get::<_, Option<i64>>(24)?.unwrap_or(0),
        gen_ai_cost_input: row.get::<_, Option<f64>>(25)?.unwrap_or(0.0),
        gen_ai_cost_output: row.get::<_, Option<f64>>(26)?.unwrap_or(0.0),
        gen_ai_cost_cache_read: row.get::<_, Option<f64>>(27)?.unwrap_or(0.0),
        gen_ai_cost_cache_write: row.get::<_, Option<f64>>(28)?.unwrap_or(0.0),
        gen_ai_cost_reasoning: row.get::<_, Option<f64>>(29)?.unwrap_or(0.0),
        gen_ai_cost_total: row.get::<_, Option<f64>>(30)?.unwrap_or(0.0),
        gen_ai_usage_details: row.get(31)?,
        metadata: row.get(32)?,
        input_preview: row.get(33)?,
        output_preview: row.get(34)?,
        // Rendered from the raw record when a reader asks for it; never a column.
        raw_span: None,
        ingested_at: micros_to_datetime(ingested_at_micros),
        scope_name: row.get(36)?,
        scope_version: row.get(37)?,
    })
}

pub(super) fn execute_session_query_values(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<SessionRow>, DuckdbError> {
    let mut stmt = conn.prepare(query.sql())?;
    let values = duckdb_values(query.params());
    let mut query_rows = stmt.query(values.as_slice())?;
    let mut rows = vec![];

    while let Some(row) = query_rows.next()? {
        rows.push(row_to_session(row)?);
    }

    Ok(rows)
}

pub(super) fn row_to_session(row: &Row<'_>) -> Result<SessionRow, DuckdbError> {
    let start_time_micros: i64 = row.get(3)?;
    let end_time_micros: Option<i64> = row.get(4)?;

    Ok(SessionRow {
        session_id: row.get(0)?,
        user_id: row.get(1)?,
        environment: row.get(2)?,
        start_time: micros_to_datetime(start_time_micros),
        end_time: end_time_micros.map(micros_to_datetime),
        trace_count: row.get(5)?,
        span_count: row.get(6)?,
        observation_count: row.get(7)?,
        input_tokens: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
        output_tokens: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
        total_tokens: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
        cache_read_tokens: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
        cache_write_tokens: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
        reasoning_tokens: row.get::<_, Option<i64>>(13)?.unwrap_or(0),
        input_cost: row.get::<_, Option<f64>>(14)?.unwrap_or(0.0),
        output_cost: row.get::<_, Option<f64>>(15)?.unwrap_or(0.0),
        cache_read_cost: row.get::<_, Option<f64>>(16)?.unwrap_or(0.0),
        cache_write_cost: row.get::<_, Option<f64>>(17)?.unwrap_or(0.0),
        reasoning_cost: row.get::<_, Option<f64>>(18)?.unwrap_or(0.0),
        total_cost: row.get::<_, Option<f64>>(19)?.unwrap_or(0.0),
    })
}

pub(super) fn execute_filter_option_query(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<FilterOptionRow>, DuckdbError> {
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;
    let mut options = Vec::new();
    while let Some(row) = rows.next()? {
        let value: Option<String> = row.get(0)?;
        let count: i64 = row.get(1)?;
        if let Some(value) = value {
            options.push(FilterOptionRow {
                value,
                count: count as u64,
            });
        }
    }
    Ok(options)
}
