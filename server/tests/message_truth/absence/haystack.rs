//! A fixture's captured payloads, decoded into what each carrier holds.
//!
//! Every trace request and log export is read whole - resource, scope, span, event, link and log
//! attributes, names, bodies and statuses - and every string is expanded through each encoding the
//! engine can read: JSON in a string, Python renderings, base64, data URIs and backslash escapes.

use std::path::PathBuf;

use base64::Engine as _;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use serde_json::Value;
use sha2::Digest as _;

use super::super::truth;

/// How deep encodings may nest before the search gives up. Exceeding it with a value that still looks
/// encoded makes every proof over the fixture unprovable, never a false absence.
pub(in super::super) const MAX_DEPTH: usize = 8;

/// One carrier's decoded content: every string it holds, every JSON subtree, every decoded byte digest.
#[derive(Debug, Default, Clone)]
pub(in super::super) struct Carrier {
    /// The span this carrier belongs to, as hex: a span with its events, or a log record a span emitted.
    pub span: Option<String>,
    /// Whitespace-collapsed strings with the location each came from.
    pub strings: Vec<(String, String)>,
    pub nodes: Vec<(String, Value)>,
    pub digests: Vec<(String, String)>,
}

/// Everything a fixture's captured payloads carry, carrier by carrier.
#[derive(Debug, Default)]
pub(in super::super) struct Haystack {
    pub carriers: Vec<Carrier>,
    /// Locations whose content still looked encoded at `MAX_DEPTH`.
    pub undecoded: Vec<String>,
}

impl Haystack {
    /// Every trace request and log export captured for one fixture.
    pub(in super::super) fn of_fixture(paths: &[PathBuf]) -> Self {
        let file = |path: &PathBuf| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let traces: Vec<(String, ExportTraceServiceRequest)> = paths
            .iter()
            .map(|p| (file(p), crate::decode_request(p)))
            .collect();
        let logs: Vec<(String, ExportLogsServiceRequest)> = crate::log_exports_beside(paths)
            .iter()
            .map(|p| (file(p), crate::decode_logs(p)))
            .collect();
        Self::from_requests(&traces, &logs)
    }

    pub(in super::super) fn from_requests(
        traces: &[(String, ExportTraceServiceRequest)],
        logs: &[(String, ExportLogsServiceRequest)],
    ) -> Self {
        let mut haystack = Haystack::default();
        for (file, request) in traces {
            for (r, resource_spans) in request.resource_spans.iter().enumerate() {
                if let Some(resource) = &resource_spans.resource {
                    let at = format!("{file} resource[{r}]");
                    haystack.carrier(None, |e| e.attributes(&at, &resource.attributes));
                }
                for (s, scope_spans) in resource_spans.scope_spans.iter().enumerate() {
                    if let Some(scope) = &scope_spans.scope {
                        let at = format!("{file} resource[{r}].scope[{s}]");
                        haystack.carrier(None, |e| {
                            e.string(&format!("{at}.name"), &scope.name, 0);
                            e.string(&format!("{at}.version"), &scope.version, 0);
                            e.attributes(&at, &scope.attributes);
                        });
                    }
                    for span in &scope_spans.spans {
                        let at = format!("{file} span {:?}", span.name);
                        haystack.carrier(Some(hex(&span.span_id)), |e| {
                            e.string(&format!("{at}.name"), &span.name, 0);
                            e.string(&format!("{at}.trace_state"), &span.trace_state, 0);
                            e.attributes(&at, &span.attributes);
                            if let Some(status) = &span.status {
                                e.string(&format!("{at}.status"), &status.message, 0);
                            }
                            for (i, event) in span.events.iter().enumerate() {
                                let at = format!("{at}.events[{i}]");
                                e.string(&format!("{at}.name"), &event.name, 0);
                                e.attributes(&at, &event.attributes);
                            }
                            for (i, link) in span.links.iter().enumerate() {
                                let at = format!("{at}.links[{i}]");
                                e.string(&format!("{at}.trace_state"), &link.trace_state, 0);
                                e.attributes(&at, &link.attributes);
                            }
                        });
                    }
                }
            }
        }
        for (file, request) in logs {
            for (r, resource_logs) in request.resource_logs.iter().enumerate() {
                if let Some(resource) = &resource_logs.resource {
                    let at = format!("{file} resource[{r}]");
                    haystack.carrier(None, |e| e.attributes(&at, &resource.attributes));
                }
                for (s, scope_logs) in resource_logs.scope_logs.iter().enumerate() {
                    if let Some(scope) = &scope_logs.scope {
                        let at = format!("{file} resource[{r}].scope[{s}]");
                        haystack.carrier(None, |e| {
                            e.string(&format!("{at}.name"), &scope.name, 0);
                            e.string(&format!("{at}.version"), &scope.version, 0);
                            e.attributes(&at, &scope.attributes);
                        });
                    }
                    for (i, record) in scope_logs.log_records.iter().enumerate() {
                        let at = format!("{file} log[{i}] {:?}", record.event_name);
                        let span = (!record.span_id.is_empty()).then(|| hex(&record.span_id));
                        haystack.carrier(span, |e| {
                            e.string(&format!("{at}.event_name"), &record.event_name, 0);
                            e.string(&format!("{at}.severity"), &record.severity_text, 0);
                            if let Some(body) = &record.body {
                                e.any_value(&format!("{at}.body"), body, 0);
                            }
                            e.attributes(&at, &record.attributes);
                        });
                    }
                }
            }
        }
        haystack
    }

    fn carrier(&mut self, span: Option<String>, fill: impl FnOnce(&mut Expander<'_>)) {
        let mut carrier = Carrier {
            span,
            ..Carrier::default()
        };
        let mut expander = Expander {
            carrier: &mut carrier,
            undecoded: &mut self.undecoded,
        };
        fill(&mut expander);
        self.carriers.push(carrier);
    }
}

/// Decodes one carrier's values into strings, JSON subtrees and byte digests.
struct Expander<'a> {
    carrier: &'a mut Carrier,
    undecoded: &'a mut Vec<String>,
}

impl Expander<'_> {
    fn attributes(&mut self, at: &str, attributes: &[KeyValue]) {
        for attribute in attributes {
            let at = format!("{at}.{}", attribute.key);
            if let Some(value) = &attribute.value {
                self.any_value(&at, value, 0);
            }
        }
    }

    fn any_value(&mut self, at: &str, value: &AnyValue, depth: usize) {
        // A structured value is also one JSON tree, so an argument object sent as a key-value list
        // compares equal to the truth's object.
        let tree = any_value_json(value);
        if tree.is_object() || tree.is_array() {
            self.carrier.nodes.push((at.to_string(), tree));
        }
        match &value.value {
            Some(any_value::Value::StringValue(text)) => self.string(at, text, depth),
            Some(any_value::Value::BytesValue(bytes)) => self.bytes(at, bytes, depth),
            Some(any_value::Value::ArrayValue(array)) => {
                for (i, item) in array.values.iter().enumerate() {
                    self.any_value(&format!("{at}[{i}]"), item, depth);
                }
            }
            Some(any_value::Value::KvlistValue(list)) => {
                for entry in &list.values {
                    self.string(&format!("{at}.<key>"), &entry.key, depth);
                    if let Some(inner) = &entry.value {
                        self.any_value(&format!("{at}.{}", entry.key), inner, depth);
                    }
                }
            }
            Some(any_value::Value::IntValue(_) | any_value::Value::DoubleValue(_))
            | Some(any_value::Value::BoolValue(_))
            | None => {}
        }
    }

    fn string(&mut self, at: &str, text: &str, depth: usize) {
        if text.is_empty() {
            return;
        }
        self.carrier
            .strings
            .push((at.to_string(), collapse_whitespace(text)));
        let trimmed = text.trim();
        if depth >= MAX_DEPTH {
            if looks_encoded(trimmed) {
                self.undecoded.push(at.to_string());
            }
            return;
        }
        let parsed = serde_json::from_str::<Value>(trimmed)
            .ok()
            .filter(|v| !v.is_number() && !v.is_boolean() && !v.is_null())
            .map(|v| (v, "json"))
            .or_else(|| {
                sideseat_domain::sideml::test_support::parse_python_rendering(trimmed)
                    .map(|v| (v, "python"))
            });
        if let Some((value, how)) = parsed {
            self.json(&format!("{at}|{how}"), &value, depth + 1);
        }
        if let Some(bytes) = data_uri(trimmed) {
            self.bytes(&format!("{at}|data-uri"), &bytes, depth + 1);
        } else if let Some(bytes) = base64_payload(trimmed) {
            self.bytes(&format!("{at}|base64"), &bytes, depth + 1);
        }
        if trimmed.contains('\\') {
            let unescaped = unescape(trimmed);
            if unescaped != trimmed {
                self.string(&format!("{at}|unescaped"), &unescaped, depth + 1);
            }
        }
    }

    fn bytes(&mut self, at: &str, bytes: &[u8], depth: usize) {
        self.carrier.digests.push((
            at.to_string(),
            truth::hex_digest(&sha2::Sha256::digest(bytes)),
        ));
        if let Ok(text) = std::str::from_utf8(bytes)
            && !text.chars().any(|c| c.is_control() && !c.is_whitespace())
        {
            self.string(at, text, depth);
        }
    }

    fn json(&mut self, at: &str, value: &Value, depth: usize) {
        match value {
            Value::String(text) => self.string(at, text, depth),
            Value::Array(items) => {
                self.carrier.nodes.push((at.to_string(), value.clone()));
                for (i, item) in items.iter().enumerate() {
                    self.json(&format!("{at}[{i}]"), item, depth);
                }
            }
            Value::Object(members) => {
                self.carrier.nodes.push((at.to_string(), value.clone()));
                for (key, item) in members {
                    // A member name is searched as it stands, never decoded.
                    self.carrier
                        .strings
                        .push((format!("{at}.<key>"), collapse_whitespace(key)));
                    self.json(&format!("{at}.{key}"), item, depth);
                }
            }
            Value::Number(_) | Value::Bool(_) | Value::Null => {}
        }
    }
}

fn any_value_json(value: &AnyValue) -> Value {
    match &value.value {
        Some(any_value::Value::StringValue(text)) => Value::String(text.clone()),
        Some(any_value::Value::BoolValue(b)) => Value::Bool(*b),
        Some(any_value::Value::IntValue(i)) => Value::from(*i),
        Some(any_value::Value::DoubleValue(d)) => Value::from(*d),
        Some(any_value::Value::BytesValue(bytes)) => {
            Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
        }
        Some(any_value::Value::ArrayValue(array)) => {
            Value::Array(array.values.iter().map(any_value_json).collect())
        }
        Some(any_value::Value::KvlistValue(list)) => Value::Object(
            list.values
                .iter()
                .map(|kv| {
                    let inner = kv.value.as_ref().map_or(Value::Null, any_value_json);
                    (kv.key.clone(), inner)
                })
                .collect(),
        ),
        None => Value::Null,
    }
}

pub(super) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a value at the depth limit would still decode further, so the search cannot vouch for it.
fn looks_encoded(text: &str) -> bool {
    text.starts_with(['{', '[', '"', '('])
        || text.starts_with("data:")
        || base64_payload(text).is_some()
        || text.contains('\\')
}

/// A `data:` URI's payload: base64 when it says so, percent-encoded otherwise.
fn data_uri(text: &str) -> Option<Vec<u8>> {
    let rest = text.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    if meta.ends_with(";base64") {
        decode_base64(payload)
    } else {
        Some(percent_decode(payload))
    }
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// A string that is entirely base64, in any of the standard alphabets, padded or not.
fn base64_payload(text: &str) -> Option<Vec<u8>> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() < 16
        || !compact
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'))
    {
        return None;
    }
    decode_base64(&compact)
}

fn decode_base64(payload: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    let compact: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(&compact).ok())
}

/// Backslash escapes as a JSON or Python string literal writes them.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('u') => {
                let code: String = (0..4).filter_map(|_| chars.next()).collect();
                match u32::from_str_radix(&code, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => out.push(decoded),
                    None => {
                        out.push_str("\\u");
                        out.push_str(&code);
                    }
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Lowercase hex, as a span id is written everywhere else.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
