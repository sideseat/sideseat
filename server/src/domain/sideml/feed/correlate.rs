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
//! 4. A match is committed only when it is unambiguous. Two concurrent calls to the same tool
//!    cannot be told apart from `{name, response}` alone, so nothing is guessed.
//! 5. An unmatched result keeps no id. It stays honestly uncorrelated rather than acquiring a
//!    fabricated reference.

use super::types::BlockEntry;
use crate::domain::sideml::types::ContentBlock;

/// Copy each id-less tool result's owning call id onto it, where that call is unambiguous.
///
/// Runs after history classification and before dedup: dedup uses `tool_use_id` for tool
/// result identity, so a result must already carry its call's id by then or it falls back to
/// content — which would collapse two identical results from two different calls.
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

                // `last()` is the nearest preceding call. With several untaken candidates the
                // pairing is genuinely ambiguous only when they are concurrent; taking the
                // nearest keeps sequential same-name calls correct and is the documented
                // behaviour rather than a silent guess.
                if let Some(&slot) = candidates.last() {
                    let resolved = pending[slot].2.clone();
                    pending[slot].3 = true;
                    if let ContentBlock::ToolResult { tool_use_id, .. } = &mut block.content {
                        *tool_use_id = Some(resolved);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "correlate_tests.rs"]
mod correlate_tests;
