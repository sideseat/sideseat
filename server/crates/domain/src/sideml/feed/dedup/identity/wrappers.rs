//! A framework's model-call step around the model call itself: a generation span enclosing another. What the
//! step reports as its response restates a call made below it, so neither plain nor tool ranking counts the
//! step's copy as an execution of its own, and dedup keeps the call's own emission over it.

use super::*;

/// The generation spans that enclose another generation span of their trace: a framework's model-call step
/// around the model call itself. What a wrapper reports as its response restates a call made below it, so
/// neither plain ranking nor tool ranking counts the wrapper's copy as an execution of its own.
pub(in crate::sideml::feed) fn generation_wrappers(
    blocks: &[BlockEntry],
) -> std::collections::HashSet<(&str, &str)> {
    let generation_spans: std::collections::HashSet<(&str, &str)> = blocks
        .iter()
        .filter(|block| block.is_generation_span())
        .map(|block| (block.trace_id.as_str(), block.span_id.as_str()))
        .collect();
    let mut wrappers = std::collections::HashSet::new();
    for block in blocks.iter().filter(|block| block.is_generation_span()) {
        for ancestor in block
            .span_path
            .iter()
            .take(block.span_path.len().saturating_sub(1))
        {
            let span = (block.trace_id.as_str(), ancestor.as_str());
            if generation_spans.contains(&span) {
                wrappers.insert(span);
            }
        }
    }
    wrappers
}

/// The provider's id of a call block, where it carries one.
pub(super) fn call_id(block: &BlockEntry) -> Option<&str> {
    match &block.content {
        ContentBlock::ToolUse { id: Some(id), .. } if !id.is_empty() => Some(id),
        _ => None,
    }
}

/// Whether `wrapper`'s call restates `candidate`, a call a generation below `wrapper`'s span made: the same
/// trace, call id and name, and arguments equal as JSON values - exactly, with no member dropped, so two calls
/// the identity hash would merge stay apart here.
pub(super) fn restates(blocks: &[BlockEntry], wrapper: usize, candidate: usize) -> bool {
    let (outer, inner) = (&blocks[wrapper], &blocks[candidate]);
    let (
        ContentBlock::ToolUse {
            name, input, id, ..
        },
        ContentBlock::ToolUse {
            name: n,
            input: i,
            id: d,
            ..
        },
    ) = (&outer.content, &inner.content)
    else {
        return false;
    };
    candidate != wrapper
        && inner.trace_id == outer.trace_id
        && inner.span_id != outer.span_id
        && inner
            .span_path
            .iter()
            .take(inner.span_path.len().saturating_sub(1))
            .any(|ancestor| *ancestor == outer.span_id)
        && n == name
        && i == input
        && d == id
}

/// Per block: a model call's own emission (a generation whose carrier never restates earlier observations) on a
/// span below a wrapper and not itself one - the copy dedup keeps over the wrapper's restatement of it.
pub(in crate::sideml::feed) fn emitted_below_a_wrapper(blocks: &[BlockEntry]) -> Vec<bool> {
    let wrappers = generation_wrappers(blocks);
    let wraps = |block: &BlockEntry, span: &String| {
        wrappers.contains(&(block.trace_id.as_str(), span.as_str()))
    };
    blocks
        .iter()
        .map(|block| {
            let (ancestors, own) = block
                .span_path
                .split_at(block.span_path.len().saturating_sub(1));
            block.is_generation_span()
                && !crate::sideml::carrier::semantics_for_context(&block.carrier_context())
                    .may_restate_prior_observations
                && !own.iter().any(|span| wraps(block, span))
                && ancestors.iter().any(|span| wraps(block, span))
        })
        .collect()
}
