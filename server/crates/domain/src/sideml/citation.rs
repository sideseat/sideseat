//! The one shape every provider's citations take in SideML.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// What a citation points into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationKind {
    /// A web page, by URL.
    Url,
    /// A document sent with the request.
    Document,
    /// A file uploaded to the provider, by id.
    File,
    /// A search result the request supplied.
    SearchResult,
    /// A citation in a shape no vocabulary reads yet, kept with the provider's own item so it is seen arriving.
    Unknown,
}

/// A source a text cites: one shape for every provider's citations. Every kind but `unknown` names its source,
/// and only an `unknown` one carries the provider's item; a value breaking either is not a citation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "CitationShape")]
pub struct Citation {
    pub kind: CitationKind,
    /// Which one: the page's URL, the file's id, the document's name. Every kind but `unknown` names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The span of **this text** the citation applies to, as offsets in the provider's own character unit,
    /// where it states one. A citation without one applies to its whole text block; where a passage sits in
    /// its source is the provider's and stays in the stored telemetry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_end: Option<u64>,
    /// The passage of the source the text rests on, where the provider quotes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cited_text: Option<String>,
    /// The provider's own citation, for an `unknown` one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<JsonValue>,
}

/// A citation's members as written, before its invariants are checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CitationShape {
    kind: CitationKind,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    text_start: Option<u64>,
    #[serde(default)]
    text_end: Option<u64>,
    #[serde(default)]
    cited_text: Option<String>,
    #[serde(default)]
    raw: Option<JsonValue>,
}

impl TryFrom<CitationShape> for Citation {
    type Error = &'static str;

    fn try_from(shape: CitationShape) -> Result<Self, Self::Error> {
        let unknown = shape.kind == CitationKind::Unknown;
        if !unknown && shape.source.is_none() {
            return Err("a citation names its source unless its kind is unknown");
        }
        if !unknown && shape.raw.is_some() {
            return Err("only an unknown citation carries the provider's item");
        }
        Ok(Self {
            kind: shape.kind,
            source: shape.source,
            title: shape.title,
            text_start: shape.text_start,
            text_end: shape.text_end,
            cited_text: shape.cited_text,
            raw: shape.raw,
        })
    }
}
