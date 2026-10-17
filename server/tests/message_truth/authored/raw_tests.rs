//! The reader-derived output oracle: each clause on its own, and agreement with the reconstruction where the readers'
//! member choice is subtle.

use super::*;
use opentelemetry_proto::tonic::common::v1::KeyValue;
use opentelemetry_proto::tonic::trace::v1::{Span, span::Event};

fn read(
    source: serde_json::Value,
    content: serde_json::Value,
) -> sideseat_domain::sideml::SideMLMessage {
    let raw: RawMessage =
        serde_json::from_value(serde_json::json!({"source": source, "content": content}))
            .expect("a raw message");
    let mut read = sideseat_domain::sideml::to_sideml(&[raw]);
    assert_eq!(read.len(), 1, "one message read");
    read.remove(0)
}

fn texts(shown: &[(Shown, Option<u64>)]) -> Vec<String> {
    shown
        .iter()
        .map(|(s, _)| match s {
            Shown::Text(t) => t.clone(),
            Shown::Json(v) => v.to_string(),
        })
        .collect()
}

const TIME: &str = "2024-01-01T00:00:00Z";

/// Each clause of `shown` refuses on its own: a carrier that does not hold the span's output, a role other than the
/// assistant's, a tool's execution, and a block that is no text or structured output.
#[test]
fn each_clause_of_what_a_span_shows_refuses_on_its_own() {
    let choice = serde_json::json!({"event": {"name": "gen_ai.choice", "time": TIME}});
    let answer = read(
        choice.clone(),
        serde_json::json!({"role": "assistant", "content": "the answer"}),
    );
    let shown_answer = shown(&answer, Some("generation"), Some("chat"));
    assert_eq!(texts(&shown_answer), ["the answer"]);
    assert!(
        shown_answer[0].1.is_some(),
        "an event carrier is dated by the event"
    );
    assert!(
        shown(&answer, Some("tool"), Some("chat")).is_empty(),
        "a tool's execution states the tool's output"
    );
    // An assistant turn in the request the span was sent: a carrier that holds no output of the span.
    let sent = read(
        serde_json::json!({"attribute": {"key": "gen_ai.input.messages", "time": TIME}}),
        serde_json::json!({"role": "assistant", "content": "an earlier answer"}),
    );
    assert!(
        shown(&sent, Some("generation"), Some("chat")).is_empty(),
        "an input carrier"
    );
    let tool = read(
        serde_json::json!({"attribute": {"key": "gen_ai.output.messages", "time": TIME}}),
        serde_json::json!({"role": "tool", "content": "a tool's message"}),
    );
    assert!(
        shown(&tool, Some("generation"), Some("chat")).is_empty(),
        "another role"
    );
    let user = read(
        serde_json::json!({"attribute": {"key": "gen_ai.output.messages", "time": TIME}}),
        serde_json::json!({"role": "user", "content": "a user's text"}),
    );
    assert!(
        shown(&user, Some("generation"), Some("chat")).is_empty(),
        "a user's text in an output carrier"
    );
    let thinking = read(
        choice,
        serde_json::json!({"role": "assistant", "content": [{"type": "thinking", "thinking": "hm"}]}),
    );
    assert!(
        shown(&thinking, Some("generation"), Some("chat")).is_empty(),
        "a thinking block"
    );
}

/// What a reader shows is held only as the raw span holds it: a text verbatim, structured output as a raw string that
/// parses to it, dated by its event or, for an attribute, the span's end.
#[test]
fn only_the_bytes_the_span_holds_are_its_output() {
    let strings = vec![
        ("the answer".to_string(), 5),
        (r#"{"city": "Wien"}"#.to_string(), 5),
    ];
    let shown = vec![
        (Shown::Text("the answer".into()), Some(3)),
        (Shown::Text("a reader's composed text".into()), None),
        (Shown::Json(serde_json::json!({"city": "Wien"})), None),
        (Shown::Json(serde_json::json!({"city": "Graz"})), None),
    ];
    assert_eq!(
        held(shown, &strings, 9),
        [
            ("the answer".to_string(), 3),
            (r#"{"city": "Wien"}"#.to_string(), 9)
        ]
    );
}

/// Round 13's shapes, where an independent approximation and the readers disagreed: the oracle now holds exactly what
/// the reconstruction shows as the span's assistant output, so the two cannot drift.
#[test]
fn the_oracle_holds_what_the_readers_show() {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::any_value;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans};
    use prost::Message;
    let (role_in_list, nested, beside_false) = (
        "Role in a list.",
        "Nested twice.",
        "Beside reasoning false.",
    );
    let messages = serde_json::json!([
        {"role": "assistant", "content": [{"role": "assistant", "reasoning": {"text": "x"}, "text": role_in_list}]},
        {"role": "assistant", "content": [[nested]]},
        {"role": "assistant", "parts": [{"type": "text", "text": beside_false, "reasoning": false}]}
    ])
    .to_string();
    let string = |key: &str, value: &str| KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
    };
    let request = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![1; 16],
                    span_id: vec![2; 8],
                    name: "chat".into(),
                    start_time_unix_nano: 1_700_000_000_000_000_000,
                    end_time_unix_nano: 1_700_000_001_000_000_000,
                    attributes: vec![
                        string("gen_ai.operation.name", "chat"),
                        string("gen_ai.output.messages", &messages),
                    ],
                    events: Vec::<Event>::new(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let dir = std::env::temp_dir().join(format!("authored-agreement-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("req-001.pb");
    std::fs::write(&path, request.encode_to_vec()).expect("the request is written");
    let paths = vec![path];
    let spans = raw_spans(&paths);
    let recon = super::super::super::recon::build("_probe/authored/agreement", &paths);
    std::fs::remove_dir_all(&dir).ok();
    for text in [role_in_list, nested, beside_false] {
        let oracle = spans
            .iter()
            .any(|s| s.outputs.iter().any(|(held, _)| held == text));
        let readers = recon.views.iter().any(|v| {
            v.kind == super::super::super::recon::ViewKind::Span
                && v.blocks
                    .iter()
                    .any(|b| b.output && b.role == "assistant" && b.text() == Some(text))
        });
        assert_eq!(
            oracle, readers,
            "{text:?}: the oracle and the readers disagree"
        );
    }
}

/// An event is dated by its own time over OTLP's whole unsigned range, never by its span's end: one past what signed
/// nanoseconds hold keeps its time, and one that cannot be stated at all is no output.
#[test]
fn an_event_keeps_its_own_time() {
    let late = read(
        serde_json::json!({"event": {"name": "gen_ai.choice", "time": "2290-01-01T00:00:00Z"}}),
        serde_json::json!({"role": "assistant", "content": "a late answer"}),
    );
    let shown_late = shown(&late, Some("generation"), Some("chat"));
    assert_eq!(texts(&shown_late), ["a late answer"]);
    let expected = nanos("2290-01-01T00:00:00Z".parse().expect("a time"));
    assert!(
        expected.is_some_and(|n| n > i64::MAX as u64),
        "the probe time is past signed nanoseconds"
    );
    assert_eq!(shown_late[0].1, expected);
    let before_epoch = read(
        serde_json::json!({"event": {"name": "gen_ai.choice", "time": "1960-01-01T00:00:00Z"}}),
        serde_json::json!({"role": "assistant", "content": "an undatable answer"}),
    );
    assert!(shown(&before_epoch, Some("generation"), Some("chat")).is_empty());
}
