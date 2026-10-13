//! Typed candidate, watermark, and arrival queries for chronological text search.
//!
//! The lowered value uses Kleene truth encoded as false=0, unknown=1, true=2.
//! `least`, `greatest`, and `2 - value` are therefore AND, OR, and NOT without
//! collapsing a truncated field's unknown state.

use sideseat_ports::types::{SearchCursor, SearchExpr, SearchField, SearchQuery, SearchSignal};

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

mod sources;
pub use sources::sources;

/// Distinct epoch-microsecond instants as DuckDB timestamp literals, which a row group's zone map folds.
fn instants(revisions_us: &[i64]) -> String {
    let mut revisions = revisions_us.to_vec();
    revisions.sort_unstable();
    revisions.dedup();
    revisions
        .iter()
        .map(|us| format!("make_timestamp({us})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A field's exact terms as a DuckDB record holds them on its row: each between single spaces, in the document's
/// order, so a term is present exactly where ` term ` is - a term is a run of alphanumerics (`tokenize`) and never
/// holds a space. `None` for a field with no terms; whether the record indexed the field at all is
/// `search_fields`'s.
pub fn duckdb_term_column(terms: &[String]) -> Option<String> {
    (!terms.is_empty()).then(|| {
        let mut column =
            String::with_capacity(terms.iter().map(|term| term.len() + 1).sum::<usize>() + 1);
        column.push(' ');
        for term in terms {
            column.push_str(term);
            column.push(' ');
        }
        column
    })
}

/// A document's term columns, one per field its signal searches, in the table's order: a span's `search_prompt`,
/// `search_completion`, `search_tool_name`, `search_tool_args`, `search_error` and `search_span_name`, a log's
/// `search_body`, `search_event_name`, `search_severity` and `search_attributes`.
pub fn duckdb_term_columns(
    signal: SearchSignal,
    document: &sideseat_ports::types::SearchDocument,
) -> Vec<Option<String>> {
    Shape::new(signal, Backend::Duckdb)
        .fields()
        .iter()
        .map(|field| {
            document
                .field(*field)
                .and_then(|entry| duckdb_term_column(&entry.terms))
        })
        .collect()
}
/// Whether a span's winner is still the revision a backfill read, by its content digest, and still not indexed
/// in every field: project, trace, span, digest.
pub fn duckdb_span_backfill_cas_sql() -> String {
    format!(
        "SELECT EXISTS (SELECT 1 FROM otel_spans \
         WHERE project_id = ? AND trace_id = ? AND span_id = ? AND superseded_at IS NULL \
         AND content_digest = ? AND search_fields != {})",
        duckdb_every_field(SearchSignal::Spans)
    )
}

/// Set a backfilled DuckDB record's search state, by row id: binds its term columns (`duckdb_term_columns`), then
/// `search_fields` and `search_truncated`, then the row id. A record's terms are on its row, so the backfill
/// writes them with the bits that say which fields it indexed.
pub fn duckdb_search_bits_update(signal: SearchSignal) -> &'static str {
    match signal {
        SearchSignal::Spans => {
            "UPDATE otel_spans SET search_prompt = ?, search_completion = ?, search_tool_name = ?, \
             search_tool_args = ?, search_error = ?, search_span_name = ?, search_fields = ?, \
             search_truncated = ? WHERE rowid = ?"
        }
        SearchSignal::Logs => {
            "UPDATE otel_logs SET search_body = ?, search_event_name = ?, search_severity = ?, \
             search_attributes = ?, search_fields = ?, search_truncated = ? WHERE rowid = ?"
        }
    }
}

/// The bit `field` holds in a DuckDB record's `search_fields` and `search_truncated`: its place among the fields
/// its signal searches. A field the signal does not search has none.
pub fn duckdb_field_bit(signal: SearchSignal, field: SearchField) -> u8 {
    Shape::new(signal, Backend::Duckdb)
        .fields()
        .iter()
        .position(|searched| *searched == field)
        .map_or(0, |position| 1 << position)
}

/// `search_fields` with every field of `signal` indexed.
pub fn duckdb_every_field(signal: SearchSignal) -> u8 {
    let fields = Shape::new(signal, Backend::Duckdb).fields().len();
    u8::try_from((1_u16 << fields) - 1).expect("a signal searches at most eight fields")
}

/// A document's `search_fields` and `search_truncated`: the fields it indexed - every field it carries, empty
/// ones included, since an empty field is indexed and holds nothing - and those whose terms it cut short. A
/// document that was not indexed has neither, which is what sends its record to the backfill.
pub fn duckdb_document_bits(
    signal: SearchSignal,
    document: &sideseat_ports::types::SearchDocument,
) -> (u8, u8) {
    if !document.indexed {
        return (0, 0);
    }
    document
        .fields
        .iter()
        .fold((0, 0), |(fields, truncated), field| {
            let bit = duckdb_field_bit(signal, field.field);
            (
                fields | bit,
                if field.truncated {
                    truncated | bit
                } else {
                    truncated
                },
            )
        })
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchCandidatePlan {
    pub query: ParameterizedQuery,
}

pub fn candidates(request: &SearchQuery, backend: Backend) -> SearchCandidatePlan {
    let shape = Shape::new(request.signal, backend);
    let lowered = lower(&request.expression, shape);
    let mut predicates = Vec::new();
    let mut predicate_params = Vec::new();
    push_time_predicates(request, shape, &mut predicates, &mut predicate_params);
    push_after_cursor(
        request.cursor.as_ref(),
        shape,
        &mut predicates,
        &mut predicate_params,
    );
    let (predicate, params) = scoped(request, &predicates, predicate_params, lowered.params);
    let fetch = request.max_examined.saturating_add(1);
    let state = match backend {
        Backend::Duckdb => format!("CAST(({}) AS SMALLINT)", lowered.sql),
        Backend::Clickhouse => format!("toInt16(({}))", lowered.sql),
    };
    let scored = format!(
        "scored AS (\
         SELECT {projection}, {timestamp} AS timestamp_us, {state} AS match_state, \
         {indexed} AS search_indexed FROM winners r {predicate})",
        projection = shape.scored_projection(),
        timestamp = shape.timestamp_micros("r"),
        indexed = shape.indexed_expression(),
    );
    // Only what orders, keys and sizes a candidate: its text is read after, a bounded chunk at a time
    // (`sources`). Carrying the page's text through the final ordering held all of it at once, which at a
    // million spans passed the memory limit.
    let sql = match (request.signal, backend) {
        // Scored by row, the page picked, and the picked rows' light columns and text lengths read by row id.
        (signal, Backend::Duckdb) => format!(
            "WITH winners AS ({winners}), {scored}, \
             picked AS (SELECT * FROM scored WHERE match_state != 0 ORDER BY {order} LIMIT {fetch}) \
             SELECT p.row_id, {light}, {revision} AS revision_us, {digest} AS content_digest, \
             {bytes} AS source_bytes, p.timestamp_us, p.match_state, p.search_indexed \
             FROM {table} r JOIN picked p ON r.rowid = p.row_id \
             ORDER BY {order}",
            winners = shape.winners(),
            light = shape.light_projection(),
            revision = shape.ingested_micros("r"),
            digest = shape.content_digest("r"),
            bytes = shape.source_bytes("r"),
            order = shape.order(),
            table = match signal {
                SearchSignal::Spans => "otel_spans",
                SearchSignal::Logs => "otel_logs",
            },
        ),
        _ => format!(
            "WITH winners AS ({winners}), {scored} \
             SELECT * FROM scored WHERE match_state != 0 ORDER BY {order} LIMIT {fetch}",
            winners = shape.winners(),
            order = shape.order(),
        ),
    };
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
    };
    ParameterizedQuery::new(
        format!(
            "WITH winners AS ({}) SELECT {aggregate} FROM winners r {predicate}",
            shape.winners()
        ),
        params,
    )
}

/// Whether every current record in the requested time range carries the
/// backend's complete search-index marker. This deliberately ignores the
/// traversal cursor so every page reports the same range-level fact.
pub fn indexing_complete(request: &SearchQuery, backend: Backend) -> ParameterizedQuery {
    let shape = Shape::new(request.signal, backend);
    let mut params = vec![QueryValue::String(request.project_id.to_string())];
    let mut predicates = Vec::new();
    push_time_predicates(request, shape, &mut predicates, &mut params);
    predicates.push(shape.unindexed_predicate());
    ParameterizedQuery::new(
        format!(
            "WITH winners AS ({}) SELECT COUNT(*) = 0 FROM winners r WHERE {}",
            shape.winners(),
            predicates.join(" AND ")
        ),
        params,
    )
}

/// Marker-checkpointed source page for the historical index backfill.
pub fn backfill_sources(
    project_id: &str,
    signal: SearchSignal,
    limit: usize,
    backend: Backend,
) -> ParameterizedQuery {
    let shape = Shape::new(signal, backend);
    ParameterizedQuery::new(
        format!(
            "WITH winners AS ({winners}) SELECT {projection} FROM winners r \
             WHERE {unindexed} ORDER BY {identity} LIMIT ?",
            winners = shape.winners(),
            projection = shape.backfill_source_projection(),
            unindexed = shape.unindexed_predicate(),
            identity = shape.identity_columns(),
        ),
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::Int64(i64::try_from(limit).unwrap_or(i64::MAX)),
        ],
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
    let mut predicates = Vec::new();
    let mut predicate_params = Vec::new();
    push_time_predicates(request, shape, &mut predicates, &mut predicate_params);
    predicates.push(format!("{} > ?", shape.ingested_micros("r")));
    predicate_params.push(QueryValue::Int64(through.started_at_us));
    push_visited_region(through, shape, &mut predicates, &mut predicate_params);
    let (predicate, params) = scoped(request, &predicates, predicate_params, lowered.params);
    let count = match backend {
        Backend::Duckdb => "COUNT(*) > 0",
        Backend::Clickhouse => "count() > 0",
    };
    ParameterizedQuery::new(
        format!(
            "WITH winners AS ({winners}), scored AS (\
             SELECT {identity}, {timestamp} AS timestamp_us, ({state}) AS match_state, \
             {ingested} AS ingested_at_us FROM winners r {predicate}) \
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
    }
}

/// A DuckDB field leaf: a term's presence from the terms the record's row holds (`duckdb_term_column`), each
/// present where ` term ` is, and the field's state from the same row - which fields it indexed and which it
/// truncated (`search_fields`, `search_truncated`).
fn duckdb_field_leaf(shape: Shape, field: SearchField, terms: &[String], phrase: bool) -> Lowered {
    let column = format!("coalesce(r.search_{}, '')", field.as_str());
    let present = if terms.is_empty() {
        "true".to_string()
    } else {
        std::iter::repeat_n(format!("contains({column}, ?)"), terms.len())
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let bit = duckdb_field_bit(shape.signal, field);
    let truncated = format!("(r.search_truncated & {bit}) != 0");
    let field_indexed = format!("(r.search_fields & {bit}) != 0");
    let positive = if phrase { 1 } else { 2 };
    Lowered {
        sql: format!(
            "CASE WHEN ({present}) THEN {positive} WHEN {truncated} THEN 1 \
             WHEN NOT ({field_indexed}) THEN 1 ELSE 0 END"
        ),
        params: terms
            .iter()
            .map(|term| QueryValue::String(format!(" {term} ")))
            .collect(),
    }
}

fn clickhouse_field_leaf(field: SearchField, terms: &[String], phrase: bool) -> Lowered {
    let column = format!("r.search_{}", field.as_str());
    let truncated = format!("{column}_truncated");
    let positive = if phrase { 1 } else { 2 };
    Lowered {
        sql: format!(
            "CASE WHEN r.search_indexed = 0 THEN 1 \
             WHEN hasAllTokens({column}, ?) THEN {positive} \
             WHEN {truncated} != 0 THEN 1 ELSE 0 END"
        ),
        params: vec![QueryValue::String(terms.join(" "))],
    }
}

/// A search's `WHERE` over its winners, or nothing, and every value in the order the statement binds them: the
/// project the winners are read for, the expression's terms (which the scored projection binds), then the
/// predicates'.
fn scoped(
    request: &SearchQuery,
    predicates: &[String],
    predicate_params: Vec<QueryValue>,
    expression_params: Vec<QueryValue>,
) -> (String, Vec<QueryValue>) {
    let condition = if predicates.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", predicates.join(" AND "))
    };
    let mut params = vec![QueryValue::String(request.project_id.to_string())];
    params.extend(expression_params);
    params.extend(predicate_params);
    (condition, params)
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
                "SELECT *, rowid AS row_id FROM otel_spans WHERE project_id = ? AND superseded_at IS NULL"
            }
            (SearchSignal::Logs, Backend::Duckdb) => {
                "SELECT *, rowid AS row_id FROM otel_logs WHERE project_id = ?"
            }
            (SearchSignal::Spans, Backend::Clickhouse) => {
                "SELECT * FROM otel_spans FINAL WHERE project_id = ?"
            }
            (SearchSignal::Logs, Backend::Clickhouse) => {
                "SELECT * FROM otel_logs FINAL WHERE project_id = ?"
            }
        }
    }

    fn identity_projection(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => "r.trace_id, r.span_id",
            SearchSignal::Logs => "r.log_digest, r.ordinal",
        }
    }

    /// The columns a candidate is scored and ordered with: never its text, which carried through the scoring held
    /// every candidate's messages - and a log's raw record, which its body views are rebuilt from, is the largest
    /// column it has. DuckDB reads the picked rows' light columns by row id afterwards; ClickHouse carries them,
    /// with the revision and the text's length, which it computes as it scans.
    fn scored_projection(self) -> String {
        match (self.signal, self.backend) {
            (SearchSignal::Spans, Backend::Duckdb) => "r.row_id, r.trace_id, r.span_id".to_string(),
            (SearchSignal::Logs, Backend::Duckdb) => {
                "r.row_id, r.log_digest, r.ordinal".to_string()
            }
            (_, Backend::Clickhouse) => format!(
                "{}, {} AS revision_us, {} AS content_digest, {} AS source_bytes",
                self.light_projection(),
                self.ingested_micros("r"),
                self.content_digest("r"),
                self.source_bytes("r")
            ),
        }
    }

    /// What a candidate carries besides its text: its identity and the previews a hit shows, which ingestion cuts
    /// to a bounded length. A span's name and a log's severity text have no such bound, so they come with the text.
    fn light_projection(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => "r.trace_id, r.span_id, r.input_preview, r.output_preview",
            SearchSignal::Logs => "r.log_digest, r.ordinal, r.trace_id, r.span_id",
        }
    }

    /// The text a candidate is verified against, read by `sources`.
    fn source_projection(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => {
                "r.messages, r.tool_definitions, r.tool_names, r.input_preview, r.output_preview, \
                 r.gen_ai_tool_name, r.status_message, r.exception_type, r.exception_message, \
                 r.exception_stacktrace, r.span_name"
            }
            SearchSignal::Logs => {
                "r.body_text, r.body, r.event_name, r.severity_text, r.attributes, r.raw_log, \
                 r.noted_references"
            }
        }
    }

    /// What the row `alias`'s source text weighs, for cutting its reads into chunks: the row's logical size,
    /// the length of its JSON form, which every text column is part of - stored with the row, so the page reads no
    /// text to know it. Reading the text's own lengths read every candidate's text in one vector, which is what the
    /// chunked reads exist to avoid.
    fn source_bytes(self, alias: &str) -> String {
        format!("{alias}.logical_bytes")
    }

    /// The content digest that, with its write instant, tells one revision of the row `alias` from another; a log's
    /// identity is its content, so it has none.
    fn content_digest(self, alias: &str) -> String {
        match self.signal {
            SearchSignal::Spans => format!("{alias}.content_digest"),
            SearchSignal::Logs => "''".to_string(),
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
        }
    }

    fn ingested_micros(self, alias: &str) -> String {
        match self.backend {
            Backend::Duckdb => format!("EPOCH_US({alias}.ingested_at)"),
            Backend::Clickhouse => {
                format!("toInt64(toUnixTimestamp64Micro({alias}.ingested_at))")
            }
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

    fn unindexed_predicate(self) -> String {
        match self.backend {
            Backend::Clickhouse => "r.search_indexed = 0".to_string(),
            Backend::Duckdb => format!("r.search_fields != {}", duckdb_every_field(self.signal)),
        }
    }

    fn indexed_expression(self) -> String {
        match self.backend {
            Backend::Clickhouse => "r.search_indexed".to_string(),
            Backend::Duckdb => format!(
                "CASE WHEN r.search_fields = {} THEN 1 ELSE 0 END",
                duckdb_every_field(self.signal)
            ),
        }
    }

    fn backfill_source_projection(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => {
                "r.trace_id, r.span_id, r.content_digest, r.messages, r.tool_definitions, \
                 r.tool_names, r.input_preview, r.output_preview, r.gen_ai_tool_name, \
                 r.status_message, r.exception_type, r.exception_message, \
                 r.exception_stacktrace, r.span_name"
            }
            SearchSignal::Logs => {
                "r.log_digest, r.ordinal, r.body_text, r.body, r.event_name, r.severity_text, \
                 r.attributes, r.raw_log, r.noted_references"
            }
        }
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
