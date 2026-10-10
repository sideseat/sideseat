//! Which emission instances only re-list what spans below them produced.

use super::*;

/// Which emission instances are a *redundant re-listing*, and so contribute presence but not order.
///
/// The Vercel defect: a root agent span re-lists a whole turn as its own output, answer first, while the
/// `chat` spans below it emitted the calls and the answer separately. Read as an emission its stated
/// order is trusted and the answer sorts ahead of the tool calls that produced it.
///
/// The discriminator is **not** the carrier - the same carrier name on a generation span is that span's
/// own emission - and not "some other span has this message either", which is true of every replay. It
/// is whether every message the instance lists was independently produced by a *descendant* span. Then
/// the re-listing adds nothing but an order, and its order is the one thing it gets wrong.
///
/// Deliberately all-or-nothing: an instance only *partly* covered still carries evidence about the
/// messages nobody below it produced, so it keeps all of it. Guessing per message would mean splitting
/// one emission's order across two readings.
pub(super) fn redundant_relistings(
    evidence: &[OrderEvidence],
    survivors: &[BlockEntry],
    survivor_of: &impl Fn(usize) -> Option<usize>,
) -> HashSet<usize> {
    // Where each instance sits, and what it claims.
    let mut instance_span: HashMap<usize, usize> = HashMap::new();
    let mut instance_accumulator: HashMap<usize, bool> = HashMap::new();
    let mut claimed: HashMap<usize, HashSet<usize>> = HashMap::new();
    // Where every span sits, from any observation on it.
    type Placement = (Vec<usize>, bool, Option<(DateTime<Utc>, DateTime<Utc>)>);
    let mut placement: HashMap<usize, Placement> = HashMap::new();
    for (observation, seen) in evidence.iter().enumerate() {
        placement.entry(seen.span).or_insert_with(|| {
            (
                seen.ancestor_spans.clone(),
                seen.ancestry_truncated,
                seen.span_interval,
            )
        });
        let Some(instance) = seen.emission else {
            continue;
        };
        instance_span.insert(instance, seen.span);
        instance_accumulator.insert(instance, seen.accumulator);
        if let Some(survivor) = survivor_of(observation) {
            claimed.entry(instance).or_default().insert(survivor);
        }
    }
    let mut out = HashSet::new();
    #[expect(
        clippy::iter_over_hash_type,
        reason = "each instance is judged alone, into a set"
    )]
    for (&instance, members) in &claimed {
        if members.is_empty() || instance_accumulator.get(&instance) != Some(&true) {
            continue;
        }
        let Some(&span) = instance_span.get(&instance) else {
            continue;
        };
        // Below this span: under it in the ancestry, or - where a span's ancestry stops at a parent
        // without messages, so the chain cannot reach this one - running inside this span's interval,
        // which the re-listing span opened before and closed after.
        let interval = placement.get(&span).and_then(|(_, _, interval)| *interval);
        let below = |other: usize| {
            other != span
                && placement
                    .get(&other)
                    .is_some_and(|(ancestors, truncated, inner)| {
                        ancestors.contains(&span)
                            || (*truncated
                                && matches!((interval, inner), (Some(outer), Some(inner))
                                if outer.0 <= inner.0 && inner.1 <= outer.1))
                    })
        };
        // A witness is another instance on a span below this one.
        let witnesses = |survivor: &usize| -> HashSet<usize> {
            claimed
                .iter()
                .filter(|&(&other, others)| {
                    other != instance
                        && others.contains(survivor)
                        && instance_span
                            .get(&other)
                            .is_some_and(|&other_span| below(other_span))
                })
                .map(|(&other, _)| other)
                .collect()
        };
        let witnessed = |survivor: &usize| !witnesses(survivor).is_empty();
        // A model cannot answer its own call inside one response: it emits the calls, and the results
        // come back from tools afterwards. So an instance holding **both** a message the span produced
        // and a result answering one is not one emission - it is a re-listing of a whole turn, whatever
        // carrier it arrived on.
        //
        // This is what separates it from the emissions whose contraction is load-bearing. A turn's intro
        // text and the call it introduces are one response (`[assistant/text, assistant/tool_use]`) and
        // must stay contracted; a span re-listing `[text, call, call, result, result]` is reporting what
        // its children did, and its order is the one thing it gets wrong.
        let holds_own_output = members
            .iter()
            .any(|&m| survivors[m].role == crate::sideml::types::ChatRole::Assistant);
        // `entry_type`, which is what the rest of this resolver keys a result on (see the call/result edges
        // below) - not the content variant, so the two cannot disagree about what a result is.
        let holds_an_answer = members
            .iter()
            .any(|&m| survivors[m].entry_type == "tool_result");
        // The other proof that it is not one response: what it lists, no single model call below it
        // produced. The AI SDK's agent span lists a failed tool turn as `[answer, call]`, one message -
        // the shape of an intro text and the call it introduces - while the call came from the first
        // model call and the answer from the second. A response that really was one is witnessed whole
        // by the one call that produced it.
        let merges_responses = || {
            let mut common: Option<HashSet<usize>> = None;
            #[expect(
                clippy::iter_over_hash_type,
                reason = "an intersection: whether it empties does not depend on the order it is taken in"
            )]
            for member in members {
                let witnesses = witnesses(member);
                let narrowed: HashSet<usize> = match common {
                    Some(previous) => previous.intersection(&witnesses).copied().collect(),
                    None => witnesses,
                };
                if narrowed.is_empty() {
                    return true;
                }
                common = Some(narrowed);
            }
            false
        };
        if !holds_own_output || !(holds_an_answer || members.len() > 1 && merges_responses()) {
            continue;
        }
        if members.iter().all(witnessed) {
            out.insert(instance);
        }
    }
    out
}
