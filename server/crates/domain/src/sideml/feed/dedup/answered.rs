//! The calls only a request's history carries, which a current result answers.

use std::collections::HashSet;

use super::*;

/// The history copies of tool calls to keep although nothing current has them, by their place in `blocks`: a
/// current result answers them.
///
/// The call's own response recorded no part of it - a streamed response reported as its role alone - and the
/// next request re-sends it. The result is what the call returned, so the call happened; dropping the re-sent
/// copy as history leaves the result answering nothing. Kept by the call id, within the result's trace, and only
/// where no current copy of that call exists (`current_calls`), so a call a response did report is still folded
/// onto it. By place rather than by identity: a copy with the same content and ordinal that history carried in
/// from another trace is that trace's call, and must stay dropped.
pub(super) fn answered_history_calls(
    blocks: &[BlockEntry],
    current_calls: &HashSet<(&str, &str)>,
) -> HashSet<usize> {
    let current_results: HashSet<(&str, &str)> = blocks
        .iter()
        .filter(|b| !b.is_history)
        .filter_map(|block| match &block.content {
            ContentBlock::ToolResult {
                tool_use_id: Some(id),
                ..
            } if !id.is_empty() => Some((block.trace_id.as_str(), id.as_str())),
            _ => None,
        })
        .collect();
    blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_history && !b.is_cross_trace_history)
        .filter(|(_, block)| match &block.content {
            ContentBlock::ToolUse { id: Some(id), .. } => {
                let call = (block.trace_id.as_str(), id.as_str());
                current_results.contains(&call) && !current_calls.contains(&call)
            }
            _ => false,
        })
        .map(|(index, _)| index)
        .collect()
}
