//! The SQL for values a list row *displays*, per dialect.
//!
//! Each displayed value has one shared expression so projections, filters, and
//! detail reads cannot disagree about which span represents a trace.

/// Which aggregate syntax to render the display name in.
#[derive(Clone, Copy)]
pub enum DisplayNameDialect {
    DuckDb,
    ClickHouse,
}

/// Columns a filter-options request may ask for, shared by both analytics backends.
///
/// These were declared separately in the DuckDB and ClickHouse repositories and had drifted in
/// both directions: ClickHouse omitted `gen_ai_agent_name`, so the Agent filter dropdown was
/// empty there, while DuckDB omitted `span_name`, `session_id` and `user_id`, so those were
/// empty on DuckDB. Which filters a user sees depended on the storage backend.
///
/// Every name here is a column on `otel_spans` in both schemas. The allowlist exists to keep
/// a caller-supplied column name out of the SQL, so adding a name that is not a real column
/// turns a filter into an error rather than an injection.
/// The trace name a list row displays: the root span's name, else the earliest named span's.
///
/// The same expression in both dialects, because three places have to agree on it - the projection
/// that displays it, the filter options that offer it, and the filter that matches it. `alias`
/// qualifies the column for queries that name their table.
///
/// A `trace_name` filter has to be evaluated against *this*, per trace. Matching the raw
/// `span_name` of any span meant selecting "agent" also returned traces displayed under another
/// name that merely contained an agent span.
pub fn trace_display_name(alias: &str, dialect_first: DisplayNameDialect) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    match dialect_first {
        // `span_id` breaks the tie, for the same reason it does in `trace_display_first`: two roots
        // stamped with the same start instant left the choice to the engine, so the name the list
        // displayed, the name a filter matched and the name the detail view showed could be three
        // different spans' - and could differ between two runs of the same query.
        DisplayNameDialect::DuckDb => format!(
            "COALESCE(\
             FIRST({prefix}span_name ORDER BY {prefix}timestamp_start, {prefix}span_id) \
             FILTER (WHERE {prefix}parent_span_id IS NULL AND {prefix}span_name IS NOT NULL), \
             FIRST({prefix}span_name ORDER BY {prefix}timestamp_start, {prefix}span_id) \
             FILTER (WHERE {prefix}span_name IS NOT NULL))"
        ),
        DisplayNameDialect::ClickHouse => format!(
            "coalesce(\
             argMinIf({prefix}span_name, ({prefix}timestamp_start, {prefix}span_id), \
             {prefix}parent_span_id IS NULL AND {prefix}span_name IS NOT NULL), \
             argMinIf({prefix}span_name, ({prefix}timestamp_start, {prefix}span_id), \
             {prefix}span_name IS NOT NULL))"
        ),
    }
}

/// The single value a trace row displays for a column that only some of its spans carry: the
/// earliest span that has one.
///
/// A session id, a user id and an environment are recorded on the spans that know them - often the
/// root alone - so the row shows one value chosen this way. A filter on such a column has to be
/// evaluated against that value, not against "some span": a trace whose root names a session and
/// whose children do not was returned by `session IS NULL` while displaying the session, and
/// excluded by nothing.
///
/// The column name comes from this crate's allowlists, never from a request.
pub fn trace_display_first(column: &str, alias: &str, dialect_first: DisplayNameDialect) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    match dialect_first {
        // `span_id` breaks the tie, making this a **total** order. Ordering by the timestamp alone left
        // same-instant spans to the engine, so a filter could match a different span's value than the one
        // the row displays and than the one session membership resolves to - three answers about one trace.
        DisplayNameDialect::DuckDb => format!(
            "FIRST({prefix}{column} ORDER BY {prefix}timestamp_start, {prefix}span_id) \
             FILTER (WHERE {prefix}{column} IS NOT NULL)"
        ),
        DisplayNameDialect::ClickHouse => format!(
            "argMinIf({prefix}{column}, ({prefix}timestamp_start, {prefix}span_id), \
             {prefix}{column} IS NOT NULL)"
        ),
    }
}

/// What makes a span a GenAI span, for the "GenAI only" trace and session lists.
///
/// A span qualifies through its observation type or through any GenAI attribute. Recognising only
/// the provider and the request model missed transport-level instrumentation that records an
/// operation name, a response model, an agent or tool name, or token usage and nothing else - those
/// traces vanished from the default list.
///
/// One definition, used by both backends, because it read differently in each and the same project
/// showed a different trace list depending on which one served it. `alias` qualifies the columns for
/// queries that name their table; pass "" when they do not.
pub fn genai_span_predicate(alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    [
        format!("{prefix}observation_type != 'span'"),
        format!("{prefix}gen_ai_system IS NOT NULL"),
        format!("{prefix}gen_ai_operation_name IS NOT NULL"),
        format!("{prefix}gen_ai_request_model IS NOT NULL"),
        format!("{prefix}gen_ai_response_model IS NOT NULL"),
        format!("{prefix}gen_ai_agent_name IS NOT NULL"),
        format!("{prefix}gen_ai_tool_name IS NOT NULL"),
        format!("{prefix}gen_ai_usage_total_tokens > 0"),
        // Cache and reasoning usage on their own. Extraction accepts each independently, so a span
        // can report cache reads or reasoning tokens and nothing else.
        format!("{prefix}gen_ai_usage_cache_read_tokens > 0"),
        format!("{prefix}gen_ai_usage_cache_write_tokens > 0"),
        format!("{prefix}gen_ai_usage_reasoning_tokens > 0"),
        // Cost as well as tokens. OpenInference reports `llm.cost.*` directly, and extraction keeps
        // it, so a span can carry what a call cost without carrying the usage it was computed from.
        // Listing only tokens left such a span a plain span, hidden from every view that filters to
        // GenAI - which the commit adding this predicate claimed was fixed, and was not.
        format!("{prefix}gen_ai_cost_total > 0"),
    ]
    .join(" OR ")
}

/// Which rows a trace or session message query returns.
///
/// One definition shared by both backends: it was declared identically in the DuckDB and
/// ClickHouse repositories, so a change to one silently changed what the other returned. The
/// predicate is plain SQL that both dialects accept; if one ever needs to diverge, that
/// divergence should be visible here rather than by two constants drifting apart.
///
/// Rows with no messages, no tools and no error are never returned, which is why the
/// message-parsing harness applies the same filter when it builds its row sets.
pub const MESSAGE_CONTENT_FILTER: &str =
    "(messages != '[]' OR tool_definitions != '[]' OR tool_names != '[]' OR status_code = 'ERROR')";
