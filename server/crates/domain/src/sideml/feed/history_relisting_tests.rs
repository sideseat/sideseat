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

/// Two tool spans holding the same call leave the occurrence ambiguous: the re-listing stands.
#[test]
fn two_tool_spans_running_one_call_shape_do_not_displace_the_relisting() {
    let relisted = call("agent", "agent", "output.value", Some("call_1"));
    let first = call("tool-a", "tool", "input.value", None);
    let second = call("tool-b", "tool", "input.value", None);
    let mut blocks = vec![relisted, first, second];
    mark_duplicate_history(&mut blocks, &HashMap::new());
    assert!(
        !blocks[0].is_history,
        "no unique execution outranks the re-listing"
    );
}

/// One tool span delivered twice is still one execution.
#[test]
fn a_redelivered_tool_span_still_outranks_the_relisting() {
    let relisted = call("agent", "agent", "output.value", Some("call_1"));
    let executed = call("tool", "tool", "input.value", None);
    let mut blocks = vec![relisted, executed.clone(), executed];
    mark_duplicate_history(&mut blocks, &HashMap::new());
    assert!(blocks[0].is_history, "the agent restates the call");
}

fn prompt(span: &str, observation_type: &str, attribute: &str) -> BlockEntry {
    let mut block = call(span, observation_type, attribute, None);
    block.entry_type = "text".to_string();
    block.content = ContentBlock::Text {
        text: "In one sentence, what is Kyoto best known for?".to_string(),
    };
    block.role = ChatRole::User;
    block.tool_use_id = None;
    block.tool_name = None;
    block.category = MessageCategory::GenAIUserMessage;
    block.content_hash = "prompt".to_string();
    block
}

/// A prompt a template renders as its output and a model call is then sent: the call's input is where it
/// was used, so the template's re-listing is the copy that yields, though it is an output and came first.
#[test]
fn a_model_call_sent_a_prompt_outranks_the_template_that_rendered_it() {
    let rendered = prompt("template", "span", "output.value");
    let received = prompt("model", "generation", "gen_ai.input.messages");
    assert!(rendered.is_output_source() && received.is_input_source());
    let mut blocks = vec![rendered, received];
    mark_duplicate_history(&mut blocks, &HashMap::new());
    assert!(blocks[0].is_history, "the template passes the prompt on");
    assert!(!blocks[1].is_history, "the model call was sent it");
}

/// Only a span that passes messages on yields: a model call's own reply stays the original over the next
/// call's input copy of it.
#[test]
fn a_model_calls_reply_still_outranks_the_next_call_sent_it() {
    let mut replied = prompt("first", "generation", "gen_ai.output.messages");
    replied.role = ChatRole::Assistant;
    let mut resent = prompt("second", "generation", "gen_ai.input.messages");
    resent.role = ChatRole::Assistant;
    let mut blocks = vec![replied, resent];
    mark_duplicate_history(&mut blocks, &HashMap::new());
    assert!(!blocks[0].is_history, "the first call produced the reply");
    assert!(blocks[1].is_history, "the second call was sent it back");
}
