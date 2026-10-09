//! Parts a request's history is the first to show, bound to the response they were re-sent with.

use super::*;

/// Where a part first seen in a re-sent message is bound, as source-ordered survivor sequences - each one
/// message's parts - and, for each bound part, the survivor whose time it takes.
///
/// A framework can leave a part out of the response it reports and still re-send it, whole, in the next
/// request's history: Strands for TypeScript reports a turn's answer without the reasoning before it,
/// and the following request carries `[reasoning, answer]` as one assistant message. Deduplication
/// folds the re-sent answer into the reported one, and the reasoning - which nothing produced - was
/// ordered at the later request's time, after the answer it preceded. The message says where it goes:
/// beside the parts it was re-sent with, and at their response's time.
///
/// Bound only where that is unambiguous. A part is *new* where no model call produced it - no
/// observation of it is a generation's output. It is bound to a response where every message re-sending
/// it pairs it with the output of one and the same response, and every other part of such a message
/// that any generation produced was produced by that response alone. Where nothing in the message was
/// produced - a span's own view, which holds no producer - it is bound to the parts of its message that
/// the same span carries in another copy too, its request reported twice: Agno flattens the request,
/// dropping the reasoning, and keeps its own messages whole beside it. Two candidate responses, a re-send
/// beside parts of two calls, or a part a call did produce: nothing moves.
pub(super) fn resent_part_bindings(
    evidence: &[OrderEvidence],
    survivor_of: &impl Fn(usize) -> Option<usize>,
    enabled: bool,
) -> (Vec<Vec<usize>>, HashMap<usize, usize>) {
    if !enabled {
        return Default::default();
    }
    /// Whose a bound part is: one response's, or its own span's other copy of the request.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Home {
        Produced(usize),
        Copied(usize),
    }
    // Which generation spans produced each survivor.
    let mut producers: HashMap<usize, BTreeSet<usize>> = HashMap::new();
    // Each input message's parts: (span, carrier, message index) -> (entry index, survivor).
    let mut messages: HashMap<(usize, usize, i32), Vec<(i32, usize)>> = HashMap::new();
    // Which received messages each survivor appears in.
    let mut received_in: HashMap<usize, BTreeSet<(usize, usize, i32)>> = HashMap::new();
    // The spans that are model calls: only a request is reported twice by one span.
    let mut requests: BTreeSet<usize> = BTreeSet::new();
    for (observation, seen) in evidence.iter().enumerate() {
        let Some(survivor) = survivor_of(observation) else {
            continue;
        };
        if seen.is_output {
            if seen.from_generation {
                producers.entry(survivor).or_default().insert(seen.span);
            }
            continue;
        }
        if seen.from_generation {
            requests.insert(seen.span);
        }
        let key = (seen.span, seen.carrier, seen.message_index);
        messages
            .entry(key)
            .or_default()
            .push((seen.entry_index, survivor));
        received_in.entry(survivor).or_default().insert(key);
    }
    // Whether a model call's span receives a survivor in another carrier too: its request in a second
    // copy. Another message of the same carrier is another turn of one request, never a copy of this one,
    // and two turns sharing a part - one answer given twice - must not bind their reasoning together. An
    // agent's or a chain's span re-lists state, which is no second copy of a request either.
    let copied_elsewhere = |survivor: usize, key: &(usize, usize, i32)| {
        requests.contains(&key.0)
            && received_in[&survivor]
                .iter()
                .any(|other| other.0 == key.0 && other.1 != key.1)
    };
    // The one home each new part is re-sent with, or `None` once two disagree.
    let mut home_of_new: HashMap<usize, Option<Home>> = HashMap::new();
    let mut bound: Vec<(Vec<usize>, Home)> = Vec::new();
    let mut keys: Vec<&(usize, usize, i32)> = messages.keys().collect();
    keys.sort_unstable();
    for key in keys {
        let mut parts = messages[key].clone();
        parts.sort_unstable();
        parts.dedup();
        // A request re-sends what came before it, never what its own span produced: a part this span
        // produced is a copy of identical content - two turns' empty reasoning - and says nothing here.
        if parts.iter().any(|(_, survivor)| {
            producers
                .get(survivor)
                .is_some_and(|spans| spans.contains(&key.0))
        }) {
            continue;
        }
        let produced: BTreeSet<usize> = parts
            .iter()
            .filter_map(|(_, survivor)| producers.get(survivor))
            .flatten()
            .copied()
            .collect();
        let new: Vec<usize> = parts
            .iter()
            .map(|&(_, survivor)| survivor)
            .filter(|survivor| !producers.contains_key(survivor))
            .filter(|&survivor| !produced.is_empty() || !copied_elsewhere(survivor, key))
            .collect();
        if new.is_empty() {
            continue;
        }
        let home = match produced.iter().collect::<Vec<_>>().as_slice() {
            [one] => Some(Home::Produced(**one)),
            [] if parts
                .iter()
                .any(|&(_, survivor)| copied_elsewhere(survivor, key)) =>
            {
                Some(Home::Copied(key.0))
            }
            _ => None,
        };
        for survivor in &new {
            let entry = home_of_new.entry(*survivor).or_insert(home);
            if *entry != home {
                *entry = None;
            }
        }
        if let Some(home) = home {
            let mut sequence: Vec<usize> = parts.iter().map(|&(_, survivor)| survivor).collect();
            sequence.dedup();
            bound.push((sequence, home));
        }
    }
    // A message survives only where every new part in it kept one home throughout.
    bound.retain(|(sequence, _)| {
        sequence
            .iter()
            .filter(|survivor| home_of_new.contains_key(survivor))
            .all(|survivor| home_of_new[survivor].is_some())
    });
    // Each bound part takes the time of the first part of its message that is at home there.
    let mut donors: HashMap<usize, usize> = HashMap::new();
    for (sequence, home) in &bound {
        let at_home = |survivor: &&usize| match home {
            Home::Produced(_) => producers.contains_key(survivor),
            Home::Copied(_) => !home_of_new.contains_key(survivor),
        };
        if let Some(&donor) = sequence.iter().find(at_home) {
            for survivor in sequence.iter().filter(|s| home_of_new.contains_key(s)) {
                donors.entry(*survivor).or_insert(donor);
            }
        }
    }
    (
        bound.into_iter().map(|(sequence, _)| sequence).collect(),
        donors,
    )
}

/// The received observations that are copies of what their own span produced: `(observation, span)`.
///
/// A request lists what came before it, never what its span is about to produce, so such a mapping is two
/// contents that happen to be identical - two turns' empty reasoning folded into one block. Its place in
/// the request states nothing about the block, and taking it as one drags the span's own output back
/// into its history.
pub(super) fn own_outputs(
    evidence: &[OrderEvidence],
    survivor_of: &impl Fn(usize) -> Option<usize>,
) -> HashSet<(usize, usize)> {
    let produced: HashSet<(usize, usize)> = evidence
        .iter()
        .enumerate()
        .filter(|(_, seen)| seen.is_output)
        .filter_map(|(observation, seen)| Some((survivor_of(observation)?, seen.span)))
        .collect();
    evidence
        .iter()
        .enumerate()
        .filter(|(_, seen)| !seen.is_output)
        .filter_map(|(observation, seen)| {
            let survivor = survivor_of(observation)?;
            produced
                .contains(&(survivor, seen.span))
                .then_some((observation, seen.span))
        })
        .collect()
}

/// A bound part as its response shows it: at the time of the part it was re-sent beside.
pub(super) fn at_home(block: &BlockEntry, donor: Option<&BlockEntry>) -> BlockEntry {
    let mut block = block.clone();
    if let Some(donor) = donor {
        block.order_time = donor.order_time;
        block.timestamp = donor.timestamp;
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One observation: on `span`, in `carrier`'s message `message` at `entry`, produced or received.
    fn seen(span: usize, carrier: usize, message: i32, entry: i32, output: bool) -> OrderEvidence {
        OrderEvidence {
            emission: None,
            message_index: message,
            entry_index: entry,
            effective: chrono::DateTime::<Utc>::UNIX_EPOCH,
            credible: false,
            history: !output,
            span,
            carrier,
            carrier_ordered: false,
            is_output: output,
            from_generation: true,
            detached_frame: false,
            tool_reference: None,
            accumulator: false,
            ancestor_spans: Vec::new(),
            ancestry_truncated: false,
            span_interval: None,
            input_family: None,
        }
    }

    fn bindings(evidence: &[OrderEvidence], survivors: &[usize]) -> Vec<Vec<usize>> {
        resent_part_bindings(
            evidence,
            &|observation| survivors.get(observation).copied(),
            true,
        )
        .0
    }

    /// The answer span 0 produced, then re-sent by span 1 with the reasoning nothing produced: the
    /// reasoning is bound ahead of that answer.
    #[test]
    fn a_part_first_re_sent_beside_one_response_is_bound_to_it() {
        let evidence = [
            seen(0, 0, 0, 0, true),
            seen(1, 1, 2, 0, false),
            seen(1, 1, 2, 1, false),
        ];
        // Survivors: 1 is the answer, 0 the reasoning.
        assert_eq!(bindings(&evidence, &[1, 0, 1]), vec![vec![0, 1]]);
    }

    /// A span's request holding a copy of what that span produced is identical content, not history:
    /// its place there is no evidence. A copy another span received is.
    #[test]
    fn a_request_listing_its_own_output_states_nothing_about_it() {
        let evidence = [
            seen(1, 0, 0, 0, true),
            seen(1, 1, 2, 0, false),
            seen(2, 1, 2, 0, false),
        ];
        let own = own_outputs(&evidence, &|observation| {
            [0, 0, 0].get(observation).copied()
        });
        assert_eq!(own, [(1, 1)].into());
    }

    /// A span that reports its request twice, one copy dropping a part: in the span's own view nothing was
    /// produced, and the part joins the message the other copy also holds, at that message's time.
    #[test]
    fn a_part_one_copy_of_a_request_drops_is_bound_to_the_other() {
        let evidence = [
            seen(1, 1, 2, 0, false),
            seen(1, 1, 2, 1, false),
            seen(1, 2, 2, 0, false),
        ];
        let survivor_of = |observation: usize| [0, 1, 1].get(observation).copied();
        let (sequences, donors) = resent_part_bindings(&evidence, &survivor_of, true);
        assert_eq!(sequences, vec![vec![0, 1]]);
        assert_eq!(donors.get(&0), Some(&1));
        // A part every copy holds is no part one of them dropped: nothing to bind.
        let both = [seen(1, 1, 2, 0, false), seen(1, 2, 2, 0, false)];
        assert!(bindings(&both, &[1, 1]).is_empty());
    }

    /// Two turns of one request share an answer: another message of the same carrier is another turn, not a
    /// second copy of the request, so neither turn's reasoning is bound to the shared answer.
    #[test]
    fn two_turns_of_one_carrier_sharing_a_part_are_not_copies_of_each_other() {
        let evidence = [
            seen(1, 1, 2, 0, false),
            seen(1, 1, 2, 1, false),
            seen(1, 1, 4, 0, false),
            seen(1, 1, 4, 1, false),
        ];
        assert!(bindings(&evidence, &[0, 1, 2, 1]).is_empty());
    }

    /// Re-sent beside parts of two responses, or with two responses in two messages: which one it
    /// belongs to is not stated, and nothing is bound.
    #[test]
    fn a_part_re_sent_beside_two_responses_is_left_where_it_is() {
        let beside_two = [
            seen(0, 0, 0, 0, true),
            seen(2, 0, 0, 0, true),
            seen(1, 1, 2, 0, false),
            seen(1, 1, 2, 1, false),
            seen(1, 1, 2, 2, false),
        ];
        assert!(bindings(&beside_two, &[1, 2, 0, 1, 2]).is_empty());
        let in_two_messages = [
            seen(0, 0, 0, 0, true),
            seen(2, 0, 0, 0, true),
            seen(1, 1, 2, 0, false),
            seen(1, 1, 2, 1, false),
            seen(3, 1, 4, 0, false),
            seen(3, 1, 4, 1, false),
        ];
        assert!(bindings(&in_two_messages, &[1, 2, 0, 1, 0, 2]).is_empty());
        // A request listing what its own span produced - identical content folded together, two turns'
        // empty reasoning - re-sends nothing of it: the earlier answer is not bound into this response.
        let own = [
            seen(1, 0, 0, 0, true),
            seen(1, 1, 2, 0, false),
            seen(1, 1, 2, 1, false),
        ];
        assert!(bindings(&own, &[0, 0, 1]).is_empty());
        // A part a call did produce is no new part: it stays where its own response put it.
        let produced = [
            seen(0, 0, 0, 0, true),
            seen(2, 0, 0, 0, true),
            seen(1, 1, 2, 0, false),
        ];
        assert!(bindings(&produced, &[1, 0, 0]).is_empty());
    }
}
