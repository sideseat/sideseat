//! The span identities a raw record holds, read from its framing without rebuilding it.
//!
//! Settlement and the exact-redelivery check ask which spans a stored record holds, of records that can be large
//! and many: a redelivery names whichever exports first carried its spans. Rebuilding a record's shape allocated
//! every medium as base64 and decoded the whole export into owned strings, so the cost followed the stored
//! content rather than the question. This walks the shape's segments instead. A protobuf record is decoded into a
//! message with only the two identity fields, so every other field - every medium among them - is skipped by its
//! length without being read. A JSON record is parsed from its stored text with the media left out - a medium
//! sits inside a string, and JSON has no lengths to keep - and only the identities are kept.
//!
//! Every stored record was decoded in full when it was received, so the two readings agree on what a stored
//! record holds; a record that is not a record, or not OTLP at all, is unreadable to both.

use std::collections::HashSet;

use prost::Message;
use prost::bytes::Buf;
use serde::Deserialize;
use sideseat_domain::raw_payload::{self, RawContent, ShapeSegment};

/// A span's identity: hex trace id and hex span id, as rows store them.
pub(crate) type SpanIdentity = (String, String);

/// The span identities `record` holds.
pub(crate) fn record_identities(record: &[u8]) -> Result<HashSet<SpanIdentity>, String> {
    let (content, segments) =
        raw_payload::shape_segments(record).map_err(|error| error.to_string())?;
    let mut held = HashSet::new();
    match content {
        RawContent::Protobuf => {
            let request = Identities::decode(ShapeBuf::new(&segments))
                .map_err(|error| format!("raw protobuf is invalid: {error}"))?;
            for span in request
                .resource_spans
                .iter()
                .flat_map(|resource| &resource.scope_spans)
                .flat_map(|scope| &scope.spans)
            {
                held.insert((hex::encode(&span.trace_id), hex::encode(&span.span_id)));
            }
        }
        RawContent::Json => {
            // JSON has no length prefixes, and a medium sits inside a string, so the text without the media
            // parses to the same structure - from the stored bytes alone, where a reader over the blanks scanned
            // every byte of every medium.
            let mut text = Vec::with_capacity(
                segments
                    .iter()
                    .map(|segment| match segment {
                        ShapeSegment::Bytes(bytes) => bytes.len(),
                        ShapeSegment::Blank(_) => 0,
                    })
                    .sum(),
            );
            for segment in &segments {
                if let ShapeSegment::Bytes(bytes) = segment {
                    text.extend_from_slice(bytes);
                }
            }
            let request: JsonIdentities = serde_json::from_slice(&text)
                .map_err(|error| format!("raw OTLP/JSON is invalid: {error}"))?;
            for span in request
                .resource_spans
                .into_iter()
                .flat_map(|resource| resource.scope_spans)
                .flat_map(|scope| scope.spans)
            {
                held.insert((span.trace_id, span.span_id));
            }
        }
    }
    Ok(held)
}

/// An OTLP trace export with only what identifies its spans; the tags are the export's own.
#[derive(Clone, PartialEq, Message)]
struct Identities {
    #[prost(message, repeated, tag = "1")]
    resource_spans: Vec<ResourceIdentities>,
}

#[derive(Clone, PartialEq, Message)]
struct ResourceIdentities {
    #[prost(message, repeated, tag = "2")]
    scope_spans: Vec<ScopeIdentities>,
}

#[derive(Clone, PartialEq, Message)]
struct ScopeIdentities {
    #[prost(message, repeated, tag = "2")]
    spans: Vec<SpanIdentities>,
}

#[derive(Clone, PartialEq, Message)]
struct SpanIdentities {
    #[prost(bytes = "vec", tag = "1")]
    trace_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    span_id: Vec<u8>,
}

/// The OTLP/JSON spelling of the same, as the export's own decoding reads it: camel case, every field optional,
/// identities as hex of either case. Normalised to lower-case hex, the form rows store.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonIdentities {
    resource_spans: Vec<JsonResourceIdentities>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonResourceIdentities {
    scope_spans: Vec<JsonScopeIdentities>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonScopeIdentities {
    spans: Vec<JsonSpanIdentities>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonSpanIdentities {
    #[serde(deserialize_with = "hex_identity")]
    trace_id: String,
    #[serde(deserialize_with = "hex_identity")]
    span_id: String,
}

fn hex_identity<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    hex::decode(text)
        .map(hex::encode)
        .map_err(serde::de::Error::custom)
}

/// Blank base64 for the shape's media: the text of zero bytes, handed out a slice at a time.
static BLANK: [u8; 4096] = [b'A'; 4096];

/// A record's shape as one buffer, over its segments, without materialising its media.
struct ShapeBuf<'a> {
    segments: &'a [ShapeSegment<'a>],
    /// Bytes already consumed of `segments[0]`.
    offset: usize,
    remaining: usize,
}

impl<'a> ShapeBuf<'a> {
    fn new(segments: &'a [ShapeSegment<'a>]) -> Self {
        let remaining = segments.iter().map(|segment| segment_len(segment)).sum();
        let mut shape = Self {
            segments,
            offset: 0,
            remaining,
        };
        shape.skip_spent();
        shape
    }

    fn skip_spent(&mut self) {
        while let Some(first) = self.segments.first() {
            if self.offset < segment_len(first) {
                break;
            }
            self.segments = &self.segments[1..];
            self.offset = 0;
        }
    }
}

fn segment_len(segment: &ShapeSegment<'_>) -> usize {
    match segment {
        ShapeSegment::Bytes(bytes) => bytes.len(),
        ShapeSegment::Blank(length) => *length,
    }
}

impl Buf for ShapeBuf<'_> {
    fn remaining(&self) -> usize {
        self.remaining
    }

    fn chunk(&self) -> &[u8] {
        match self.segments.first() {
            Some(ShapeSegment::Bytes(bytes)) => &bytes[self.offset..],
            Some(ShapeSegment::Blank(length)) => &BLANK[..(length - self.offset).min(BLANK.len())],
            None => &[],
        }
    }

    fn advance(&mut self, mut count: usize) {
        assert!(
            count <= self.remaining,
            "advanced past the end of a record's shape"
        );
        self.remaining -= count;
        while count > 0 {
            let Some(first) = self.segments.first() else {
                return;
            };
            let step = (segment_len(first) - self.offset).min(count);
            self.offset += step;
            count -= step;
            self.skip_spent();
        }
    }
}

#[cfg(test)]
#[path = "raw_identities_tests.rs"]
mod tests;
