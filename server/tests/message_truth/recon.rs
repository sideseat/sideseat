//! A fixture's reconstruction, as the truth checks see it.
//!
//! Built by the golden pipeline itself (`build_golden`), so the truth is compared with exactly the four
//! views the goldens record - span, trace, session, feed - but holding each block's full content. The
//! mutation catalogue edits this model directly, which is what lets it simulate a parser defect without
//! hand-editing a captured fixture.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use sideseat_ports::types::{MessageSpanRow, NormalizedSpan, ObservationType};

use crate::{Built, Scope, attach_log_messages, build_golden, decode_request, message_row};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Block {
    pub role: String,
    pub kind: String,
    pub content: Value,
    pub tool_use_id: Option<String>,
    pub trace: String,
    pub span: String,
    /// The pipeline reads the block as its span's output, not as context sent to it.
    pub output: bool,
    /// The normalised finish reason the block carries.
    pub finish: Option<&'static str>,
    /// SHA-256 of an attachment's decoded bytes, where the block keeps them inline.
    pub media_sha256: Option<String>,
    /// Digest of role, type and full content: "the same block" in every view.
    pub digest: String,
}

impl Block {
    pub fn is(&self, role: &str, kind: &str) -> bool {
        self.role == role && self.kind == kind
    }

    pub fn text(&self) -> Option<&str> {
        self.content.get("text").and_then(Value::as_str)
    }

    pub fn is_tool_result(&self) -> bool {
        self.kind == "tool_result"
    }

    /// The call id a tool result answers.
    pub fn result_call_id(&self) -> Option<&str> {
        self.content
            .get("tool_use_id")
            .and_then(Value::as_str)
            .or(self.tool_use_id.as_deref())
    }

    /// A tool call's own id.
    pub fn call_id(&self) -> Option<&str> {
        self.content
            .get("id")
            .and_then(Value::as_str)
            .or(self.tool_use_id.as_deref())
    }

    /// Recomputes what is derived from the role, type and content after any of them changed.
    pub fn refresh(&mut self) {
        self.media_sha256 = media_sha256(&self.content);
        self.digest = format!(
            "{}/{}/{}",
            self.role,
            self.kind,
            crate::content_digest(&self.content)
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ViewKind {
    Span,
    Trace,
    Session,
    Feed,
}

impl ViewKind {
    pub fn name(self) -> &'static str {
        match self {
            ViewKind::Span => "span",
            ViewKind::Trace => "trace",
            ViewKind::Session => "session",
            ViewKind::Feed => "feed",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct View {
    pub kind: ViewKind,
    /// The span a span view shows, or the session a session view shows.
    pub key: String,
    pub blocks: Vec<Block>,
}

/// What a span reports about the model call it may have recorded, beside its messages.
///
/// Every span is a candidate, not only those typed `generation`: Bedrock's native instrumentation
/// records a model call as a plain span and CrewAI's as an agent or chain. The type only breaks ties.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Generation {
    pub label: String,
    pub trace: String,
    pub span: String,
    /// Typed `generation` by the pipeline.
    pub typed: bool,
    /// Every ancestor span id, nearest first.
    pub ancestors: Vec<String>,
    pub start: chrono::DateTime<chrono::Utc>,
    pub request_model: Option<String>,
    pub response_model: Option<String>,
    pub response_id: Option<String>,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
    /// The span's finish reasons and those its output blocks carry.
    pub finish: Vec<String>,
    pub failed: bool,
    pub error: Option<String>,
    /// The span as it arrived, for telling a value the telemetry never carried from one the
    /// pipeline failed to read.
    pub raw: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct Recon {
    pub fixture: String,
    pub views: Vec<View>,
    pub generations: Vec<Generation>,
    pub session_of_trace: BTreeMap<String, String>,
}

impl Recon {
    pub fn span_view(&self, span: &str) -> Option<&View> {
        self.views
            .iter()
            .find(|v| v.kind == ViewKind::Span && v.key == span)
    }
}

/// Every span of a fixture, keyed `<trace>/<span>`; a span re-delivered by a later request keeps its
/// first copy, which is the one the golden rows also read first.
pub(crate) type Spans = BTreeMap<String, NormalizedSpan>;

/// Replays one fixture's requests and logs, keeping each span's metadata beside its message row.
///
/// The rows are exactly `rows_for`'s: one ingestion pass serves the golden and the truth checks.
pub(crate) fn read(paths: &[PathBuf]) -> (Vec<(String, MessageSpanRow)>, Spans) {
    let pricing =
        sideseat_domain::pricing::PricingService::init_for_test().expect("offline pricing service");
    let mut rows = Vec::new();
    let mut spans: BTreeMap<String, NormalizedSpan> = BTreeMap::new();
    for path in paths {
        let request = decode_request(path);
        let Some(normalized) = sideseat_ingestion::traces::process_request_for_test_with_mode(
            &request,
            &pricing,
            sideseat_ingestion::traces::extract::ExtractionMode::PerCarrier,
        ) else {
            continue;
        };
        for span in normalized {
            rows.push((span.span_name.clone(), message_row(&span)));
            spans
                .entry(format!("{}/{}", span.trace_id, span.span_id))
                .or_insert(span);
        }
    }
    attach_log_messages(paths, &mut rows);
    (rows, spans)
}

pub(super) fn build(fixture: &str, paths: &[PathBuf]) -> Recon {
    let (rows, spans) = read(paths);
    let built = build_golden(fixture, paths, &rows);
    from_built(fixture, &built, spans)
}

/// SHA-256 of inline base64 bytes; `None` for a URL, a placeholder, or no data at all.
fn media_sha256(content: &Value) -> Option<String> {
    use base64::Engine as _;
    use sha2::Digest as _;
    if content.get("source").and_then(Value::as_str) != Some("base64") {
        return None;
    }
    let data = content.get("data").and_then(Value::as_str)?.trim();
    if data.is_empty() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some(super::truth::hex_digest(&sha2::Sha256::digest(&bytes)))
}

pub(super) fn from_built(fixture: &str, built: &Built, mut spans: Spans) -> Recon {
    let parent_of: BTreeMap<(String, String), String> = spans
        .values()
        .filter_map(|s| {
            s.parent_span_id
                .clone()
                .map(|p| ((s.trace_id.clone(), s.span_id.clone()), p))
        })
        .collect();
    let mut views = Vec::new();
    let mut generations = Vec::new();
    for (label, scope, rows) in &built.invariants {
        let (kind, key) = match scope {
            Scope::Span { span_id, .. } => (ViewKind::Span, span_id.clone()),
            Scope::Trace { trace_id } => (ViewKind::Trace, trace_id.clone()),
            Scope::Session => (
                ViewKind::Session,
                label.strip_prefix("session ").unwrap_or(label).to_string(),
            ),
            Scope::Feed => (ViewKind::Feed, String::new()),
        };
        let blocks: Vec<Block> = rows
            .iter()
            .map(|row| {
                let content: Value =
                    serde_json::from_str(&row.full_content).expect("a reconstructed block is JSON");
                let mut block = Block {
                    role: row.role.clone(),
                    kind: row.entry_type.clone(),
                    media_sha256: None,
                    content,
                    tool_use_id: row.tool_use_id.clone(),
                    trace: row.trace_id.clone(),
                    span: row.span_id.clone(),
                    output: row.is_output,
                    finish: row.finish,
                    digest: String::new(),
                };
                block.refresh();
                block
            })
            .collect();
        if let Scope::Span { trace_id, span_id } = scope
            && let Some(span) = spans.get_mut(&format!("{trace_id}/{span_id}"))
        {
            let mut finish = span.gen_ai_finish_reasons.clone();
            finish.extend(
                blocks
                    .iter()
                    .filter(|b| b.output)
                    .filter_map(|b| b.finish.map(str::to_owned)),
            );
            finish.dedup();
            let mut ancestors = Vec::new();
            let mut cursor = span_id.clone();
            while let Some(parent) = parent_of.get(&(trace_id.clone(), cursor.clone())) {
                if ancestors.contains(parent) {
                    break;
                }
                ancestors.push(parent.clone());
                cursor = parent.clone();
            }
            generations.push(Generation {
                label: label.strip_prefix("span ").unwrap_or(label).to_string(),
                trace: trace_id.clone(),
                span: span_id.clone(),
                typed: span.observation_type == Some(ObservationType::Generation),
                ancestors,
                start: span.timestamp_start,
                request_model: span.gen_ai_request_model.clone(),
                response_model: span.gen_ai_response_model.clone(),
                response_id: span.gen_ai_response_id.clone(),
                input: span.gen_ai_usage_input_tokens,
                output: span.gen_ai_usage_output_tokens,
                cache_read: span.gen_ai_usage_cache_read_tokens,
                cache_write: span.gen_ai_usage_cache_write_tokens,
                reasoning: span.gen_ai_usage_reasoning_tokens,
                finish,
                failed: span.status_code.as_deref() == Some("ERROR"),
                error: span
                    .exception_message
                    .clone()
                    .or_else(|| span.status_message.clone()),
                raw: span.raw_span.take(),
            });
        }
        views.push(View { kind, key, blocks });
    }
    let session_of_trace = built
        .traces_of_session
        .iter()
        .flat_map(|(session, traces)| traces.iter().map(move |t| (t.clone(), session.clone())))
        .collect();
    Recon {
        fixture: fixture.to_string(),
        views,
        generations,
        session_of_trace,
    }
}
