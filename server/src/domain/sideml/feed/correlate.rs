//! Tool call/result correlation.
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
//! 1. A result that already has an id keeps it - real ids from the provider win - but it does
//!    claim the call it names, so a later id-less result cannot adopt a call already answered.
//! 2. Matching is scoped to one trace. A call in one trace never answers a result in another.
//! 3. A result is matched to a *preceding* unclaimed call with the same tool name.
//! 4. Among several such calls, the oldest unclaimed one answers the next result. `{name,
//!    response}` carries nothing else to go on, and the frameworks that omit result ids - Gemini
//!    and Google ADK, the reason this module exists - emit results in request order. Reversing
//!    this by taking the *nearest* call mis-paired every parallel group. Where a framework returns
//!    results out of order it supplies ids, so it never reaches this rule; a framework that did
//!    both would be mis-paired here, and nothing in the payload would reveal it.
//! 5. An unmatched result keeps no id. It stays honestly uncorrelated rather than acquiring a
//!    fabricated reference.

use super::types::BlockEntry;
use crate::domain::sideml::types::ContentBlock;

/// Mark every pending entry for this call id as answered.
///
/// One call is flattened once per span that carries it, so the same call appears in `pending`
/// several times over. Marking only the first left the others available, and a later id-less result
/// then adopted an id that had already been answered - two results with one id, which dedup
/// resolves by dropping one of them.
fn claim(pending: &mut [(String, String, String, bool)], trace: &str, id: &str) {
    for entry in pending.iter_mut() {
        if entry.0 == trace && entry.2 == id {
            entry.3 = true;
        }
    }
}

/// Copy each id-less tool result's owning call id onto it.
///
/// Runs before history classification and dedup, both of which decide what is a duplicate tool
/// result and both of which need the call reference to do it: a result that reaches either
/// without its call's id falls back to content, and two identical results answering two
/// different calls collapse into one. Needs the blocks in source order, which is what they are
/// in straight after flattening.
pub fn correlate_tool_results(blocks: &mut [BlockEntry]) {
    // (trace_id, tool_name) -> call ids in document order, and whether each is taken.
    let mut pending: Vec<(String, String, String, bool)> = Vec::new();

    // One forward pass. Blocks are in source order at this stage, so a call always precedes
    // the result it answers.
    for block in blocks.iter_mut() {
        let trace = block.trace_id.clone();
        match &block.content {
            ContentBlock::ToolUse { id, name, .. } => {
                if let Some(id) = id.as_ref().filter(|s| !s.is_empty()) {
                    pending.push((trace, name.clone(), id.clone(), false));
                }
            }
            ContentBlock::ToolResult {
                tool_use_id, name, ..
            } => {
                // Rule 1: never overwrite a real id - but do claim the call it names.
                //
                // Returning without claiming left the call available, so a later id-less result
                // for the same tool adopted an id that was already answered. Both results then
                // had the same id, and dedup - which identifies a result by its id - dropped one
                // of them whatever their contents. A framework that supplies ids for some
                // results and not others is enough to hit this.
                if let Some(id) = tool_use_id.as_ref().filter(|s| !s.is_empty()) {
                    claim(&mut pending, &trace, id);
                    continue;
                }
                let Some(result_name) = name.clone() else {
                    // Rule 5: with no name there is nothing to match on.
                    continue;
                };
                // Rules 2-4: preceding unclaimed calls for the same name in this trace.
                let candidates: Vec<usize> = pending
                    .iter()
                    .enumerate()
                    .filter(|(_, (t, n, _, taken))| !*taken && *t == trace && *n == result_name)
                    .map(|(idx, _)| idx)
                    .collect();

                // Rule 4: the OLDEST untaken call with this name, not the nearest.
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
                if let Some(&slot) = candidates.first() {
                    let resolved = pending[slot].2.clone();
                    claim(&mut pending, &trace, &resolved);
                    if let ContentBlock::ToolResult { tool_use_id, .. } = &mut block.content {
                        *tool_use_id = Some(resolved.clone());
                    }
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
pub fn withdraw_unbacked_ids(blocks: Vec<BlockEntry>) -> Vec<BlockEntry> {
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
        return blocks;
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
    for block in blocks {
        let id_less_result = matches!(
            &block.content,
            ContentBlock::ToolResult {
                tool_use_id: None,
                ..
            }
        );
        if id_less_result && !seen.insert((block.trace_id.clone(), block.content_hash.clone())) {
            continue;
        }
        keep.push(block);
    }
    keep
}
