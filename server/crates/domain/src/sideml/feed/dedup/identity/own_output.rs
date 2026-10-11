use super::*;

/// A generation span's input never contains its own output: the request was sent before the model answered, so a
/// plain block on the input side and one on the output side of one generation span are two occurrences however
/// equal their content - the model was shown the words and said them again.
///
/// Across a whole trace the generation anchors already keep them apart: each generation span's output is ranked
/// as its own execution, and an input re-send takes the rank of the execution that produced it
/// (`plain_occurrence`). A span view's rows are one span, where that numbering collapses to 0 on both sides and
/// the span's view folded the re-sent reasoning it was given into the answer it produced. So where the two sides
/// still share a rank, the output side moves to one of its own - which repeats what the anchors decide where
/// they can see the whole trace, and changes nothing there.
///
/// Only the strict sides: a block promoted to span output is the snapshot's own copy of the answer, with no twin
/// to be told apart from, and an output carrier that may restate what the span received - accumulated state,
/// which lists the conversation beside the answer - holds re-sent copies on its output side, which are the input
/// said again, not the answer.
pub(super) fn separate_input_from_own_output(blocks: &[BlockEntry], ordinals: &mut [u32]) {
    /// Everything that already tells two plain blocks apart, plus the rank.
    type Slot<'a> = (&'a str, &'a str, u64, u32);
    let mut inputs: std::collections::HashSet<Slot<'_>> = std::collections::HashSet::new();
    let strict_input = |block: &BlockEntry| {
        block.is_generation_span()
            && !block.promoted_to_span_output
            && block.is_input_source()
            && !block.is_output_source()
    };
    let strict_output = |block: &BlockEntry| {
        block.is_generation_span()
            && !block.promoted_to_span_output
            && block.is_output_source()
            && !crate::sideml::carrier::semantics_for_context(&block.carrier_context())
                .may_restate_prior_observations
    };
    for (index, block) in blocks.iter().enumerate() {
        if strict_input(block)
            && let Some(shape) = plain_message_shape(block)
        {
            inputs.insert((
                block.trace_id.as_str(),
                block.span_id.as_str(),
                shape,
                ordinals[index],
            ));
        }
    }
    if inputs.is_empty() {
        return;
    }
    for (index, block) in blocks.iter().enumerate() {
        if strict_output(block)
            && let Some(shape) = plain_message_shape(block)
            && inputs.contains(&(
                block.trace_id.as_str(),
                block.span_id.as_str(),
                shape,
                ordinals[index],
            ))
        {
            // A rank no input of this span holds, derived from the colliding one rather than counted across
            // the set, so one block's rank never depends on how many others there are.
            ordinals[index] |= 1 << 30;
        }
    }
}
