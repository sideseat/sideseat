//! SideML Feed Pipeline
//!
//! Reconstructs conversation timelines from OTEL spans that may contain
//! duplicated messages (history duplication) from multiple AI frameworks.
//!
//! # The Problem
//!
//! OTEL traces often contain duplicate messages because:
//! - **Event-based frameworks** (Strands): Child spans re-emit parent events
//! - **Attribute-based frameworks** (LangGraph): Message arrays accumulate history
//! - **Session history**: Previous turns passed as context to new LLM calls
//! - **Tool chains**: ToolUse → Tool execution → ToolResult need logical ordering
//!
//! # The Solution
//!
//! ## Output Classification
//!
//! First, classify each block as OUTPUT or INPUT:
//! - **OUTPUT**: LLM responses that should NEVER be marked as history
//!   - `gen_ai.choice` events (always output, regardless of span type)
//!   - Assistant text/thinking blocks
//!   - ToolUse from generation spans (LLM decided to call tool)
//! - **INPUT**: Everything else (user messages, system, tool results, history)
//!
//! ## Eight-Phase History Detection
//!
//! See `history.rs` for the full algorithm. Key phases:
//! 0. **Output Protection**: OUTPUT blocks are NEVER marked as history
//! 2. **Timestamp-based**: Message timestamp < span start → historical context
//! 3. **Accumulator span input**: Input events from non-root accumulator spans
//! 4. **Intermediate text**: Assistant text from generation spans (event-based frameworks)
//!    - **(4b) Input-source assistant**: Assistant from input attrs in non-root gen spans
//! 5. **Multi-turn history**: All unprotected content in generation spans with tool_results
//! 6. **Orphan tool_results**: Tool_results with unknown tool_use_id
//! 7. **Deduplication**: Later occurrences of same content within trace
//!
//! ## Content-Based Identity (mostly not ID-based)
//!
//! - Tool calls: `hash(name + input)` — call_id ignored (regenerated in history)
//! - Tool results: `tool_use_id` when present, `hash(content)` otherwise. Correlation (below)
//!   supplies the id for frameworks that omit it, so this is the usual case rather than the
//!   fallback.
//! - Regular: `hash(trace_id + role + content)`
//! - Structured JSON answers: members with no value are dropped before hashing, so a
//!   schema-filled object and the model's raw one are one answer. Tool inputs and results keep
//!   the distinction — an empty collection there is an answer.
//!
//! ## Quality Scoring
//!
//! Picks best version when deduplicating:
//! - Non-history (+100), finish_reason (+10), enrichment (+5), output-source (+4),
//!   tool-span (+3), event source (+2), model info (+1)
//!
//! # Pipeline Stages
//!
//! ```text
//! 1. PARSE       Vec<MessageSpanRow> → SideML messages
//! 2. FLATTEN     One ContentBlock per BlockEntry with all metadata; never filtered
//! 3. CORRELATE   id-less tool results adopt their call's id (see correlate.rs)
//! 4. CLASSIFY    Determine uses_span_end for each block
//! 5. MARK HISTORY Eight-phase detection (see history.rs)
//! 6. DEDUP       Identity-based, keep highest quality version
//! 7. WITHDRAW    Clear a correlated id whose call did not survive dedup
//! 8. SORT        (birth_time, message_index, entry_index)
//! 9. ROLE FILTER `?role=` applied here, to the finished feed, on each block's derived role
//! 10. RETURN     FeedResult with blocks, tool_definitions, metadata
//! ```
//!
//! Stages 3 and 5-6 all decide what counts as the same tool result, and all three need the call
//! reference, which is why correlation precedes them.
//!
//! ## Known limit: identical repeats within one trace
//!
//! Two tool calls with the same name and arguments, or two messages with the same role and text,
//! are treated as one within a trace. That is not incidental - a framework re-sending its history
//! is indistinguishable from a genuine repeat once content is all there is, and re-sends are what
//! this pipeline exists to collapse. Telling them apart would need a per-call id that survives
//! re-sending, which no framework in the fixture suite provides. So a conversation that really
//! ran the same tool twice with the same arguments shows it once.
//!
//! # Framework Compatibility
//!
//! Works for all frameworks without special cases:
//! - **With history**: Strands, LangGraph, LangChain (duplicates detected/filtered)
//! - **Without history**: AutoGen, CrewAI (passes through unchanged)

mod classify;
mod correlate;
mod dedup;
mod history;
#[cfg(test)]
mod props;
mod types;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::{Value as JsonValue, json};

use super::normalize::to_sideml_with_context;
use super::tools::{extract_tool_name, normalize_tools, tool_definition_quality};
use super::types::ContentBlock;
use crate::data::types::{MessageCategory, MessageSpanRow};
use crate::domain::traces::{MessageSource, RawMessage};

use classify::uses_span_end;
use dedup::{
    FeedPosition, SpanTimestamps, feed_positions, normalize_json_for_hash,
    normalize_structured_json_for_hash, normalize_tool_result_content, process_dedup,
};
use history::mark_history;

// Re-exports for public API
pub use types::{BlockEntry, ExtractedTools, FeedMetadata, FeedOptions, FeedResult};

// ============================================================================
// SHARED CONSTANTS
// ============================================================================

/// Observation type values (used for span classification).
pub(crate) mod obs_type {
    pub const GENERATION: &str = "generation";
    pub const TOOL: &str = "tool";
    pub const AGENT: &str = "agent";
    pub const SPAN: &str = "span";
    pub const CHAIN: &str = "chain";
}

/// Source type values (event vs attribute).
pub(crate) mod source_type {
    pub const EVENT: &str = "event";
    pub const ATTRIBUTE: &str = "attribute";
}

/// Status code values.
pub(crate) mod status {
    pub const ERROR: &str = "ERROR";
}

/// GenAI output event names (OpenTelemetry semantic conventions).
/// These represent completion events that should use span_end timestamp.
///
/// `gen_ai.output.messages` is the bundled form the current conventions use, carried on the
/// `gen_ai.client.inference.operation.details` event. Without it here, a bundled output was not
/// recognised as output at all: it did not take the span-end timestamp, it was not protected from
/// history marking, and it shared a response with the input event emitted at the same instant - so
/// it reported the input's time, which is the defect the direction-keyed batching fixes for
/// attribute sources.
pub(crate) const GENAI_OUTPUT_EVENTS: &[&str] = &[
    "gen_ai.choice",
    "gen_ai.content.completion",
    "gen_ai.output.messages",
];

/// GenAI input event names (OpenTelemetry semantic conventions).
/// These represent context/input that may be history copies.
pub(crate) const GENAI_INPUT_EVENTS: &[&str] = &[
    "gen_ai.user.message",
    "gen_ai.assistant.message",
    "gen_ai.system.message",
    "gen_ai.tool.message",
    "gen_ai.content.prompt",
    // The bundled form, paired with gen_ai.output.messages above.
    "gen_ai.input.messages",
];

// ============================================================================
// INTERMEDIATE TYPE FOR PARSING
// ============================================================================

/// Intermediate message after parsing, before flattening.
#[derive(Debug, Clone)]
struct ParsedMessage {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    session_id: Option<String>,
    message_index: i32,
    timestamp: DateTime<Utc>,
    source: MessageSource,
    message: super::types::ChatMessage,
    category: MessageCategory,
    model: Option<String>,
    provider: Option<String>,
    status_code: Option<String>,
    total_tokens: i64,
    cost_total: f64,
    observation_type: Option<String>,
}

/// Incremental cross-trace prefix state for replay stripping.
///
/// Stores an ordered prefix plus an index for O(log n) next-position lookup.
#[derive(Debug, Default)]
struct CrossTracePrefixState {
    len: usize,
    positions_by_role: HashMap<super::types::ChatRole, HashMap<String, Vec<usize>>>,
}

impl CrossTracePrefixState {
    #[inline]
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn len(&self) -> usize {
        self.len
    }

    /// Add a block to accumulated cross-trace prefix history.
    fn push_block(&mut self, block: &BlockEntry) {
        let idx = self.len;
        self.len += 1;
        self.positions_by_role
            .entry(block.role)
            .or_default()
            .entry(block.content_hash.clone())
            .or_default()
            .push(idx);
    }

    /// Find first accumulated position >= `min_index` for `(role, content_hash)`.
    fn next_position(
        &self,
        role: super::types::ChatRole,
        content_hash: &str,
        min_index: usize,
    ) -> Option<usize> {
        let positions = self.positions_by_role.get(&role)?.get(content_hash)?;
        let rel = positions.partition_point(|&idx| idx < min_index);
        positions.get(rel).copied()
    }
}

// ============================================================================
// PUBLIC API
// ============================================================================

/// Process span rows through the complete feed pipeline.
///
/// Routes to `process_trace_spans` for single-trace data, or
/// `process_multi_trace_spans` for multi-trace data (cross-trace prefix stripping).
pub fn process_spans(rows: Vec<MessageSpanRow>, options: &FeedOptions) -> FeedResult {
    apply_role_filter(process_spans_unfiltered(rows), options.role.as_deref())
}

/// [`process_spans`] without the role filter, for callers that filter once at their own boundary.
fn process_spans_unfiltered(rows: Vec<MessageSpanRow>) -> FeedResult {
    // Detect multi-trace: if all rows share the same trace_id, single-trace path
    let is_multi_trace = rows.len() > 1
        && rows
            .first()
            .map(|first| rows.iter().any(|r| r.trace_id != first.trace_id))
            .unwrap_or(false);

    if is_multi_trace {
        process_multi_trace_spans(rows)
    } else {
        process_trace_spans_core(rows, None)
    }
}

/// Process span rows from a single trace through the complete feed pipeline.
///
/// This is the core pipeline for processing raw message data from the database.
/// Raw messages are converted to SideML at query time, then flattened to blocks.
///
/// # Pipeline
///
/// 1. Parse raw messages from JSON and convert to SideML
/// 2. Flatten to individual content blocks with metadata
/// 3. Deduplicate by identity (collapse history to first occurrence)
/// 4. Sort by birth time + semantic order
/// 5. Return FeedResult with blocks, tool definitions, and metadata
pub fn process_trace_spans(rows: Vec<MessageSpanRow>, options: &FeedOptions) -> FeedResult {
    apply_role_filter(
        process_trace_spans_core(rows, None),
        options.role.as_deref(),
    )
}

/// Core pipeline with optional cross-trace prefix marking.
///
/// When `cross_trace_prefix` is provided, input-source blocks matching the
/// accumulated prefix from previous traces are marked as history BEFORE dedup.
/// This allows within-trace dedup to correctly preserve genuine repeated content
/// (the non-history copy wins via +100 quality bonus) while stripping the
/// history re-send copy.
fn process_trace_spans_core(
    rows: Vec<MessageSpanRow>,
    cross_trace_prefix: Option<&CrossTracePrefixState>,
) -> FeedResult {
    // Build span hierarchy for span_path computation
    let span_hierarchy = build_span_hierarchy(&rows);

    // Build span timestamps map for birth time computation
    let span_timestamps = build_span_timestamps(&rows);

    // Stage 1: Parse raw messages and convert to SideML
    let mut parsed_messages = parse_span_rows(&rows);

    // Extract tools from all rows
    let extracted_tools = extract_tools_from_rows(&rows);

    // Stage 1b: Append error messages from leaf error spans
    append_error_messages(&mut parsed_messages, &rows);

    // Debug: Log parsed message counts by role
    if tracing::enabled!(tracing::Level::DEBUG) {
        let msg_count_by_role: HashMap<_, usize> = parsed_messages
            .iter()
            .map(|m| m.message.role)
            .fold(HashMap::new(), |mut acc, role| {
                *acc.entry(role).or_insert(0) += 1;
                acc
            });
        tracing::trace!(
            total = parsed_messages.len(),
            by_role = ?msg_count_by_role,
            "Feed: after parse_span_rows"
        );
    }

    // Stage 2: Flatten to individual blocks with metadata
    // All blocks start with is_history = false
    let mut blocks = flatten_to_blocks(parsed_messages, &span_hierarchy);

    // Stage 2.5: Cross-trace prefix marking (multi-trace sessions only)
    // MUST run BEFORE classify_blocks (which includes Phase 7 duplicate detection).
    // If run after, Phase 7 would mark the second occurrence as history, then
    // cross-trace would mark the first → both become history → genuine content lost.
    // Running before ensures Phase 7 sees the first copy as already-history and
    // skips it, preserving the genuine (second) copy.
    if let Some(prefix) = cross_trace_prefix {
        mark_cross_trace_prefix(&mut blocks, prefix);
    }

    // Stage 2.6: Correlate tool results to their calls.
    //
    // Runs BEFORE classification and dedup, because both decide what is a duplicate tool result
    // and both need the call reference to do it. Two results with the same text are either one
    // call re-sent or two different calls, and only the call tells them apart - so a result that
    // reaches either stage without its call's id has both of them fall back to text, and a
    // genuine second result is dropped from the feed. Correlation needs the blocks in source
    // order, which is what they are in right after flattening.
    correlate::correlate_tool_results(&mut blocks);

    // Stages 3-4: Classify blocks and mark history
    // - uses_span_end: determines timestamp strategy (span_end vs event_time)
    // - is_history: marks non-authoritative blocks for filtering
    classify_blocks(&mut blocks, &span_timestamps);

    // Debug: Log block counts by entry_type after flatten
    if tracing::enabled!(tracing::Level::DEBUG) {
        let block_count_by_type: HashMap<_, usize> = blocks
            .iter()
            .map(|b| b.entry_type.as_str())
            .fold(HashMap::new(), |mut acc, t| {
                *acc.entry(t).or_insert(0) += 1;
                acc
            });
        let history_count = blocks.iter().filter(|b| b.is_history).count();
        tracing::trace!(
            total = blocks.len(),
            by_type = ?block_count_by_type,
            history_count,
            "Feed: after flatten_to_blocks"
        );
    }

    // Stages 5-6: Deduplicate by identity, sort by birth time
    let blocks = process_dedup(blocks, span_timestamps);

    // Stage 6.5: Withdraw a correlated id whose call did not survive.
    //
    // Correlation only ever links to a call in the same block list, but dedup and history
    // marking can drop that call afterwards - leaving the result pointing at something the
    // response does not contain. Clearing the id restores "honestly uncorrelated"; keeping the
    // block, because the result's content is real either way. Only correlated ids are withdrawn:
    // a provider's own id may legitimately reference a call outside the requested scope.
    let blocks = correlate::withdraw_unbacked_ids(blocks);

    // Debug: Log block counts after dedup
    if tracing::enabled!(tracing::Level::DEBUG) {
        let dedup_count_by_type: HashMap<_, usize> = blocks
            .iter()
            .map(|b| b.entry_type.as_str())
            .fold(HashMap::new(), |mut acc, t| {
                *acc.entry(t).or_insert(0) += 1;
                acc
            });
        tracing::trace!(
            total = blocks.len(),
            by_type = ?dedup_count_by_type,
            "Feed: after process_dedup"
        );
    }

    // Stage 7: Compute metadata and return
    let metadata = compute_metadata(&blocks, &rows);

    FeedResult {
        messages: blocks,
        tool_definitions: extracted_tools.tool_definitions,
        tool_names: extracted_tools.tool_names,
        metadata,
    }
}

/// Process spans from multiple traces with cross-trace prefix marking.
///
/// Groups rows by trace_id, sorts traces chronologically, processes each through
/// the within-trace pipeline with accumulated prefix entries from prior traces.
/// The prefix marking happens BEFORE within-trace dedup (in `process_trace_spans_core`),
/// so genuine repeated content (same content as prior trace) is preserved: the history
/// re-send copy is marked as `is_history`, while the genuine copy stays non-history
/// and wins dedup via +100 quality bonus.
///
/// # Accumulated Prefix
///
/// All non-System blocks are accumulated as `(role, content_hash)` entries.
/// Role-aware matching prevents cross-role false matches when content repeats.
/// The prefix scan handles both:
/// - **Root gen spans**: No Phase 4b, all input-source blocks (including assistant)
///   are matched directly against accumulated.
/// - **Non-root gen spans**: Phase 4b marks assistant input-source blocks as history.
///   Prefix scan consumes matched Phase 4b entries without re-marking.
fn process_multi_trace_spans(rows: Vec<MessageSpanRow>) -> FeedResult {
    let trace_groups = group_and_sort_traces(rows);

    let mut accumulated = CrossTracePrefixState::default();
    let mut all_blocks: Vec<BlockEntry> = Vec::new();
    let mut all_tool_defs: Vec<serde_json::Value> = Vec::new();
    let mut all_tool_names: Vec<String> = Vec::new();
    let mut total_tokens: i64 = 0;
    let mut total_cost: f64 = 0.0;

    for (trace_idx, trace_rows) in trace_groups.into_iter().enumerate() {
        // Once per span, not once per row, for the same reason `compute_metadata` does it: a
        // re-ingested span is two rows in the DuckDB row set (that query reads the raw table,
        // ClickHouse reads it with FINAL), and summing rows billed the retry as a second call.
        // The session view was the last place still summing rows, so a session and the traces
        // inside it disagreed about their totals whenever a delivery had been retried.
        let mut counted: HashSet<(&str, &str)> = HashSet::new();
        let mut trace_tokens = 0i64;
        let mut trace_cost = 0.0f64;
        for row in &trace_rows {
            if counted.insert((row.trace_id.as_str(), row.span_id.as_str())) {
                trace_tokens += row.total_tokens;
                trace_cost += row.cost_total;
            }
        }

        // First trace: no prefix. Subsequent traces: pass accumulated prefix
        // for pre-dedup marking of history re-sends.
        let cross_trace_prefix = if trace_idx == 0 {
            None
        } else {
            Some(&accumulated)
        };

        let result = process_trace_spans_core(trace_rows, cross_trace_prefix);

        // First trace always contributes. Subsequent traces contribute only if
        // they have new non-system content (pure replay traces are skipped).
        let has_new_content = trace_idx == 0
            || result
                .messages
                .iter()
                .any(|b| b.role != super::types::ChatRole::System);

        if has_new_content {
            // Accumulate role-aware prefix entries from all non-System blocks.
            // The prefix scan matches these against input-source blocks in
            // subsequent traces, handling both root gen spans (where assistant
            // blocks survive) and non-root gen spans (where Phase 4b marks them).
            for block in &result.messages {
                if block.role != super::types::ChatRole::System {
                    accumulated.push_block(block);
                }
            }
            all_blocks.extend(result.messages);
            all_tool_defs.extend(result.tool_definitions);
            all_tool_names.extend(result.tool_names);
        }

        // Counted whether or not the trace contributed a message. Cost is what the spans in scope
        // were billed, not what survived history removal: a trace that only re-sent an earlier turn
        // still called the model, and skipping it reported a session as cheaper than it was.
        total_tokens += trace_tokens;
        total_cost += trace_cost;
    }

    let block_count = all_blocks.len();
    let span_count = all_blocks
        .iter()
        .map(|b| (&b.trace_id, &b.span_id))
        .collect::<HashSet<_>>()
        .len();
    let tool_definitions = deduplicate_tools(all_tool_defs);
    let tool_names = deduplicate_names(all_tool_names);

    FeedResult {
        messages: all_blocks,
        tool_definitions,
        tool_names,
        metadata: FeedMetadata {
            block_count,
            span_count,
            total_tokens,
            total_cost,
        },
    }
}

/// Mark input-source blocks matching the accumulated cross-trace prefix as history.
///
/// Runs BEFORE `classify_blocks` (before Phase 4b and Phase 7) so that:
/// - Phase 7 (duplicate detection) sees the marked copies as history and skips them,
///   preserving the genuine copy when content repeats.
/// - Phase 4b and other history phases layer on top correctly.
///
/// # Algorithm
///
/// 1. **Guard**: If there are no attribute-sourced input blocks, skip.
///    Event-based frameworks (Strands) should remain independent across traces.
/// 2. **Per-span sequential scan**: For each span, iterate input-source blocks
///    in order, matching against accumulated prefix entries. Mark matches as
///    history. Stop at first non-match for that span.
fn mark_cross_trace_prefix(blocks: &mut [BlockEntry], accumulated: &CrossTracePrefixState) {
    if accumulated.is_empty() {
        return;
    }

    // A block is "cross-trace strippable" if it represents history re-sent to a new LLM call:
    // - Attribute-sourced input (LangGraph, ADK, Vercel, etc.)
    // - gen_ai.input.messages event (Strands JS bundled format: all messages share event_time
    //   so timestamp-based Phase 2 can't detect history within a single span)
    // Pure per-message event frameworks (Strands Python: gen_ai.user.message etc.) are excluded
    // because they preserve original timestamps and Phase 2 handles them within each trace.
    let is_strippable = |b: &BlockEntry| {
        (b.is_input_source() && b.source_type == source_type::ATTRIBUTE)
            || b.event_name.as_deref() == Some("gen_ai.input.messages")
    };

    let input_source_count = blocks.iter().filter(|b| b.is_input_source()).count();
    let strippable_input_count = blocks.iter().filter(|b| is_strippable(b)).count();
    if strippable_input_count == 0 {
        return;
    }

    // Sequential prefix match per span on strippable input blocks.
    // Since this runs before any history marking, no blocks are is_history yet.
    let mut acc_idx = 0;
    let mut current_span_id: Option<String> = None;
    let mut span_prefix_active = true;
    let mut marked = 0;
    let mut skipped = 0;
    let mut spans_scanned = 0;
    for block in blocks.iter_mut() {
        // Prefix scan resets at each span boundary. ADK/LangGraph often replay
        // history at the start of every generation span, not just trace start.
        if current_span_id.as_deref() != Some(block.span_id.as_str()) {
            current_span_id = Some(block.span_id.clone());
            acc_idx = 0;
            span_prefix_active = true;
            spans_scanned += 1;
        }
        if acc_idx >= accumulated.len() || !span_prefix_active {
            continue;
        }
        let strippable = (block.is_input_source() && block.source_type == source_type::ATTRIBUTE)
            || block.event_name.as_deref() == Some("gen_ai.input.messages");
        if !strippable {
            continue;
        }
        // System prompts are per-trace framing, not semantic history prefix.
        // Treat them as transparent so leading system blocks don't break
        // cross-trace prefix matching for subsequent user/tool content.
        if block.role == super::types::ChatRole::System {
            continue;
        }

        // Match against accumulated as an ordered subsequence (not strict
        // adjacency). Prior traces can contain extra output-only blocks that
        // are not replayed in the next trace's input prefix.
        if let Some(next_idx) = accumulated.next_position(block.role, &block.content_hash, acc_idx)
        {
            block.is_history = true;
            skipped += next_idx.saturating_sub(acc_idx);
            acc_idx = next_idx + 1;
            marked += 1;
        } else {
            span_prefix_active = false; // Prefix ends for this span
        }
    }

    tracing::debug!(
        accumulated_len = accumulated.len(),
        input_source_count,
        strippable_input_count,
        spans_scanned,
        marked,
        skipped,
        "cross-trace prefix marking complete"
    );
}

/// Group rows by trace_id and sort trace groups chronologically.
///
/// Sort key: (min span_timestamp, min ingested_at, first_seen_row_index, trace_id).
/// The first-seen index preserves caller/query order when timestamps tie, which
/// keeps cross-trace prefix stripping stable for same-timestamp traces.
fn group_and_sort_traces(rows: Vec<MessageSpanRow>) -> Vec<Vec<MessageSpanRow>> {
    let mut by_trace: HashMap<String, (usize, Vec<MessageSpanRow>)> = HashMap::new();
    for (row_index, row) in rows.into_iter().enumerate() {
        let entry = by_trace
            .entry(row.trace_id.clone())
            .or_insert_with(|| (row_index, Vec::new()));
        entry.1.push(row);
    }

    let mut trace_groups: Vec<_> = by_trace
        .into_iter()
        .map(|(trace_id, (first_seen_index, rows))| {
            let min_ts = rows.iter().map(|r| r.span_timestamp).min().unwrap();
            let min_ingest = rows.iter().map(|r| r.ingested_at).min().unwrap();
            (trace_id, min_ts, min_ingest, first_seen_index, rows)
        })
        .collect();

    trace_groups.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.cmp(&b.3))
            .then_with(|| a.0.cmp(&b.0))
    });

    trace_groups
        .into_iter()
        .map(|(_, _, _, _, rows)| rows)
        .collect()
}

/// Process spans from multiple conversations for a feed.
///
/// Groups spans by conversation boundary (session_id or trace_id),
/// processes each conversation separately, then merges results.
pub fn process_feed(rows: Vec<MessageSpanRow>, options: &FeedOptions) -> FeedResult {
    // A trace's session is resolved from any of its rows that names one, then applied to all of
    // them.
    //
    // Reading each row's own session id split a conversation in half whenever the id is recorded on
    // the root span only, which is how several frameworks record it: the root went to the session
    // group and its children to a trace group, so history detection ran on the two halves
    // separately and had nothing to recognise a re-send against.
    let mut session_of_trace: HashMap<&str, &str> = HashMap::new();
    for row in &rows {
        if let Some(session) = row.session_id.as_deref().filter(|s| !s.is_empty()) {
            session_of_trace.entry(&row.trace_id).or_insert(session);
        }
    }
    let conversation_of_trace: HashMap<String, String> = rows
        .iter()
        .map(|row| {
            let key = session_of_trace
                .get(row.trace_id.as_str())
                .map(|session| (*session).to_string())
                .unwrap_or_else(|| format!("trace:{}", row.trace_id));
            (row.trace_id.clone(), key)
        })
        .collect();

    let mut spans_by_conversation: HashMap<String, Vec<MessageSpanRow>> = HashMap::new();
    for row in rows {
        let key = conversation_of_trace
            .get(&row.trace_id)
            .cloned()
            .unwrap_or_else(|| format!("trace:{}", row.trace_id));
        spans_by_conversation.entry(key).or_default().push(row);
    }

    // Process each conversation separately
    let mut all_blocks: Vec<BlockEntry> = Vec::new();
    let mut all_tool_defs: Vec<JsonValue> = Vec::new();
    let mut all_tool_names: Vec<String> = Vec::new();
    let mut total_tokens: i64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut span_ids: HashSet<(String, String)> = HashSet::new();

    for (_, conversation_spans) in spans_by_conversation {
        for row in &conversation_spans {
            // Once per span: a re-ingested span appears twice in the DuckDB row set, and summing
            // rows doubled the page's tokens and cost.
            if span_ids.insert((row.trace_id.clone(), row.span_id.clone())) {
                total_tokens += row.total_tokens;
                total_cost += row.cost_total;
            }
        }
        let processed = process_spans_unfiltered(conversation_spans);
        all_blocks.extend(processed.messages);
        all_tool_defs.extend(processed.tool_definitions);
        all_tool_names.extend(processed.tool_names);
    }

    // Sorted newest-first by an explicit key, for the same reason process_dedup is: a comparator
    // with a same-batch special case is not a total order. Here the batch was keyed on the
    // timestamp it also ordered by, so no cycle was constructible - but two blocks in different
    // traces sharing a span id and a timestamp compared *equal*, which let their position depend on
    // the order conversations came out of a HashMap.
    //
    // Time descending, then the response, then position within it.
    //
    // Position comes from the same helper the trace views use, so a tool result whose call sits in
    // another span at the same instant follows that call here too. Sorting by the block's own span id
    // instead ordered that tie arbitrarily - the answer could precede the question in the feed while
    // the trace view had it right.
    let positions = feed_positions(&all_blocks, |i| all_blocks[i].timestamp);
    let mut keyed: Vec<(FeedPosition, BlockEntry)> =
        positions.into_iter().zip(all_blocks).collect();
    keyed.sort_by(|(a_pos, a), (b_pos, b)| {
        b.timestamp
            .cmp(&a.timestamp)
            .then_with(|| a_pos.span.cmp(&b_pos.span))
            .then_with(|| a_pos.message_index.cmp(&b_pos.message_index))
            .then_with(|| a_pos.entry_index.cmp(&b_pos.entry_index))
            .then_with(|| a_pos.after_call.cmp(&b_pos.after_call))
            .then_with(|| a.content_hash.cmp(&b.content_hash))
    });
    let all_blocks: Vec<BlockEntry> = keyed.into_iter().map(|(_, block)| block).collect();

    // Deduplicate tools across conversations
    let tool_definitions = deduplicate_tools(all_tool_defs);
    let tool_names = deduplicate_names(all_tool_names);
    let block_count = all_blocks.len();

    apply_role_filter(
        FeedResult {
            messages: all_blocks,
            tool_definitions,
            tool_names,
            metadata: FeedMetadata {
                block_count,
                span_count: span_ids.len(),
                total_tokens,
                total_cost,
            },
        },
        options.role.as_deref(),
    )
}

/// Keep only the blocks inside a requested time window.
///
/// A window is a filter on the answer, not on the input. Applying it to the *rows* - which is what
/// passing it to the message query does for `from` - removes the earlier traces that history
/// detection and cross-trace prefix stripping read, and those stages then have nothing to
/// recognise a re-send against: a later turn's request comes back showing the whole conversation
/// again as new messages. The lower bound therefore belongs here, after the pipeline has seen the
/// context. The upper bound is still applied to the query as well, because everything after it is
/// irrelevant to what came before and there is no reason to load it.
///
/// Compares the timestamps the API returns, and is half-open: `from <= t < to`, as the queries are.
pub fn apply_time_window(
    result: FeedResult,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> FeedResult {
    if from.is_none() && to.is_none() {
        return result;
    }

    let messages: Vec<BlockEntry> = result
        .messages
        .into_iter()
        .filter(|b| from.is_none_or(|from| b.timestamp >= from))
        // Half-open at the top, matching the `timestamp_start < to` the message queries apply:
        // with `<=` here, a message exactly on the bound was returned when its span started
        // earlier and dropped when its span started on the bound too.
        .filter(|b| to.is_none_or(|to| b.timestamp < to))
        .collect();
    let span_count = messages
        .iter()
        .map(|b| (&b.trace_id, &b.span_id))
        .collect::<HashSet<_>>()
        .len();

    FeedResult {
        metadata: FeedMetadata {
            block_count: messages.len(),
            span_count,
            ..result.metadata
        },
        messages,
        ..result
    }
}

/// Keep only blocks whose role matches `role`, if one was requested.
///
/// Applied to the finished feed, never during flattening. The role a block reports is derived
/// from its content, not from the raw message role - a Gemini or ADK tool result arrives inside a
/// `user` message - so the filter has to see the derived role, which is why it once lived in
/// `flatten_to_blocks`. Filtering there removes blocks that later stages read:
///
/// - `role=tool` deletes the assistant `ToolUse` blocks that `correlate_tool_results` uses to give
///   an id-less result its call's id. Without the id, dedup falls back to content identity and
///   collapses two results of two different calls into one.
/// - history detection reads user and system messages to decide what is a re-send, so filtering
///   them away changes which of the *remaining* blocks are marked history.
///
/// The filter is a view over the finished feed, so it is applied to the finished feed. Block and
/// span counts are restated from the blocks that survive, so they describe the response rather
/// than the scope that was scanned. Token and cost totals are left as span-level sums: they are
/// the cost of producing the conversation, which filtering the view does not reduce.
fn apply_role_filter(result: FeedResult, role: Option<&str>) -> FeedResult {
    let Some(role) = role else {
        return result;
    };

    let messages: Vec<BlockEntry> = result
        .messages
        .into_iter()
        .filter(|b| b.role.as_str() == role)
        .collect();
    let span_count = messages
        .iter()
        .map(|b| (&b.trace_id, &b.span_id))
        .collect::<HashSet<_>>()
        .len();

    FeedResult {
        metadata: FeedMetadata {
            block_count: messages.len(),
            span_count,
            ..result.metadata
        },
        messages,
        ..result
    }
}

// ============================================================================
// INTERNAL: PARSING
// ============================================================================

/// Parse span rows into parsed messages.
fn parse_span_rows(rows: &[MessageSpanRow]) -> Vec<ParsedMessage> {
    let mut messages: Vec<ParsedMessage> = Vec::with_capacity(rows.len() * 4);

    for row in rows {
        // Determine if this is a tool execution span
        let is_tool_span = row.observation_type.as_deref() == Some(obs_type::TOOL);

        // Parse raw messages and convert to SideML
        match serde_json::from_str::<Vec<RawMessage>>(&row.messages_json) {
            Ok(raw_msgs) => {
                // Debug: Log raw message count
                tracing::trace!(
                    span_id = %row.span_id,
                    raw_msg_count = raw_msgs.len(),
                    "parse_span_rows: raw messages parsed"
                );
                let sideml_msgs = to_sideml_with_context(&raw_msgs, is_tool_span);
                tracing::trace!(
                    span_id = %row.span_id,
                    sideml_msg_count = sideml_msgs.len(),
                    "parse_span_rows: SideML conversion done"
                );
                for (index, msg) in sideml_msgs.into_iter().enumerate() {
                    let timestamp = msg.timestamp;
                    messages.push(ParsedMessage {
                        trace_id: row.trace_id.clone(),
                        span_id: row.span_id.clone(),
                        parent_span_id: row.parent_span_id.clone(),
                        session_id: row.session_id.clone(),
                        message_index: index as i32,
                        timestamp,
                        source: msg.source,
                        message: msg.sideml,
                        category: msg.category,
                        model: row.model.clone(),
                        provider: row.provider.clone(),
                        status_code: row.status_code.clone(),
                        total_tokens: row.total_tokens,
                        cost_total: row.cost_total,
                        observation_type: row.observation_type.clone(),
                    });
                }
            }
            Err(e) => {
                tracing::debug!(
                    span_id = %row.span_id,
                    error = %e,
                    "Failed to parse messages JSON"
                );
            }
        }
    }

    messages
}

/// Extract tool definitions and names from span rows.
///
/// Standalone function decoupled from message parsing so handlers can
/// scope tool extraction to specific rows (e.g., a single trace).
pub fn extract_tools_from_rows<'a>(
    rows: impl IntoIterator<Item = &'a MessageSpanRow>,
) -> ExtractedTools {
    let mut tool_defs: Vec<JsonValue> = Vec::new();
    let mut tool_names_raw: Vec<String> = Vec::new();

    for row in rows {
        match serde_json::from_str::<Vec<JsonValue>>(&row.tool_definitions_json) {
            Ok(defs) => tool_defs.extend(defs),
            Err(e) => {
                tracing::debug!(
                    span_id = %row.span_id,
                    error = %e,
                    "Failed to parse tool definitions JSON"
                );
            }
        }

        match serde_json::from_str::<Vec<String>>(&row.tool_names_json) {
            Ok(names) => tool_names_raw.extend(names),
            Err(e) => {
                tracing::debug!(
                    span_id = %row.span_id,
                    error = %e,
                    "Failed to parse tool names JSON"
                );
            }
        }
    }

    ExtractedTools {
        tool_definitions: deduplicate_tools(tool_defs),
        tool_names: deduplicate_names(tool_names_raw),
    }
}

/// Compose error display text from structured exception fields.
/// Presentation logic at query time — raw data preserved in DB columns.
fn compose_error_text(
    exception_type: Option<&str>,
    exception_message: Option<&str>,
    exception_stacktrace: Option<&str>,
) -> Option<String> {
    let header = match (exception_type, exception_message) {
        (Some(t), Some(m)) if !t.is_empty() && !m.is_empty() => Some(format!("{t}: {m}")),
        (_, Some(m)) if !m.is_empty() => Some(m.to_string()),
        (Some(t), _) if !t.is_empty() => Some(t.to_string()),
        _ => None,
    };

    let stacktrace = exception_stacktrace.filter(|s| !s.is_empty());

    match (header, stacktrace) {
        (Some(h), Some(st)) => Some(format!("{h}\n\n```\n{st}\n```")),
        (Some(h), None) => Some(h),
        (None, Some(st)) => Some(format!("```\n{st}\n```")),
        (None, None) => None,
    }
}

/// Append error messages from leaf error spans.
///
/// Creates ParsedMessage objects from exception fields of ERROR spans.
/// These flow through flatten_to_blocks -> classify -> dedup naturally.
/// Only leaf error spans get messages (deepest ERROR in hierarchy).
///
/// Leaf detection is scoped by trace_id to prevent cross-trace collisions
/// when process_feed groups multiple traces into one session.
fn append_error_messages(messages: &mut Vec<ParsedMessage>, rows: &[MessageSpanRow]) {
    let spans_with_error_children: HashSet<(&str, &str)> = rows
        .iter()
        .filter(|r| r.status_code.as_deref() == Some(status::ERROR) && r.parent_span_id.is_some())
        .filter_map(|r| {
            r.parent_span_id
                .as_deref()
                .map(|p| (r.trace_id.as_str(), p))
        })
        .collect();

    for row in rows {
        if row.status_code.as_deref() != Some(status::ERROR) {
            continue;
        }
        let error_msg = match compose_error_text(
            row.exception_type.as_deref(),
            row.exception_message.as_deref(),
            row.exception_stacktrace.as_deref(),
        ) {
            Some(m) => m,
            None => continue,
        };
        // Skip non-leaf: this span has an ERROR child within the same trace
        if spans_with_error_children.contains(&(row.trace_id.as_str(), row.span_id.as_str())) {
            continue;
        }

        let timestamp = row.span_end_timestamp.unwrap_or(row.span_timestamp);
        let max_msg_idx = messages
            .iter()
            .filter(|m| m.span_id == row.span_id)
            .map(|m| m.message_index)
            .max()
            .unwrap_or(-1);

        messages.push(ParsedMessage {
            trace_id: row.trace_id.clone(),
            span_id: row.span_id.clone(),
            parent_span_id: row.parent_span_id.clone(),
            session_id: row.session_id.clone(),
            message_index: max_msg_idx + 1,
            timestamp,
            source: MessageSource::Attribute {
                key: "exception".to_string(),
                time: timestamp,
            },
            message: super::types::ChatMessage {
                role: super::types::ChatRole::Assistant,
                content: vec![ContentBlock::Text { text: error_msg }],
                finish_reason: Some(super::types::FinishReason::Error),
                ..Default::default()
            },
            category: MessageCategory::Exception,
            model: row.model.clone(),
            provider: row.provider.clone(),
            status_code: row.status_code.clone(),
            total_tokens: 0,
            cost_total: 0.0,
            observation_type: row.observation_type.clone(),
        });
    }
}

// ============================================================================
// INTERNAL: FLATTENING
// ============================================================================

/// Build span hierarchy map for span_path computation.
///
/// Includes cycle detection to prevent infinite loops from malformed data.
fn build_span_hierarchy(span_rows: &[MessageSpanRow]) -> HashMap<String, Vec<String>> {
    let parent_map: HashMap<_, _> = span_rows
        .iter()
        .filter_map(|s| {
            s.parent_span_id
                .as_ref()
                .map(|p| (s.span_id.clone(), p.clone()))
        })
        .collect();

    let mut paths = HashMap::new();
    let max_depth = span_rows.len().max(256); // Floor for partial views (single-span queries)

    for span in span_rows {
        let mut path = vec![span.span_id.clone()];
        let mut current = span.span_id.clone();
        let mut visited = HashSet::with_capacity(max_depth.min(32));
        visited.insert(current.clone());

        while let Some(parent) = parent_map.get(&current) {
            // Cycle detection: stop if we've seen this parent before
            if !visited.insert(parent.clone()) {
                tracing::warn!(
                    span_id = %span.span_id,
                    cycle_at = %parent,
                    "Cycle detected in span hierarchy, truncating path"
                );
                break;
            }

            // Depth limit: prevent runaway in malformed data
            if path.len() >= max_depth {
                tracing::warn!(
                    span_id = %span.span_id,
                    depth = path.len(),
                    "Span hierarchy depth exceeded limit, truncating path"
                );
                break;
            }

            path.push(parent.clone());
            current = parent.clone();
        }

        path.reverse(); // [root, ..., current]
        paths.insert(span.span_id.clone(), path);
    }

    paths
}

/// Build span timestamps map for birth time computation.
fn build_span_timestamps(span_rows: &[MessageSpanRow]) -> HashMap<String, SpanTimestamps> {
    span_rows
        .iter()
        .map(|row| {
            (
                row.span_id.clone(),
                SpanTimestamps {
                    span_start: row.span_timestamp,
                    span_end: row.span_end_timestamp,
                },
            )
        })
        .collect()
}

/// Derive role from content block type, overriding raw message role when needed.
///
/// This handles provider-specific message formats where tool-related content
/// may come with unexpected roles:
/// - ADK/Gemini: ToolResult in "user" role messages (Gemini protocol)
/// - All: ToolUse should always be "assistant" (LLM decided to call)
///
/// For regular content types (text, image, etc.), the original role is preserved.
fn derive_role_from_content(
    block: &ContentBlock,
    original_role: super::types::ChatRole,
) -> super::types::ChatRole {
    match block {
        // Tool results MUST be "tool" role, regardless of raw message
        // Gemini stores these in user messages, but semantically they're tool outputs
        ContentBlock::ToolResult { .. } => super::types::ChatRole::Tool,
        // Tool calls MUST be "assistant" role (LLM decided to call a tool)
        ContentBlock::ToolUse { .. } => super::types::ChatRole::Assistant,
        // All other content types preserve original role
        _ => original_role,
    }
}

/// Flatten parsed messages into individual content blocks.
///
/// All blocks start with `is_history = false`. History detection is done
/// separately by `mark_history()` based on actual
/// content duplication across spans.
///
/// Deliberately unfiltered: every block the spans contain reaches the later stages, because
/// correlation, history detection and dedup all read blocks they do not return. See
/// [`apply_role_filter`].
fn flatten_to_blocks(
    messages: Vec<ParsedMessage>,
    span_hierarchy: &HashMap<String, Vec<String>>,
) -> Vec<BlockEntry> {
    let mut blocks = Vec::new();

    for msg in messages {
        // Skip empty messages
        if msg.message.content.is_empty() {
            tracing::trace!(
                span_id = %msg.span_id,
                role = ?msg.message.role,
                "flatten_to_blocks: skipping empty message"
            );
            continue;
        }

        // Skip spurious tool input JSON blocks from tool spans
        // These are tool invocation parameters that shouldn't appear as messages.
        // Exception: output.value attributes may contain legitimate structured output.
        let is_tool_span = msg.observation_type.as_deref() == Some(obs_type::TOOL);
        let is_output_attr = matches!(
            &msg.source,
            MessageSource::Attribute { key, .. } if key == "output.value" || key.starts_with("output.")
        );
        if is_tool_span
            && !is_output_attr
            && msg.message.content.len() == 1
            && matches!(msg.message.content.first(), Some(ContentBlock::Json { .. }))
        {
            continue;
        }

        let span_path = span_hierarchy
            .get(&msg.span_id)
            .cloned()
            .unwrap_or_default();

        // Source type, event name, and attribute key
        let (src_type, event_name, source_attribute) = match &msg.source {
            MessageSource::Event { name, .. } => (source_type::EVENT, Some(name.clone()), None),
            MessageSource::Attribute { key, .. } => {
                (source_type::ATTRIBUTE, None, Some(key.clone()))
            }
        };

        // Flatten each content block into its own BlockEntry
        // is_history starts as false; will be set by mark_history()
        for (entry_index, block) in msg.message.content.iter().enumerate() {
            let entry_type = block.block_type().to_string();
            let tool_use_id =
                extract_tool_use_id_from_block(block).or_else(|| msg.message.tool_use_id.clone());
            let tool_name = extract_tool_name_from_block(block);
            let content_hash = compute_block_hash(block);
            let is_semantic = block.is_semantic();

            // Derive role from content type, not raw message role.
            // This is critical for frameworks like ADK/Gemini where:
            // - ToolResult comes in "user" role messages (Gemini protocol)
            // - ToolUse should always be "assistant" (LLM decided to call tool)
            let role = derive_role_from_content(block, msg.message.role);

            blocks.push(BlockEntry {
                entry_type,
                content: block.clone(),
                role,

                trace_id: msg.trace_id.clone(),
                span_id: msg.span_id.clone(),
                session_id: msg.session_id.clone(),
                message_index: msg.message_index,
                entry_index: entry_index as i32,

                parent_span_id: msg.parent_span_id.clone(),
                span_path: span_path.clone(),

                timestamp: msg.timestamp,

                observation_type: msg.observation_type.clone(),

                model: msg.model.clone(),
                provider: msg.provider.clone(),

                name: msg.message.name.clone(),
                finish_reason: msg.message.finish_reason,

                tool_use_id,
                tool_name,

                tokens: Some(msg.total_tokens),
                cost: Some(msg.cost_total),

                status_code: msg.status_code.clone(),
                is_error: msg.status_code.as_deref() == Some(status::ERROR),

                source_type: src_type.to_string(),
                event_name: event_name.clone(),
                source_attribute: source_attribute.clone(),
                category: msg.category,

                content_hash: format!("{:016x}", content_hash),
                is_semantic,
                uses_span_end: false, // Will be set by classify_blocks()
                is_history: false,    // Will be set by classify_blocks()
                tool_use_id_correlated: false, // Will be set by correlate_tool_results()
            });
        }
    }

    blocks
}

// ============================================================================
// BLOCK CLASSIFICATION
// ============================================================================

/// Classify blocks and detect history.
///
/// This function performs two key operations:
///
/// 1. **Timestamp classification** (`uses_span_end`): Determines whether each block
///    uses span_end or event_time for ordering. See `classify` module.
///
/// 2. **History detection** (`is_history`): Marks blocks that should be filtered
///    (context copies, intermediate output, duplicates). See `history` module.
///
/// # Pipeline Position
///
/// This runs after flattening and before dedup/sort:
/// ```text
/// Parse → Flatten → [CLASSIFY] → Dedup → Sort
/// ```
fn classify_blocks(blocks: &mut [BlockEntry], span_timestamps: &HashMap<String, SpanTimestamps>) {
    // Step 1: Classify timestamp strategy for each block
    let mut output_count = 0;
    for block in blocks.iter_mut() {
        block.uses_span_end = uses_span_end(block);
        if block.uses_span_end {
            output_count += 1;
        }
    }

    // Step 1b: Promote assistant messages in choiceless generation spans.
    // Logfire/OpenAI Agents store LLM output as gen_ai.assistant.message (not gen_ai.choice).
    // Without promotion, these sort by array index alongside input events → wrong order.
    // Promoting to uses_span_end + GenAIChoice category fixes ordering and history protection.
    //
    // Check at TRACE level: if any span in the trace has gen_ai.choice, skip promotion
    // for the entire trace. This prevents promoting intermediate assistant text in
    // frameworks like Strands where gen_ai.choice lives in a parent/sibling span.
    let traces_with_choice: HashSet<String> = blocks
        .iter()
        .filter(|b| b.is_output_event())
        .map(|b| b.trace_id.clone())
        .collect();

    let mut promoted = 0;
    for block in blocks.iter_mut() {
        if block.is_generation_span()
            && !block.is_tool_use()
            && !traces_with_choice.contains(&block.trace_id)
            && block.event_name.as_deref() == Some("gen_ai.assistant.message")
        {
            block.uses_span_end = true;
            block.category = MessageCategory::GenAIChoice;
            // Update timestamp to span_end so the block exits the same-batch group
            // (Logfire emits all events at span_start, so without this the sort
            // would preserve array index order instead of using birth_time).
            if let Some(ts) = span_timestamps.get(&block.span_id)
                && let Some(end) = ts.span_end
            {
                block.timestamp = end;
            }
            output_count += 1;
            promoted += 1;
        }
    }

    tracing::trace!(
        total = blocks.len(),
        output_count,
        promoted,
        "timestamp classification complete"
    );

    // Step 2: Detect and mark history blocks
    let stats = mark_history(blocks, span_timestamps);

    tracing::trace!(
        total_history = stats.total_history(),
        "history detection complete"
    );
}

/// Extract tool_use_id from a content block if applicable.
fn extract_tool_use_id_from_block(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::ToolUse { id, .. } => id.clone(),
        ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.clone(),
        _ => None,
    }
}

/// Extract tool name from a content block if applicable.
fn extract_tool_name_from_block(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::ToolUse { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// Hash binary content robustly for deduplication.
///
/// Instead of just the first N bytes (which could miss differences),
/// we hash: length + first chunk + last chunk. This catches:
/// - Different file sizes (length differs)
/// - Different headers (first chunk differs)
/// - Different content/endings (last chunk differs)
#[inline]
fn hash_binary_content<H: std::hash::Hasher>(data: &[u8], hasher: &mut H) {
    use std::hash::Hash;

    const CHUNK_SIZE: usize = 128;

    // Always hash length - different sizes = different content
    data.len().hash(hasher);

    if data.len() <= CHUNK_SIZE * 2 {
        // Small data: hash everything
        data.hash(hasher);
    } else {
        // Large data: hash first + last chunks
        data[..CHUNK_SIZE].hash(hasher);
        data[data.len() - CHUNK_SIZE..].hash(hasher);
    }
}

/// Compute a hash for a content block.
fn compute_block_hash(block: &ContentBlock) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();

    // Hash based on block type and key content
    match block {
        ContentBlock::Text { text } => {
            "text".hash(&mut hasher);
            text.trim().hash(&mut hasher); // Normalize whitespace
        }
        ContentBlock::ToolUse { name, input, .. } => {
            // Hash by name + normalized input only (not id)
            "tool_use".hash(&mut hasher);
            name.hash(&mut hasher);
            normalize_json_for_hash(input).hash(&mut hasher);
        }
        ContentBlock::ToolResult {
            name,
            content,
            is_error,
            ..
        } => {
            // Hash by tool name, error flag and normalized content - not tool_use_id, which a
            // history re-send regenerates.
            //
            // Content alone made every "ok" the same message: two tools both reporting success,
            // or a success and a failure whose text happens to match, collapsed into one wherever
            // this hash is the identity - which is the case for a result with no id.
            "tool_result".hash(&mut hasher);
            name.hash(&mut hasher);
            is_error.hash(&mut hasher);
            normalize_tool_result_content(content).hash(&mut hasher);
        }
        ContentBlock::Thinking { text, .. } => {
            "thinking".hash(&mut hasher);
            text.trim().hash(&mut hasher); // Normalize whitespace
        }
        ContentBlock::RedactedThinking { data } => {
            "redacted_thinking".hash(&mut hasher);
            data.hash(&mut hasher);
        }
        ContentBlock::Image { source, data, .. } => {
            "image".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::Audio { source, data, .. } => {
            "audio".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::Video { source, data, .. } => {
            "video".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::Document {
            source, data, name, ..
        } => {
            "document".hash(&mut hasher);
            source.hash(&mut hasher);
            name.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::File {
            source, data, name, ..
        } => {
            "file".hash(&mut hasher);
            source.hash(&mut hasher);
            name.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::ToolDefinitions { tools, .. } => {
            "tool_definitions".hash(&mut hasher);
            tools.len().hash(&mut hasher);
        }
        ContentBlock::Context { data, context_type } => {
            "context".hash(&mut hasher);
            context_type.hash(&mut hasher);
            normalize_json_for_hash(data).hash(&mut hasher); // Sort keys for consistent hash
        }
        ContentBlock::Refusal { message } => {
            "refusal".hash(&mut hasher);
            message.hash(&mut hasher);
        }
        ContentBlock::Json { data } => {
            "json".hash(&mut hasher);
            // Structured normalization: a schema-filled answer and the model's raw one are the
            // same answer. See normalize_structured_json_for_hash.
            normalize_structured_json_for_hash(data).hash(&mut hasher);
        }
        ContentBlock::Unknown { raw } => {
            "unknown".hash(&mut hasher);
            normalize_json_for_hash(raw).hash(&mut hasher); // Sort keys for consistent hash
        }
    }

    hasher.finish()
}

// ============================================================================
// INTERNAL: METADATA
// ============================================================================

/// Compute metadata from processed blocks.
fn compute_metadata(blocks: &[BlockEntry], span_rows: &[MessageSpanRow]) -> FeedMetadata {
    // Keyed by (trace, span): a span id is unique only within a trace, and a session view holds
    // several traces, so counting by span id alone under-reported the span count.
    let span_ids: HashSet<_> = blocks.iter().map(|b| (&b.trace_id, &b.span_id)).collect();

    // Summed once per span, not once per row. A re-ingested span appears twice in the DuckDB row
    // set - that query reads the raw table, while ClickHouse reads it with FINAL - so summing rows
    // doubled the tokens and cost of a conversation whose spans had been delivered twice, even
    // though the messages themselves are deduplicated and appear once.
    let mut counted: HashSet<(&str, &str)> = HashSet::new();
    let mut total_tokens = 0i64;
    let mut total_cost = 0.0f64;
    for row in span_rows {
        if counted.insert((row.trace_id.as_str(), row.span_id.as_str())) {
            total_tokens += row.total_tokens;
            total_cost += row.cost_total;
        }
    }

    FeedMetadata {
        block_count: blocks.len(),
        span_count: span_ids.len(),
        total_tokens,
        total_cost,
    }
}

// ============================================================================
// INTERNAL: DEDUPLICATION
// ============================================================================

/// Deduplicate tool definitions by name, sort alphabetically.
///
/// Strategy:
/// 1. Normalize provider-specific formats to OpenAI-style tool definitions.
/// 2. Merge definitions with the same name to preserve complementary fields.
/// 3. Use quality score only to choose merge base / break ties.
pub fn deduplicate_tools(raw: Vec<JsonValue>) -> Vec<JsonValue> {
    let mut by_name: HashMap<String, JsonValue> = HashMap::with_capacity(raw.len());

    for def in raw {
        let normalized = normalize_tools(&def);
        let defs = match normalized {
            JsonValue::Array(arr) => arr,
            single => vec![single],
        };

        for tool in defs {
            let canonical = canonicalize_tool_definition(tool);
            if let Some(name) = extract_tool_name(&canonical) {
                by_name
                    .entry(name)
                    .and_modify(|existing| {
                        let merged = merge_tool_definitions(existing.clone(), canonical.clone());
                        *existing = merged;
                    })
                    .or_insert(canonical);
            }
        }
    }

    let mut tools: Vec<(String, JsonValue)> = by_name.into_iter().collect();
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    tools.into_iter().map(|(_, def)| def).collect()
}

fn canonicalize_tool_definition(tool: JsonValue) -> JsonValue {
    if tool.get("function").is_some() {
        return tool;
    }

    let Some(name) = tool.get("name").and_then(|n| n.as_str()) else {
        return tool;
    };
    let mut function = json!({ "name": name });
    if let Some(desc) = tool.get("description") {
        function["description"] = desc.clone();
    }
    if let Some(params) = tool
        .get("parameters")
        .or_else(|| tool.get("input_schema"))
        .or_else(|| tool.get("inputSchema"))
    {
        function["parameters"] = params.clone();
    }

    let mut canonical = json!({
        "type": "function",
        "function": function
    });
    if let Some(strict) = tool.get("strict") {
        canonical["strict"] = strict.clone();
    }
    canonical
}

fn function_map(def: &JsonValue) -> Option<&serde_json::Map<String, JsonValue>> {
    def.get("function")
        .and_then(|f| f.as_object())
        .or_else(|| def.as_object())
}

fn function_map_mut(def: &mut JsonValue) -> Option<&mut serde_json::Map<String, JsonValue>> {
    if def.get("function").and_then(|f| f.as_object()).is_some() {
        return def.get_mut("function").and_then(|f| f.as_object_mut());
    }
    def.as_object_mut()
}

fn is_weak_description(desc: &str) -> bool {
    let d = desc.trim();
    d.is_empty()
        || d.eq_ignore_ascii_case("none")
        || d.eq_ignore_ascii_case("n/a")
        || d.eq_ignore_ascii_case("unknown")
        || d.eq_ignore_ascii_case("no description")
}

fn merge_tool_definitions(a: JsonValue, b: JsonValue) -> JsonValue {
    let qa = tool_definition_quality(&a);
    let qb = tool_definition_quality(&b);

    let (mut primary, secondary) = if qb > qa { (b, a) } else { (a, b) };

    let secondary_func = function_map(&secondary).cloned();
    let Some(secondary_func) = secondary_func else {
        return primary;
    };

    let Some(primary_func) = function_map_mut(&mut primary) else {
        return primary;
    };

    if let Some(secondary_desc) = secondary_func.get("description").and_then(|d| d.as_str()) {
        let primary_desc = primary_func
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if is_weak_description(primary_desc) && !is_weak_description(secondary_desc) {
            primary_func.insert(
                "description".to_string(),
                JsonValue::String(secondary_desc.to_string()),
            );
        }
    }

    if let Some(secondary_params) = secondary_func.get("parameters") {
        match primary_func.get_mut("parameters") {
            Some(primary_params) => merge_json_schema(primary_params, secondary_params),
            None => {
                primary_func.insert("parameters".to_string(), secondary_params.clone());
            }
        }
    }

    if let Some(strict_val) = secondary.get("strict").and_then(|v| v.as_bool())
        && strict_val
    {
        primary["strict"] = JsonValue::Bool(true);
    }

    primary
}

fn merge_json_schema(primary: &mut JsonValue, secondary: &JsonValue) {
    let (Some(primary_obj), Some(secondary_obj)) = (primary.as_object_mut(), secondary.as_object())
    else {
        if primary.is_null() && !secondary.is_null() {
            *primary = secondary.clone();
        }
        return;
    };

    for (key, secondary_val) in secondary_obj {
        match key.as_str() {
            "properties" => merge_properties(primary_obj, secondary_val),
            "required" => merge_required(primary_obj, secondary_val),
            _ => match primary_obj.get_mut(key) {
                Some(primary_val) => {
                    if primary_val.is_null() {
                        *primary_val = secondary_val.clone();
                    } else if primary_val.is_object() && secondary_val.is_object() {
                        merge_json_schema(primary_val, secondary_val);
                    }
                }
                None => {
                    primary_obj.insert(key.clone(), secondary_val.clone());
                }
            },
        }
    }
}

fn merge_properties(
    primary_obj: &mut serde_json::Map<String, JsonValue>,
    secondary_props: &JsonValue,
) {
    let Some(secondary_props_obj) = secondary_props.as_object() else {
        return;
    };

    match primary_obj.get_mut("properties") {
        Some(JsonValue::Object(primary_props_obj)) => {
            for (prop_name, secondary_prop) in secondary_props_obj {
                match primary_props_obj.get_mut(prop_name) {
                    Some(primary_prop) => merge_property_schema(primary_prop, secondary_prop),
                    None => {
                        primary_props_obj.insert(prop_name.clone(), secondary_prop.clone());
                    }
                }
            }
        }
        _ => {
            primary_obj.insert(
                "properties".to_string(),
                JsonValue::Object(secondary_props_obj.clone()),
            );
        }
    }
}

fn merge_property_schema(primary_prop: &mut JsonValue, secondary_prop: &JsonValue) {
    let (Some(primary_obj), Some(secondary_obj)) =
        (primary_prop.as_object_mut(), secondary_prop.as_object())
    else {
        if primary_prop.is_null() && !secondary_prop.is_null() {
            *primary_prop = secondary_prop.clone();
        }
        return;
    };

    for (key, secondary_val) in secondary_obj {
        match primary_obj.get_mut(key) {
            Some(primary_val) => {
                if key == "description" {
                    let current = primary_val.as_str().unwrap_or("");
                    let incoming = secondary_val.as_str().unwrap_or("");
                    if is_weak_description(current) && !is_weak_description(incoming) {
                        *primary_val = JsonValue::String(incoming.to_string());
                    }
                    continue;
                }

                if primary_val.is_null() {
                    *primary_val = secondary_val.clone();
                } else if primary_val.is_object() && secondary_val.is_object() {
                    merge_json_schema(primary_val, secondary_val);
                }
            }
            None => {
                primary_obj.insert(key.clone(), secondary_val.clone());
            }
        }
    }
}

fn merge_required(primary_obj: &mut serde_json::Map<String, JsonValue>, secondary_req: &JsonValue) {
    let Some(secondary_arr) = secondary_req.as_array() else {
        return;
    };

    let mut merged: Vec<JsonValue> = primary_obj
        .get("required")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for req in secondary_arr {
        if !merged.iter().any(|r| r == req) {
            merged.push(req.clone());
        }
    }

    if !merged.is_empty() {
        primary_obj.insert("required".to_string(), JsonValue::Array(merged));
    }
}

/// Deduplicate tool names, sort alphabetically.
pub fn deduplicate_names(raw: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::with_capacity(raw.len());
    let mut names: Vec<String> = Vec::with_capacity(raw.len());

    for name in raw {
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }

    names.sort();
    names
}

#[cfg(test)]
mod tests;
