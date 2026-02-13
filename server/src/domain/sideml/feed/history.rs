//! History Detection for Feed Pipeline
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
//! # Eight-Phase Detection
//!
//! 1. **Build current tool_use_id set**: From protected tool_uses and agent spans
//! 2. **Timestamp-based**: Mark messages with timestamp < span_start
//! 3. **Accumulator span input**: Mark input events from non-root accumulator spans
//! 4. **Intermediate text**: Mark assistant text from generation spans (when has_agent_spans)
//!    - **(4b) Input-source assistant**: Mark assistant from input attrs in non-root gen spans
//! 5. **Multi-turn history**: Mark all unprotected content in generation spans with tool_results
//! 6. **Orphan tool_results**: Mark tool_results with unknown tool_use_id
//! 7. **Deduplication**: Mark duplicate content by identity (keep earliest)

use std::collections::{HashMap, HashSet};

use super::dedup::{SpanTimestamps, effective_timestamp};
use super::types::BlockEntry;
use crate::domain::sideml::types::{ChatRole, ContentBlock};

// ============================================================================
// TOOL USE ID MAP
// ============================================================================

/// Build a map of tool_use_ids to their "current" status, **per trace**.
///
/// A tool_use is "current" (not history) if:
/// 1. It's protected (GenAIChoice, finish_reason)
/// 2. It's in an agent span (authoritative)
///
/// Returns: Map of trace_id -> Set of tool_use_ids that are current (not history)
///
/// IMPORTANT: This must be per-trace to avoid cross-trace contamination when
/// processing sessions. Tool_use_ids from previous traces should not be
/// considered "current" for subsequent traces.
fn build_current_tool_use_ids(blocks: &[BlockEntry]) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();

    for block in blocks {
        // Only protected tool_uses or agent span tool_uses are current
        if !block.is_protected() && !block.is_agent_span() {
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
    /// When true, Phase 4 intermediate filtering applies (events bubble up)
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

/// Mark blocks as history based on universal signals.
///
/// # Algorithm
///
/// 1. Build set of current tool_use_ids (from protected blocks and agent spans)
/// 2. Timestamp-based: Mark messages with timestamp < span_start
/// 3. Accumulator spans: Mark input events from non-root accumulator spans
/// 4. Intermediate text: Mark assistant text from generation spans (when has_agent_spans)
/// 5. Multi-turn: If tool_results in generation, mark all unprotected generation content
/// 6. Orphan tool_results: Mark tool_results with unknown tool_use_id
/// 7. Deduplicate remaining blocks
pub fn mark_history(
    blocks: &mut [BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> HistoryStats {
    let mut stats = HistoryStats {
        protected: blocks.iter().filter(|b| b.is_protected()).count(),
        ..Default::default()
    };

    // Phase 1: Detect session history and build tool_use_id map
    let current_tool_ids = build_current_tool_use_ids(blocks);
    let history_info = detect_session_history(blocks);

    tracing::trace!(
        current_tool_ids = current_tool_ids.len(),
        has_agent_spans = history_info.has_agent_spans,
        has_event_based = history_info.has_event_based_messages,
        traces_with_multi_turn = history_info.traces_with_multi_turn_history.len(),
        "history detection: analysis complete"
    );

    // Phase 2: Mark timestamp-based history in child generation spans
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

    // Phase 3: Filter intermediate state from spans
    //
    // This phase handles clear intermediate state that should be filtered:
    // 1. Raw JSON output from chain spans (framework state) - even root
    // 2. Input events from non-root accumulator spans (context copies)
    //
    // We DON'T aggressively filter all input-source content because:
    // - Phase 2 (timestamp) already catches messages predating the span
    // - Phase 7 (dedup) catches duplicate content
    // - Some input sources contain unique authoritative messages
    for block in blocks.iter_mut() {
        if block.is_protected() || block.is_history {
            continue;
        }

        // Tool results from execution should be kept (unless orphan - handled in Phase 6)
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

        // Root span content is generally authoritative (except JSON handled above)
        if block.is_root_span() {
            continue;
        }

        // Accumulator spans (agent/chain/span) pass through messages
        // Their input events are context copies, not authoritative
        if block.is_accumulator_span() && block.is_input_event() {
            block.is_history = true;
            stats.accumulator_history += 1;
        }
    }

    // Phase 4: Event-based framework intermediate content filtering
    //
    // For frameworks using OTEL events (gen_ai.choice, gen_ai.user.message):
    // - Events bubble up from child spans to root agent span
    // - Root agent span has authoritative current-turn messages
    // - Child generation span content is intermediate, duplicated at root
    //
    // This phase only applies when BOTH conditions are true:
    // - has_agent_spans: Root span is an agent that collects events
    // - has_event_based_messages: Framework uses gen_ai.* events
    //
    // For attribute-based frameworks (LangGraph, OpenInference):
    // - Generation spans have authoritative output in llm.output_messages
    // - No event bubbling, child generation spans ARE the source of truth
    // - This phase is SKIPPED
    if history_info.has_agent_spans && history_info.has_event_based_messages {
        for block in blocks.iter_mut() {
            if block.is_protected() || block.is_history {
                continue;
            }

            // Only non-root generation spans
            if !block.is_generation_span() || block.is_root_span() {
                continue;
            }

            // Filter based on role (both event and attribute sources are intermediate)
            match block.role {
                // User/System in child generation spans = history context copies
                ChatRole::User | ChatRole::System => {
                    block.is_history = true;
                    stats.generation_history += 1;
                }
                // Assistant text/thinking = intermediate output (final at root)
                ChatRole::Assistant if block.is_text() || block.is_thinking() => {
                    block.is_history = true;
                    stats.generation_history += 1;
                }
                // Tool role and ToolUse preserved for matching
                _ => {}
            }
        }
    }

    // Phase 4b: Input-source assistant history
    //
    // For attribute-based frameworks (ADK, Vercel, LiveKit, etc.):
    // A non-root generation span's INPUT attributes (e.g. llm_request) re-send
    // previous assistant responses as context. The current response comes from
    // OUTPUT attributes (e.g. llm_response / gen_ai.choice).
    //
    // This phase marks assistant content from input sources in non-root generation
    // spans as history. It catches re-sent assistant text/thinking/tool_use that
    // Phase 7 (hash dedup) misses because the LLM regenerates different text.
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

    // Phase 5: Multi-turn history - filter ALL unprotected generation span content
    // When tool_results exist in generation spans, it indicates full history re-send
    // IMPORTANT: Check per-trace to avoid cross-trace contamination
    for block in blocks.iter_mut() {
        if block.is_protected() || block.is_history {
            continue;
        }

        if !block.is_generation_span() {
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

    // Phase 6: Mark orphan tool_results
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

        // Only applies to tool_results with tool_use_id
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

    // Phase 7: Deduplicate remaining blocks
    let duplicate_indices = find_duplicate_indices(blocks, span_timestamps);
    for idx in duplicate_indices {
        blocks[idx].is_history = true;
        stats.duplicates += 1;
    }

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

/// Find indices of duplicate blocks that should be marked as history.
fn find_duplicate_indices(
    blocks: &[BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> Vec<usize> {
    let mut blocks_by_key: HashMap<(&str, &str), Vec<usize>> = HashMap::new();

    for (idx, block) in blocks.iter().enumerate() {
        if block.is_protected() || block.is_history {
            continue;
        }
        blocks_by_key
            .entry((&block.trace_id, &block.content_hash))
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
        // Phase 7 keeps the authoritative version (e.g. llm_response over llm_request)
        let mut sorted: Vec<_> = indices
            .iter()
            .map(|&i| {
                let is_output = blocks[i].is_output_source();
                let uses_span_end = blocks[i].uses_span_end;
                let effective = effective_timestamp(&blocks[i], span_timestamps);
                (i, is_output, uses_span_end, effective)
            })
            .collect();

        sorted.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| a.3.cmp(&b.3))
        });

        // Keep first (best), mark others
        to_mark.extend(sorted.into_iter().skip(1).map(|(idx, _, _, _)| idx));
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
    /// Input-source assistant history (Phase 4b)
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
mod tests {
    use super::*;
    use crate::data::types::MessageCategory;
    use crate::domain::sideml::types::FinishReason;
    use chrono::Utc;

    fn make_block(
        entry_type: &str,
        observation_type: Option<&str>,
        event_name: Option<&str>,
        category: MessageCategory,
        finish_reason: Option<FinishReason>,
    ) -> BlockEntry {
        let content = match entry_type {
            "tool_use" => ContentBlock::ToolUse {
                id: Some("call_1".to_string()),
                name: "test".to_string(),
                input: serde_json::json!({}),
            },
            "tool_result" => ContentBlock::ToolResult {
                tool_use_id: Some("call_1".to_string()),
                content: serde_json::json!("result"),
                is_error: false,
            },
            _ => ContentBlock::Text {
                text: "test".to_string(),
            },
        };

        BlockEntry {
            entry_type: entry_type.to_string(),
            content,
            role: ChatRole::Assistant,
            trace_id: "trace1".to_string(),
            span_id: "span1".to_string(),
            session_id: None,
            message_index: 0,
            entry_index: 0,
            parent_span_id: Some("parent".to_string()),
            span_path: vec!["span1".to_string()],
            timestamp: Utc::now(),
            observation_type: observation_type.map(String::from),
            model: None,
            provider: None,
            name: None,
            finish_reason,
            tool_use_id: None,
            tool_name: None,
            tokens: None,
            cost: None,
            status_code: None,
            is_error: false,
            source_type: "event".to_string(),
            event_name: event_name.map(String::from),
            source_attribute: None,
            category,
            content_hash: "hash".to_string(),
            is_semantic: true,
            uses_span_end: false,
            is_history: false,
        }
    }

    #[test]
    fn test_gen_ai_choice_is_protected() {
        let block = make_block(
            "text",
            Some("generation"),
            Some("gen_ai.choice"),
            MessageCategory::GenAIChoice,
            Some(FinishReason::Stop),
        );
        assert!(block.is_protected());
    }

    #[test]
    fn test_finish_reason_is_protected() {
        let block = make_block(
            "text",
            Some("generation"),
            None,
            MessageCategory::GenAIAssistantMessage,
            Some(FinishReason::Stop),
        );
        assert!(block.is_protected());
    }

    #[test]
    fn test_intermediate_text_is_not_protected() {
        let block = make_block(
            "text",
            Some("generation"),
            None,
            MessageCategory::GenAIAssistantMessage,
            None,
        );
        assert!(!block.is_protected());
    }

    use std::sync::atomic::{AtomicU32, Ordering};
    static BLOCK_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn make_block_with_source(
        entry_type: &str,
        observation_type: Option<&str>,
        event_name: Option<&str>,
        source_type: &str,
        category: MessageCategory,
        role: ChatRole,
    ) -> BlockEntry {
        let counter = BLOCK_COUNTER.fetch_add(1, Ordering::SeqCst);
        let content = match entry_type {
            "tool_use" => ContentBlock::ToolUse {
                id: Some(format!("call_{counter}")),
                name: "test".to_string(),
                input: serde_json::json!({}),
            },
            "tool_result" => ContentBlock::ToolResult {
                tool_use_id: Some(format!("call_{counter}")),
                content: serde_json::json!("result"),
                is_error: false,
            },
            _ => ContentBlock::Text {
                text: format!("test_{counter}"),
            },
        };

        BlockEntry {
            entry_type: entry_type.to_string(),
            content,
            role,
            trace_id: "trace1".to_string(),
            span_id: format!("span_{counter}"),
            session_id: None,
            message_index: 0,
            entry_index: 0,
            parent_span_id: Some("parent".to_string()),
            span_path: vec![format!("span_{counter}")],
            timestamp: Utc::now(),
            observation_type: observation_type.map(String::from),
            model: None,
            provider: None,
            name: None,
            finish_reason: None,
            tool_use_id: None,
            tool_name: None,
            tokens: None,
            cost: None,
            status_code: None,
            is_error: false,
            source_type: source_type.to_string(),
            event_name: event_name.map(String::from),
            source_attribute: None,
            category,
            content_hash: format!("hash_{counter}"),
            is_semantic: true,
            uses_span_end: false,
            is_history: false,
        }
    }

    #[test]
    fn test_detect_event_based_strands() {
        // Strands pattern: has agent spans + event-based messages (gen_ai.choice)
        let blocks = vec![
            make_block_with_source(
                "text",
                Some("agent"),
                None,
                "attribute",
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
            ),
            make_block_with_source(
                "text",
                Some("generation"),
                Some("gen_ai.choice"),
                "event",
                MessageCategory::GenAIChoice,
                ChatRole::Assistant,
            ),
        ];
        let info = detect_session_history(&blocks);
        assert!(info.has_agent_spans);
        assert!(info.has_event_based_messages);
    }

    #[test]
    fn test_detect_attribute_based_langgraph() {
        // LangGraph pattern: has agent spans + attribute-based messages (no gen_ai.* events)
        let blocks = vec![
            make_block_with_source(
                "text",
                Some("agent"),
                None,
                "attribute",
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
            ),
            make_block_with_source(
                "text",
                Some("generation"),
                None, // no event name - from llm.output_messages attribute
                "attribute",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            ),
        ];
        let info = detect_session_history(&blocks);
        assert!(info.has_agent_spans);
        assert!(!info.has_event_based_messages); // key difference: no event-based messages
    }

    #[test]
    fn test_langgraph_assistant_text_not_marked_history() {
        // LangGraph: assistant text from generation span should NOT be marked as history
        // because it's the actual LLM output, not intermediate
        let mut blocks = vec![
            make_block_with_source(
                "text",
                Some("agent"),
                None,
                "attribute",
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
            ),
            make_block_with_source(
                "text",
                Some("generation"),
                None, // attribute-based, no event
                "attribute",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            ),
        ];

        // Verify setup: should have agent spans but no event-based messages
        let info = detect_session_history(&blocks);
        assert!(info.has_agent_spans, "should have agent spans");
        assert!(
            !info.has_event_based_messages,
            "should NOT have event-based messages for LangGraph"
        );

        let span_timestamps = HashMap::new();
        mark_history(&mut blocks, &span_timestamps);

        // User message should not be history
        assert!(!blocks[0].is_history, "user message should not be history");
        // Assistant text from generation span should NOT be history in LangGraph
        assert!(
            !blocks[1].is_history,
            "LangGraph assistant text should not be marked as history"
        );
    }

    #[test]
    fn test_strands_assistant_text_marked_history() {
        // Strands: assistant text from non-root generation span IS marked as history
        // because it's intermediate output that bubbles up via events
        let mut blocks = vec![
            make_block_with_source(
                "text",
                Some("agent"),
                None,
                "attribute",
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
            ),
            make_block_with_source(
                "text",
                Some("generation"),
                Some("gen_ai.user.message"), // event-based input
                "event",
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
            ),
            make_block_with_source(
                "text",
                Some("generation"),
                None, // assistant text without protection
                "attribute",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            ),
        ];

        let span_timestamps = HashMap::new();
        mark_history(&mut blocks, &span_timestamps);

        // Assistant text from generation span IS history in Strands (events bubble up)
        assert!(
            blocks[2].is_history,
            "Strands intermediate assistant text should be marked as history"
        );
    }

    #[test]
    fn test_langgraph_chain_span_json_filtered() {
        // LangGraph "tools" chain node output (JSON with tool results) should be filtered
        // because actual semantic tool_results come from generation spans
        let mut blocks = vec![
            make_block_with_source(
                "text",
                Some("chain"), // root chain span
                None,
                "attribute",
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
            ),
            {
                // Non-root chain span JSON output (LangGraph "tools" node)
                let mut b = make_block_with_source(
                    "json", // raw state output
                    Some("chain"),
                    None,
                    "attribute",
                    MessageCategory::GenAIAssistantMessage,
                    ChatRole::Assistant,
                );
                b.parent_span_id = Some("parent".to_string()); // non-root
                b
            },
            make_block_with_source(
                "tool_result",
                Some("generation"),
                None,
                "attribute",
                MessageCategory::GenAIToolMessage,
                ChatRole::Tool,
            ),
        ];

        // Make first block root span (no parent)
        blocks[0].parent_span_id = None;

        let span_timestamps = HashMap::new();
        mark_history(&mut blocks, &span_timestamps);

        // User message from root should not be history
        assert!(
            !blocks[0].is_history,
            "root user message should not be history"
        );
        // JSON from non-root chain span should be history
        assert!(
            blocks[1].is_history,
            "non-root chain span JSON should be marked as history"
        );
        // Tool result from generation span should not be history
        assert!(!blocks[2].is_history, "tool result should not be history");
    }

    #[test]
    fn test_chain_span_json_filtered_even_root() {
        // JSON output from chain spans should be filtered (framework state)
        // This includes ROOT chain spans because LangGraph root span output.value
        // contains raw graph state, not semantic messages
        let mut blocks = vec![{
            let mut b = make_block_with_source(
                "json",
                Some("chain"),
                None,
                "attribute",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            );
            b.parent_span_id = None; // root span
            b
        }];

        let span_timestamps = HashMap::new();
        mark_history(&mut blocks, &span_timestamps);

        // Chain span JSON should be history even if root
        assert!(
            blocks[0].is_history,
            "chain span JSON should be marked as history even when root"
        );
    }

    #[test]
    fn test_root_agent_span_text_preserved() {
        // Text output from ROOT agent span should be preserved (actual response)
        let mut blocks = vec![{
            let mut b = make_block_with_source(
                "text",
                Some("agent"),
                None,
                "attribute",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            );
            b.parent_span_id = None; // root span
            b
        }];

        let span_timestamps = HashMap::new();
        mark_history(&mut blocks, &span_timestamps);

        // Root agent span text should not be history
        assert!(
            !blocks[0].is_history,
            "root agent span text should not be marked as history"
        );
    }

    // ========================================================================
    // PHASE 4b: INPUT-SOURCE ASSISTANT HISTORY TESTS
    // ========================================================================

    #[test]
    fn test_phase4b_marks_input_source_assistant() {
        // ADK pattern: assistant text from llm_request (input) in non-root gen span
        let mut blocks = vec![{
            let mut b = make_block_with_source(
                "text",
                Some("generation"),
                None,
                "attribute",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            );
            b.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
            b.parent_span_id = Some("agent_root".to_string());
            b
        }];

        let span_timestamps = HashMap::new();
        let stats = mark_history(&mut blocks, &span_timestamps);

        assert!(
            blocks[0].is_history,
            "input-source assistant should be history"
        );
        assert_eq!(stats.input_source_history, 1);
    }

    #[test]
    fn test_phase4b_skips_output_source_assistant() {
        // Assistant text from llm_response (output) should NOT be marked
        let mut blocks = vec![{
            let mut b = make_block_with_source(
                "text",
                Some("generation"),
                None,
                "attribute",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            );
            b.source_attribute = Some("gcp.vertex.agent.llm_response".to_string());
            b.parent_span_id = Some("agent_root".to_string());
            b
        }];

        let span_timestamps = HashMap::new();
        let stats = mark_history(&mut blocks, &span_timestamps);

        assert!(
            !blocks[0].is_history,
            "output-source assistant should NOT be history"
        );
        assert_eq!(stats.input_source_history, 0);
    }

    #[test]
    fn test_phase4b_skips_protected_blocks() {
        // Protected blocks (finish_reason) should never be marked by Phase 4b
        let mut blocks = vec![{
            let mut b = make_block_with_source(
                "text",
                Some("generation"),
                None,
                "attribute",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            );
            b.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
            b.parent_span_id = Some("agent_root".to_string());
            b.finish_reason = Some(FinishReason::Stop);
            b
        }];

        let span_timestamps = HashMap::new();
        mark_history(&mut blocks, &span_timestamps);

        assert!(
            !blocks[0].is_history,
            "protected block should NOT be marked by Phase 4b"
        );
    }

    #[test]
    fn test_phase4b_skips_user_role() {
        // User messages from input source should NOT be marked (they're current turn prompts)
        let mut blocks = vec![{
            let mut b = make_block_with_source(
                "text",
                Some("generation"),
                None,
                "attribute",
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
            );
            b.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
            b.parent_span_id = Some("agent_root".to_string());
            b
        }];

        let span_timestamps = HashMap::new();
        mark_history(&mut blocks, &span_timestamps);

        assert!(
            !blocks[0].is_history,
            "user role should NOT be marked by Phase 4b"
        );
    }

    #[test]
    fn test_phase4b_skips_root_span() {
        // Root span input-source assistant should NOT be marked (root is authoritative)
        let mut blocks = vec![{
            let mut b = make_block_with_source(
                "text",
                Some("generation"),
                None,
                "attribute",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            );
            b.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
            b.parent_span_id = None; // root span
            b
        }];

        let span_timestamps = HashMap::new();
        mark_history(&mut blocks, &span_timestamps);

        assert!(
            !blocks[0].is_history,
            "root span should NOT be marked by Phase 4b"
        );
    }

    #[test]
    fn test_phase4b_skips_event_source() {
        // Event-sourced blocks are handled by Phase 4, not 4b
        let mut blocks = vec![{
            let mut b = make_block_with_source(
                "text",
                Some("generation"),
                Some("gen_ai.assistant.message"),
                "event",
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
            );
            b.parent_span_id = Some("agent_root".to_string());
            b
        }];

        let span_timestamps = HashMap::new();
        let stats = mark_history(&mut blocks, &span_timestamps);

        assert_eq!(
            stats.input_source_history, 0,
            "event source should not be counted in Phase 4b"
        );
    }
}
