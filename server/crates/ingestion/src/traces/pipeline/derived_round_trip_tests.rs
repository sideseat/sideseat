//! **Every derived span column survives the store.**
//!
//! A derived column is computed when a span is ingested and then has to travel: from extraction into the
//! normalised span, into the store's row, and back out through the read a view is built from. Each hop is a
//! place to drop it, and two of them did - a conversion wrote `span_marks: 0` right after extraction computed
//! the marks, and a test harness did the same - so every stored span held nothing, and the tests that should
//! have seen it read the same constant. One hop at a time, each looked correct.
//!
//! This test is the whole trip at once. One span goes through the real pipeline (`ingest_now`, the code
//! production runs), extracted by the embedded corpus plus a probe asset that declares a mark and a thread no
//! shipped asset does, so neither is vacuously empty. Every derived value the two span reads carry is then
//! held to what extraction computed for that span, and each one is required to be a value other than the
//! default, so a column stored as its default cannot pass by agreeing with an extraction that also produced
//! nothing.

use std::sync::OnceLock;

use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use sideseat_domain::raw_payload::RawContent;
use sideseat_domain::rules::Ruleset;
use sideseat_ports::types::{MessageQueryParams, ProjectId};

use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::*;

const PROJECT: &str = "default";

/// A probe asset: one mark and one thread, on a span name no producer uses.
const PROBE: &str = r#"{
  "id": "zz-derived-round-trip-probe",
  "doc": "A test asset beside the embedded corpus, declaring what no shipped asset declares yet.",
  "span_marks": [
    {
      "id": "probe.streamed",
      "because": "the round trip needs a mark that holds for its span",
      "where": {"source": "attr:probe.options", "parses": "json", "member": {"path": "$.stream", "equals": true}}
    }
  ],
  "request_threads": [
    {
      "id": "probe.thread",
      "where": {"source": "span_name", "equals": "probe.request"},
      "key": ["attr:session.id"]
    }
  ]
}"#;

/// The embedded corpus with the probe beside it, compiled once and kept for the process, as the embedded one is.
fn probe_rules() -> &'static Ruleset {
    static RULES: OnceLock<Ruleset> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut sources = sideseat_domain::rules::schema::embedded_sources();
        sources.insert(
            "producers/zz-derived-round-trip-probe.json".to_string(),
            PROBE.as_bytes().to_vec(),
        );
        let assets = sideseat_domain::rules::assets::ParsedAssets::parse(&sources)
            .expect("the probe parses beside the corpus");
        Ruleset::build(&assets).expect("the probe compiles beside the corpus")
    })
}

fn attr(key: &str, value: any_value::Value) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue { value: Some(value) }),
    }
}

fn text(key: &str, value: &str) -> KeyValue {
    attr(key, any_value::Value::StringValue(value.to_string()))
}

/// One model call, with every attribute a derived column reads set to something other than its default.
fn export(answer: &str) -> ExportTraceServiceRequest {
    let span = Span {
        trace_id: vec![7; 16],
        span_id: vec![9; 8],
        name: "probe.request".to_string(),
        start_time_unix_nano: 1_700_000_000_000_000_000,
        end_time_unix_nano: 1_700_000_002_000_000_000,
        attributes: vec![
            text("session.id", "session-1"),
            text("user.id", "user-1"),
            text("probe.options", r#"{"stream": true, "temperature": 0.5}"#),
            // The key the embedded corpus frames requests by, so the span is framed.
            text("system_prompt_hash", "sp_probe"),
            text("gen_ai.operation.name", "chat"),
            text("gen_ai.system", "openai"),
            text("gen_ai.request.model", "gpt-4o"),
            text("gen_ai.response.model", "gpt-4o-2024-08-06"),
            text("gen_ai.response.id", "resp-1"),
            attr(
                "gen_ai.request.temperature",
                any_value::Value::DoubleValue(0.5),
            ),
            attr("gen_ai.request.top_p", any_value::Value::DoubleValue(0.9)),
            attr("gen_ai.request.max_tokens", any_value::Value::IntValue(64)),
            text("gen_ai.response.finish_reasons", r#"["stop"]"#),
            attr("gen_ai.usage.input_tokens", any_value::Value::IntValue(120)),
            attr("gen_ai.usage.output_tokens", any_value::Value::IntValue(30)),
            text(
                "gen_ai.input.messages",
                r#"[{"role": "user", "parts": [{"type": "text", "content": "Name a city."}]}]"#,
            ),
            text(
                "gen_ai.output.messages",
                &format!(
                    r#"[{{"role": "assistant", "parts": [{{"type": "text", "content": "{answer}"}}], "finish_reason": "stop"}}]"#
                ),
            ),
        ],
        ..Default::default()
    };
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![text("deployment.environment", "test")],
                ..Default::default()
            }),
            scope_spans: vec![ScopeSpans {
                scope: Some(InstrumentationScope {
                    name: "probe.instrumentation".to_string(),
                    version: "1.2.3".to_string(),
                    ..Default::default()
                }),
                spans: vec![span],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// Ingest a body exactly as the request path does: staged, prepared, written.
async fn ingest(pipeline: &TracePipeline, body: &ExportTraceServiceRequest) {
    let at = chrono::Utc::now();
    let received = ReceivedPayload::new(body.encode_to_vec(), RawContent::Protobuf, at);
    let (request, received) =
        crate::received::staged_traces(&received.staged(), PROJECT, at).expect("prepare");
    let outcome = pipeline.ingest_now(&request, &received).await;
    assert!(matches!(outcome, IngestOutcome::Stored), "{outcome:?}");
}

/// What extraction computed for the span, by the same rules the pipeline was given.
fn extracted(body: &ExportTraceServiceRequest) -> crate::traces::extract::SpanData {
    let mut request = body.clone();
    crate::otlp::inject_project_id_traces(&mut request, PROJECT);
    let mut spans = crate::traces::extract::extract_attributes_batch(&request, probe_rules());
    assert_eq!(spans.len(), 1);
    spans.remove(0)
}

#[tokio::test]
async fn every_derived_span_column_survives_the_store() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let pipeline = pipeline.with_rules(probe_rules());
    let body = export("Lisbon.");
    ingest(&pipeline, &body).await;
    let span = extracted(&body);
    let trace_id = hex::encode([7u8; 16]);
    let span_id = hex::encode([9u8; 8]);

    // What extraction computed must itself be worth comparing: a column that arrived as its default would
    // otherwise pass by agreeing with an extraction that also said nothing.
    let mark = 1
        << probe_rules()
            .span_marks
            .bit_of("probe.streamed")
            .expect("the probe's mark");
    assert_eq!(span.span_marks, mark, "the probe's mark holds for its span");
    assert_eq!(span.request_thread, r#"["probe.thread","session-1"]"#);

    let rows = analytics
        .get_messages(&MessageQueryParams {
            project_id: ProjectId::from(PROJECT),
            span_id: Some(span_id.clone()),
            trace_id: Some(trace_id.clone()),
            ..Default::default()
        })
        .await
        .expect("the span's message row")
        .rows;
    assert_eq!(rows.len(), 1, "one stored row for the span");
    let row = &rows[0];
    let stored = analytics
        .get_span(&ProjectId::from(PROJECT), &trace_id, &span_id)
        .await
        .expect("the span read")
        .expect("the stored span");

    // Each derived value: what the message row and the span row read back, beside what extraction computed.
    let observation = span.observation_type.map(|t| t.as_str().to_string());
    let category = span.span_category.map(|c| c.as_str().to_string());
    let finish = serde_json::to_string(&span.gen_ai_finish_reasons).expect("finish reasons");
    let mut checked = Vec::new();
    let mut check = |column: &str, stored: String, extracted: String, default: String| {
        assert_ne!(
            extracted, default,
            "`{column}`: extraction produced its default, so the round trip would prove nothing - give the \
             probe span a value for it"
        );
        assert_eq!(
            stored, extracted,
            "`{column}` did not survive the store: extraction computed {extracted}, the read returned {stored}"
        );
        checked.push(column.to_string());
    };
    let some = |value: &Option<String>| format!("{value:?}");
    check(
        "span_marks",
        row.span_marks.to_string(),
        span.span_marks.to_string(),
        "0".into(),
    );
    check(
        "request_thread",
        row.request_thread.clone(),
        span.request_thread.clone(),
        String::new(),
    );
    check(
        "request_frame",
        row.request_frame.clone(),
        span.request_frame.clone(),
        String::new(),
    );
    check(
        "framework",
        some(&row.framework),
        some(&span.framework),
        "None".into(),
    );
    check(
        "observation_type",
        some(&row.observation_type),
        format!("{observation:?}"),
        "None".into(),
    );
    check(
        "span_category",
        some(&stored.span_category),
        format!("{category:?}"),
        "None".into(),
    );
    check(
        "session_id",
        some(&row.session_id),
        some(&span.session_id),
        "None".into(),
    );
    check(
        "user_id",
        some(&stored.user_id),
        some(&span.user_id),
        "None".into(),
    );
    check(
        "environment",
        some(&stored.environment),
        some(&span.environment),
        "None".into(),
    );
    check(
        "scope_name",
        some(&row.scope_name),
        some(&span.scope_name),
        "None".into(),
    );
    check(
        "scope_version",
        some(&row.scope_version),
        some(&span.scope_version),
        "None".into(),
    );
    check(
        "gen_ai_system",
        some(&row.provider),
        some(&span.gen_ai_system),
        "None".into(),
    );
    check(
        "gen_ai_request_model",
        some(&row.model),
        some(&span.gen_ai_request_model),
        "None".into(),
    );
    check(
        "gen_ai_response_model",
        some(&row.response_model),
        some(&span.gen_ai_response_model),
        "None".into(),
    );
    check(
        "gen_ai_response_id",
        some(&row.response_id),
        some(&span.gen_ai_response_id),
        "None".into(),
    );
    check(
        "gen_ai_temperature",
        format!("{:?}", row.temperature),
        format!("{:?}", span.gen_ai_temperature),
        "None".into(),
    );
    check(
        "gen_ai_top_p",
        format!("{:?}", row.top_p),
        format!("{:?}", span.gen_ai_top_p),
        "None".into(),
    );
    check(
        "gen_ai_max_tokens",
        format!("{:?}", row.max_tokens),
        format!("{:?}", span.gen_ai_max_tokens),
        "None".into(),
    );
    check(
        "gen_ai_finish_reasons",
        format!("{:?}", stored.gen_ai_finish_reasons),
        format!("{:?}", span.gen_ai_finish_reasons),
        "[]".into(),
    );
    check(
        "gen_ai_usage_input_tokens",
        row.input_tokens.to_string(),
        span.gen_ai_usage_input_tokens.to_string(),
        "0".into(),
    );
    check(
        "gen_ai_usage_output_tokens",
        row.output_tokens.to_string(),
        span.gen_ai_usage_output_tokens.to_string(),
        "0".into(),
    );
    check(
        "gen_ai_usage_total_tokens",
        row.total_tokens.to_string(),
        span.gen_ai_usage_total_tokens.to_string(),
        "0".into(),
    );
    // The finish reasons as the message row states them, beside the span row's list.
    assert_eq!(
        row.finish_reasons
            .as_deref()
            .map(|text| text.replace(' ', "")),
        Some(finish.replace(' ', "")),
        "the message row's finish reasons"
    );

    // The previews and the messages come from the SideML stage and the message extraction: present, and the
    // answer is where a view reads it.
    assert!(
        stored
            .output_preview
            .as_deref()
            .is_some_and(|preview| preview.contains("Lisbon")),
        "the output preview: {:?}",
        stored.output_preview
    );
    assert!(
        row.messages_json.contains("Lisbon") && row.messages_json.contains("Name a city"),
        "the stored messages: {}",
        row.messages_json
    );

    // The search terms: the span is found by a word of its prompt.
    let found = analytics
        .search(&sideseat_ports::types::SearchQuery {
            project_id: ProjectId::from(PROJECT),
            signal: sideseat_ports::types::SearchSignal::Spans,
            expression: sideseat_ports::types::SearchExpr::Term {
                field: None,
                term: "city".into(),
            },
            limit: 10,
            max_examined: 100,
            cursor: None,
            from_timestamp: None,
            to_timestamp: None,
        })
        .await
        .expect("search");
    assert_eq!(found.candidates.len(), 1, "the span is found by its terms");

    // A later revision of the same span supersedes the first: the reads answer with its derived values only.
    let revised = export("Porto.");
    ingest(&pipeline, &revised).await;
    let rows = analytics
        .get_messages(&MessageQueryParams {
            project_id: ProjectId::from(PROJECT),
            span_id: Some(span_id.clone()),
            trace_id: Some(trace_id.clone()),
            ..Default::default()
        })
        .await
        .expect("the revised row")
        .rows;
    assert_eq!(rows.len(), 1, "one winning row after a revision");
    assert!(
        rows[0].messages_json.contains("Porto") && !rows[0].messages_json.contains("Lisbon"),
        "the revision's messages, and only them"
    );
    assert_eq!(
        (rows[0].span_marks, rows[0].request_thread.as_str()),
        (span.span_marks, span.request_thread.as_str()),
        "the revision keeps its derived columns"
    );
    assert!(checked.len() >= 20, "the columns compared: {checked:?}");
}
