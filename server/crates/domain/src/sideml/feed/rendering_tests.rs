//! A rendering - a turn a producer re-sent as text, whose call and result other carriers hold losslessly - is
//! shown on the span that sent it and left out of the trace and session views.

use chrono::{DateTime, Utc};
use serde_json::json;
use sideseat_ports::types::MessageSpanRow;

use super::*;
use crate::observations::{MessageSource, RawMessage};

fn at() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn message(key: &str, content: serde_json::Value, rendering: bool) -> RawMessage {
    RawMessage {
        source: MessageSource::Attribute {
            key: key.to_string(),
            time: at(),
        },
        content,
        rendering,
    }
}

fn row(messages: &[RawMessage]) -> MessageSpanRow {
    MessageSpanRow {
        trace_id: "trace".to_string(),
        span_id: "span".to_string(),
        parent_span_id: None,
        span_timestamp: at(),
        span_end_timestamp: Some(at() + chrono::Duration::seconds(1)),
        messages_json: serde_json::to_string(messages).unwrap(),
        tool_definitions_json: "[]".to_string(),
        tool_names_json: "[]".to_string(),
        log_messages_json: "[]".to_string(),
        body_cache_key: None,
        model: Some("model".to_string()),
        provider: None,
        status_code: None,
        exception_type: None,
        exception_message: None,
        exception_stacktrace: None,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cost_total: 0.0,
        observation_type: Some("generation".to_string()),
        session_id: None,
        ingested_at: at(),
        scope_name: None,
        scope_version: None,
        span_name: None,
        framework: None,
        response_model: None,
        response_id: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        finish_reasons: None,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_input: 0.0,
        cost_output: 0.0,
        request_thread: String::new(),
        span_marks: 0,
    }
}

fn texts(result: &FeedResult) -> Vec<String> {
    result
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            crate::sideml::types::ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn conversation(rendering: bool) -> Vec<MessageSpanRow> {
    vec![row(&[
        message(
            "gen_ai.input.messages",
            json!({"role": "user", "content": "What is the weather?"}),
            false,
        ),
        message(
            "gen_ai.input.messages",
            json!({"role": "assistant", "content": "Calling tools: weather(city='Paris')"}),
            rendering,
        ),
        message(
            "gen_ai.input.messages",
            json!({"role": "user", "content": "Observation: sunny"}),
            rendering,
        ),
        message(
            "gen_ai.output.messages",
            json!({"role": "assistant", "content": "It is sunny."}),
            false,
        ),
    ])]
}

#[test]
fn a_rendering_is_on_its_span_and_not_in_the_trace() {
    let options = FeedOptions::default();
    let span = texts(&process_span(conversation(true), &options));
    let trace = texts(&process_spans(conversation(true), &options));
    for rendered in ["Calling tools: weather(city='Paris')", "Observation: sunny"] {
        assert!(
            span.iter().any(|t| t == rendered),
            "the span shows what it sent: {span:?}"
        );
        assert!(
            !trace.iter().any(|t| t == rendered),
            "the trace leaves the rendering out: {trace:?}"
        );
    }
    for kept in ["What is the weather?", "It is sunny."] {
        assert!(
            trace.iter().any(|t| t == kept),
            "the rest of the turn stays: {trace:?}"
        );
    }
    // Unmarked, the same turns are ordinary input and the trace keeps them, so the marker is what decides.
    let unmarked = texts(&process_spans(conversation(false), &options));
    assert!(
        unmarked.iter().any(|t| t == "Observation: sunny"),
        "{unmarked:?}"
    );
}

/// The flag is written only when true, so every stored message that is not a rendering keeps its bytes.
#[test]
fn a_stored_message_carries_the_flag_only_when_it_is_one() {
    let plain = message("k", json!({"role": "user", "content": "x"}), false);
    let encoded = serde_json::to_string(&plain).unwrap();
    assert!(!encoded.contains("rendering"), "{encoded}");
    let decoded: RawMessage = serde_json::from_str(&encoded).unwrap();
    assert!(!decoded.rendering);
    let marked = message("k", json!({"role": "user", "content": "x"}), true);
    let decoded: RawMessage =
        serde_json::from_str(&serde_json::to_string(&marked).unwrap()).unwrap();
    assert!(decoded.rendering, "the flag survives storage");
}

/// Splitting one stored message into several - a list of turns, a tool call beside text - keeps the flag on
/// every part, so no part of a rendering reaches a collapsed view.
#[test]
fn every_part_of_an_expanded_rendering_is_one() {
    let rows = vec![row(&[message(
        "gen_ai.input.messages",
        json!([
            {"role": "assistant", "content": [
                {"type": "text", "text": "Calling tools"},
                {"type": "tool_use", "id": "call_1", "name": "weather", "input": {"city": "Paris"}}
            ]},
            {"role": "user", "content": "Observation: sunny"}
        ]),
        true,
    )])];
    let options = FeedOptions::default();
    assert!(
        !process_span(rows.clone(), &options).messages.is_empty(),
        "the span shows the parts"
    );
    let trace = process_spans(rows, &options);
    assert!(
        trace.messages.is_empty(),
        "no part survives in the trace: {:?}",
        texts(&trace)
    );
}
