//! SideML feed pipeline.
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
//! # Shape of the pipeline
//!
//! ```mermaid
//! flowchart TD
//!     rows[MessageSpanRow set<br/>one span per row] --> parse[parse_span_rows<br/>JSON to SideML]
//!     parse --> flatten[flatten_to_blocks<br/>one BlockEntry per ContentBlock]
//!     flatten --> corr[correlate_tool_results<br/>id-less result adopts its call's id]
//!     corr --> classify[classify_blocks<br/>uses_span_end, then eight-phase history]
//!     classify --> evidence[collect_order_evidence<br/>the facts the resolver reads]
//!     classify --> dedup[process_dedup_with_lineage<br/>identity, quality, birth times]
//!     dedup --> withdraw[withdraw_unbacked_ids]
//!     withdraw --> resolve[order_graph::resolve<br/>partial order over units]
//!     evidence -.observations.-> resolve
//!     dedup -.lineage.-> resolve
//!     resolve --> out[FeedResult]
//! ```
//!
//! The dotted edges are why the pre-dedup stages are kept: the resolver places *survivors* using
//! evidence from *every* observation, and the lineage is what connects the two. Neither can be
//! re-derived afterwards - dedup collapses on a key carrying a call's rank across the whole input, and
//! withdrawal changes identities and drops blocks.
//!
//! ## Ordering is not downstream of deduplication across traces
//!
//! Within one trace it is: dedup picks survivors, then the resolver orders them. Across traces of one
//! session it is circular, and that is worth stating because it makes every ordering change riskier
//! than it looks:
//!
//! ```mermaid
//! flowchart LR
//!     t1[trace 1<br/>reconstruct] --> p1[accumulated prefix<br/>role + content, in order]
//!     p1 --> t2[trace 2<br/>mark re-sent prefix as history]
//!     t2 --> d2[dedup keeps the genuine copy]
//!     d2 --> o2[resolve orders trace 2]
//!     o2 --> p2[prefix grows]
//!     p2 --> t3[trace 3 ...]
//! ```
//!
//! The prefix is accumulated from each trace's *finished, ordered* messages, and the next trace's scan
//! consumes it as a sequence. So a change to presentation order changes what the next trace strips,
//! which changes its message *set*: promoting the generation-dataflow constraint moved
//! `adk/tool_use`'s session view from 24 messages to 29 by this route alone. A session's
//! deduplication should not be a function of a sibling trace's presentation order; until that is
//! separated, `promoted_constraints_do_not_change_which_messages_appear` can only hold the
//! single-trace path.
//!
//! # Framework Compatibility
//!
//! Works for all frameworks without special cases:
//! - **With history**: Strands, LangGraph, LangChain (duplicates detected/filtered)
//! - **Without history**: AutoGen, CrewAI (passes through unchanged)

pub mod cache;
mod classify;
mod correlate;
mod dedup;
mod history;
// The order resolver of the reconstruction redesign. Production runs `Constraints::PRODUCTION`; the
// all-off `Constraints::NEUTRAL` is provably unable to move a block and is what the neutrality
// property test checks, so the machinery stays verifiable as classes are promoted one at a time.
pub mod order_graph;
#[cfg(test)]
mod props;
mod types;

mod block_hash;
mod extraction;
mod prefix;
mod session;
mod tool_merge;

use block_hash::compute_block_hash;
#[cfg(test)]
use extraction::compose_error_text;
pub use extraction::extract_tools_from_rows;
use extraction::{
    append_error_messages, build_span_hierarchy, build_span_timestamps, classify_blocks,
    classify_span_view_blocks, flatten_to_blocks, parse_span_rows,
};
use prefix::CrossTracePrefixState;
#[cfg(test)]
use session::sort_feed_newest_first;
use session::{
    apply_role_filter, mark_cross_trace_prefix, process_multi_trace_spans, project_role,
};
pub use session::{apply_time_window, process_feed};
#[cfg(test)]
pub(super) use tool_merge::canonicalize_tool_definition;
use tool_merge::compute_metadata;
pub use tool_merge::{deduplicate_names, deduplicate_tools};

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
#[cfg(test)]
use serde_json::json;

use super::normalize::to_sideml_with_context;
use super::provenance::PositionPath;
use super::tools::{extract_tool_name, normalize_tools, tool_definition_quality};
use super::types::ContentBlock;
use crate::observations::{MessageSource, RawMessage};
use sideseat_ports::types::{MessageCategory, MessageSpanRow};

use classify::uses_span_end;
use dedup::{
    SpanTimestamps, hash_json_into, hash_structured_json_into, hash_tool_result_content_into,
};
use history::{mark_history, mark_span_history};

// Re-exports for public API
pub use types::{BlockEntry, ExtractedTools, FeedMetadata, FeedOptions, FeedResult};

// The dedup tie-break hook, for the test that varies which copy survives.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub use dedup::PREFER_LATER_ON_TIE;

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
    /// Where this message sat in its span's stored payload - see `sideml::provenance`.
    position: PositionPath,
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
    /// The span's name, and the instrumentation scope that produced it.
    ///
    /// Carried because a carrier's meaning can depend on the span that wrote it, and a rule clause may
    /// narrow on either. Scope is *narrowing evidence only*: historical rows may carry none, and
    /// several frameworks share one instrumentation package, so a clause may never key on it alone.
    span_name: Option<String>,
    scope_name: Option<String>,
    scope_version: Option<String>,
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

/// Process the rows for one span while preserving the context that span received.
///
/// Trace and session reconstruction collapse replayed history. A span view instead exposes the normalized
/// payload of that exact span, so prior assistant turns and tool results supplied as input remain visible.
pub fn process_span(rows: Vec<MessageSpanRow>, options: &FeedOptions) -> FeedResult {
    apply_role_filter(process_span_unfiltered(rows), options.role.as_deref())
}

/// [`process_span`], memoised independently from chronological trace/session reconstruction.
pub fn process_span_cached(
    cache: &cache::ReconstructionCache,
    rows: Vec<MessageSpanRow>,
    options: &FeedOptions,
) -> Arc<FeedResult> {
    let reconstructed =
        cache.get_or_reconstruct(cache::Reconstruction::Span, rows, process_span_unfiltered);
    project_role(reconstructed, options.role.as_deref())
}

/// [`process_spans`], memoised on the rows - see [`cache::ReconstructionCache`] for why that is safe.
///
/// The *unfiltered* reconstruction is what is remembered, and the role filter narrows it rather than
/// changing it, so one cached reconstruction serves every role a caller asks for.
///
/// Returns the memo itself when nothing narrows. It used to deep-clone the whole answer on every read,
/// filtered or not - for a long session that is tens of thousands of blocks copied to hand back the same
/// content, on a path whose whole purpose is to avoid recomputing it. With no `role` the caller now gets the
/// `Arc`; with one, only the surviving blocks are copied.
pub fn process_spans_cached(
    cache: &cache::ReconstructionCache,
    rows: Vec<MessageSpanRow>,
    options: &FeedOptions,
) -> Arc<FeedResult> {
    let reconstructed =
        cache.get_or_reconstruct(cache::Reconstruction::Spans, rows, process_spans_unfiltered);
    project_role(reconstructed, options.role.as_deref())
}

/// [`process_feed`], memoised on the rows, exactly as [`process_spans_cached`] is.
pub fn process_feed_cached(
    cache: &cache::ReconstructionCache,
    rows: Vec<MessageSpanRow>,
    options: &FeedOptions,
) -> Arc<FeedResult> {
    // The grouping is passed through, and is part of the cache key: it is the caller's authoritative
    // trace → session mapping, and the reconstruction's answer depends on it. Reconstructing with a bare
    // `FeedOptions::new()` silently discarded it, so the route's fix had no effect on the cached path -
    // which is every production read.
    let grouping = options.session_of_trace.clone();
    let reconstructed = cache.get_or_reconstruct_grouped(
        cache::Reconstruction::Feed,
        rows,
        &options.session_of_trace,
        move |rows| process_feed(rows, &FeedOptions::new().with_session_of_trace(grouping)),
    );
    project_role(reconstructed, options.role.as_deref())
}

/// [`process_spans`] without the role filter, for callers that filter once at their own boundary.
fn process_spans_unfiltered(rows: Vec<MessageSpanRow>) -> FeedResult {
    process_spans_unfiltered_with(rows, order_graph::Constraints::PRODUCTION)
}

fn process_span_unfiltered(rows: Vec<MessageSpanRow>) -> FeedResult {
    reconstruct_trace(
        rows,
        None,
        order_graph::Constraints::SPAN,
        false,
        ReplayPolicy::Preserve,
    )
    .0
}

/// As [`process_spans_unfiltered`], with the ordering constraints named explicitly.
///
/// The parameter exists so a test can hold everything else fixed and vary only the presentation
/// constraints - which is the acceptance property for this whole redesign: changing them must preserve
/// which messages a session returns. Production always passes `PRODUCTION`.
fn process_spans_unfiltered_with(
    rows: Vec<MessageSpanRow>,
    constraints: order_graph::Constraints,
) -> FeedResult {
    // Detect multi-trace: if all rows share the same trace_id, single-trace path
    let is_multi_trace = rows.len() > 1
        && rows
            .first()
            .map(|first| rows.iter().any(|r| r.trace_id != first.trace_id))
            .unwrap_or(false);

    if is_multi_trace {
        process_multi_trace_spans(rows, constraints)
    } else {
        reconstruct_trace(rows, None, constraints, false, ReplayPolicy::Collapse).0
    }
}

#[derive(Clone, Copy)]
enum ReplayPolicy {
    Collapse,
    Preserve,
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
/// Stages 1-4 of the single-trace pipeline: the classified, pre-dedup blocks and the span
/// timestamps, which is the complete evidence set before any observation is collapsed to a
/// representative. `process_trace_spans_core` continues from here into dedup and sorting; the shadow
/// order resolver reads the same output so it judges the evidence production actually reconstructs.
fn classify_span_blocks(
    rows: &[MessageSpanRow],
    cross_trace_prefix: Option<&CrossTracePrefixState>,
    replay_policy: ReplayPolicy,
) -> (Vec<BlockEntry>, HashMap<String, SpanTimestamps>, bool) {
    // Build span hierarchy for span_path computation
    let span_hierarchy = build_span_hierarchy(rows);

    // Build span timestamps map for birth time computation
    let span_timestamps = build_span_timestamps(rows);

    // Stage 1: Parse raw messages and convert to SideML
    let mut parsed_messages = parse_span_rows(rows);

    // Stage 1b: Append error messages from leaf error spans
    append_error_messages(&mut parsed_messages, rows);

    // Stage 2: Flatten to individual blocks with metadata
    // All blocks start with is_history = false
    let mut blocks = flatten_to_blocks(parsed_messages, &span_hierarchy);

    // Cross-trace prefix marking must run before history classification and duplicate detection.
    // If run after, duplicate detection would mark the second occurrence as history, then
    // cross-trace would mark the first → both become history → genuine content lost.
    // Running before ensures duplicate detection sees the first copy as already-history and
    // skips it, preserving the genuine (second) copy.
    let replay_matching_complete = match cross_trace_prefix {
        Some(prefix) => mark_cross_trace_prefix(&mut blocks, prefix),
        None => true,
    };

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
    match replay_policy {
        ReplayPolicy::Collapse => classify_blocks(&mut blocks, &span_timestamps),
        ReplayPolicy::Preserve => classify_span_view_blocks(&mut blocks, &span_timestamps),
    }

    (blocks, span_timestamps, replay_matching_complete)
}

/// The resolver's order over one trace's blocks under a chosen constraint set — what production runs
/// (it is the ordering authority for all four views now), exposed so tests can compare constraint
/// sets against each other.
/// Stage timings for one trace's pipeline, for the performance bench.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn stage_timings(rows: Vec<MessageSpanRow>) -> Vec<(&'static str, std::time::Duration)> {
    let mut out = Vec::new();
    let t = std::time::Instant::now();
    let span_hierarchy = build_span_hierarchy(&rows);
    let span_timestamps = build_span_timestamps(&rows);
    out.push(("  hierarchy+timestamps", t.elapsed()));

    let t = std::time::Instant::now();
    let mut parsed_messages = parse_span_rows(&rows);
    out.push(("  parse_span_rows", t.elapsed()));

    let t = std::time::Instant::now();
    append_error_messages(&mut parsed_messages, &rows);
    out.push(("  append_error_messages", t.elapsed()));

    let t = std::time::Instant::now();
    let mut blocks = flatten_to_blocks(parsed_messages, &span_hierarchy);
    out.push(("  flatten_to_blocks", t.elapsed()));

    let t = std::time::Instant::now();
    correlate::correlate_tool_results(&mut blocks);
    out.push(("  correlate", t.elapsed()));

    let t = std::time::Instant::now();
    classify_blocks(&mut blocks, &span_timestamps);
    out.push(("  classify_blocks(history)", t.elapsed()));

    let t = std::time::Instant::now();
    let evidence = order_graph::collect_order_evidence(&blocks, &span_timestamps);
    out.push(("collect_evidence", t.elapsed()));

    let t = std::time::Instant::now();
    let (survivors, lineage, repeat_ordinals) = survivors_with_lineage(blocks, &span_timestamps);
    out.push(("dedup+withdraw", t.elapsed()));

    let t = std::time::Instant::now();
    let resolved = order_graph::resolve(
        &evidence,
        &survivors,
        &lineage,
        &repeat_ordinals,
        &span_timestamps,
        order_graph::Constraints::NEUTRAL,
    );
    out.push(("resolve", t.elapsed()));

    let t = std::time::Instant::now();
    let _ = compute_metadata(&resolved, &rows, true);
    out.push(("metadata", t.elapsed()));
    out
}

/// The classified, pre-dedup blocks, for diagnosing what the resolver is given.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn classified_blocks_for_test(rows: Vec<MessageSpanRow>) -> Vec<BlockEntry> {
    classify_span_blocks(&rows, None, ReplayPolicy::Collapse).0
}

/// The classified observations with the occurrence rank used by deduplication.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn classified_blocks_with_ordinals_for_test(
    rows: Vec<MessageSpanRow>,
) -> Vec<(BlockEntry, u32)> {
    let blocks = classify_span_blocks(&rows, None, ReplayPolicy::Collapse).0;
    let ordinals = dedup::call_repeat_ordinals(&blocks);
    blocks.into_iter().zip(ordinals).collect()
}

/// One view built twice: with generation dataflow expressed through a barrier node, and as the product
/// of each span's inputs and outputs.
///
/// The barrier exists only to bound the edge count; it must not change the answer.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn barrier_and_pairwise_order(rows: Vec<MessageSpanRow>) -> (Vec<BlockEntry>, Vec<BlockEntry>) {
    let barrier = process_spans_unfiltered_with(rows.clone(), order_graph::Constraints::PRODUCTION);
    let pairwise = process_spans_unfiltered_with(
        rows,
        order_graph::Constraints {
            pairwise_dataflow_edges: true,
            ..order_graph::Constraints::PRODUCTION
        },
    );
    (barrier.messages, pairwise.messages)
}

/// One view built twice: with the constraints production enforces, and with every class off.
///
/// This is the acceptance property of the ordering redesign, as a function a test can call: the two
/// runs must return the *same messages*, differing only in their order. It takes the whole row set, so
/// it exercises the multi-trace path where deduplication and ordering were coupled - a session's
/// cross-trace replay stripping used to consume the presented order, so promoting a constraint changed
/// which messages a later trace kept.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn presented_and_unconstrained(
    rows: Vec<MessageSpanRow>,
) -> (Vec<BlockEntry>, Vec<BlockEntry>) {
    let presented =
        process_spans_unfiltered_with(rows.clone(), order_graph::Constraints::PRODUCTION);
    let unconstrained = process_spans_unfiltered_with(rows, order_graph::Constraints::NEUTRAL);
    (presented.messages, unconstrained.messages)
}

/// The order before the resolver, and the order the resolver produces with every class off.
///
/// The two must be identical: that is what "the resolver cannot move a block by itself" means, and it
/// has to be a property rather than an observation about the current goldens, which are regenerable.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn legacy_and_neutral_order(rows: Vec<MessageSpanRow>) -> (Vec<BlockEntry>, Vec<BlockEntry>) {
    let (blocks, span_timestamps, _) = classify_span_blocks(&rows, None, ReplayPolicy::Collapse);
    let evidence = order_graph::collect_order_evidence(&blocks, &span_timestamps);
    let (legacy, lineage, repeat_ordinals) = survivors_with_lineage(blocks, &span_timestamps);
    let scaffold = order_graph::resolve(
        &evidence,
        &legacy,
        &lineage,
        &repeat_ordinals,
        &span_timestamps,
        order_graph::Constraints::NEUTRAL,
    );
    (legacy, scaffold)
}

/// The post-dedup blocks with the occurrence rank used to distinguish repeated messages.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn deduped_blocks_with_ordinals_for_test(rows: Vec<MessageSpanRow>) -> Vec<(BlockEntry, u32)> {
    let (blocks, span_timestamps, _) = classify_span_blocks(&rows, None, ReplayPolicy::Collapse);
    let (blocks, _, ordinals) = survivors_with_lineage(blocks, &span_timestamps);
    blocks.into_iter().zip(ordinals).collect()
}

#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn shadow_resolved_order(rows: Vec<MessageSpanRow>) -> Vec<BlockEntry> {
    let (blocks, span_timestamps, _) = classify_span_blocks(&rows, None, ReplayPolicy::Collapse);
    let evidence = order_graph::collect_order_evidence(&blocks, &span_timestamps);
    let (survivors, lineage, repeat_ordinals) = survivors_with_lineage(blocks, &span_timestamps);
    order_graph::resolve(
        &evidence,
        &survivors,
        &lineage,
        &repeat_ordinals,
        &span_timestamps,
        order_graph::Constraints::FULL,
    )
}

/// Dedup and withdrawal, with a lineage from each pre-dedup observation to the block it became.
///
/// Both stages change the mapping and neither can be inverted afterwards: dedup collapses on a key
/// that carries a call's rank across the whole input, and withdrawal clears ids *and* drops blocks. So
/// the two remaps are composed here, once, for every caller that needs to trace evidence.
fn survivors_with_lineage(
    blocks: Vec<BlockEntry>,
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> (Vec<BlockEntry>, Vec<Option<usize>>, Vec<u32>) {
    let (blocks, dedup_lineage, dedup_ordinals) =
        dedup::process_dedup_with_lineage_and_ordinals(blocks, span_timestamps.clone());
    let (mut blocks, withdrawal_remap) = correlate::withdraw_unbacked_ids_with_remap(blocks);
    let lineage = dedup_lineage
        .into_iter()
        .map(|survivor| survivor.and_then(|s| withdrawal_remap.get(s).copied().flatten()))
        .collect();
    let mut repeat_ordinals = vec![0; blocks.len()];
    for (old, survivor) in withdrawal_remap.into_iter().enumerate() {
        if let Some(survivor) = survivor {
            repeat_ordinals[survivor] = dedup_ordinals[old];
        }
    }
    for (block, ordinal) in blocks.iter_mut().zip(&repeat_ordinals) {
        block.occurrence_ordinal = *ordinal;
    }
    (blocks, lineage, repeat_ordinals)
}

fn process_trace_spans_core(
    rows: Vec<MessageSpanRow>,
    cross_trace_prefix: Option<&CrossTracePrefixState>,
) -> FeedResult {
    reconstruct_trace(
        rows,
        cross_trace_prefix,
        order_graph::Constraints::PRODUCTION,
        false,
        ReplayPolicy::Collapse,
    )
    .0
}

/// The trace's answer, and the **causal transcript** a following trace matches its replay against.
///
/// Two separate outputs on purpose. The transcript is the survivors in the order reconstruction
/// established *before* the order resolver applies any presentation constraint, so it is a function of
/// the evidence alone. A session's deduplication must not depend on how the messages are laid out for
/// a reader: accumulating the prefix from the presented order made promoting an ordering constraint
/// change which messages a *later* trace kept, and `adk/tool_use`'s session view gained five replayed
/// messages that way - the previous turns' tool results, re-appearing because the presented sequence no
/// longer lined up with what ADK replays.
fn reconstruct_trace(
    rows: Vec<MessageSpanRow>,
    cross_trace_prefix: Option<&CrossTracePrefixState>,
    constraints: order_graph::Constraints,
    needs_replay_relation: bool,
    replay_policy: ReplayPolicy,
) -> (FeedResult, Vec<BlockEntry>, order_graph::Precedence) {
    // Extract tools from all rows
    let extracted_tools = extract_tools_from_rows(&rows);

    // Stages 1-4: parse, flatten, correlate and classify - everything the pipeline knows before
    // dedup collapses the observations to one representative each. Kept, because the order resolver
    // reads the *evidence*: the emission binding a turn's intro text to its call is on one span while
    // dedup may keep a re-listed copy of that text from another, and only the pre-dedup set says so.
    let (blocks, span_timestamps, replay_matching_complete) =
        classify_span_blocks(&rows, cross_trace_prefix, replay_policy);
    // Reduced to what the resolver reads, from the borrowed slice: holding the blocks themselves
    // would clone every message's content, which on a trace carrying base64 images dominates the
    // whole request and is never read.
    let evidence = order_graph::collect_order_evidence(&blocks, &span_timestamps);

    // Stages 5-6: Deduplicate by identity and sort by birth time, then stage 6.5, withdrawing a
    // correlated id whose call did not survive.
    //
    // Correlation only ever links to a call in the same block list, but dedup and history marking can
    // drop that call afterwards - leaving the result pointing at something the response does not
    // contain. Clearing the id restores "honestly uncorrelated"; keeping the block, because the
    // result's content is real either way. Only correlated ids are withdrawn: a provider's own id may
    // legitimately reference a call outside the requested scope.
    //
    // The two run together because the resolver below needs the *lineage* across both: each says which
    // block an observation became, and neither mapping can be re-derived afterwards.
    let (blocks, lineage, repeat_ordinals) = survivors_with_lineage(blocks, &span_timestamps);

    // Stage 6.6: Resolve the order as a partial order rather than a scalar key.
    //
    // This is the ordering authority: `Constraints::PRODUCTION` names exactly which classes may
    // change the answer, `NEUTRAL` provably cannot move a block
    // (`the_neutral_resolver_reproduces_the_legacy_order`), and promotions happen one class at a
    // time with a reviewed corpus delta. Runs after id withdrawal so the call/result edges see the
    // ids the view will actually show.
    // The transcript: survivors as reconstruction established them, before any presentation choice.
    let transcript = blocks.clone();

    // The relation a following trace matches its replay against, over the *pre-resolve* survivors -
    // which is what the transcript is. Built only when a later trace can use it: for a single-trace
    // request nothing ever asks.
    let replay_relation = if needs_replay_relation {
        order_graph::causal_precedence(&evidence, &blocks, &lineage, &repeat_ordinals)
    } else {
        order_graph::Precedence::default()
    };

    let blocks = order_graph::resolve(
        &evidence,
        &blocks,
        &lineage,
        &repeat_ordinals,
        &span_timestamps,
        constraints,
    );

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
    let metadata = compute_metadata(&blocks, &rows, replay_matching_complete);

    (
        FeedResult {
            messages: blocks,
            tool_definitions: extracted_tools.tool_definitions,
            tool_names: extracted_tools.tool_names,
            metadata,
        },
        transcript,
        replay_relation,
    )
}

#[cfg(test)]
mod tests;
