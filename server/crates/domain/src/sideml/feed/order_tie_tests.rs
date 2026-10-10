//! Two requests at one instant are ordered the same way whatever order their rows arrive in.

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

fn request(span: &str, results: [(&str, &str); 2]) -> MessageSpanRow {
    let mut messages =
        vec![json!({"role": "user", "content": "How is the weather in Paris and Rome?"})];
    for (call, text) in results {
        messages.push(json!({"role": "tool", "content": [
            {"type": "tool_result", "tool_use_id": call, "content": text}
        ]}));
    }
    row(span, "trace", messages)
}

fn row(span: &str, trace: &str, messages: Vec<serde_json::Value>) -> MessageSpanRow {
    let raw = [RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.input.messages".to_string(),
            time: at(),
        },
        content: serde_json::Value::Array(messages),
        rendering: false,
        direction: None,
        stream: None,
    }];
    MessageSpanRow {
        request_frame: String::new(),
        trace_id: trace.to_string(),
        span_id: span.to_string(),
        parent_span_id: None,
        span_timestamp: at(),
        span_end_timestamp: Some(at() + chrono::Duration::seconds(1)),
        messages_json: serde_json::to_string(&raw).unwrap(),
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

fn order(result: &FeedResult) -> Vec<String> {
    result
        .messages
        .iter()
        .map(|block| format!("{}:{}", block.span_id, block.content_hash))
        .collect()
}

/// Two model calls at one instant list the same two results in opposite orders. Which request's order a view
/// follows, and which call keeps each result, is decided by the calls' ids, so reversing the rows changes
/// nothing.
#[test]
fn simultaneous_requests_are_ordered_by_span_not_by_arrival() {
    let a = request(
        "span-a",
        [("call_1", "Paris: sunny"), ("call_2", "Rome: rain")],
    );
    let b = request(
        "span-b",
        [("call_2", "Rome: rain"), ("call_1", "Paris: sunny")],
    );
    let options = FeedOptions::default();
    let forward = order(&process_spans(vec![a.clone(), b.clone()], &options));
    let reversed = order(&process_spans(vec![b, a], &options));
    assert!(!forward.is_empty());
    assert_eq!(forward, reversed);
}

/// Two model calls at one instant, each sent a question of its own: nothing in the evidence orders them,
/// so the calls' ids do, and the rows' delivery order does not.
#[test]
fn distinct_simultaneous_requests_are_ordered_by_span_not_by_arrival() {
    let a = row(
        "span-a",
        "trace",
        vec![json!({"role": "user", "content": "What is the weather in Paris?"})],
    );
    let b = row(
        "span-b",
        "trace",
        vec![json!({"role": "user", "content": "What is the weather in Rome?"})],
    );
    let options = FeedOptions::default();
    let forward = order(&process_spans(vec![a.clone(), b.clone()], &options));
    let reversed = order(&process_spans(vec![b, a], &options));
    assert_eq!(forward.len(), 2);
    assert_eq!(forward, reversed);
}

/// Two traces of one session at one instant, each with a question of its own: the order their rows arrive
/// in does not decide which comes first.
#[test]
fn simultaneous_traces_are_ordered_whatever_order_their_rows_arrive_in() {
    let mut a = row(
        "span-a",
        "trace-a",
        vec![json!({"role": "user", "content": "What is the weather in Paris?"})],
    );
    let mut b = row(
        "span-b",
        "trace-b",
        vec![json!({"role": "user", "content": "What is the weather in Rome?"})],
    );
    a.session_id = Some("session".to_string());
    b.session_id = Some("session".to_string());
    let options = FeedOptions::default();
    let forward = order(&process_spans(vec![a.clone(), b.clone()], &options));
    let reversed = order(&process_spans(vec![b, a], &options));
    assert_eq!(forward.len(), 2);
    assert_eq!(forward, reversed);
}

/// Of two traces at one instant, the one re-sending the other's messages follows it - and delivering the
/// earlier one's row twice, as a retried export does, changes nothing.
#[test]
fn a_simultaneous_trace_that_replays_another_follows_it() {
    let question = json!({"role": "user", "content": "What is the weather in Paris?"});
    let answer = json!({"role": "assistant", "content": "Sunny."});
    let follow_up = json!({"role": "user", "content": "And tomorrow?"});
    let mut first = row("span-z", "trace-z", vec![question.clone()]);
    let mut second = row("span-a", "trace-a", vec![question, answer, follow_up]);
    first.session_id = Some("session".to_string());
    second.session_id = Some("session".to_string());
    let options = FeedOptions::default();
    let once = order(&process_spans(
        vec![second.clone(), first.clone()],
        &options,
    ));
    let twice = order(&process_spans(
        vec![first.clone(), second.clone(), first.clone()],
        &options,
    ));
    assert_eq!(once, twice);
    assert!(
        once.first()
            .is_some_and(|block| block.starts_with("span-z:")),
        "the trace whose messages the other re-sends comes first: {once:?}"
    );
}
