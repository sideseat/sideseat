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
//! ## Content-Based Identity (not ID-based)
//!
//! - Tool calls: `hash(name + input)` — call_id ignored (regenerated in history)
//! - Tool results: `hash(content)` — tool_use_id ignored
//! - Regular: `hash(trace_id + role + content)`
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
//! 2. FLATTEN     One ContentBlock per BlockEntry with all metadata
//! 3. CLASSIFY    Determine uses_span_end for each block
//! 4. MARK HISTORY Eight-phase detection (see history.rs)
//! 5. DEDUP       Identity-based, keep highest quality version
//! 6. SORT        (birth_time, message_index, entry_index)
//! 7. RETURN      FeedResult with blocks, tool_definitions, metadata
//! ```
//!
//! # Framework Compatibility
//!
//! Works for all frameworks without special cases:
//! - **With history**: Strands, LangGraph, LangChain (duplicates detected/filtered)
//! - **Without history**: AutoGen, CrewAI (passes through unchanged)

mod classify;
mod dedup;
mod history;
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
    SpanTimestamps, normalize_json_for_hash, normalize_tool_result_content, process_dedup,
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
pub(crate) const GENAI_OUTPUT_EVENTS: &[&str] = &["gen_ai.choice", "gen_ai.content.completion"];

/// GenAI input event names (OpenTelemetry semantic conventions).
/// These represent context/input that may be history copies.
pub(crate) const GENAI_INPUT_EVENTS: &[&str] = &[
    "gen_ai.user.message",
    "gen_ai.assistant.message",
    "gen_ai.system.message",
    "gen_ai.tool.message",
    "gen_ai.content.prompt",
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
    // Detect multi-trace: if all rows share the same trace_id, single-trace path
    let is_multi_trace = rows.len() > 1
        && rows
            .first()
            .map(|first| rows.iter().any(|r| r.trace_id != first.trace_id))
            .unwrap_or(false);

    if is_multi_trace {
        process_multi_trace_spans(rows, options)
    } else {
        process_trace_spans(rows, options)
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
    process_trace_spans_core(rows, options, None)
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
    options: &FeedOptions,
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
    let mut blocks = flatten_to_blocks(parsed_messages, &span_hierarchy, options);

    // Stage 2.5: Cross-trace prefix marking (multi-trace sessions only)
    // MUST run BEFORE classify_blocks (which includes Phase 7 duplicate detection).
    // If run after, Phase 7 would mark the second occurrence as history, then
    // cross-trace would mark the first → both become history → genuine content lost.
    // Running before ensures Phase 7 sees the first copy as already-history and
    // skips it, preserving the genuine (second) copy.
    if let Some(prefix) = cross_trace_prefix {
        mark_cross_trace_prefix(&mut blocks, prefix);
    }

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
fn process_multi_trace_spans(rows: Vec<MessageSpanRow>, options: &FeedOptions) -> FeedResult {
    let trace_groups = group_and_sort_traces(rows);

    let mut accumulated = CrossTracePrefixState::default();
    let mut all_blocks: Vec<BlockEntry> = Vec::new();
    let mut all_tool_defs: Vec<serde_json::Value> = Vec::new();
    let mut all_tool_names: Vec<String> = Vec::new();
    let mut total_tokens: i64 = 0;
    let mut total_cost: f64 = 0.0;

    for (trace_idx, trace_rows) in trace_groups.into_iter().enumerate() {
        let trace_tokens: i64 = trace_rows.iter().map(|r| r.total_tokens).sum();
        let trace_cost: f64 = trace_rows.iter().map(|r| r.cost_total).sum();

        // First trace: no prefix. Subsequent traces: pass accumulated prefix
        // for pre-dedup marking of history re-sends.
        let cross_trace_prefix = if trace_idx == 0 {
            None
        } else {
            Some(&accumulated)
        };

        let result = process_trace_spans_core(trace_rows, options, cross_trace_prefix);

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
            total_tokens += trace_tokens;
            total_cost += trace_cost;
        }
    }

    let block_count = all_blocks.len();
    let span_count = all_blocks
        .iter()
        .map(|b| &b.span_id)
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

    // Cross-trace prefix stripping is for attribute-sourced history re-send.
    // Event-based frameworks should remain independent across traces.
    let input_source_count = blocks.iter().filter(|b| b.is_input_source()).count();
    let attribute_input_count = blocks
        .iter()
        .filter(|b| b.is_input_source() && b.source_type == source_type::ATTRIBUTE)
        .count();
    if attribute_input_count == 0 {
        return;
    }

    // Sequential prefix match per span on input-source blocks.
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
        if !block.is_input_source() {
            continue;
        }
        if block.source_type != source_type::ATTRIBUTE {
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
        attribute_input_count,
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
    // Group by conversation boundary
    let mut spans_by_conversation: HashMap<String, Vec<MessageSpanRow>> = HashMap::new();
    for row in rows {
        let key = row
            .session_id
            .as_ref()
            .filter(|s| !s.is_empty())
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
    let mut span_ids: HashSet<String> = HashSet::new();

    for (_, conversation_spans) in spans_by_conversation {
        for row in &conversation_spans {
            span_ids.insert(row.span_id.clone());
            total_tokens += row.total_tokens;
            total_cost += row.cost_total;
        }
        let processed = process_spans(conversation_spans, options);
        all_blocks.extend(processed.messages);
        all_tool_defs.extend(processed.tool_definitions);
        all_tool_names.extend(processed.tool_names);
    }

    // Sort merged blocks for feed display (DESC order: newest first)
    // But within a single response (same span + same timestamp), preserve natural order
    all_blocks.sort_by(|a, b| {
        use std::cmp::Ordering;

        // Same-batch detection: same span + same event timestamp
        // These blocks are from the same response and should preserve original order
        let same_batch = a.span_id == b.span_id && a.timestamp == b.timestamp;

        if same_batch {
            // Within a batch: ASC order (text before tool_use)
            match a.message_index.cmp(&b.message_index) {
                Ordering::Equal => return a.entry_index.cmp(&b.entry_index),
                other => return other,
            }
        }

        // Primary: timestamp DESC (newest first)
        match b.timestamp.cmp(&a.timestamp) {
            Ordering::Equal => {}
            other => return other,
        }

        // Different batches with same timestamp: span_id ASC for stability
        a.span_id.cmp(&b.span_id)
    });

    // Deduplicate tools across conversations
    let tool_definitions = deduplicate_tools(all_tool_defs);
    let tool_names = deduplicate_names(all_tool_names);
    let block_count = all_blocks.len();

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
    let max_depth = span_rows.len(); // Can't have more ancestors than total spans

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
fn flatten_to_blocks(
    messages: Vec<ParsedMessage>,
    span_hierarchy: &HashMap<String, Vec<String>>,
    options: &FeedOptions,
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

        // Apply role filter
        if let Some(ref role_filter) = options.role
            && msg.message.role.as_str() != role_filter
        {
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
        ContentBlock::ToolResult { content, .. } => {
            // Hash by normalized content only (not tool_use_id)
            "tool_result".hash(&mut hasher);
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
            normalize_json_for_hash(data).hash(&mut hasher); // Sort keys for consistent hash
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
    let span_ids: HashSet<_> = blocks.iter().map(|b| &b.span_id).collect();
    let total_tokens: i64 = span_rows.iter().map(|r| r.total_tokens).sum();
    let total_cost: f64 = span_rows.iter().map(|r| r.cost_total).sum();

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
