use super::*;
use crate::sideml::provenance::PositionPath;
use crate::sideml::types::FinishReason;
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
        },
        "tool_result" => ContentBlock::ToolResult {
            tool_use_id: Some("call_1".to_string()),
            name: None,
            content: serde_json::json!("result"),
            is_error: false,
        },
        _ => ContentBlock::Text {
            text: "test".to_string(),
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
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
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
            name: None,
            content: serde_json::json!("result"),
            is_error: false,
        },
        _ => ContentBlock::Text {
            text: format!("test_{counter}"),
        },
    };

    BlockEntry {
        scope_version: None,
        span_name: None,
        scope_name: None,
        position: PositionPath::default(),
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
        order_time: Utc::now(),
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
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
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

// Input-source assistant history

#[test]
fn marks_input_source_assistant_history() {
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
fn preserves_output_source_assistant() {
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
fn preserves_protected_input_source_blocks() {
    // Protected blocks, such as finish reasons, are authoritative.
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
        "protected input-source block should remain current"
    );
}

#[test]
fn preserves_input_source_user_messages() {
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
        "input-source user messages are current-turn prompts"
    );
}

#[test]
fn preserves_root_span_input_source_assistant() {
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

    assert!(!blocks[0].is_history, "the root span is authoritative");
}

#[test]
fn leaves_event_sources_to_event_history_detection() {
    // Event-sourced blocks are handled by event history detection.
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
        "event sources should not count as input-attribute history"
    );
}

// Orphan tool results in multi-turn history

/// Reproduces the Strands JS bug where historical tool_use events bubble up
/// to the root agent span, causing their tool_use_ids to be collected as
/// "current" and preventing orphan detection from marking their tool results as history.
///
/// Scenario: trace has NYC turn (history) + London turn (current).
/// The root agent span has both NYC and London tool_use events (due to bubbling).
/// Only the London tool_use appears in gen_ai.choice → only London is "current".
/// NYC tool_result should be orphan (its tool_use_id not in gen_ai.choice).
#[test]
fn historical_tool_results_are_orphans_despite_bubbled_agent_span() {
    // Simulate: root agent span has NYC tool_use (historical, bubbled) +
    //           London gen_ai.choice with London tool_use (current/protected)
    // execute_agent_loop_cycle has NYC tool_result + London tool_result

    let nyc_tool_use_id = "tooluse_NYC_historical";
    let london_tool_use_id = "tooluse_London_current";

    // Protected gen_ai.choice on root agent span — contains London tool_use
    let mut choice_block = make_block_with_source(
        "tool_use",
        Some("agent"),
        Some("gen_ai.choice"),
        "event",
        MessageCategory::GenAIChoice,
        ChatRole::Assistant,
    );
    if let ContentBlock::ToolUse { id, name, .. } = &mut choice_block.content {
        *id = Some(london_tool_use_id.to_string());
        *name = "weather_forecast".to_string();
    }
    choice_block.parent_span_id = None; // root span
    choice_block.finish_reason = Some(FinishReason::ToolUse);

    // Historical NYC tool_use on root agent span (bubbled, NOT protected)
    let mut nyc_tool_use = make_block_with_source(
        "tool_use",
        Some("agent"),
        Some("gen_ai.assistant.message"),
        "event",
        MessageCategory::GenAIAssistantMessage,
        ChatRole::Assistant,
    );
    if let ContentBlock::ToolUse { id, name, .. } = &mut nyc_tool_use.content {
        *id = Some(nyc_tool_use_id.to_string());
        *name = "weather_forecast".to_string();
    }
    nyc_tool_use.parent_span_id = None; // root span

    // NYC tool_result on execute_agent_loop_cycle (non-root agent span, multi-turn history)
    let mut nyc_tool_result = make_block_with_source(
        "tool_result",
        Some("agent"),
        Some("gen_ai.tool.message"),
        "event",
        MessageCategory::GenAIToolMessage,
        ChatRole::Tool,
    );
    if let ContentBlock::ToolResult { tool_use_id, .. } = &mut nyc_tool_result.content {
        *tool_use_id = Some(nyc_tool_use_id.to_string());
    }
    nyc_tool_result.parent_span_id = Some("root_agent".to_string()); // non-root

    // London tool_result on execute_agent_loop_cycle (non-root agent span, current turn)
    let mut london_tool_result = make_block_with_source(
        "tool_result",
        Some("agent"),
        Some("gen_ai.tool.message"),
        "event",
        MessageCategory::GenAIToolMessage,
        ChatRole::Tool,
    );
    if let ContentBlock::ToolResult { tool_use_id, .. } = &mut london_tool_result.content {
        *tool_use_id = Some(london_tool_use_id.to_string());
    }
    london_tool_result.parent_span_id = Some("root_agent".to_string()); // non-root

    // Tool_result in generation span (makes traces_with_multi_turn_history fire)
    let mut gen_tool_result = make_block_with_source(
        "tool_result",
        Some("generation"),
        Some("gen_ai.tool.message"),
        "event",
        MessageCategory::GenAIToolMessage,
        ChatRole::Tool,
    );
    if let ContentBlock::ToolResult { tool_use_id, .. } = &mut gen_tool_result.content {
        *tool_use_id = Some(nyc_tool_use_id.to_string());
    }
    gen_tool_result.parent_span_id = Some("exec_loop".to_string());

    // Gen_ai.choice event on generation span (bubbled) — enables has_event_based_messages
    let mut gen_choice = make_block_with_source(
        "text",
        Some("generation"),
        Some("gen_ai.choice"),
        "event",
        MessageCategory::GenAIChoice,
        ChatRole::Assistant,
    );
    gen_choice.parent_span_id = Some("exec_loop".to_string());
    gen_choice.finish_reason = Some(FinishReason::Stop);

    let mut blocks = vec![
        choice_block,
        nyc_tool_use,
        nyc_tool_result,
        london_tool_result,
        gen_tool_result,
        gen_choice,
    ];

    let span_timestamps = HashMap::new();
    let stats = mark_history(&mut blocks, &span_timestamps);

    // NYC tool_result (index 2) should be orphan — its tool_use_id not in gen_ai.choice
    assert!(
        blocks[2].is_history,
        "NYC tool_result should be marked as orphan (historical tool_use_id not in gen_ai.choice)"
    );

    // London tool_result (index 3) should NOT be orphan — London ID is in gen_ai.choice
    assert!(
        !blocks[3].is_history,
        "London tool_result should NOT be orphan (tool_use_id IS in gen_ai.choice)"
    );

    assert!(stats.orphan_tool_results >= 1, "at least 1 orphan expected");
}
