//! History detection for the feed pipeline.
//!
//! This module detects and marks historical/intermediate content that should
//! be filtered from the conversation timeline.
//!
//! # Design Principles
//!
//! 1. **No cross-trace deduplication**: Different traces in a session can
//!    legitimately have the same message content. All deduplication happens
//!    within a single trace only.
//!
//! 2. **Tool linking via tool_use_id**: Tool calls and results are linked.
//!    If a tool_use is history, its tool_result is also history.
//!
//! 3. **Universal signals**: Detection uses OTel conventions, span structure,
//!    and timestamps - not framework-specific logic.
//!
//! # What is "History"?
//!
//! In AI agent traces, the same content often appears multiple times:
//! - **Session history**: Previous turns re-sent as context to LLM calls
//! - **Context copies**: Parent span messages duplicated in child spans
//! - **Intermediate output**: Non-final responses during tool-use loops
//!
//! # Detection Strategy
//!
//! The algorithm uses multiple signals to identify history:
//!
//! 1. **Protected = Current**: GenAIChoice, finish_reason → always kept
//! 2. **Timestamp-based**: Message timestamp < span start → historical context
//! 3. **Tool linking**: Tool_results are current iff their tool_use_id is current
//! 4. **Intermediate filtering**: Assistant text in generation spans (when agent
//!    spans exist) without finish_reason → intermediate output
//!
//! # Detection order
//!
//! The order is part of the algorithm: establish current tool calls, mark timestamp-derived history,
//! remove accumulator and intermediate copies, suppress replayed generation input, mark orphan results,
//! preserve a witness for completed turns, and finally deduplicate the remaining blocks.

use std::collections::{HashMap, HashSet};

use super::dedup::{
    SpanTimestamps, compute_tool_call_hash, effective_timestamp, hash_tool_result_content_into,
};
use super::types::BlockEntry;
use crate::sideml::types::{ChatRole, ContentBlock};

// ============================================================================
// TOOL USE ID MAP
// ============================================================================

/// Build a map of tool_use_ids to their "current" status, **per trace**.
///
/// A tool_use is "current" (not history) if it appears in a protected block
/// (GenAIChoice or finish_reason). This identifies tool calls that are part of
/// the current turn's LLM output.
///
/// Returns: Map of trace_id -> Set of tool_use_ids that are current (not history)
///
/// IMPORTANT: This must be per-trace to avoid cross-trace contamination when
/// processing sessions. Tool_use_ids from previous traces should not be
/// considered "current" for subsequent traces.
///
/// NOTE: We intentionally use ONLY protected blocks (not all agent span blocks).
/// Event-based frameworks (Strands) bubble ALL events (including historical ones
/// from previous turns) up to the root agent span. Including agent span blocks
/// would incorrectly collect historical tool_use_ids as "current", preventing their tool results from being
/// recognised as orphans.
fn build_current_tool_use_ids(blocks: &[BlockEntry]) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();

    for block in blocks {
        // Only protected tool_uses (gen_ai.choice or finish_reason) are current.
        // Agent span tool_uses are excluded because bubbled-up historical events
        // would contaminate the set with tool_use_ids from previous turns.
        if !block.is_protected() {
            continue;
        }

        if let ContentBlock::ToolUse { id: Some(id), .. } = &block.content {
            map.entry(block.trace_id.clone())
                .or_default()
                .insert(id.clone());
        }
    }

    map
}

/// Session history detection result.
#[derive(Debug, Default)]
struct SessionHistoryInfo {
    /// Has agent spans (Strands-like structure)
    has_agent_spans: bool,
    /// Has event-based messages (Strands pattern with gen_ai.* events)
    /// When true, child-generation intermediate filtering applies because events bubble up.
    /// When false, generation spans hold authoritative output (LangGraph pattern)
    has_event_based_messages: bool,
    /// Traces that have multi-turn history (tool_results in generation spans)
    /// IMPORTANT: This is per-trace to avoid cross-trace contamination
    traces_with_multi_turn_history: HashSet<String>,
}

/// Check if traces have session history.
///
/// Returns info about what kind of history exists:
/// - `has_agent_spans`: Strands-like structure with authoritative root
/// - `has_event_based_messages`: Whether trace uses event-based (Strands) or
///   attribute-based (LangGraph) message pattern
/// - `traces_with_multi_turn_history`: Per-trace detection of multi-turn history
///
/// IMPORTANT: Multi-turn history detection must be per-trace. A trace has
/// multi-turn history if it has tool_results in generation spans, which indicates
/// the LLM was sent previous turn context.
fn detect_session_history(blocks: &[BlockEntry]) -> SessionHistoryInfo {
    let has_agent_spans = blocks.iter().any(BlockEntry::is_agent_span);

    // Detect event-based vs attribute-based message pattern
    // Event-based (Strands): Messages come from gen_ai.* events, bubble up to root
    // Attribute-based (LangGraph): Messages in llm.output_messages attributes, gen spans authoritative
    let has_event_based_messages = blocks
        .iter()
        .any(|b| b.is_from_event() && (b.is_output_event() || b.is_input_event()));

    // Detect multi-turn history PER TRACE
    // A trace has multi-turn history if it has tool_results in generation spans
    let mut traces_with_multi_turn_history = HashSet::new();

    // Only detect multi-turn history for event-based frameworks
    // Attribute-based frameworks don't have this pattern
    if has_agent_spans && has_event_based_messages {
        for block in blocks {
            if block.is_generation_span() && block.is_tool_result() {
                traces_with_multi_turn_history.insert(block.trace_id.clone());
            }
        }
    }

    SessionHistoryInfo {
        has_agent_spans,
        has_event_based_messages,
        traces_with_multi_turn_history,
    }
}

// ============================================================================
// HISTORY DETECTION
// ============================================================================

/// Spans at the root of the conversation: no ancestor carries conversation content.
///
/// The accumulator and child-generation passes treat the root's copy of a turn as authoritative and
/// the copies below it as replays. The trace's first span is the wrong root when the application wraps
/// its work in a span of its own - `sideseat.trace(...)` around several agent calls is the ordinary
/// way to group a conversation - because that wrapper holds no messages. Every agent invocation under
/// it then looked like a nested accumulator, every copy of each question became history, and a
/// three-question conversation showed one question. A span is a root here when none of its ancestors
/// carries a message.
fn conversation_root_spans(blocks: &[BlockEntry]) -> HashSet<String> {
    let content_spans: HashSet<&str> = blocks.iter().map(|b| b.span_id.as_str()).collect();
    blocks
        .iter()
        .filter(|block| match block.span_path.split_last() {
            Some((_, ancestors)) => !ancestors
                .iter()
                .any(|span| content_spans.contains(span.as_str())),
            // Without a computed path, the parent is the only ancestor known.
            None => block
                .parent_span_id
                .as_deref()
                .is_none_or(|parent| !content_spans.contains(parent)),
        })
        .map(|block| block.span_id.clone())
        .collect()
}

/// Preserve the context one span carried while still removing redundant carriers and framework state.
///
/// A span message endpoint answers a different question from a trace or session endpoint: it shows the
/// normalized conversation payload of that one span, including history supplied to the call. The ordinary
/// history passes intentionally remove that replay from wider views, so applying them here drops real
/// span-local evidence such as an assistant turn in `llm.input_messages`.
pub fn mark_span_history(
    blocks: &mut [BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> HistoryStats {
    let mut stats = HistoryStats {
        protected: blocks.iter().filter(|b| b.is_protected()).count(),
        ..Default::default()
    };

    // A chain's raw JSON output is implementation state rather than a conversation message. This is not
    // replay removal, so it remains hidden even when the caller asks what one span carried.
    for block in blocks.iter_mut() {
        if block.observation_type.as_deref() == Some("chain") && block.entry_type == "json" {
            block.is_history = true;
            stats.accumulator_history += 1;
        }
    }

    stats.duplicates = mark_duplicate_history(blocks, span_timestamps);
    stats
}

/// Mark blocks as history based on universal signals.
///
/// # Algorithm
///
/// The passes are intentionally ordered; later passes depend on history and call identity established by
/// earlier ones.
pub fn mark_history(
    blocks: &mut [BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> HistoryStats {
    let mut stats = HistoryStats {
        protected: blocks.iter().filter(|b| b.is_protected()).count(),
        ..Default::default()
    };

    // Detect the trace shape and index tool calls that belong to the current turn.
    let conversation_roots = conversation_root_spans(blocks);
    let at_root = |block: &BlockEntry| conversation_roots.contains(&block.span_id);
    // A configured agent states its own instructions; a pass-through loop span does not, even where a
    // producer classifies its loop spans as agents.
    let configured_agents: HashSet<String> = blocks
        .iter()
        .filter(|b| b.is_agent_span() && b.role == ChatRole::System)
        .map(|b| b.span_id.clone())
        .collect();
    let is_configured_agent = |block: &BlockEntry| configured_agents.contains(&block.span_id);
    let current_tool_ids = build_current_tool_use_ids(blocks);
    let history_info = detect_session_history(blocks);

    tracing::trace!(
        current_tool_ids = current_tool_ids.len(),
        has_agent_spans = history_info.has_agent_spans,
        has_event_based = history_info.has_event_based_messages,
        traces_with_multi_turn = history_info.traces_with_multi_turn_history.len(),
        "history detection: analysis complete"
    );

    // Mark timestamp-derived history in child generation spans.
    // Messages with timestamp < span_start are historical context passed to the span.
    // This handles both simple history (previous turn) and complex multi-turn history.
    for block in blocks.iter_mut() {
        if block.is_protected() || block.is_history {
            continue;
        }

        // Only child spans (has parent) - root span content is authoritative
        if block.is_root_span() {
            continue;
        }

        // Only generation spans contain session history context
        if !block.is_generation_span() {
            continue;
        }

        // Check if block timestamp is before span start
        if let Some(span_ts) = span_timestamps.get(&block.span_id)
            && block.timestamp < span_ts.span_start
        {
            block.is_history = true;
            stats.generation_history += 1;
            tracing::trace!(
                span_id = %block.span_id,
                block_time = %block.timestamp,
                span_start = %span_ts.span_start,
                "marked as history (timestamp < span_start)"
            );
        }
    }

    // Filter intermediate state from accumulator and chain spans.
    //
    // This phase handles clear intermediate state that should be filtered:
    // 1. Raw JSON output from chain spans (framework state) - even root
    // 2. Input events from non-root accumulator spans (context copies)
    //
    // We DON'T aggressively filter all input-source content because:
    // - timestamp comparison already catches messages predating the span
    // - final identity deduplication catches duplicate content
    // - Some input sources contain unique authoritative messages
    for block in blocks.iter_mut() {
        if block.is_protected() || block.is_history {
            continue;
        }

        // Tool results from execution remain unless the orphan-result pass rejects them.
        if block.is_tool_result() {
            continue;
        }

        // Raw JSON output from chain spans = framework state, not semantic messages
        // This applies to ALL chain spans including root because:
        // - LangGraph root span output.value = raw graph state
        // - Actual semantic messages are in child generation spans
        // - This must be checked BEFORE the root span skip
        if block.observation_type.as_deref() == Some("chain") && block.entry_type == "json" {
            block.is_history = true;
            stats.accumulator_history += 1;
            continue;
        }

        // Root content is authoritative (except JSON handled above).
        if at_root(block) {
            continue;
        }

        // Accumulators pass messages through, so their input events are copies of the turn above them.
        // A configured agent below the root is the exception: a swarm member or sub-agent carries its
        // own system prompt and a request its orchestrator composed, which no other span carries. Its
        // inputs that do repeat the parent's are removed by identity deduplication instead.
        if block.is_accumulator_span() && !is_configured_agent(block) && block.is_input_event() {
            block.is_history = true;
            stats.accumulator_history += 1;
        }
    }

    // Filter intermediate content from event-based traces.
    //
    // For frameworks using OTEL events (gen_ai.choice, gen_ai.user.message):
    // - Events bubble up from child spans to root agent span
    // - Root agent span has authoritative current-turn messages
    // - Child generation span content is intermediate, duplicated at root
    //
    // This pass only applies when BOTH conditions are true:
    // - has_agent_spans: Root span is an agent that collects events
    // - has_event_based_messages: Framework uses gen_ai.* events
    //
    // For attribute-based frameworks (LangGraph, OpenInference):
    // - Generation spans have authoritative output in llm.output_messages
    // - No event bubbling, child generation spans ARE the source of truth
    // - This pass is skipped
    let mut generation_marked_users: Vec<usize> = Vec::new();
    if history_info.has_agent_spans && history_info.has_event_based_messages {
        for (index, block) in blocks.iter_mut().enumerate() {
            if block.is_protected() || block.is_history {
                continue;
            }

            // Only generation spans below the conversation root
            if !block.is_generation_span() || at_root(block) {
                continue;
            }

            // Filter based on role (both event and attribute sources are intermediate)
            match block.role {
                // User/System in child generation spans = history context copies
                ChatRole::User | ChatRole::System => {
                    block.is_history = true;
                    stats.generation_history += 1;
                    if block.role == ChatRole::User {
                        generation_marked_users.push(index);
                    }
                }
                // Assistant text/thinking = intermediate output (final at root). Not reasoning whose text
                // was withheld: the root restates what a response said, a withheld block says nothing it
                // could restate, and marking it would drop the conversation's only copy.
                ChatRole::Assistant
                    if block.is_text()
                        || (block.is_thinking() && !block.is_withheld_thinking()) =>
                {
                    block.is_history = true;
                    stats.generation_history += 1;
                }
                // Tool role and ToolUse preserved for matching
                _ => {}
            }
        }
    }

    // Mark assistant history re-sent through input attributes.
    //
    // For attribute-based frameworks (ADK, Vercel, LiveKit, etc.):
    // A non-root generation span's INPUT attributes (e.g. llm_request) re-send
    // previous assistant responses as context. The current response comes from
    // OUTPUT attributes (e.g. llm_response / gen_ai.choice).
    //
    // This pass marks assistant content from input sources in non-root generation
    // spans as history. It catches re-sent assistant text/thinking/tool_use that
    // identity deduplication misses because the LLM regenerates different text.
    for block in blocks.iter_mut() {
        if block.is_protected() || block.is_history {
            continue;
        }
        if !block.is_generation_span() || block.is_root_span() {
            continue;
        }
        if block.role != ChatRole::Assistant {
            continue;
        }
        if block.is_from_event() {
            continue;
        }
        if !block.is_input_source() {
            continue;
        }
        block.is_history = true;
        stats.input_source_history += 1;
    }

    // Suppress all unprotected generation content in traces that re-send complete turns.
    // When tool_results exist in generation spans, it indicates full history re-send
    // IMPORTANT: Check per-trace to avoid cross-trace contamination
    for block in blocks.iter_mut() {
        if block.is_protected() || block.is_history {
            continue;
        }

        // A generation span at the conversation root is the only carrier of its turn: no agent span
        // above it holds the authoritative copy that this pass assumes, so its re-sent inputs are left
        // to identity deduplication instead.
        if !block.is_generation_span() || at_root(block) {
            continue;
        }

        // Only filter if THIS trace has multi-turn history
        if !history_info
            .traces_with_multi_turn_history
            .contains(&block.trace_id)
        {
            continue;
        }

        block.is_history = true;
        stats.generation_history += 1;
    }

    // Mark tool results whose calls do not belong to the current turn.
    // Tool_results whose tool_use_id is not in current set FOR THE SAME TRACE are history
    // IMPORTANT: Check against the same trace's tool_use_ids only to avoid cross-trace contamination
    // IMPORTANT: Only applies to traces with multi-turn history
    for block in blocks.iter_mut() {
        if block.is_protected() || block.is_history {
            continue;
        }

        // Only for traces with multi-turn history
        if !history_info
            .traces_with_multi_turn_history
            .contains(&block.trace_id)
        {
            continue;
        }

        // Only applies to tool_results with a tool_use_id the framework itself sent. A
        // correlated id was taken from a call in this same trace, so "names no current call"
        // cannot be read as "belongs to a past turn" - before correlation ran this early, such
        // results reached this phase with no id and were skipped, and they must stay skipped.
        if block.tool_use_id_correlated {
            continue;
        }
        let tool_use_id = match &block.content {
            ContentBlock::ToolResult {
                tool_use_id: Some(id),
                ..
            } => id,
            _ => continue,
        };

        // If tool_use_id not in current set FOR THIS TRACE, it's orphan
        let trace_tool_ids = current_tool_ids.get(&block.trace_id);
        let is_orphan = trace_tool_ids
            .map(|ids| !ids.contains(tool_use_id))
            .unwrap_or(true); // No tool_ids for this trace = all are orphan

        if is_orphan {
            block.is_history = true;
            stats.orphan_tool_results += 1;
            tracing::trace!(
                span_id = %block.span_id,
                trace_id = %block.trace_id,
                tool_use_id = %tool_use_id,
                "marked as history (orphan tool_result)"
            );
        }
    }

    // Preserve at least one witness that a completed turn happened.
    //
    // The accumulator and child-generation passes both assume that a copy on the *root* agent span is the
    // authoritative one, so a child's copy is an intermediate duplicate. Where that holds, marking the
    // child costs nothing. `strands-js/swarm` is where it does not: its root agent span carries
    // `system, assistant` and never re-lists the user's request, so child-generation filtering marked the
    // chat-span copy and accumulator filtering marked the other; every copy became history and the
    // class entirely. The trace and the feed showed a plan with no request, while one span view still
    // displayed it.
    //
    // So one user witness is kept when nothing non-history is left to carry the turn. Two conditions,
    // and the second is what makes it safe:
    //
    // - it was marked by **child-generation filtering** specifically, not by accumulator filtering. That
    //   matters because a span view loads one span, where "nothing else carries the turn" is trivially
    //   true - rescuing accumulator-marked blocks there gave langgraph's `tools` span views a message
    //   they had never shown. Child-generation filtering only runs when an agent span is in scope, so it is
    //   scope-safe by construction;
    // - the block's own time is **at or after** its span's start, which is what distinguishes an input
    //   this span was given from a previous turn re-sent into it. A genuine re-send predates the span it
    //   was sent to, so this rescue cannot resurrect one;
    // - nothing non-history in the trace already carries that role, so where the root copy *does* exist
    //   this changes nothing at all.
    //
    // The earliest surviving candidate is chosen, and only one, so a trace whose question was re-sent to
    // nine generation spans still shows it once.
    let traces_needing_a_user: HashSet<String> = {
        let mut with = HashSet::new();
        let mut without: HashSet<String> = HashSet::new();
        for block in blocks.iter() {
            if block.role != ChatRole::User {
                continue;
            }
            if block.is_history {
                without.insert(block.trace_id.clone());
            } else {
                with.insert(block.trace_id.clone());
            }
        }
        without.difference(&with).cloned().collect()
    };
    if !traces_needing_a_user.is_empty() {
        let mut rescued: HashSet<String> = HashSet::new();
        let mut candidates: Vec<usize> = generation_marked_users
            .iter()
            .copied()
            .filter(|&i| {
                let block = &blocks[i];
                block.is_history
                    && !block.is_cross_trace_history
                    && block.role == ChatRole::User
                    && traces_needing_a_user.contains(&block.trace_id)
                    && span_timestamps
                        .get(&block.span_id)
                        .is_none_or(|t| block.timestamp >= t.span_start)
            })
            .collect();
        candidates.sort_by_key(|&i| (blocks[i].timestamp, blocks[i].span_id.clone()));
        for i in candidates {
            if rescued.insert(blocks[i].trace_id.clone()) {
                blocks[i].is_history = false;
                tracing::debug!(
                    span_id = %blocks[i].span_id,
                    trace_id = %blocks[i].trace_id,
                    "kept a user turn that every phase had marked history - nothing else carried it"
                );
            }
        }
    }

    // Deduplicate the remaining blocks.
    stats.duplicates = mark_duplicate_history(blocks, span_timestamps);

    tracing::trace!(
        protected = stats.protected,
        accumulator = stats.accumulator_history,
        generation = stats.generation_history,
        input_source = stats.input_source_history,
        orphan_results = stats.orphan_tool_results,
        duplicates = stats.duplicates,
        "history detection complete"
    );

    stats
}

fn mark_duplicate_history(
    blocks: &mut [BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> usize {
    let duplicate_indices = find_duplicate_indices(blocks, span_timestamps);
    let duplicate_count = duplicate_indices.len();
    for idx in duplicate_indices {
        blocks[idx].is_history = true;
    }
    duplicate_count
}

/// The key used to group blocks that represent the same message.
///
/// Content for everything except a tool result, which is keyed by the *call it answers*.
#[derive(PartialEq, Eq, Hash)]
enum DuplicateKey<'a> {
    Content(&'a str, &'a str),
    /// (trace_id, hash of the answered call's name + input, hash of the result text)
    ///
    /// The result's *text*, not its whole block identity. The block identity also covers the tool
    /// name and the error flag, which is what distinguishes two results that name no call - but
    /// once the call is known those are redundant, and a framework that includes the tool name on
    /// the original and omits it on the re-send would produce two keys for one message.
    ToolResultForCall(&'a str, u64, u64),
}

/// Hash a tool result's text alone, for comparing two results of the same call.
fn hash_tool_result_text(content: &serde_json::Value) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_tool_result_content_into(content, &mut hasher);
    hasher.finish()
}

/// Tool-call identity, by call id, for every call in the block set.
///
/// A history re-send regenerates the call id but not the name or input, so two re-sends of one
/// call map to the same hash - which is what lets a re-sent result be recognised as a duplicate
/// while two results answering genuinely different calls are not.
fn tool_call_identities(blocks: &[BlockEntry]) -> HashMap<(&str, &str), u64> {
    let mut identities = HashMap::new();
    for block in blocks {
        if let ContentBlock::ToolUse {
            id, name, input, ..
        } = &block.content
            && let Some(id) = id.as_deref().filter(|s| !s.is_empty())
        {
            identities.insert(
                (block.trace_id.as_str(), id),
                compute_tool_call_hash(name, input),
            );
        }
    }
    identities
}

/// How far apart two spans' clocks may be. OpenTelemetry JavaScript fixes each span's clock offset
/// from a millisecond wall clock when the span starts, so events on different spans can disagree by
/// up to this much.
const SPAN_CLOCK_SKEW: chrono::TimeDelta = chrono::TimeDelta::milliseconds(1);

/// The copy of a received message that is its original: the earliest, unless it is a model call's
/// and an agent span reported the message within the clock skew of it.
///
/// An agent receives the question it hands to a model call, but inside the skew time cannot say so:
/// the model call's copy can carry the earlier time. The survivor then followed clock noise, and
/// with it the observation the message was attributed to. The spans relating the two copies may
/// carry no messages and be absent from the rows, so the kinds of span decide. Beyond the skew, and
/// between any other kinds of span, time stands.
///
/// A produced copy is the original when it is a model call's own emission, which the sort before this
/// already puts first (see `is_fresh_emission`). Otherwise every produced copy re-lists, and the one
/// nearest the work is the original: an enclosing span's report of what a span below it produced is
/// the enclosing span restating it, whichever of the two ended first - so the earliest is replaced by a
/// produced copy on a span it encloses, repeatedly, until none is left below it.
fn original_copy(
    blocks: &[BlockEntry],
    sorted: &[(usize, bool, bool, chrono::DateTime<chrono::Utc>)],
) -> usize {
    let (first, is_output, _, time) = sorted[0];
    if is_output {
        // A model call's own emission is the original, unless a model call below it emitted the same copy:
        // it is then a framework step restating that call's response, and the walk below finds the call.
        if is_fresh_emission(&blocks[first])
            && !sorted.iter().any(|&(index, other_output, _, _)| {
                other_output
                    && is_fresh_emission(&blocks[index])
                    && encloses(&blocks[first], &blocks[index])
            })
        {
            return 0;
        }
        // Bounded by the number of copies, so a span path that loops cannot.
        let mut keep = 0;
        for _ in 0..sorted.len() {
            match sorted.iter().position(|&(index, other_output, _, _)| {
                other_output && encloses(&blocks[sorted[keep].0], &blocks[index])
            }) {
                Some(inner) => keep = inner,
                None => break,
            }
        }
        // An agent or chain span's output that may restate what its run already did lists a call when
        // the run reported it; the tool span that ran the call is where it happened. Ancestry cannot
        // decide this one: a span that carried no message is absent from every path, which is what a
        // framework's step spans between the agent and its tools usually are.
        let kept = &blocks[sorted[keep].0];
        if kept.is_accumulator_span()
            && crate::sideml::carrier::semantics_for_context(&kept.carrier_context())
                .may_restate_prior_observations
            && let Some(executed) = sole_execution(blocks, sorted)
        {
            // Exactly one span ran it. Two executions of one shape are told apart by their ranks before
            // this, so two spans here mean the occurrence is ambiguous, and the re-listing stands.
            return executed;
        }
        // Likewise a message a span only passes on - a template's rendered prompt, a chain's state - when a
        // model call was sent it: the call's input is where the message was used, and the re-listing is the
        // passing span restating it. Only a carrier that may restate yields; a span that emits what it
        // produced, a model call's or a tool's, keeps its copy.
        if !produces(kept)
            && crate::sideml::carrier::semantics_for_context(&kept.carrier_context())
                .may_restate_prior_observations
            // The earliest receiving call, and between calls the clock cannot order, the lowest span id:
            // which call keeps the message must not depend on the order rows arrived in.
            && let Some(received) = sorted
                .iter()
                .enumerate()
                .filter(|&(_, &(index, other_output, _, _))| {
                    !other_output && blocks[index].is_generation_span()
                })
                .min_by(|(_, a), (_, b)| {
                    (a.3, blocks[a.0].span_id.as_str()).cmp(&(b.3, blocks[b.0].span_id.as_str()))
                })
                .map(|(position, _)| position)
        {
            return received;
        }
        return keep;
    }
    if !blocks[first].is_generation_span() {
        return 0;
    }
    sorted
        .iter()
        .position(|&(index, other_output, _, other_time)| {
            !other_output && other_time - time < SPAN_CLOCK_SKEW && blocks[index].is_agent_span()
        })
        .unwrap_or(0)
}

/// Whether a span makes what it outputs: a model call's reply, a tool's result. Every other span's output
/// passes on messages that something else made.
fn produces(block: &BlockEntry) -> bool {
    block.is_generation_span() || block.observation_type.as_deref() == Some(super::obs_type::TOOL)
}

/// Where in `sorted` the one tool span that ran this call sits, when exactly one span did. A span
/// delivered twice is still one span.
fn sole_execution(
    blocks: &[BlockEntry],
    sorted: &[(usize, bool, bool, chrono::DateTime<chrono::Utc>)],
) -> Option<usize> {
    let executions: Vec<usize> = sorted
        .iter()
        .enumerate()
        .filter(|&(_, &(index, _, _, _))| executes_call(&blocks[index]))
        .map(|(position, _)| position)
        .collect();
    let first = *executions.first()?;
    let span = &blocks[sorted[first].0].span_id;
    executions
        .iter()
        .all(|&position| &blocks[sorted[position].0].span_id == span)
        .then_some(first)
}

/// A tool span's record of the call it ran: where the call was observed, though its carrier is the tool's
/// input rather than anything the span produced.
fn executes_call(block: &BlockEntry) -> bool {
    matches!(block.content, ContentBlock::ToolUse { .. })
        && block.observation_type.as_deref() == Some(super::obs_type::TOOL)
        && !block.is_output_source()
}

/// Whether `outer`'s span is a strict ancestor of `inner`'s.
fn encloses(outer: &BlockEntry, inner: &BlockEntry) -> bool {
    outer.span_id != inner.span_id
        && inner
            .span_path
            .split_last()
            .is_some_and(|(_, ancestors)| ancestors.contains(&outer.span_id))
}

/// A model call reporting what it produced, in a carrier that never restates earlier observations.
fn is_fresh_emission(block: &BlockEntry) -> bool {
    block.is_generation_span()
        && !crate::sideml::carrier::semantics_for_context(&block.carrier_context())
            .may_restate_prior_observations
}

/// Find indices of duplicate blocks that should be marked as history.
fn find_duplicate_indices(
    blocks: &[BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> Vec<usize> {
    let call_identities = tool_call_identities(blocks);
    // The same call rank the final dedup uses, so the two stages agree about what "the same message"
    // is. Without it this phase marked the second of two identical calls in one response as history,
    // and the final dedup then had nothing to keep it for.
    let ordinals = super::dedup::call_repeat_ordinals(blocks);
    let mut blocks_by_key: HashMap<(DuplicateKey<'_>, u32), Vec<usize>> = HashMap::new();

    for (idx, block) in blocks.iter().enumerate() {
        if block.is_protected() || block.is_history {
            continue;
        }
        // A tool result is keyed by the call it answers *and* its text, not by text alone.
        //
        // Keyed by text alone, two results with the same text collapsed into one - and identical
        // text is ordinary ("ok", "[]", the same search hit). Keyed by the call alone, two calls
        // that happen to be identical (same tool, same input, run twice) collapsed their two
        // different results into one. Both parts are needed:
        //
        // - Strands re-sends one call's result with a regenerated id: same call identity, same
        //   text -> collapsed, which is why this phase exists.
        // - Two different calls returning the same text: different call identity -> both kept.
        // - One call shape run twice returning different text: same identity, different text ->
        //   both kept.
        //
        // Falls back to text when the result names no call in this trace, the only signal left.
        let key = match &block.content {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => tool_use_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .and_then(|id| call_identities.get(&(block.trace_id.as_str(), id)))
                .map(|&call_hash| {
                    DuplicateKey::ToolResultForCall(
                        &block.trace_id,
                        call_hash,
                        hash_tool_result_text(content),
                    )
                })
                .unwrap_or(DuplicateKey::Content(&block.trace_id, &block.content_hash)),
            _ => DuplicateKey::Content(&block.trace_id, &block.content_hash),
        };
        blocks_by_key
            .entry((key, ordinals[idx]))
            .or_default()
            .push(idx);
    }

    let mut to_mark = Vec::new();

    for indices in blocks_by_key.into_values() {
        if indices.len() <= 1 {
            continue;
        }

        // Sort: output-source DESC, uses_span_end DESC, then timestamp ASC
        // Output-source blocks are preferred over input-source copies to ensure
        // Prefer the authoritative output representation (for example, response over request).
        let mut sorted: Vec<_> = indices
            .iter()
            .map(|&i| {
                let is_output = blocks[i].is_output_source();
                let uses_span_end = blocks[i].uses_span_end;
                let effective = effective_timestamp(&blocks[i], span_timestamps);
                (i, is_output, uses_span_end, effective)
            })
            .collect();

        // Between two *produced* copies, a model call's own emission decides before time does: it is
        // the response itself, and an enclosing span - or a later call's accumulated state - that
        // reports the same content re-lists it. Time cannot say which is which: an enclosing span that
        // ends in the same millisecond, or a callback that closes the model call's span after its
        // parent's, put the re-listing first. Only an emission counts, so a generation carrier that may
        // restate earlier observations is left to time like any other re-listing.
        sorted.sort_by(|a, b| {
            let emission_first = || {
                if a.1 && b.1 {
                    is_fresh_emission(&blocks[b.0]).cmp(&is_fresh_emission(&blocks[a.0]))
                } else {
                    std::cmp::Ordering::Equal
                }
            };
            b.1.cmp(&a.1)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(emission_first)
                .then_with(|| a.3.cmp(&b.3))
                .then_with(|| blocks[a.0].origin_rank().cmp(&blocks[b.0].origin_rank()))
                // Last, the span: copies the clock and their kind cannot tell apart are taken in the order
                // of their spans' ids, never of the rows' arrival.
                .then_with(|| blocks[a.0].span_id.cmp(&blocks[b.0].span_id))
        });

        // Keep the original, mark the others.
        let keep = original_copy(blocks, &sorted);
        to_mark.extend(
            sorted
                .into_iter()
                .enumerate()
                .filter(|(position, _)| *position != keep)
                .map(|(_, (idx, _, _, _))| idx),
        );
    }

    to_mark
}

// ============================================================================
// STATISTICS
// ============================================================================

/// Statistics from history detection.
#[derive(Debug, Default)]
pub struct HistoryStats {
    /// Blocks protected from filtering
    pub protected: usize,
    /// Accumulator span input events (history context)
    pub accumulator_history: usize,
    /// Generation span history (session history context)
    pub generation_history: usize,
    /// Assistant history re-sent through input attributes.
    pub input_source_history: usize,
    /// Orphan tool results (tool_use_id not in current set)
    pub orphan_tool_results: usize,
    /// Duplicate content within trace
    pub duplicates: usize,
}

impl HistoryStats {
    /// Total blocks marked as history.
    pub fn total_history(&self) -> usize {
        self.accumulator_history
            + self.generation_history
            + self.input_source_history
            + self.orphan_tool_results
            + self.duplicates
    }
}
#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "history_relisting_tests.rs"]
mod relisting_tests;

#[cfg(test)]
mod duplicate_key_tests {
    use super::*;
    use crate::sideml::types::ContentBlock;

    /// A re-sent result that drops the tool name must still collapse into the original.
    ///
    /// Once the answered call is known, the tool name and the error flag are redundant: they exist
    /// to tell apart results that name no call. Keying on the full block identity here meant a
    /// framework that includes the name on the original and omits it on the re-send produced two
    /// keys for one message, so the feed showed it twice - and dedup could not collapse them
    /// either, because their regenerated ids differ.
    #[test]
    fn a_resend_that_drops_the_tool_name_still_collapses() {
        let text = serde_json::json!([{"type": "text", "text": "ok"}]);
        let named = ContentBlock::ToolResult {
            tool_use_id: Some("call-1".to_string()),
            name: Some("lookup".to_string()),
            content: text.clone(),
            is_error: false,
            provider_executed: false,
        };
        // The re-send: same answer to the same call, with a regenerated id and no tool name.
        let resent = ContentBlock::ToolResult {
            tool_use_id: Some("call-1-regenerated".to_string()),
            name: None,
            content: text.clone(),
            is_error: false,
            provider_executed: false,
        };

        // Their block identities differ, which is correct - that is what tells uncorrelated
        // results apart - but the key this phase uses must not.
        assert_ne!(
            crate::sideml::feed::compute_block_hash(&named),
            crate::sideml::feed::compute_block_hash(&resent),
            "the block identity is expected to include the tool name"
        );
        let key_of = |block: &ContentBlock| match block {
            ContentBlock::ToolResult { content, .. } => hash_tool_result_text(content),
            _ => unreachable!(),
        };
        assert_eq!(
            key_of(&named),
            key_of(&resent),
            "a re-send of one call's result must collapse whatever metadata it drops"
        );

        // The text still separates two different answers to the same call.
        let different = ContentBlock::ToolResult {
            tool_use_id: Some("call-1".to_string()),
            name: Some("lookup".to_string()),
            content: serde_json::json!([{"type": "text", "text": "failed"}]),
            is_error: false,
            provider_executed: false,
        };
        assert_ne!(key_of(&named), key_of(&different));
    }

    /// Two results that name no call keep the name and error flag in their identity, so a success
    /// and a failure with matching text stay distinct.
    #[test]
    fn uncorrelated_results_are_separated_by_name_and_error() {
        let content = serde_json::json!([{"type": "text", "text": "ok"}]);
        let hash = |name: Option<&str>, is_error: bool| {
            crate::sideml::feed::compute_block_hash(&ContentBlock::ToolResult {
                tool_use_id: None,
                name: name.map(str::to_owned),
                content: content.clone(),
                is_error,
                provider_executed: false,
            })
        };
        assert_ne!(hash(Some("lookup"), false), hash(Some("write"), false));
        assert_ne!(hash(Some("lookup"), false), hash(Some("lookup"), true));
        assert_eq!(hash(Some("lookup"), false), hash(Some("lookup"), false));
    }
}
