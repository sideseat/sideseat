use super::render::*;
use super::*;

/// Fetch the winning delivery of a span identity.
pub fn span_by_id() -> SelectStatement {
    SelectStatement {
        operation: QueryOperation::GetSpan,
        projection: Projection::SpanDetail,
        relation: Relation::Spans,
        predicates: vec![
            Equality {
                column: Column::Project,
                binding: Binding::ProjectId,
            },
            Equality {
                column: Column::Trace,
                binding: Binding::TraceId,
            },
            Equality {
                column: Column::Span,
                binding: Binding::SpanId,
            },
        ],
        limit: Some(1),
    }
}

/// Build the filtered span page as one semantic operation.
pub fn list_spans(params: &ListSpansParams, backend: Backend) -> PageQuery {
    let dialect = analytics_dialect(backend);
    let mut conditions = vec!["project_id = ?".to_string()];
    let mut values = vec![QueryValue::String(params.project_id.to_string())];

    push_optional_eq(
        &mut conditions,
        &mut values,
        "trace_id",
        params.trace_id.as_deref(),
    );

    if let Some(session_id) = params.session_id.as_deref() {
        conditions.push(format!(
            "trace_id IN ({})",
            dialect.traces_of_session_relation()
        ));
        values.extend(dialect.traces_of_session_values(params.project_id.as_str(), session_id));
    }

    push_optional_eq(
        &mut conditions,
        &mut values,
        "user_id",
        params.user_id.as_deref(),
    );
    if let Some(environments) = &params.environment
        && !environments.is_empty()
    {
        conditions.push(format!(
            "environment IN ({})",
            placeholders(environments.len())
        ));
        values.extend(environments.iter().cloned().map(QueryValue::String));
    }
    push_optional_eq(
        &mut conditions,
        &mut values,
        "span_category",
        params.span_category.as_deref(),
    );
    push_optional_eq(
        &mut conditions,
        &mut values,
        "observation_type",
        params.observation_type.as_deref(),
    );
    push_optional_eq(
        &mut conditions,
        &mut values,
        "framework",
        params.framework.as_deref(),
    );
    push_optional_eq(
        &mut conditions,
        &mut values,
        "gen_ai_request_model",
        params.gen_ai_request_model.as_deref(),
    );
    push_optional_eq(
        &mut conditions,
        &mut values,
        "status_code",
        params.status_code.as_deref(),
    );

    if let Some(from) = &params.from_timestamp {
        conditions.push(dialect.timestamp_comparison("timestamp_start", ">="));
        values.push(dialect.timestamp_value(from));
    }
    if let Some(to) = &params.to_timestamp {
        conditions.push(dialect.timestamp_comparison("timestamp_start", "<="));
        values.push(dialect.timestamp_value(to));
    }
    if params.is_observation == Some(true) {
        conditions.push(format!("({})", crate::display::genai_span_predicate("")));
    }

    for filter in &params.filters {
        if filter.is_vacuous() {
            continue;
        }
        if filter.column() == "session_id" {
            let twin = filter.positive_twin();
            let rendered = twin.as_ref().unwrap_or(filter);
            let quantifier = if twin.is_some() { "NOT IN" } else { "IN" };
            let mut inner_values = Vec::new();
            let condition = dialect.render_filter_against(
                rendered,
                dialect.canonical_session_column(),
                &mut inner_values,
            );
            conditions.push(format!(
                "trace_id {quantifier} (SELECT cts.trace_id FROM ({}) cts \
                 WHERE cts.project_id = ? AND {condition})",
                dialect.canonical_trace_sessions_relation()
            ));
            values.push(QueryValue::String(params.project_id.to_string()));
            values.extend(inner_values);
            continue;
        }
        conditions.push(dialect.render_filter_column(
            filter,
            columns::map_span_column(filter.column()),
            "",
            &mut values,
        ));
    }

    let where_clause = conditions.join(" AND ");
    let source = dialect.span_page_relation();
    let order = span_order(params);
    let offset = params.page.saturating_sub(1) * params.limit;

    PageQuery {
        count: ParameterizedQuery {
            sql: dialect.span_count_sql(source, &where_clause),
            params: values.clone(),
        },
        rows: ParameterizedQuery {
            sql: format!(
                "SELECT\n{}\nFROM {source}\nWHERE {where_clause}\n\
                 ORDER BY {order}\nLIMIT {} OFFSET {offset}",
                dialect.span_detail_projection(),
                params.limit,
            ),
            params: values,
        },
    }
}

/// Build a stable cursor page over winning span deliveries.
///
/// The optional traversal watermark is part of the winner relation itself. Applying it after
/// deduplication would discard a post-watermark re-delivery without promoting the older delivery
/// that the traversal is supposed to see.
pub fn feed_spans(params: &FeedSpansParams, backend: Backend) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let (source, mut values) = match (backend, params.ingested_before_us) {
        (Backend::Duckdb, Some(watermark)) => crate::winners::duckdb_winning_spans(Some(watermark)),
        (Backend::Duckdb, None) => (
            DuckdbAnalyticsDialect.span_page_relation().to_string(),
            Vec::new(),
        ),
        (Backend::Clickhouse, Some(watermark)) => (
            "(SELECT * FROM otel_spans \
              WHERE toInt64(toUnixTimestamp64Micro(ingested_at)) < ? \
              ORDER BY ingested_at DESC LIMIT 1 BY project_id, trace_id, span_id)"
                .to_string(),
            vec![QueryValue::Int64(watermark)],
        ),
        (Backend::Clickhouse, None) => (
            ClickhouseAnalyticsDialect.span_page_relation().to_string(),
            Vec::new(),
        ),
    };
    let mut conditions = vec!["project_id = ?".to_string()];
    values.push(QueryValue::String(params.project_id.to_string()));

    if let Some((cursor_time_us, cursor_span_id, cursor_trace_id)) = &params.cursor {
        let ingested = match backend {
            Backend::Duckdb => "EPOCH_US(ingested_at)",
            Backend::Clickhouse => "toInt64(toUnixTimestamp64Micro(ingested_at))",
        };
        conditions.push(format!("({ingested}, span_id, trace_id) < (?, ?, ?)"));
        values.push(QueryValue::Int64(*cursor_time_us));
        values.push(QueryValue::String(cursor_span_id.clone()));
        values.push(QueryValue::String(cursor_trace_id.clone()));
    }
    if let Some(start) = &params.start_time {
        conditions.push(dialect.timestamp_comparison("timestamp_start", ">="));
        values.push(dialect.timestamp_value(start));
    }
    if let Some(end) = &params.end_time {
        conditions.push(dialect.timestamp_comparison("timestamp_start", "<"));
        values.push(dialect.timestamp_value(end));
    }
    if params.is_observation == Some(true) {
        conditions.push(format!("({})", crate::display::genai_span_predicate("")));
    }

    ParameterizedQuery {
        sql: format!(
            "SELECT\n{}\nFROM {source}\nWHERE {}\n\
             ORDER BY ingested_at DESC, span_id DESC, trace_id DESC\nLIMIT {}",
            dialect.span_detail_projection(),
            conditions.join(" AND "),
            params.limit,
        ),
        params: values,
    }
}

fn filter_option_scope(
    project_id: &str,
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    alias: &str,
    backend: Backend,
) -> (String, Vec<QueryValue>) {
    let dialect = analytics_dialect(backend);
    let column = |name: &str| {
        if alias.is_empty() {
            name.to_string()
        } else {
            format!("{alias}.{name}")
        }
    };
    let mut conditions = vec![format!("{} = ?", column("project_id"))];
    let mut values = vec![QueryValue::String(project_id.to_string())];
    if let Some(from) = from_timestamp {
        conditions.push(dialect.timestamp_comparison(&column("timestamp_start"), ">="));
        values.push(dialect.timestamp_value(from));
    }
    if let Some(to) = to_timestamp {
        conditions.push(dialect.timestamp_comparison(&column("timestamp_start"), "<="));
        values.push(dialect.timestamp_value(to));
    }
    (conditions.join(" AND "), values)
}

fn filter_option_value(expression: &str, backend: Backend) -> String {
    match backend {
        Backend::Duckdb => expression.to_string(),
        Backend::Clickhouse => format!("toNullable({expression})"),
    }
}

/// Build validated trace filter-option statements.
pub fn trace_filter_options(
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    backend: Backend,
) -> Vec<FilterOptionQuery> {
    let dialect = analytics_dialect(backend);
    let source = dialect.span_page_relation();
    let (scope, values) =
        filter_option_scope(project_id, from_timestamp, to_timestamp, "s", backend);
    let canonical_name = dialect
        .canonical_session_column()
        .rsplit('.')
        .next()
        .expect("canonical session column");
    columns
        .iter()
        .filter_map(|column| {
            let span_column = TRACE_FILTER_OPTION_COLUMNS
                .iter()
                .find(|(view_column, _)| *view_column == column.as_str())
                .map(|(_, span_column)| *span_column)?;
            let sql = if column == "session_id" {
                let value = filter_option_value(&format!("cts.{canonical_name}"), backend);
                format!(
                    "SELECT {value} AS value, \
                            COUNT(DISTINCT cts.trace_id) AS count \
                     FROM ({}) cts \
                     WHERE (cts.project_id, cts.trace_id) IN (\
                       SELECT s.project_id, s.trace_id FROM {source} s WHERE {scope}\
                     ) \
                     GROUP BY cts.{canonical_name} \
                     ORDER BY count DESC, value ASC LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}",
                    dialect.canonical_trace_sessions_relation(),
                )
            } else if column == "trace_name" {
                let flavor = match backend {
                    Backend::Duckdb => crate::display::DisplayNameDialect::DuckDb,
                    Backend::Clickhouse => crate::display::DisplayNameDialect::ClickHouse,
                };
                let display_name = crate::display::trace_display_name("s", flavor);
                let value = filter_option_value(&display_name, backend);
                format!(
                    "SELECT value, COUNT(DISTINCT trace_id) AS count \
                     FROM (\
                       SELECT s.trace_id, {value} AS value \
                       FROM {source} s WHERE {scope} GROUP BY s.trace_id\
                     ) names \
                     WHERE value IS NOT NULL \
                     GROUP BY value ORDER BY count DESC, value ASC \
                     LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
                )
            } else {
                let value = filter_option_value(&format!("s.{span_column}"), backend);
                format!(
                    "SELECT {value} AS value, \
                            COUNT(DISTINCT s.trace_id) AS count \
                     FROM {source} s \
                     WHERE {scope} AND s.{span_column} IS NOT NULL \
                     GROUP BY s.{span_column} \
                     ORDER BY count DESC, value ASC LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
                )
            };
            Some(FilterOptionQuery {
                column: column.clone(),
                query: ParameterizedQuery {
                    sql,
                    params: values.clone(),
                },
            })
        })
        .collect()
}

/// Build the trace-tag option statement.
pub fn trace_tag_options(
    project_id: &str,
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    backend: Backend,
) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let source = dialect.span_page_relation();
    let (scope, values) =
        filter_option_scope(project_id, from_timestamp, to_timestamp, "s", backend);
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT tag AS value, COUNT(DISTINCT trace_id) AS count \
             FROM (\
               SELECT s.trace_id, \
                      UNNEST(from_json(s.tags, '[\"VARCHAR\"]')) AS tag \
               FROM {source} s \
               WHERE {scope} AND s.tags IS NOT NULL AND s.tags != '[]'\
             ) tags \
             WHERE tag IS NOT NULL AND tag != '' \
             GROUP BY tag ORDER BY count DESC, value ASC \
             LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
        ),
        Backend::Clickhouse => format!(
            "SELECT toNullable(arrayJoin(JSONExtractArrayRaw(ifNull(s.tags, '[]')))) AS value, \
                    count(DISTINCT s.trace_id) AS count \
             FROM {source} s \
             WHERE {scope} AND s.tags IS NOT NULL AND s.tags != '[]' \
             GROUP BY value ORDER BY count DESC, value ASC \
             LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
        ),
    };
    ParameterizedQuery {
        sql,
        params: values,
    }
}

/// Build validated span filter-option statements over winning span identities.
pub fn span_filter_options(
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    observations_only: bool,
    backend: Backend,
) -> Vec<FilterOptionQuery> {
    let dialect = analytics_dialect(backend);
    let source = dialect.span_page_relation();
    let (mut scope, values) =
        filter_option_scope(project_id, from_timestamp, to_timestamp, "s", backend);
    if observations_only {
        scope.push_str(&format!(
            " AND ({})",
            crate::display::genai_span_predicate("s")
        ));
    }
    columns
        .iter()
        .filter(|column| SPAN_FILTER_OPTION_COLUMNS.contains(&column.as_str()))
        .map(|column| FilterOptionQuery {
            column: column.clone(),
            query: ParameterizedQuery {
                sql: {
                    let value = filter_option_value(&format!("s.{column}"), backend);
                    format!(
                        "SELECT {value} AS value, COUNT(*) AS count \
                         FROM {source} s \
                         WHERE {scope} AND s.{column} IS NOT NULL \
                         GROUP BY s.{column} \
                         ORDER BY count DESC, value ASC LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
                    )
                },
                params: values.clone(),
            },
        })
        .collect()
}

/// Build validated session filter-option statements.
///
/// A value belongs to a session when any winning span of any canonically-owned trace carries it;
/// the naming span itself need not carry the value or even a non-null `session_id`.
pub fn session_filter_options(
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    backend: Backend,
) -> Vec<FilterOptionQuery> {
    let dialect = analytics_dialect(backend);
    let source = dialect.span_page_relation();
    let (scope, values) =
        filter_option_scope(project_id, from_timestamp, to_timestamp, "s", backend);
    let canonical_column = dialect.canonical_session_column();
    columns
        .iter()
        .filter(|column| SESSION_FILTER_OPTION_COLUMNS.contains(&column.as_str()))
        .map(|column| FilterOptionQuery {
            column: column.clone(),
            query: ParameterizedQuery {
                sql: {
                    let value = filter_option_value(&format!("s.{column}"), backend);
                    format!(
                        "SELECT {value} AS value, \
                            COUNT(DISTINCT {canonical_column}) AS count \
                     FROM {source} s \
                     JOIN ({}) cts \
                       ON cts.project_id = s.project_id AND cts.trace_id = s.trace_id \
                     WHERE {scope} AND s.{column} IS NOT NULL \
                     GROUP BY s.{column} \
                     ORDER BY count DESC, value ASC LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}",
                        dialect.canonical_trace_sessions_relation(),
                    )
                },
                params: values.clone(),
            },
        })
        .collect()
}
