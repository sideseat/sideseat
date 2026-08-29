//! Order resolver: the timeline as a partial order, not a scalar key.
//!
//! The previous timeline was one sort key whose anchor is a mutable per-response minimum. Because
//! that anchor is computed *after* dedup, the order depends on which copy of a message survived, and
//! two copies tie on quality routinely — so reading a carrier that was previously ignored silently
//! reorders unrelated messages. Six scalar-anchor candidates were tried and rejected (see the plan);
//! the conclusion, reviewed with Codex, is that ordering is a **partial order** and time is a
//! *priority*, not a constraint.
//!
//! This module builds that partial order and resolves it. Production runs
//! [`Constraints::PRODUCTION`], which lists exactly which classes are enforced and what each one was
//! measured to change; [`Constraints::NEUTRAL`] enforces nothing and is provably unable to move a
//! block, which keeps the machinery itself verifiable
//! (`the_neutral_resolver_reproduces_the_legacy_order`) as classes are promoted one at a time. Under
//! [`Constraints::FULL`] it produces the redesign's intended answer, which tests compare against.
//!
//! # Model (Codex's framing)
//!
//! Three levels, deliberately distinct:
//!
//! - **Evidence occurrences**: every pre-dedup observation, with its exact carrier instance. This is
//!   why the resolver reads the classified blocks *before* dedup — the emission that binds a turn's
//!   intro text to its tool call is on one span, but dedup may keep a re-listed copy of the text from
//!   another span, which would lose the binding.
//! - **Logical identities**: the dedup equivalence classes (the survivors).
//! - **Ordering units**: one identity, or several identities contracted because they were one atomic
//!   emission. Contiguity cannot be a pairwise edge — a DAG says `A < B`, never "nothing between A
//!   and B" — so an emission becomes a single node and external edges attach to its boundary.
//!
//! # Constraints built so far
//!
//! Only the two highest-confidence classes, which is all the `strands-js/swarm` case needs:
//!
//! 1. **Atomic-emission contraction**: identities emitted together by one `gen_ai.choice` instance
//!    are one unit, ordered by their source position.
//! 2. **Exact call → result**: a unit holding a tool result follows the unit holding its call, when
//!    the call id is unambiguous among survivors.
//!
//! Generation input→output and snapshot-sequence edges are the next classes to add; they are
//! deliberately absent here. Credible time is a **priority** for the topological pop, never an edge.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::dedup::{SpanTimestamps, effective_timestamp};
use super::types::BlockEntry;

/// A survivor's contribution to an emission instance: `(message_index, entry_index, survivor)`.
/// The pair fixes the block's source order within the emission; the index points back at the
/// survivor being contracted.
type EmissionMember = (i32, i32, usize);

/// What the resolver needs to know about one pre-dedup observation.
///
/// Deliberately not the observation itself. The resolver reads the evidence set *before* dedup, and
/// holding onto whole blocks for that meant cloning every message's content on every request - on a
/// fixture whose tool results carry base64 images that is the dominant cost, and none of it is ever
/// read. This is the same set of facts in a handful of words per observation.
pub(super) struct OrderEvidence {
    /// Which emission instance this observation belongs to, when it is a credible emission of one.
    emission: Option<usize>,
    /// Where the observation sat in its payload, for the emission's own order.
    message_index: i32,
    entry_index: i32,
    /// The observation's own effective time. The *survivor's* time is no use here: dedup overwrites it
    /// with the old batch anchor, so reading it would make the new order a function of the old one.
    effective: DateTime<Utc>,
    /// Usable as evidence of when the message happened: a credible emission, not a history re-send.
    credible: bool,
    /// Which span carried this observation, interned.
    span: usize,
    /// Which carrier of that span, interned - the event or attribute it was read from.
    carrier: usize,
    /// That carrier's positions state the order its observations belong in.
    carrier_ordered: bool,
    /// The span produced this observation, rather than receiving it.
    is_output: bool,
    /// The span is a generation - a model call, so its input caused its output.
    from_generation: bool,
}

/// Reduce the classified, pre-dedup blocks to what the resolver reads.
///
/// An emission instance is `(span, position-path root)`: two `gen_ai.choice` events on one span have
/// different roots, so this separates them while the blocks of one choice share it. Instances are
/// interned to indices, so the only allocation is one key per distinct emission.
pub(super) fn collect_order_evidence(
    blocks: &[BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> Vec<OrderEvidence> {
    let mut instances: HashMap<(String, String), usize> = HashMap::new();
    let mut spans: HashMap<&str, usize> = HashMap::new();
    let mut carriers: HashMap<(&str, Option<&str>, Option<&str>), usize> = HashMap::new();
    blocks
        .iter()
        .map(|block| {
            let credible = is_credible_emission(block);
            let next_span = spans.len();
            let span = *spans.entry(block.span_id.as_str()).or_insert(next_span);
            let semantics = crate::domain::sideml::carrier::semantics_for(
                block.event_name.as_deref(),
                block.source_attribute.as_deref(),
            );
            let next_carrier = carriers.len();
            let carrier = *carriers
                .entry((
                    block.span_id.as_str(),
                    block.event_name.as_deref(),
                    block.source_attribute.as_deref(),
                ))
                .or_insert(next_carrier);
            let emission = credible.then(|| {
                let root = block
                    .position
                    .to_string()
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let next = instances.len();
                *instances
                    .entry((block.span_id.clone(), root))
                    .or_insert(next)
            });
            OrderEvidence {
                emission,
                message_index: block.message_index,
                entry_index: block.entry_index,
                effective: effective_timestamp(block, span_timestamps),
                credible: credible && !block.is_history,
                span,
                carrier,
                carrier_ordered: semantics.position_provides_sequence_order && !block.is_history,
                is_output: block.is_output_source(),
                from_generation: block.is_generation_span(),
            }
        })
        .collect()
}

/// Whether an observation is evidence of *when* a message happened and part of *one emission*.
///
/// An atomic emission the span itself produced: `gen_ai.choice`, and only when the span is the
/// emitter (`is_output_source`), never a received copy or a re-listed snapshot. A framework handing
/// a past result back to the model carries an emission-shaped carrier but its time is the hand-back,
/// not the occurrence — reading it as evidence moved a result ahead of its call.
fn is_credible_emission(block: &BlockEntry) -> bool {
    block.is_output_source()
        && crate::domain::sideml::carrier::semantics_for(
            block.event_name.as_deref(),
            block.source_attribute.as_deref(),
        )
        .carrier_is_atomic_emission
}

/// A disjoint-set over survivor indices, used to contract co-emitted identities into one unit.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            // Deterministic: the smaller index becomes the representative.
            let (root, child) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent[child] = root;
        }
    }
}

/// Which constraints the resolver is allowed to *change the answer* with.
///
/// This is the promotion dial for the redesign. `SCAFFOLD` builds the whole graph and runs the whole
/// resolve, but enforces only what the existing order already satisfies, so its output is provably
/// the existing order — that is what lets the machinery go into production before any behaviour
/// changes.
///
/// One field per behaviour, deliberately: promoting a class means flipping *one* of them, so the
/// resulting golden delta is attributable to that class alone. A single bundled flag turned all four
/// on at once, which would have made the first promotion's diff uninterpretable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Constraints {
    /// Contract an emission whose survivors are not already adjacent in the legacy order.
    ///
    /// This is the half of neutrality that filtering edges does not cover: moving an emission's
    /// scattered members together is a reorder, and it is precisely the reorder the redesign exists
    /// to make (the `strands-js/swarm` intro text).
    pub contract_non_contiguous_emissions: bool,
    /// Enforce an edge of *any* class that the legacy order has backwards.
    ///
    /// This is the dial that lets the graph actually change an order rather than merely agree with
    /// one, so it is the second half of every promotion: a class whose edges are all forward already
    /// changes nothing.
    pub enforce_backward_edges: bool,
    /// Order units by time first, rather than by their legacy index.
    pub time_priority: bool,
    /// Order a unit's members by their source position rather than by legacy index.
    pub source_position_member_order: bool,
    /// Enforce the order a carrier's own payload states between its surviving observations.
    ///
    /// A message array is a sequence, and two blocks of one message are ordered by their position in
    /// it. Without this the time priority can swap them - two blocks of one ADK `llm_response` came
    /// back reversed - because a per-unit anchor says nothing about order *inside* a payload. This is
    /// the constraint form of the `assert_carrier_subsequence` invariant.
    pub carrier_sequence_edges: bool,
    /// Enforce that what a generation span *received* precedes what it *produced*.
    ///
    /// The minimal turn structure, and deliberately local dataflow rather than a rule about roles: a
    /// model call's input caused its output, so the system prompt and the tool results a call was given
    /// precede the answer it produced, and transitivity carries that across spans. A global rule like
    /// "the terminal assistant message follows the last user message and every intervening tool" says
    /// something similar and is false for parallel branches, subagents, retries and abandoned calls.
    pub generation_dataflow_edges: bool,
}

impl Constraints {
    /// Provably output-neutral: every constraint is built and resolved, none can move a block.
    ///
    /// Kept as its own configuration after promotions begin, because it is what
    /// `the_neutral_resolver_reproduces_the_legacy_order` tests: the proof that the machinery cannot
    /// move anything on its own has to stay checkable, or a promotion could hide a resolver bug.
    #[cfg(test)]
    pub(super) const NEUTRAL: Self = Self {
        contract_non_contiguous_emissions: false,
        enforce_backward_edges: false,
        time_priority: false,
        source_position_member_order: false,
        carrier_sequence_edges: false,
        generation_dataflow_edges: false,
    };

    /// What production enforces today.
    ///
    /// One class is promoted at a time, and each promotion's golden delta is read fixture by fixture
    /// before it lands. Promoted so far:
    ///
    /// - **Atomic-emission contraction** with source-position member order: a turn's intro text and
    ///   the call it introduces are one `gen_ai.choice`, so they stay together in the order that event
    ///   listed them. `strands-js/swarm` was the case - the intro text trailed the tool result it was
    ///   meant to introduce, because the text took its span's end time and the call took its event
    ///   time, so they grouped separately.
    /// - **Carrier-sequence edges**: the order a payload states between its own surviving blocks. On
    ///   its own this changes nothing (it enforces what the previous sort already produced); it is
    ///   promoted with contraction because it is what keeps two blocks of one message from being
    ///   separated once anything else can move them.
    ///
    /// Not yet promoted, with what each does to the corpus, measured:
    ///
    /// - `time_priority`: blocked on the emission-instance key. An instance is `(span, position-path
    ///   root)`, so it holds one payload - but Vercel spreads one response across *sibling attributes*
    ///   (`ai.response.text` beside `ai.response.toolCalls`), which are different roots and so
    ///   different instances. Contraction therefore cannot hold that response together, and promoting
    ///   time alone displaces its intro text behind its own calls, which is the defect promotion 1
    ///   fixed for Strands. It needs an instance notion that spans the sibling attributes of one
    ///   response before it can be promoted. Measured with contraction already on: 3 fixtures change,
    ///   `vercel-ai-js/image-gen` and `vercel-ai-js/tool-use` for the worse; `agent-framework/swarm`
    ///   for the better, interleaving each specialist's system prompt with its own answer instead of
    ///   listing all prompts first.
    /// - `enforce_backward_edges`: moves `adk/tool_use`; needed before any edge class can *correct* an
    ///   order rather than only agree with one.
    /// - `generation_dataflow_edges`: removes every reorder that blocks `PerCarrier` extraction, which
    ///   is what would recover the answers 20 span views are missing - the largest correctness win
    ///   available, and gated on the two above.
    pub(super) const PRODUCTION: Self = Self {
        contract_non_contiguous_emissions: true,
        enforce_backward_edges: false,
        time_priority: false,
        source_position_member_order: true,
        carrier_sequence_edges: true,
        generation_dataflow_edges: false,
    };

    /// Every constraint enforced - the redesign's intended answer.
    #[cfg(test)]
    pub(super) const FULL: Self = Self {
        contract_non_contiguous_emissions: true,
        enforce_backward_edges: true,
        time_priority: true,
        source_position_member_order: true,
        carrier_sequence_edges: true,
        generation_dataflow_edges: true,
    };
}

/// Resolve the order over the surviving blocks.
///
/// `pre_dedup` is the classified evidence set (every observation); `survivors` is the deduplicated
/// result in the current pipeline order — that order is the deterministic tie-break and the
/// neutrality seed. Returns the survivors permuted into the partial order's resolution.
///
/// # Neutrality
///
/// Under [`Constraints::NEUTRAL`] the result is exactly `survivors`. Every enforced edge is already
/// forward in the legacy order and every contracted unit is already contiguous in it, so the legacy
/// order is itself a topological order of the graph; popping the ready unit with the smallest legacy
/// index therefore yields the legacy order, because any predecessor of the smallest-index unfinished
/// unit would have a smaller index and would already be done.
pub(super) fn resolve(
    evidence: &[OrderEvidence],
    survivors: &[BlockEntry],
    lineage: &[Option<usize>],
    span_timestamps: &HashMap<String, SpanTimestamps>,
    constraints: Constraints,
) -> Vec<BlockEntry> {
    let n = survivors.len();
    if n <= 1 {
        return survivors.to_vec();
    }

    // Which survivor each observation became, as the pipeline recorded it - not recomputed here.
    //
    // Recomputing was wrong twice over. Survivors are not unique by `MessageIdentity`, because dedup
    // keys on `(identity, repeat ordinal)`: a response holding two identical tool calls with distinct
    // ids keeps both, and one map entry then took the other's evidence (`crewai/mcp_tools` is the
    // corpus trace that does this). And `withdraw_unbacked_ids` runs in between, clearing a
    // correlated result's id - which changes its identity outright, so its evidence stopped matching
    // anything at all.
    let survivor_of =
        |observation: usize| -> Option<usize> { lineage.get(observation).copied().flatten() };

    // Co-emission sets from the evidence: group credible-emission observations by instance, collect
    // the surviving identities in each, in source order. A block whose identity did not survive is
    // ignored - the unit is over survivors.
    let mut by_instance: HashMap<usize, Vec<EmissionMember>> = HashMap::new();
    for (observation, seen) in evidence.iter().enumerate() {
        let Some(instance) = seen.emission else {
            continue;
        };
        let Some(survivor) = survivor_of(observation) else {
            continue;
        };
        by_instance.entry(instance).or_default().push((
            seen.message_index,
            seen.entry_index,
            survivor,
        ));
    }

    // Contract each instance's survivors into one unit, and remember the source order within it.
    //
    // Iterated in a deterministic order: a HashMap's iteration order varies per run, and while
    // union-find's result does not depend on the order the unions arrive in, the intra-unit keys and
    // any later diagnostics do.
    let mut instances: Vec<(&usize, &Vec<EmissionMember>)> = by_instance.iter().collect();
    instances.sort_by_key(|(instance, _)| **instance);

    // A survivor is routinely claimed by *two* emission instances: an inner generation span emits a
    // message and its parent agent span re-emits the same one as its own output. That is the common
    // shape, not an anomaly, so instances are merged rather than rejected - and the consequence is
    // that a unit's members can carry position paths rooted in different payloads, whose coordinates
    // are not comparable. Taking a global minimum position across them can violate the source order
    // of both emissions.
    //
    // So each instance contributes *adjacency* rather than coordinates: consecutive members of one
    // emission become an edge, and a unit's members are ordered by resolving those edges. Two
    // emissions that agree are both honoured; if they disagree the unit falls back to legacy order,
    // which is the only answer that cannot claim to satisfy evidence it contradicts.
    let mut uf = UnionFind::new(n);
    let mut intra_edges: Vec<(usize, usize)> = Vec::new();
    for (_, members) in instances {
        let mut legacy: Vec<usize> = members.iter().map(|&(_, _, s)| s).collect();
        legacy.sort_unstable();
        legacy.dedup();

        if !constraints.contract_non_contiguous_emissions
            && !legacy.windows(2).all(|w| w[1] == w[0] + 1)
        {
            continue;
        }

        // This emission's own order, by source position, deduplicated: two blocks of one message can
        // map to one survivor.
        let mut ordered: Vec<EmissionMember> = members.clone();
        ordered.sort_by_key(|&(msg_idx, entry_idx, survivor)| (msg_idx, entry_idx, survivor));
        let mut sequence: Vec<usize> = Vec::with_capacity(ordered.len());
        for (_, _, survivor) in ordered {
            if sequence.last() != Some(&survivor) {
                sequence.push(survivor);
            }
        }
        for pair in sequence.windows(2) {
            if pair[0] != pair[1] {
                intra_edges.push((pair[0], pair[1]));
            }
        }
        // Union all survivors of this instance together.
        let first = sequence[0];
        for &survivor in &sequence[1..] {
            uf.union(first, survivor);
        }
    }

    let unit_of: Vec<usize> = (0..n).map(|i| uf.find(i)).collect();

    // Priority per unit: the earliest time the *evidence* gives it. Time seeds the topological pop;
    // it never forces an order an edge does not.
    //
    // Taken from the pre-dedup occurrences, not from the survivors. A survivor's `timestamp` has
    // already been overwritten by `process_dedup` with its old batch anchor, so reading it here would
    // make the new order a function of the order it is replacing - and would carry the very
    // copy-survival dependence this redesign exists to remove: whichever copy won would decide the
    // anchor. Only a credible emission counts as evidence of a time (a re-listed snapshot's time is
    // when it was assembled), with the survivor's own effective time as the fallback where an
    // identity has no emission at all - a user message read from an attribute array, say.
    let mut unit_priority: HashMap<usize, DateTime<Utc>> = HashMap::new();
    let record = |unit: usize, time: DateTime<Utc>, map: &mut HashMap<usize, DateTime<Utc>>| {
        map.entry(unit)
            .and_modify(|t| {
                if time < *t {
                    *t = time;
                }
            })
            .or_insert(time);
    };
    let mut from_emission: HashMap<usize, DateTime<Utc>> = HashMap::new();
    for (observation, seen) in evidence.iter().enumerate() {
        if !seen.credible {
            continue;
        }
        let Some(survivor) = survivor_of(observation) else {
            continue;
        };
        record(unit_of[survivor], seen.effective, &mut from_emission);
    }
    for (i, block) in survivors.iter().enumerate() {
        let unit = unit_of[i];
        let time = from_emission
            .get(&unit)
            .copied()
            .unwrap_or_else(|| effective_timestamp(block, span_timestamps));
        unit_priority
            .entry(unit)
            .and_modify(|t| {
                if time < *t {
                    *t = time;
                }
            })
            .or_insert(time);
    }
    // The smallest legacy index in each unit, for a stable tie-break and the neutrality seed.
    let mut unit_min_legacy: HashMap<usize, usize> = HashMap::new();
    for (i, &unit) in unit_of.iter().enumerate() {
        unit_min_legacy
            .entry(unit)
            .and_modify(|m| *m = (*m).min(i))
            .or_insert(i);
    }

    // Exact call -> result edges over units. A result's unit follows its call's unit, but only when
    // exactly one surviving call carries the id: a reused or regenerated id is ambiguous and adds no
    // edge (Codex: do not treat "first call with this id" as a hard constraint).
    let mut call_units: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, block) in survivors.iter().enumerate() {
        if block.entry_type == "tool_use"
            && let Some(id) = block.tool_use_id.as_deref().filter(|s| !s.is_empty())
        {
            call_units.entry(id).or_default().push(unit_of[i]);
        }
    }
    let units: Vec<usize> = {
        let mut u: Vec<usize> = unit_priority.keys().copied().collect();
        u.sort_unstable();
        u
    };
    let mut successors: HashMap<usize, Vec<usize>> =
        units.iter().map(|&u| (u, Vec::new())).collect();
    let mut indegree: HashMap<usize, usize> = units.iter().map(|&u| (u, 0)).collect();
    for (i, block) in survivors.iter().enumerate() {
        if block.entry_type != "tool_result" {
            continue;
        }
        let Some(id) = block.tool_use_id.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some(callers) = call_units.get(id) else {
            continue;
        };
        if callers.len() != 1 {
            continue; // ambiguous id
        }
        let call_unit = callers[0];
        let result_unit = unit_of[i];
        if call_unit == result_unit {
            continue; // one emission already orders them
        }
        add_edge(
            call_unit,
            result_unit,
            constraints,
            &unit_min_legacy,
            &mut successors,
            &mut indegree,
        );
    }

    // Carrier sequence: the order a payload states between its own surviving observations.
    //
    // Adjacent pairs only; the transitive closure adds nothing to a topological order. Ordered by the
    // payload position, which is what `message_index`/`entry_index` carry for a carrier's blocks.
    if constraints.carrier_sequence_edges {
        let mut by_carrier: HashMap<usize, Vec<(i32, i32, usize)>> = HashMap::new();
        for (observation, seen) in evidence.iter().enumerate() {
            if !seen.carrier_ordered {
                continue;
            }
            let Some(survivor) = survivor_of(observation) else {
                continue;
            };
            by_carrier.entry(seen.carrier).or_default().push((
                seen.message_index,
                seen.entry_index,
                survivor,
            ));
        }
        let mut carriers: Vec<&usize> = by_carrier.keys().collect();
        carriers.sort_unstable();
        let carriers: Vec<usize> = carriers.into_iter().copied().collect();
        for carrier in carriers {
            let mut members = by_carrier.remove(&carrier).unwrap_or_default();
            members.sort_unstable();
            let mut sequence: Vec<usize> = Vec::with_capacity(members.len());
            for (_, _, survivor) in members {
                let unit = unit_of[survivor];
                if sequence.last() != Some(&unit) {
                    sequence.push(unit);
                }
            }
            for pair in sequence.windows(2) {
                add_edge(
                    pair[0],
                    pair[1],
                    constraints,
                    &unit_min_legacy,
                    &mut successors,
                    &mut indegree,
                );
            }
        }
    }

    // Generation dataflow: what a model call received precedes what it produced.
    //
    // Read from the evidence rather than from the survivors, because the copy on display often comes
    // from a different span - the answer a chain span re-lists is still the generation's output, and
    // the system prompt a generation received is still its input even when the surviving copy of it
    // was read somewhere else.
    //
    // Only edges between *different* units are added, and an edge already implied by contraction is
    // skipped. Under the scaffold an edge the legacy order has backwards is dropped, as everywhere.
    if constraints.generation_dataflow_edges {
        let mut inputs_by_span: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut outputs_by_span: HashMap<usize, Vec<usize>> = HashMap::new();
        for (observation, seen) in evidence.iter().enumerate() {
            if !seen.from_generation {
                continue;
            }
            let Some(survivor) = survivor_of(observation) else {
                continue;
            };
            let side = if seen.is_output {
                &mut outputs_by_span
            } else {
                &mut inputs_by_span
            };
            let units = side.entry(seen.span).or_default();
            let unit = unit_of[survivor];
            if !units.contains(&unit) {
                units.push(unit);
            }
        }
        let mut spans: Vec<&usize> = inputs_by_span.keys().collect();
        spans.sort_unstable();
        let spans: Vec<usize> = spans.into_iter().copied().collect();
        for span in spans {
            let Some(outputs) = outputs_by_span.get(&span) else {
                continue;
            };
            let inputs = &inputs_by_span[&span];
            for &input in inputs {
                for &output in outputs {
                    add_edge(
                        input,
                        output,
                        constraints,
                        &unit_min_legacy,
                        &mut successors,
                        &mut indegree,
                    );
                }
            }
        }
    }

    // Kahn's algorithm, popping the ready unit with the smallest (priority, min-legacy, unit-id).
    // On a stall - a cycle - break it deterministically by the same key over the remaining units,
    // so the resolver is total rather than panicking. (Full SCC condensation is a later increment;
    // no corpus fixture cycles at this constraint density.)
    let mut order: Vec<usize> = Vec::with_capacity(units.len());
    let mut done: HashMap<usize, bool> = units.iter().map(|&u| (u, false)).collect();
    // Under the scaffold the seed is the legacy index alone (`None` sorts before any `Some`, so the
    // time term drops out entirely): that is what makes the resolve reproduce the legacy order rather
    // than merely agree with it on this corpus. Promoting time to the primary key is its own delta.
    let key = |u: usize,
               unit_priority: &HashMap<usize, DateTime<Utc>>,
               unit_min_legacy: &HashMap<usize, usize>| {
        let primary = if constraints.time_priority {
            Some(unit_priority[&u])
        } else {
            None
        };
        (primary, unit_min_legacy[&u], u)
    };
    while order.len() < units.len() {
        let mut ready: Vec<usize> = units
            .iter()
            .copied()
            .filter(|u| !done[u] && indegree[u] == 0)
            .collect();
        if ready.is_empty() {
            // Cycle: release the smallest-key remaining unit.
            ready = units.iter().copied().filter(|u| !done[u]).collect();
        }
        let next = *ready
            .iter()
            .min_by_key(|&&u| key(u, &unit_priority, &unit_min_legacy))
            .expect("at least one unit remains");
        order.push(next);
        done.insert(next, true);
        for &s in &successors[&next] {
            if let Some(d) = indegree.get_mut(&s) {
                *d = d.saturating_sub(1);
            }
        }
    }

    // Emit each unit's members, ordered by the emissions' own adjacency.
    let mut members_of: HashMap<usize, Vec<usize>> =
        units.iter().map(|&u| (u, Vec::new())).collect();
    for (i, &unit) in unit_of.iter().enumerate() {
        members_of.get_mut(&unit).expect("unit present").push(i);
    }
    let mut out = Vec::with_capacity(n);
    for unit in order {
        let mut members = members_of.remove(&unit).unwrap_or_default();
        members.sort_unstable();
        if constraints.source_position_member_order {
            members = order_within_unit(&members, &intra_edges);
        }
        for i in members {
            out.push(survivors[i].clone());
        }
    }
    out
}

/// Add one precedence edge between units, unless the scaffold forbids it.
///
/// The scaffold enforces only an edge the legacy order already respects, so no edge of its graph can
/// move anything - which is what makes the resolver safe to run in production before any class is
/// promoted. Duplicate edges are skipped so an indegree cannot be counted twice.
fn add_edge(
    from: usize,
    to: usize,
    constraints: Constraints,
    unit_min_legacy: &HashMap<usize, usize>,
    successors: &mut HashMap<usize, Vec<usize>>,
    indegree: &mut HashMap<usize, usize>,
) {
    if from == to {
        return;
    }
    let backward = unit_min_legacy[&from] > unit_min_legacy[&to];
    if backward && !constraints.enforce_backward_edges {
        return;
    }
    let succ = successors.get_mut(&from).expect("unit present");
    if !succ.contains(&to) {
        succ.push(to);
        *indegree.get_mut(&to).expect("unit present") += 1;
    }
}

/// Order one unit's members by the adjacency its emissions stated, smallest legacy index first among
/// the members nothing else has to precede.
///
/// Falls back to the given (legacy) order when the edges restricted to this unit contain a cycle: two
/// emissions disagreeing about the order of the same pair is a contradiction in the evidence, and
/// legacy order is the one answer that does not claim to satisfy either.
fn order_within_unit(members: &[usize], intra_edges: &[(usize, usize)]) -> Vec<usize> {
    if members.len() < 2 {
        return members.to_vec();
    }
    let inside: HashMap<usize, ()> = members.iter().map(|&m| (m, ())).collect();
    let mut successors: HashMap<usize, Vec<usize>> =
        members.iter().map(|&m| (m, Vec::new())).collect();
    let mut indegree: HashMap<usize, usize> = members.iter().map(|&m| (m, 0)).collect();
    for &(from, to) in intra_edges {
        if !inside.contains_key(&from) || !inside.contains_key(&to) {
            continue;
        }
        let succ = successors.get_mut(&from).expect("member present");
        if !succ.contains(&to) {
            succ.push(to);
            *indegree.get_mut(&to).expect("member present") += 1;
        }
    }

    let mut out: Vec<usize> = Vec::with_capacity(members.len());
    let mut remaining: Vec<usize> = members.to_vec();
    while !remaining.is_empty() {
        let Some(&next) = remaining
            .iter()
            .filter(|m| indegree[m] == 0)
            .min_by_key(|&&m| m)
        else {
            // Cycle: the emissions contradict each other about this unit.
            return members.to_vec();
        };
        out.push(next);
        remaining.retain(|&m| m != next);
        for &s in &successors[&next] {
            if let Some(d) = indegree.get_mut(&s) {
                *d = d.saturating_sub(1);
            }
        }
    }
    out
}
