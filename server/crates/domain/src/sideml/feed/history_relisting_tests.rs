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
            provider_executed: false,
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
        is_rendering: false,
        declared_direction: None,
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
        citations: Vec::new(),
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

/// Two model calls sent one prompt at the same instant keep it on the same one whatever order their rows
/// arrive in: the earlier span id, as the clock cannot tell them apart.
#[test]
fn which_receiving_call_keeps_a_prompt_does_not_depend_on_row_order() {
    let at = Utc::now();
    let copy = |span: &str, observation_type: &str, attribute: &str| {
        let mut block = prompt(span, observation_type, attribute);
        block.timestamp = at;
        block.order_time = at;
        block
    };
    for order in [["model-a", "model-b"], ["model-b", "model-a"]] {
        let mut blocks = vec![copy("template", "span", "output.value")];
        blocks.extend(order.map(|span| copy(span, "generation", "gen_ai.input.messages")));
        mark_duplicate_history(&mut blocks, &HashMap::new());
        let kept: Vec<&str> = blocks
            .iter()
            .filter(|b| !b.is_history)
            .map(|b| b.span_id.as_str())
            .collect();
        assert_eq!(kept, vec!["model-a"], "rows in the order {order:?}");
    }
}

/// A finish reason a producer writes on the messages it was sent is shown as stated, and is never read as
/// the call's output; on what the call produced it still is.
#[test]
fn a_finish_reason_marks_output_only_on_what_the_span_produced() {
    let mut sent = prompt("model", "generation", "gen_ai.input.messages");
    sent.finish_reason = Some(crate::sideml::types::FinishReason::Stop);
    assert!(!sent.is_protected(), "a request is not the call's output");
    assert!(sent.finish_reason.is_some(), "the value is kept as stated");
    let mut produced = prompt("model", "generation", "gen_ai.output.messages");
    produced.role = ChatRole::Assistant;
    produced.finish_reason = Some(crate::sideml::types::FinishReason::Stop);
    assert!(
        produced.is_protected(),
        "a finished reply is the call's output"
    );
}

/// An event-based framework's model call under an agent span: the call's assistant text is intermediate, since
/// the agent span restates it, but reasoning whose text was withheld is the conversation's only copy.
#[test]
fn withheld_reasoning_is_not_intermediate_output() {
    use super::tests::make_block_with_source;
    let assistant = || {
        make_block_with_source(
            "text",
            Some("generation"),
            None,
            "attribute",
            MessageCategory::GenAIAssistantMessage,
            ChatRole::Assistant,
        )
    };
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
            Some("gen_ai.user.message"),
            "event",
            MessageCategory::GenAIUserMessage,
            ChatRole::User,
        ),
        assistant(),
        assistant(),
    ];
    blocks[3].content = ContentBlock::Thinking {
        text: String::new(),
        signature: Some("sig".to_string()),
    };
    let agent_span = blocks[0].span_id.clone();
    for block in &mut blocks[1..] {
        block.parent_span_id = Some(agent_span.clone());
        block.span_path = vec![agent_span.clone(), block.span_id.clone()];
    }
    mark_history(&mut blocks, &std::collections::HashMap::new());
    assert!(
        blocks[2].is_history,
        "the call's text is restated by the agent span"
    );
    assert!(!blocks[3].is_history, "the withheld reasoning is kept");
}
