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

mod identity;
mod timing;

pub(super) use identity::*;
pub use timing::{SpanTimestamps, effective_timestamp};
use timing::{build_birth_times, get_birth_time};

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

/// A response: the batch a block belongs to, and which response of that batch.
type ResponseKey = ((String, String, DateTime<Utc>, bool), usize);

/// The response each block belongs to, in the order of `blocks`.
///
/// One output carrier can hold several responses. A client that runs the tool loop itself reports
/// every round of it on one span - the call, then the answer written after the call's result - and
/// all of them carry the carrier's one timestamp. A message with a finish reason ends a response, so
/// the output messages after it start the next one, and a tool result can sit between the two.
/// Inputs are left whole: what a span was sent is one request, whatever finish reasons it repeats; so
/// is an agent span's output, which re-lists a turn rather than reporting model rounds.
fn response_keys<'a>(blocks: impl Iterator<Item = &'a BlockEntry> + Clone) -> Vec<ResponseKey> {
    let mut finished: HashMap<
        (String, String, DateTime<Utc>, bool),
        std::collections::BTreeSet<i32>,
    > = HashMap::new();
    for block in blocks.clone() {
        if block.is_output_source() && block.is_generation_span() && block.finish_reason.is_some() {
            finished
                .entry(batch_key(block))
                .or_default()
                .insert(block.message_index);
        }
    }
    blocks
        .map(|block| {
            let batch = batch_key(block);
            let ordinal = finished
                .get(&batch)
                .map_or(0, |ends| ends.range(..block.message_index).count());
            (batch, ordinal)
        })
        .collect()
}

/// The earliest birth time in each response, which is the time all of its blocks sort at.
fn batch_times(
    paired: &[(DateTime<Utc>, BlockEntry)],
    responses: &[ResponseKey],
) -> HashMap<ResponseKey, DateTime<Utc>> {
    let mut times: HashMap<ResponseKey, DateTime<Utc>> = HashMap::new();
    for ((birth, _), response) in paired.iter().zip(responses) {
        times
            .entry(response.clone())
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

/// Give a surviving attachment the filename a duplicate kept.
///
/// An attachment is identified by its bytes, so one instrumentation's copy with the filename and
/// another's without it are one block; whichever survives on quality, the name is not lost.
fn adopt_attachment_name(survivor: &mut BlockEntry, other: &BlockEntry) {
    let named = match &other.content {
        ContentBlock::Document {
            name: Some(name), ..
        }
        | ContentBlock::File {
            name: Some(name), ..
        } => name,
        _ => return,
    };
    if let ContentBlock::Document {
        name: name @ None, ..
    }
    | ContentBlock::File {
        name: name @ None, ..
    } = &mut survivor.content
    {
        *name = Some(named.clone());
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
                // An ordered snapshot can prove distinct historical occurrences rather than merely
                // re-listing an undifferentiated state. Atomic emissions also prove occurrences, but
                // if one was already classified as history it is still history and must not survive
                // on position alone.
                (!b.is_cross_trace_history && {
                    let semantics =
                        crate::sideml::carrier::semantics_for_context(&b.carrier_context());
                    semantics.may_restate_prior_observations
                        && semantics.position_proves_distinct_occurrence
                }) || non_history_ids.contains(&(MessageIdentity::from_block(b), *ordinal))
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
                // Equal quality falls to the more original copy, and only then to arrival order:
                // between spans whose clocks agree only to the millisecond, arrival order is the
                // order of their span ids.
                let wins = match quality.cmp(existing_quality) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => {
                        match block.origin_rank().cmp(&existing.origin_rank()) {
                            std::cmp::Ordering::Less => true,
                            std::cmp::Ordering::Greater => false,
                            std::cmp::Ordering::Equal => prefer_later_on_tie(),
                        }
                    }
                };
                let other = if wins {
                    let replaced = std::mem::replace(existing, block.clone());
                    *existing_quality = quality;
                    replaced
                } else {
                    block.clone()
                };
                adopt_attachment_name(existing, &other);
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
#[cfg(test)]
pub fn process_dedup_with_lineage(
    blocks: Vec<BlockEntry>,
    span_timestamps: HashMap<String, SpanTimestamps>,
) -> (Vec<BlockEntry>, Vec<Option<usize>>) {
    let (blocks, lineage, _) = process_dedup_with_lineage_and_ordinals(blocks, span_timestamps);
    (blocks, lineage)
}

/// Deduplicate and order while retaining lineage and each survivor's repeat ordinal.
pub(super) fn process_dedup_with_lineage_and_ordinals(
    blocks: Vec<BlockEntry>,
    span_timestamps: HashMap<String, SpanTimestamps>,
) -> (Vec<BlockEntry>, Vec<Option<usize>>, Vec<u32>) {
    if blocks.is_empty() {
        return (blocks, Vec::new(), Vec::new());
    }

    let input_occurrences: Vec<(DateTime<Utc>, bool, bool)> = blocks
        .iter()
        .map(|block| {
            (
                effective_timestamp(block, &span_timestamps),
                block.is_history,
                !matches!(
                    &block.content,
                    ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                ),
            )
        })
        .collect();

    // Deduplicate by identity (keeps highest quality version)
    let (deduped, input_keys, survivor_keys) = deduplicate_with_lineage(blocks);
    let survivor_occurrences: Vec<(DateTime<Utc>, bool)> = deduped
        .iter()
        .map(|block| {
            (
                effective_timestamp(block, &span_timestamps),
                block.is_history,
            )
        })
        .collect();

    // Build birth time map (after dedup, from deduped blocks)
    let survivor_ordinals: Vec<u32> = survivor_keys.iter().map(|key| key.1).collect();
    let birth_map = build_birth_times(&deduped, &survivor_ordinals, &span_timestamps);

    // Pre-compute birth times once (O(n)) — avoids O(n log n) identity
    // recomputation (String clones + hashing) during sort comparisons.
    let birth_times: Vec<DateTime<Utc>> = deduped
        .iter()
        .zip(&survivor_ordinals)
        .map(|(b, ordinal)| get_birth_time(b, *ordinal, &birth_map, &span_timestamps))
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
    let responses = response_keys(paired.iter().map(|(_, block)| block));
    let times = batch_times(&paired, &responses);
    let mut keyed: Vec<(BlockSortKey, DateTime<Utc>, BlockEntry, usize)> = paired
        .into_iter()
        .zip(dedup_order)
        .zip(responses)
        .map(|(((birth, block), dedup_index), response)| {
            let span = (block.trace_id.clone(), block.span_id.clone());
            let key = BlockSortKey {
                batch_time: times.get(&response).copied().unwrap_or(birth),
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
    let survivor_of_key: HashMap<&DedupKey, (usize, usize)> = survivor_keys
        .iter()
        .enumerate()
        .map(|(dedup_index, key)| (key, (dedup_index, final_of_dedup[dedup_index])))
        .collect();
    let lineage: Vec<Option<usize>> = input_keys
        .iter()
        .zip(input_occurrences)
        .map(|(key, (input_time, input_is_history, input_is_plain))| {
            let (dedup_index, final_index) = key
                .as_ref()
                .and_then(|key| survivor_of_key.get(key))
                .copied()?;
            let (survivor_time, survivor_is_history) = survivor_occurrences[dedup_index];
            // A history observation before a newly produced survivor cannot be a copy of that future
            // occurrence. This happens when consecutive turns have byte-identical answers: mapping
            // the previous turn's replay onto the current output makes dataflow point both ways.
            if input_is_plain
                && input_is_history
                && !survivor_is_history
                && input_time < survivor_time
            {
                None
            } else {
                Some(final_index)
            }
        })
        .collect();
    let mut repeat_ordinals = vec![0; survivor_keys.len()];
    for (dedup_index, key) in survivor_keys.iter().enumerate() {
        repeat_ordinals[final_of_dedup[dedup_index]] = key.1;
    }

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
    (blocks, lineage, repeat_ordinals)
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
#[path = "dedup_tests.rs"]
mod tests;
