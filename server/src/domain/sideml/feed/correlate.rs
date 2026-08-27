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
//! 1. A result that already has an id is never touched. Real ids from the provider win.
//! 2. Matching is scoped to one trace. A call in one trace never answers a result in another.
//! 3. A result is matched to the nearest *preceding* unmatched call with the same tool name.
//! 4. Among several outstanding calls to one tool, results are taken in call order: the oldest
//!    unclaimed call answers the next result. `{name, response}` carries nothing else, and both
//!    Gemini and the OpenAI-shaped protocols return results in the order they were requested.
//!    Reversing this - taking the *nearest* call - mis-paired every parallel tool call.
//! 5. An unmatched result keeps no id. It stays honestly uncorrelated rather than acquiring a
//!    fabricated reference.

use super::types::BlockEntry;
use crate::domain::sideml::types::ContentBlock;

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
                // Rule 1: never overwrite a real id.
                if tool_use_id.as_ref().is_some_and(|s| !s.is_empty()) {
                    continue;
                }
                let Some(result_name) = name.clone() else {
                    // Rule 5: with no name there is nothing to match on.
                    continue;
                };
                // Rules 2-4: nearest preceding untaken call for the same name in this trace.
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
                    pending[slot].3 = true;
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

/// Clear correlated ids whose call is no longer present.
///
/// Runs after dedup: a result correlated to a call that dedup then dropped would otherwise carry
/// a reference to a block the response does not contain, which is the dangling id this module
/// exists to prevent - just arrived at from the other direction.
pub fn withdraw_unbacked_ids(blocks: &mut [BlockEntry]) {
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

    for idx in unbacked {
        let block = &mut blocks[idx];
        if let ContentBlock::ToolResult { tool_use_id, .. } = &mut block.content {
            *tool_use_id = None;
        }
        block.tool_use_id = None;
        block.tool_use_id_correlated = false;
    }
}
