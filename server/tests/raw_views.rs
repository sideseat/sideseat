//! The raw span rendered from a stored record is the one ingestion used to store beside it.
//!
//! `raw_span` was a JSON column: the whole OTLP span, serialized again next to the record it came from. It is
//! now rendered on demand (`sideseat_ingestion::traces::raw_views`), so two things have to hold over the whole
//! corpus, and both are checked here: the record carries everything the view needs - rendering from the decoded
//! record gives the same JSON as rendering from the export itself, media spliced back included - and the
//! rendering is a function of the telemetry, so the same input always gives the same string. The second is not
//! theoretical: the column was built from a `HashMap` of resource attributes and serialized them in a different
//! order on every call.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use sideseat_domain::pricing::PricingService;
use sideseat_domain::raw_payload::{self, RawContent};
use sideseat_ingestion::traces::extract::ExtractionMode;
use sideseat_ingestion::traces::raw_views;

fn trace_exports() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("req-"))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/messages"),
        &mut out,
    );
    out.sort();
    out
}

fn request_of(path: &Path) -> Option<ExportTraceServiceRequest> {
    let bytes = std::fs::read(path).expect("read export");
    if path.extension().is_some_and(|e| e == "json") {
        serde_json::from_slice(&bytes).ok()
    } else {
        ExportTraceServiceRequest::decode(bytes.as_slice()).ok()
    }
}

#[test]
fn rendering_from_the_record_equals_rendering_from_the_export() {
    let exports = trace_exports();
    assert!(
        exports.len() > 500,
        "the corpus is missing: {} exports",
        exports.len()
    );
    let mut compared = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for path in &exports {
        let Some(request) = request_of(path) else {
            continue;
        };
        let content = if path.extension().is_some_and(|e| e == "json") {
            RawContent::Json
        } else {
            RawContent::Protobuf
        };
        let received = std::fs::read(path).expect("read export");
        let encoded = raw_payload::encode(&received, content);
        // Media comes back from the store, which holds every object the record cut out.
        let media: HashMap<[u8; 32], Vec<u8>> = encoded
            .media
            .iter()
            .map(|object| (object.hash, object.bytes.clone()))
            .collect();
        let (_, decoded) = raw_payload::decode(&encoded.record, |hash| {
            media.get(hash).map(|bytes| Cow::Borrowed(bytes.as_slice()))
        })
        .expect("the record decodes");
        let from_record: ExportTraceServiceRequest = match content {
            RawContent::Protobuf => ExportTraceServiceRequest::decode(decoded.as_slice())
                .expect("the decoded record is the export"),
            RawContent::Json => {
                serde_json::from_slice(&decoded).expect("the decoded record is the export")
            }
        };

        for files_enabled in [false, true] {
            let direct = raw_views::render(&request, None, files_enabled);
            let through_record = raw_views::render(&from_record, None, files_enabled);
            if direct != through_record {
                failures.push(format!(
                    "{}: the record renders differently (files {files_enabled})",
                    path.display()
                ));
            }
            // A second rendering of the same input must be the same string, byte for byte.
            if direct != raw_views::render(&request, None, files_enabled) {
                failures.push(format!(
                    "{}: rendering is not deterministic (files {files_enabled})",
                    path.display()
                ));
            }
            compared += direct.len();
        }
    }

    assert!(
        failures.is_empty(),
        "{} exports differ:\n  {}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    assert!(compared > 1000, "only {compared} spans were rendered");
}

/// The stored event and link counts are the ones the rendered span carries: the two columns a list shows are
/// the only reason a reader needed the JSON, so they have to agree with it.
#[test]
fn stored_counts_agree_with_the_rendered_span() {
    let pricing = PricingService::init_for_test().expect("offline pricing");
    let mut events = 0usize;
    let mut links = 0usize;
    for path in trace_exports() {
        let Some(request) = request_of(&path) else {
            continue;
        };
        let rendered = raw_views::render(&request, None, false);
        let Some(spans) = sideseat_ingestion::traces::process_request_for_test_with_files(
            &request,
            &pricing,
            ExtractionMode::PerCarrier,
            false,
        ) else {
            continue;
        };
        for span in &spans {
            let Some(json) = rendered.get(&(span.trace_id.clone(), span.span_id.clone())) else {
                continue;
            };
            let json: serde_json::Value = serde_json::from_str(json).expect("rendered JSON");
            let count = |key: &str| {
                json.get(key)
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0)
            };
            assert_eq!(
                count("events"),
                span.event_count as usize,
                "the stored event count and the rendered events disagree for {}/{}",
                span.trace_id,
                span.span_id
            );
            assert_eq!(count("links"), span.link_count as usize);
            events += count("events");
            links += count("links");
        }
    }
    assert!(events > 100, "only {events} events were rendered");
    assert!(links > 0, "no links were rendered");
}
