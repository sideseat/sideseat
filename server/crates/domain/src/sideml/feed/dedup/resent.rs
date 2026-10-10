//! The parts of a re-sent response that only the re-send carries.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::*;

/// History copies to keep although nothing current has them: the parts a re-sent response is the only
/// copy of.
///
/// A history copy survives only beside a current equivalent, which is how a request's re-listing of the
/// conversation so far is folded away. A framework can leave a part out of the response it reports and
/// still re-send it whole: Agno reports a turn's answer without the reasoning before it, and the next
/// request carries the turn with that reasoning and its signature. The re-sent answer has its current
/// equivalent - the reported one - and the reasoning has none, so it was dropped as history, and the
/// conversation lost a step its own telemetry holds.
///
/// Kept only where the message says whose it is: every other part of it that a model call produced was
/// produced by one and the same call, and every message re-sending the part says the same call. Anything
/// else - a re-send of two calls' parts, two re-sends disagreeing, a message with nothing current in it -
/// stays history.
pub(super) fn resent_parts<'a>(
    blocks: impl Iterator<Item = (&'a BlockEntry, u32)> + Clone,
    current: &HashSet<DedupKey>,
) -> HashSet<DedupKey> {
    // Which model calls produced each current part.
    let mut producers: HashMap<DedupKey, BTreeSet<&str>> = HashMap::new();
    for (block, ordinal) in blocks.clone() {
        if !block.is_history && block.is_output_source() && block.is_generation_span() {
            producers
                .entry((MessageIdentity::from_block(block), ordinal))
                .or_default()
                .insert(&block.span_id);
        }
    }
    // Each re-sent message's parts: (trace, span, carrier, message index) -> keys.
    type Message<'b> = (&'b str, &'b str, Option<&'b str>, Option<&'b str>, i32);
    let mut messages: HashMap<Message<'a>, Vec<DedupKey>> = HashMap::new();
    for (block, ordinal) in blocks {
        if !block.is_history {
            continue;
        }
        messages
            .entry((
                &block.trace_id,
                &block.span_id,
                block.event_name.as_deref(),
                block.source_attribute.as_deref(),
                block.message_index,
            ))
            .or_default()
            .push((MessageIdentity::from_block(block), ordinal));
    }
    // The one call each orphaned part is re-sent with, or `None` once two messages disagree.
    let mut call_of: HashMap<DedupKey, Option<&str>> = HashMap::new();
    #[expect(
        clippy::iter_over_hash_type,
        reason = "agreement is commutative: a part keeps its call only where every message re-sending it names that one call"
    )]
    for parts in messages.values() {
        let calls: BTreeSet<&str> = parts
            .iter()
            .filter(|key| current.contains(key))
            .filter_map(|key| producers.get(key))
            .flatten()
            .copied()
            .collect();
        let call = match calls.iter().collect::<Vec<_>>().as_slice() {
            [one] => Some(**one),
            _ => None,
        };
        for key in parts.iter().filter(|key| !current.contains(key)) {
            let entry = call_of.entry(key.clone()).or_insert(call);
            if *entry != call {
                *entry = None;
            }
        }
    }
    call_of
        .into_iter()
        .filter_map(|(key, call)| call.map(|_| key))
        .collect()
}
