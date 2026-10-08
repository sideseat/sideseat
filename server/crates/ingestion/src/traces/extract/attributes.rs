//! Span attribute extraction.
//!
//! Extracts GenAI attributes, semantic conventions, and classifies spans.

#![allow(clippy::collapsible_if)]

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::trace::v1::Span;
use serde_json::{Value as JsonValue, json};

use sideseat_core::constants;
use sideseat_ports::types::{ObservationType, SpanCategory};
// Only the equivalence oracle names the enum now: detection produces a label from the assets.
#[cfg(test)]
use crate::traces::extract::framework_oracle::Framework;
#[cfg(test)]
use sideseat_core::utils::string::parse_string_array;
use sideseat_core::utils::time::nanos_to_datetime;
use sideseat_domain::pricing;

use super::truncate_bytes;

/// Only the oracles parse a JSON attribute here now; production reads through the declared resolvers.
#[cfg(test)]
use super::extract_json;
use super::keys;

mod classification;
mod framework;
#[cfg(test)]
mod semantic_oracle;
mod usage;

#[cfg(test)]
use classification::extract_autogen_tokens;
pub use classification::{categorize_span, detect_observation_type};
#[cfg(any(test, feature = "test-support"))]
pub use classification::{categorize_span_legacy, detect_observation_type_legacy};
pub(crate) use framework::detect_framework_scoped;
#[cfg(test)]
pub(crate) use framework::{detect_framework, legacy_detect_framework};
#[cfg(test)]
pub(super) use semantic_oracle::extract_semantic_legacy;
#[cfg(test)]
use usage::INPUT_TOKENS;
use usage::counters_already_read;
#[cfg(test)]
pub(super) use usage::token_readings_legacy;

// ============================================================================
// SHARED HELPER FUNCTIONS
// ============================================================================

/// Check if haystack contains needle (case-insensitive, ASCII only).
/// Zero-allocation alternative to `haystack.to_lowercase().contains(needle)`.
#[inline]
/// Retired with the sweeps that used it: the declared form asks the same question through a case-insensitive
/// phrase search over a named attribute.
#[cfg(any(test, feature = "test-support"))]
fn contains_ascii_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Merge tags from multiple attribute keys, deduplicating.
///
/// Only the oracle's now: the declared `merge_all` combine does this, and a producer's tag key is stated in
/// the asset rather than in a list here.
#[cfg(test)]
pub(super) fn merge_tags(attrs: &HashMap<String, String>, tag_keys: &[&str]) -> Vec<String> {
    let mut tags = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for key in tag_keys {
        if let Some(val) = attrs.get(*key) {
            for tag in parse_string_array(val) {
                if seen.insert(tag.clone()) {
                    tags.push(tag);
                }
            }
        }
    }
    tags
}

/// Get first matching value from attribute keys.
///
/// An **empty** value is not a value, so the chain keeps looking. This is what a fallback chain is for: a
/// framework that sets `session.id=""` alongside `gen_ai.conversation.id="conv-1"` would otherwise have the
/// empty string win, and retrieval treats a stored empty session id as *no session* - so the conversation got
/// no session view, a trace read could not load its siblings, and the project feed could not widen its context,
/// which lets replayed history through as duplicates. Present-but-empty is exactly the shape a chain exists to
/// step over, and no caller wants an empty string in preference to a real one.
#[cfg(test)]
pub(super) fn get_first(attrs: &HashMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|k| attrs.get(*k))
        .find(|v| !v.is_empty())
        .cloned()
}

/// Parse a value from attributes.
#[cfg(test)]
pub(super) fn parse_opt<T: std::str::FromStr>(
    attrs: &HashMap<String, String>,
    key: &str,
) -> Option<T> {
    attrs.get(key).and_then(|v| v.parse().ok())
}

// ============================================================================
// OTLP CORE FIELD EXTRACTION
// ============================================================================

pub(super) fn set_core_fields(s: &mut SpanData, span: &Span) {
    s.trace_id = hex::encode(&span.trace_id);
    s.span_id = hex::encode(&span.span_id);
    s.parent_span_id = if span.parent_span_id.is_empty() {
        None
    } else {
        Some(hex::encode(&span.parent_span_id))
    };
    s.trace_state = if span.trace_state.is_empty() {
        None
    } else {
        Some(span.trace_state.clone())
    };
    s.span_name = span.name.clone();
    s.span_kind = Some(span_kind_to_string(span.kind).to_string());
    s.status_code = span
        .status
        .as_ref()
        .map(|st| status_code_to_string(st.code).to_string());
    s.status_message = span.status.as_ref().and_then(|st| {
        if st.message.is_empty() {
            None
        } else if st.message.len() > constants::ERROR_MESSAGE_MAX_LEN {
            Some(format!(
                "{}...",
                truncate_bytes(&st.message, constants::ERROR_MESSAGE_MAX_LEN)
            ))
        } else {
            Some(st.message.clone())
        }
    });
    s.timestamp_start = nanos_to_datetime(span.start_time_unix_nano);
    s.timestamp_end = if span.end_time_unix_nano > 0 {
        Some(nanos_to_datetime(span.end_time_unix_nano))
    } else {
        None
    };
    s.duration_ms = if span.end_time_unix_nano > span.start_time_unix_nano {
        ((span.end_time_unix_nano - span.start_time_unix_nano) / 1_000_000) as i64
    } else {
        0
    };
}

fn span_kind_to_string(kind: i32) -> &'static str {
    match kind {
        0 => "UNSPECIFIED",
        1 => "INTERNAL",
        2 => "SERVER",
        3 => "CLIENT",
        4 => "PRODUCER",
        5 => "CONSUMER",
        _ => "UNKNOWN",
    }
}

fn status_code_to_string(code: i32) -> &'static str {
    match code {
        0 => "UNSET",
        1 => "OK",
        2 => "ERROR",
        _ => "UNKNOWN",
    }
}

// ============================================================================
// SPAN NAME RESOLUTION
// ============================================================================

/// Resolve the display span name from attributes.
///
/// Some instrumentation libraries store a template or internal identifier as the
/// OTLP span name and put the human-readable resolved name in an attribute.
/// This function checks for those patterns and overrides `span_name` when a
/// better display name is available.
///
/// Currently handles:
/// - **Logfire** (`logfire.msg_template` + `logfire.msg`): Span name is a Python
///   f-string template like `"Chat Completion with {request_data[model]!r}"`.
///   When `logfire.msg_template` exists, the resolved `logfire.msg` is used.
///
/// Retired: the display name is the `display_span_name` field target now, declared in
/// `rules/vocabulary/span-fields-display.json`. Kept as the equivalence oracle.
#[cfg(test)]
pub(super) fn resolve_span_name(span: &mut SpanData, attrs: &HashMap<String, String>) {
    // Logfire: span name is the unresolved msg_template; logfire.msg is the resolved version
    if attrs.contains_key(keys::LOGFIRE_MSG_TEMPLATE) {
        if let Some(resolved) = attrs.get(keys::LOGFIRE_MSG) {
            if !resolved.is_empty() {
                span.span_name = resolved.clone();
            }
        }
    }
}

// ============================================================================
// SPAN DATA
// ============================================================================

/// Extracted span data for pipeline processing.
#[derive(Debug, Clone, Default)]
pub struct SpanData {
    // Identity
    pub project_id: Option<String>,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub trace_state: Option<String>,

    // Session/User
    pub session_id: Option<String>,
    pub user_id: Option<String>,

    // Classification
    pub span_name: String,
    pub span_kind: Option<String>,
    pub span_category: Option<SpanCategory>,
    pub observation_type: Option<ObservationType>,
    /// The producer label detection resolved, for provenance, display and filtering.
    ///
    /// A `String` rather than an enum on purpose: an enum is a list of frameworks in Rust, and adding
    /// one would then be a code change rather than an asset. Nothing reads this to decide behaviour.
    pub framework: Option<String>,
    /// The conversation thread this span is a request of, where a rule names one; empty on every other span.
    ///
    /// Derived here, from the span's attributes, because the read path holds a span's messages and not its
    /// attributes. A cache a re-parse rebuilds identically, like every other extracted column.
    pub request_thread: String,
    /// The instrumentation scope that produced this span - `ScopeSpans.scope.name`/`.version`.
    ///
    /// The one fact about a span nothing else derives: the resource names the *process*, the span
    /// attributes name the call, and only the scope names the **library** that emitted the telemetry,
    /// versioned. It was never captured for spans (the metrics path always had it), which meant no
    /// rule could ever be keyed on a producer's identity-and-version - the design record's stage 3
    /// calls that the fact that makes an undeclared producer decidable.
    pub scope_name: Option<String>,
    pub scope_version: Option<String>,
    pub status_code: Option<String>,
    pub status_message: Option<String>,
    pub exception_type: Option<String>,
    pub exception_message: Option<String>,
    pub exception_stacktrace: Option<String>,

    // Time
    pub timestamp_start: DateTime<Utc>,
    pub timestamp_end: Option<DateTime<Utc>>,
    pub duration_ms: i64,

    // Environment
    pub environment: Option<String>,

    // GenAI Core
    pub gen_ai_system: Option<String>,
    pub gen_ai_operation_name: Option<String>,
    pub gen_ai_request_model: Option<String>,
    pub gen_ai_response_model: Option<String>,
    pub gen_ai_response_id: Option<String>,

    // GenAI Parameters
    pub gen_ai_temperature: Option<f64>,
    pub gen_ai_top_p: Option<f64>,
    pub gen_ai_top_k: Option<i64>,
    pub gen_ai_max_tokens: Option<i64>,
    pub gen_ai_frequency_penalty: Option<f64>,
    pub gen_ai_presence_penalty: Option<f64>,
    pub gen_ai_stop_sequences: Vec<String>,
    pub gen_ai_finish_reasons: Vec<String>,

    // GenAI Agent/Tool
    pub gen_ai_agent_id: Option<String>,
    pub gen_ai_agent_name: Option<String>,
    pub gen_ai_tool_name: Option<String>,
    pub gen_ai_tool_call_id: Option<String>,

    // GenAI Performance
    pub gen_ai_server_ttft_ms: Option<i64>,
    pub gen_ai_server_request_duration_ms: Option<i64>,

    // Token Usage
    pub gen_ai_usage_input_tokens: i64,
    pub gen_ai_usage_output_tokens: i64,
    pub gen_ai_usage_total_tokens: i64,
    /// The total the provider *stated*, or 0 when it stated none. Not persisted.
    ///
    /// Kept apart from the synthesised total above so enrichment can redo the synthesis once pricing has
    /// resolved which provider's convention applies. Folding the two together meant the synthesised value
    /// became a floor that could not be lowered: `system=anthropic, model=gpt-4o` synthesised
    /// `input + output + cache` here, pricing then resolved OpenAI (cache already inside the input), and
    /// `max(synthesised, corrected)` kept the too-large number.
    pub gen_ai_usage_total_tokens_reported: i64,
    pub gen_ai_usage_cache_read_tokens: i64,
    pub gen_ai_usage_cache_write_tokens: i64,
    pub gen_ai_usage_reasoning_tokens: i64,
    pub gen_ai_usage_details: JsonValue,

    // Pre-calculated costs (from OpenInference llm.cost.* or other sources)
    // These are used as fallback when pricing service cannot calculate costs
    pub extracted_cost_total: Option<f64>,
    pub extracted_cost_input: Option<f64>,
    pub extracted_cost_output: Option<f64>,

    // External Services
    pub http_method: Option<String>,
    pub http_url: Option<String>,
    pub http_status_code: Option<i64>,
    pub db_system: Option<String>,
    pub db_name: Option<String>,
    pub db_operation: Option<String>,
    pub db_statement: Option<String>,
    pub storage_system: Option<String>,
    pub storage_bucket: Option<String>,
    pub storage_object: Option<String>,
    pub messaging_system: Option<String>,
    pub messaging_destination: Option<String>,

    // Tags/Metadata
    pub tags: Vec<String>,
    pub metadata: JsonValue,
}

// ============================================================================
// ATTRIBUTE EXTRACTION
// ============================================================================

/// The stored fields whose only framework-specific part is *which key* carries them.
///
/// Every chain this replaced was an ordered `&[&str]` of provider spellings - framework knowledge in the
/// code, where adding a producer meant editing a list. The order is declared in
/// `rules/vocabulary/span-fields-semantic.json`; the retired chains stay below as the equivalence oracle.
/// What the declared resolvers found for each token counter.
///
/// `Option`, not the stored `i64`, because presence is the fact the fallbacks downstream need and the column
/// cannot hold it: a counter nothing carried and a genuine `0` are different statements about a call, and no
/// arithmetic recovers the difference once it is gone. Returned rather than written, for the same reason.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TokenReadings {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub total_reported: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
    pub reasoning: Option<i64>,
    /// What one dialect's embedded object states, resolved whatever the chains above answered.
    ///
    /// Its embedded *total* describes the embedded parts, so it is usable only when the parts actually stored
    /// are those parts - and that test needs the candidate values even where a flat attribute won, which
    /// ordinary resolution would have hidden.
    pub candidate_input: Option<i64>,
    pub candidate_output: Option<i64>,
    pub candidate_cache_read: Option<i64>,
    pub candidate_total: Option<i64>,
    /// Usage a dialect records per message, summed - see `UsageSummedInput`.
    pub summed_input: Option<i64>,
    pub summed_output: Option<i64>,
}

pub(crate) fn apply_span_fields(
    span: &mut SpanData,
    span_name: &str,
    attrs: &HashMap<String, String>,
    events: &[sideseat_domain::rules::span_fields::SpanEvent],
) -> TokenReadings {
    // Its own step, not a subroutine of either legacy function. Resolution is over *every* declared rule, so
    // calling it from two entry points wrote the same answers twice and made "which entry point owns a target"
    // a question with no answer - while calling it from one left the other silently not doing what its name
    // says. The plan is asked once per span, here.
    //
    // The **real** span name, because a source may read it and a gate may ask about it. Passed as `""` this
    // was the same defect the message path had: such a declaration compiles and can never hold.
    let mut tokens = TokenReadings::default();
    for resolved in sideseat_domain::rules::ruleset()
        .span_fields
        .resolve(span_name, attrs, events)
    {
        // **The refusals are reported.** Resolution records every source that was present and unreadable, and
        // this loop applied only the answer and dropped them - so `gen_ai.usage.input_tokens = "many"` produced
        // exactly the same stored span as the attribute being absent, and a column that is empty because three
        // producers wrote it wrongly was indistinguishable from one nobody wrote. That is the same class of
        // defect as a 200 that precedes a drop: the system knew and said nothing.
        //
        // A log rather than a rejected span, deliberately: one unreadable field is not a reason to refuse
        // telemetry, and the answer is already the honest one (absent, not a guess). What was missing is that
        // anyone could find out. Named by the source's `ClausePath`, so the message says which producer's
        // spelling was believed and which was refused rather than only which column is empty.
        for refusal in &resolved.refused {
            tracing::debug!(
                target: "sideseat::rules",
                clause = %refusal.clause,
                carrier = %refusal.carrier,
                span = %span_name,
                field = ?resolved.target,
                cause = %refusal.cause,
                "a span field source was present and could not be read"
            );
        }
        apply_field(span, &resolved, &mut tokens);
    }
    tokens
}

/// Write one resolved field onto the span.
///
/// The one place that knows the shape of **our own** DTO, which is not framework knowledge: a target names a
/// column, and the resolver has already decided what a source had to produce to fill it.
#[cfg(test)]
pub(super) fn apply_field_for_test(
    span: &mut SpanData,
    resolved: &sideseat_domain::rules::span_fields::Resolved,
    tokens: &mut TokenReadings,
) {
    apply_field(span, resolved, tokens);
}

fn apply_field(
    span: &mut SpanData,
    resolved: &sideseat_domain::rules::span_fields::Resolved,
    tokens: &mut TokenReadings,
) {
    use sideseat_domain::rules::schema::FieldTarget as T;
    use sideseat_domain::rules::span_fields::Reading;

    let text = || match &resolved.reading {
        Reading::Text(value) => Some(value.clone()),
        _ => None,
    };
    let integer = || match &resolved.reading {
        Reading::Integer(value) => Some(*value),
        _ => None,
    };
    let float = || match &resolved.reading {
        Reading::Float(value) => Some(*value),
        _ => None,
    };
    let list = || match &resolved.reading {
        Reading::StringList(items) => items.clone(),
        _ => Vec::new(),
    };
    match resolved.target {
        // Presentation only. The raw name stays in `set_core_fields`' hands and is what every behavioural
        // check is given; a producer's unresolved template is a poor thing to show a reader and a fine thing
        // to key on.
        T::DisplaySpanName => {
            if let Some(name) = text() {
                span.span_name = name;
            }
        }
        // The counters go to the caller, not to a column: see `TokenReadings`.
        T::UsageInputTokens => tokens.input = integer(),
        T::UsageOutputTokens => tokens.output = integer(),
        T::UsageTotalTokensReported => tokens.total_reported = integer(),
        T::UsageCacheReadTokens => tokens.cache_read = integer(),
        T::UsageCacheWriteTokens => tokens.cache_write = integer(),
        T::UsageReasoningTokens => tokens.reasoning = integer(),
        T::UsageCandidateInput => tokens.candidate_input = integer(),
        T::UsageCandidateOutput => tokens.candidate_output = integer(),
        T::UsageCandidateCacheRead => tokens.candidate_cache_read = integer(),
        T::UsageCandidateTotal => tokens.candidate_total = integer(),
        T::UsageSummedInput => tokens.summed_input = integer(),
        T::UsageSummedOutput => tokens.summed_output = integer(),
        T::ReportedCostTotal => span.extracted_cost_total = float(),
        T::ReportedCostInput => span.extracted_cost_input = float(),
        T::ReportedCostOutput => span.extracted_cost_output = float(),
        T::SessionId => span.session_id = text(),
        T::Metadata => {
            span.metadata = text()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(JsonValue::Null)
        }
        T::UserId => span.user_id = text(),
        T::HttpMethod => span.http_method = text(),
        T::HttpUrl => span.http_url = text(),
        T::HttpStatusCode => span.http_status_code = integer(),
        T::GenAiSystem => span.gen_ai_system = text(),
        T::GenAiOperationName => span.gen_ai_operation_name = text(),
        T::GenAiRequestModel => span.gen_ai_request_model = text(),
        T::GenAiResponseModel => span.gen_ai_response_model = text(),
        T::GenAiResponseId => span.gen_ai_response_id = text(),
        T::GenAiTemperature => span.gen_ai_temperature = float(),
        T::GenAiTopP => span.gen_ai_top_p = float(),
        T::GenAiTopK => span.gen_ai_top_k = integer(),
        T::GenAiMaxTokens => span.gen_ai_max_tokens = integer(),
        T::GenAiFrequencyPenalty => span.gen_ai_frequency_penalty = float(),
        T::GenAiPresencePenalty => span.gen_ai_presence_penalty = float(),
        T::GenAiStopSequences => span.gen_ai_stop_sequences = list(),
        T::GenAiFinishReasons => span.gen_ai_finish_reasons = list(),
        T::GenAiAgentId => span.gen_ai_agent_id = text(),
        T::GenAiAgentName => span.gen_ai_agent_name = text(),
        T::GenAiToolName => span.gen_ai_tool_name = text(),
        T::GenAiToolCallId => span.gen_ai_tool_call_id = text(),
        T::GenAiServerTtftMs => span.gen_ai_server_ttft_ms = integer(),
        T::GenAiServerRequestDurationMs => span.gen_ai_server_request_duration_ms = integer(),
        T::DbSystem => span.db_system = text(),
        T::DbName => span.db_name = text(),
        T::DbOperation => span.db_operation = text(),
        T::DbStatement => span.db_statement = text(),
        T::StorageSystem => span.storage_system = text(),
        T::StorageBucket => span.storage_bucket = text(),
        T::StorageObject => span.storage_object = text(),
        T::MessagingSystem => span.messaging_system = text(),
        T::MessagingDestination => span.messaging_destination = text(),
        T::Tags => span.tags = list(),
    }
}

/// The GenAI fields of a span: everything but the token accounting.
///
/// The field half is declared in `rules/vocabulary/span-fields-genai.json`; the retired chains are kept below as the
/// equivalence oracle. Token arithmetic stays here, because a synthesised total and every pricing decision are
/// statements about our own accounting rather than about a producer's spelling.
/// The token accounting of a span: what the declared field resolvers deliberately do not do.
///
/// Arithmetic, a synthesised total and every pricing-dependent decision are statements about our own
/// accounting rather than about a producer's spelling, which is why they are code and not data.
pub(crate) fn extract_genai(
    span: &mut SpanData,
    attrs: &HashMap<String, String>,
    span_name: &str,
    tokens: &TokenReadings,
) {
    // Where each counter was written is declared (`rules/vocabulary/span-fields-usage.json`); the resolver hands them over
    // as `Option`, so presence - which is what every framework fallback below tests, since a genuine `0` is a
    // reported count and must not be replaced - survives the boundary.
    let flat_input = tokens.input;
    let flat_output = tokens.output;
    span.gen_ai_usage_input_tokens = flat_input.unwrap_or(0);
    span.gen_ai_usage_output_tokens = flat_output.unwrap_or(0);
    // Whether each counter has been *supplied* - by the flat attributes or by a fallback that already ran.
    // Every fallback below reads and updates these rather than testing the stored value, for two reasons: a
    // reported `0` is a supplied count and must not be replaced, and the fallbacks run in sequence, so
    // testing the value let a later JSON source overwrite what an earlier one had legitimately provided.
    let mut input_supplied = flat_input.is_some();
    let mut output_supplied = flat_output.is_some();

    // The three embedded usage objects those flags used to be threaded through - one dialect's own usage blob,
    // the usage nested in a serialised response, and a third dialect's response object - are declared sources
    // on the same targets now, in the same order. An ordered chain already means "only for a counter nothing
    // before supplied", so the sequential flag updates had nothing left to say.

    // Read after the last source, which is also what keeps every source maintaining the tracker: a chain
    // whose final link need not update it is a chain the next link will not either. A generation span with no
    // counter at all is worth seeing - it bills as free, and the usual cause is an instrumentation version
    // that moved its attribute names.
    // Held rather than applied: the total is synthesised once, at the end, from every counter the
    // fallbacks below may still fill in. Computed here it could only ever floor at input + output, which
    // is *below* the true total for a provider that reports its cache counters beside them.
    // Presence, not the value, for the total as well: with only the number, "the provider said 1,100" and
    // "nobody said anything" are the same 0, and a framework fallback's `max` could then raise a total the
    // provider had stated explicitly.
    let total_supplied = tokens.total_reported.is_some();
    let mut reported_total = tokens.total_reported.unwrap_or(0);
    // Presence, not the value: a reported `0` is a fact the framework fallbacks must not overwrite, exactly
    // as for the input and output sides.
    //
    // Read from the *resolved* counter, which is every declared source's answer and not just a flat
    // attribute's - the distinction that used to need a `mut` flag threaded through each fallback in turn. One
    // still is mutable, because CrewAI's block below can supply a cache read and stays in Rust.
    let mut cache_read_supplied = tokens.cache_read.is_some();
    let cache_write_supplied = tokens.cache_write.is_some();
    span.gen_ai_usage_cache_read_tokens = tokens.cache_read.unwrap_or(0);
    span.gen_ai_usage_cache_write_tokens = tokens.cache_write.unwrap_or(0);
    span.gen_ai_usage_reasoning_tokens = tokens.reasoning.unwrap_or(0);

    // A payload's own usage object - declared as the `candidate_*` span fields - fills what the flat counters
    // left unsaid.
    //
    // Gated on what was *supplied*, per side, rather than on the stored value being zero. A zero is two
    // different facts - "the provider said 0" and "nobody said anything" - and testing it conflated them in
    // both directions: with a flat input of 200 and no output attribute the whole fallback was skipped, so
    // the output stayed 0 and its cost was never charged; and an explicit flat `0/0` was overwritten by
    // whatever the fallback found. The `*_supplied` flags exist precisely to tell those apart. The candidate
    // also carries a cache counter and a reported total, and each value decides for itself whether it was
    // already supplied: with both flat sides present, `{prompt:500, completion:600, total:2000, cached:100}`
    // once stored a total of 1,100 and no cache at all.
    {
        let mut took_input = false;
        let mut took_output = false;
        if !input_supplied && let Some(v) = tokens.candidate_input {
            span.gen_ai_usage_input_tokens = v;
            input_supplied = true;
            took_input = true;
        }
        if !output_supplied && let Some(v) = tokens.candidate_output {
            span.gen_ai_usage_output_tokens = v;
            output_supplied = true;
            took_output = true;
        }
        if !cache_read_supplied && let Some(v) = tokens.candidate_cache_read {
            span.gen_ai_usage_cache_read_tokens = v;
            cache_read_supplied = true;
        }
        // The embedded total describes the embedded *parts*, so it is usable exactly when the parts actually
        // stored are those parts - either this payload supplied a side, or the flat attribute already agreed
        // with it. Requiring that *this* payload supplied both discarded a perfectly good total whenever one
        // side happened to be reported twice with the same value; taking it regardless produced a row whose
        // total did not match its own input and output, claiming 1,099 for 0 + 100 tokens.
        let side_agrees =
            |took: bool, stored: i64, candidate: Option<i64>| took || candidate == Some(stored);
        // And only when the provider did not state a total itself: `max` against an explicit flat total can
        // only raise it, which replaces the provider's own statement about the call with the framework's -
        // flat `500/600` and a flat total of 1,100 became 2,000.
        if !total_supplied
            && side_agrees(
                took_input,
                span.gen_ai_usage_input_tokens,
                tokens.candidate_input,
            )
            && side_agrees(
                took_output,
                span.gen_ai_usage_output_tokens,
                tokens.candidate_output,
            )
        {
            reported_total = tokens.candidate_total.unwrap_or(0).max(reported_total);
        }
    }

    // Usage a dialect records per message: where it summed to anything, **both** sides are filled together.
    // That pairing is why the sums are candidates rather than sources on the counter chains - per side, one
    // side's silence would not be the same fact as the pair being absent.
    if !input_supplied || !output_supplied {
        let pt = tokens.summed_input.unwrap_or(0);
        let ct = tokens.summed_output.unwrap_or(0);
        if pt > 0 || ct > 0 {
            // Per side, for the same reason as the CrewAI fallback above.
            if !input_supplied {
                span.gen_ai_usage_input_tokens = pt;
                input_supplied = true;
            }
            if !output_supplied {
                span.gen_ai_usage_output_tokens = ct;
                output_supplied = true;
            }
        }
    }

    // After every fallback, not before them: reported ahead of the CrewAI and AutoGen paths it announced
    // "no counters" for spans those paths went on to fill in.
    //
    // Every counter, not only the two sides: a span reporting cache tokens and nothing else is billed, so
    // announcing it as free would be wrong - and reading each flag here is also what keeps every source
    // maintaining it, since a chain whose final link need not update the tracker is a chain the next link
    // will not either. That is exactly how `cache_read_supplied` came to mean "a flat attribute existed"
    // rather than "the span has one", letting a later source overwrite an earlier source's value.
    if !input_supplied && !output_supplied && !cache_read_supplied && !cache_write_supplied {
        tracing::trace!(
            span_name,
            "No token counters were supplied by any source; this span bills as free"
        );
    }

    // The synthesised floor counts what the provider reports *beside* its input and output, and counts
    // nothing it reports inside them. The convention is `pricing`'s, not a second copy of it: a total that
    // assumes the cache counters are already in the input while the charge assumes they are extra describes
    // two different calls, and the one on screen would contradict the money. So an Anthropic response with
    // `input=10, cache_write=17_649, output=205` totals 17_864 rather than 215, and an OpenAI one with
    // `input=1_000` of which `cached=800` still totals its own input.
    let counted_beside_input =
        if pricing::cache_counters_are_separate(span.gen_ai_system.as_deref()) {
            span.gen_ai_usage_cache_read_tokens + span.gen_ai_usage_cache_write_tokens
        } else {
            0
        };
    let counted_beside_output = if pricing::reasoning_is_separate(span.gen_ai_system.as_deref()) {
        span.gen_ai_usage_reasoning_tokens
    } else {
        0
    };
    span.gen_ai_usage_total_tokens_reported = reported_total;
    span.gen_ai_usage_total_tokens = reported_total.max(
        span.gen_ai_usage_input_tokens
            + span.gen_ai_usage_output_tokens
            + counted_beside_input
            + counted_beside_output,
    );

    // Usage details: every `gen_ai.usage.*` member no declared counter reads.
    let already_read = counters_already_read();
    let mut details = serde_json::Map::new();
    for (key, value) in attrs {
        if let Some(field) = key.strip_prefix("gen_ai.usage.")
            && !already_read.contains(field)
        {
            let json_val = value
                .parse::<i64>()
                .map(|n| json!(n))
                .or_else(|_| value.parse::<f64>().map(|n| json!(n)))
                .unwrap_or_else(|_| json!(value));
            details.insert(field.to_string(), json_val);
        }
    }
    span.gen_ai_usage_details = if details.is_empty() {
        JsonValue::Null
    } else {
        JsonValue::Object(details)
    };
}

#[cfg(test)]
#[path = "attributes_tests.rs"]
pub(crate) mod tests;

/// The GenAI field chains the declared resolvers replaced, kept as the equivalence oracle.
#[cfg(test)]
pub(super) fn extract_genai_fields_legacy(
    span: &mut SpanData,
    attrs: &HashMap<String, String>,
    span_name: &str,
) {
    // System and operation
    span.gen_ai_system = get_first(
        attrs,
        &[
            keys::GEN_AI_PROVIDER_NAME,
            keys::GEN_AI_SYSTEM,
            "az.ai.inference.model_provider",
            "ai.model.provider",
            "llm.provider",
        ],
    );
    span.gen_ai_operation_name = attrs.get(keys::GEN_AI_OPERATION_NAME).cloned();

    // Models (including embedding/reranker model names as fallback)
    span.gen_ai_request_model = get_first(
        attrs,
        &[
            keys::GEN_AI_REQUEST_MODEL,
            "ai.model.id",
            "llm.model_name",
            keys::EMBEDDING_MODEL_NAME,
            keys::RERANKER_MODEL_NAME,
        ],
    );
    span.gen_ai_response_model =
        get_first(attrs, &[keys::GEN_AI_RESPONSE_MODEL, "llm.response.model"]);
    span.gen_ai_response_id = attrs.get(keys::GEN_AI_RESPONSE_ID).cloned();

    // Google ADK: model from llm_request JSON
    if span.gen_ai_request_model.is_none() {
        if let Some(req) = extract_json::<JsonValue>(attrs, keys::GCP_VERTEX_LLM_REQUEST) {
            if let Some(model) = req.get("model").and_then(|v| v.as_str()) {
                if !model.is_empty() {
                    span.gen_ai_request_model = Some(model.to_string());
                }
            }
        }
    }

    // CrewAI: model from crew_agents JSON (agent.llm field)
    if span.gen_ai_request_model.is_none() {
        if let Some(agents) = extract_json::<JsonValue>(attrs, "crew_agents") {
            if let Some(arr) = agents.as_array() {
                for agent in arr {
                    if let Some(model) = agent.get("llm").and_then(|v| v.as_str()) {
                        if !model.is_empty() {
                            span.gen_ai_request_model = Some(model.to_string());
                            break;
                        }
                    }
                }
            }
        }
    }

    // Logfire: model, system, operation name and max tokens from request_data JSON.
    //
    // The gate covers *everything this block can fill*, not just the model and system. Gated on those two
    // alone, a span that already carried a flat model and provider skipped the parse entirely - so
    // `request_data.max_tokens` / `max_completion_tokens` and the operation-name fallback were unreachable
    // exactly when the rest of the span was well populated, which is the common Logfire shape rather than an
    // edge one. Each field inside is still filled only when it is the one missing.
    if span.gen_ai_request_model.is_none()
        || span.gen_ai_system.is_none()
        || span.gen_ai_operation_name.is_none()
        || span.gen_ai_max_tokens.is_none()
    {
        if let Some(req) = extract_json::<JsonValue>(attrs, keys::REQUEST_DATA) {
            if span.gen_ai_request_model.is_none() {
                if let Some(model) = req.get("model").and_then(|v| v.as_str()) {
                    if !model.is_empty() {
                        span.gen_ai_request_model = Some(model.to_string());
                    }
                }
            }
            if span.gen_ai_system.is_none() {
                // Anthropic: top-level "system" key (string/array); OpenAI: messages[0].role=system
                if req.get("system").is_some() {
                    span.gen_ai_system = Some("anthropic".to_string());
                } else if req.get("messages").is_some() {
                    span.gen_ai_system = Some("openai".to_string());
                }
            }
            if span.gen_ai_operation_name.is_none() && req.get("messages").is_some() {
                span.gen_ai_operation_name = Some("chat".to_string());
            }
            if span.gen_ai_max_tokens.is_none() {
                span.gen_ai_max_tokens = req
                    .get("max_tokens")
                    .or_else(|| req.get("max_completion_tokens"))
                    .and_then(|v| v.as_i64());
            }
        }
    }

    // Request parameters
    span.gen_ai_temperature = parse_opt(attrs, keys::GEN_AI_TEMPERATURE);
    span.gen_ai_top_p = parse_opt(attrs, keys::GEN_AI_TOP_P);
    span.gen_ai_top_k = parse_opt(attrs, keys::GEN_AI_TOP_K);
    // Only when the flat attribute is actually present. Assigning unconditionally overwrote the
    // `request_data` fallback above with `None` whenever the flat attribute was absent - which is every
    // Logfire span, so `request_data.max_tokens` / `max_completion_tokens` never survived extraction.
    if let Some(max_tokens) = parse_opt(attrs, keys::GEN_AI_MAX_TOKENS) {
        span.gen_ai_max_tokens = Some(max_tokens);
    }
    span.gen_ai_frequency_penalty = parse_opt(attrs, keys::GEN_AI_FREQUENCY_PENALTY);
    span.gen_ai_presence_penalty = parse_opt(attrs, keys::GEN_AI_PRESENCE_PENALTY);

    // OpenInference llm.invocation_parameters fallback
    if let Some(params_json) = attrs.get(keys::LLM_INVOCATION_PARAMETERS) {
        if let Ok(params) = serde_json::from_str::<JsonValue>(params_json) {
            if span.gen_ai_temperature.is_none() {
                span.gen_ai_temperature = params.get("temperature").and_then(|v| v.as_f64());
            }
            if span.gen_ai_top_p.is_none() {
                span.gen_ai_top_p = params.get("top_p").and_then(|v| v.as_f64());
            }
            if span.gen_ai_top_k.is_none() {
                span.gen_ai_top_k = params.get("top_k").and_then(|v| v.as_i64());
            }
            if span.gen_ai_max_tokens.is_none() {
                span.gen_ai_max_tokens = params
                    .get("max_tokens")
                    .or_else(|| params.get("max_output_tokens"))
                    .and_then(|v| v.as_i64());
            }
            if span.gen_ai_frequency_penalty.is_none() {
                span.gen_ai_frequency_penalty =
                    params.get("frequency_penalty").and_then(|v| v.as_f64());
            }
            if span.gen_ai_presence_penalty.is_none() {
                span.gen_ai_presence_penalty =
                    params.get("presence_penalty").and_then(|v| v.as_f64());
            }
        }
    }

    if let Some(stops) = attrs.get(keys::GEN_AI_STOP_SEQUENCES) {
        span.gen_ai_stop_sequences = parse_string_array(stops);
    }
    if let Some(reasons) = attrs.get(keys::GEN_AI_FINISH_REASONS) {
        span.gen_ai_finish_reasons = parse_string_array(reasons);
    }

    // Agent fields
    span.gen_ai_agent_id = get_first(attrs, &[keys::GEN_AI_AGENT_ID, keys::AWS_BEDROCK_AGENT_ID]);
    span.gen_ai_agent_name = get_first(
        attrs,
        &[
            keys::GEN_AI_AGENT_NAME,
            // OpenInference agent span attribute.
            "agent.name",
            "agent_role",
            "recipient_agent_class",
            "sender_agent_class",
        ],
    );

    // Tool fields - logfire.msg is used by Pydantic AI for descriptive tool names
    span.gen_ai_tool_name = get_first(
        attrs,
        &[
            keys::GEN_AI_TOOL_NAME,
            "tool.name",
            "tool_name",
            keys::LOGFIRE_MSG,
        ],
    )
    .or_else(|| span_name.strip_prefix("execute_tool ").map(String::from));
    span.gen_ai_tool_call_id = attrs.get(keys::GEN_AI_TOOL_CALL_ID).cloned();

    // Performance
    span.gen_ai_server_ttft_ms = parse_opt(attrs, keys::GEN_AI_TTFT);
    span.gen_ai_server_request_duration_ms = parse_opt(attrs, keys::GEN_AI_REQUEST_DURATION);
}
