//! Typed candidate, watermark, and arrival queries for chronological text search.
//!
//! The lowered value uses Kleene truth encoded as false=0, unknown=1, true=2.
//! `least`, `greatest`, and `2 - value` are therefore AND, OR, and NOT without
//! collapsing a truncated field's unknown state.

use sideseat_ports::types::{SearchCursor, SearchExpr, SearchField, SearchQuery, SearchSignal};

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

/// Remove the term rows of many span identities in one statement.
///
/// A span writes about 35 term rows, so a batch of a thousand spans used to issue a thousand deletes and
/// thirty-five thousand inserts; the rows are appended now (DuckDB's bulk path, which the span rows already
/// use) and the deletes are one statement per batch. Row-value `IN` so the index on the identity can serve it.
pub fn duckdb_span_term_delete(
    identities: &[(String, String, String)],
) -> Option<ParameterizedQuery> {
    if identities.is_empty() {
        return None;
    }
    let tuples = std::iter::repeat_n("(?, ?, ?)", identities.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = Vec::with_capacity(identities.len() * 3);
    for (project_id, trace_id, span_id) in identities {
        params.push(QueryValue::String(project_id.clone()));
        params.push(QueryValue::String(trace_id.clone()));
        params.push(QueryValue::String(span_id.clone()));
    }
    Some(ParameterizedQuery::new(
        format!("DELETE FROM span_terms WHERE (project_id, trace_id, span_id) IN ({tuples})"),
        params,
    ))
}

/// The same for log term rows, keyed by the log identity.
pub fn duckdb_log_term_delete(identities: &[(String, String, u32)]) -> Option<ParameterizedQuery> {
    if identities.is_empty() {
        return None;
    }
    let tuples = std::iter::repeat_n("(?, ?, ?)", identities.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = Vec::with_capacity(identities.len() * 3);
    for (project_id, log_digest, ordinal) in identities {
        params.push(QueryValue::String(project_id.clone()));
        params.push(QueryValue::String(log_digest.clone()));
        params.push(QueryValue::Int64(i64::from(*ordinal)));
    }
    Some(ParameterizedQuery::new(
        format!("DELETE FROM log_terms WHERE (project_id, log_digest, ordinal) IN ({tuples})"),
        params,
    ))
}
/// Remove the term rows the stored revisions of these span identities wrote.
///
/// `revisions_us` are those revisions' ingest instants in epoch microseconds. A revision's terms carry its
/// instant, and terms are appended in ingest order, so the condition on `ingested_at` lets DuckDB skip every row
/// group whose range holds none of them: the delete reads the row groups those revisions were written to, not
/// the whole term table. The instants are written as literals, which is what the row-group check folds.
pub fn duckdb_span_term_delete_of_revisions(
    identities: &[(String, String, String)],
    revisions_us: &[i64],
) -> Option<ParameterizedQuery> {
    if identities.is_empty() || revisions_us.is_empty() {
        return None;
    }
    let mut params = Vec::with_capacity(identities.len() * 3);
    for (project_id, trace_id, span_id) in identities {
        params.push(QueryValue::String(project_id.clone()));
        params.push(QueryValue::String(trace_id.clone()));
        params.push(QueryValue::String(span_id.clone()));
    }
    Some(ParameterizedQuery::new(
        format!(
            "DELETE FROM span_terms WHERE ingested_at IN ({}) AND (project_id, trace_id, span_id) IN ({})",
            instants(revisions_us),
            std::iter::repeat_n("(?, ?, ?)", identities.len())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        params,
    ))
}

/// The same for log term rows, keyed by the log identity.
pub fn duckdb_log_term_delete_of_revisions(
    identities: &[(String, String, u32)],
    revisions_us: &[i64],
) -> Option<ParameterizedQuery> {
    if identities.is_empty() || revisions_us.is_empty() {
        return None;
    }
    let mut params = Vec::with_capacity(identities.len() * 3);
    for (project_id, log_digest, ordinal) in identities {
        params.push(QueryValue::String(project_id.clone()));
        params.push(QueryValue::String(log_digest.clone()));
        params.push(QueryValue::Int64(i64::from(*ordinal)));
    }
    Some(ParameterizedQuery::new(
        format!(
            "DELETE FROM log_terms WHERE ingested_at IN ({}) AND (project_id, log_digest, ordinal) IN ({})",
            instants(revisions_us),
            std::iter::repeat_n("(?, ?, ?)", identities.len())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        params,
    ))
}

/// Distinct epoch-microsecond instants as DuckDB timestamp literals.
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

pub const DUCKDB_SPAN_TERMS_DELETE_TRACE_SQL: &str =
    "DELETE FROM span_terms WHERE project_id = ? AND trace_id = ?";
pub const DUCKDB_LOG_TERMS_DELETE_TRACE_SQL: &str = "DELETE FROM log_terms WHERE project_id = ? AND EXISTS (\
     SELECT 1 FROM otel_logs l WHERE l.project_id = log_terms.project_id \
     AND l.log_digest = log_terms.log_digest AND l.ordinal = log_terms.ordinal \
     AND l.trace_id = ?)";
pub const DUCKDB_LOG_TERMS_DELETE_SPAN_SQL: &str = "DELETE FROM log_terms WHERE project_id = ? AND EXISTS (\
     SELECT 1 FROM otel_logs l WHERE l.project_id = log_terms.project_id \
     AND l.log_digest = log_terms.log_digest AND l.ordinal = log_terms.ordinal \
     AND l.trace_id = ? AND l.span_id = ?)";
pub const DUCKDB_SPAN_TERMS_DELETE_PROJECT_SQL: &str =
    "DELETE FROM span_terms WHERE project_id = ?";
pub const DUCKDB_LOG_TERMS_DELETE_PROJECT_SQL: &str = "DELETE FROM log_terms WHERE project_id = ?";
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

/// Set a backfilled DuckDB record's `search_fields` and `search_truncated`, by row id: binds the two, then the
/// row id.
pub fn duckdb_search_bits_update(signal: SearchSignal) -> &'static str {
    match signal {
        SearchSignal::Spans => {
            "UPDATE otel_spans SET search_fields = ?, search_truncated = ? WHERE rowid = ?"
        }
        SearchSignal::Logs => {
            "UPDATE otel_logs SET search_fields = ?, search_truncated = ? WHERE rowid = ?"
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
    let mut hit_params = Vec::new();
    let (hits, from) = shape.term_matches(request, &mut hit_params);
    let (hits, predicate, params) = scope_hits(
        request,
        hits,
        &predicates,
        predicate_params,
        hit_params,
        lowered.params,
    );
    let fetch = request.max_examined.saturating_add(1);
    let state = match backend {
        Backend::Duckdb => format!("CAST(({}) AS SMALLINT)", lowered.sql),
        Backend::Clickhouse => format!("toInt16(({}))", lowered.sql),
    };
    let scored = format!(
        "scored AS (\
         SELECT {projection}, {timestamp} AS timestamp_us, {state} AS match_state, \
         {indexed} AS search_indexed FROM {from} {predicate})",
        projection = shape.scored_projection(),
        timestamp = shape.timestamp_micros("r"),
        indexed = shape.indexed_expression(),
    );
    let sql = match (request.signal, backend) {
        // Scored by row, the page picked, and only the page's rows read whole, by row id: carrying the text
        // through the scoring and the ordering, or joining it back by identity, scanned every winner's text.
        (SearchSignal::Spans, Backend::Duckdb) => format!(
            "WITH winners AS ({winners}){hits}, {scored}, \
             picked AS (SELECT * FROM scored WHERE match_state != 0 ORDER BY {order} LIMIT {fetch}) \
             SELECT {text}, p.timestamp_us, p.match_state, p.search_indexed \
             FROM otel_spans r JOIN picked p ON r.rowid = p.row_id \
             ORDER BY {order}",
            winners = shape.winners(),
            text = shape.candidate_projection(),
            order = shape.order(),
        ),
        _ => format!(
            "WITH winners AS ({winners}){hits}, {scored} \
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
    let mut hit_params = Vec::new();
    let (hits, from) = shape.term_matches(request, &mut hit_params);
    let (hits, predicate, params) = scope_hits(
        request,
        hits,
        &predicates,
        predicate_params,
        hit_params,
        lowered.params,
    );
    let count = match backend {
        Backend::Duckdb => "COUNT(*) > 0",
        Backend::Clickhouse => "count() > 0",
    };
    ParameterizedQuery::new(
        format!(
            "WITH winners AS ({winners}){hits}, scored AS (\
             SELECT {identity}, {timestamp} AS timestamp_us, ({state}) AS match_state, \
             {ingested} AS ingested_at_us FROM {from} {predicate}) \
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

/// DuckDB answers a term's presence from the term table and the field's state from the record's own row: which
/// fields it indexed and which it truncated (`search_fields`, `search_truncated`). Asking the term table for those
/// too joined every record against every term row of the field - at a million spans, more than the memory limit.
fn duckdb_field_leaf(shape: Shape, field: SearchField, terms: &[String], phrase: bool) -> Lowered {
    let present = format!(
        "list_has_all(coalesce(h.pairs, []::VARCHAR[]), [{}])",
        std::iter::repeat_n("?", terms.len())
            .collect::<Vec<_>>()
            .join(", ")
    );
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
            .map(|term| QueryValue::String(term_pair(field, term)))
            .collect(),
    }
}

/// A term as DuckDB's term matches name it: `field:term`. Terms are runs of alphanumerics (`tokenize`), so the
/// separator cannot occur in one.
fn term_pair(field: SearchField, term: &str) -> String {
    format!("{}:{term}", field.as_str())
}

/// Every term the expression's leaves name, in first-use order, each once.
fn expression_terms<'a>(expression: &'a SearchExpr, terms: &mut Vec<&'a str>) {
    match expression {
        SearchExpr::MatchAll => {}
        SearchExpr::Term { term, .. } => {
            if !terms.contains(&term.as_str()) {
                terms.push(term);
            }
        }
        SearchExpr::Phrase { terms: phrase, .. } => {
            for term in phrase {
                if !terms.contains(&term.as_str()) {
                    terms.push(term);
                }
            }
        }
        SearchExpr::And(left, right) | SearchExpr::Or(left, right) => {
            expression_terms(left, terms);
            expression_terms(right, terms);
        }
        SearchExpr::Not(inner) => expression_terms(inner, terms),
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

/// Where a search's own predicates go, with its values in the order the statement binds them.
///
/// With term matches, the predicates define the `candidates` the matches are read for, ahead of them; the scored
/// relation then reads the candidates and needs no predicate of its own. Without, they stay on the scored relation.
/// Returns the common tables after `winners`, the scored relation's `WHERE` (or nothing), and every value.
fn scope_hits(
    request: &SearchQuery,
    hits: String,
    predicates: &[String],
    predicate_params: Vec<QueryValue>,
    hit_params: Vec<QueryValue>,
    expression_params: Vec<QueryValue>,
) -> (String, String, Vec<QueryValue>) {
    let condition = if predicates.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", predicates.join(" AND "))
    };
    let mut params = vec![QueryValue::String(request.project_id.to_string())];
    if hits.is_empty() {
        params.extend(expression_params);
        params.extend(predicate_params);
        return (hits, condition, params);
    }
    params.extend(predicate_params);
    params.extend(hit_params);
    params.extend(expression_params);
    (
        format!(", candidates AS (SELECT * FROM winners r {condition}){hits}"),
        String::new(),
        params,
    )
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
            (SearchSignal::Logs, Backend::Duckdb) => "SELECT * FROM otel_logs WHERE project_id = ?",
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

    /// The columns a candidate is scored and ordered with. DuckDB's span candidates carry only their identity
    /// and are read whole once chosen (`candidates`), since carrying their text through the scoring held every
    /// candidate's messages and, at a million spans, exceeded the memory limit.
    fn scored_projection(self) -> &'static str {
        match (self.signal, self.backend) {
            (SearchSignal::Spans, Backend::Duckdb) => "r.row_id, r.trace_id, r.span_id",
            _ => self.candidate_projection(),
        }
    }

    fn candidate_projection(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => {
                "r.trace_id, r.span_id, r.span_name, r.input_preview, r.output_preview, \
                 r.messages, r.tool_definitions, r.tool_names, r.gen_ai_tool_name, \
                 r.status_message, r.exception_type, r.exception_message, r.exception_stacktrace"
            }
            SearchSignal::Logs => {
                "r.log_digest, r.ordinal, r.severity_text, r.body_text, r.trace_id, r.span_id, \
                 r.body, r.event_name, r.attributes"
            }
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

    /// The relation a search scores, and the common table it needs before it. ClickHouse reads its token columns
    /// from the record. DuckDB joins the records to their term matches: one aggregate over the term rows that hold
    /// any of the expression's terms, each matching record with the `field:term` pairs it holds. Asked per
    /// record and field instead, as correlated `EXISTS`, every subquery carried every candidate's identity - at a
    /// million spans, more than the memory limit.
    ///
    /// The aggregate reads only the term rows of the `candidates` the caller defines - the winners its time range
    /// and cursor admit - so its memory follows the candidates rather than the project: over every record holding
    /// a term, a range of ten spans grouped a million and ran out of the limit at three million.
    fn term_matches(self, request: &SearchQuery, params: &mut Vec<QueryValue>) -> (String, String) {
        let mut terms = Vec::new();
        expression_terms(&request.expression, &mut terms);
        if self.backend == Backend::Clickhouse || terms.is_empty() {
            return (String::new(), "winners r".to_string());
        }
        let (key, candidate, join) = match self.signal {
            SearchSignal::Spans => (
                "t.trace_id AS trace_id, t.span_id AS span_id",
                "(t.trace_id, t.span_id) IN (SELECT trace_id, span_id FROM candidates)",
                "h.trace_id = r.trace_id AND h.span_id = r.span_id",
            ),
            SearchSignal::Logs => (
                "t.log_digest AS log_digest, t.ordinal AS ordinal",
                "(t.log_digest, t.ordinal) IN (SELECT log_digest, ordinal FROM candidates)",
                "h.log_digest = r.log_digest AND h.ordinal = r.ordinal",
            ),
        };
        params.push(QueryValue::String(request.project_id.to_string()));
        params.extend(
            terms
                .iter()
                .map(|term| QueryValue::String((*term).to_string())),
        );
        (
            format!(
                ", hits AS (SELECT {key}, list(DISTINCT t.field || ':' || t.term) AS pairs \
                 FROM {table} t WHERE t.project_id = ? AND t.term IN ({placeholders}) AND {candidate} \
                 GROUP BY ALL)",
                table = self.term_table(),
                placeholders = std::iter::repeat_n("?", terms.len())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            format!("candidates r LEFT JOIN hits h ON {join}"),
        )
    }

    fn term_table(self) -> &'static str {
        match self.signal {
            SearchSignal::Spans => "span_terms",
            SearchSignal::Logs => "log_terms",
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
                 r.attributes"
            }
        }
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
