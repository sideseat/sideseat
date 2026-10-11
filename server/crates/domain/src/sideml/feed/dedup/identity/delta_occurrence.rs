use std::collections::BTreeMap;

use super::*;

/// A request delta states only what is new since its thread's previous request
/// (`carrier_holds_request_delta`). Where its positions also prove occurrences - the carrier declares every
/// member an occurrence of its own - two deltas of one trace stating the same words are two statements: the
/// client sent the reminder again, and the truth of such a conversation holds it twice. Folding them kept one
/// environment reminder for two turns, and one task preamble for two subagents.
///
/// Within one delta those positions already prove occurrences; across spans the position ranks restart at 0,
/// which is what made two deltas' copies one. Each span's such blocks move to a rank band of their own, by the
/// span's place among the trace's and carrier's delta-carrying spans in the pipeline's sorted order -
/// deterministic, since rows arrive sorted by `(span_timestamp, span_id)` - so a redelivered row keeps its
/// span's band and still folds onto itself, and two threads' spans never share a band.
///
/// One shape still restates rather than repeats: within one declared request thread (`request_threads`), a
/// resumed turn's delta that carries every member of the thread's previous delta, in order, and then says
/// more. The captures show exactly that accumulation - a resumed process reports the reminders of the turns
/// before it beside its own - so the restated prefix folds onto the originals by inheriting their bands, and
/// only the tail is new. The extension must be strict: a delta that says exactly what the previous one said is
/// what the live captures prove to be a fresh delivery (the client sent the same reminder again, and the
/// recorded requests hold it twice), so equal or unrelated lists take a fresh band. The comparison never
/// crosses threads - another thread's extending delta is that thread's own statement - and where no rule names
/// a thread nothing proves two spans share a history, so no prefix folds.
///
/// Deltas whose positions prove nothing stay out: such a carrier may list a neighbouring turn again (the
/// captures show one repeating the previous prompt), and separating its spans would show the repeat twice. And
/// within one trace only: a new process resumed over an old conversation starts a new trace and restates the
/// old reminders in its first delta, which is the cross-trace replay logic's question, not this rule's.
pub(super) fn separate_deltas_by_span(blocks: &[BlockEntry], ordinals: &mut [u32]) {
    /// Bands sit above the per-value position ranks, in a namespace of their own: bits 0-15 hold the rank,
    /// bits 16-29 the band, and bit 30 stays what `own_output` set it to. No shipped delta holds 2^16
    /// members or 2^14 delta spans per trace and carrier, but the boundary is enforced, not assumed: a rank
    /// is clamped below the band width and a band below bit 30, so an adversarial payload folds its overflow
    /// onto the last rank of its own span, or the last band of its own trace and carrier, instead of
    /// colliding with another span's band or another pass's namespace.
    const BAND: u32 = 1 << 16;
    const LAST_BAND: u32 = (1 << 14) - 1;
    const KEPT: u32 = 1 << 30;
    /// Bands are numbered per trace and carrier, so two spans of one group never share one.
    type Group<'a> = (&'a str, &'a str);
    /// A thread's statements so far: each member's shape, and the complete ordinal it was assigned.
    type Previous = Option<(Vec<u64>, Vec<u32>)>;
    /// One span's delta: its thread, and its members in payload order, each with its content shape and
    /// block indices.
    struct SpanDelta<'a> {
        span: &'a str,
        thread: &'a str,
        members: BTreeMap<(i32, i32), (Option<u64>, Vec<usize>)>,
    }

    let mut groups: Vec<Vec<SpanDelta<'_>>> = Vec::new();
    let mut group_at: HashMap<Group<'_>, usize> = HashMap::new();
    // Indexed per group so grouping stays O(blocks); the maps live as long as the pass, which is one walk
    // over blocks the caller already holds, so the working set is proportional to the input and nothing else.
    let mut span_at: HashMap<(usize, &str), usize> = HashMap::new();
    for (index, block) in blocks.iter().enumerate() {
        let semantics = crate::sideml::carrier::semantics_for_context(&block.carrier_context());
        if !(semantics.carrier_holds_request_delta && semantics.position_proves_distinct_occurrence)
        {
            continue;
        }
        let carrier = block
            .source_attribute
            .as_deref()
            .or(block.event_name.as_deref())
            .unwrap_or("");
        let at = *group_at
            .entry((block.trace_id.as_str(), carrier))
            .or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
        let spans = &mut groups[at];
        // A redelivered row revisits an earlier span; it must land in that span's delta, not open a new one.
        let found = *span_at
            .entry((at, block.span_id.as_str()))
            .or_insert_with(|| {
                spans.push(SpanDelta {
                    span: block.span_id.as_str(),
                    thread: block.request_thread.as_str(),
                    members: BTreeMap::new(),
                });
                spans.len() - 1
            });
        debug_assert_eq!(spans[found].span, block.span_id.as_str());
        spans[found]
            .members
            .entry((block.message_index, block.entry_index))
            .or_insert_with(|| (plain_message_shape(block), Vec::new()))
            .1
            .push(index);
    }

    for spans in &groups {
        let mut next_band: u32 = 0;
        // Per thread, what a resumption would restate: every delta's members so far, in statement order, each
        // with the *complete* ordinal it was assigned - band and rank, since a restated copy must land exactly
        // on its original, whatever rank its own position ranks gave it. A restating delta carries the whole
        // history, not only the previous span's, so the comparison is against the accumulation. `None` once
        // any member has no plain shape, since such a history cannot be shown to be a prefix of anything.
        let mut history: HashMap<&str, Previous> = HashMap::new();
        for span in spans {
            let shapes: Option<Vec<u64>> = span.members.values().map(|(shape, _)| *shape).collect();
            // A fold needs proof of a shared history, which is the thread key; without one the spans are
            // only known to be distinct statements.
            let inherited = match (history.get(span.thread), &shapes) {
                (Some(Some((prior, _))), Some(current))
                    if !span.thread.is_empty()
                        && current.len() > prior.len()
                        && current[..prior.len()] == prior[..] =>
                {
                    prior.len()
                }
                _ => 0,
            };
            let band = next_band;
            next_band = (next_band + 1).min(LAST_BAND);
            let mut assigned = vec![0u32; span.members.len()];
            for (slot, (_, indices)) in span.members.values().enumerate() {
                assigned[slot] = if slot < inherited {
                    let Some(Some((_, prior_ordinals))) = history.get(span.thread) else {
                        unreachable!("`inherited` is nonzero only with a usable history");
                    };
                    prior_ordinals[slot]
                } else {
                    let rank = indices
                        .first()
                        .map(|&index| (ordinals[index] & !KEPT).min(BAND - 1))
                        .unwrap_or(0);
                    (band * BAND) | rank
                };
                for &index in indices {
                    ordinals[index] = (ordinals[index] & KEPT) | assigned[slot];
                }
            }
            // A restating delta *is* the history up to its own tail; a fresh one adds its statements to it.
            let grown = match (history.remove(span.thread), &shapes, inherited) {
                (_, Some(current), at) if at > 0 => Some((current.clone(), assigned)),
                (Some(Some((mut prior, mut prior_ordinals))), Some(current), _) => {
                    prior.extend_from_slice(current);
                    prior_ordinals.extend_from_slice(&assigned);
                    Some((prior, prior_ordinals))
                }
                (None, Some(current), _) => Some((current.clone(), assigned)),
                _ => None,
            };
            history.insert(span.thread, grown);
        }
    }
}
