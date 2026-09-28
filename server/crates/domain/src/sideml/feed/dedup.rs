//! SideML message deduplication and ordering.
//!
//! This module handles the complex task of reconstructing real conversation timelines
//! from OTEL spans that may contain duplicated messages (history duplication) and
//! need proper ordering (tool chains).
//!
//! # The Core Challenge
//!
//! OTEL traces often contain duplicate messages because:
//! 1. **History duplication**: Child spans re-send parent span messages as context
//! 2. **Streaming**: Same message sent in chunks with different timestamps
//! 3. **Tool chains**: ToolUse → Tool execution → ToolResult must maintain logical order
//!
//! # Solution: Birth Time Algorithm
//!
//! For each unique message identity:
//! - **Birth time** = earliest timestamp where this identity appeared
//! - This is when the message was REALLY sent/received
//!
//! History messages have `event_time=T+N` but `birth_time=T` (from first occurrence).
//!
//! # Message Types
//!
//! Blocks are pre-classified as OUTPUT or INPUT in the classification phase (mod.rs):
//! - **OUTPUT**: Uses span_end time, protected from history marking
//! - **INPUT**: Uses earliest occurrence time (birth time), can be marked as history
//!
//! The `uses_span_end` field is set on each block before this module processes them.
//!
//! # Quality Scoring
//!
//! When deduplicating, the highest-quality version is kept:
//! - Non-history block (+100) - strongly prefer current-turn messages
//! - Has finish_reason (+10) - complete response
//! - Enrichment content (+5) - thinking blocks
//! - Output source (+4) - vs input source copies
//! - Tool span (+3) - actual execution, not re-sent context
//! - Event source (+2) - vs attribute source
//! - Has model info (+1)
//!
//! # Ordering
//!
//! For messages with the same birth time, original message order is preserved
//! (by message_index, then entry_index). This maintains the order as it
//! appeared in the source data.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Utc};

use super::super::provenance::PositionPath;
use super::types::BlockEntry;
use crate::sideml::types::{ChatRole, ContentBlock};
use sideseat_ports::types::MessageCategory;

// ============================================================================
// MESSAGE IDENTITY
// ============================================================================

/// Unique identity of a message for deduplication.
///
/// Two messages with the same identity are considered duplicates.
/// The identity is based on content, not position or timing.
///
/// # Tool Call Identity
///
/// Tool calls are identified by content hash (name + input), NOT by call_id.
/// This is because history re-sends often regenerate call IDs, causing the
/// same semantic tool call to appear with different IDs.
///
/// # Tool Result Identity
///
/// Tool results use `tool_use_id` as primary identity when present, falling back
/// to content hash. This is universal across frameworks:
/// - **Vercel AI SDK** (`toModelOutput`): Same `tool_use_id`, different content format.
///   Caught by `tool_use_id`-based identity here.
/// - **Strands** (history re-sends): Same content, regenerated `tool_use_id`.
///   Caught by the independent content-hash deduplication in `history.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum MessageIdentity {
    /// Regular message identified by trace, role, and content hash
    Regular {
        trace_id: String,
        role: ChatRole,
        /// Hash of semantic content blocks only (excludes enrichment/metadata)
        semantic_hash: u64,
    },
    /// Tool call identified by content hash (name + input)
    /// Using content hash instead of call_id because history re-sends regenerate IDs
    ToolCall {
        trace_id: String,
        /// Hash of tool name + input (call_id is ignored for identity)
        content_hash: u64,
    },
    /// Tool result identified by tool_use_id (primary) or content hash (fallback).
    ///
    /// `tool_use_id` is stable across content transformations (e.g., Vercel AI SDK's
    /// `toModelOutput`), making it the preferred identity signal. Falls back to
    /// content hash when `tool_use_id` is absent.
    ToolResult {
        trace_id: String,
        /// Hash of tool_use_id when present, or content hash as fallback
        identity_hash: u64,
    },
}

impl MessageIdentity {
    /// Create identity for a block entry.
    pub fn from_block(block: &BlockEntry) -> Self {
        // Tool use: identify by content hash (name + input)
        // We use content hash instead of call_id because history re-sends regenerate IDs
        if let ContentBlock::ToolUse { name, input, .. } = &block.content {
            return Self::ToolCall {
                trace_id: block.trace_id.clone(),
                content_hash: compute_tool_call_hash(name, input),
            };
        }

        // Tool result: identify by tool_use_id (primary) or content hash (fallback)
        // tool_use_id is stable across content transformations (Vercel toModelOutput).
        // History re-sends with regenerated IDs are caught by content-hash deduplication.
        if let ContentBlock::ToolResult {
            tool_use_id,
            name,
            content,
            is_error,
        } = &block.content
        {
            let identity_hash = match tool_use_id {
                Some(tid) if !tid.is_empty() => compute_tool_use_id_hash(tid),
                // Name and error flag, not content alone: without them two tools both reporting
                // "ok" were one message, and so were a success and a failure with matching text.
                _ => compute_tool_result_hash(name.as_deref(), *is_error, content),
            };
            return Self::ToolResult {
                trace_id: block.trace_id.clone(),
                identity_hash,
            };
        }

        // An exception is a fact *about a span*, so two spans' exceptions are two occurrences.
        //
        // This block is not read out of a payload at all - it is composed from the span's own
        // `exception_*` fields (`compose_error_text`), which is why its identity may safely carry the
        // span: there is no history re-send of a span's exception to collapse against, and a
        // re-delivered span keeps its span id, so re-delivery still collapses.
        //
        // Without it, `openai-agents/image_gen` reported **one** error where three separate
        // `generate_image` executions had each failed with the same message: the three tool results
        // survived, the three exceptions became one, and the trace showed three failures and one
        // explanation. The goldens had recorded that as correct, and `assert_no_duplicates` would have
        // reported the repair as a defect - which is exactly the false-equivalence shape the design
        // record calls the hardest to detect, found here in committed data.
        if block.category == MessageCategory::Exception {
            return Self::Regular {
                trace_id: block.trace_id.clone(),
                role: block.role,
                semantic_hash: {
                    let mut hasher = DefaultHasher::new();
                    block.span_id.hash(&mut hasher);
                    block.content_hash.hash(&mut hasher);
                    hasher.finish()
                },
            };
        }

        // Regular message: identify by role + content hash
        Self::Regular {
            trace_id: block.trace_id.clone(),
            role: block.role,
            semantic_hash: compute_semantic_hash(&block.content),
        }
    }
}

/// How many *other* calls of the same shape precede this one in its own response.
///
/// A tool call's identity deliberately ignores the provider's call id, because a framework re-sending
/// its history regenerates ids and the same call would otherwise appear twice. The cost was that a
/// model asking for the same thing twice in one response - two `generate_image` calls with the same
/// prompt, distinct ids - collapsed into one, and its second result was left answering a call that
/// was no longer there.
///
/// Within one response, though, distinct ids are unambiguous: a provider does not issue two ids for
/// one call. So each call takes the rank of its id among the same-shaped calls of that response, and
/// that rank joins its identity. A single call ranks 0 and behaves exactly as before; a re-send of a
/// pair ranks 0 and 1 again, whatever the ids were regenerated to, so the pair still collapses onto
/// the pair rather than into one call.
///
/// A response is `(trace, span, source)` - the event or attribute the blocks arrived in. Not the
/// message index: normalisation gives every tool call a message of its own, so two calls from one
/// event already have different indices, and keying on them would put each call in a bucket of one.
/// The source is what keeps a span's input array separate from its output event, which is where the
/// same call legitimately appears twice.
///
/// A tool *result* inherits the rank of the call it answers, so two results of two identical calls
/// stay two. Everything else ranks 0: without an id there is no evidence of a genuine repeat, and
/// treating repeated text as two messages would undo the history collapsing this pipeline exists for.
/// What identifies one call among same-shaped calls of a response.
#[derive(PartialEq, Eq, Hash)]
enum CallKey<'a> {
    /// The provider's id, which is proof on its own: no provider issues two ids for one call.
    Id(&'a str),
    /// Where the call sat, used only where the carrier's structure is evidence of multiplicity.
    Position(&'a PositionPath),
    /// Nothing distinguishes it - accumulated state with no ids, where two positions describe one
    /// call. Every such call of a response collapses onto the first.
    Indistinct,
}

/// Which evidence decides whether two same-shaped calls are two calls.
///
/// The id wins wherever there is one. Failing that, the carrier decides: a single emission means two
/// positions are two calls, while accumulated state re-lists itself and means they are one. That
/// judgement lives in `sideml::carrier`, declared per carrier, rather than being the unstated global
/// rule it used to be.
fn call_key<'a>(block: &'a BlockEntry, id: Option<&'a str>) -> CallKey<'a> {
    if let Some(id) = id.filter(|s| !s.is_empty()) {
        return CallKey::Id(id);
    }
    let semantics = crate::sideml::carrier::semantics_for_context(&block.carrier_context());
    if semantics.position_proves_distinct_occurrence {
        CallKey::Position(&block.position)
    } else {
        CallKey::Indistinct
    }
}

/// Bucket key for the response-scoped position map: `(trace, span, source, event, attribute, shape)`.
type ResponseKey<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    u64,
);

pub(super) fn call_repeat_ordinals(blocks: &[BlockEntry]) -> Vec<u32> {
    // (trace, span, source, call shape) -> the ids seen, in order of first appearance.

    // What tells two same-shaped calls of one response apart: the provider's id where there is one,
    // and the position otherwise.
    //
    // The id has to win when present. A framework's accumulated state re-lists its own messages -
    // LangChain's `output.value` carries each tool call twice - and those copies sit at different
    // positions while describing one call. Ranking by position alone turned every such echo into a
    // second call, which the goldens' duplicate invariant caught. The position still earns its place
    // where no id was sent: two identical calls with no ids are otherwise indistinguishable.
    // The rank assigned to each key, not a list to search. As a `Vec` the lookup was
    // `iter().position(..)`, so a response - or, for id-bearing calls, a whole trace - with N same-shaped
    // calls cost N^2 comparisons to rank them. A map from key to rank gives the same answer by construction:
    // the rank *is* the order of first appearance, which is the map's size when the key is inserted.
    let mut keys_by_response: HashMap<ResponseKey<'_>, HashMap<CallKey<'_>, u32>> = HashMap::new();
    // (trace, call id) -> that call's rank, for the results that answer it.
    let mut rank_by_call: HashMap<(&str, &str), u32> = HashMap::new();

    // How many calls of one shape a single response lists, which is what decides *which* evidence
    // separates two same-shaped calls - see `rank_scope`.
    let mut shape_count: HashMap<ResponseKey<'_>, usize> = HashMap::new();
    for block in blocks {
        if let ContentBlock::ToolUse { name, input, .. } = &block.content {
            *shape_count
                .entry(response_scope(block, compute_tool_call_hash(name, input)))
                .or_insert(0) += 1;
        }
    }

    for block in blocks {
        // Plain messages get an ordinal too, but only inside a carrier whose structure is evidence of
        // distinct occurrences - `gen_ai.choice`, `gen_ai.output.messages` and the other atomic emissions.
        // The model producing the same text twice in one emission is two turns; the same text repeating
        // inside a conversation snapshot is a re-statement, and that read is preserved by
        // `position_proves_distinct_occurrence: false` on `SNAPSHOT` and `ACCUMULATED_STATE`.
        //
        // A shape distinct from tool-call shapes, so calls and text of the same "shape" never share a
        // bucket in the response map. Text does not carry an id, so `call_key` returns `Indistinct` in
        // any snapshot/state carrier - which keeps ADK's repeated "For context:" as one message rather
        // than becoming N.
        if let ContentBlock::ToolUse { id, name, input } = &block.content {
            let shape = compute_tool_call_hash(name, input);
            record_position(
                block,
                shape,
                &mut keys_by_response,
                &mut rank_by_call,
                id.as_deref(),
                &shape_count,
            );
        } else if let Some(shape) = plain_message_shape(block) {
            let semantics = crate::sideml::carrier::semantics_for_context(&block.carrier_context());
            if semantics.position_proves_distinct_occurrence {
                record_position(
                    block,
                    shape,
                    &mut keys_by_response,
                    &mut rank_by_call,
                    None,
                    &shape_count,
                );
            }
        }
    }

    blocks
        .iter()
        .map(|block| match &block.content {
            ContentBlock::ToolUse { id, name, input } => {
                let shape = compute_tool_call_hash(name, input);
                lookup_position(block, shape, &keys_by_response, id.as_deref(), &shape_count)
            }
            ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .and_then(|id| rank_by_call.get(&(block.trace_id.as_str(), id)))
                .copied()
                .unwrap_or(0),
            _ => {
                let Some(shape) = plain_message_shape(block) else {
                    return 0;
                };
                let semantics =
                    crate::sideml::carrier::semantics_for_context(&block.carrier_context());
                if !semantics.position_proves_distinct_occurrence {
                    return 0;
                }
                lookup_position(block, shape, &keys_by_response, None, &shape_count)
            }
        })
        .collect()
}

/// A per-message shape that is not a tool call.
///
/// Used to key the plain-message ordinal within an atomic emission. Tool calls have their own shape and
/// their own bucket; tool results take their call's ordinal; and non-emission carriers (snapshots,
/// accumulated state) do not enter this bucket at all - see `call_repeat_ordinals`.
fn plain_message_shape(block: &BlockEntry) -> Option<u64> {
    match &block.content {
        ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. } => None,
        _ => {
            let mut hasher = DefaultHasher::new();
            "plain_message".hash(&mut hasher);
            block.role.hash(&mut hasher);
            Some(compute_semantic_hash(&block.content) ^ hasher.finish())
        }
    }
}

/// Record a block's position among same-shape blocks of its response, and remember the id-to-rank map.
fn record_position<'a>(
    block: &'a BlockEntry,
    shape: u64,
    keys_by_response: &mut HashMap<ResponseKey<'a>, HashMap<CallKey<'a>, u32>>,
    rank_by_call: &mut HashMap<(&'a str, &'a str), u32>,
    id: Option<&'a str>,
    shape_count: &HashMap<ResponseKey<'a>, usize>,
) {
    let key = call_key(block, id);
    // An id-bearing call ranks across its **trace**, not within one response.
    //
    // Two executions of the same call in one trace arrive on different spans, and each span holds one
    // of them - so a per-response rank gave both ordinal 0 and dedup merged them. Measured on
    // `agent-framework/tool_use`: four executions with four distinct provider ids, two of them the same
    // NYC request, came back as **three** calls beside **two** NYC answers. Same shape in
    // `openai-agents/tool_use`, both `mcp_tools` suites and both `subagents` suites.
    //
    // The provider's id is what separates them, and it is trustworthy for this: a re-listing of one
    // execution repeats the *same* id (which is why the copies across three spans still collapse), while
    // a second execution gets a new one. The id is still not the *identity* - a history re-send may
    // regenerate it - which is why it decides the rank rather than the key.
    //
    // Position keeps its job where no id was sent, and only there: two identical id-less calls of one
    // response are otherwise indistinguishable, and ranking them across a trace would turn a snapshot's
    // echo into a second call.
    let seen = keys_by_response
        .entry(rank_scope(block, shape, id, shape_count))
        .or_default();
    let next_rank = seen.len() as u32;
    let rank = *seen.entry(key).or_insert(next_rank);
    if let Some(id) = id.filter(|s| !s.is_empty()) {
        rank_by_call
            .entry((block.trace_id.as_str(), id))
            .or_insert(rank);
    }
}

/// The scope a repeat rank is counted within - see the note in `record_position`. Defined once, because
/// a recording site and a lookup site that disagree would silently rank everything zero.
fn response_scope<'a>(block: &'a BlockEntry, shape: u64) -> ResponseKey<'a> {
    (
        block.trace_id.as_str(),
        block.span_id.as_str(),
        block.source_type.as_str(),
        block.event_name.as_deref(),
        block.source_attribute.as_deref(),
        shape,
    )
}

fn rank_scope<'a>(
    block: &'a BlockEntry,
    shape: u64,
    id: Option<&'a str>,
    shape_count: &HashMap<ResponseKey<'a>, usize>,
) -> ResponseKey<'a> {
    // Only an id from a carrier that **cannot be a re-send** ranks trace-wide. A history re-send
    // regenerates the id, so trusting a snapshot's id turns one execution's echo into a second
    // execution. The shape-multiplicity test below covers the *pair* form (a re-sent pair lists its
    // shape twice in one response); it cannot cover a **single** call re-sent once, which lists its
    // shape once in each response and so looks exactly like two executions -
    // `a_resent_single_call_with_a_regenerated_id_is_still_one_call` reproduced the duplicate.
    //
    // The fact is the carrier's, deliberately, not the block's `is_history` flag: duplicate
    // detection groups by these very ordinals, so a flag it sets cannot gate the rank that decides
    // whether it fires - the two executions this rank exists to keep (`agent-framework/tool_use` and
    // the five suites beside it) all report their calls through *emission* carriers, and a re-send by
    // definition arrives through one that `may_restate_prior_observations`. That fact and not
    // `may_contain_framework_state`: the question here is whether the id could have been *regenerated*,
    // which is a property of replaying an earlier observation, not of holding a scratchpad.
    let id_is_execution_evidence =
        !crate::sideml::carrier::semantics_for_context(&block.carrier_context())
            .may_restate_prior_observations;
    let response = response_scope(block, shape);
    let lists_shape_once = shape_count.get(&response).copied().unwrap_or(1) <= 1;
    if id_is_execution_evidence && id.is_some_and(|s| !s.is_empty()) && lists_shape_once {
        (block.trace_id.as_str(), "", "", None, None, shape)
    } else {
        response
    }
}

fn lookup_position<'a>(
    block: &'a BlockEntry,
    shape: u64,
    keys_by_response: &HashMap<ResponseKey<'a>, HashMap<CallKey<'a>, u32>>,
    id: Option<&'a str>,
    shape_count: &HashMap<ResponseKey<'a>, usize>,
) -> u32 {
    let key = call_key(block, id);
    keys_by_response
        .get(&rank_scope(block, shape, id, shape_count))
        .and_then(|seen| seen.get(&key).copied())
        .unwrap_or(0)
}

/// Compute hash for tool call identity (name + input).
pub(super) fn compute_tool_call_hash(name: &str, input: &serde_json::Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_call".hash(&mut hasher);
    name.hash(&mut hasher);
    hash_json_into(input, &mut hasher);
    hasher.finish()
}

/// Compute hash for tool result identity (content).
/// Used as fallback when tool_use_id is absent.
fn compute_tool_result_hash(
    name: Option<&str>,
    is_error: bool,
    content: &serde_json::Value,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_result".hash(&mut hasher);
    name.hash(&mut hasher);
    is_error.hash(&mut hasher);
    hash_tool_result_content_into(content, &mut hasher);
    hasher.finish()
}

/// Compute hash for tool_use_id-based identity.
/// Primary identity signal for tool results — stable across content transformations.
fn compute_tool_use_id_hash(tool_use_id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_result_by_id".hash(&mut hasher);
    tool_use_id.hash(&mut hasher);
    hasher.finish()
}

// ============================================================================
// CONTENT NORMALIZATION FOR HASHING
// ============================================================================

/// Normalize JSON for consistent hashing: sort object keys.
pub(super) fn normalize_json_for_hash(value: &serde_json::Value) -> String {
    normalize_json(value, EmptyMembers::Keep)
}

/// As [`normalize_json_for_hash`], but treating a member with no value as absent.
///
/// For a **structured answer only**. Filling a schema adds the fields the model did not produce,
/// as `null`, `[]`, `{}` or `""`, so an SDK that reports both the model's raw object and its
/// schema-shaped one - Vercel's `generateObject` puts the raw object on the inner span and the
/// normalized one on the outer - emits the same answer twice with different bytes, and both
/// reached the feed as separate assistant messages.
///
/// Deliberately not applied to tool inputs or tool results, where an explicitly empty collection
/// is a different answer from a missing one: a search result of `{"results": []}` says "no
/// matches" and `{}` says nothing, and collapsing those would drop a real message. The
/// distinction only stops mattering once a schema has supplied the empty value itself.
///
/// Only identity is affected: what the API returns is still the block's own content.
#[cfg(test)]
pub(super) fn normalize_structured_json_for_hash(value: &serde_json::Value) -> String {
    normalize_json(value, EmptyMembers::Drop)
}

/// Whether object members carrying no value take part in identity.
#[derive(Clone, Copy, PartialEq)]
enum EmptyMembers {
    Keep,
    Drop,
}

fn normalize_json(value: &serde_json::Value, empty: EmptyMembers) -> String {
    use serde_json::Value as JsonValue;
    match value {
        JsonValue::Object(map) => {
            let mut pairs: Vec<_> = map
                .iter()
                .filter(|(_, v)| empty == EmptyMembers::Keep || !is_empty_json_member(v))
                .collect();
            pairs.sort_by_key(|(k, _)| *k);
            let sorted: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}:{}", k, normalize_json(v, empty)))
                .collect();
            format!("{{{}}}", sorted.join(","))
        }
        JsonValue::Array(arr) => {
            let items: Vec<String> = arr.iter().map(|v| normalize_json(v, empty)).collect();
            format!("[{}]", items.join(","))
        }
        _ => value.to_string(),
    }
}

/// Feed a JSON value's canonical form to a hasher **without building it**.
///
/// The canonical form used to be materialised as a `String` - nested `format!` and `join` at every
/// level - purely so it could be hashed. For a tool result carrying a base64 image that copies
/// megabytes once per nesting level, and it happens twice per block (once in flatten, again in
/// history marking), which was 620 ms of a 700 ms request on `vercel-ai-js/image-gen`.
///
/// The bytes differ from the old string form, so hash *values* differ; what matters is preserved,
/// which is that equal content hashes equally and unequal content does not. Lengths are hashed
/// alongside the members so that concatenation cannot make two different shapes agree.
pub(super) fn hash_json_into<H: Hasher>(value: &serde_json::Value, hasher: &mut H) {
    hash_json_streaming(value, EmptyMembers::Keep, hasher);
}

/// As [`hash_json_into`], but treating a member with no value as absent - see
/// [`normalize_structured_json_for_hash`] for why that is right for a structured answer only.
pub(super) fn hash_structured_json_into<H: Hasher>(value: &serde_json::Value, hasher: &mut H) {
    hash_json_streaming(value, EmptyMembers::Drop, hasher);
}

fn hash_json_streaming<H: Hasher>(value: &serde_json::Value, empty: EmptyMembers, hasher: &mut H) {
    use serde_json::Value as JsonValue;
    // A per-kind tag, so a string cannot hash as the object whose rendering it matches.
    match value {
        JsonValue::Object(map) => {
            0u8.hash(hasher);
            let mut keys: Vec<&str> = map
                .iter()
                .filter(|(_, v)| empty == EmptyMembers::Keep || !is_empty_json_member(v))
                .map(|(k, _)| k.as_str())
                .collect();
            keys.sort_unstable();
            keys.len().hash(hasher);
            for key in keys {
                key.hash(hasher);
                if let Some(member) = map.get(key) {
                    hash_json_streaming(member, empty, hasher);
                }
            }
        }
        JsonValue::Array(arr) => {
            1u8.hash(hasher);
            arr.len().hash(hasher);
            for item in arr {
                hash_json_streaming(item, empty, hasher);
            }
        }
        // The one that matters: hashed in place, never copied or escaped.
        JsonValue::String(s) => {
            2u8.hash(hasher);
            s.hash(hasher);
        }
        JsonValue::Number(n) => {
            3u8.hash(hasher);
            n.to_string().hash(hasher);
        }
        JsonValue::Bool(b) => {
            4u8.hash(hasher);
            b.hash(hasher);
        }
        JsonValue::Null => 5u8.hash(hasher),
    }
}

/// Feed a tool result's canonical form to a hasher, unwrapping the shapes frameworks wrap them in.
///
/// A tool result arrives wrapped differently per framework - a bare string, an array of content
/// blocks, `{"json": ...}`, `{"type": "text", "text": ...}`, `{"type": "json", "data": ...}` - and two
/// wrappings of one answer must hash alike. The array-with-text branch still builds its joined text,
/// which is small by construction: the megabytes live in image blocks, which yield no text and so take
/// the streaming JSON path.
pub(super) fn hash_tool_result_content_into<H: Hasher>(
    content: &serde_json::Value,
    hasher: &mut H,
) {
    use serde_json::Value as JsonValue;
    match content {
        JsonValue::String(s) => s.trim().hash(hasher),
        JsonValue::Array(arr) => {
            let texts: Vec<String> = arr.iter().filter_map(extract_text_from_block).collect();
            if texts.is_empty() {
                hash_json_into(content, hasher);
            } else {
                texts.join("\n").trim().hash(hasher);
            }
        }
        JsonValue::Object(obj) => {
            if let Some(inner) = obj.get("json") {
                return hash_json_into(inner, hasher);
            }
            if obj.get("type").and_then(|t| t.as_str()) == Some("text")
                && let Some(text) = obj.get("text").and_then(|t| t.as_str())
            {
                return text.trim().hash(hasher);
            }
            if obj.get("type").and_then(|t| t.as_str()) == Some("json")
                && let Some(data) = obj.get("data")
            {
                if let Some(json) = data.get("json") {
                    return hash_json_into(json, hasher);
                }
                return hash_json_into(data, hasher);
            }
            hash_json_into(content, hasher)
        }
        _ => content.to_string().hash(hasher),
    }
}

/// True for a value that carries no information: `null`, `""`, `[]`, `{}`, or a container whose
/// every member is itself empty.
fn is_empty_json_member(value: &serde_json::Value) -> bool {
    use serde_json::Value as JsonValue;
    match value {
        JsonValue::Null => true,
        JsonValue::String(s) => s.trim().is_empty(),
        JsonValue::Array(arr) => arr.iter().all(is_empty_json_member),
        JsonValue::Object(map) => map.values().all(is_empty_json_member),
        _ => false,
    }
}

/// Extract text from a content block for normalization.
fn extract_text_from_block(block: &serde_json::Value) -> Option<String> {
    let obj = block.as_object()?;
    let block_type = obj.get("type").and_then(|t| t.as_str())?;

    match block_type {
        "text" => obj
            .get("text")
            .and_then(|t| t.as_str())
            .map(|s| s.trim().to_string()),
        "json" => {
            if let Some(data) = obj.get("data") {
                if let Some(json) = data.get("json") {
                    return Some(normalize_json_for_hash(json));
                }
                return Some(normalize_json_for_hash(data));
            }
            None
        }
        _ => None,
    }
}

/// Compute hash of semantic content (excludes enrichment/metadata blocks).
/// Re-exported from mod.rs to avoid duplication.
pub(super) use super::compute_block_hash as compute_semantic_hash;

// ============================================================================
// BIRTH TIME MAP
// ============================================================================

/// Combine trace_id and hash into a single u128 lookup key.
/// Avoids String allocation on HashMap lookups.
#[inline]
fn make_key(trace_id: &str, hash: u64) -> u128 {
    let mut hasher = DefaultHasher::new();
    trace_id.hash(&mut hasher);
    let trace_hash = hasher.finish();
    ((trace_hash as u128) << 64) | (hash as u128)
}

/// Combine trace_id, role, and semantic hash into a single u128 lookup key.
#[inline]
fn make_regular_key(trace_id: &str, role: ChatRole, semantic_hash: u64) -> u128 {
    let mut hasher = DefaultHasher::new();
    trace_id.hash(&mut hasher);
    role.hash(&mut hasher);
    let combined = hasher.finish();
    ((combined as u128) << 64) | (semantic_hash as u128)
}

/// Maps message identities to their "birth time" (earliest occurrence).
///
/// This is the key data structure for deduplication:
/// - INPUT messages: birth_time = min(all occurrences)
/// - OUTPUT messages: birth_time = their own effective timestamp
///
/// Uses u128 combined hash keys to avoid String allocation on lookups.
#[derive(Debug, Default)]
struct BirthTimeMap {
    /// Regular message identity → earliest effective timestamp
    regular_times: HashMap<u128, DateTime<Utc>>,
    /// Tool call content hash → earliest timestamp
    tool_call_times: HashMap<u128, DateTime<Utc>>,
    /// Tool result identity hash → earliest timestamp
    tool_result_times: HashMap<u128, DateTime<Utc>>,
}

impl BirthTimeMap {
    /// Record a timestamp for a regular message identity (keeps minimum).
    fn record_regular(
        &mut self,
        trace_id: &str,
        role: ChatRole,
        semantic_hash: u64,
        timestamp: DateTime<Utc>,
    ) {
        let key = make_regular_key(trace_id, role, semantic_hash);
        self.regular_times
            .entry(key)
            .and_modify(|t| {
                if timestamp < *t {
                    *t = timestamp;
                }
            })
            .or_insert(timestamp);
    }

    /// Record a timestamp for a tool call (keeps minimum for dedup).
    fn record_tool_call(&mut self, trace_id: &str, content_hash: u64, timestamp: DateTime<Utc>) {
        let key = make_key(trace_id, content_hash);
        self.tool_call_times
            .entry(key)
            .and_modify(|t| {
                if timestamp < *t {
                    *t = timestamp;
                }
            })
            .or_insert(timestamp);
    }

    /// Record a timestamp for a tool result (keeps minimum for dedup).
    fn record_tool_result(&mut self, trace_id: &str, identity_hash: u64, timestamp: DateTime<Utc>) {
        let key = make_key(trace_id, identity_hash);
        self.tool_result_times
            .entry(key)
            .and_modify(|t| {
                if timestamp < *t {
                    *t = timestamp;
                }
            })
            .or_insert(timestamp);
    }

    /// Get birth time for a regular message.
    #[inline]
    fn get_regular(
        &self,
        trace_id: &str,
        role: ChatRole,
        semantic_hash: u64,
    ) -> Option<DateTime<Utc>> {
        let key = make_regular_key(trace_id, role, semantic_hash);
        self.regular_times.get(&key).copied()
    }

    /// Get birth time for a tool call.
    #[inline]
    fn get_tool_call(&self, trace_id: &str, content_hash: u64) -> Option<DateTime<Utc>> {
        let key = make_key(trace_id, content_hash);
        self.tool_call_times.get(&key).copied()
    }

    /// Get birth time for a tool result.
    #[inline]
    fn get_tool_result(&self, trace_id: &str, identity_hash: u64) -> Option<DateTime<Utc>> {
        let key = make_key(trace_id, identity_hash);
        self.tool_result_times.get(&key).copied()
    }
}

// ============================================================================
// EFFECTIVE TIMESTAMP
// ============================================================================

/// Context needed for timestamp computation.
#[derive(Debug, Clone)]
pub struct SpanTimestamps {
    pub span_start: DateTime<Utc>,
    pub span_end: Option<DateTime<Utc>>,
}

/// Compute the effective timestamp for ordering.
///
/// The `uses_span_end` field determines timestamp strategy:
/// - `uses_span_end=true`: Use span_end (block represents COMPLETION of an operation)
/// - `uses_span_end=false`: Use event_time (block is intermediate or input)
///
/// # What uses_span_end Really Means
///
/// `uses_span_end=true` means "this block represents a COMPLETION event":
/// - `gen_ai.choice` events (generation completed)
/// - `gen_ai.content.completion` events
/// - Blocks with `finish_reason` (explicit completion marker)
/// - ToolResult from tool spans (tool execution completed)
///
/// `uses_span_end=false` means "this block is intermediate or input":
/// - ToolUse (decision made DURING generation, not at completion)
/// - Assistant text without finish_reason (intermediate streaming)
/// - User/System messages (input)
/// - Tool messages from non-tool spans (history copies)
///
/// # Why This Matters for Ordering
///
/// Consider a generation span producing: ToolUse → ToolResult → FinalText
/// - ToolUse event_time: T=100 (mid-generation)
/// - ToolResult event_time: T=200 (after tool execution)
/// - FinalText event_time: T=300 (generation complete)
/// - span_end: T=300
///
/// If ToolUse used span_end, it would have effective_time=300, sorting AFTER
/// ToolResult (effective_time=200). This would be wrong.
///
/// By using event_time for ToolUse, we get: 100 < 200 < 300 (correct order).
pub fn effective_timestamp(
    block: &BlockEntry,
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> DateTime<Utc> {
    let timestamps = span_timestamps.get(&block.span_id);

    if block.uses_span_end {
        // COMPLETION: use span_end (when operation finished)
        // Fallback chain: span_end → event_time
        // Safety: .max(event_time) handles malformed data where span_end < event_time
        timestamps
            .and_then(|t| t.span_end)
            .unwrap_or(block.timestamp)
            .max(block.timestamp)
    } else {
        // INTERMEDIATE/INPUT: use event_time (when event was recorded)
        // Safety: .max(span_start) ensures events aren't placed before their span
        let span_start = timestamps.map(|t| t.span_start).unwrap_or(block.timestamp);
        block.timestamp.max(span_start)
    }
}

// ============================================================================
// BIRTH TIME COMPUTATION
// ============================================================================

/// Build birth time map from blocks.
///
/// Pass 1: Record all timestamps to find birth times.
///
/// IMPORTANT: Only non-history blocks contribute to birth times.
/// History copies often have earlier timestamps (when context was assembled)
/// but shouldn't affect the ordering of actual message occurrences.
fn build_birth_times(
    blocks: &[BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> BirthTimeMap {
    let mut map = BirthTimeMap::default();

    for block in blocks {
        // Skip history blocks - they shouldn't affect birth time calculation
        // History copies have misleading timestamps (when context was assembled)
        if block.is_history {
            continue;
        }

        let effective = effective_timestamp(block, span_timestamps);
        let identity = MessageIdentity::from_block(block);

        // Debug: log tool block registration
        if block.is_tool_use() || block.is_tool_result() {
            tracing::trace!(
                entry_type = %block.entry_type,
                span_id = %block.span_id,
                uses_span_end = block.uses_span_end,
                is_history = block.is_history,
                event_time = %block.timestamp,
                effective_time = %effective,
                tool_name = ?block.tool_name,
                "build_birth_times: registering tool block"
            );
        }

        match identity {
            MessageIdentity::ToolCall {
                ref trace_id,
                content_hash,
            } => {
                // Record tool call timestamp (earliest occurrence)
                map.record_tool_call(trace_id, content_hash, effective);
            }
            MessageIdentity::ToolResult {
                ref trace_id,
                identity_hash,
            } => {
                // Record tool result timestamp (earliest occurrence)
                map.record_tool_result(trace_id, identity_hash, effective);
            }
            MessageIdentity::Regular {
                ref trace_id,
                role,
                semantic_hash,
            } => {
                // Record regular message timestamp (earliest occurrence)
                map.record_regular(trace_id, role, semantic_hash, effective);
            }
        }
    }

    map
}

/// Get birth time for a block.
fn get_birth_time(
    block: &BlockEntry,
    birth_map: &BirthTimeMap,
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> DateTime<Utc> {
    let effective = effective_timestamp(block, span_timestamps);
    let identity = MessageIdentity::from_block(block);

    match identity {
        MessageIdentity::ToolCall {
            ref trace_id,
            content_hash,
        } => {
            // Tool calls: look up birth time
            birth_map
                .get_tool_call(trace_id, content_hash)
                .unwrap_or(effective)
        }
        MessageIdentity::ToolResult {
            ref trace_id,
            identity_hash,
        } => {
            // Tool results: look up birth time
            birth_map
                .get_tool_result(trace_id, identity_hash)
                .unwrap_or(effective)
        }
        MessageIdentity::Regular {
            ref trace_id,
            role,
            semantic_hash,
        } => {
            // Look up birth time
            birth_map
                .get_regular(trace_id, role, semantic_hash)
                .unwrap_or(effective)
        }
    }
}

// ============================================================================
// QUALITY SCORING
// ============================================================================

/// Quality score weights for deduplication.
///
/// Higher scores = preferred version when deduplicating identical content.
/// Weights are ordered by importance (non-history >> finish_reason >> enrichment >> source >> model).
mod quality {
    /// Non-history blocks are strongly preferred over history copies.
    pub const NON_HISTORY: u32 = 100;
    /// Complete responses (with finish_reason) preferred over streaming chunks.
    pub const HAS_FINISH_REASON: u32 = 10;
    /// Enrichment content (thinking blocks) adds value.
    pub const IS_ENRICHMENT: u32 = 5;
    /// Output-source blocks preferred over input-source copies.
    pub const IS_OUTPUT_SOURCE: u32 = 4;
    /// Tool result from actual tool execution span (not re-sent in generation context).
    pub const FROM_TOOL_SPAN: u32 = 3;
    /// Event source preferred over attribute source (more structured).
    pub const FROM_EVENT: u32 = 2;
    /// Having model info is a minor quality signal.
    pub const HAS_MODEL: u32 = 1;
}

/// Compute quality score for a block.
///
/// Higher score = more complete/enriched version.
/// When deduplicating, keep the highest quality version.
fn compute_quality(block: &BlockEntry) -> u32 {
    let mut score = 0u32;

    // Strong preference for non-history blocks
    // History blocks only win if there's no non-history equivalent
    if !block.is_history {
        score += quality::NON_HISTORY;
    }

    // Prefer blocks with finish_reason (complete response)
    if block.finish_reason.is_some() {
        score += quality::HAS_FINISH_REASON;
    }

    // Prefer enrichment content (thinking blocks)
    if block.content.is_enrichment() {
        score += quality::IS_ENRICHMENT;
    }

    // Prefer output-source blocks over input-source copies
    if block.is_output_source() {
        score += quality::IS_OUTPUT_SOURCE;
    }

    // Tool results from tool spans are the actual execution output, not context re-sends
    if block.is_tool_result() && block.observation_type.as_deref() == Some("tool") {
        score += quality::FROM_TOOL_SPAN;
    }

    // Prefer event source over attribute source
    if block.is_from_event() {
        score += quality::FROM_EVENT;
    }

    // Prefer blocks with model info
    if block.model.is_some() {
        score += quality::HAS_MODEL;
    }

    score
}

// ============================================================================
// DEDUPLICATION
// ============================================================================

/// Identity of the message a block came from: one message of one span.
///
/// Blocks sharing it are the content blocks of a single message - the text and tool calls of one
/// `gen_ai.choice`, say - so they keep their source order and move through the feed as a unit.
///
/// Direction is part of the key, not just the span and timestamp. Attribute extraction gives every
/// message of a span the span's start time, so keying on time alone made a span's input and its
/// output one unit: `input.value` and `output.value` were treated as one response, the earlier time
/// was materialised onto both, and the completed output was reported as having happened when the
/// span started - which a time window could then drop.
///
/// Splitting further, by message index, is wrong in the other direction: a turn's introductory text
/// and the tool calls it introduces arrive as separate messages of one span, and separating them let
/// the text sort after the results by its span-end timestamp. What a span sent and what it produced
/// are different things; within one direction, blocks sharing a timestamp are one response.
fn batch_key(block: &BlockEntry) -> (String, String, DateTime<Utc>, bool) {
    (
        block.trace_id.clone(),
        block.span_id.clone(),
        block.timestamp,
        block.is_output_source(),
    )
}

/// The earliest birth time in each response, which is the time all of its blocks sort at.
fn batch_times(
    paired: &[(DateTime<Utc>, BlockEntry)],
) -> HashMap<(String, String, DateTime<Utc>, bool), DateTime<Utc>> {
    let mut times: HashMap<(String, String, DateTime<Utc>, bool), DateTime<Utc>> = HashMap::new();
    for (birth, block) in paired {
        times
            .entry(batch_key(block))
            .and_modify(|earliest| {
                if birth < earliest {
                    *earliest = *birth;
                }
            })
            .or_insert(*birth);
    }
    times
}

/// Give a tool result the position of the call it answers, where its own position says nothing.
///
/// A message index and an entry index are positions *within a span*; between spans they are not
/// comparable. Two blocks in different spans that report the same instant were therefore ordered by
/// whichever happened to have the smaller index, and a tool result could come back before its call.
///
/// Only that case is adjusted: same batch time, different spans. A same-span pair already orders
/// correctly by index, and taking the call's position there would break the two shapes the ordering
/// note describes - ADK's `user, call, result, call` in one span, and Vercel's parallel
/// `call, call, result, result`, which would interleave.
///
/// This runs before sorting and derives each key from the block and its own call, never from the
/// pair being compared, so it cannot introduce the cycle that a role-ranking comparator did.
/// Where a block sorts within its response: which span it belongs to and where in it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct FeedPosition {
    pub span: (String, String),
    pub message_index: i32,
    pub entry_index: i32,
    /// Set on a tool result that took the position of the call it answers, so it follows that call
    /// rather than tying with it.
    pub after_call: bool,
}

/// The position each block sorts at, with cross-span tool results moved behind their calls.
///
/// `order_time` is the time a block sorts at - its response's anchor. Only a *tie* there can reach a
/// position comparison, which is why adoption is conditioned on it: two blocks anchored differently
/// are already ordered by time. Every view sorts by a position, so every view takes it from here -
/// the project feed sorts its own way (newest first) and would otherwise order a cross-span tie by
/// span id, which is the same arbitrary answer the trace views used to give.
pub(super) fn feed_positions(
    blocks: &[BlockEntry],
    order_time: impl Fn(usize) -> DateTime<Utc>,
) -> Vec<FeedPosition> {
    let mut positions: Vec<FeedPosition> = blocks
        .iter()
        .map(|block| FeedPosition {
            span: (block.trace_id.clone(), block.span_id.clone()),
            message_index: block.message_index,
            entry_index: block.entry_index,
            after_call: false,
        })
        .collect();

    // Keyed by trace and call id. Two calls in one trace share an id only where a framework reuses
    // it, in which case the first is the one a result answers.
    let mut calls: HashMap<(String, String), (DateTime<Utc>, FeedPosition)> = HashMap::new();
    for (i, block) in blocks.iter().enumerate() {
        if block.entry_type != "tool_use" {
            continue;
        }
        if let Some(id) = block.tool_use_id.as_ref().filter(|s| !s.is_empty()) {
            calls
                .entry((block.trace_id.clone(), id.clone()))
                .or_insert((order_time(i), positions[i].clone()));
        }
    }
    if calls.is_empty() {
        return positions;
    }

    for (i, block) in blocks.iter().enumerate() {
        if block.entry_type != "tool_result" {
            continue;
        }
        let Some(id) = block.tool_use_id.as_ref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some((call_time, call_position)) = calls.get(&(block.trace_id.clone(), id.clone()))
        else {
            continue;
        };
        // Tied in time, different span: only then does this block's own index order nothing.
        if *call_time != order_time(i) || call_position.span == positions[i].span {
            continue;
        }
        positions[i] = FeedPosition {
            after_call: true,
            ..call_position.clone()
        };
    }
    positions
}

/// The sort key of one block, built once.
struct BlockSortKey {
    batch_time: DateTime<Utc>,
    span: (String, String),
    message_index: i32,
    entry_index: i32,
    /// Set on a tool result that took its position from the call it answers, so it sorts
    /// immediately after that call rather than tying with it.
    after_call: bool,
    content_hash: String,
}

impl BlockSortKey {
    /// Compare two blocks. Every term is a value carried on the key, which is what makes this a
    /// total order.
    ///
    /// Two rules have to hold at once: within one response, blocks keep the order the framework
    /// emitted them in (ADK puts a whole multi-turn conversation in one span at one timestamp, in
    /// conversation order), and across responses at the same time, a tool call precedes the result
    /// that answers it. Deciding per pair - position for a same-span pair, role otherwise - looks
    /// reasonable and is cyclic: intro text A and call B in one span give A < B by position, B < C
    /// by role against a result C in another span, and C < A by role. `sort_by` may then panic or
    /// return anything.
    ///
    /// Position decides after time, and role does not enter into it. Ranking by role was tried
    /// three ways and each broke a framework, because the three shapes want different things from a
    /// tie and only one order can be total:
    ///
    /// - per pair (position within a span, role across spans) is cyclic, which is the bug this key
    ///   exists to remove;
    /// - per response, ranking a span's input messages against its output messages, merges ADK's
    ///   turns: one span holds `user, call, result, call` in conversation order and comes back as
    ///   `user, result, call, call`;
    /// - per span merges Vercel's parallel calls with their results, turning `call, call, result,
    ///   result` into `call, result, call, result`.
    ///
    /// An index is not comparable across spans, so two blocks in different spans reporting the same
    /// instant would be ordered by a number that means nothing between them - and a tool result
    /// could precede the call it answers. That one case is settled before sorting instead, by
    /// [`feed_positions`]: such a result takes its position *from* its call, which is a
    /// property of the block, so the key stays a set of values and the order stays total. Ranking by
    /// role at comparison time is what could not work.
    fn compare(&self, other: &Self) -> std::cmp::Ordering {
        self.batch_time
            .cmp(&other.batch_time)
            .then_with(|| self.message_index.cmp(&other.message_index))
            .then_with(|| self.entry_index.cmp(&other.entry_index))
            .then_with(|| self.span.cmp(&other.span))
            .then_with(|| self.after_call.cmp(&other.after_call))
            .then_with(|| self.content_hash.cmp(&other.content_hash))
    }
}

/// Deduplicate blocks by identity, keeping highest quality version.
///
/// Note: Birth time is computed during sorting, not here. Deduplication only
/// needs identity and quality scoring.
///
/// Tool results use `tool_use_id` as identity when present, which naturally
/// handles content transformations (e.g., Vercel AI SDK's `toModelOutput`).
/// History re-sends with regenerated IDs are handled upstream by content-hash duplicate detection in
/// `history.rs`.
/// Map an *id-less* tool result onto the identity of the id-bearing result it is a copy of.
///
/// Two signals both mean "the same result", and neither subsumes the other:
///
///   same id, different content   - Vercel's `toModelOutput` rewrites a result and keeps its id
///   no id, same content          - a framework re-sends a past result without the provider's id
///
/// Keying identity on "the id when present, else the content" made those two copies two identities, so
/// they never deduped against each other: ADK's session view showed each forecast twice, once with an
/// id and once without, and the only thing suppressing that was the cross-trace prefix scan - which
/// consumes a *sequence*, so any ordering change resurrected the duplicates.
///
/// The rule is deliberately asymmetric, because the symmetric closure is unsound. Unioning on content
/// alone collapsed three `generate_image` results that carried three distinct ids and happened to
/// return identical bytes: a provider issues one id per result, so two ids are two results, whatever
/// their content. So an id *always* decides, and content only speaks for a result that has no id -
/// and then only when it names exactly one id-bearing result. Content matching several is ambiguous,
/// and ambiguity leaves the block alone, for the same reason an ambiguous call id adds no edge.
///
/// Scoped to one `(trace, ordinal)`: the ordinal is what keeps a model's two genuinely identical calls
/// - and their two identical results - apart, so matching across it would undo that.
fn tool_result_aliases(blocks: &[(usize, BlockEntry, u32)]) -> HashMap<DedupKey, MessageIdentity> {
    /// A result of one (trace, ordinal), by content: which identities carry an id, and which do not.
    type Group = (String, u32, u64);

    let mut with_id: HashMap<Group, Vec<(String, MessageIdentity)>> = HashMap::new();
    let mut without_id: HashMap<Group, Vec<MessageIdentity>> = HashMap::new();
    for (_, block, ordinal) in blocks {
        let ContentBlock::ToolResult {
            tool_use_id,
            name,
            content,
            is_error,
        } = &block.content
        else {
            continue;
        };
        let group = (
            block.trace_id.clone(),
            *ordinal,
            compute_tool_result_hash(name.as_deref(), *is_error, content),
        );
        let identity = MessageIdentity::from_block(block);
        match tool_use_id.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => with_id
                .entry(group)
                .or_default()
                .push((id.to_string(), identity)),
            None => without_id.entry(group).or_default().push(identity),
        }
    }

    let mut aliases: HashMap<DedupKey, MessageIdentity> = HashMap::new();
    for (group, idless) in without_id {
        let Some(candidates) = with_id.get(&group) else {
            continue;
        };
        // Exactly one id-bearing result of this content, or the copy names nothing in particular.
        let mut ids: Vec<&str> = candidates.iter().map(|(id, _)| id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != 1 {
            continue;
        }
        let canonical = candidates[0].1.clone();
        for identity in idless {
            aliases.insert((identity, group.1), canonical.clone());
        }
    }
    aliases
}

// Test-only: prefer the *later* of two tied copies instead of the earlier.
//
// Which copy survives a quality tie is decided by arrival order, and it is not something a caller can
// vary - reversing the row order does not reach it, because rows are re-sorted by timestamp before
// dedup ever sees them. So the property "the *order* of the answer does not depend on which copy
// survived" had no test, which is the central claim of the ordering redesign.
//
// Flipping this changes which copy survives, and content differs between copies - so the test asserts
// the shape is unchanged *and* that the content moved somewhere, since a perturbation that reaches
// nothing proves nothing.
//
// Thread-local, not a global: the suite runs in parallel, and a process-wide flag changed what every
// other test was measuring at the same time. The read path runs on its caller's thread, so a
// thread-local reaches exactly the pipeline under test.
#[cfg(any(test, feature = "test-support"))]
thread_local! {
    #[doc(hidden)]
    pub static PREFER_LATER_ON_TIE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(any(test, feature = "test-support"))]
fn prefer_later_on_tie() -> bool {
    PREFER_LATER_ON_TIE.with(|flag| flag.get())
}

#[cfg(not(any(test, feature = "test-support")))]
fn prefer_later_on_tie() -> bool {
    false
}

/// The key an observation was collapsed under: its identity and the rank of its call within its
/// response. Two identical tool calls of one response differ only in the rank.
pub(super) type DedupKey = (MessageIdentity, u32);

/// Deduplicate, and say which key each input block and each survivor was collapsed under.
///
/// The keys are what a caller needs to trace an observation to the survivor it became. Recomputing an
/// identity afterwards does not work: the rank is a property of the whole pre-dedup list, and
/// `withdraw_unbacked_ids` later clears a correlated result's id, which changes its identity outright.
fn deduplicate_with_lineage(
    blocks: Vec<BlockEntry>,
) -> (Vec<BlockEntry>, Vec<Option<DedupKey>>, Vec<DedupKey>) {
    use std::collections::HashSet;

    let input_count = blocks.len();
    let input_text_count = blocks.iter().filter(|b| b.entry_type == "text").count();

    // Identity carries the rank of a call within its response - see `call_repeat_ordinals`. Without
    // it, a model asking for the same thing twice in one response came back as one call.
    let ordinals = call_repeat_ordinals(&blocks);

    // First: collect identities of non-history blocks
    // History-only messages (no current-turn equivalent) will be filtered out
    let non_history_ids: HashSet<(MessageIdentity, u32)> = blocks
        .iter()
        .zip(&ordinals)
        .filter(|(b, _)| !b.is_history)
        .map(|(b, ordinal)| (MessageIdentity::from_block(b), *ordinal))
        .collect();

    // Filter: keep non-history blocks, and history blocks only if they have a non-history equivalent
    // This removes messages from previous turns that appear in history
    //
    // The input index rides along so each observation can be told which key it ended up under; a
    // block dropped here has no key, which is what `None` in the lineage means.
    let mut input_keys: Vec<Option<DedupKey>> = vec![None; input_count];
    let blocks: Vec<(usize, BlockEntry, u32)> = blocks
        .into_iter()
        .zip(ordinals)
        .enumerate()
        .filter(|(_, (b, ordinal))| {
            if b.is_history {
                // Keep history only if there's a non-history version to dedupe with
                non_history_ids.contains(&(MessageIdentity::from_block(b), *ordinal))
            } else {
                true
            }
        })
        .map(|(i, (b, ordinal))| (i, b, ordinal))
        .collect();

    let after_history_filter = blocks.len();

    // A tool result's identity is the equivalence *closure* of two signals, because both are real and
    // keying on either alone loses the other:
    //
    //   same id, different content   - Vercel's `toModelOutput` rewrites a result and keeps its id
    //   no id, same content         - a framework re-sends a past result without the provider's id
    //
    // Keying on "id when present, else content" made those two copies two identities, so they never
    // deduped against each other: ADK's session view showed each forecast twice, once with an id and
    // once without, and the only thing suppressing that was the cross-trace prefix scan - which
    // consumes a *sequence*, so any ordering change resurrected the duplicates. Identity has to close
    // over both signals for the collapse to be order-independent.
    //
    // Scoped to one `(trace, ordinal)`: the ordinal is what keeps a model's two genuinely identical
    // calls - and their two identical results - apart, so unioning across it would undo that.
    let result_alias = tool_result_aliases(&blocks);

    // Identity-based dedup: non-history will win due to quality scoring.
    let mut candidates: HashMap<DedupKey, (BlockEntry, u32)> = HashMap::new();

    for (input_index, block, ordinal) in blocks {
        let identity = match result_alias.get(&(MessageIdentity::from_block(&block), ordinal)) {
            Some(canonical) => (canonical.clone(), ordinal),
            None => (MessageIdentity::from_block(&block), ordinal),
        };
        let quality = compute_quality(&block);
        input_keys[input_index] = Some(identity.clone());

        candidates
            .entry(identity)
            .and_modify(|(existing, existing_quality)| {
                let wins = if prefer_later_on_tie() {
                    quality >= *existing_quality
                } else {
                    quality > *existing_quality
                };
                if wins {
                    *existing = block.clone();
                    *existing_quality = quality;
                }
            })
            .or_insert((block, quality));
    }

    // Survivors keep their key alongside them, so the sort below permutes both together.
    let mut result: Vec<(BlockEntry, DedupKey)> = candidates
        .into_iter()
        .map(|(key, (block, _))| (block, key))
        .collect();

    // HashMap iteration order is arbitrary, and the feed sort downstream has ties it
    // cannot break (same span, same timestamp, same indices). Leaving those ties to hash
    // order lets two identical requests return the message list in different orders.
    // Anchor it here so the pipeline is a pure function of its input.
    result.sort_by(|(a, _), (b, _)| {
        a.trace_id
            .cmp(&b.trace_id)
            .then_with(|| a.span_id.cmp(&b.span_id))
            .then_with(|| a.message_index.cmp(&b.message_index))
            .then_with(|| a.entry_index.cmp(&b.entry_index))
            .then_with(|| a.entry_type.cmp(&b.entry_type))
            // Role next: two blocks can share span, indices and entry_type and still be
            // distinct messages, and without this the tie falls back to hash order.
            .then_with(|| a.role.as_str().cmp(b.role.as_str()))
            // Content last, so the comparator is TOTAL. Everything above can be equal for two
            // blocks that differ only in content - a span that emits several parts under one
            // message/entry index - and those were left in HashMap order, so two identical
            // requests returned the same messages in a different order. Observed directly:
            // repeated runs over one fixture disagreed on 3 then 4 views.
            .then_with(|| a.content_hash.cmp(&b.content_hash))
    });

    tracing::trace!(
        input = input_count,
        input_text = input_text_count,
        non_history_ids = non_history_ids.len(),
        after_history_filter,
        output = result.len(),
        output_text = result
            .iter()
            .filter(|(b, _)| b.entry_type == "text")
            .count(),
        "deduplicate_blocks: complete"
    );

    let (survivors, survivor_keys): (Vec<BlockEntry>, Vec<DedupKey>) = result.into_iter().unzip();
    (survivors, input_keys, survivor_keys)
}

// ============================================================================
// PUBLIC API
// ============================================================================

/// The ordered survivors, discarding the lineage.
///
/// Production wants the lineage - the order resolver reads it - so this remains for the tests that
/// only assert on the blocks. See [`process_dedup_with_lineage`] for the pipeline itself.
#[cfg(test)]
pub fn process_dedup(
    blocks: Vec<BlockEntry>,
    span_timestamps: HashMap<String, SpanTimestamps>,
) -> Vec<BlockEntry> {
    process_dedup_with_lineage(blocks, span_timestamps).0
}

/// Deduplicate and order, and say for each input observation which surviving block it became.
///
/// # Pipeline
///
/// 1. Deduplicate by identity (keep highest quality)
/// 2. Compute birth times for all blocks
/// 3. Pre-compute birth times into Vec (O(n) — avoids O(n log n) recomputation in sort)
/// 4. Sort by birth time + semantic order (using pre-computed times)
/// 5. Materialize birth times into block timestamps for API clients
///
/// `lineage[i]` is the index in the returned list of the survivor observation `i` was collapsed onto,
/// or `None` where the observation was dropped as history-only. The order resolver needs this to
/// project evidence: it cannot recompute the mapping, because the dedup key carries a call's rank
/// within the whole pre-dedup list, and two identical calls of one response share everything else.
pub fn process_dedup_with_lineage(
    blocks: Vec<BlockEntry>,
    span_timestamps: HashMap<String, SpanTimestamps>,
) -> (Vec<BlockEntry>, Vec<Option<usize>>) {
    if blocks.is_empty() {
        return (blocks, Vec::new());
    }

    // Deduplicate by identity (keeps highest quality version)
    let (deduped, input_keys, survivor_keys) = deduplicate_with_lineage(blocks);

    // Build birth time map (after dedup, from deduped blocks)
    let birth_map = build_birth_times(&deduped, &span_timestamps);

    // Pre-compute birth times once (O(n)) — avoids O(n log n) identity
    // recomputation (String clones + hashing) during sort comparisons.
    let birth_times: Vec<DateTime<Utc>> = deduped
        .iter()
        .map(|b| get_birth_time(b, &birth_map, &span_timestamps))
        .collect();

    // Debug: log birth times for tool blocks
    if tracing::enabled!(tracing::Level::TRACE) {
        for (i, block) in deduped.iter().enumerate() {
            if block.is_tool_use() || block.is_tool_result() {
                let effective = effective_timestamp(block, &span_timestamps);
                tracing::trace!(
                    entry_type = %block.entry_type,
                    span_id = %block.span_id,
                    uses_span_end = block.uses_span_end,
                    event_time = %block.timestamp,
                    effective_time = %effective,
                    birth_time = %birth_times[i],
                    "process_dedup: tool block"
                );
            }
        }
    }

    // Paired with their birth times so a sort keeps the two together.
    let paired: Vec<(DateTime<Utc>, BlockEntry)> = birth_times.into_iter().zip(deduped).collect();
    // Where each deduped block started, so the lineage can follow it through the sort below.
    let dedup_order: Vec<usize> = (0..paired.len()).collect();

    // Sorted by an explicit key, not by a comparator with a special case.
    //
    // Two intents: responses are ordered in time, and within one response blocks keep their source
    // order. As a comparator - source order for a same-batch pair, birth time otherwise - those
    // contradict each other, because one response's blocks have different birth times (text uses
    // span_end, tool_use uses event_time). A third block timestamped between them closes a cycle:
    // text < tool by source order, tool < third and third < text by time. `sort_by` requires a
    // total order and may panic or return anything without one.
    //
    // Giving each response one time - the earliest birth time among its blocks - makes both intents
    // hold at once: responses sort by that time, blocks inside one sort by position, and every
    // comparison follows from the key.
    // Keyed once per block, not per comparison: `batch_key` allocates, and a comparator that
    // builds it on both sides does so O(n log n) times on a feed that can hold thousands of blocks.
    let times = batch_times(&paired);
    let mut keyed: Vec<(BlockSortKey, DateTime<Utc>, BlockEntry, usize)> = paired
        .into_iter()
        .zip(dedup_order)
        .map(|((birth, block), dedup_index)| {
            let span = (block.trace_id.clone(), block.span_id.clone());
            let key = BlockSortKey {
                batch_time: times.get(&batch_key(&block)).copied().unwrap_or(birth),
                span,
                message_index: block.message_index,
                entry_index: block.entry_index,
                after_call: false,
                content_hash: block.content_hash.clone(),
            };
            (key, birth, block, dedup_index)
        })
        .collect();
    // Positions come from the shared helper, so this view and the project feed agree on where a
    // cross-span tool result sits.
    let blocks_for_positions: Vec<BlockEntry> =
        keyed.iter().map(|(_, _, block, _)| block.clone()).collect();
    let batch_times_by_index: Vec<DateTime<Utc>> =
        keyed.iter().map(|(key, _, _, _)| key.batch_time).collect();
    let positions = feed_positions(&blocks_for_positions, |i| batch_times_by_index[i]);
    for (key, position) in keyed.iter_mut().zip(positions) {
        key.0.span = position.span;
        key.0.message_index = position.message_index;
        key.0.entry_index = position.entry_index;
        key.0.after_call = position.after_call;
    }
    keyed.sort_by(|(a, _, _, _), (b, _, _, _)| a.compare(b));

    // The response's time, not each block's own birth time, is what gets materialised - see the
    // note at the end of this function.
    // The sort's permutation, as a map from the deduped index to the final one.
    let mut final_of_dedup: Vec<usize> = vec![0; keyed.len()];
    for (final_index, (_, _, _, dedup_index)) in keyed.iter().enumerate() {
        final_of_dedup[*dedup_index] = final_index;
    }
    let final_of_key: HashMap<&DedupKey, usize> = survivor_keys
        .iter()
        .enumerate()
        .map(|(dedup_index, key)| (key, final_of_dedup[dedup_index]))
        .collect();
    let lineage: Vec<Option<usize>> = input_keys
        .iter()
        .map(|key| key.as_ref().and_then(|k| final_of_key.get(k).copied()))
        .collect();

    let paired: Vec<(DateTime<Utc>, BlockEntry)> = keyed
        .into_iter()
        .map(|(key, _birth, block, _)| (key.batch_time, block))
        .collect();

    // Materialize computed timestamps for API clients.
    //
    // Raw event timestamps can be misleading - an attribute-sourced message inherits its span's
    // start time rather than the moment it was produced - so what is reported is the time the
    // message sorted at.
    //
    // That is the *response's* time, shared by every block of one response, not each block's own
    // birth time. Materialising individual birth times split a response back apart downstream:
    // `process_feed` recognises a response by its blocks sharing a timestamp, so a response whose
    // text was timestamped at span end and whose tool call was timestamped at event time stopped
    // being one response, and a tool result timestamped between them was returned *before the call
    // it answers*. One response, one time.
    let blocks = paired
        .into_iter()
        .map(|(batch_time, mut block)| {
            // Two fields, one value: what the block sorts at, and what it reports. Equal today, so
            // this is not a behaviour change - but the project feed now sorts by the first rather
            // than reading the ordering time back out of the second.
            block.order_time = batch_time;
            block.timestamp = batch_time;
            block
        })
        .collect();
    (blocks, lineage)
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
#[path = "dedup_tests.rs"]
mod tests;
