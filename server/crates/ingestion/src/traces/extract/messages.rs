//! Span message extraction.
//!
//! Extracts messages from OTEL events and span attributes for various frameworks.

#![allow(clippy::collapsible_if)]

#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::trace::v1::Span;
use opentelemetry_proto::tonic::trace::v1::span::Event;
#[cfg(test)]
use serde_json::Value as JsonValue;
use serde_json::json;

#[cfg(test)]
use crate::message_events::parse_json_with_fallback;
use crate::message_events::{EventSpan, is_message_event, read_message_event};
use crate::otlp::extract_attributes;
use sideseat_core::utils::time::nanos_to_datetime;
pub use sideseat_domain::observations::{
    MessageSource, RawMessage, RawToolDefinition, RawToolNames, ToolDefinitionSource,
};
use sideseat_ports::types::ObservationType;

#[cfg(test)]
use super::extract_json;
use super::keys;

// ============================================================================
// MESSAGE EXTRACTION FROM EVENTS
// ============================================================================

pub(crate) fn extract_messages_from_events(
    messages: &mut Vec<RawMessage>,
    events: &[Event],
    span: EventSpan<'_>,
) {
    for event in events {
        messages.extend(read_span_event(event, span));
    }
}

/// One span event, read by the shared event reader with its span as context.
fn read_span_event(event: &Event, span: EventSpan<'_>) -> Vec<RawMessage> {
    if !is_message_event(span.rules, &event.name) {
        return vec![];
    }
    read_message_event(
        &event.name,
        &extract_attributes(&event.attributes),
        nanos_to_datetime(event.time_unix_nano),
        span,
    )
}

/// One span event of an unscoped span, as the extractor's tests read it.
#[cfg(test)]
pub(crate) fn extract_message_from_event(
    event: &Event,
    span_name: &str,
    span_attrs: &HashMap<String, String>,
    is_tool_span: bool,
) -> Vec<RawMessage> {
    read_span_event(
        event,
        EventSpan {
            rules: sideseat_domain::rules::ruleset(),
            name: span_name,
            attrs: span_attrs,
            scope: None,
            is_tool_span,
        },
    )
}

// ============================================================================
// MESSAGE EXTRACTION FROM ATTRIBUTES
// ============================================================================

#[derive(Clone, Copy)]
struct SpanExtraction<'a> {
    /// The ruleset this span is read by: the embedded one in production, a test's own where it injects one.
    rules: &'a sideseat_domain::rules::Ruleset,
    name: &'a str,
    attrs: &'a HashMap<String, String>,
    scope_name: Option<&'a str>,
    is_tool_span: bool,
}

impl<'a> SpanExtraction<'a> {
    #[cfg(test)]
    fn unscoped(name: &'a str, attrs: &'a HashMap<String, String>, is_tool_span: bool) -> Self {
        Self {
            rules: sideseat_domain::rules::ruleset(),
            name,
            attrs,
            scope_name: None,
            is_tool_span,
        }
    }

    fn message_context(self) -> sideseat_domain::rules::MessageContext<'a> {
        sideseat_domain::rules::MessageContext::for_scoped_span(
            self.name,
            self.scope_name,
            self.attrs,
            self.is_tool_span,
        )
    }
}

/// Run every **declared** message rule: the carriers an asset says to read, parsed as it says.
///
/// This is the production entry point for message extraction. It names no framework and performs no
/// producer dispatch: all producer keys, shapes, transforms, ownership, and fallback stages are compiled
/// from `server/assets/rules/`.
///
/// The producer-specific readers retained below are test-only equivalence oracles. Production does not
/// register or call them.
#[cfg(test)]
pub(crate) fn try_declared_rules(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    span_name: &str,
    timestamp: DateTime<Utc>,
    claims: &mut std::collections::HashSet<sideseat_domain::rules::message_rules::OwnedCarrier>,
) -> bool {
    try_declared_rules_for_span(
        messages,
        tool_definitions,
        SpanExtraction::unscoped(
            span_name,
            attrs,
            is_tool_execution_span(sideseat_domain::rules::ruleset(), attrs),
        ),
        timestamp,
        claims,
    )
}

fn try_declared_rules_for_span(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    span: SpanExtraction<'_>,
    timestamp: DateTime<Utc>,
    claims: &mut std::collections::HashSet<sideseat_domain::rules::message_rules::OwnedCarrier>,
) -> bool {
    let emissions = span.rules.messages.run(&span.message_context());
    // "Was the message payload handled?" - which is what the caller does with this answer, since it uses it
    // to decide whether the generic reader still needs to run.
    //
    // A `Claim` counts: it exists precisely to say "this carrier is mine and holds nothing worth reading",
    // and its whole effect is to stop the generic reader presenting that payload as a conversation. A
    // `ToolDefinitions` emission does **not**: a span stating a dialect's tool list has said nothing about
    // its conversation, and counting it let an `LLMCall` carrying only `tools` suppress an unrelated
    // `input.value`. The retired extractors counted it, and preserving that preserved the defect.
    // What each retained observation *owns* - the carrier it read. Taken from the emission rather than
    // derived from a message's source, because a `tag_as` rule reports under a name it never read, so
    // reconstructing ownership from the report leaves the key it actually read unclaimed.
    let mut owned: std::collections::HashSet<sideseat_domain::rules::message_rules::OwnedCarrier> =
        std::collections::HashSet::new();
    let found = emissions.iter().any(|emission| {
        matches!(
            emission.target,
            sideseat_domain::rules::schema::EmitTarget::Message
                | sideseat_domain::rules::schema::EmitTarget::Claim
        )
    });
    for emission in emissions {
        let key = emission.carrier.name();
        if matches!(
            emission.target,
            sideseat_domain::rules::schema::EmitTarget::Message
                | sideseat_domain::rules::schema::EmitTarget::Claim
        ) {
            owned.extend(emission.owns.iter().cloned());
        }
        match emission.target {
            // An event carrier is recorded as one: carrier semantics are looked up by kind, so reporting
            // an event as an attribute would change what the pipeline reads it as evidence of.
            sideseat_domain::rules::schema::EmitTarget::Message if emission.carrier.is_event() => {
                messages.push(
                    RawMessage::from_event(key, timestamp, emission.value)
                        .rendered(emission.rendering),
                );
            }
            sideseat_domain::rules::schema::EmitTarget::Message => {
                messages.push(
                    RawMessage::from_attr(key, timestamp, emission.value)
                        .rendered(emission.rendering),
                );
            }
            sideseat_domain::rules::schema::EmitTarget::ToolDefinitions => {
                tool_definitions.push(RawToolDefinition::from_attr(key, timestamp, emission.value));
            }
            // Read on the metadata path, which is where a rule targeting names is evaluated. Reaching here
            // means a *message* rule named this target on one of its readings, which is not a shape any
            // asset declares; the emission is dropped rather than filed as something it is not.
            sideseat_domain::rules::schema::EmitTarget::ToolNames => {}
            // The claim itself is the whole effect: the carrier is this dialect's and holds no message. It
            // is *reported* so the fallback stage can inherit it - a claimed carrier has been read, and the
            // fallback reading it again would present the payload a dialect said holds nothing.
            // The claim itself is the whole effect: the carrier is this dialect's and holds no message.
            // What it *owns* is recorded above, so the fallback stage inherits it.
            sideseat_domain::rules::schema::EmitTarget::Claim => {}
        }
    }
    claims.extend(owned);
    found
}

/// How a span's attributes are shared out among the extractors.
///
/// Production reads every carrier (`PerCarrier`). The other mode is kept as the *baseline* the
/// metamorphic invariant compares against: `reading_more_carriers_only_adds_messages` builds every
/// fixture both ways and requires the richer reading to add messages without moving the ones already
/// visible. Deleting it would delete the only check that an extraction improvement is monotonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionMode {
    /// The narrower reading, kept as a baseline: the first extractor that recognises anything takes the
    /// whole span, including the carriers it does not read. This is what hid the answer on LangChain
    /// `RunnableSequence` spans, where `openinference` won by order and emitted the question from
    /// `llm.input_messages` while the span's own `output.value` held the reply.
    #[cfg_attr(not(test), allow(dead_code))]
    FirstMatch,
    /// Every extractor runs; each observation is kept for the carrier it names unless an earlier
    /// extractor already produced one for that carrier.
    PerCarrier,
}

#[cfg(test)]
pub(crate) fn extract_messages_from_attrs(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    span_name: &str,
    timestamp: DateTime<Utc>,
    mode: ExtractionMode,
    is_tool_span: bool,
) {
    extract_messages_from_context(
        messages,
        tool_definitions,
        SpanExtraction::unscoped(span_name, attrs, is_tool_span),
        timestamp,
        mode,
    );
}

fn extract_messages_from_context(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    span: SpanExtraction<'_>,
    timestamp: DateTime<Utc>,
    mode: ExtractionMode,
) {
    if mode == ExtractionMode::PerCarrier {
        extract_per_carrier(messages, tool_definitions, span, timestamp);
        return;
    }

    let mut claims = std::collections::HashSet::new();
    if try_declared_rules_for_span(messages, tool_definitions, span, timestamp, &mut claims) {
        return;
    }
    // The fallback stage, which this baseline used to lose entirely: a span carrying only the generic pair
    // produced no messages at all under it, which also made the metamorphic oracle's baseline smaller than
    // the thing it is a baseline for.
    if !span.is_tool_span {
        // Nothing was produced, so nothing has been read.
        messages.extend(fallback_messages(
            span,
            timestamp,
            &std::collections::HashSet::new(),
        ));
    }
}

/// Extractors claim *carriers*, not spans.
///
/// Stopping at the first extractor that recognised anything gives one framework's reader the whole
/// span, including the attributes it does not read. On a LangChain `RunnableSequence` span the
/// OpenInference reader claims because `llm.input_messages` is present and reads only that, so the
/// span keeps the question and drops the answer sitting in its own `output.value` - which the LangGraph
/// reader would have read. Five of seven langgraph fixtures show exactly that.
fn extract_per_carrier(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    span: SpanExtraction<'_>,
    timestamp: DateTime<Utc>,
) {
    let mut claimed: HashSet<String> = messages.iter().map(|m| carrier_of(&m.source)).collect();
    // What the dialect stage owns, typed and taken from the emissions themselves - which is what the
    // fallback stage inherits when answer recovery reaches it.
    let mut owned_by_dialects: std::collections::HashSet<
        sideseat_domain::rules::message_rules::OwnedCarrier,
    > = std::collections::HashSet::new();
    let mut any_specific = false;
    let observation_type =
        super::attributes::detect_observation_type(span.rules, span.name, span.attrs);

    // One evaluator, called directly: the table of framework extractors it replaced is gone, and with one
    // entry left the indirection only hid which code runs.
    {
        let mut produced = Vec::new();
        let mut claims = std::collections::HashSet::new();
        if try_declared_rules_for_span(
            &mut produced,
            tool_definitions,
            span,
            timestamp,
            &mut claims,
        ) {
            any_specific = true;
        }
        // A claimed carrier has been read even though it produced nothing, so the fallback stage must not
        // read it again - that is what a claim means.
        owned_by_dialects.extend(claims);

        // Carriers are recorded after the whole batch, not per message: one extractor legitimately
        // emits several observations for one carrier - an expanded message array is the usual case -
        // and claiming as it goes would keep only the first.
        let mut newly_claimed = Vec::new();
        for message in produced {
            let carrier = carrier_of(&message.source);
            if claimed.contains(&carrier) {
                continue;
            }
            newly_claimed.push(carrier);
            messages.push(message);
        }
        claimed.extend(newly_claimed);
    }

    if span.is_tool_span {
        return;
    }

    if !any_specific {
        messages.extend(fallback_messages(
            span,
            timestamp,
            &std::collections::HashSet::new(),
        ));
        return;
    }

    // A dialect read this span, but a dialect reads the carriers it knows. If it left the span's
    // *answer* unaccounted for, the generic pair is where the answer is - and on a **generation** span
    // that is what `output.value` means.
    //
    // The observation type is what makes this safe, and it is the whole point: `output.value` means "the
    // answer" on a generation span and "node state" on a chain span. Gated on the carrier's *name* alone
    // this admitted LangGraph's `Prompt` and `should_continue` node state as answers and took that suite
    // from 12 messages to 28. The type is a pure function of the span name and attributes, already
    // computed for every span at `extract/mod.rs`, so asking it here costs nothing.
    if observation_type != ObservationType::Generation {
        return;
    }
    if messages
        .iter()
        .any(|m| carrier_holds_span_output(&m.source, span.name, observation_type))
    {
        return;
    }

    // What a dialect already read. Passed in, because this call happens *after* the dialect stage produced
    // something - the one case where the two stages meet, and where independent claim sets let one carrier be
    // read twice.
    let produced = fallback_messages(span, timestamp, &owned_by_dialects);
    for message in produced {
        if !carrier_holds_span_output(&message.source, span.name, observation_type) {
            continue;
        }
        let carrier = carrier_of(&message.source);
        if claimed.insert(carrier) {
            messages.push(message);
        }
    }
}

/// Whether a carrier is one the span's own output is reported through, per the declared rules.
///
/// Asked *with* the span context this path already has. Reading the carrier name alone here meant a
/// clause qualified by observation type or span name could never apply at ingestion, so the same carrier
/// would be read one way when a span was written and another when it was read - the split answer this
/// engine exists to remove. Scope is not available on this path, and a clause that asks about it
/// therefore cannot match here, which is the conservative direction: it falls back to the generic clause
/// rather than guessing.
#[doc(hidden)]
pub fn carrier_holds_span_output(
    source: &MessageSource,
    span_name: &str,
    observation_type: ObservationType,
) -> bool {
    let (event, attribute) = match source {
        MessageSource::Event { name, .. } => (Some(name.as_str()), None),
        MessageSource::Attribute { key, .. } => (None, Some(key.as_str())),
    };
    sideseat_domain::sideml::carrier::semantics_for_context(
        &sideseat_domain::rules::CarrierContext {
            event,
            attribute,
            observation_type: Some(observation_type.as_str()),
            span_name: Some(span_name),
        },
    )
    .carrier_holds_span_output
}

/// The attribute or event an observation was read from - what an extractor claims.
fn carrier_of(source: &MessageSource) -> String {
    match source {
        MessageSource::Event { name, .. } => format!("event:{name}"),
        MessageSource::Attribute { key, .. } => format!("attr:{key}"),
    }
}

/// Extract tool definitions from any span (runs even on tool spans)
///
/// This is separate from `try_otel_genai_messages` because tool definitions
/// are metadata that should be extracted from tool execution spans too,
/// not just chat spans.
#[cfg(test)]
pub(crate) fn extract_tool_definitions(
    span_name: &str,
    attrs: &HashMap<String, String>,
    timestamp: DateTime<Utc>,
) -> (Vec<RawToolDefinition>, Vec<RawToolNames>) {
    extract_tool_definitions_for_span(
        SpanExtraction::unscoped(
            span_name,
            attrs,
            is_tool_execution_span(sideseat_domain::rules::ruleset(), attrs),
        ),
        timestamp,
    )
}

fn extract_tool_definitions_for_span(
    span: SpanExtraction<'_>,
    timestamp: DateTime<Utc>,
) -> (Vec<RawToolDefinition>, Vec<RawToolNames>) {
    let mut tool_definitions = Vec::new();
    let mut tool_names = Vec::new();

    // Declared `repr` grammars. Every span, like the rest of this function: a tool definition is not a
    // message, so carrier claiming does not apply - a framework may state its tools on a carrier another
    // rule reads as a conversation, and both statements are true.
    for emission in span
        .rules
        .messages
        .tool_definitions(&span.message_context())
    {
        let key = emission.carrier.name();
        match emission.target {
            // A list of names is not a list of definitions, and filing one as the other reports tools whose
            // parameters are absent rather than unstated.
            sideseat_domain::rules::schema::EmitTarget::ToolNames => {
                tool_names.push(RawToolNames::from_attr(key, timestamp, emission.value));
            }
            _ => {
                tool_definitions.push(RawToolDefinition::from_attr(key, timestamp, emission.value));
            }
        }
    }

    (tool_definitions, tool_names)
}

/// Whether this span *is a tool running*, so its messages are the tool's input and result rather than a
/// model's turn.
///
/// The evidence is declared per dialect (`span_facts`, `SpanFact::ToolExecution`) and the union answers,
/// because one question has several conventions answering it - an operation name, a span-kind attribute, a
/// pair of attributes that appear together only on a call being run - and which dialect supplied the
/// answer is not something a reader of it should have to know.
pub(crate) fn is_tool_execution_span(
    rules: &sideseat_domain::rules::Ruleset,
    attrs: &HashMap<String, String>,
) -> bool {
    rules.span_facts.holds(
        sideseat_domain::rules::schema::SpanFact::ToolExecution,
        attrs,
    )
}

#[cfg(test)]
mod oracle_agents;
#[cfg(test)]
mod oracle_dialects;
#[cfg(test)]
mod oracle_events;
#[cfg(test)]
mod oracle_helpers;

#[cfg(test)]
use oracle_agents::*;
#[cfg(test)]
use oracle_dialects::*;
#[cfg(test)]
use oracle_events::*;
#[cfg(test)]
use oracle_helpers::*;

/// Wrap a plain data object (structured output) with message structure.
/// If the value is a plain data object without message-structure keys,
/// wraps it as `{"role": <role>, "content": <value>}` so normalize() can process it.
/// Non-plain-data values (already message-shaped, arrays, strings) pass through unchanged.
/// The fallback stage's messages: the declared last-resort carriers, read for this span.
fn fallback_messages(
    span: SpanExtraction<'_>,
    timestamp: DateTime<Utc>,
    already_read: &std::collections::HashSet<sideseat_domain::rules::message_rules::OwnedCarrier>,
) -> Vec<RawMessage> {
    span.rules
        .messages
        .fallback(&span.message_context(), already_read)
        .into_iter()
        .filter(|emission| {
            matches!(
                emission.target,
                sideseat_domain::rules::schema::EmitTarget::Message
            )
        })
        .map(|emission| {
            RawMessage::from_attr(emission.carrier.name(), timestamp, emission.value)
                .rendered(emission.rendering)
        })
        .collect()
}

// ============================================================================
// SPAN MESSAGE EXTRACTION ORCHESTRATION
// ============================================================================

/// Extract messages and tool definitions for a single span.
#[cfg(test)]
pub(super) fn extract_messages_for_span(
    otlp_span: &Span,
    span_attrs: &HashMap<String, String>,
    timestamp: DateTime<Utc>,
    mode: ExtractionMode,
) -> (Vec<RawMessage>, Vec<RawToolDefinition>, Vec<RawToolNames>) {
    extract_messages_for_scoped_span(
        sideseat_domain::rules::ruleset(),
        otlp_span,
        span_attrs,
        None,
        timestamp,
        mode,
    )
}

pub(super) fn extract_messages_for_scoped_span(
    rules: &sideseat_domain::rules::Ruleset,
    otlp_span: &Span,
    span_attrs: &HashMap<String, String>,
    scope_name: Option<&str>,
    timestamp: DateTime<Utc>,
    mode: ExtractionMode,
) -> (Vec<RawMessage>, Vec<RawToolDefinition>, Vec<RawToolNames>) {
    let is_tool_span = is_tool_execution_span(rules, span_attrs);
    let span = SpanExtraction {
        rules,
        name: &otlp_span.name,
        attrs: span_attrs,
        scope_name,
        is_tool_span,
    };

    let mut raw_messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let mut tool_names = Vec::new();

    // Always extract system_prompt if present (comes before conversation)
    extract_messages_from_events(
        &mut raw_messages,
        &otlp_span.events,
        EventSpan {
            rules,
            name: &otlp_span.name,
            attrs: span_attrs,
            scope: scope_name,
            is_tool_span,
        },
    );

    // Enrich tool span messages with metadata from span attributes
    // Check event name (not role) since role is now derived at query-time
    if is_tool_span {
        let tool_name = span_attrs.get(keys::GEN_AI_TOOL_NAME);
        let tool_call_id = span_attrs.get(keys::GEN_AI_TOOL_CALL_ID);

        for msg in &mut raw_messages {
            // Get event name from message source
            let event_name = match &msg.source {
                MessageSource::Event { name, .. } => Some(name.as_str()),
                MessageSource::Attribute { .. } => None,
            };

            match event_name {
                // Tool input events (gen_ai.tool.message in tool span)
                // Raw role preserved; semantic role derived at query-time in SideML
                Some(keys::EVENT_TOOL_MESSAGE) => {
                    // Set role only if not already set (preserve raw data)
                    if msg.content.get("role").is_none() {
                        msg.content["role"] = json!("tool_call");
                    }
                    // Map "id" to "tool_call_id"
                    if msg.content.get("tool_call_id").is_none() {
                        let id = msg
                            .content
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                            .or_else(|| tool_call_id.cloned());
                        if let Some(id) = id {
                            msg.content["tool_call_id"] = json!(id);
                        }
                    }
                    // Add tool name
                    if msg.content.get("name").is_none() {
                        if let Some(name) = tool_name {
                            msg.content["name"] = json!(name);
                        }
                    }
                }
                // Tool output events (gen_ai.choice in tool span)
                // Role is derived at query-time as "tool" by role_from_event_name_with_context
                Some(keys::EVENT_CHOICE) | Some(keys::EVENT_CONTENT_COMPLETION)
                    // Add tool_call_id for correlation with tool call
                    // (extract_tool_use_id in sideml/tools.rs looks for tool_call_id)
                    if msg.content.get("tool_call_id").is_none() =>
                {
                    let id = msg
                        .content
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .or_else(|| tool_call_id.cloned());
                    if let Some(id) = id {
                        msg.content["tool_call_id"] = json!(id);
                    }
                }
                _ => {}
            }
        }
    }

    // Vercel's tool-call attributes are declared in `server/assets/rules/producers/vercel-ai.json`, with tool-span
    // permission, because that is the only kind of span they appear on - those spans carry no events, only
    // attributes.
    //
    // The block that used to live here was gated on `raw_messages.is_empty()`, so any recognised event
    // suppressed the call and the result entirely. That is a loss rather than a precedence, and it was
    // invisible: the equivalence oracle applies the caller's tool-span exclusion, so it compared both
    // implementations *after* the suppression.

    // Always extract tool definitions and tool names from any span (they're metadata, not conversation)
    let (defs, names) = extract_tool_definitions_for_span(span, timestamp);
    tool_definitions.extend(defs);
    tool_names.extend(names);

    // Attributes are read whatever the events said, **and on tool spans too** - but on a tool span only the
    // conventions' own tool attributes are read; see `SEMCONV_RULE`.
    //
    // Attribute extraction used to be skipped wholesale for a tool span, because its `input.value` and
    // `output.value` hold tool parameters and results rather than conversation. That is true of those
    // carriers and false of `gen_ai.tool.call.arguments` / `.result`, which are how the current conventions
    // report a tool call and appear on exactly this kind of span - so a producer following them had its tool
    // calls silently dropped.
    //
    // One recognised event used to suppress *every* attribute carrier on the span, on the grounds that
    // "events contain the authoritative conversation data". That is true of the carrier an event covers and
    // false of the rest: a span whose question arrives as `gen_ai.user.message` and whose answer sits only in
    // `output.value` returned the question alone - the recorded LangChain failure, across the event/attribute
    // boundary instead of between two attribute carriers. Nothing detected it because no captured fixture
    // has that shape; `_synthetic/event_question_attribute_answer` now does, and the answer invariant fires
    // on it with the gate restored.
    //
    // Reading both is safe *because* claiming is per carrier: an event's blocks are already claimed, so the
    // attribute pass contributes only what no event covered, and content-based dedup collapses a genuine
    // overlap. Measured across all 111 fixtures and four views: removing the gate changed nothing.

    // Debug: Log extraction decision
    extract_messages_from_context(
        &mut raw_messages,
        &mut tool_definitions,
        span,
        timestamp,
        mode,
    );

    // Debug: Log final message count
    // Debug: Log AutoGen extraction results
    (raw_messages, tool_definitions, tool_names)
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
