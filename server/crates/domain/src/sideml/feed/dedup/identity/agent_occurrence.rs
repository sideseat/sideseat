use super::*;

type Span<'a> = (&'a str, &'a str);
type IdScope<'a> = (&'a str, &'a str, &'a str);
type ShapeScope<'a> = (&'a str, &'a str, u64);

/// Align tool copies with the execution inside their top-level agent invocation.
///
/// Agent spans report their accumulated output at the span start timestamp even though the child
/// generation happens later. Timestamp matching therefore attaches the second agent's copy to the
/// first execution when a provider reuses an id. A unique child execution is stronger evidence:
/// every matching call/result in that agent subtree is a representation of that execution.
pub(super) fn align_tool_ordinals_with_agent_invocations(
    blocks: &[BlockEntry],
    ordinals: &mut [u32],
) {
    let agent_spans: std::collections::HashSet<Span<'_>> = blocks
        .iter()
        .filter(|block| block.is_agent_span())
        .map(|block| (block.trace_id.as_str(), block.span_id.as_str()))
        .collect();
    let top_agent_by_block: Vec<Option<&str>> = blocks
        .iter()
        .map(|block| {
            block
                .span_path
                .iter()
                .find(|span| agent_spans.contains(&(block.trace_id.as_str(), span.as_str())))
                .map(String::as_str)
        })
        .collect();

    let mut ranks_by_id: HashMap<IdScope<'_>, std::collections::HashSet<u32>> = HashMap::new();
    let mut ranks_by_shape: HashMap<ShapeScope<'_>, std::collections::HashSet<u32>> =
        HashMap::new();
    for (index, block) in blocks.iter().enumerate() {
        let ContentBlock::ToolUse {
            id, name, input, ..
        } = &block.content
        else {
            continue;
        };
        if !is_generation_execution_anchor(block) {
            continue;
        }
        let Some(top_agent) = top_agent_by_block[index] else {
            continue;
        };
        let rank = ordinals[index];
        ranks_by_shape
            .entry((
                block.trace_id.as_str(),
                top_agent,
                compute_tool_call_hash(name, input),
            ))
            .or_default()
            .insert(rank);
        if let Some(id) = id.as_deref().filter(|id| !id.is_empty()) {
            ranks_by_id
                .entry((block.trace_id.as_str(), top_agent, id))
                .or_default()
                .insert(rank);
        }
    }

    let unique = |ranks: Option<&std::collections::HashSet<u32>>| {
        ranks
            .filter(|ranks| ranks.len() == 1)
            .and_then(|ranks| ranks.iter().next().copied())
    };
    for (index, block) in blocks.iter().enumerate() {
        let Some(top_agent) = top_agent_by_block[index] else {
            continue;
        };
        let rank = match &block.content {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => id
                .as_deref()
                .filter(|id| !id.is_empty())
                .and_then(|id| unique(ranks_by_id.get(&(block.trace_id.as_str(), top_agent, id))))
                .or_else(|| {
                    unique(ranks_by_shape.get(&(
                        block.trace_id.as_str(),
                        top_agent,
                        compute_tool_call_hash(name, input),
                    )))
                }),
            ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .and_then(|id| unique(ranks_by_id.get(&(block.trace_id.as_str(), top_agent, id)))),
            _ => None,
        };
        if let Some(rank) = rank {
            ordinals[index] = rank;
        }
    }
}
