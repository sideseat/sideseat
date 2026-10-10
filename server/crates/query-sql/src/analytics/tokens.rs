//! DuckDB's token and cost totals: which spans' usage counts, by the one rule every list, detail and statistic
//! applies.
//!
//! A span counts when it carries usage - any token count above zero, or a cost - and either
//! - it is a generation, and no direct generation child of it carries usage; or
//! - it is not a generation, no generation of its trace carries usage, and its parent carries none.
//!
//! Every span the rule compares with carries usage itself, so it is evaluated over those spans alone: the ones a
//! read selects, and for each of their traces the **peers** the rule looks at. Reading the peers from the whole
//! table instead - three anti-joins over every winning span of the project, whichever traces the read was about -
//! was most of the time of the trace list, the session list and the project statistics on a million spans.

use crate::winners::DUCKDB_WINNING_SPANS;

/// The usage a counted span contributes: its column and the name the totals give it.
pub(crate) const USAGE_TOTALS: [(&str, &str); 12] = [
    ("gen_ai_usage_input_tokens", "input_tokens"),
    ("gen_ai_usage_output_tokens", "output_tokens"),
    ("gen_ai_usage_total_tokens", "total_tokens"),
    ("gen_ai_usage_cache_read_tokens", "cache_read_tokens"),
    ("gen_ai_usage_cache_write_tokens", "cache_write_tokens"),
    ("gen_ai_usage_reasoning_tokens", "reasoning_tokens"),
    ("gen_ai_cost_input", "input_cost"),
    ("gen_ai_cost_output", "output_cost"),
    ("gen_ai_cost_cache_read", "cache_read_cost"),
    ("gen_ai_cost_cache_write", "cache_write_cost"),
    ("gen_ai_cost_reasoning", "reasoning_cost"),
    ("gen_ai_cost_total", "total_cost"),
];

/// Whether the span under `alias` carries usage. Plain SQL, the same on both backends.
pub(crate) fn tokenful(alias: &str) -> String {
    format!(
        "(({alias}.gen_ai_usage_input_tokens + {alias}.gen_ai_usage_output_tokens \
         + {alias}.gen_ai_usage_total_tokens + {alias}.gen_ai_usage_cache_read_tokens \
         + {alias}.gen_ai_usage_cache_write_tokens + {alias}.gen_ai_usage_reasoning_tokens) > 0 \
         OR {alias}.gen_ai_cost_total > 0)"
    )
}

/// Whether the span under `alias` is a generation, as a non-null boolean: a span with no observation type is not
/// one, which the rule's second branch spells `observation_type IS NULL OR observation_type != 'generation'`.
fn duckdb_generation(alias: &str) -> String {
    format!("COALESCE({alias}.observation_type = 'generation', false)")
}

/// Where the rule finds a counted span's peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenPeers {
    /// The selection holds every span of each trace it touches - its condition names traces, never spans - so
    /// the peers are the selection itself.
    Selected,
    /// The selection may hold only some spans of a trace (a time window), so the peers are read again, for the
    /// traces the selection touches.
    OfSelectedTraces,
}

/// The peers relation for the spans that carry usage among `{source} g WHERE {selection}`: every winning span of
/// their traces that carries usage, with its generation flag. For [`duckdb_counted`].
pub(crate) fn duckdb_token_peers(selection: &str) -> String {
    format!(
        "SELECT t.project_id, t.trace_id, t.span_id, t.parent_span_id, {generation} AS generation \
         FROM {DUCKDB_WINNING_SPANS} t \
         WHERE (t.project_id, t.trace_id) IN (\
           SELECT g.project_id, g.trace_id FROM {DUCKDB_WINNING_SPANS} g \
           WHERE {selection} AND {tokenful_g}\
         ) \
           AND {tokenful_t}",
        generation = duckdb_generation("t"),
        tokenful_g = tokenful("g"),
        tokenful_t = tokenful("t"),
    )
}

/// The rule, for the row under `row` that carries usage and whose generation flag is `generation`, against the
/// peers in `peers` (a relation with the columns [`duckdb_token_peers`] gives).
pub(crate) fn duckdb_counted(row: &str, generation: &str, peers: &str) -> String {
    format!(
        "(({generation} AND NOT EXISTS (\
             SELECT 1 FROM {peers} c WHERE c.generation AND c.project_id = {row}.project_id \
               AND c.trace_id = {row}.trace_id AND c.parent_span_id = {row}.span_id)) \
          OR (NOT {generation} \
             AND NOT EXISTS (\
               SELECT 1 FROM {peers} gen WHERE gen.generation AND gen.project_id = {row}.project_id \
                 AND gen.trace_id = {row}.trace_id) \
             AND NOT EXISTS (\
               SELECT 1 FROM {peers} p WHERE p.project_id = {row}.project_id \
                 AND p.trace_id = {row}.trace_id AND p.span_id = {row}.parent_span_id)))"
    )
}

/// The rule for a span of a statistics read, `g` over the winning spans: it carries usage and counts, its peers
/// in the CTE named `peers`.
pub(crate) fn duckdb_counted_span(peers: &str) -> String {
    format!(
        "({tokenful} AND {counted})",
        tokenful = tokenful("g"),
        counted = duckdb_counted("g", &duckdb_generation("g"), peers),
    )
}

/// Token and cost totals per `key` over the spans `{source} g{join} WHERE {where_clause}` selects, one row per key
/// value: the twelve [`USAGE_TOTALS`], named as there.
///
/// A self-contained statement (it carries its own `WITH`), so callers embed it as a CTE body or a subquery.
pub(crate) fn duckdb_token_totals(
    key: &str,
    join: &str,
    where_clause: &str,
    peers: TokenPeers,
) -> String {
    let bare_key = key.rsplit('.').next().unwrap_or(key);
    let usage = USAGE_TOTALS
        .iter()
        .map(|(column, _)| format!("g.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sums = USAGE_TOTALS
        .iter()
        .map(|(column, name)| format!("COALESCE(SUM(r.{column}), 0) AS {name}"))
        .collect::<Vec<_>>()
        .join(",\n            ");
    let join = if join.is_empty() {
        String::new()
    } else {
        format!(" {join}")
    };
    let (peers_cte, peers_relation) = match peers {
        TokenPeers::Selected => (String::new(), "token_rows"),
        TokenPeers::OfSelectedTraces => (
            format!(
                ",\n        token_peers AS MATERIALIZED (\
                 SELECT t.project_id, t.trace_id, t.span_id, t.parent_span_id, {generation} AS generation \
                 FROM {DUCKDB_WINNING_SPANS} t \
                 WHERE (t.project_id, t.trace_id) IN (SELECT project_id, trace_id FROM token_rows) \
                   AND {tokenful_t})",
                generation = duckdb_generation("t"),
                tokenful_t = tokenful("t"),
            ),
            "token_peers",
        ),
    };
    format!(
        r#"WITH token_rows AS MATERIALIZED (
            SELECT {key} AS token_scope_key, g.project_id, g.trace_id, g.span_id, g.parent_span_id,
                   {generation} AS generation, {usage}
            FROM {DUCKDB_WINNING_SPANS} g{join}
            WHERE {where_clause}
              AND {tokenful_g}
        ){peers_cte}
        SELECT
            r.token_scope_key AS {bare_key},
            {sums}
        FROM token_rows r
        WHERE {counted}
        GROUP BY r.token_scope_key"#,
        generation = duckdb_generation("g"),
        tokenful_g = tokenful("g"),
        counted = duckdb_counted("r", "r.generation", peers_relation),
    )
}
