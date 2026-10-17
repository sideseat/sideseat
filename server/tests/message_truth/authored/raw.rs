//! The raw spans a framework text is proven against: every string each holds, and apart from them what the span
//! states as its own output - as the readers read it.
//!
//! The output side is not restated here. It is what the extraction readers produced for the span (the ingested
//! row's messages and log messages, normalized), kept where the carrier holds the span's output by the carrier facts
//! and the message is the assistant's: its text blocks, and its structured output compared as JSON. So the oracle's
//! carrier, member, role and reasoning vocabulary is the assets' and cannot drift from the readers. The one thing kept
//! independent is the bytes: a shown text counts only where the raw span holds it verbatim.

use std::collections::BTreeMap;
use std::path::PathBuf;

use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value::Value as Otlp};
use serde_json::Value;
use sideseat_domain::observations::{MessageSource, RawMessage};
use sideseat_domain::sideml::{ChatRole, ContentBlock};

use super::super::truth::hex_digest;

/// One raw span: its id, and every string its attributes and events hold, JSON decoded, each with when it was
/// written - an event's own time, or the span's end for its attributes.
pub(super) struct RawSpan {
    pub trace: String,
    pub id: String,
    pub strings: Vec<(String, u64)>,
    /// What the span states as its own output, as the readers read it (`reader_outputs`), each as the raw span holds
    /// it, with when it was written.
    pub outputs: Vec<(String, u64)>,
}

/// What a reader showed as a span's own output: a text, or structured output.
enum Shown {
    Text(String),
    Json(Value),
}

/// A shown output with its event's time, where an event carried it.
type Dated = (Shown, Option<u64>);

/// A time as OTLP's unsigned nanoseconds: the whole u64 range, which a signed nanosecond count does not reach.
fn nanos(time: chrono::DateTime<chrono::Utc>) -> Option<u64> {
    u64::try_from(time.timestamp())
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u64::from(time.timestamp_subsec_nanos()))
}

/// For each span, by trace and id, what the readers show as its own output (`shown`).
fn reader_outputs(paths: &[PathBuf]) -> BTreeMap<(String, String), Vec<Dated>> {
    let (rows, _) = super::super::recon::read(paths);
    let mut out: BTreeMap<(String, String), Vec<Dated>> = BTreeMap::new();
    for (_, row) in rows {
        let mut raw: Vec<RawMessage> = serde_json::from_str(&row.messages_json).unwrap_or_default();
        raw.extend(
            serde_json::from_str::<Vec<RawMessage>>(&row.log_messages_json).unwrap_or_default(),
        );
        for message in sideseat_domain::sideml::to_sideml(&raw) {
            let found = shown(
                &message,
                row.observation_type.as_deref(),
                row.span_name.as_deref(),
            );
            if !found.is_empty() {
                out.entry((row.trace_id.clone(), row.span_id.clone()))
                    .or_default()
                    .extend(found);
            }
        }
    }
    out
}

/// What one read message shows as its span's own output: on a span the assets do not classify as a tool's execution
/// (whose output is the tool's), an assistant's message from a carrier the carrier facts say holds the span's
/// output - its text blocks, and its structured output - with the event's time for an event carrier.
fn shown(
    message: &sideseat_domain::sideml::SideMLMessage,
    observation_type: Option<&str>,
    span_name: Option<&str>,
) -> Vec<Dated> {
    if observation_type == Some("tool") || message.sideml.role != ChatRole::Assistant {
        return Vec::new();
    }
    let (event, attribute, at) = match &message.source {
        // An event is dated by its own time and nothing else: one that cannot be stated is no output that can be
        // placed, never the span's end in its stead.
        MessageSource::Event { name, time } => match nanos(*time) {
            Some(at) => (Some(name.as_str()), None, Some(at)),
            None => return Vec::new(),
        },
        MessageSource::Attribute { key, .. } => (None, Some(key.as_str()), None),
    };
    let output = sideseat_domain::sideml::carrier::semantics_for_context(
        &sideseat_domain::rules::CarrierContext {
            event,
            attribute,
            observation_type,
            span_name,
            direction: message.direction,
        },
    )
    .carrier_holds_span_output;
    if !output {
        return Vec::new();
    }
    message
        .sideml
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some((Shown::Text(text.clone()), at)),
            ContentBlock::Json { data } => Some((Shown::Json(data.clone()), at)),
            _ => None,
        })
        .collect()
}

/// Each shown output as the raw span holds it, byte for byte: a text verbatim, structured output as a string that
/// parses to it. A reader's text no payload of the span holds is no output of the span.
fn held(
    shown: Vec<(Shown, Option<u64>)>,
    strings: &[(String, u64)],
    end: u64,
) -> Vec<(String, u64)> {
    let mut outputs = Vec::new();
    for (shown, at) in shown {
        let at = at.unwrap_or(end);
        match shown {
            Shown::Text(text) => {
                if strings.iter().any(|(s, _)| *s == text) {
                    outputs.push((text, at));
                }
            }
            Shown::Json(data) => outputs.extend(
                strings
                    .iter()
                    .filter(|(s, _)| serde_json::from_str::<Value>(s).ok().as_ref() == Some(&data))
                    .map(|(s, _)| (s.clone(), at)),
            ),
        }
    }
    outputs.sort_unstable();
    outputs.dedup();
    outputs
}

pub(super) fn raw_spans(paths: &[PathBuf]) -> Vec<RawSpan> {
    let mut shown = reader_outputs(paths);
    let mut out = Vec::new();
    for path in paths {
        let request = crate::decode_request(path);
        for span in request
            .resource_spans
            .iter()
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
        {
            let mut strings = Vec::new();
            for kv in &span.attributes {
                if let Some(value) = &kv.value {
                    let mut found = Vec::new();
                    collect_otlp(value, &mut found);
                    strings.extend(found.into_iter().map(|s| (s, span.end_time_unix_nano)));
                }
            }
            for event in &span.events {
                for kv in &event.attributes {
                    if let Some(value) = &kv.value {
                        let mut found = Vec::new();
                        collect_otlp(value, &mut found);
                        strings.extend(found.into_iter().map(|s| (s, event.time_unix_nano)));
                    }
                }
            }
            let (trace, id) = (hex_digest(&span.trace_id), hex_digest(&span.span_id));
            let outputs = held(
                shown
                    .remove(&(trace.clone(), id.clone()))
                    .unwrap_or_default(),
                &strings,
                span.end_time_unix_nano,
            );
            out.push(RawSpan {
                trace,
                id,
                strings,
                outputs,
            });
        }
    }
    out
}

pub(super) fn collect_otlp(value: &AnyValue, out: &mut Vec<String>) {
    match &value.value {
        Some(Otlp::StringValue(s)) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(s)
                && (parsed.is_object() || parsed.is_array())
            {
                collect_json(&parsed, out);
            }
            out.push(s.clone());
        }
        Some(Otlp::ArrayValue(items)) => items.values.iter().for_each(|v| collect_otlp(v, out)),
        Some(Otlp::KvlistValue(list)) => list
            .values
            .iter()
            .filter_map(|kv| kv.value.as_ref())
            .for_each(|v| collect_otlp(v, out)),
        _ => {}
    }
}

fn collect_json(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| collect_json(v, out)),
        Value::Object(members) => members.values().for_each(|v| collect_json(v, out)),
        _ => {}
    }
}

#[cfg(test)]
#[path = "raw_tests.rs"]
mod tests;
