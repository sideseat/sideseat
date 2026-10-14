//! A metric data point's exemplars as they are stored: each once, in one list on the point's row.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use sideseat_core::utils::time::nanos_to_datetime;

/// One exemplar of a metric data point, as stored.
///
/// Each exemplar is kept once, in the order the producer sent them. The answers that name an exemplar - the first
/// one's trace, span and instant, and the whole set as a JSON array - are rebuilt from these, so the ids are their
/// bytes rather than hex and the instant is the producer's nanoseconds rather than text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricExemplar {
    /// The sampled trace's id; `None` where the producer sent none or all zeroes.
    pub trace_id: Option<Vec<u8>>,
    /// The sampled span's id; `None` where the producer sent none or all zeroes.
    pub span_id: Option<Vec<u8>>,
    /// When it was sampled, in nanoseconds since the epoch; 0 where the producer did not say.
    pub time_unix_nano: u64,
    pub value_int: Option<i64>,
    pub value_double: Option<f64>,
    /// The filtered attributes as typed JSON (`otlp::attrs_to_typed_json`); `None` where there are none.
    pub attributes: Option<String>,
}

impl MetricExemplar {
    /// A trace id as stored: `None` for an empty id or the all-zero one OTLP uses for "none".
    pub fn stored_trace_id(bytes: &[u8]) -> Option<Vec<u8>> {
        stored_id(bytes, 16)
    }

    /// A span id as stored: `None` for an empty id or the all-zero one OTLP uses for "none".
    pub fn stored_span_id(bytes: &[u8]) -> Option<Vec<u8>> {
        stored_id(bytes, 8)
    }

    /// An id from the hex a store keeps it as; `None` for text that is not an id's hex.
    pub fn id_from_hex(hex: &str) -> Option<Vec<u8>> {
        hex::decode(hex).ok()
    }

    pub fn trace_id_hex(&self) -> Option<String> {
        self.trace_id.as_deref().map(hex::encode)
    }

    pub fn span_id_hex(&self) -> Option<String> {
        self.span_id.as_deref().map(hex::encode)
    }

    /// When it was sampled; `None` where the producer did not say.
    pub fn timestamp(&self) -> Option<DateTime<Utc>> {
        (self.time_unix_nano > 0).then(|| nanos_to_datetime(self.time_unix_nano))
    }

    /// When it was sampled, to the microsecond the analytics stores keep instants at, as the API's first-exemplar
    /// instant answers; the array keeps the nanoseconds.
    pub fn stored_timestamp(&self) -> Option<DateTime<Utc>> {
        self.timestamp()
            .and_then(|instant| DateTime::from_timestamp_micros(instant.timestamp_micros()))
    }

    /// The exemplar as the API's array holds it, written into `out`: its trace and span ids in hex, its value, its
    /// instant in RFC 3339 and its attributes, each only where it has one, in that order. The attributes are the
    /// stored JSON text itself: parsed and written again, a double in them could come back as another spelling.
    fn write_json(&self, out: &mut String) {
        let mut fields: Vec<String> = Vec::with_capacity(5);
        let text = |value: &str| JsonValue::String(value.to_string()).to_string();
        if let Some(trace_id) = self.trace_id_hex() {
            fields.push(format!("\"trace_id\":{}", text(&trace_id)));
        }
        if let Some(span_id) = self.span_id_hex() {
            fields.push(format!("\"span_id\":{}", text(&span_id)));
        }
        if let Some(value) = self.value_int {
            fields.push(format!("\"value_int\":{}", serde_json::json!(value)));
        } else if let Some(value) = self.value_double {
            fields.push(format!("\"value_double\":{}", serde_json::json!(value)));
        }
        if let Some(timestamp) = self.timestamp() {
            fields.push(format!("\"timestamp\":{}", text(&timestamp.to_rfc3339())));
        }
        if let Some(attributes) = self.attributes.as_deref() {
            fields.push(format!("\"attributes\":{attributes}"));
        }
        out.push('{');
        out.push_str(&fields.join(","));
        out.push('}');
    }
}

/// An id as stored: `None` for an empty one or the all-zero id of its width, which OTLP sends for "none".
fn stored_id(bytes: &[u8], width: usize) -> Option<Vec<u8>> {
    let none = bytes.is_empty() || (bytes.len() == width && bytes.iter().all(|byte| *byte == 0));
    (!none).then(|| bytes.to_vec())
}

/// A data point's exemplars as the API's JSON array; `None` where it has none.
pub fn exemplars_json(exemplars: &[MetricExemplar]) -> Option<String> {
    (!exemplars.is_empty()).then(|| {
        let mut out = String::from("[");
        for (index, exemplar) in exemplars.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            exemplar.write_json(&mut out);
        }
        out.push(']');
        out
    })
}
