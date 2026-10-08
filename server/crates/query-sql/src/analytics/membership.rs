use super::*;
use super::{render::*, traces::*};

/// Build the two-stage trace aggregate page.
pub fn list_traces(params: &ListTracesParams, backend: Backend) -> PageQuery {
    match backend {
        Backend::Duckdb => duckdb_trace_page(params),
        Backend::Clickhouse => clickhouse_trace_page(params),
    }
}

/// Resolve each requested trace to the session on its earliest winning span.
pub fn trace_session_pairs(
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if trace_ids.is_empty() {
        return None;
    }
    let session = match backend {
        Backend::Duckdb => "arg_min(session_id, (timestamp_start, span_id))",
        Backend::Clickhouse => "argMin(assumeNotNull(session_id), (timestamp_start, span_id))",
    };
    if backend == Backend::Duckdb {
        let (winner, winner_values) = crate::winners::duckdb_winner_condition(as_of_us);
        let mut params = Vec::with_capacity(3 + trace_ids.len());
        params.push(QueryValue::String(project_id.to_string()));
        params.extend(winner_values);
        params.extend(trace_ids.iter().cloned().map(QueryValue::String));
        return Some(ParameterizedQuery {
            sql: format!(
                "SELECT trace_id, {session} AS session \
                 FROM (SELECT * FROM otel_spans \
                       WHERE project_id = ? AND {winner} AND trace_id IN ({})) \
                 WHERE session_id IS NOT NULL AND session_id != '' GROUP BY trace_id",
                placeholders(trace_ids.len())
            ),
            params,
        });
    }

    let (source, mut params) = membership_source(as_of_us, backend);
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(trace_ids.iter().cloned().map(QueryValue::String));
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT trace_id, {session} AS session \
             FROM {source} \
             WHERE project_id = ? AND trace_id IN ({}) \
             AND session_id IS NOT NULL AND session_id != '' GROUP BY trace_id",
            placeholders(trace_ids.len())
        ),
        params,
    })
}

/// Resolve requested traces to the distinct sessions on their earliest winning spans.
pub fn session_ids_for_traces(
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if trace_ids.is_empty() {
        return None;
    }
    let (source, mut params) = membership_source(as_of_us, backend);
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(trace_ids.iter().cloned().map(QueryValue::String));
    let session = match backend {
        Backend::Duckdb => "arg_min(session_id, (timestamp_start, span_id))",
        Backend::Clickhouse => "argMin(assumeNotNull(session_id), (timestamp_start, span_id))",
    };
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT DISTINCT canonical_session FROM ( \
               SELECT {session} AS canonical_session \
               FROM {source} \
               WHERE project_id = ? AND trace_id IN ({}) \
               AND session_id IS NOT NULL AND session_id != '' GROUP BY trace_id \
             )",
            placeholders(trace_ids.len())
        ),
        params,
    })
}

/// Resolve requested sessions to the traces whose earliest winning span names one of them.
pub fn trace_ids_for_sessions(
    project_id: &str,
    session_ids: &[String],
    as_of_us: Option<i64>,
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if session_ids.is_empty() {
        return None;
    }
    let (source, mut params) = membership_source(as_of_us, backend);
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(session_ids.iter().cloned().map(QueryValue::String));
    let session = match backend {
        Backend::Duckdb => "arg_min(session_id, (timestamp_start, span_id))",
        Backend::Clickhouse => "argMin(assumeNotNull(session_id), (timestamp_start, span_id))",
    };
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT trace_id FROM ( \
               SELECT trace_id, {session} AS canonical_session \
               FROM {source} \
               WHERE project_id = ? AND session_id IS NOT NULL AND session_id != '' \
               GROUP BY trace_id \
             ) WHERE canonical_session IN ({})",
            placeholders(session_ids.len())
        ),
        params,
    })
}

fn membership_source(as_of_us: Option<i64>, backend: Backend) -> (String, Vec<QueryValue>) {
    match (backend, as_of_us) {
        (Backend::Duckdb, Some(us)) => crate::winners::duckdb_winning_spans(Some(us)),
        (Backend::Duckdb, None) => (
            DuckdbAnalyticsDialect.span_page_relation().to_string(),
            Vec::new(),
        ),
        (Backend::Clickhouse, Some(us)) => (
            "(SELECT * FROM otel_spans \
              WHERE toInt64(toUnixTimestamp64Micro(ingested_at)) < ? \
              ORDER BY ingested_at DESC LIMIT 1 BY project_id, trace_id, span_id)"
                .to_string(),
            vec![QueryValue::Int64(us)],
        ),
        (Backend::Clickhouse, None) => ("otel_spans FINAL".to_string(), Vec::new()),
    }
}

/// Read the event and link counts of each requested span identity's winning row.
///
/// Columns, not a length over the raw span JSON: the counts are two small integers the extraction already knows,
/// and reading them from a JSON blob was the only reason a list needed that blob at all.
pub fn span_counts_bulk(
    project_id: &str,
    spans: &[(String, String)],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if spans.is_empty() {
        return None;
    }
    let (source, mut params) = winning_identities(spans, "event_count, link_count", backend);
    params.push(QueryValue::String(project_id.to_string()));
    let identities = identity_params(spans, &mut params);
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT trace_id, span_id, event_count, link_count \
             FROM {source} \
             WHERE project_id = ? AND (trace_id, span_id) IN ({identities})"
        ),
        params,
    })
}

/// The winning rows of the requested span identities, with `columns` beside the identity.
///
/// DuckDB reads the identities' revisions by `span_id` through its index and ranks only those, so the cost is
/// the identities' revisions rather than the table (`crate::keyed`); ranking every revision of every span and
/// then filtering, as `span_page_relation` does, read and numbered the whole table. A keyed read binds its span
/// ids first, so they lead the parameters. ClickHouse merges per sorting key with `FINAL`.
fn winning_identities(
    spans: &[(String, String)],
    columns: &str,
    backend: Backend,
) -> (String, Vec<QueryValue>) {
    match backend {
        Backend::Duckdb => {
            let keys =
                crate::keyed::distinct_keys(spans.iter().map(|(_, span_id)| span_id.as_str()));
            let params = keys
                .iter()
                .map(|key| QueryValue::String((*key).to_string()))
                .collect();
            let keyed = crate::keyed::duckdb_keyed(
                "otel_spans",
                "span_id",
                &format!("project_id, trace_id, span_id, superseded_at, {columns}"),
                keys.len(),
            );
            (
                format!("(SELECT * FROM {keyed} WHERE superseded_at IS NULL)"),
                params,
            )
        }
        Backend::Clickhouse => (
            analytics_dialect(backend).span_page_relation().to_string(),
            Vec::new(),
        ),
    }
}

/// `(?, ?), …` for `spans`, their values appended to `params`.
fn identity_params(spans: &[(String, String)], params: &mut Vec<QueryValue>) -> String {
    for (trace_id, span_id) in spans {
        params.push(QueryValue::String(trace_id.clone()));
        params.push(QueryValue::String(span_id.clone()));
    }
    std::iter::repeat_n("(?, ?)", spans.len())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Read the raw record each requested span identity's winning row names.
pub fn span_raw_ids(
    project_id: &str,
    spans: &[(String, String)],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if spans.is_empty() {
        return None;
    }
    let (source, mut params) = winning_identities(spans, "raw_id", backend);
    params.push(QueryValue::String(project_id.to_string()));
    let identities = identity_params(spans, &mut params);
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT trace_id, span_id, raw_id FROM {source} \
             WHERE project_id = ? AND (trace_id, span_id) IN ({identities}) AND raw_id IS NOT NULL"
        ),
        params,
    })
}

/// Read the subset of requested trace ids that still have at least one winning span.
pub fn surviving_trace_ids(
    project_id: &str,
    trace_ids: &[String],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    trace_identity_read(
        project_id,
        trace_ids,
        backend,
        "DISTINCT trace_id",
        "",
        QueryOperation::TracesWithoutSpans,
    )
}

/// Read every field that can carry an external file reference for selected winning spans.
pub fn file_reference_fields(
    project_id: &str,
    trace_ids: &[String],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    let projection = match backend {
        Backend::Duckdb => "messages, tool_definitions, metadata",
        Backend::Clickhouse => {
            "coalesce(messages, ''), coalesce(tool_definitions, ''), coalesce(metadata, '')"
        }
    };
    trace_identity_read(
        project_id,
        trace_ids,
        backend,
        projection,
        ", messages, tool_definitions, metadata",
        QueryOperation::FileReferenceFieldsForTraces,
    )
}

/// `projection` over the winning rows of these traces. `columns` lists - after a comma - the columns
/// `projection` reads beyond the identity: DuckDB fetches only those for the keyed rows, and fetching every
/// column measured three times slower than scanning a small table whole.
fn trace_identity_read(
    project_id: &str,
    trace_ids: &[String],
    backend: Backend,
    projection: &str,
    columns: &str,
    _operation: QueryOperation,
) -> Option<ParameterizedQuery> {
    if trace_ids.is_empty() {
        return None;
    }
    // DuckDB reads the traces' rows by `trace_id` through its index and ranks only those (`crate::keyed`).
    let mut params = Vec::with_capacity(1 + trace_ids.len() * 2);
    let source = match backend {
        Backend::Duckdb => {
            let keys = crate::keyed::distinct_keys(trace_ids.iter().map(String::as_str));
            params.extend(
                keys.iter()
                    .map(|key| QueryValue::String((*key).to_string())),
            );
            let keyed = crate::keyed::duckdb_keyed(
                "otel_spans",
                "trace_id",
                &format!("project_id, trace_id, span_id, superseded_at{columns}"),
                keys.len(),
            );
            format!("(SELECT * FROM {keyed} WHERE superseded_at IS NULL)")
        }
        Backend::Clickhouse => analytics_dialect(backend).span_page_relation().to_string(),
    };
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(trace_ids.iter().cloned().map(QueryValue::String));
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT {projection} FROM {source} \
             WHERE project_id = ? AND trace_id IN ({})",
            placeholders(trace_ids.len())
        ),
        params,
    })
}

/// Build exact span/metric row counts for one project.
pub fn project_row_count(
    project_id: &str,
    backend: Backend,
    metrics_table: Option<&str>,
) -> ProjectRowCountPlan {
    let project_params = || vec![QueryValue::String(project_id.to_string())];
    match backend {
        Backend::Duckdb => ProjectRowCountPlan {
            spans: ParameterizedQuery {
                sql: "SELECT COUNT(*) FROM otel_spans WHERE project_id = ?".to_string(),
                params: project_params(),
            },
            metrics_table_exists: None,
            metrics: ParameterizedQuery {
                sql: "SELECT COUNT(*) FROM otel_metrics WHERE project_id = ?".to_string(),
                params: project_params(),
            },
            logs: ParameterizedQuery {
                sql: "SELECT COUNT(*) FROM otel_logs WHERE project_id = ?".to_string(),
                params: project_params(),
            },
        },
        Backend::Clickhouse => {
            let metrics_table = metrics_table
                .expect("ClickHouse project row count needs its configured metric table");
            assert!(
                is_plain_identifier(metrics_table),
                "invalid metrics table: {metrics_table:?}"
            );
            ProjectRowCountPlan {
                spans: ParameterizedQuery {
                    sql: "SELECT count() FROM otel_spans FINAL WHERE project_id = ?".to_string(),
                    params: project_params(),
                },
                metrics_table_exists: Some(ParameterizedQuery {
                    sql: "SELECT count() FROM system.tables \
                          WHERE database = currentDatabase() AND name = ?"
                        .to_string(),
                    params: vec![QueryValue::String(metrics_table.to_string())],
                }),
                metrics: ParameterizedQuery {
                    sql: "SELECT count() FROM otel_metrics FINAL WHERE project_id = ?".to_string(),
                    params: project_params(),
                },
                logs: ParameterizedQuery {
                    sql: "SELECT count() FROM otel_logs FINAL WHERE project_id = ?".to_string(),
                    params: project_params(),
                },
            }
        }
    }
}

/// Build attributable logical-byte sums, optionally restricted to rows under an active hold.
pub fn project_logical_bytes(
    project_id: &str,
    backend: Backend,
    held_at: Option<chrono::DateTime<chrono::Utc>>,
) -> ProjectLogicalBytesPlan {
    let predicate = if held_at.is_some() {
        "project_id = ? AND hold_until IS NOT NULL AND hold_until >= ?"
    } else {
        "project_id = ?"
    };
    let params = || {
        let mut values = vec![QueryValue::String(project_id.to_string())];
        if let Some(held_at) = held_at {
            values.push(match backend {
                Backend::Duckdb => QueryValue::String(held_at.to_rfc3339()),
                Backend::Clickhouse => QueryValue::Int64(held_at.timestamp_micros()),
            });
        }
        values
    };
    let sum = |relation: &str| ParameterizedQuery {
        sql: match backend {
            Backend::Duckdb => format!(
                "SELECT COALESCE(SUM(logical_bytes), 0)::UBIGINT FROM {relation} WHERE {predicate}"
            ),
            Backend::Clickhouse => format!(
                "SELECT toUInt64(sum(logical_bytes)) FROM {relation} FINAL WHERE {predicate}"
            ),
        },
        params: params(),
    };
    let spans = match backend {
        Backend::Duckdb => sum(crate::winners::DUCKDB_WINNING_SPANS),
        Backend::Clickhouse => sum("otel_spans"),
    };
    ProjectLogicalBytesPlan {
        spans,
        metrics: sum("otel_metrics"),
        logs: sum("otel_logs"),
    }
}

/// Select a bounded oldest-first pressure batch from the winning, non-held span relation.
pub fn oldest_reclaimable_spans(
    project_id: &str,
    backend: Backend,
    target_bytes: u64,
    now: chrono::DateTime<chrono::Utc>,
    limit: usize,
) -> ParameterizedQuery {
    let relation = match backend {
        Backend::Duckdb => crate::winners::DUCKDB_WINNING_SPANS,
        Backend::Clickhouse => "otel_spans FINAL",
    };
    let now = match backend {
        Backend::Duckdb => QueryValue::String(now.to_rfc3339()),
        Backend::Clickhouse => QueryValue::Int64(now.timestamp_micros()),
    };
    ParameterizedQuery::new(
        format!(
            "SELECT trace_id, span_id, logical_bytes FROM (\
                 SELECT trace_id, span_id, logical_bytes, \
                        ROW_NUMBER() OVER (\
                            ORDER BY timestamp_start ASC, trace_id ASC, span_id ASC\
                        ) AS pressure_rank, \
                        COALESCE(SUM(logical_bytes) OVER (\
                            ORDER BY timestamp_start ASC, trace_id ASC, span_id ASC \
                            ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING\
                        ), 0) AS bytes_before \
                 FROM {relation} \
                 WHERE project_id = ? AND (hold_until IS NULL OR hold_until < ?)\
             ) WHERE pressure_rank <= ? AND bytes_before < ? \
             ORDER BY pressure_rank"
        ),
        vec![
            QueryValue::String(project_id.to_string()),
            now,
            QueryValue::Int64(i64::try_from(limit).unwrap_or(i64::MAX)),
            QueryValue::Int64(i64::try_from(target_bytes).unwrap_or(i64::MAX)),
        ],
    )
}

/// The newest committed ingestion timestamp for one project.
///
/// This intentionally reads the raw relation: a later re-delivery is itself a commit that must advance the
/// traversal watermark, whether or not it replaces an earlier logical span.
pub fn max_ingested_at_us(project_id: &str, backend: Backend) -> ParameterizedQuery {
    let expression = match backend {
        Backend::Duckdb => "MAX(EPOCH_US(ingested_at))",
        Backend::Clickhouse => "max(toInt64(toUnixTimestamp64Micro(ingested_at)))",
    };
    ParameterizedQuery::new(
        format!(
            "SELECT {expression} AS max_ingested_at_us \
             FROM otel_spans WHERE project_id = ?"
        ),
        vec![QueryValue::String(project_id.to_string())],
    )
}

/// Enumerate every project represented by an analytics signal.
pub fn analytics_project_ids(backend: Backend, limit: usize) -> ParameterizedQuery {
    let (spans, metrics, logs) = match backend {
        Backend::Duckdb => ("otel_spans", "otel_metrics", "otel_logs"),
        Backend::Clickhouse => ("otel_spans FINAL", "otel_metrics FINAL", "otel_logs FINAL"),
    };
    ParameterizedQuery::new(
        format!(
            "SELECT project_id FROM (\
                 SELECT project_id FROM {spans} \
                 UNION ALL SELECT project_id FROM {metrics} \
                 UNION ALL SELECT project_id FROM {logs}\
             ) GROUP BY project_id ORDER BY project_id LIMIT ?"
        ),
        vec![QueryValue::Int64(i64::try_from(limit).unwrap_or(i64::MAX))],
    )
}

/// Count winning span identities for an explicit maintenance project set.
pub fn span_counts_by_project(
    project_ids: &[String],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if project_ids.is_empty() {
        return None;
    }
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT project_id, COUNT(*) AS cnt FROM (\
             SELECT DISTINCT project_id, trace_id, span_id FROM otel_spans \
             WHERE project_id IN ({})) GROUP BY project_id",
            placeholders(project_ids.len())
        ),
        Backend::Clickhouse => format!(
            "SELECT project_id, count() AS cnt FROM otel_spans FINAL \
             WHERE project_id IN ({}) GROUP BY project_id",
            placeholders(project_ids.len())
        ),
    };
    Some(ParameterizedQuery {
        sql,
        params: project_ids
            .iter()
            .cloned()
            .map(QueryValue::String)
            .collect(),
    })
}

/// Read a bounded page of winning spans from one trace in event-time order.
pub fn spans_for_trace(
    project_id: &str,
    trace_id: &str,
    limit: usize,
    backend: Backend,
) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let limit = limit.clamp(1, QUERY_MAX_SPANS_PER_TRACE as usize);
    ParameterizedQuery {
        sql: format!(
            "SELECT\n{}\nFROM {}\nWHERE project_id = ? AND trace_id = ? \
             ORDER BY timestamp_start LIMIT ?",
            dialect.span_detail_projection(),
            dialect.span_page_relation(),
        ),
        params: vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(trace_id.to_string()),
            QueryValue::Int64(i64::try_from(limit).unwrap_or(i64::MAX)),
        ],
    }
}

/// Aggregate one trace using the same projection and token-dedup rules as the trace list.
pub fn trace_by_id(project_id: &str, trace_id: &str, backend: Backend) -> ParameterizedQuery {
    let source = analytics_dialect(backend).span_page_relation();
    let sql = match backend {
        Backend::Duckdb => format!(
            "WITH gen_totals AS ({}), \
             target AS (\
               SELECT project_id, trace_id FROM {source} \
               WHERE project_id = ? AND trace_id = ? \
               GROUP BY project_id, trace_id\
             ) \
             SELECT {} \
             FROM target t \
             JOIN {source} s ON t.project_id = s.project_id AND t.trace_id = s.trace_id \
             LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id \
             GROUP BY t.trace_id",
            duckdb_gen_totals_sql("g.project_id = ? AND g.trace_id = ?"),
            duckdb_trace_projection(),
        ),
        Backend::Clickhouse => format!(
            "WITH {}, {}, \
             target AS (\
               SELECT project_id, trace_id FROM otel_spans FINAL \
               WHERE project_id = ? AND trace_id = ? \
               GROUP BY project_id, trace_id\
             ) \
             SELECT {} \
             FROM target t \
             JOIN otel_spans s FINAL \
               ON t.project_id = s.project_id AND t.trace_id = s.trace_id \
             LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id \
             GROUP BY t.trace_id",
            clickhouse_dedup_lookup("trace_id = ?"),
            clickhouse_gen_totals_cte(Some("g.trace_id"), "g.project_id = ? AND g.trace_id = ?"),
            clickhouse_trace_projection(),
        ),
    };
    let pairs = match backend {
        Backend::Duckdb => 2,
        Backend::Clickhouse => 3,
    };
    let mut params = Vec::with_capacity(pairs * 2);
    for _ in 0..pairs {
        params.push(QueryValue::String(project_id.to_string()));
        params.push(QueryValue::String(trace_id.to_string()));
    }
    ParameterizedQuery { sql, params }
}

/// Aggregate all traces whose canonical earliest-span session matches the requested session.
pub fn traces_for_session(
    project_id: &str,
    session_id: &str,
    backend: Backend,
) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let session_relation = dialect.traces_of_session_relation();
    let source = dialect.span_page_relation();
    let sql = match backend {
        Backend::Duckdb => format!(
            "WITH session_traces AS ({session_relation}), \
             gen_totals AS ({}), \
             target AS (\
               SELECT s.project_id, s.trace_id FROM {source} s \
               WHERE s.project_id = ? \
                 AND s.trace_id IN (SELECT trace_id FROM session_traces) \
               GROUP BY s.project_id, s.trace_id\
             ) \
             SELECT {} \
             FROM target t \
             JOIN {source} s ON t.project_id = s.project_id AND t.trace_id = s.trace_id \
             LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id \
             GROUP BY t.trace_id \
             ORDER BY MIN(s.timestamp_start) DESC",
            duckdb_gen_totals_sql(
                "g.project_id = ? AND g.trace_id IN (SELECT trace_id FROM session_traces)"
            ),
            duckdb_trace_projection(),
        ),
        Backend::Clickhouse => format!(
            "WITH session_traces AS ({session_relation}), \
             {}, {}, \
             target AS (\
               SELECT s.project_id, s.trace_id FROM otel_spans s FINAL \
               WHERE s.project_id = ? \
                 AND s.trace_id IN (SELECT trace_id FROM session_traces) \
               GROUP BY s.project_id, s.trace_id\
             ) \
             SELECT {} \
             FROM target t \
             JOIN otel_spans s FINAL \
               ON t.project_id = s.project_id AND t.trace_id = s.trace_id \
             LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id \
             GROUP BY t.trace_id \
             ORDER BY min(s.timestamp_start) DESC",
            clickhouse_dedup_lookup("trace_id IN (SELECT trace_id FROM session_traces)"),
            clickhouse_gen_totals_cte(
                Some("g.trace_id"),
                "g.project_id = ? AND g.trace_id IN (SELECT trace_id FROM session_traces)"
            ),
            clickhouse_trace_projection(),
        ),
    };
    let mut params = dialect.traces_of_session_values(project_id, session_id);
    if backend == Backend::Clickhouse {
        params.push(QueryValue::String(project_id.to_string()));
    }
    params.push(QueryValue::String(project_id.to_string()));
    params.push(QueryValue::String(project_id.to_string()));
    ParameterizedQuery { sql, params }
}

/// Aggregate one session using canonical trace membership and shared token-dedup rules.
pub fn session_by_id(project_id: &str, session_id: &str, backend: Backend) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let session_relation = dialect.traces_of_session_relation();
    let source = dialect.span_page_relation();
    let totals_columns = [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "reasoning_tokens",
        "input_cost",
        "output_cost",
        "cache_read_cost",
        "cache_write_cost",
        "reasoning_cost",
        "total_cost",
    ];
    let session_totals = totals_columns
        .iter()
        .map(|column| format!("COALESCE(SUM({column}), 0) AS {column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let projected_totals = totals_columns
        .iter()
        .map(|column| {
            let cast = if backend == Backend::Duckdb && column.ends_with("_cost") {
                "::DOUBLE"
            } else {
                ""
            };
            format!("COALESCE(MAX(gt.{column}), 0){cast} AS {column}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let (session_expr, user_expr, environment_expr, start_expr, end_expr, observation_count) =
        match backend {
            Backend::Duckdb => (
                "?",
                "FIRST(s.user_id ORDER BY s.timestamp_start, s.span_id) \
                 FILTER (WHERE s.user_id IS NOT NULL)",
                "FIRST(s.environment ORDER BY s.timestamp_start, s.span_id) \
                 FILTER (WHERE s.environment IS NOT NULL)",
                "EPOCH_US(MIN(s.timestamp_start))",
                "EPOCH_US(MAX(COALESCE(s.timestamp_end, s.timestamp_start)))",
                "COUNT(*) FILTER (WHERE s.observation_type != 'span')",
            ),
            Backend::Clickhouse => (
                "toNullable(?)",
                "argMinIf(s.user_id, (s.timestamp_start, s.span_id), s.user_id IS NOT NULL)",
                "argMinIf(s.environment, (s.timestamp_start, s.span_id), \
                 s.environment IS NOT NULL)",
                "toInt64(toUnixTimestamp64Micro(min(s.timestamp_start)))",
                "toInt64(toUnixTimestamp64Micro(\
                 max(coalesce(s.timestamp_end, s.timestamp_start))))",
                "countIf(s.observation_type != 'span')",
            ),
        };
    let prefix = match backend {
        Backend::Duckdb => format!(
            "WITH session_traces AS ({session_relation}), \
             gen_totals_by_trace AS ({}),",
            duckdb_gen_totals_sql(
                "g.project_id = ? AND g.trace_id IN (SELECT trace_id FROM session_traces)"
            )
        ),
        Backend::Clickhouse => {
            let totals = clickhouse_gen_totals_cte(
                Some("g.trace_id"),
                "g.project_id = ? AND g.trace_id IN (SELECT trace_id FROM session_traces)",
            )
            .replacen("gen_totals AS", "gen_totals_by_trace AS", 1);
            format!(
                "WITH session_traces AS ({session_relation}), {}, {totals},",
                clickhouse_dedup_lookup("trace_id IN (SELECT trace_id FROM session_traces)"),
            )
        }
    };
    let sql = format!(
        "{prefix} \
         session_totals AS (SELECT {session_totals} FROM gen_totals_by_trace) \
         SELECT {session_expr} AS session_id, \
                {user_expr} AS user_id, \
                {environment_expr} AS environment, \
                {start_expr} AS start_time, \
                {end_expr} AS end_time, \
                COUNT(DISTINCT s.trace_id) AS trace_count, \
                COUNT(*) AS span_count, \
                {observation_count} AS observation_count, \
                {projected_totals} \
         FROM {source} s CROSS JOIN session_totals gt \
         WHERE s.project_id = ? \
           AND s.trace_id IN (SELECT trace_id FROM session_traces)"
    );
    let mut params = dialect.traces_of_session_values(project_id, session_id);
    if backend == Backend::Clickhouse {
        params.push(QueryValue::String(project_id.to_string()));
    }
    params.push(QueryValue::String(project_id.to_string()));
    params.push(QueryValue::String(session_id.to_string()));
    params.push(QueryValue::String(project_id.to_string()));
    ParameterizedQuery { sql, params }
}
