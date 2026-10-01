use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message as _;
use serde::{Deserialize, Serialize};
use serde_json::json;

use sideseat_api::routes::otel::messages::scope_feed_to_trace;
use sideseat_domain::pricing::PricingService;
use sideseat_domain::sideml::feed::{
    FeedOptions, extract_tools_from_rows, legacy_and_neutral_order, presented_and_unconstrained,
    process_feed, process_span, process_spans, shadow_resolved_order,
};
use sideseat_ingestion::traces::extract::ExtractionMode;
use sideseat_ports::types::{MessageSpanRow, ObservationType, ProjectId};

#[path = "../message_goldens/source_program.rs"]
mod source_program;
#[path = "../message_goldens/source_program_tests.rs"]
mod source_program_tests;

fn normalize_for_test(
    request: &ExportTraceServiceRequest,
    pricing: &PricingService,
) -> Vec<(String, MessageSpanRow)> {
    normalize_for_test_with_mode(request, pricing, ExtractionMode::PerCarrier)
}

fn normalize_for_test_with_mode(
    request: &ExportTraceServiceRequest,
    pricing: &PricingService,
    mode: ExtractionMode,
) -> Vec<(String, MessageSpanRow)> {
    let Some(spans) =
        sideseat_ingestion::traces::process_request_for_test_with_mode(request, pricing, mode)
    else {
        return Vec::new();
    };

    spans
        .into_iter()
        .map(|span| {
            let row = MessageSpanRow {
                trace_id: span.trace_id.clone(),
                span_id: span.span_id.clone(),
                parent_span_id: span.parent_span_id.clone(),
                span_timestamp: span.timestamp_start,
                span_end_timestamp: span.timestamp_end,
                messages_json: span.messages.clone().unwrap_or_else(|| "[]".to_string()),
                tool_definitions_json: span
                    .tool_definitions
                    .clone()
                    .unwrap_or_else(|| "[]".to_string()),
                tool_names_json: span.tool_names.clone().unwrap_or_else(|| "[]".to_string()),
                body_cache_key: None,
                model: span
                    .gen_ai_response_model
                    .clone()
                    .or_else(|| span.gen_ai_request_model.clone()),
                provider: span.gen_ai_system.clone(),
                status_code: span.status_code.clone(),
                exception_type: span.exception_type.clone(),
                exception_message: span.exception_message.clone(),
                exception_stacktrace: span.exception_stacktrace.clone(),
                input_tokens: span.gen_ai_usage_input_tokens,
                output_tokens: span.gen_ai_usage_output_tokens,
                total_tokens: span.gen_ai_usage_total_tokens,
                cost_total: span.gen_ai_cost_total,
                observation_type: span
                    .observation_type
                    .map(|value| value.as_str().to_string()),
                session_id: span.session_id.clone(),
                ingested_at: span.timestamp_start,
                scope_name: span.scope_name.clone(),
                scope_version: span.scope_version.clone(),
                span_name: Some(span.span_name.clone()),
                framework: span.framework.clone(),
                response_model: span.gen_ai_response_model.clone(),
                response_id: None,
                temperature: None,
                top_p: None,
                max_tokens: None,
                finish_reasons: None,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
                cost_input: 0.0,
                cost_output: 0.0,
            };
            (span.span_name, row)
        })
        .collect()
}

// ============================================================================
// Fixture discovery
// ============================================================================

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/messages")
}

/// `(label, request paths)` for every captured sample, sorted for stable test output.
fn discover_fixtures() -> Vec<(String, Vec<PathBuf>)> {
    let root = fixture_root();
    let mut out: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    collect(&root, &root, &mut out);
    out.into_iter()
        .map(|(k, mut v)| {
            v.sort();
            (k, v)
        })
        .collect()
}

fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<PathBuf>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_captured_request)
        {
            let label = path
                .parent()
                .and_then(|p| p.strip_prefix(root).ok())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            out.entry(label).or_default().push(path);
        }
    }
}

// ============================================================================
// Replay: OTLP bytes -> MessageSpanRow
// ============================================================================

fn decode_request(path: &Path) -> ExportTraceServiceRequest {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("decode JSON {}: {e}", path.display())),
        _ => ExportTraceServiceRequest::decode(bytes.as_slice())
            .unwrap_or_else(|e| panic!("decode protobuf {}: {e}", path.display())),
    }
}

/// Run the real ingestion path over every captured request, in capture order.
/// Returns `(span_name, row)`: `MessageSpanRow` carries no span name, but the golden keys
/// span views by name so a diff points at a recognisable span rather than a raw id.
/// Whether a file in a sample directory is a captured request the golden runner will replay.
///
/// One predicate, shared with `the_corpus_matches_the_support_matrix`. They had two: discovery required a
/// `req-` prefix while the matrix counted every `.pb`/`.json` that was not `expected.json`. So renaming
/// `req-001.pb` to `capture.pb` left the documented count unchanged while silently removing that request
/// from every golden check - the corpus would still claim to cover it.
fn is_captured_request(name: &str) -> bool {
    name.starts_with("req-") && (name.ends_with(".pb") || name.ends_with(".json"))
}

fn rows_for(paths: &[PathBuf]) -> Vec<(String, MessageSpanRow)> {
    let pricing = PricingService::init_for_test().expect("offline pricing service");
    let mut rows = Vec::new();
    for path in paths {
        let request = decode_request(path);
        rows.extend(normalize_for_test(&request, &pricing));
    }
    rows
}

// ============================================================================
// Comparable projection
// ============================================================================

/// One message as the golden records it.
///
/// Volatile fields (ids, timestamps, tokens, cost) are deliberately excluded: they change on
/// every capture and would make every golden a false failure. What is kept is exactly what the
/// user reads — order, role, kind and content — plus the fields the pipeline is expected to
/// derive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct GoldenMessage {
    /// Position in the returned feed. Recorded explicitly so a reordering fails loudly
    /// rather than silently shifting every line of the diff.
    index: usize,
    role: String,
    entry_type: String,
    /// Content, normalised: long text is truncated and whitespace collapsed so the golden
    /// stays reviewable, and model output wording differences do not dominate the diff.
    content: String,
    /// Digest of the FULL content block, so a change past the preview's cutoff — or one that
    /// survives whitespace collapse — still fails. The preview above is for reading; this is
    /// what actually guards the content.
    content_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observation_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct GoldenView {
    message_count: usize,
    /// Role sequence on its own line: the single most reviewable signal for ordering bugs.
    role_sequence: Vec<String>,
    tool_names: Vec<String>,
    tool_definition_count: usize,
    messages: Vec<GoldenMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Golden {
    label: String,
    /// Number of OTLP requests and spans the fixture carries, so a re-capture that lost
    /// data is obvious at the top of the diff.
    request_count: usize,
    span_count: usize,
    trace_count: usize,
    /// One entry per span, keyed `<trace_id_prefix>/<span_name>` (ids are unstable, so only
    /// a short prefix is kept for grouping).
    session_count: usize,
    span_views: BTreeMap<String, GoldenView>,
    trace_views: BTreeMap<String, GoldenView>,
    /// One per session, keyed by session id (or `trace:<id>` for a sessionless trace), because
    /// the endpoint serves one session at a time. Merging them all into a single view tested a
    /// request no client can make.
    session_views: BTreeMap<String, GoldenView>,
    /// The project feed over every row of the fixture, newest first.
    #[serde(default)]
    feed_view: GoldenView,
}

const MAX_CONTENT: usize = 240;

/// Stable digest of a full content block. Canonicalised through serde_json so key order does
/// not matter, then hashed - short enough to keep the golden readable, wide enough that a
/// real change cannot collide.
fn content_digest(value: &serde_json::Value) -> String {
    use std::hash::{Hash, Hasher};
    // Keys sorted before hashing. The workspace enables serde_json/preserve_order, so
    // to_string() keeps insertion order and two blocks that differ only in key order hashed
    // differently - a re-capture could churn every golden, and duplicate detection could miss a
    // genuine repeat that arrived with its keys in another order.
    let canonical = canonical_json(value);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.len().hash(&mut hasher);
    canonical.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Remove producer-generated values that are embedded in span names but are not conversation
/// semantics. The raw span name is still asserted by its own ingestion tests; this representation
/// exists only to compare two independent executions of the same framework sample.
fn stable_span_name(span_name: &str) -> String {
    let without_duration = span_name
        .rsplit_once(" took ")
        .and_then(|(prefix, suffix)| {
            suffix
                .strip_suffix('s')
                .and_then(|seconds| seconds.parse::<f64>().ok())
                .map(|_| format!("{prefix} took <duration>s"))
        })
        .unwrap_or_else(|| span_name.to_string());

    without_duration
        .split_whitespace()
        .map(|token| {
            let bytes = token.as_bytes();
            let is_uuid = bytes.len() == 36
                && bytes
                    .iter()
                    .enumerate()
                    .all(|(index, byte)| match index {
                        8 | 13 | 18 | 23 => *byte == b'-',
                        _ => byte.is_ascii_hexdigit(),
                    });
            if is_uuid { "<uuid>" } else { token }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Serialize with object keys in sorted order, recursively.
fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical_json(&map[k])
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn normalise_content(value: &serde_json::Value) -> String {
    // Prefer the human-visible text; fall back to compact JSON for structural blocks.
    let raw = value
        .get("text")
        .and_then(|t| t.as_str())
        .map(str::to_owned)
        .or_else(|| {
            value
                .get("content")
                .and_then(|c| c.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default());

    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > MAX_CONTENT {
        let head: String = collapsed.chars().take(MAX_CONTENT).collect();
        format!("{head}…[{} chars]", collapsed.chars().count())
    } else {
        collapsed
    }
}

/// Identity of a message for invariant purposes, including the trace it belongs to.
///
/// Not part of the golden: trace ids change on every capture. But invariants have to respect
/// conversation boundaries, so they need it. The same prompt in two different traces is two
/// legitimate messages, not a duplicate - samples that run a conversation twice (as
/// `strands/tool_use` does, once with a session id and once without) produce exactly that.
#[derive(Debug, Clone)]
struct InvariantRow {
    trace_id: String,
    span_id: String,
    /// The span's ancestor chain (self last), so a check can tell an exception propagating up a
    /// hierarchy from the same failure reported by independent sibling spans.
    span_path: Vec<String>,
    index: usize,
    role: String,
    entry_type: String,
    content: String,
    /// Digest of the FULL content. Duplicate identity must not use the truncated, whitespace
    /// collapsed preview: two genuinely different long messages share a preview and would be
    /// reported as duplicates, while a whitespace-only difference would hide a real one.
    content_digest: String,
    /// Deduplication's proved occurrence rank for otherwise identical content.
    occurrence_ordinal: u32,
    /// Correlation id, so a result can be matched to the call it answers rather than merely
    /// counted against it.
    tool_use_id: Option<String>,
    /// The event or attribute this block was read from - what an extractor claims.
    carrier: String,
    /// Whether that carrier's positions state the order its observations belong in.
    ///
    /// Read from the same declared carrier fact the pipeline read, with the same span context, rather
    /// than re-derived here. The subsequence invariant applies only where a carrier *claims* an order:
    /// an orchestration span re-listing a turn is a bag, and holding it to its listing order asserted
    /// that a final answer preceded the tool calls that produced it.
    carrier_orders_positions: bool,
    /// Whether distinct positions in this carrier prove distinct occurrences.
    carrier_proves_occurrence: bool,
    /// Where the block sat in that carrier's payload, as a sortable string.
    position: String,
}

/// Which API endpoint a view reproduces. Every one of them calls `process_spans`; what
/// differs is the row set and the post-scoping, and getting that wrong means testing
/// something the API never does.
enum View<'a> {
    /// `/spans/{trace}/{span}/messages` - rows for one span.
    Span,
    /// `/sessions/{id}/messages` - rows for one session.
    Session,
    /// `/traces/{id}/messages` - when the trace belongs to a session the endpoint loads the
    /// WHOLE session so cross-trace prefix stripping can run, then scopes the result back to
    /// the requested trace. Processing the trace in isolation skips that stripping entirely.
    Trace {
        trace_id: &'a str,
        session_scoped: bool,
    },
    /// `/feed/messages` - the project feed, newest first, over every row the fixture holds.
    ///
    /// The one view that used `process_feed`, and the one the harness did not check. It is where a
    /// duplicate can still surface: it has its own ordering and, before the trace-complete
    /// reconstruction, its own answer to what collapses. A page is not modelled - pagination is a
    /// property of the endpoint, not of parsing - so this is the whole fixture as one page, which is
    /// what a page of a small project is.
    Feed,
}

fn build_view(rows: Vec<MessageSpanRow>, view: View<'_>) -> (GoldenView, Vec<InvariantRow>) {
    let options = FeedOptions::new();

    // All three endpoints call process_spans; `process_feed` belongs to the project feed
    // endpoint (routes/otel/feed.rs) and has different ordering semantics, so using it for
    // the session view tested behaviour no session request can produce.
    let result = match &view {
        View::Trace {
            trace_id,
            session_scoped: true,
        } => {
            let scoped_tools =
                extract_tools_from_rows(rows.iter().filter(|r| r.trace_id == **trace_id));
            let processed = process_spans(rows, &options);
            scope_feed_to_trace(&processed, scoped_tools, trace_id)
        }
        View::Feed => process_feed(rows, &options),
        View::Span => process_span(rows, &options),
        _ => process_spans(rows, &options),
    };

    let mut messages = Vec::new();
    for (index, block) in result.messages.iter().enumerate() {
        let content_json = serde_json::to_value(&block.content).unwrap_or(json!(null));
        messages.push(GoldenMessage {
            index,
            role: block.role.as_str().to_string(),
            entry_type: block.entry_type.clone(),
            content: normalise_content(&content_json),
            content_digest: content_digest(&content_json),
            tool_name: content_json
                .get("name")
                .and_then(|n| n.as_str())
                .map(str::to_owned),
            finish_reason: block.finish_reason.as_ref().map(|f| format!("{f:?}")),
            observation_type: block.observation_type.clone(),
        });
    }

    let invariant_rows = result
        .messages
        .iter()
        .zip(messages.iter())
        .map(|(block, m)| {
            let semantics = sideseat_domain::sideml::carrier::semantics_for_context(
                &block.carrier_context(),
            );
            InvariantRow {
                trace_id: block.trace_id.clone(),
                span_id: block.span_id.clone(),
                span_path: block.span_path.clone(),
                index: m.index,
                role: m.role.clone(),
                entry_type: m.entry_type.clone(),
                content: m.content.clone(),
                content_digest: m.content_digest.clone(),
                occurrence_ordinal: block.occurrence_ordinal,
                tool_use_id: block.tool_use_id.clone(),
                carrier: match (&block.event_name, &block.source_attribute) {
                    (Some(event), _) => format!("event:{event}"),
                    (None, Some(attribute)) => format!("attr:{attribute}"),
                    (None, None) => "synthesised".to_string(),
                },
                position: block.position.to_string(),
                carrier_orders_positions: semantics.position_provides_sequence_order,
                carrier_proves_occurrence: semantics.position_proves_distinct_occurrence,
            }
        })
        .collect();

    (
        GoldenView {
            message_count: messages.len(),
            role_sequence: messages.iter().map(|m| m.role.clone()).collect(),
            tool_names: result.tool_names.clone(),
            tool_definition_count: result.tool_definitions.len(),
            messages,
        },
        invariant_rows,
    )
}

/// What a view is scoped to, carried explicitly.
///
/// Previously inferred by string-matching the display key, which broke the moment the key
/// became a canonical label instead of an id prefix - and a scope check that silently stops
/// checking is worse than none.
#[derive(Debug, Clone)]
enum Scope {
    Span {
        trace_id: String,
        span_id: String,
    },
    Trace {
        trace_id: String,
    },
    Session,
    /// The project feed: every trace in the project belongs, so scope constrains nothing - and the
    /// order is newest-first, which the answer check has to account for.
    Feed,
}

/// Golden plus the per-view invariant rows, which are checked but never serialized.
struct Built {
    golden: Golden,
    invariants: Vec<(String, Scope, Vec<InvariantRow>)>,
    /// session id -> its traces, and trace id -> canonical label, for the cross-view invariant.
    ///
    /// Session membership is a set, not a single value: a trace can belong to more than one
    /// session. Google ADK emits its own session id on some spans and the sample's `session.id` on
    /// others - which used to put one ADK trace under two sessions. A trace now belongs to the session
    /// on its earliest span, so each appears exactly once.
    traces_of_session: BTreeMap<String, BTreeSet<String>>,
    trace_labels: BTreeMap<String, String>,
}

/// The content filter every trace/session message query applies
/// (`MESSAGE_CONTENT_FILTER` in server/crates/adapter-duckdb/src/repositories/messages.rs). Rows with no messages,
/// no tools and no error are never returned, so feeding them to the pipeline tests an input
/// the pipeline never sees. Including them made whole sessions come back empty.
fn passes_content_filter(row: &MessageSpanRow) -> bool {
    row.messages_json != "[]"
        || row.tool_definitions_json != "[]"
        || row.tool_names_json != "[]"
        || row.status_code.as_deref() == Some("ERROR")
}

/// `ORDER BY timestamp_start ASC`, as the query does. Capture order is not query order.
fn sorted_by_timestamp(mut rows: Vec<MessageSpanRow>) -> Vec<MessageSpanRow> {
    rows.sort_by(|a, b| {
        a.span_timestamp
            .cmp(&b.span_timestamp)
            .then_with(|| a.span_id.cmp(&b.span_id))
    });
    rows
}

/// A capture-stable description of one span's user-visible projection.
///
/// Span and trace ids are regenerated on every run. When a runtime rounds sibling start times to
/// the same millisecond, ids therefore cannot break the tie without making golden keys flaky.
fn stable_span_projection(span_name: &str, rows: &[MessageSpanRow]) -> String {
    let (view, _) = build_view(sorted_by_timestamp(rows.to_vec()), View::Span);
    format!(
        "{span_name}\0{}",
        serde_json::to_string(&view).expect("golden view is serializable")
    )
}

type StableSpanOrderKey = (
    chrono::DateTime<chrono::Utc>,
    String,
    String,
    String,
);

/// Stable label for each trace: `trace-1`, `trace-2`, ... ordered by earliest span timestamp,
/// then by its semantic span projections.
///
/// The previous key was the first eight characters of the trace id, which silently collided:
/// these ids are time-ordered (UUIDv7-style), so traces created moments apart share a long
/// prefix. Seven fixtures lost trace views to `BTreeMap` overwrites - `openai/session` reported
/// trace_count 3 while comparing one. An index is also stable across re-captures, where a raw
/// id changes every time.
fn trace_labels(rows: &[(String, MessageSpanRow)]) -> BTreeMap<String, String> {
    let mut traces: BTreeMap<
        String,
        BTreeMap<String, (String, Vec<MessageSpanRow>)>,
    > = BTreeMap::new();
    for (span_name, row) in rows {
        traces
            .entry(row.trace_id.clone())
            .or_default()
            .entry(row.span_id.clone())
            .or_insert_with(|| (span_name.clone(), Vec::new()))
            .1
            .push(row.clone());
    }

    let mut ordered = Vec::new();
    for (trace_id, spans) in traces {
        let first = spans
            .values()
            .flat_map(|(_, rows)| rows.iter().map(|row| row.span_timestamp))
            .min()
            .unwrap_or_default();
        let mut projections: Vec<String> = spans
            .values()
            .map(|(name, rows)| stable_span_projection(name, rows))
            .collect();
        projections.sort();
        ordered.push((first, projections.join("\u{1f}"), trace_id));
    }
    ordered.sort();
    ordered
        .into_iter()
        .enumerate()
        .map(|(i, (_, _, id))| (id, format!("trace-{}", i + 1)))
        .collect()
}

fn build_golden(label: &str, paths: &[PathBuf], rows: &[(String, MessageSpanRow)]) -> Built {
    let mut span_views = BTreeMap::new();
    let mut trace_views = BTreeMap::new();
    let mut session_views = BTreeMap::new();
    let mut invariants: Vec<(String, Scope, Vec<InvariantRow>)> = Vec::new();

    let labels = trace_labels(rows);
    let mut by_span: BTreeMap<(String, String), (String, Vec<MessageSpanRow>)> = BTreeMap::new();
    let mut by_trace: BTreeMap<String, Vec<MessageSpanRow>> = BTreeMap::new();
    // session id -> the traces that belong to it. The query is
    // `trace_id IN (SELECT trace_id WHERE session_id = ?)`, so membership is decided per
    // TRACE and then every row of those traces is returned - not only the rows that
    // themselves carry the session id. Filtering by each row's own session_id dropped the
    // rows holding the messages and made sessions look empty.
    let mut traces_of_session: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // trace id -> (earliest timestamp carrying a session id, that session id)
    // Keyed by `(timestamp_start, span_id)`, so the earliest span is unambiguous - the same total order
    // production uses.
    #[allow(clippy::type_complexity)]
    let mut session_of_trace: BTreeMap<
        String,
        (
            (chrono::DateTime<chrono::Utc>, String),
            chrono::DateTime<chrono::Utc>,
            String,
        ),
    > = BTreeMap::new();

    for (span_name, row) in rows {
        by_span
            .entry((row.trace_id.clone(), row.span_id.clone()))
            .or_insert_with(|| (span_name.clone(), Vec::new()))
            .1
            .push(row.clone());
        by_trace
            .entry(row.trace_id.clone())
            .or_default()
            .push(row.clone());

        // Which session a trace belongs to, chosen the way production does: the session on its **earliest**
        // span, ordered by `(timestamp_start, span_id)` so the answer is total. `arg_min` over that key is
        // what every query uses - the display, the membership subquery, the lists and both deletions.
        if let Some(sid) = row.session_id.clone().filter(|s| !s.is_empty()) {
            let key = (row.span_timestamp, row.span_id.clone());
            let ingested = row.ingested_at;
            match session_of_trace.entry(row.trace_id.clone()) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert((key, ingested, sid));
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    // Earliest span wins, and an exact tie is broken by the session id itself.
                    //
                    // That is deliberately **not** production's rule, which is "the latest delivery wins"
                    // (`DEDUP_SPANS` orders by `ingested_at DESC, rowid DESC`). Production cannot face this
                    // tie: it reads one row per span, so two different sessions never compete. This harness
                    // is handed *pre-deduplication* rows - the same span appears once per request that
                    // carried it - and it has no delivery order to consult, because `normalize_for_test`
                    // stamps `ingested_at` from the span's own start time. Three rules were tried: `<` keeps
                    // whichever row came first and `<=` whichever came last, both order-dependent, and
                    // `ingested_at` cannot separate them for the reason just given. Only a function of the
                    // *values* is order-independent here, which is what
                    // `the_order_spans_arrive_in_does_not_change_the_answer` requires.
                    if (&key, &sid) < (&slot.get().0, &slot.get().2) {
                        slot.insert((key, ingested, sid));
                    }
                }
            }
        }
    }

    // Membership follows from that, rather than being collected per row. Collecting each row's own session
    // gave a trace whose spans name two sessions a session view under **both** - which production does not
    // do, so the goldens described an answer no endpoint returns.
    for (trace_id, (_, _, session)) in &session_of_trace {
        traces_of_session
            .entry(session.clone())
            .or_default()
            .insert(trace_id.clone());
    }

    let mut span_numbers = BTreeMap::new();
    let mut spans_by_trace: BTreeMap<String, Vec<StableSpanOrderKey>> = BTreeMap::new();
    for ((trace_id, span_id), (name, span_rows)) in &by_span {
        let first = span_rows
            .iter()
            .map(|row| row.span_timestamp)
            .min()
            .unwrap_or_default();
        spans_by_trace.entry(trace_id.clone()).or_default().push((
            first,
            name.clone(),
            stable_span_projection(name, span_rows),
            span_id.clone(),
        ));
    }
    for (trace_id, mut spans) in spans_by_trace {
        // The final id tie-breaker only distinguishes semantically identical spans. If those ids
        // swap on a recapture, their views are identical, so the numbered golden map is unchanged.
        spans.sort();
        for (index, (_, _, _, span_id)) in spans.into_iter().enumerate() {
            span_numbers.insert((trace_id.clone(), span_id), index + 1);
        }
    }

    // Rows a session query would return: every row of every trace in the session, content
    // filtered, timestamp ordered.
    let session_rows = |sid: &str| -> Vec<MessageSpanRow> {
        let traces = traces_of_session.get(sid).cloned().unwrap_or_default();
        sorted_by_timestamp(
            rows.iter()
                .map(|(_, r)| r)
                .filter(|r| traces.contains(&r.trace_id) && passes_content_filter(r))
                .cloned()
                .collect(),
        )
    };

    for ((trace_id, span_id), (name, span_rows)) in &by_span {
        // `<trace-N>/<span name>/<span-M>`: the span index disambiguates repeats of the same
        // name and, like the trace label, survives a re-capture.
        //
        // Numbered by timestamp, name, and semantic projection. Random ids participate only when
        // two spans are indistinguishable to every golden view.
        let span_no = span_numbers
            .get(&(trace_id.clone(), span_id.clone()))
            .copied()
            .unwrap_or(0);
        let key = format!(
            "{}/{name}/span-{span_no}",
            labels
                .get(trace_id)
                .map(String::as_str)
                .unwrap_or("trace-?")
        );
        // The span query filters by span_id alone and applies no content filter.
        let (view, inv) = build_view(sorted_by_timestamp(span_rows.clone()), View::Span);
        invariants.push((
            format!("span {key}"),
            Scope::Span {
                trace_id: trace_id.clone(),
                span_id: span_id.clone(),
            },
            inv,
        ));
        span_views.insert(key, view);
    }

    for (trace_id, trace_rows) in &by_trace {
        let key = labels
            .get(trace_id)
            .cloned()
            .unwrap_or_else(|| "trace-?".to_string());
        let session = session_of_trace
            .get(trace_id)
            .map(|(_, _, sid)| sid.clone());
        let (rows_for_view, session_scoped) = match &session {
            Some(sid) => (session_rows(sid), true),
            None => (
                sorted_by_timestamp(
                    trace_rows
                        .iter()
                        .filter(|r| passes_content_filter(r))
                        .cloned()
                        .collect(),
                ),
                false,
            ),
        };
        let (view, inv) = build_view(
            rows_for_view,
            View::Trace {
                trace_id,
                session_scoped,
            },
        );
        invariants.push((
            format!("trace {key}"),
            Scope::Trace {
                trace_id: trace_id.clone(),
            },
            inv,
        ));
        trace_views.insert(key, view);
    }

    for sid in traces_of_session.keys() {
        let (view, inv) = build_view(session_rows(sid), View::Session);
        invariants.push((format!("session {sid}"), Scope::Session, inv));
        session_views.insert(sid.clone(), view);
    }

    // The project feed over every row the fixture holds. The endpoint applies the same content
    // filter the trace and session queries do, so the row set is built the same way.
    let feed_rows = sorted_by_timestamp(
        by_span
            .values()
            .flat_map(|rows| rows.1.iter())
            .filter(|r| passes_content_filter(r))
            .cloned()
            .collect(),
    );
    let (feed_view, feed_inv) = build_view(feed_rows, View::Feed);
    invariants.push(("feed".to_string(), Scope::Feed, feed_inv));

    Built {
        golden: Golden {
            label: label.to_string(),
            request_count: paths.len(),
            span_count: by_span.len(),
            trace_count: by_trace.len(),
            session_count: traces_of_session.len(),
            span_views,
            trace_views,
            session_views,
            feed_view,
        },
        invariants,
        traces_of_session,
        trace_labels: labels,
    }
}

/// Duplicate detection, the property most at risk from dedup changes.
///
/// Partitioned by trace: identity is (role, entry_type, content) **within one trace**. Two
/// identical prompts in two different traces are two legitimate messages, not a duplicate -
/// several samples run their whole conversation twice, once with a session id and once
/// without, so a session view contains each prompt twice by design.
///
/// A genuine repeat needs independent occurrence evidence. Ordered payload positions prove repeated
/// members within one span. Separate execution spans prove them only when deduplication assigned a
/// different occurrence rank to every copy. A re-delivery retains both the span and rank, while peer
/// spans merely re-listing one request retain the same rank.
fn assert_no_duplicates(label: &str, view_name: &str, rows: &[InvariantRow]) {
    type Identity<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);
    let mut seen: HashMap<Identity<'_>, Vec<&InvariantRow>> = HashMap::new();
    for r in rows {
        // An exception is a fact about a *span*, and it is composed from that span's own
        // `exception_*` fields rather than read from a payload - so two spans reporting the same
        // failure are two failures, not a re-send. `openai-agents/image_gen` is the case: three
        // `generate_image` executions each failed with the same message, and collapsing them showed
        // three tool results beside one explanation.
        //
        // This is the amendment the check's own doc comment anticipated: the pipeline learned to keep a
        // repeat that has no id, so the invariant had to learn it in the same change, or it reports the
        // repair as the defect.
        let scope = if r.carrier == "attr:exception" {
            r.span_id.as_str()
        } else {
            ""
        };
        seen
            .entry((
                r.trace_id.as_str(),
                scope,
                r.role.as_str(),
                r.entry_type.as_str(),
                r.content_digest.as_str(),
            ))
            .or_default()
            .push(r);
    }
    let mut dupes: Vec<String> = seen
        .iter()
        .filter(|(_, copies)| {
            if copies.len() < 2 {
                return false;
            }
            let occurrences: HashSet<(&str, &str, &str)> = copies
                .iter()
                .filter(|row| row.carrier_proves_occurrence && !row.position.is_empty())
                .map(|row| {
                    (
                        row.span_id.as_str(),
                        row.carrier.as_str(),
                        row.position.as_str(),
                    )
                })
                .collect();
            let ordinals: HashSet<u32> =
                copies.iter().map(|row| row.occurrence_ordinal).collect();
            let spans: HashSet<&str> = copies.iter().map(|row| row.span_id.as_str()).collect();
            let carrier_proves_all = occurrences.len() == copies.len();
            let separate_executions_prove_all =
                ordinals.len() == copies.len() && spans.len() == copies.len();
            !carrier_proves_all && !separate_executions_prove_all
        })
        .map(|((trace, _scope, role, kind, content), copies)| {
            let head: String = content.chars().take(70).collect();
            format!(
                "{}x in trace {} [{role}/{kind}] {head}",
                copies.len(),
                &trace[..trace.len().min(8)]
            )
        })
        .collect();
    dupes.sort();
    assert!(
        dupes.is_empty(),
        "{label} / {view_name}: duplicate messages within one trace:\n  {}",
        dupes.join("\n  ")
    );
}

/// A tool_use block's own call id: from the block field when set, otherwise from the id
/// embedded in the serialized content.
fn extract_tool_use_id(row: &InvariantRow) -> Option<&str> {
    row.tool_use_id.as_deref().or_else(|| {
        let start = row.content.find("\"id\":\"")? + 6;
        let rest = &row.content[start..];
        let end = rest.find('"')?;
        Some(&rest[..end])
    })
}

/// Tool calls and results must balance within a trace.
///
/// Counted per `(trace, tool_use_id)`: parallel calls may return out of order, and some producers
/// legitimately reuse an id for multiple sequential executions.
///
/// A result whose id matches no call is a defect, and results cannot outnumber calls for that id.
/// Occurrence order is checked separately by `assert_tool_causality`.
///
/// Unanswered calls are NOT asserted - a cancelled or failed turn legitimately leaves one open,
/// and a time-filtered view can cut between the two halves.
/// Fixtures whose SOURCE telemetry cannot satisfy tool pairing, with the reason.
///
/// A capability limit of the framework, not a parsing defect, so it is recorded per fixture
/// rather than weakening the check for everyone. Verified by reading the raw payload: the id in
/// question appears only on `claude_code.tool.execution` / `claude_code.tool` spans as
/// `tool_use_id`, and the Claude Code CLI never emits a matching `tool_use` block for a
/// SUBAGENT's tool call — the subagent's assistant message is not part of the exported
/// conversation. The result is therefore genuinely callless upstream.
const PAIRING_EXEMPT: &[(&str, &str)] = &[
    (
        "claude-agent-sdk/subagents",
        "Claude Code CLI emits subagent tool executions without the matching tool_use block",
    ),
    (
        "claude-agent-sdk-js/subagents",
        "Claude Code CLI emits subagent tool executions without the matching tool_use block",
    ),
];

/// Fixtures whose source has no answer to show, with the reason.
///
/// Everything else is required to have one: a run that was asked something and completed must show
/// what it replied. CrewAI's answers were dropped for months because no invariant said so - its
/// reasoning fixture recorded `system -> user -> user -> user`, three questions and no answers, and
/// the goldens blessed it as correct.
/// Empty, and that is the point: the one entry it held is gone because the defect behind it was fixed.
///
/// `strands/error` was exempted on the grounds that "the sample exists to fail, so the run never
/// produced an answer". The run *did* produce an answer - a `ValidationException` - and three span views
/// displayed it while the trace view showed `system, user` and nothing else, because a parent error span
/// deferred to a child that had ERROR status and no exception fields to render. The exemption was
/// describing a defect rather than a property of the telemetry, which is exactly what an exemption must
/// never do: the rule is that a legitimate one proves the invariant's antecedent is false in the source.
///
/// Kept as an empty list rather than deleted, because the next fixture that genuinely cannot answer -
/// a cancelled run, a hard transport failure - needs somewhere to be declared with its reason.
const NO_ANSWER_EXPECTED: &[(&str, &str)] = &[];
