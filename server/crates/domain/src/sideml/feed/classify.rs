//! Block classification for the feed pipeline.
//!
//! This module determines the timestamp strategy for each block,
//! which affects how blocks are ordered in the final output.
//!
//! # Timestamp Strategy
//!
//! Blocks can use two timestamp strategies for ordering:
//!
//! | Strategy | When | Effective Time |
//! |----------|------|----------------|
//! | span_end | Completion events | When operation finished |
//! | event_time | Everything else | When event was recorded |
//!
//! # Why This Matters
//!
//! Consider messages in a single generation span:
//! - ToolUse at event_time T=100 (LLM decided to call tool)
//! - ToolResult at event_time T=200 (tool executed)
//! - FinalText at event_time T=300, span_end=300
//!
//! If ToolUse used span_end (T=300), it would sort AFTER ToolResult (T=200).
//! By using event_time (T=100), ToolUse correctly sorts before ToolResult.
//!
//! # Classification Rules
//!
//! **Use span_end (uses_span_end=true):**
//! - gen_ai.choice events (generation completed)
//! - gen_ai.content.completion events
//! - GenAIChoice category (attribute-based completion)
//! - Blocks with finish_reason
//! - Tool results from tool spans (execution completed)
//!
//! **Use event_time (uses_span_end=false):**
//! - Tool use (decision made during generation, not at end)
//! - Intermediate text (streaming, no finish_reason)
//! - Input messages (user, system)
//! - Tool results from non-tool spans

use super::types::BlockEntry;
use crate::sideml::types::ChatRole;

/// Determine if a block should use span_end for effective timestamp.
///
/// Returns `true` for completion events (use span_end).
/// Returns `false` for intermediate/input events (use event_time).
///
/// # Important: ToolUse always uses event_time
///
/// ToolUse blocks represent a decision made DURING generation, not at completion.
/// Even when contained in a gen_ai.choice event (which indicates the generation
/// completed), the ToolUse itself happened at the event's timestamp, not at span_end.
///
/// Example: Generation span produces ToolUse (T=100) → ToolResult (T=200)
/// - If ToolUse used span_end (T=200), it would sort AFTER ToolResult
/// - By using event_time (T=100), ToolUse correctly sorts BEFORE ToolResult
pub fn uses_span_end(block: &BlockEntry) -> bool {
    // ToolUse always uses event_time, even from gen_ai.choice events
    // The decision to call a tool happens DURING generation, not at the end
    if block.is_tool_use() {
        return false;
    }

    // Protected blocks (output events, choice category, finish_reason) use span_end
    if block.is_protected() {
        return true;
    }

    // Tool result from tool span (execution completed)
    if block.is_tool_result() && block.is_tool_span() {
        return true;
    }

    // JSON structured output from output sources (e.g. output.value on root spans)
    // Without this, effective_time = span_start, same as user input → wrong sort order.
    // A user's or system's JSON in an output carrier that restates earlier observations is the input
    // re-listed in a state snapshot - an attachment the request carried - and was given, not produced,
    // so it keeps its own time. In a carrier that only emits, the role stands as stated.
    if block.is_json_block() && block.is_output_source() && !is_restated_input(block) {
        return true;
    }

    // Everything else uses event_time
    false
}

fn is_restated_input(block: &BlockEntry) -> bool {
    matches!(block.role, ChatRole::User | ChatRole::System)
        && crate::sideml::carrier::semantics_for_context(&block.carrier_context())
            .may_restate_prior_observations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sideml::provenance::PositionPath;
    use crate::sideml::types::{ChatRole, ContentBlock, FinishReason};
    use chrono::Utc;
    use sideseat_ports::types::MessageCategory;

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
                provider_executed: false,
            },
            "tool_result" => ContentBlock::ToolResult {
                tool_use_id: Some("call_1".to_string()),
                name: None,
                content: serde_json::json!("result"),
                is_error: false,
                provider_executed: false,
            },
            _ => ContentBlock::Text {
                text: "test".to_string(),
                citations: Vec::new(),
            },
        };

        BlockEntry {
            scope_version: None,
            span_name: None,
            scope_name: None,
            position: PositionPath::default(),
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
            order_time: Utc::now(),
            occurrence_ordinal: 0,
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
            is_cross_trace_history: false,
            tool_use_id_correlated: false,
            promoted_to_span_output: false,
            is_rendering: false,
            declared_direction: None,
        }
    }

    #[test]
    fn test_gen_ai_choice_uses_span_end() {
        let block = make_block(
            "text",
            Some("generation"),
            Some("gen_ai.choice"),
            MessageCategory::GenAIChoice,
            Some(FinishReason::Stop),
        );
        assert!(uses_span_end(&block));
    }

    #[test]
    fn test_tool_use_uses_event_time() {
        let block = make_block(
            "tool_use",
            Some("generation"),
            None,
            MessageCategory::GenAIAssistantMessage,
            None,
        );
        assert!(!uses_span_end(&block));
    }

    #[test]
    fn test_tool_result_from_tool_span_uses_span_end() {
        let block = make_block(
            "tool_result",
            Some("tool"),
            None,
            MessageCategory::GenAIToolMessage,
            None,
        );
        assert!(uses_span_end(&block));
    }

    #[test]
    fn test_tool_result_from_generation_span_uses_event_time() {
        let block = make_block(
            "tool_result",
            Some("generation"),
            None,
            MessageCategory::GenAIToolMessage,
            None,
        );
        assert!(!uses_span_end(&block));
    }

    #[test]
    fn test_intermediate_text_uses_event_time() {
        let block = make_block(
            "text",
            Some("generation"),
            None,
            MessageCategory::GenAIAssistantMessage,
            None,
        );
        assert!(!uses_span_end(&block));
    }

    #[test]
    fn test_finish_reason_uses_span_end() {
        let block = make_block(
            "text",
            Some("generation"),
            None,
            MessageCategory::GenAIAssistantMessage,
            Some(FinishReason::Stop),
        );
        assert!(uses_span_end(&block));
    }

    #[test]
    fn test_json_output_uses_span_end() {
        let mut block = make_block(
            "json",
            Some("span"),
            None,
            MessageCategory::GenAIAssistantMessage,
            None,
        );
        block.content = ContentBlock::Json {
            data: serde_json::json!({"name": "Jane"}),
        };
        block.source_type = "attribute".to_string();
        block.source_attribute = Some("output.value".to_string());
        assert!(uses_span_end(&block));
    }

    #[test]
    fn test_json_input_uses_event_time() {
        let mut block = make_block(
            "json",
            Some("span"),
            None,
            MessageCategory::GenAIUserMessage,
            None,
        );
        block.content = ContentBlock::Json {
            data: serde_json::json!({"name": "Jane"}),
        };
        block.source_type = "attribute".to_string();
        block.source_attribute = Some("input.value".to_string());
        assert!(!uses_span_end(&block));
    }

    /// A user's attachment re-listed in an output snapshot was given to the span, not produced by it,
    /// so it keeps its own time instead of sorting after the answer at the span's end.
    #[test]
    fn test_user_json_in_an_output_snapshot_uses_event_time() {
        let mut block = make_block(
            "json",
            Some("agent"),
            None,
            MessageCategory::GenAIUserMessage,
            None,
        );
        block.role = ChatRole::User;
        block.content = ContentBlock::Json {
            data: serde_json::json!({"file": {"filename": "task.pdf"}}),
        };
        block.source_type = "attribute".to_string();
        block.source_attribute = Some("output.value".to_string());
        assert!(block.is_output_source());
        assert!(!uses_span_end(&block));
    }

    /// In a carrier that only emits, a stated role does not make the block a restatement.
    #[test]
    fn test_user_json_in_an_emission_uses_span_end() {
        let mut block = make_block(
            "json",
            Some("generation"),
            None,
            MessageCategory::GenAIUserMessage,
            None,
        );
        block.role = ChatRole::User;
        block.content = ContentBlock::Json {
            data: serde_json::json!({"name": "Jane"}),
        };
        block.source_type = "attribute".to_string();
        block.source_attribute = Some("llm.output_messages.0.message".to_string());
        assert!(block.is_output_source());
        assert!(uses_span_end(&block));
    }

    /// `output.value` on a model call is that call's ordered response; elsewhere it is framework state
    /// whose re-listed positions say nothing.
    #[test]
    fn a_model_calls_output_value_orders_parts_even_when_restated() {
        let semantics = |observation_type| {
            crate::sideml::carrier::semantics_for_context(&crate::rules::CarrierContext {
                event: None,
                attribute: Some("output.value"),
                observation_type: Some(observation_type),
                span_name: None,
                direction: None,
            })
        };
        assert!(semantics("generation").history_positions_provide_sequence_order);
        assert!(semantics("generation").may_restate_prior_observations);
        assert!(!semantics("chain").history_positions_provide_sequence_order);
    }
}
