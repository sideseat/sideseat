use super::*;

type Anchor<'a> = (&'a str, &'a str, u64, u32);
type Span<'a> = (&'a str, &'a str);
type ScopedShape<'a> = (&'a str, &'a str, u64);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum OccurrenceGroup<'a> {
    GenerationOutput(&'a str, u64),
    AgentRequest(&'a str, u64, Option<&'a str>),
}

/// Give repeated plain messages an occurrence rank where span structure proves separate turns.
///
/// Content alone remains the identity for snapshots and replays. Two places provide stronger
/// evidence:
///
/// - each generation span's own output is a model execution;
/// - the last user input of each top-level agent span is that agent invocation's request.
///
/// Copies inherit the rank only inside the same top-level agent invocation. That lets a generation
/// input point to its own repeated user request and an agent output point to its child generation,
/// without turning nested-agent context into another conversation turn.
pub(super) fn apply_plain_occurrence_ordinals(blocks: &[BlockEntry], ordinals: &mut [u32]) {
    // Only an agent operation at the trace boundary can establish an external request. Workflow
    // engines also label nested loop nodes as agents; ranking those nodes turns one request replayed
    // through the graph into a new user turn on every iteration.
    let agent_spans: std::collections::HashSet<Span<'_>> = blocks
        .iter()
        .filter(|block| block.is_agent_span() && block.span_path.len() <= 2)
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

    let user_input_shapes: std::collections::HashSet<ScopedShape<'_>> = blocks
        .iter()
        .filter(|block| block.role == ChatRole::User && block.is_input_source())
        .filter_map(|block| {
            plain_message_shape(block)
                .map(|shape| (block.trace_id.as_str(), block.span_id.as_str(), shape))
        })
        .collect();
    let generation_spans: std::collections::HashSet<Span<'_>> = blocks
        .iter()
        .filter(|block| block.is_generation_span())
        .map(|block| (block.trace_id.as_str(), block.span_id.as_str()))
        .collect();
    let generation_wrappers = generation_wrappers(blocks);

    // A request snapshot may contain the whole conversation. Only its last user member is the
    // current request. The same rule on descendant generation spans identifies the copy that should
    // inherit that request's rank rather than the rank of an earlier identical turn.
    let mut last_user_position: HashMap<Span<'_>, (i32, i32)> = HashMap::new();
    for (index, block) in blocks.iter().enumerate() {
        if block.role != ChatRole::User
            || !block.is_input_source()
            || top_agent_by_block[index].is_none()
        {
            continue;
        }
        let span = (block.trace_id.as_str(), block.span_id.as_str());
        let position = (block.message_index, block.entry_index);
        last_user_position
            .entry(span)
            .and_modify(|current| *current = (*current).max(position))
            .or_insert(position);
    }

    // Every representation of one proved occurrence shares an anchor. This keeps a redelivered
    // span, or one generation publishing the same response through two carriers, at one rank.
    let base_ordinals = ordinals.to_vec();
    let mut members_by_anchor: HashMap<Anchor<'_>, Vec<usize>> = HashMap::new();
    for (index, block) in blocks.iter().enumerate() {
        let Some(shape) = plain_message_shape(block) else {
            continue;
        };
        let generation_output = block.role == ChatRole::Assistant
            && is_generation_execution_anchor(block)
            && !generation_wrappers.contains(&(block.trace_id.as_str(), block.span_id.as_str()));
        let request_already_carried_by_ancestor = block
            .span_path
            .iter()
            .take(block.span_path.len().saturating_sub(1))
            .any(|ancestor| {
                user_input_shapes.contains(&(block.trace_id.as_str(), ancestor.as_str(), shape))
            });
        let agent_request = block.role == ChatRole::User
            && block.is_input_source()
            && !request_already_carried_by_ancestor
            && top_agent_by_block[index] == Some(block.span_id.as_str())
            && last_user_position
                .get(&(block.trace_id.as_str(), block.span_id.as_str()))
                .is_some_and(|position| *position == (block.message_index, block.entry_index));
        if !generation_output && !agent_request {
            continue;
        }
        members_by_anchor
            .entry((
                block.trace_id.as_str(),
                block.span_id.as_str(),
                shape,
                base_ordinals[index],
            ))
            .or_default()
            .push(index);
    }

    let mut anchors_by_group: HashMap<OccurrenceGroup<'_>, Vec<Anchor<'_>>> = HashMap::new();
    #[expect(
        clippy::iter_over_hash_type,
        reason = "each group's anchors are sorted below by a key that tells every two of them apart"
    )]
    for &anchor in members_by_anchor.keys() {
        let block = &blocks[members_by_anchor[&anchor][0]];
        let group = if block.role == ChatRole::User {
            OccurrenceGroup::AgentRequest(
                anchor.0,
                anchor.2,
                block.name.as_deref().or(block.span_name.as_deref()),
            )
        } else {
            OccurrenceGroup::GenerationOutput(anchor.0, anchor.2)
        };
        anchors_by_group.entry(group).or_default().push(anchor);
    }
    let mut rank_by_anchor: HashMap<Anchor<'_>, u32> = HashMap::new();
    #[expect(
        clippy::iter_over_hash_type,
        reason = "the groups are disjoint: each ranks its own anchors and writes only their members"
    )]
    for (group, anchors) in &mut anchors_by_group {
        anchors.sort_by_key(|anchor| {
            let index = members_by_anchor[anchor][0];
            (
                blocks[index].timestamp,
                blocks[index].span_id.as_str(),
                anchor.3,
                blocks[index].message_index,
                blocks[index].entry_index,
            )
        });
        for (rank, anchor) in anchors.iter().enumerate() {
            // Without an agent identity, several peer agents cannot be distinguished from repeated
            // invocations. Collapsing is the conservative answer; a declared identity can prove
            // repeated calls of the same agent.
            let rank = match group {
                OccurrenceGroup::AgentRequest(_, _, None) => 0,
                _ => rank as u32,
            };
            rank_by_anchor.insert(*anchor, rank);
            for &member in &members_by_anchor[anchor] {
                ordinals[member] = rank;
            }
        }
    }

    let mut request_rank: HashMap<ScopedShape<'_>, u32> = HashMap::new();
    let mut output_ranks: HashMap<ScopedShape<'_>, std::collections::HashSet<u32>> = HashMap::new();
    let mut nested_generation_rank: HashMap<ScopedShape<'_>, (DateTime<Utc>, u32)> = HashMap::new();
    // In anchor order, so what each key ends up holding depends on the anchors and not on a hash order.
    let mut anchors: Vec<(&Anchor<'_>, &Vec<usize>)> = members_by_anchor.iter().collect();
    anchors.sort_unstable_by_key(|(anchor, _)| **anchor);
    for (&anchor, members) in anchors {
        let block = &blocks[members[0]];
        let rank = rank_by_anchor[&anchor];
        if block.role == ChatRole::Assistant {
            for ancestor in block
                .span_path
                .iter()
                .take(block.span_path.len().saturating_sub(1))
            {
                let ancestor_span = (block.trace_id.as_str(), ancestor.as_str());
                if !generation_spans.contains(&ancestor_span) {
                    continue;
                }
                nested_generation_rank
                    .entry((block.trace_id.as_str(), ancestor.as_str(), anchor.2))
                    // The latest output below the wrapper; of two at one instant, the later rank.
                    .and_modify(|current| {
                        if (block.timestamp, rank) > *current {
                            *current = (block.timestamp, rank);
                        }
                    })
                    .or_insert((block.timestamp, rank));
            }
        }
        if let Some(top_agent) = top_agent_by_block[members[0]] {
            let key = (block.trace_id.as_str(), top_agent, anchor.2);
            if block.role == ChatRole::User {
                request_rank.insert(key, rank);
            } else if block.role == ChatRole::Assistant {
                output_ranks.entry(key).or_default().insert(rank);
            }
        }
    }

    for (index, block) in blocks.iter().enumerate() {
        let Some(shape) = plain_message_shape(block) else {
            continue;
        };
        if block.role == ChatRole::Assistant
            && block.is_generation_span()
            && block.is_output_source()
            && let Some((_, rank)) = nested_generation_rank.get(&(
                block.trace_id.as_str(),
                block.span_id.as_str(),
                shape,
            ))
        {
            ordinals[index] = *rank;
        }
        let Some(top_agent) = top_agent_by_block[index] else {
            continue;
        };
        let key = (block.trace_id.as_str(), top_agent, shape);
        if block.role == ChatRole::User
            && block.is_input_source()
            && last_user_position
                .get(&(block.trace_id.as_str(), block.span_id.as_str()))
                .is_some_and(|position| *position == (block.message_index, block.entry_index))
            && let Some(rank) = request_rank.get(&key)
        {
            ordinals[index] = *rank;
        } else if block.role == ChatRole::Assistant
            && block.is_output_source()
            && let Some(ranks) = output_ranks.get(&key)
            && ranks.len() == 1
        {
            ordinals[index] = *ranks.iter().next().expect("one output rank");
        }
    }
}
