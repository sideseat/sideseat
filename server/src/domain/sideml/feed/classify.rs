//! Block Classification for Feed Pipeline
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
    // Without this, effective_time = span_start, same as user input → wrong sort order
    if block.is_json_block() && block.is_output_source() {
        return true;
    }

    // Everything else uses event_time
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::types::MessageCategory;
    use crate::domain::sideml::types::{ChatRole, ContentBlock, FinishReason};
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
}
