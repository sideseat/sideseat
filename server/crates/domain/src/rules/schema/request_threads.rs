//! The conversation threads of a producer that exports each request as what it added.

use serde::Deserialize;

use super::SpanWhere;

/// The requests of one conversation, for a producer whose request spans carry what is **new** since the previous
/// request of the same conversation rather than the request it sent.
///
/// A stateless model API is sent the whole history every time; such a producer's telemetry holds only the delta,
/// so a span view can show what the call was sent only by composing the thread's earlier requests. This declares
/// which spans are requests and what identifies their thread; which carriers hold the delta is a carrier fact,
/// `carrier_holds_request_delta`.
///
/// The key is derived when the span is ingested, beside its messages - a cache a re-parse rebuilds, since the read
/// path sees a span's messages and not its attributes.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RequestThreadRule {
    /// This rule's own name. Part of every key it derives, so two producers' threads never share one.
    pub id: String,
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The spans that are requests of a thread. At most one rule may hold for a span, which compilation proves.
    #[serde(rename = "where")]
    pub condition: SpanWhere,
    /// The span attributes whose values identify a thread, each as an `attr:<key>` source, in a fixed order. Two
    /// requests share a thread when every source agrees, absent included - so a subagent with its own id is a
    /// thread of its own. Never the trace: a thread continues across the interactions and traces of a session.
    pub key: Vec<String>,
}
