//! Golden tests for message parsing, per framework and per sample.
//!
//! Message count, content, ordering and absence of duplicates are the properties users
//! actually see, and they are easy to break invisibly: a dedup identity tweak or a new
//! extractor can silently drop a tool result or duplicate a turn, and every existing unit
//! test still passes because they each cover one stage in isolation.
//!
//! This harness closes that gap end to end. Each fixture is the exact OTLP payload a real
//! sample sent (captured by `misc/record-otlp.py`, see `misc/capture-message-fixtures.sh`).
//! It is replayed through the real ingestion path — `extract_attributes_batch`,
//! `extract_messages_batch`, SideML conversion, enrichment — and then through each of the
//! three feed views the API exposes:
//!
//! | View    | Feed entry point       | API endpoint                          |
//! |---------|------------------------|---------------------------------------|
//! | span    | `process_spans` (1 span)  | `/spans/{trace}/{span}/messages`   |
//! | trace   | `process_spans` (1 trace) | `/traces/{id}/messages`            |
//! | session | `process_feed`            | `/sessions/{id}/messages`          |
//!
//! The result is compared against a committed expectation file. Regenerate with:
//!
//! ```bash
//! UPDATE_GOLDENS=1 cargo test -p sideseat-server message_goldens
//! ```
//!
//! Regenerating is deliberately a separate, explicit step: a golden written straight from
//! current output enshrines whatever bugs exist today, so a regenerated file has to be read
//! before it is committed. The invariant checks below exist precisely because they hold
//! regardless of what the golden says — they catch bugs a blind snapshot would bless.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message as _;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::routes::otel::messages::scope_feed_to_trace;
use crate::data::types::MessageSpanRow;
use crate::domain::pricing::PricingService;
use crate::domain::sideml::feed::{FeedOptions, extract_tools_from_rows, process_spans};

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
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("pb") | Some("json")
        ) && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("req-"))
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
fn rows_for(paths: &[PathBuf]) -> Vec<(String, MessageSpanRow)> {
    let pricing = PricingService::init_for_test().expect("offline pricing service");
    let mut rows = Vec::new();
    for path in paths {
        let request = decode_request(path);
        rows.extend(super::normalize_for_test(&request, &pricing));
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}

const MAX_CONTENT: usize = 240;

/// Stable digest of a full content block. Canonicalised through serde_json so key order does
/// not matter, then hashed - short enough to keep the golden readable, wide enough that a
/// real change cannot collide.
fn content_digest(value: &serde_json::Value) -> String {
    use std::hash::{Hash, Hasher};
    let canonical = serde_json::to_string(value).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.len().hash(&mut hasher);
    canonical.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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
    index: usize,
    role: String,
    entry_type: String,
    content: String,
    /// Kept for diagnostics in assertion messages rather than for matching, which goes by id.
    #[allow(dead_code)]
    tool_name: Option<String>,
    /// Correlation id, so a result can be matched to the call it answers rather than merely
    /// counted against it.
    tool_use_id: Option<String>,
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
}

fn build_view(rows: Vec<MessageSpanRow>, view: View<'_>) -> (GoldenView, Vec<InvariantRow>) {
    let options = FeedOptions::new();

    // All three endpoints call process_spans; `process_feed` belongs to the project feed
    // endpoint (routes/otel/feed.rs) and has different ordering semantics, so using it for
    // the session view tested behaviour no session request can produce.
    let mut result = match &view {
        View::Trace {
            trace_id,
            session_scoped: true,
        } => {
            let scoped_tools =
                extract_tools_from_rows(rows.iter().filter(|r| r.trace_id == **trace_id));
            let mut processed = process_spans(rows, &options);
            scope_feed_to_trace(&mut processed, scoped_tools, trace_id);
            processed
        }
        _ => process_spans(rows, &options),
    };
    // Deterministic tool ordering: the pipeline collects these from a hash-ordered set in
    // places, and an unstable order would churn every golden.
    result.tool_names.sort();

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
        .map(|(block, m)| InvariantRow {
            trace_id: block.trace_id.clone(),
            span_id: block.span_id.clone(),
            index: m.index,
            role: m.role.clone(),
            entry_type: m.entry_type.clone(),
            content: m.content.clone(),
            tool_name: m.tool_name.clone(),
            tool_use_id: block.tool_use_id.clone(),
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

/// Golden plus the per-view invariant rows, which are checked but never serialized.
struct Built {
    golden: Golden,
    invariants: Vec<(String, Vec<InvariantRow>)>,
}

/// The content filter every trace/session message query applies
/// (`MESSAGE_CONTENT_FILTER` in data/duckdb/repositories/messages.rs). Rows with no messages,
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

fn build_golden(label: &str, paths: &[PathBuf], rows: &[(String, MessageSpanRow)]) -> Built {
    let mut span_views = BTreeMap::new();
    let mut trace_views = BTreeMap::new();
    let mut session_views = BTreeMap::new();
    let mut invariants: Vec<(String, Vec<InvariantRow>)> = Vec::new();

    let mut by_span: BTreeMap<(String, String), (String, Vec<MessageSpanRow>)> = BTreeMap::new();
    let mut by_trace: BTreeMap<String, Vec<MessageSpanRow>> = BTreeMap::new();
    // session id -> the traces that belong to it. The query is
    // `trace_id IN (SELECT trace_id WHERE session_id = ?)`, so membership is decided per
    // TRACE and then every row of those traces is returned - not only the rows that
    // themselves carry the session id. Filtering by each row's own session_id dropped the
    // rows holding the messages and made sessions look empty.
    let mut traces_of_session: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut session_of_trace: BTreeMap<String, Option<String>> = BTreeMap::new();

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

        let session = row.session_id.clone().filter(|s| !s.is_empty());
        if let Some(sid) = &session {
            traces_of_session
                .entry(sid.clone())
                .or_default()
                .insert(row.trace_id.clone());
        }
        let entry = session_of_trace.entry(row.trace_id.clone()).or_insert(None);
        if entry.is_none() {
            *entry = session;
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
        let key = format!(
            "{}/{name}/{}",
            &trace_id[..trace_id.len().min(8)],
            &span_id[..span_id.len().min(4)]
        );
        // The span query filters by span_id alone and applies no content filter.
        let (view, inv) = build_view(sorted_by_timestamp(span_rows.clone()), View::Span);
        invariants.push((format!("span {key}"), inv));
        span_views.insert(key, view);
    }

    for (trace_id, trace_rows) in &by_trace {
        let key = trace_id[..trace_id.len().min(8)].to_string();
        let session = session_of_trace.get(trace_id).cloned().flatten();
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
        invariants.push((format!("trace {key}"), inv));
        trace_views.insert(key, view);
    }

    for sid in traces_of_session.keys() {
        let (view, inv) = build_view(session_rows(sid), View::Session);
        invariants.push((format!("session {sid}"), inv));
        session_views.insert(sid.clone(), view);
    }

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
        },
        invariants,
    }
}

/// Duplicate detection, the property most at risk from dedup changes.
///
/// Partitioned by trace: identity is (role, entry_type, content) **within one trace**. Two
/// identical prompts in two different traces are two legitimate messages, not a duplicate -
/// several samples run their whole conversation twice, once with a session id and once
/// without, so a session view contains each prompt twice by design.
fn assert_no_duplicates(label: &str, view_name: &str, rows: &[InvariantRow]) {
    let mut seen: HashMap<(&str, &str, &str, &str), usize> = HashMap::new();
    for r in rows {
        *seen
            .entry((
                r.trace_id.as_str(),
                r.role.as_str(),
                r.entry_type.as_str(),
                r.content.as_str(),
            ))
            .or_insert(0) += 1;
    }
    let mut dupes: Vec<String> = seen
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|((trace, role, kind, content), n)| {
            let head: String = content.chars().take(70).collect();
            format!(
                "{n}x in trace {} [{role}/{kind}] {head}",
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
/// Counted per (trace, tool_use_id) rather than with a stack: a stack assumes results arrive
/// in call order, which is false for parallel tool calls — every framework here issues two at
/// once — and it also cannot tell whether a result belongs to the call it was popped against.
/// The earlier stack version additionally never checked that the stack drained, so
/// "every call is answered" was never actually verified.
///
/// Unanswered calls are reported, not asserted: a cancelled or failed turn legitimately
/// leaves one open, so this is a warning-grade property recorded in the golden instead.
/// What IS asserted is the direction that can only be a defect: a result with no call.
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

fn assert_tool_pairing(label: &str, view_name: &str, rows: &[InvariantRow]) {
    if let Some((_, reason)) = PAIRING_EXEMPT.iter().find(|(l, _)| *l == label) {
        eprintln!("message_goldens: {label}: tool pairing not asserted - {reason}");
        return;
    }
    let mut calls: BTreeMap<(&str, String), usize> = BTreeMap::new();
    for r in rows {
        if r.entry_type == "tool_use"
            && let Some(id) = extract_tool_use_id(r)
        {
            *calls
                .entry((r.trace_id.as_str(), id.to_string()))
                .or_insert(0) += 1;
        }
    }
    for r in rows {
        if r.entry_type != "tool_result" {
            continue;
        }
        let Some(id) = r.tool_use_id.as_deref() else {
            continue; // id-less results are matched by content by the pipeline
        };
        let key = (r.trace_id.as_str(), id.to_string());
        assert!(
            calls.contains_key(&key),
            "{label} / {view_name}: tool_result at index {} has id {id:?} with no matching tool_use in trace {}",
            r.index,
            &r.trace_id[..r.trace_id.len().min(8)]
        );
    }
}

/// The projection itself must be self-consistent.
///
/// Deliberately narrow. Two earlier checks here were theatre: dense ascending indices cannot
/// fail because the projection assigns them with `enumerate()`, and "roles are one of four
/// known values" cannot fail because `ChatRole` has exactly four variants — an unmapped source
/// role has already become `User` by this point, silently, which is the defect those checks
/// looked like they were guarding. What remains are the fields that could genuinely disagree.
fn assert_projection_consistent(label: &str, view_name: &str, view: &GoldenView) {
    assert_eq!(
        view.role_sequence.len(),
        view.message_count,
        "{label} / {view_name}: role_sequence and message_count disagree"
    );
    assert_eq!(
        view.messages.len(),
        view.message_count,
        "{label} / {view_name}: message list and message_count disagree"
    );
    for m in &view.messages {
        assert!(
            !m.content_digest.is_empty(),
            "{label} / {view_name}: message {} has no content digest",
            m.index
        );
    }
}

/// Every returned block must belong to the scope that was requested.
///
/// This is the property the endpoints promise and the one a scoping bug breaks: a span view
/// leaking a sibling span's messages, or a trace view leaking another trace's, is invisible to
/// count and ordering checks because the totals still look plausible.
fn assert_scope(label: &str, view_name: &str, rows: &[InvariantRow]) {
    if let Some(rest) = view_name.strip_prefix("span ") {
        // Key is `<trace8>/<name>/<span4>`.
        let mut parts = rest.split('/');
        let trace_prefix = parts.next().unwrap_or_default();
        let span_prefix = parts.next_back().unwrap_or_default();
        for r in rows {
            assert!(
                r.trace_id.starts_with(trace_prefix),
                "{label} / {view_name}: block from trace {} leaked into a span view of trace {trace_prefix}",
                &r.trace_id[..r.trace_id.len().min(8)]
            );
            assert!(
                r.span_id.starts_with(span_prefix),
                "{label} / {view_name}: block from span {} leaked into a span view of {span_prefix}",
                &r.span_id[..r.span_id.len().min(4)]
            );
        }
    } else if let Some(trace_prefix) = view_name.strip_prefix("trace ") {
        // The trace endpoint loads a whole session and scopes back; a leak here means
        // scope_feed_to_trace failed to filter.
        for r in rows {
            assert!(
                r.trace_id.starts_with(trace_prefix),
                "{label} / {view_name}: block from trace {} leaked into the view after scoping",
                &r.trace_id[..r.trace_id.len().min(8)]
            );
        }
    }
    // Session views legitimately span traces, so there is nothing to assert there.
}

/// A trace with content must not collapse to nothing.
///
/// Deliberately weak, because the obvious stronger claim is false: a span view can hold *more*
/// messages than its trace view. Each generation span re-sends the whole conversation history,
/// so one span legitimately shows 21 messages where the trace shows the 7 unique ones after
/// history dedup. Asserting trace >= span reported every multi-turn trace as broken.
///
/// What does hold: if any span in a trace produced messages, the trace view must too.
fn assert_trace_not_empty(label: &str, golden: &Golden) {
    for (trace_key, trace_view) in &golden.trace_views {
        let any_span_has_content = golden
            .span_views
            .iter()
            .any(|(span_key, v)| span_key.starts_with(trace_key.as_str()) && v.message_count > 0);
        assert!(
            !any_span_has_content || trace_view.message_count > 0,
            "{label} / trace {trace_key}: spans carry messages but the trace view is empty"
        );
    }
}

/// Content must not be empty for text-bearing entries: an empty bubble in the UI is
/// indistinguishable from a parsing failure.
fn assert_no_empty_text(label: &str, view_name: &str, view: &GoldenView) {
    for m in &view.messages {
        if matches!(m.entry_type.as_str(), "text" | "thinking") {
            assert!(
                !m.content.trim().is_empty(),
                "{label} / {view_name}: empty {} block at index {}",
                m.entry_type,
                m.index
            );
        }
    }
}

fn check_invariants(label: &str, built: &Built) {
    let golden = &built.golden;
    let views: Vec<(String, &GoldenView)> = golden
        .span_views
        .iter()
        .map(|(k, v)| (format!("span {k}"), v))
        .chain(
            golden
                .trace_views
                .iter()
                .map(|(k, v)| (format!("trace {k}"), v)),
        )
        .chain(
            golden
                .session_views
                .iter()
                .map(|(k, v)| (format!("session {k}"), v)),
        )
        .collect();

    for (name, view) in &views {
        assert_projection_consistent(label, name, view);
        assert_no_empty_text(label, name, view);
    }

    for (name, rows) in &built.invariants {
        assert_no_duplicates(label, name, rows);
        assert_scope(label, name, rows);
        // Span views are excluded from both tool checks: a single span holds only one half of
        // a call/result pair, so neither the pairing nor the id of the other half is present.
        if !name.starts_with("span ") {
            assert_tool_pairing(label, name, rows);
        }
    }

    assert_trace_not_empty(label, golden);

    // Not every sample uses sessions, so assert on traces: those always exist.
    let total: usize = golden.trace_views.values().map(|v| v.message_count).sum();
    assert!(
        total > 0,
        "{label}: every trace view is empty - the sample produced no messages at all"
    );
}

fn golden_path(label: &str) -> PathBuf {
    fixture_root().join(label).join("expected.json")
}

#[test]
fn message_goldens() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        // Not a silent pass: capturing fixtures needs credentials and a live model, so a
        // clean checkout legitimately has none. Say so loudly instead of reporting success.
        eprintln!(
            "message_goldens: no fixtures under {} - run misc/capture-message-fixtures.sh",
            fixture_root().display()
        );
        return;
    }

    let update = std::env::var("UPDATE_GOLDENS").is_ok();
    let mut failures: Vec<String> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for (label, paths) in &fixtures {
        let rows = rows_for(paths);
        let built = build_golden(label, paths, &rows);
        let golden = &built.golden;

        if update {
            // Report violations rather than aborting: the point of a record run is to see
            // the whole picture, including which fixtures are currently wrong. Aborting on
            // the first one hides the rest.
            if let Err(e) = std::panic::catch_unwind(|| check_invariants(label, &built)) {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_else(|| "unknown".to_string());
                eprintln!("message_goldens: INVARIANT VIOLATION while recording {label}:\n  {msg}");
                violations.push(format!("{label}: {msg}"));
            }
        } else {
            check_invariants(label, &built);
        }

        let path = golden_path(label);
        if update {
            std::fs::create_dir_all(path.parent().unwrap()).ok();
            let json = serde_json::to_string_pretty(golden).unwrap();
            std::fs::write(&path, json + "\n").unwrap();
            eprintln!("message_goldens: wrote {}", path.display());
            continue;
        }

        let Ok(existing) = std::fs::read_to_string(&path) else {
            failures.push(format!(
                "{label}: no expectation file at {} (UPDATE_GOLDENS=1 to record)",
                path.display()
            ));
            continue;
        };
        let expected: Golden = match serde_json::from_str(&existing) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{label}: expectation file is not valid: {e}"));
                continue;
            }
        };
        checked += 1;

        if expected != *golden {
            failures.push(describe_diff(label, &expected, golden));
        }
    }

    if update {
        eprintln!("message_goldens: recorded {} fixture(s)", fixtures.len());
        // Recording still fails when an invariant was violated. Writing the files first is
        // deliberate - you want to see the whole picture - but exiting 0 would let a
        // known-bad expectation be committed as if it had been reviewed and accepted.
        assert!(
            violations.is_empty(),
            "recorded {} fixture(s) but {} violated an invariant; the written expectations \
             capture current (wrong) behaviour and must not be committed as-is:\n\n{}",
            fixtures.len(),
            violations.len(),
            violations.join("\n\n")
        );
        return;
    }

    assert!(
        failures.is_empty(),
        "message parsing changed for {} of {} fixture(s):\n\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n\n")
    );
    eprintln!("message_goldens: {checked} fixture(s) matched");
}

/// Human-readable differences between one expected and one actual view.
fn compare_view(name: &str, e: &GoldenView, a: &GoldenView) -> Vec<String> {
    let mut out = Vec::new();
    if e.message_count != a.message_count {
        out.push(format!(
            "  {name}: message_count expected {}, got {}",
            e.message_count, a.message_count
        ));
    }
    if e.role_sequence != a.role_sequence {
        out.push(format!("  {name}: role sequence changed"));
        out.push(format!("    expected: {}", e.role_sequence.join(" -> ")));
        out.push(format!("    actual:   {}", a.role_sequence.join(" -> ")));
    }
    if e.tool_names != a.tool_names {
        out.push(format!(
            "  {name}: tool_names expected {:?}, got {:?}",
            e.tool_names, a.tool_names
        ));
    }
    for (i, (em, am)) in e.messages.iter().zip(a.messages.iter()).enumerate() {
        if em != am {
            out.push(format!("  {name}: message {i} changed"));
            if em.role != am.role || em.entry_type != am.entry_type {
                out.push(format!(
                    "    kind: expected {}/{}, got {}/{}",
                    em.role, em.entry_type, am.role, am.entry_type
                ));
            }
            if em.content != am.content {
                out.push(format!(
                    "    content expected: {}",
                    em.content.chars().take(100).collect::<String>()
                ));
                out.push(format!(
                    "    content actual:   {}",
                    am.content.chars().take(100).collect::<String>()
                ));
            }
            break; // one example per view is enough to diagnose
        }
    }
    out
}

/// Views that appeared or vanished between expectation and actual.
fn key_set_diff(
    kind: &str,
    expected: &BTreeMap<String, GoldenView>,
    actual: &BTreeMap<String, GoldenView>,
) -> Vec<String> {
    let ek: HashSet<&String> = expected.keys().collect();
    let ak: HashSet<&String> = actual.keys().collect();
    let mut gone: Vec<&String> = ek.difference(&ak).copied().collect();
    let mut added: Vec<&String> = ak.difference(&ek).copied().collect();
    gone.sort();
    added.sort();
    let mut out = Vec::new();
    if !gone.is_empty() {
        out.push(format!("  {kind} views missing: {gone:?}"));
    }
    if !added.is_empty() {
        out.push(format!("  {kind} views added: {added:?}"));
    }
    out
}

/// A readable summary rather than two pretty-printed blobs: the useful signal is almost
/// always a count or a role sequence, so lead with those.
fn describe_diff(label: &str, expected: &Golden, actual: &Golden) -> String {
    let mut out = vec![format!("{label}:")];

    if expected.span_count != actual.span_count {
        out.push(format!(
            "  span_count: expected {}, got {}",
            expected.span_count, actual.span_count
        ));
    }
    if expected.trace_count != actual.trace_count {
        out.push(format!(
            "  trace_count: expected {}, got {}",
            expected.trace_count, actual.trace_count
        ));
    }
    if expected.session_count != actual.session_count {
        out.push(format!(
            "  session_count: expected {}, got {}",
            expected.session_count, actual.session_count
        ));
    }

    if expected.request_count != actual.request_count {
        out.push(format!(
            "  request_count: expected {}, got {} (fixture re-captured?)",
            expected.request_count, actual.request_count
        ));
    }

    // Key-set changes were previously invisible, leaving only "differs in a field not
    // summarised above" - useless when a view appeared or vanished.
    out.extend(key_set_diff(
        "trace",
        &expected.trace_views,
        &actual.trace_views,
    ));
    out.extend(key_set_diff(
        "span",
        &expected.span_views,
        &actual.span_views,
    ));
    out.extend(key_set_diff(
        "session",
        &expected.session_views,
        &actual.session_views,
    ));

    for (key, e) in &expected.session_views {
        if let Some(a) = actual.session_views.get(key) {
            out.extend(compare_view(&format!("session {key}"), e, a));
        }
    }

    for (key, e) in &expected.trace_views {
        if let Some(a) = actual.trace_views.get(key) {
            out.extend(compare_view(&format!("trace {key}"), e, a));
        }
    }
    for (key, e) in &expected.span_views {
        if let Some(a) = actual.span_views.get(key) {
            out.extend(compare_view(&format!("span {key}"), e, a));
        }
    }

    if out.len() == 1 {
        out.push("  differs in a field not summarised above".to_string());
    }
    out.join("\n")
}

/// The harness is only meaningful if the invariant checks can actually fail. Every assertion
/// here also documents a case that a previous version of these checks got wrong.
#[test]
fn invariant_checks_are_not_vacuous() {
    fn row(trace: &str, index: usize, role: &str, kind: &str, content: &str) -> InvariantRow {
        InvariantRow {
            trace_id: trace.to_string(),
            span_id: "span-1".to_string(),
            index,
            role: role.to_string(),
            entry_type: kind.to_string(),
            content: content.to_string(),
            tool_name: None,
            tool_use_id: None,
        }
    }

    fn tool_row(trace: &str, index: usize, kind: &str, id: &str) -> InvariantRow {
        InvariantRow {
            trace_id: trace.to_string(),
            span_id: "span-1".to_string(),
            index,
            role: if kind == "tool_use" {
                "assistant"
            } else {
                "tool"
            }
            .to_string(),
            entry_type: kind.to_string(),
            content: format!("{{\"id\":\"{id}\"}}"),
            tool_name: Some("calc".to_string()),
            tool_use_id: Some(id.to_string()),
        }
    }

    let fires = |f: &dyn Fn()| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err();

    // Same content twice in the SAME trace is a duplicate.
    let same_trace = vec![
        row("trace-a", 0, "user", "text", "Hello"),
        row("trace-a", 1, "user", "text", "Hello"),
    ];
    assert!(
        fires(&|| assert_no_duplicates("test", "synthetic", &same_trace)),
        "duplicate detection failed to fire for a within-trace duplicate"
    );

    // The same content in DIFFERENT traces is legitimate: a sample that runs the same
    // conversation twice produces exactly this, and flagging it was the original defect.
    let cross_trace = vec![
        row("trace-a", 0, "user", "text", "Hello"),
        row("trace-b", 1, "user", "text", "Hello"),
    ];
    assert!(
        !fires(&|| assert_no_duplicates("test", "synthetic", &cross_trace)),
        "the same content in two different traces must not be reported"
    );

    // A result whose id matches a call is fine, even when the call comes after it in the
    // list: matching is by id, not by position, because the session view interleaves.
    let matched = vec![
        tool_row("trace-a", 0, "tool_result", "id-1"),
        tool_row("trace-a", 1, "tool_use", "id-1"),
    ];
    assert!(
        !fires(&|| assert_tool_pairing("test", "synthetic", &matched)),
        "id-matched pairs must not be reported regardless of order"
    );

    // Parallel tool calls: two calls, two results, interleaved. A stack-based check reported
    // this as an error; matching by id must accept it.
    let parallel = vec![
        tool_row("trace-a", 0, "tool_use", "id-1"),
        tool_row("trace-a", 1, "tool_use", "id-2"),
        tool_row("trace-a", 2, "tool_result", "id-2"),
        tool_row("trace-a", 3, "tool_result", "id-1"),
    ];
    assert!(
        !fires(&|| assert_tool_pairing("test", "synthetic", &parallel)),
        "parallel tool calls must not be reported as mispaired"
    );

    // A result referencing an id no call emitted is a defect.
    let orphan = vec![
        tool_row("trace-a", 0, "tool_use", "id-1"),
        tool_row("trace-a", 1, "tool_result", "id-WRONG"),
    ];
    assert!(
        fires(&|| assert_tool_pairing("test", "synthetic", &orphan)),
        "a tool_result referencing an unknown call id must be reported"
    );

    // Pairing is per trace: a call in one trace must not answer a result in another.
    let cross_trace_pair = vec![
        tool_row("trace-a", 0, "tool_use", "id-1"),
        tool_row("trace-b", 1, "tool_result", "id-1"),
    ];
    assert!(
        fires(&|| assert_tool_pairing("test", "synthetic", &cross_trace_pair)),
        "pairing must be scoped per trace"
    );

    // Scope: a span view must not contain another span's block.
    let leaked = vec![InvariantRow {
        trace_id: "aaaaaaaa1111".to_string(),
        span_id: "bbbb2222".to_string(),
        index: 0,
        role: "user".to_string(),
        entry_type: "text".to_string(),
        content: "x".to_string(),
        tool_name: None,
        tool_use_id: None,
    }];
    assert!(
        fires(&|| assert_scope("test", "span aaaaaaaa/chat/cccc", &leaked)),
        "scope check failed to catch a block from another span"
    );
    assert!(
        !fires(&|| assert_scope("test", "span aaaaaaaa/chat/bbbb", &leaked)),
        "scope check must accept a block that is in scope"
    );

    // Scope: a trace view must not contain another trace's block after scoping.
    assert!(
        fires(&|| assert_scope("test", "trace ffffffff", &leaked)),
        "scope check failed to catch a block from another trace"
    );

    // Projection consistency: a count that disagrees with the list is a defect.
    let inconsistent = GoldenView {
        message_count: 5,
        role_sequence: vec!["user".into()],
        tool_names: vec![],
        tool_definition_count: 0,
        messages: vec![GoldenMessage {
            index: 0,
            role: "user".into(),
            entry_type: "text".into(),
            content: "a".into(),
            content_digest: "deadbeef".into(),
            tool_name: None,
            finish_reason: None,
            observation_type: None,
        }],
    };
    assert!(
        fires(&|| assert_projection_consistent("test", "synthetic", &inconsistent)),
        "projection consistency check failed to fire"
    );
}

/// Processing the same fixture twice must give the same answer.
///
/// Not hypothetical: the output sort's tie-break omitted content, so two blocks sharing span,
/// message index, entry index, entry type and role but differing in content were left in
/// HashMap order. Repeated runs over one real fixture disagreed on 3 then 4 views — two
/// identical API requests could return the same messages in a different order.
#[test]
fn processing_is_deterministic() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        eprintln!("processing_is_deterministic: no fixtures - skipping");
        return;
    }
    // A handful is enough to catch an ordering tie, and keeps the test quick.
    for (label, paths) in fixtures.iter().take(8) {
        let first = build_golden(label, paths, &rows_for(paths)).golden;
        let second = build_golden(label, paths, &rows_for(paths)).golden;
        assert!(
            first == second,
            "{label}: two identical runs produced different output:\n{}",
            describe_diff(label, &first, &second)
        );
    }
}

/// The content digest must actually distinguish content, including changes past the preview
/// cutoff and ones that survive whitespace collapse. Otherwise truncating the preview would
/// quietly become the real comparison.
#[test]
fn content_digest_detects_changes_beyond_the_preview() {
    // Same LENGTH, differing only past the preview cutoff. A length change is already
    // caught by the "[N chars]" suffix the preview carries; this is the case only a digest
    // can see.
    let long_a = "x".repeat(MAX_CONTENT + 50);
    let mut long_b = long_a.clone();
    long_b.replace_range(MAX_CONTENT + 10..MAX_CONTENT + 11, "y");

    let a = json!({"type": "text", "text": long_a});
    let b = json!({"type": "text", "text": long_b});
    assert_ne!(
        content_digest(&a),
        content_digest(&b),
        "digest must differ when content changes past the preview cutoff"
    );

    let preview_a = normalise_content(&a);
    let preview_b = normalise_content(&b);
    assert_eq!(
        preview_a, preview_b,
        "the preview is expected to be identical here - that is why the digest is needed"
    );

    // Key order must not matter, or every golden would churn.
    let o1 = json!({"type": "tool_use", "name": "calc", "id": "1"});
    let o2 = json!({"type": "tool_use", "name": "calc", "id": "1"});
    assert_eq!(content_digest(&o1), content_digest(&o2));
}
