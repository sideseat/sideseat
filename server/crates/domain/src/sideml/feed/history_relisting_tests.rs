use super::*;
use crate::sideml::provenance::PositionPath;
use chrono::Utc;
use sideseat_ports::types::MessageCategory;

fn call(span: &str, observation_type: &str, attribute: &str, id: Option<&str>) -> BlockEntry {
    BlockEntry {
        scope_version: None,
        span_name: None,
        scope_name: None,
        position: PositionPath::default(),
        entry_type: "tool_use".to_string(),
        content: ContentBlock::ToolUse {
            id: id.map(str::to_string),
            name: "get_weather".to_string(),
            input: serde_json::json!({"city": "Rome"}),
        },
        role: ChatRole::Assistant,
        trace_id: "trace1".to_string(),
        span_id: span.to_string(),
        session_id: None,
        message_index: 0,
        entry_index: 0,
        parent_span_id: Some("root".to_string()),
        // The step span between the agent and the tool carried no message, so neither path reaches the
        // other: ancestry cannot settle this pair.
        span_path: vec![span.to_string()],
        timestamp: Utc::now(),
        order_time: Utc::now(),
        occurrence_ordinal: 0,
        observation_type: Some(observation_type.to_string()),
        model: None,
        provider: None,
        name: None,
        finish_reason: None,
        tool_use_id: id.map(str::to_string),
        tool_name: Some("get_weather".to_string()),
        tokens: None,
        cost: None,
        status_code: None,
        is_error: false,
        source_type: "attribute".to_string(),
        event_name: None,
        source_attribute: Some(attribute.to_string()),
        category: MessageCategory::GenAIAssistantMessage,
        content_hash: "call".to_string(),
        is_semantic: true,
        uses_span_end: false,
        is_history: false,
        is_cross_trace_history: false,
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
    }
}

/// An agent's output re-listing a call its run made loses to the tool span that ran the call, though the
/// re-listing is the span's output and the tool span holds the call only as its input.
#[test]
fn a_tool_span_running_a_call_outranks_an_agent_relisting_it() {
    let relisted = call("agent", "agent", "output.value", Some("call_1"));
    let executed = call("tool", "tool", "input.value", None);
    assert!(relisted.is_output_source() && !executed.is_output_source());
    let mut blocks = vec![relisted, executed];
    mark_duplicate_history(&mut blocks, &HashMap::new());
    assert!(blocks[0].is_history, "the agent restates the call");
    assert!(!blocks[1].is_history, "the tool span ran it");
}

/// Only a re-listing yields: a model call's own output stays the original over the tool span's input.
#[test]
fn a_model_calls_output_still_outranks_the_tool_span_running_it() {
    let emitted = call("model", "generation", "output.value", Some("call_1"));
    let executed = call("tool", "tool", "input.value", None);
    let mut blocks = vec![executed, emitted];
    mark_duplicate_history(&mut blocks, &HashMap::new());
    assert!(
        blocks[0].is_history,
        "the tool span's input copies the model's call"
    );
    assert!(!blocks[1].is_history, "the model call emitted it");
}
