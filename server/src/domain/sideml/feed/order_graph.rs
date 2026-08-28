//! Shadow order resolver — increment 1 of the ingestion/reconstruction redesign.
//!
//! The production timeline is one sort key whose anchor is a mutable per-response minimum. Because
//! that anchor is computed *after* dedup, the order depends on which copy of a message survived, and
//! two copies tie on quality routinely — so reading a carrier that was previously ignored silently
//! reorders unrelated messages. Six scalar-anchor candidates were tried and rejected (see the plan);
//! the conclusion, reviewed with Codex, is that ordering is a **partial order** and time is a
//! *priority*, not a constraint.
//!
//! This module builds that partial order and resolves it, but its output is **not wired into any
//! view yet**. It is exercised only by tests, which assert it derives the orders the scalar key gets
//! wrong (`strands-js/swarm`: intro text, then its call, then the result). Landing it inert first
//! lets each real constraint class be promoted afterwards as its own small, explainable golden delta,
//! rather than as one large rewrite.
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
//! # Constraints in this increment
//!
//! Only the two highest-confidence classes, which is all the `strands-js/swarm` case needs:
//!
//! 1. **Atomic-emission contraction**: identities emitted together by one `gen_ai.choice` instance
//!    are one unit, ordered by their source position.
//! 2. **Exact call → result**: a unit holding a tool result follows the unit holding its call, when
//!    the call id is unambiguous among survivors.
//!
//! Generation input→output and snapshot-sequence edges are the next classes to promote; they are
//! deliberately absent here. Credible time is a **priority** for the topological pop, never an edge.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::dedup::{MessageIdentity, SpanTimestamps, effective_timestamp};
use super::types::BlockEntry;

/// A survivor's contribution to an emission instance: `(message_index, entry_index, survivor)`.
/// The pair fixes the block's source order within the emission; the index points back at the
/// survivor being contracted.
type EmissionMember = (i32, i32, usize);

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

/// The emission instance a block belongs to: its span and the root of its position path. Two
/// `gen_ai.choice` events in one span have different roots, so this separates them; the blocks of
/// one choice share it.
fn emission_instance(block: &BlockEntry) -> Option<(String, String)> {
    if !is_credible_emission(block) {
        return None;
    }
    let root = block
        .position
        .to_string()
        .split('.')
        .next()
        .unwrap_or("")
        .to_string();
    Some((block.span_id.clone(), root))
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

/// Resolve the shadow order over the surviving blocks.
///
/// `pre_dedup` is the classified evidence set (every observation); `survivors` is the deduplicated
/// result in the current pipeline order — that order is the deterministic tie-break and, later, the
/// neutrality seed. Returns the survivors permuted into the partial order's resolution.
pub(super) fn resolve(
    pre_dedup: &[BlockEntry],
    survivors: &[BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> Vec<BlockEntry> {
    let n = survivors.len();
    if n <= 1 {
        return survivors.to_vec();
    }

    // Identity -> survivor index (survivors are unique by identity after dedup).
    let survivor_of: HashMap<MessageIdentity, usize> = survivors
        .iter()
        .enumerate()
        .map(|(i, b)| (MessageIdentity::from_block(b), i))
        .collect();

    // Co-emission sets from the evidence: group credible-emission observations by instance, collect
    // the surviving identities in each, in source order. A block whose identity did not survive is
    // ignored - the unit is over survivors.
    let mut by_instance: HashMap<(String, String), Vec<EmissionMember>> = HashMap::new();
    for block in pre_dedup {
        let Some(instance) = emission_instance(block) else {
            continue;
        };
        let Some(&survivor) = survivor_of.get(&MessageIdentity::from_block(block)) else {
            continue;
        };
        by_instance.entry(instance).or_default().push((
            block.message_index,
            block.entry_index,
            survivor,
        ));
    }

    // Contract each instance's survivors into one unit, and remember the source order within it.
    let mut uf = UnionFind::new(n);
    let mut intra_unit_key = vec![(i32::MAX, i32::MAX); n];
    for members in by_instance.values() {
        for &(msg_idx, entry_idx, survivor) in members {
            // Keep the earliest source position seen for this survivor as its within-unit key.
            if (msg_idx, entry_idx) < (intra_unit_key[survivor].0, intra_unit_key[survivor].1) {
                intra_unit_key[survivor] = (msg_idx, entry_idx);
            }
        }
        // Union all survivors of this instance together.
        let first = members[0].2;
        for &(_, _, survivor) in &members[1..] {
            uf.union(first, survivor);
        }
    }

    let unit_of: Vec<usize> = (0..n).map(|i| uf.find(i)).collect();

    // Priority per unit: the earliest effective time among its members. Time seeds the topological
    // pop; it never forces an order an edge does not. (A later increment weights a credible emission
    // above a re-listed copy here; this increment takes the plain minimum.)
    let mut unit_priority: HashMap<usize, DateTime<Utc>> = HashMap::new();
    for (i, block) in survivors.iter().enumerate() {
        let time = effective_timestamp(block, span_timestamps);
        let unit = unit_of[i];
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
        // Adjacent edge only; avoid duplicates.
        let succ = successors.get_mut(&call_unit).expect("unit present");
        if !succ.contains(&result_unit) {
            succ.push(result_unit);
            *indegree.get_mut(&result_unit).expect("unit present") += 1;
        }
    }

    // Kahn's algorithm, popping the ready unit with the smallest (priority, min-legacy, unit-id).
    // On a stall - a cycle - break it deterministically by the same key over the remaining units,
    // so the resolver is total rather than panicking. (Full SCC condensation is a later increment;
    // no corpus fixture cycles at this constraint density.)
    let mut order: Vec<usize> = Vec::with_capacity(units.len());
    let mut done: HashMap<usize, bool> = units.iter().map(|&u| (u, false)).collect();
    let key = |u: usize,
               unit_priority: &HashMap<usize, DateTime<Utc>>,
               unit_min_legacy: &HashMap<usize, usize>| {
        (unit_priority[&u], unit_min_legacy[&u], u)
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

    // Emit each unit's members in source order, then legacy order as a final tie-break.
    let mut members_of: HashMap<usize, Vec<usize>> =
        units.iter().map(|&u| (u, Vec::new())).collect();
    for (i, &unit) in unit_of.iter().enumerate() {
        members_of.get_mut(&unit).expect("unit present").push(i);
    }
    let mut out = Vec::with_capacity(n);
    for unit in order {
        let mut members = members_of.remove(&unit).unwrap_or_default();
        members.sort_by_key(|&i| (intra_unit_key[i], i));
        for i in members {
            out.push(survivors[i].clone());
        }
    }
    out
}
