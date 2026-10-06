use super::*;

pub(super) fn push_optional_eq(
    conditions: &mut Vec<String>,
    values: &mut Vec<QueryValue>,
    column: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        conditions.push(format!("{column} = ?"));
        values.push(QueryValue::String(value.to_string()));
    }
}

pub(super) fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn span_order(params: &ListSpansParams) -> String {
    let (column, direction) = params
        .order_by
        .as_ref()
        .map(|order| {
            let column = match order.column.as_str() {
                "start_time" | "timestamp_start" => "timestamp_start",
                "end_time" | "timestamp_end" => "timestamp_end",
                "duration_ms" => "duration_ms",
                "span_name" => "span_name",
                _ => "timestamp_start",
            };
            let direction = match order.direction {
                OrderDirection::Asc => "ASC",
                OrderDirection::Desc => "DESC",
            };
            (column, direction)
        })
        .unwrap_or(("timestamp_start", "DESC"));
    format!("{column} {direction}, trace_id ASC, span_id ASC")
}

impl SelectStatement {
    pub fn operation(&self) -> QueryOperation {
        self.operation
    }

    pub fn render(&self, backend: Backend) -> RenderedQuery {
        let dialect = analytics_dialect(backend);

        let projection = match self.projection {
            Projection::SpanDetail => dialect.span_detail_projection(),
        };
        let relation = match self.relation {
            Relation::Spans => dialect.winning_spans_relation(),
        };

        let predicates = self
            .predicates
            .iter()
            .enumerate()
            .map(|(index, predicate)| {
                format!(
                    "{} = {}",
                    predicate.column.name(),
                    dialect.placeholder(index + 1)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let suffix = dialect.after_where();
        let limit = self
            .limit
            .map(|limit| format!("\nLIMIT {limit}"))
            .unwrap_or_default();

        RenderedQuery {
            sql: format!(
                "SELECT\n{projection}\nFROM {relation}\nWHERE {predicates}{suffix}{limit}"
            ),
            bindings: self
                .predicates
                .iter()
                .map(|predicate| predicate.binding)
                .collect(),
        }
    }
}

pub(super) fn analytics_dialect(backend: Backend) -> &'static dyn AnalyticsDialect {
    match backend {
        Backend::Duckdb => &DuckdbAnalyticsDialect,
        Backend::Clickhouse => &ClickhouseAnalyticsDialect,
    }
}

/// Capabilities needed to lower analytical reads.
pub(super) trait AnalyticsDialect {
    fn placeholder(&self, index: usize) -> String;
    fn winning_spans_relation(&self) -> &'static str;
    fn after_where(&self) -> &'static str;
    fn span_detail_projection(&self) -> &'static str;
    fn span_page_relation(&self) -> &'static str;
    fn traces_of_session_relation(&self) -> &'static str;
    fn traces_of_session_values(&self, project_id: &str, session_id: &str) -> Vec<QueryValue>;
    fn canonical_trace_sessions_relation(&self) -> &'static str;
    fn canonical_session_column(&self) -> &'static str;
    fn timestamp_comparison(&self, column: &str, operator: &str) -> String;
    fn timestamp_value(&self, value: &chrono::DateTime<chrono::Utc>) -> QueryValue;
    fn span_count_sql(&self, source: &str, where_clause: &str) -> String;
    fn render_filter_column(
        &self,
        filter: &Filter,
        column: &str,
        alias: &str,
        values: &mut Vec<QueryValue>,
    ) -> String;
    fn render_filter_against(
        &self,
        filter: &Filter,
        expression: &str,
        values: &mut Vec<QueryValue>,
    ) -> String;
}

pub(super) struct DuckdbAnalyticsDialect;

impl AnalyticsDialect for DuckdbAnalyticsDialect {
    fn placeholder(&self, _index: usize) -> String {
        "?".to_string()
    }

    fn winning_spans_relation(&self) -> &'static str {
        "otel_spans"
    }

    fn after_where(&self) -> &'static str {
        "\nQUALIFY ROW_NUMBER() OVER (\
         PARTITION BY project_id, trace_id, span_id \
         ORDER BY ingested_at DESC, rowid DESC) = 1"
    }

    fn span_page_relation(&self) -> &'static str {
        "(SELECT * FROM otel_spans \
         QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
         ORDER BY ingested_at DESC, rowid DESC) = 1)"
    }

    fn span_detail_projection(&self) -> &'static str {
        r#"    trace_id,
    span_id,
    parent_span_id,
    span_name,
    span_kind,
    span_category,
    observation_type,
    framework,
    status_code,
    EPOCH_US(timestamp_start) AS start_time,
    CASE WHEN timestamp_end IS NOT NULL THEN EPOCH_US(timestamp_end) END AS end_time,
    duration_ms,
    environment,
    session_id,
    user_id,
    gen_ai_system,
    gen_ai_request_model,
    gen_ai_agent_name,
    gen_ai_finish_reasons,
    gen_ai_usage_input_tokens,
    gen_ai_usage_output_tokens,
    gen_ai_usage_total_tokens,
    gen_ai_usage_cache_read_tokens,
    gen_ai_usage_cache_write_tokens,
    gen_ai_usage_reasoning_tokens,
    gen_ai_cost_input::DOUBLE AS gen_ai_cost_input,
    gen_ai_cost_output::DOUBLE AS gen_ai_cost_output,
    gen_ai_cost_cache_read::DOUBLE AS gen_ai_cost_cache_read,
    gen_ai_cost_cache_write::DOUBLE AS gen_ai_cost_cache_write,
    gen_ai_cost_reasoning::DOUBLE AS gen_ai_cost_reasoning,
    gen_ai_cost_total::DOUBLE AS gen_ai_cost_total,
    gen_ai_usage_details::VARCHAR AS gen_ai_usage_details,
    metadata::VARCHAR AS metadata,
    input_preview,
    output_preview,
    EPOCH_US(ingested_at) AS ingested_at_us,
    scope_name,
    scope_version"#
    }

    fn traces_of_session_relation(&self) -> &'static str {
        "SELECT trace_id FROM ( \
           SELECT trace_id, arg_min(session_id, (timestamp_start, span_id)) AS canonical_session \
           FROM (SELECT * FROM otel_spans \
                 WHERE project_id = ? \
                 AND trace_id IN (SELECT trace_id FROM otel_spans \
                                  WHERE project_id = ? AND session_id = ?) \
                 QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                            ORDER BY ingested_at DESC, rowid DESC) = 1) \
           WHERE session_id IS NOT NULL AND session_id != '' \
           GROUP BY trace_id \
         ) WHERE canonical_session = ?"
    }

    fn traces_of_session_values(&self, project_id: &str, session_id: &str) -> Vec<QueryValue> {
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(project_id.to_string()),
            QueryValue::String(session_id.to_string()),
            QueryValue::String(session_id.to_string()),
        ]
    }

    fn canonical_trace_sessions_relation(&self) -> &'static str {
        "SELECT project_id, trace_id, \
           arg_min(session_id, (timestamp_start, span_id)) AS session_id \
         FROM (SELECT * FROM otel_spans \
               QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                          ORDER BY ingested_at DESC, rowid DESC) = 1) \
         WHERE session_id IS NOT NULL AND session_id != '' \
         GROUP BY project_id, trace_id"
    }

    fn canonical_session_column(&self) -> &'static str {
        "cts.session_id"
    }

    fn timestamp_comparison(&self, column: &str, operator: &str) -> String {
        format!("{column} {operator} ?")
    }

    fn timestamp_value(&self, value: &chrono::DateTime<chrono::Utc>) -> QueryValue {
        QueryValue::String(value.to_rfc3339())
    }

    fn span_count_sql(&self, source: &str, where_clause: &str) -> String {
        format!(
            "SELECT COUNT(*) FROM (SELECT DISTINCT trace_id, span_id \
             FROM {source} WHERE {where_clause}) _c"
        )
    }

    fn render_filter_column(
        &self,
        filter: &Filter,
        column: &str,
        alias: &str,
        values: &mut Vec<QueryValue>,
    ) -> String {
        render_filter(
            FilterFlavor::Duckdb,
            filter,
            FilterTarget::Column { column, alias },
            values,
        )
    }

    fn render_filter_against(
        &self,
        filter: &Filter,
        expression: &str,
        values: &mut Vec<QueryValue>,
    ) -> String {
        render_filter(
            FilterFlavor::Duckdb,
            filter,
            FilterTarget::Expression(expression),
            values,
        )
    }
}

pub(super) struct ClickhouseAnalyticsDialect;

impl AnalyticsDialect for ClickhouseAnalyticsDialect {
    fn placeholder(&self, _index: usize) -> String {
        "?".to_string()
    }

    fn winning_spans_relation(&self) -> &'static str {
        "otel_spans FINAL"
    }

    fn after_where(&self) -> &'static str {
        ""
    }

    fn span_page_relation(&self) -> &'static str {
        "(SELECT * FROM otel_spans FINAL)"
    }

    fn span_detail_projection(&self) -> &'static str {
        r#"    trace_id,
    span_id,
    parent_span_id,
    span_name,
    span_kind,
    span_category,
    observation_type,
    framework,
    status_code,
    toInt64(toUnixTimestamp64Micro(timestamp_start)) AS start_time,
    if(timestamp_end IS NOT NULL, toInt64(toUnixTimestamp64Micro(timestamp_end)), NULL) AS end_time,
    duration_ms,
    environment,
    session_id,
    user_id,
    gen_ai_system,
    gen_ai_request_model,
    gen_ai_agent_name,
    gen_ai_finish_reasons,
    gen_ai_usage_input_tokens,
    gen_ai_usage_output_tokens,
    gen_ai_usage_total_tokens,
    gen_ai_usage_cache_read_tokens,
    gen_ai_usage_cache_write_tokens,
    gen_ai_usage_reasoning_tokens,
    toFloat64(gen_ai_cost_input) AS gen_ai_cost_input,
    toFloat64(gen_ai_cost_output) AS gen_ai_cost_output,
    toFloat64(gen_ai_cost_cache_read) AS gen_ai_cost_cache_read,
    toFloat64(gen_ai_cost_cache_write) AS gen_ai_cost_cache_write,
    toFloat64(gen_ai_cost_reasoning) AS gen_ai_cost_reasoning,
    toFloat64(gen_ai_cost_total) AS gen_ai_cost_total,
    gen_ai_usage_details,
    metadata,
    input_preview,
    output_preview,
    toInt64(toUnixTimestamp64Micro(ingested_at)) AS ingested_at_us,
    scope_name,
    scope_version"#
    }

    fn traces_of_session_relation(&self) -> &'static str {
        "SELECT trace_id FROM ( \
           SELECT trace_id, argMin(assumeNotNull(session_id), (timestamp_start, span_id)) \
                  AS canonical_session \
           FROM otel_spans FINAL \
           WHERE project_id = ? \
           AND trace_id IN (SELECT trace_id FROM otel_spans \
                            WHERE project_id = ? AND session_id = ?) \
           AND session_id IS NOT NULL AND session_id != '' \
           GROUP BY trace_id \
         ) WHERE canonical_session = ?"
    }

    fn traces_of_session_values(&self, project_id: &str, session_id: &str) -> Vec<QueryValue> {
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(project_id.to_string()),
            QueryValue::String(session_id.to_string()),
            QueryValue::String(session_id.to_string()),
        ]
    }

    fn canonical_trace_sessions_relation(&self) -> &'static str {
        "SELECT project_id, trace_id, \
           argMin(assumeNotNull(session_id), (timestamp_start, span_id)) AS canonical_session \
         FROM otel_spans FINAL \
         WHERE session_id IS NOT NULL AND session_id != '' \
         GROUP BY project_id, trace_id"
    }

    fn canonical_session_column(&self) -> &'static str {
        "cts.canonical_session"
    }

    fn timestamp_comparison(&self, column: &str, operator: &str) -> String {
        format!("{column} {operator} fromUnixTimestamp64Micro(?)")
    }

    fn timestamp_value(&self, value: &chrono::DateTime<chrono::Utc>) -> QueryValue {
        QueryValue::Int64(value.timestamp_micros())
    }

    fn span_count_sql(&self, source: &str, where_clause: &str) -> String {
        format!("SELECT count() AS cnt FROM {source} WHERE {where_clause}")
    }

    fn render_filter_column(
        &self,
        filter: &Filter,
        column: &str,
        alias: &str,
        values: &mut Vec<QueryValue>,
    ) -> String {
        render_filter(
            FilterFlavor::Clickhouse,
            filter,
            FilterTarget::Column { column, alias },
            values,
        )
    }

    fn render_filter_against(
        &self,
        filter: &Filter,
        expression: &str,
        values: &mut Vec<QueryValue>,
    ) -> String {
        render_filter(
            FilterFlavor::Clickhouse,
            filter,
            FilterTarget::Expression(expression),
            values,
        )
    }
}

#[derive(Clone, Copy)]
pub(super) enum FilterFlavor {
    Duckdb,
    Clickhouse,
}

pub(super) enum FilterTarget<'a> {
    Column { column: &'a str, alias: &'a str },
    Expression(&'a str),
}

impl FilterTarget<'_> {
    fn expression(&self) -> String {
        match self {
            Self::Column { column, alias: "" } => (*column).to_string(),
            Self::Column { column, alias } => format!("{alias}.{column}"),
            Self::Expression(expression) => (*expression).to_string(),
        }
    }

    fn column(&self) -> Option<&str> {
        match self {
            Self::Column { column, .. } => Some(column),
            Self::Expression(_) => None,
        }
    }
}

pub(super) fn render_filter(
    flavor: FilterFlavor,
    filter: &Filter,
    target: FilterTarget<'_>,
    values: &mut Vec<QueryValue>,
) -> String {
    if matches!(target, FilterTarget::Column { .. }) && !is_plain_identifier(filter.column()) {
        return "1 = 0".to_string();
    }

    let expression = target.expression();
    match filter {
        Filter::Datetime {
            operator, value, ..
        } => {
            let operator = match operator {
                DatetimeOp::Gt => ">",
                DatetimeOp::Lt => "<",
                DatetimeOp::Gte => ">=",
                DatetimeOp::Lte => "<=",
            };
            match flavor {
                FilterFlavor::Duckdb => {
                    values.push(QueryValue::String(value.clone()));
                    format!("{expression} {operator} ?")
                }
                FilterFlavor::Clickhouse => match chrono::DateTime::parse_from_rfc3339(value) {
                    Ok(value) => {
                        values.push(QueryValue::Int64(value.timestamp_micros()));
                        format!("{expression} {operator} fromUnixTimestamp64Micro(?)")
                    }
                    Err(_) => "1 = 0".to_string(),
                },
            }
        }
        Filter::String {
            operator, value, ..
        } => {
            let (pattern, comparison) = match operator {
                StringOp::Eq => (value.clone(), "="),
                StringOp::Contains => (format!("%{}%", escape_like_pattern(value)), "LIKE"),
                StringOp::StartsWith => (format!("{}%", escape_like_pattern(value)), "LIKE"),
                StringOp::EndsWith => (format!("%{}", escape_like_pattern(value)), "LIKE"),
            };
            values.push(QueryValue::String(pattern));
            let escape = match (flavor, operator) {
                (
                    FilterFlavor::Duckdb,
                    StringOp::Contains | StringOp::StartsWith | StringOp::EndsWith,
                ) => " ESCAPE '\\'",
                _ => "",
            };
            format!("{expression} {comparison} ?{escape}")
        }
        Filter::Number {
            operator, value, ..
        } => {
            let operator = match operator {
                NumberOp::Eq => "=",
                NumberOp::Gt => ">",
                NumberOp::Lt => "<",
                NumberOp::Gte => ">=",
                NumberOp::Lte => "<=",
            };
            let expression = match (flavor, target.column()) {
                (FilterFlavor::Clickhouse, Some(column))
                    if [
                        "gen_ai_cost_input",
                        "gen_ai_cost_output",
                        "gen_ai_cost_cache_read",
                        "gen_ai_cost_cache_write",
                        "gen_ai_cost_reasoning",
                        "gen_ai_cost_total",
                    ]
                    .contains(&column) =>
                {
                    format!("toFloat64({expression})")
                }
                _ => expression,
            };
            values.push(match flavor {
                FilterFlavor::Duckdb => QueryValue::String(value.to_string()),
                FilterFlavor::Clickhouse => QueryValue::Float64(*value),
            });
            format!("{expression} {operator} ?")
        }
        Filter::StringOptions {
            operator, value, ..
        } => {
            if value.is_empty() {
                return "1 = 1".to_string();
            }
            if target.column() == Some("tags") {
                values.extend(value.iter().cloned().map(QueryValue::String));
                return match flavor {
                    FilterFlavor::Duckdb => {
                        let tests = value
                            .iter()
                            .map(|_| {
                                let test = format!(
                                    "list_contains(from_json(ifnull({expression}, '[]'), \
                                     '[\"VARCHAR\"]'), ?)"
                                );
                                match operator {
                                    OptionsOp::AnyOf => test,
                                    OptionsOp::NoneOf => format!("NOT {test}"),
                                }
                            })
                            .collect::<Vec<_>>();
                        let join = match operator {
                            OptionsOp::AnyOf => " OR ",
                            OptionsOp::NoneOf => " AND ",
                        };
                        format!("({})", tests.join(join))
                    }
                    FilterFlavor::Clickhouse => {
                        let extracted =
                            format!("JSONExtract(ifNull({expression}, '[]'), 'Array(String)')");
                        let condition =
                            format!("hasAny({extracted}, [{}])", placeholders(value.len()));
                        match operator {
                            OptionsOp::AnyOf => condition,
                            OptionsOp::NoneOf => format!("NOT {condition}"),
                        }
                    }
                };
            }
            values.extend(value.iter().cloned().map(QueryValue::String));
            let operator = match operator {
                OptionsOp::AnyOf => "IN",
                OptionsOp::NoneOf => "NOT IN",
            };
            format!("{expression} {operator} ({})", placeholders(value.len()))
        }
        Filter::Boolean {
            operator, value, ..
        } => {
            let (operator, literal) = match flavor {
                FilterFlavor::Duckdb => (
                    match operator {
                        BooleanOp::Eq => "=",
                        BooleanOp::Ne => "<>",
                    },
                    if *value { "TRUE" } else { "FALSE" },
                ),
                FilterFlavor::Clickhouse => (
                    match operator {
                        BooleanOp::Eq => "=",
                        BooleanOp::Ne => "!=",
                    },
                    if *value { "true" } else { "false" },
                ),
            };
            format!("{expression} {operator} {literal}")
        }
        Filter::Null { operator, .. } => match operator {
            NullOp::IsNull => format!("{expression} IS NULL"),
            NullOp::IsNotNull => format!("{expression} IS NOT NULL"),
        },
    }
}
