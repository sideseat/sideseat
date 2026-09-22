//! Typed candidate, watermark, and arrival queries for chronological text search.
//!
//! The lowered value uses Kleene truth encoded as false=0, unknown=1, true=2.
//! `least`, `greatest`, and `2 - value` are therefore AND, OR, and NOT without
//! collapsing a truncated field's unknown state.

use sideseat_ports::types::{SearchCursor, SearchExpr, SearchField, SearchQuery, SearchSignal};

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

#[derive(Debug, Clone, PartialEq)]
pub struct SearchCandidatePlan {
    pub query: ParameterizedQuery,
}

pub fn candidates(request: &SearchQuery, backend: Backend) -> SearchCandidatePlan {
    let shape = Shape::new(request.signal, backend);
    let lowered = lower(&request.expression, shape);
    let mut params = vec![QueryValue::String(request.project_id.to_string())];
    params.extend(lowered.params);

    let mut predicates = Vec::new();
    push_time_predicates(request, shape, &mut predicates, &mut params);
    push_after_cursor(request.cursor.as_ref(), shape, &mut predicates, &mut params);
    let predicate = if predicates.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", predicates.join(" AND "))
    };
    let fetch = request.max_examined.saturating_add(1);
    let state = match backend {
        Backend::Duckdb => format!("CAST(({}) AS SMALLINT)", lowered.sql),
        Backend::Clickhouse => format!("toInt16(({}))", lowered.sql),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    let sql = format!(
        "WITH winners AS ({winners}), scored AS (\
         SELECT {identity}, {timestamp} AS timestamp_us, {state} AS match_state \
         FROM winners r {predicate}) \
         SELECT {output_identity}, timestamp_us, match_state FROM scored \
         WHERE match_state != 0 ORDER BY {order} LIMIT {fetch}",
        winners = shape.winners(),
        identity = shape.identity_projection(),
        output_identity = shape.identity_columns(),
        timestamp = shape.timestamp_micros("r"),
        state = state,
        order = shape.order(),
    );
    SearchCandidatePlan {
        query: ParameterizedQuery::new(sql, params),
    }
}

/// Store-derived first-page watermark. It deliberately does not use the API
/// process clock: writers may run on other hosts.
pub fn watermark(request: &SearchQuery, backend: Backend) -> ParameterizedQuery {
    let shape = Shape::new(request.signal, backend);
    let mut params = vec![QueryValue::String(request.project_id.to_string())];
    let mut predicates = Vec::new();
    push_time_predicates(request, shape, &mut predicates, &mut params);
    let predicate = if predicates.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", predicates.join(" AND "))
    };
    let aggregate = match backend {
        Backend::Duckdb => "COALESCE(MAX(EPOCH_US(r.ingested_at)), 0)",
        Backend::Clickhouse => {
            "ifNull(max(toInt64(toUnixTimestamp64Micro(r.ingested_at))), toInt64(0))"
        }
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    ParameterizedQuery::new(
        format!(
            "WITH winners AS ({}) SELECT {aggregate} FROM winners r {predicate}",
            shape.winners()
        ),
        params,
    )
}

/// Detect a current candidate that arrived after the traversal watermark in
/// the portion of the descending order already visited.
pub fn arrivals(
    request: &SearchQuery,
    through: &SearchCursor,
    backend: Backend,
) -> ParameterizedQuery {
    let shape = Shape::new(request.signal, backend);
    let lowered = lower(&request.expression, shape);
    let mut params = vec![QueryValue::String(request.project_id.to_string())];
    params.extend(lowered.params);
    let mut predicates = Vec::new();
    push_time_predicates(request, shape, &mut predicates, &mut params);
    predicates.push(format!("{} > ?", shape.ingested_micros("r")));
    params.push(QueryValue::Int64(through.started_at_us));
    push_visited_region(through, shape, &mut predicates, &mut params);
    let predicate = predicates.join(" AND ");
    let count = match backend {
        Backend::Duckdb => "COUNT(*) > 0",
        Backend::Clickhouse => "count() > 0",
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    ParameterizedQuery::new(
        format!(
            "WITH winners AS ({winners}), scored AS (\
             SELECT {identity}, {timestamp} AS timestamp_us, ({state}) AS match_state, \
             {ingested} AS ingested_at_us FROM winners r WHERE {predicate}) \
             SELECT {count} FROM scored WHERE match_state != 0",
            winners = shape.winners(),
            identity = shape.identity_projection(),
            timestamp = shape.timestamp_micros("r"),
            state = lowered.sql,
            ingested = shape.ingested_micros("r"),
        ),
        params,
    )
}

#[derive(Debug)]
struct Lowered {
    sql: String,
    params: Vec<QueryValue>,
}

fn lower(expression: &SearchExpr, shape: Shape) -> Lowered {
    match expression {
        SearchExpr::MatchAll => Lowered {
            sql: "2".to_string(),
            params: Vec::new(),
        },
        SearchExpr::Term { field, term } => leaf(shape, *field, std::slice::from_ref(term), false),
        SearchExpr::Phrase { field, terms, .. } => leaf(shape, *field, terms, true),
        SearchExpr::And(left, right) => binary("least", lower(left, shape), lower(right, shape)),
        SearchExpr::Or(left, right) => binary("greatest", lower(left, shape), lower(right, shape)),
        SearchExpr::Not(inner) => {
            let inner = lower(inner, shape);
            Lowered {
                sql: format!("(2 - ({}))", inner.sql),
                params: inner.params,
            }
        }
    }
}

fn binary(function: &str, left: Lowered, right: Lowered) -> Lowered {
    let mut params = left.params;
    params.extend(right.params);
    Lowered {
        sql: format!("{function}(({}), ({}))", left.sql, right.sql),
        params,
    }
}

fn leaf(shape: Shape, restricted: Option<SearchField>, terms: &[String], phrase: bool) -> Lowered {
    let fields = shape
        .fields()
        .iter()
        .copied()
        .filter(|field| restricted.is_none_or(|wanted| wanted == *field))
        .map(|field| field_leaf(shape, field, terms, phrase))
        .collect::<Vec<_>>();
    let mut params = Vec::new();
    let sql = fields
        .iter()
        .map(|field| {
            params.extend(field.params.clone());
            format!("({})", field.sql)
        })
        .collect::<Vec<_>>();
    Lowered {
        sql: match sql.as_slice() {
            [] => "0".to_string(),
            [only] => only.clone(),
            _ => format!("greatest({})", sql.join(", ")),
        },
        params,
    }
}

fn field_leaf(shape: Shape, field: SearchField, terms: &[String], phrase: bool) -> Lowered {
    match shape.backend {
        Backend::Duckdb => duckdb_field_leaf(shape, field, terms, phrase),
        Backend::Clickhouse => clickhouse_field_leaf(field, terms, phrase),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn duckdb_field_leaf(shape: Shape, field: SearchField, terms: &[String], phrase: bool) -> Lowered {
    let key = shape.term_key_predicate();
    let present = terms
        .iter()
        .map(|_| {
            format!(
                "EXISTS (SELECT 1 FROM {table} t WHERE {key} AND t.field = '{field}' AND t.term = ?)",
                table = shape.term_table(),
                field = field.as_str(),
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let truncated = format!(
        "EXISTS (SELECT 1 FROM {table} t WHERE {key} AND t.field = '{field}' AND t.truncated)",
        table = shape.term_table(),
        field = field.as_str(),
    );
    let positive = if phrase { 1 } else { 2 };
    Lowered {
        sql: format!("CASE WHEN ({present}) THEN {positive} WHEN {truncated} THEN 1 ELSE 0 END"),
        params: terms.iter().cloned().map(QueryValue::String).collect(),
    }
}

fn clickhouse_field_leaf(field: SearchField, terms: &[String], phrase: bool) -> Lowered {
    let column = format!("r.search_{}", field.as_str());
    let truncated = format!("{column}_truncated");
    let positive = if phrase { 1 } else { 2 };
    Lowered {
        sql: format!(
            "CASE WHEN hasAllTokens({column}, ?) THEN {positive} \
             WHEN {truncated} != 0 THEN 1 ELSE 0 END"
        ),
        params: vec![QueryValue::String(terms.join(" "))],
    }
}

fn push_time_predicates(
    request: &SearchQuery,
    shape: Shape,
    predicates: &mut Vec<String>,
    params: &mut Vec<QueryValue>,
) {
    if let Some(from) = request.from_timestamp {
        predicates.push(format!("{} >= ?", shape.timestamp_micros("r")));
        params.push(QueryValue::Int64(from.timestamp_micros()));
    }
    if let Some(to) = request.to_timestamp {
        predicates.push(format!("{} <= ?", shape.timestamp_micros("r")));
        params.push(QueryValue::Int64(to.timestamp_micros()));
    }
}

fn push_after_cursor(
    cursor: Option<&SearchCursor>,
    shape: Shape,
    predicates: &mut Vec<String>,
    params: &mut Vec<QueryValue>,
) {
    let Some(cursor) = cursor else {
        return;
    };
    let timestamp = shape.timestamp_micros("r");
    match shape.signal {
        SearchSignal::Spans => {
            let (trace_id, span_id) = split_span_tie(&cursor.tie_breaker);
            predicates.push(format!(
                "({timestamp} < ? OR ({timestamp} = ? AND \
                 (r.trace_id > ? OR (r.trace_id = ? AND r.span_id > ?))))"
            ));
            params.extend([
                QueryValue::Int64(cursor.timestamp_us),
                QueryValue::Int64(cursor.timestamp_us),
                QueryValue::String(trace_id.to_string()),
                QueryValue::String(trace_id.to_string()),
                QueryValue::String(span_id.to_string()),
            ]);
        }
        SearchSignal::Logs => {
            predicates.push(format!(
                "({timestamp} < ? OR ({timestamp} = ? AND \
                 (r.log_digest > ? OR (r.log_digest = ? AND r.ordinal > ?))))"
            ));
            params.extend([
                QueryValue::Int64(cursor.timestamp_us),
                QueryValue::Int64(cursor.timestamp_us),
                QueryValue::String(cursor.tie_breaker.clone()),
                QueryValue::String(cursor.tie_breaker.clone()),
                QueryValue::Int64(i64::from(cursor.ordinal)),
            ]);
        }
    }
}

fn push_visited_region(
    through: &SearchCursor,
    shape: Shape,
    predicates: &mut Vec<String>,
    params: &mut Vec<QueryValue>,
) {
    let timestamp = shape.timestamp_micros("r");
    match shape.signal {
        SearchSignal::Spans => {
            let (trace_id, span_id) = split_span_tie(&through.tie_breaker);
            predicates.push(format!(
                "({timestamp} > ? OR ({timestamp} = ? AND \
                 (r.trace_id < ? OR (r.trace_id = ? AND r.span_id <= ?))))"
            ));
            params.extend([
                QueryValue::Int64(through.timestamp_us),
                QueryValue::Int64(through.timestamp_us),
                QueryValue::String(trace_id.to_string()),
                QueryValue::String(trace_id.to_string()),
                QueryValue::String(span_id.to_string()),
            ]);
        }
        SearchSignal::Logs => {
            predicates.push(format!(
                "({timestamp} > ? OR ({timestamp} = ? AND \
                 (r.log_digest < ? OR (r.log_digest = ? AND r.ordinal <= ?))))"
            ));
            params.extend([
                QueryValue::Int64(through.timestamp_us),
                QueryValue::Int64(through.timestamp_us),
                QueryValue::String(through.tie_breaker.clone()),
                QueryValue::String(through.tie_breaker.clone()),
                QueryValue::Int64(i64::from(through.ordinal)),
            ]);
        }
    }
}

fn split_span_tie(value: &str) -> (&str, &str) {
    value.split_once('\0').unwrap_or((value, ""))
}

#[derive(Debug, Clone, Copy)]
struct Shape {
    signal: SearchSignal,
    backend: Backend,
}

impl Shape {
    const fn new(signal: SearchSignal, backend: Backend) -> Self {
        Self { signal, backend }
    }

    fn winners(self) -> &'static str {
        match (self.signal, self.backend) {
            (SearchSignal::Spans, Backend::Duckdb) => {
                "SELECT * EXCLUDE (_search_rn) FROM (\
                 SELECT *, ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                 ORDER BY ingested_at DESC, rowid DESC) AS _search_rn \
                 FROM otel_spans WHERE project_id = ?) WHERE _search_rn = 1"
            }
            (SearchSignal::Logs, Backend::Duckdb) => "SELECT * FROM otel_logs WHERE project_id = ?",
            (SearchSignal::Spans, Backend::Clickhouse) => {
                "SELECT * FROM otel_spans FINAL WHERE project_id = ?"
            }
            (SearchSignal::Logs, Backend::Clickhouse) => {
                "SELECT * FROM otel_logs FINAL WHERE project_id = ?"
            }
            (_, Backend::Sqlite | Backend::Postgres) => unreachable!(),
        }
    }

    fn identity_projection(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => "r.trace_id, r.span_id",
            SearchSignal::Logs => "r.log_digest, r.ordinal",
        }
    }

    fn identity_columns(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => "trace_id, span_id",
            SearchSignal::Logs => "log_digest, ordinal",
        }
    }

    fn order(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => "timestamp_us DESC, trace_id ASC, span_id ASC",
            SearchSignal::Logs => "timestamp_us DESC, log_digest ASC, ordinal ASC",
        }
    }

    fn timestamp_column(self, alias: &str) -> String {
        match self.signal {
            SearchSignal::Spans => format!("{alias}.timestamp_start"),
            SearchSignal::Logs => format!("{alias}.timestamp"),
        }
    }

    fn timestamp_micros(self, alias: &str) -> String {
        let column = self.timestamp_column(alias);
        match self.backend {
            Backend::Duckdb => format!("EPOCH_US({column})"),
            Backend::Clickhouse => format!("toInt64(toUnixTimestamp64Micro({column}))"),
            Backend::Sqlite | Backend::Postgres => unreachable!(),
        }
    }

    fn ingested_micros(self, alias: &str) -> String {
        match self.backend {
            Backend::Duckdb => format!("EPOCH_US({alias}.ingested_at)"),
            Backend::Clickhouse => {
                format!("toInt64(toUnixTimestamp64Micro({alias}.ingested_at))")
            }
            Backend::Sqlite | Backend::Postgres => unreachable!(),
        }
    }

    fn fields(self) -> &'static [SearchField] {
        const SPANS: &[SearchField] = &[
            SearchField::Prompt,
            SearchField::Completion,
            SearchField::ToolName,
            SearchField::ToolArgs,
            SearchField::Error,
            SearchField::SpanName,
        ];
        const LOGS: &[SearchField] = &[
            SearchField::Body,
            SearchField::EventName,
            SearchField::Severity,
            SearchField::Attributes,
        ];
        match self.signal {
            SearchSignal::Spans => SPANS,
            SearchSignal::Logs => LOGS,
        }
    }

    fn term_table(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => "span_terms",
            SearchSignal::Logs => "log_terms",
        }
    }

    fn term_key_predicate(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => {
                "t.project_id = r.project_id AND t.trace_id = r.trace_id AND t.span_id = r.span_id"
            }
            SearchSignal::Logs => {
                "t.project_id = r.project_id AND t.log_digest = r.log_digest AND t.ordinal = r.ordinal"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use sideseat_ports::types::{ProjectId, SearchQuery};

    use super::*;

    fn request(expression: SearchExpr, signal: SearchSignal) -> SearchQuery {
        SearchQuery {
            project_id: ProjectId::from("p"),
            signal,
            expression,
            limit: 10,
            max_examined: 7,
            cursor: Some(SearchCursor {
                timestamp_us: 42,
                tie_breaker: "trace\0span".to_string(),
                ordinal: 0,
                started_at_us: 30,
            }),
            from_timestamp: Some(Utc.timestamp_micros(1).single().unwrap()),
            to_timestamp: Some(Utc.timestamp_micros(100).single().unwrap()),
        }
    }

    #[test]
    fn negated_phrase_stays_a_candidate_on_both_backends() {
        let expression = SearchExpr::Not(Box::new(SearchExpr::Phrase {
            field: Some(SearchField::Prompt),
            phrase: "foo bar".to_string(),
            terms: vec!["foo".to_string(), "bar".to_string()],
        }));
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let plan = candidates(&request(expression.clone(), SearchSignal::Spans), backend);
            assert!(plan.query.sql().contains("2 -"));
            assert!(plan.query.sql().contains("match_state != 0"));
            assert!(plan.query.sql().contains("LIMIT 8"));
        }
    }

    #[test]
    fn duckdb_uses_term_relations_and_clickhouse_uses_native_token_functions() {
        let expression = SearchExpr::And(
            Box::new(SearchExpr::Term {
                field: Some(SearchField::Prompt),
                term: "foo".to_string(),
            }),
            Box::new(SearchExpr::Term {
                field: Some(SearchField::Completion),
                term: "bar".to_string(),
            }),
        );
        let duckdb = candidates(
            &request(expression.clone(), SearchSignal::Spans),
            Backend::Duckdb,
        );
        assert!(duckdb.query.sql().contains("FROM span_terms"));
        assert!(duckdb.query.sql().contains("least("));
        let clickhouse = candidates(
            &request(expression, SearchSignal::Spans),
            Backend::Clickhouse,
        );
        assert!(
            clickhouse
                .query
                .sql()
                .contains("hasAllTokens(r.search_prompt")
        );
        assert!(
            clickhouse
                .query
                .sql()
                .contains("hasAllTokens(r.search_completion")
        );
    }
}
