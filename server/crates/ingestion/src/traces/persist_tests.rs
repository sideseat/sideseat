use super::*;
use crate::traces::MessageSource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans};
use serde_json::json;

fn make_span(id: &str) -> SpanData {
    SpanData {
        project_id: Some("test-project".to_string()),
        trace_id: "trace1".to_string(),
        span_id: id.to_string(),
        span_name: format!("span-{}", id),
        timestamp_start: chrono::DateTime::UNIX_EPOCH,
        ..Default::default()
    }
}

fn make_otlp_span(id: &str) -> Span {
    Span {
        trace_id: b"trace1__________".to_vec(),
        span_id: id.as_bytes().to_vec(),
        name: format!("span-{}", id),
        ..Default::default()
    }
}

fn make_request(span_count: usize) -> ExportTraceServiceRequest {
    let spans: Vec<Span> = (0..span_count)
        .map(|i| make_otlp_span(&format!("span{}", i + 1)))
        .collect();
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn make_raw_message(content: &str) -> RawMessage {
    RawMessage {
        source: MessageSource::Attribute {
            key: "test".to_string(),
            time: chrono::DateTime::UNIX_EPOCH,
        },
        content: json!({
            "role": "user",
            "content": content
        }),
        rendering: false,
        direction: None,
        stream: None,
    }
}

fn make_enrichment() -> SpanEnrichment {
    SpanEnrichment::default()
}

#[test]
fn test_flatten_empty() {
    let request = ExportTraceServiceRequest::default();
    let spans: Vec<SpanData> = vec![];
    let messages: Vec<Vec<RawMessage>> = vec![];
    let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![];
    let tool_names: Vec<Vec<RawToolNames>> = vec![];
    let enrichments: Vec<SpanEnrichment> = vec![];
    let (result, _) = flatten(
        &request,
        spans,
        messages,
        tool_definitions,
        tool_names,
        enrichments,
        false,
        None,
    );
    assert!(result.is_empty());
}

#[test]
fn test_flatten_single_span_no_messages() {
    let request = make_request(1);
    let spans = vec![make_span("span1")];
    let messages: Vec<Vec<RawMessage>> = vec![vec![]];
    let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![vec![]];
    let tool_names: Vec<Vec<RawToolNames>> = vec![vec![]];
    let enrichments = vec![make_enrichment()];
    let (result, _) = flatten(
        &request,
        spans,
        messages,
        tool_definitions,
        tool_names,
        enrichments,
        false,
        None,
    );

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].span_id, "span1");
    assert_eq!(result[0].messages.as_deref().unwrap_or("[]"), "[]");
    assert_eq!(result[0].tool_definitions.as_deref().unwrap_or("[]"), "[]");
    assert_eq!(result[0].tool_names.as_deref().unwrap_or("[]"), "[]");
}

#[test]
fn test_flatten_multiple_spans() {
    let request = make_request(3);
    let spans = vec![make_span("span1"), make_span("span2"), make_span("span3")];
    let messages: Vec<Vec<RawMessage>> = vec![vec![], vec![], vec![]];
    let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![vec![], vec![], vec![]];
    let tool_names: Vec<Vec<RawToolNames>> = vec![vec![], vec![], vec![]];
    let enrichments = vec![make_enrichment(), make_enrichment(), make_enrichment()];
    let (result, _) = flatten(
        &request,
        spans,
        messages,
        tool_definitions,
        tool_names,
        enrichments,
        false,
        None,
    );

    assert_eq!(result.len(), 3);
}

#[test]
fn test_flatten_stores_raw_messages() {
    let request = make_request(1);
    let spans = vec![make_span("span1")];
    let messages = vec![vec![
        make_raw_message("Hello"),
        make_raw_message("Hi there"),
    ]];
    let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![vec![]];
    let tool_names: Vec<Vec<RawToolNames>> = vec![vec![]];
    let enrichments = vec![make_enrichment()];

    let (result, _) = flatten(
        &request,
        spans,
        messages,
        tool_definitions,
        tool_names,
        enrichments,
        false,
        None,
    );

    assert_eq!(result.len(), 1);

    // Verify messages are stored as pre-serialized JSON string
    let messages_str = result[0].messages.as_deref().unwrap();
    let stored_messages: Vec<serde_json::Value> = serde_json::from_str(messages_str).unwrap();
    assert_eq!(stored_messages.len(), 2);
    // Raw messages have "content" field with original message data
    assert_eq!(stored_messages[0]["content"]["content"], "Hello");
    assert_eq!(stored_messages[1]["content"]["content"], "Hi there");
}

#[test]
fn test_flatten_applies_enrichment() {
    let request = make_request(1);
    let spans = vec![make_span("span1")];
    let messages: Vec<Vec<RawMessage>> = vec![vec![]];
    let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![vec![]];
    let tool_names: Vec<Vec<RawToolNames>> = vec![vec![]];
    let enrichments = vec![SpanEnrichment {
        input_cost: 0.001,
        output_cost: 0.002,
        total_cost: 0.003,
        input_preview: Some("Hello".to_string()),
        output_preview: Some("Hi".to_string()),
        ..Default::default()
    }];

    let (result, _) = flatten(
        &request,
        spans,
        messages,
        tool_definitions,
        tool_names,
        enrichments,
        false,
        None,
    );

    assert_eq!(result[0].gen_ai_cost_input, 0.001);
    assert_eq!(result[0].gen_ai_cost_output, 0.002);
    assert_eq!(result[0].gen_ai_cost_total, 0.003);
    assert_eq!(result[0].input_preview, Some("Hello".to_string()));
    assert_eq!(result[0].output_preview, Some("Hi".to_string()));
}

/// A reference to a file that was rejected must not be committed as though the file exists.
///
/// The failure it guards is silent: a `#!B64!#` reference to a quota-rejected file renders exactly
/// like a reference to a corrupt one, and nothing on the span distinguishes them.
#[test]
fn a_rejected_file_reference_is_replaced_with_a_note() {
    let uri = sideseat_core::utils::file_uri::build_file_uri("abc123", Some("image/png"));
    let other = sideseat_core::utils::file_uri::build_file_uri("def456", Some("image/png"));
    let mut spans = vec![NormalizedSpan {
        project_id: Some("proj".to_string()),
        messages: Some(format!(r#"[{{"content":"{uri}"}}]"#)),
        tool_definitions: Some(format!(r#"[{{"icon":"{uri}"}}]"#)),
        metadata: Some(format!(r#"{{"attr":"{other}"}}"#)),
        ..NormalizedSpan::default()
    }];

    let rewritten = note_unstored_files(&mut spans, &[("proj".to_string(), uri.clone())]);

    assert_eq!(
        rewritten, 2,
        "both references to the rejected file are rewritten"
    );
    let messages = spans[0].messages.as_deref().unwrap();
    assert!(
        !messages.contains(&uri),
        "the rejected reference is still committed: {messages}"
    );
    assert!(
        messages.contains("not stored") && messages.contains("image/png"),
        "the note must say what happened and to what: {messages}"
    );
    assert!(
        spans[0].metadata.as_deref().unwrap().contains(&other),
        "a reference to a file that *was* stored must be left alone"
    );
}

/// Nothing to rewrite must cost nothing and change nothing.
#[test]
fn no_rejected_files_leaves_spans_untouched() {
    let uri = sideseat_core::utils::file_uri::build_file_uri("abc123", None);
    let mut spans = vec![NormalizedSpan {
        messages: Some(format!(r#"[{{"content":"{uri}"}}]"#)),
        ..NormalizedSpan::default()
    }];
    let before = spans[0].messages.clone();
    assert_eq!(note_unstored_files(&mut spans, &[]), 0);
    assert_eq!(spans[0].messages, before);
}

/// A hash is content-addressed, so the same URI can be rejected in one project and stored in
/// another. Rewriting it everywhere would replace a *working* reference with a note.
#[test]
fn a_quota_rejection_only_rewrites_the_project_it_happened_in() {
    let uri = sideseat_core::utils::file_uri::build_file_uri("shared", Some("image/png"));
    let mut spans = vec![
        NormalizedSpan {
            project_id: Some("over-quota".to_string()),
            messages: Some(format!(r#"[{{"content":"{uri}"}}]"#)),
            ..NormalizedSpan::default()
        },
        NormalizedSpan {
            project_id: Some("healthy".to_string()),
            messages: Some(format!(r#"[{{"content":"{uri}"}}]"#)),
            ..NormalizedSpan::default()
        },
    ];

    let rewritten = note_unstored_files(&mut spans, &[("over-quota".to_string(), uri.clone())]);

    assert_eq!(
        rewritten, 1,
        "only the rejected project's reference is rewritten"
    );
    assert!(!spans[0].messages.as_deref().unwrap().contains(&uri));
    assert!(
        spans[1].messages.as_deref().unwrap().contains(&uri),
        "the other project stored this file; its reference must be left alone"
    );
}

/// What the extraction derived about a span reaches the stored row: the marks a read-time projection asks are
/// answered once, at ingest, and a row that lost them answers every such question "no".
#[test]
fn flattening_keeps_the_derived_span_columns() {
    let request = make_request(1);
    let mut span = make_span("span1");
    span.span_marks = 0b101;
    span.request_thread = "thread-1".to_string();
    let (result, _) = flatten(
        &request,
        vec![span],
        vec![vec![]],
        vec![vec![]],
        vec![vec![]],
        vec![make_enrichment()],
        false,
        None,
    );
    assert_eq!(result[0].span_marks, 0b101);
    assert_eq!(result[0].request_thread, "thread-1");
}
