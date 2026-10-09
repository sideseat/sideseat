//! Facts about a span that a read needs and the read path cannot see.

use serde::Deserialize;

use super::SpanWhere;

/// One fact about a span, decided from its attributes when it is ingested and carried to the read as a bit.
///
/// A read holds a span's messages and its extracted columns, not its attributes, so a read-time rule - a message
/// projection - cannot ask what the span carried. Storing the attributes again to answer that would be a second
/// copy of raw content, which is the one thing storage here never keeps. A mark stores the **answer** instead: the
/// condition runs once at ingest and its truth occupies one bit, so the cost does not grow with the payload the
/// question was about.
///
/// Derived, like every extracted column: a re-parse rebuilds it from the raw span, and the bit a span holds says
/// nothing that the asset's condition does not say about that span. The width is bounded
/// ([`MARK_LIMIT`](crate::rules::span_marks::MARK_LIMIT)), and a corpus that declares more marks than fit is
/// refused at startup rather than silently losing the last ones.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SpanMarkRule {
    /// This mark's own name, which a read-time condition names as `mark:<id>`. Unique across every asset: the
    /// marks are one shared, bounded set rather than one set per producer, because they share a stored byte.
    pub id: String,
    /// Why this fact cannot be read where it is needed, which is what justifies storing it. Required: a mark is
    /// storage per span, and the reason it is not a plain condition belongs beside it.
    pub because: String,
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The spans this holds for, asked of the span's own name, attributes and scope at ingest. It may not ask
    /// about a mark: the marks are decided in one pass, in no declared order, so one reading another would
    /// depend on which came first.
    #[serde(rename = "where")]
    pub condition: SpanWhere,
}
