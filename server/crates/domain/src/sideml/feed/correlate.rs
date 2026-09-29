//! Tool-call/result correlation.
//!
//! Some frameworks identify a tool result only by function name and emit no call id at all —
//! Gemini and Google ADK are the case that motivated this. A result with no id cannot be tied
//! to its call by the UI, the API, or anything downstream.
//!
//! The previous approach was to synthesise an id inside content normalization, hashing the
//! call's arguments for the call and the result's response for the result. Those two hashes
//! are of different things, so the ids never matched: every ADK tool result carried a
//! `tool_use_id` that referenced a call that did not exist. A dangling id is worse than none,
//! because it looks like a working reference.
//!
//! Correlation belongs here rather than in normalization because this is the first point where
//! both halves of a pair are visible at once: normalization sees one block at a time and
//! cannot know which call a result answers.
//!
//! Rules, in order:
//!
//! 1. A result whose id names a visible call keeps it: provider ids win.
//! 2. `opentelemetry-util-genai` synthesises a missing Gemini response id as
//!    `<tool_name>_<part_index>`, independently of the real id on the call. If that synthetic id
//!    is dangling and its encoded name identifies a visible call, it is replaced with that call's
//!    id. An arbitrary unknown provider id is never guessed over.
//! 3. Matching is scoped to one trace. A call in one trace never answers a result in another.
//! 4. An id-less result is matched to a *preceding* unclaimed call with the same tool name.
//! 5. Among several such calls, the oldest unclaimed one answers the next result. `{name,
//!    response}` carries nothing else to go on, and the frameworks that omit result ids - Gemini
//!    and Google ADK, the reason this module exists - emit results in request order. Reversing
//!    this by taking the *nearest* call mis-paired every parallel group. Where a framework returns
//!    results out of order it supplies ids, so it never reaches this rule; a framework that did
//!    both would be mis-paired here, and nothing in the payload would reveal it.
//! 6. An unmatched id-less result keeps no id. It stays honestly uncorrelated rather than
//!    acquiring a fabricated reference.

use super::types::BlockEntry;
use crate::sideml::types::ContentBlock;

/// An outstanding tool call, in the document order rule 4 pairs by.
struct PendingCall {
    id: String,
    taken: bool,
}

/// The outstanding calls, with the two lookups the rules need.
///
/// The rules are unchanged; only their cost is. `claim` used to scan every pending entry and the
/// oldest-untaken search used to scan them again and collect a `Vec` of candidates, so correlation was
/// quadratic in the number of tool calls in the scope - and a session is exactly where that number gets
/// large. Both are now index lookups, and the answers are the same ones by construction:
///
/// - `by_id` holds every slot sharing one `(trace, id)`, which is the set the old scan marked. One call is
///   flattened once per span that carries it, so a call really does appear several times over; marking only
///   the first left the others available, and a later id-less result then adopted an id that had already been
///   answered - two results with one id, which dedup resolves by dropping one of them.
/// - `by_name` holds the slots for one `(trace, tool name)` in ascending document order, so the first
///   *untaken* one from the front is exactly the "oldest unclaimed" the old `candidates.first()` returned.
///   Entries claimed by id are skipped when they reach the front rather than being removed from the middle,
///   which keeps the pass amortised linear.
#[derive(Default)]
struct Outstanding {
    calls: Vec<PendingCall>,
    by_id: std::collections::HashMap<(String, String), Vec<usize>>,
    by_name: std::collections::HashMap<(String, String), std::collections::VecDeque<usize>>,
}

impl Outstanding {
    fn push(&mut self, trace: &str, name: &str, id: &str) {
        let slot = self.calls.len();
        self.calls.push(PendingCall {
            id: id.to_string(),
            taken: false,
        });
        self.by_id
            .entry((trace.to_string(), id.to_string()))
            .or_default()
            .push(slot);
        self.by_name
            .entry((trace.to_string(), name.to_string()))
            .or_default()
            .push_back(slot);
    }

    /// Mark every pending entry for this call id as answered.
    fn claim(&mut self, trace: &str, id: &str) -> bool {
        if let Some(slots) = self.by_id.get(&(trace.to_string(), id.to_string())) {
            for &slot in slots {
                self.calls[slot].taken = true;
            }
            return true;
        }
        false
    }

    /// The id of the oldest unclaimed call for this tool in this trace.
    fn oldest_unclaimed(&mut self, trace: &str, name: &str) -> Option<String> {
        let queue = self
            .by_name
            .get_mut(&(trace.to_string(), name.to_string()))?;
        while let Some(&slot) = queue.front() {
            if self.calls[slot].taken {
                queue.pop_front();
                continue;
            }
            return Some(self.calls[slot].id.clone());
        }
        None
    }
}

/// The fallback id generated by `opentelemetry-util-genai` when Gemini supplies a function name
/// but no response id: `<name>_<part index>`.
///
/// The shape alone is not enough to rewrite anything. The caller also requires a preceding,
/// unclaimed call whose name is exactly the extracted prefix.
fn indexed_fallback_tool_name(id: &str) -> Option<&str> {
    let (name, index) = id.rsplit_once('_')?;
    (!name.is_empty() && !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
        .then_some(name)
}

/// Copy each id-less tool result's owning call id onto it.
///
/// Runs before history classification and dedup, both of which decide what is a duplicate tool
/// result and both of which need the call reference to do it: a result that reaches either
/// without its call's id falls back to content, and two identical results answering two
/// different calls collapse into one. Needs the blocks in source order, which is what they are
/// in straight after flattening.
pub fn correlate_tool_results(blocks: &mut [BlockEntry]) {
    let mut pending = Outstanding::default();
    // OTLP exporters may retry the same span batch. Correlation runs before dedup because dedup needs the
    // resolved id, so an id-less result from a repeated delivery would otherwise consume the *next* pending
    // call of the same name. Remember the answer for one exact source occurrence and reuse it on redelivery.
    // Attribute/event identity plus the position *inside that carrier* is stable across evolving exports,
    // while flattened message and entry indices are not. Content is deliberately absent: one source result
    // can be represented twice (for example a scrubbed raw output and its normalized error text), and those
    // two representations must answer the same call. Distinct array results have distinct source positions.
    let mut resolved_occurrences: std::collections::HashMap<
        (
            String,
            String,
            String,
            crate::sideml::provenance::PositionPath,
        ),
        String,
    > = std::collections::HashMap::new();

    // One forward pass. Blocks are in source order at this stage, so a call always precedes
    // the result it answers.
    for block in blocks.iter_mut() {
        let trace = block.trace_id.clone();
        match &mut block.content {
            ContentBlock::ToolUse { id, name, .. } => {
                if let Some(id) = id.as_ref().filter(|s| !s.is_empty()) {
                    pending.push(&trace, name, id);
                }
            }
            ContentBlock::ToolResult {
                tool_use_id, name, ..
            } => {
                // Rule 1: an id that names a visible call is authoritative.
                //
                // Returning without claiming left the call available, so a later id-less result
                // for the same tool adopted an id that was already answered. Both results then
                // had the same id, and dedup - which identifies a result by its id - dropped one
                // of them whatever their contents. A framework that supplies ids for some
                // results and not others is enough to hit this.
                if let Some(id) = tool_use_id.as_ref().filter(|s| !s.is_empty()).cloned() {
                    if pending.claim(&trace, &id) {
                        continue;
                    }

                    // Rule 2: Google GenAI's current OTel utility gives a FunctionResponse with
                    // no id a synthetic `<name>_<index>` id, even when the preceding FunctionCall
                    // had a real provider id. Repair only that documented fallback shape, and only
                    // when its encoded name proves which visible call it answers.
                    if name.is_none()
                        && let Some(inferred_name) = indexed_fallback_tool_name(&id)
                        && let Some(resolved) = pending.oldest_unclaimed(&trace, inferred_name)
                    {
                        pending.claim(&trace, &resolved);
                        *tool_use_id = Some(resolved.clone());
                        *name = Some(inferred_name.to_string());
                        block.tool_use_id = Some(resolved);
                        block.tool_use_id_correlated = true;
                    }
                    continue;
                }
                let Some(result_name) = name.clone() else {
                    // Rule 6: with no name there is nothing to match on.
                    continue;
                };
                let source_occurrence = block
                    .source_attribute
                    .as_ref()
                    .map(|attribute| format!("attribute:{attribute}"))
                    .or_else(|| {
                        block
                            .event_name
                            .as_ref()
                            .map(|event| format!("event:{event}"))
                    });
                let occurrence = source_occurrence.map(|source| {
                    (
                        trace.clone(),
                        block.span_id.clone(),
                        source,
                        block.position.clone(),
                    )
                });
                if let Some(resolved) = occurrence
                    .as_ref()
                    .and_then(|key| resolved_occurrences.get(key))
                    .cloned()
                {
                    *tool_use_id = Some(resolved.clone());
                    block.tool_use_id = Some(resolved);
                    block.tool_use_id_correlated = true;
                    continue;
                }
                // Rules 3-5: preceding unclaimed calls for the same name in this trace.
                // Rule 5: the OLDEST untaken call with this name, not the nearest.
                //
                // Both Gemini and the OpenAI-shaped protocols emit their tool results in the same
                // order as the calls they answer, so among several outstanding calls to one tool
                // position is the pairing - and it is the only signal `{name, response}` leaves.
                //
                // Taking the nearest (`last()`) reversed every concurrent group: ADK's three
                // parallel `generate_image` calls b06/91f/593 had their results attached
                // 593/91f/b06, so every image in that fixture pointed at the wrong prompt. The
                // reference looked valid, which is why it went unnoticed.
                //
                // Sequential calls are unaffected: the earlier call is already taken by the time
                // the second result arrives, so oldest-untaken is the second call.
                if let Some(resolved) = pending.oldest_unclaimed(&trace, &result_name) {
                    pending.claim(&trace, &resolved);
                    if let Some(occurrence) = occurrence {
                        resolved_occurrences.insert(occurrence, resolved.clone());
                    }
                    *tool_use_id = Some(resolved.clone());
                    // Recorded so history detection can tell a correlated id from a provider's
                    // own: the orphan-result phase reads an unknown id as proof the result is
                    // from a past turn, which is the opposite of what a correlated id means.
                    block.tool_use_id = Some(resolved);
                    block.tool_use_id_correlated = true;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "correlate_tests.rs"]
mod correlate_tests;

/// Clear correlated ids whose call is no longer present, and collapse any results that become
/// indistinguishable as a result.
///
/// Runs after dedup: a result correlated to a call that dedup then dropped would otherwise carry
/// a reference to a block the response does not contain, which is the dangling id this module
/// exists to prevent - just arrived at from the other direction.
///
/// Because it runs after dedup, it can undo the very distinction dedup relied on. Two results
/// with the same text and different call ids are two messages to dedup; withdraw both ids and
/// they become one message reported twice, and dedup has already run. So anything that collapses
/// only after withdrawal is collapsed here, in source order, keeping the first.
/// The withdrawn blocks, discarding the remap. Production composes the remap into a lineage, so this
/// remains for the tests that only assert on the blocks.
#[cfg(test)]
pub fn withdraw_unbacked_ids(blocks: Vec<BlockEntry>) -> Vec<BlockEntry> {
    withdraw_unbacked_ids_with_remap(blocks).0
}

/// Withdraw, and say where each block ended up.
///
/// `remap[i]` is the new index of the block that was at `i`, or `None` where it was collapsed away.
/// This stage both mutates identities (clearing a withdrawn id) and drops blocks, so a caller holding
/// a lineage from before it has to compose this remap onto it - index-based lineage would otherwise
/// silently point at the wrong block from the first dropped duplicate onward.
pub fn withdraw_unbacked_ids_with_remap(
    blocks: Vec<BlockEntry>,
) -> (Vec<BlockEntry>, Vec<Option<usize>>) {
    let mut blocks = blocks;
    let surviving: std::collections::HashSet<(&str, &str)> = blocks
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::ToolUse { id: Some(id), .. } if !id.is_empty() => {
                Some((b.trace_id.as_str(), id.as_str()))
            }
            _ => None,
        })
        .collect();

    let unbacked: Vec<usize> = blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.tool_use_id_correlated)
        .filter(|(_, b)| match &b.content {
            ContentBlock::ToolResult {
                tool_use_id: Some(id),
                ..
            } => !surviving.contains(&(b.trace_id.as_str(), id.as_str())),
            _ => false,
        })
        .map(|(idx, _)| idx)
        .collect();

    if unbacked.is_empty() {
        let remap = (0..blocks.len()).map(Some).collect();
        return (blocks, remap);
    }

    for idx in unbacked {
        let block = &mut blocks[idx];
        if let ContentBlock::ToolResult { tool_use_id, .. } = &mut block.content {
            *tool_use_id = None;
        }
        block.tool_use_id = None;
        block.tool_use_id_correlated = false;
    }

    // Keep the first of any id-less results that are now the same message, whichever of them was
    // the withdrawn one. `content_hash` is the block identity, which for a tool result covers the
    // tool name and the error flag as well as the text - two tools both returning "ok" are two
    // messages.
    //
    // Dropping only the withdrawn block made the outcome depend on their order: a withdrawn result
    // ahead of a natively id-less twin left both in place, while the reverse order dropped one.
    // Position cannot decide which is the duplicate. Two id-less results with identical content
    // are safe to collapse in general, because dedup identifies such a result by content and would
    // already have collapsed any pair that reached it that way - so a surviving pair can only have
    // been created here.
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut keep = Vec::with_capacity(blocks.len());
    let mut remap = Vec::with_capacity(blocks.len());
    for block in blocks {
        let id_less_result = matches!(
            &block.content,
            ContentBlock::ToolResult {
                tool_use_id: None,
                ..
            }
        );
        if id_less_result && !seen.insert((block.trace_id.clone(), block.content_hash.clone())) {
            remap.push(None);
            continue;
        }
        remap.push(Some(keep.len()));
        keep.push(block);
    }
    (keep, remap)
}
