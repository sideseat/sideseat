//! Resolves the timeline as a partial order.
//!
//! The previous timeline was one sort key whose anchor is a mutable per-response minimum. Because
//! that anchor is computed *after* dedup, the order depends on which copy of a message survived, and
//! two copies tie on quality routinely — so reading a carrier that was previously ignored silently
//! reorders unrelated messages. A scalar anchor cannot represent the required relations: ordering is
//! a **partial order**, and time is a *priority*, not a constraint.
//!
//! This module builds that partial order and resolves it. Production runs
//! [`Constraints::PRODUCTION`], which lists exactly which classes are enforced and what each one was
//! measured to change; [`Constraints::NEUTRAL`] enforces nothing and is provably unable to move a
//! block, which keeps the machinery itself verifiable
//! (`the_neutral_resolver_reproduces_the_legacy_order`) as classes are promoted one at a time. Under
//! [`Constraints::FULL`] it produces the redesign's intended answer, which tests compare against.
//!
//! # Model
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
//! # Constraint classes
//!
//! See [`Constraints`] for the full list and what each was measured to change: atomic-emission
//! contraction, exact call → result, carrier sequence, generation dataflow, request framing, and the
//! fragmented ordered-input family. Credible time is a **priority** for the topological pop, never an
//! edge.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};

use chrono::{DateTime, Utc};

use super::dedup::{SpanTimestamps, effective_timestamp};
use super::types::BlockEntry;
use crate::sideml::types::{ChatRole, FinishReason};

mod resolve;

pub(super) use resolve::resolve;

/// A survivor's contribution to an emission instance: `(message_index, entry_index, survivor)`.
/// The pair fixes the block's source order within the emission; the index points back at the
/// survivor being contracted.
type EmissionMember = (i32, i32, usize);

/// What orders one unit against another when the constraints leave them free: the anchor its evidence
/// gives it, where it was first observed, and its own id as the final discriminator.
type PopKey = (Option<DateTime<Utc>>, usize, usize);

/// One payload instance: its span, the event or attribute it arrived on, and which event instance it
/// was. A span can emit `gen_ai.choice` more than once, so events need the position root to distinguish
/// payloads. An attribute exists only once on a span; all roots extracted from it belong to that one
/// payload and must retain their sequence.
type PayloadKey<'a> = (&'a str, Option<&'a str>, Option<&'a str>, String);

// How many cycles the resolver broke, visible to tests: the `warn!` below reaches production
// telemetry, and tests install no subscriber, so without this a contradiction in the evidence is
// invisible exactly where the corpus could catch it.
#[cfg(any(test, feature = "test-support"))]
thread_local! {
    #[doc(hidden)]
    pub static CYCLES_BROKEN_IN_TESTS: std::cell::RefCell<usize> =
        const { std::cell::RefCell::new(0) };
}

/// The synthetic node standing for "everything this span received precedes everything it produced".
///
/// Numbered past the real units so it cannot collide with one, and it emits no blocks - it exists only
/// to keep the edge count linear in a span's messages rather than quadratic.
/// A synthetic ordering node for one span's dataflow, in a range disjoint from the survivor indices.
///
/// `generation` distinguishes the two a span may need: an overlapping input/output set is expressed as two
/// barriers rather than as a product (see the dataflow class), and they must be distinct nodes.
fn barrier_unit(span: usize, survivor_count: usize, span_upper: usize, generation: usize) -> usize {
    survivor_count + generation * span_upper + span
}

/// What the resolver needs to know about one pre-dedup observation.
///
/// Deliberately not the observation itself. The resolver reads the evidence set *before* dedup, and
/// holding onto whole blocks for that meant cloning every message's content on every request - on a
/// fixture whose tool results carry base64 images that is the dominant cost, and none of it is ever
/// read. This is the same set of facts in a handful of words per observation.
enum ToolReference {
    Call(String),
    Result(String),
}

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
    /// The observation came from a *detached request frame* carrier - the system instruction a
    /// generation was given, reported beside the conversation. A carrier fact, never a role fact:
    /// see `CarrierSemantics::carrier_is_detached_request_frame`.
    detached_frame: bool,
    /// Exact tool causality carried by this observation before dedup chooses a representative.
    tool_reference: Option<ToolReference>,
    /// The span is an *accumulator* - an agent, chain or plain span, which collects what its children
    /// produced rather than producing it.
    accumulator: bool,
    /// The interned spans this observation's span sits under, nearest ancestor last.
    ///
    /// Ancestry, not membership of the trace: "some other span also has this message" is true of a replay
    /// and says nothing, while "a span *below* this one produced it" is what makes a re-listing redundant.
    ancestor_spans: Vec<usize>,
    /// The ordered-input carrier *family* this observation belongs to, interned per span, when its
    /// carrier is an ordered input array of a generation span. `llm.input_messages.0.message` and
    /// `.1.message` are one array, and the family is what groups them - the exact key interns them
    /// apart and a one-member sequence orders nothing. History is deliberately **not** folded in here
    /// (unlike `carrier_ordered`): the framing block reads this and neutralises replays with its
    /// first-seen rule instead, which is what lets the array order the request's own new turns even
    /// when their surviving copies live on other spans.
    input_family: Option<usize>,
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
    // Keyed by payload *instance*, not carrier name: a span can emit `gen_ai.choice` several times,
    // and interning events by name merged those into one carrier - so a sequence edge could be drawn
    // between two different emissions as though one payload had listed them. Attributes are the
    // opposite: there is only one value for a key on a span, and its several extracted roots are one
    // ordered payload. Splitting those roots lost the sequence in ADK's
    // `gcp.vertex.agent.llm_request` and LangGraph's `output.value`.
    let mut carriers: HashMap<PayloadKey<'_>, usize> = HashMap::new();
    let mut input_families: HashMap<(String, String), usize> = HashMap::new();
    blocks
        .iter()
        .map(|block| {
            let credible = is_credible_emission(block);
            let next_span = spans.len();
            let span = *spans.entry(block.span_id.as_str()).or_insert(next_span);
            // Ancestors interned eagerly, since a parent may be met after its child.
            let ancestor_spans: Vec<usize> = block
                .span_path
                .iter()
                .take(block.span_path.len().saturating_sub(1))
                .map(|ancestor| {
                    let next = spans.len();
                    *spans.entry(ancestor.as_str()).or_insert(next)
                })
                .collect();
            // One resolution per observation, and every fact this loop needs comes off it. The clause
            // carries the facts *and* the ordering family, so asking twice - once for each - looked up
            // the same declaration twice and, worse, allowed the two answers to come from different
            // clauses if the context ever differed between the calls.
            let clause = crate::rules::ruleset()
                .carriers
                .resolve(&block.carrier_context());
            let semantics = clause
                .map_or(crate::sideml::carrier::CarrierSemantics::SNAPSHOT, |c| {
                    c.semantics
                });
            let payload_root = block
                .position
                .to_string()
                .split('.')
                .next()
                .unwrap_or("")
                .to_string();
            let payload_instance = if block.source_attribute.is_some() {
                String::new()
            } else {
                payload_root
            };
            let next_carrier = carriers.len();
            let carrier = *carriers
                .entry((
                    block.span_id.as_str(),
                    block.event_name.as_deref(),
                    block.source_attribute.as_deref(),
                    payload_instance,
                ))
                .or_insert(next_carrier);
            let emission = credible.then(|| {
                let next = instances.len();
                *instances
                    .entry((block.span_id.clone(), emission_scope(block)))
                    .or_insert(next)
            });
            // Two fragmented input families, each measured on its own, deliberately not every ordered
            // input carrier. Broadening to Vercel's `ai.prompt` regressed a sequential two-step
            // trace, measured: request 2's array lists `call1, result1` and its first-seen
            // sequencing pulled the second step's call ahead of the first step's result -
            // `call1, result1, call2, result2` became `call1, call2, result1, result2`, which
            // misrepresents the causality the old order showed. Each further family needs its own
            // measured pass, exactly like every other promotion in this module.
            //
            // The event-stream form of the same fragmentation (`gen_ai.system.message`,
            // `gen_ai.user.message` interning apart on one generation span) was implemented and
            // reverted: it fired thousands of forward no-op edges, created no new cycles, and fixed
            // nothing - for `strands/swarm`, the one case it was aimed at, the target does not exist
            // as an orderable unit. Strands rewrites the question before the model sees it
            // (`Context: User Request: ...`), so no identity links the surviving question to any
            // request, and the wrapped copy is correctly filtered as a context echo. A constraint
            // class with no measured repair is surface without benefit.
            // Which fragmented ordered-input family this observation belongs to is a *declared* carrier
            // fact now, not a key comparison here. It used to read
            // `source_attribute.starts_with("llm.input_messages")` - one framework's spelling, written
            // into the resolver, where no rule file could state it and nothing but this comment
            // recorded that the family was deliberately narrow.
            let declared_family = clause.and_then(|c| c.ordering_family.as_deref());
            let ordered_input = block.is_generation_span()
                && semantics.position_provides_sequence_order
                && !semantics.carrier_holds_span_output
                && !semantics.carrier_is_detached_request_frame
                && declared_family.is_some();
            let input_family = declared_family.filter(|_| ordered_input).map(|family| {
                // Grouped by the **declared family name**, not by a name derived from the key.
                //
                // Deriving it stripped the first dotted digit off the carrier, so
                // `llm.input_messages.0.message` and `.1.message` grouped together - correct here, and a
                // heuristic that mis-groups any carrier whose name happens to contain an earlier
                // version-like segment. The declaration already says which array a member belongs to, so
                // reading its `is_some()` and then re-deriving the answer was using half a fact.
                let next = input_families.len();
                *input_families
                    .entry((block.span_id.clone(), family.to_string()))
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
                accumulator: block.is_accumulator_span(),
                ancestor_spans,
                detached_frame: semantics.carrier_is_detached_request_frame,
                tool_reference: block
                    .tool_use_id
                    .as_ref()
                    .filter(|id| !id.is_empty())
                    .and_then(|id| match block.entry_type.as_str() {
                        "tool_use" => Some(ToolReference::Call(id.clone())),
                        "tool_result" => Some(ToolReference::Result(id.clone())),
                        _ => None,
                    }),
                input_family,
            }
        })
        .collect()
}

/// Which emission of its span an observation belongs to.
///
/// Two different scopes, because frameworks split one response two different ways:
///
/// - An **event** carrier, or an array under one attribute, is one payload per instance, and a span
///   can emit `gen_ai.choice` more than once - so the scope is the root of the position path, which
///   distinguishes those instances.
/// - A **split** attribute carrier spreads one response across sibling keys: Vercel writes
///   `ai.response.text` beside `ai.response.toolCalls`, and those have different position roots while
///   being one emission. So the scope is the attribute's *family* - the key with its last segment
///   dropped - which binds the siblings without merging every output the span produced.
///
/// `(span, direction)` was the tempting generalisation and it overreaches: one span can hold several
/// emissions, and duplicate output forms of the same one.
fn emission_scope(block: &BlockEntry) -> String {
    let payload_root = || {
        block
            .position
            .to_string()
            .split('.')
            .next()
            .unwrap_or("")
            .to_string()
    };
    match block.source_attribute.as_deref() {
        // A family only where the key actually has one: `ai.response.text` -> `ai.response`. A
        // single-segment key is its own family.
        Some(attribute) => match attribute.rsplit_once('.') {
            Some((family, _)) if !family.is_empty() => format!("attr:{family}"),
            _ => format!("attr:{attribute}"),
        },
        None => format!("event:{}", payload_root()),
    }
}

/// Whether an observation is evidence of *when* a message happened and part of *one emission*.
///
/// An atomic emission the span itself produced: `gen_ai.choice`, and only when the span is the
/// emitter (`is_output_source`), never a received copy or a re-listed snapshot. A framework handing
/// a past result back to the model carries an emission-shaped carrier but its time is the hand-back,
/// not the occurrence — reading it as evidence moved a result ahead of its call.
fn is_credible_emission(block: &BlockEntry) -> bool {
    block.is_output_source()
        && crate::sideml::carrier::semantics_for_context(&block.carrier_context())
            .carrier_is_atomic_emission
}

/// Which emission instances are a *redundant re-listing* - present, but with no authority over order.
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
fn redundant_relistings(
    evidence: &[OrderEvidence],
    survivors: &[BlockEntry],
    survivor_of: &impl Fn(usize) -> Option<usize>,
) -> HashSet<usize> {
    // Where each instance sits, and what it claims.
    let mut instance_span: HashMap<usize, usize> = HashMap::new();
    let mut instance_accumulator: HashMap<usize, bool> = HashMap::new();
    let mut instance_ancestors: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut claimed: HashMap<usize, HashSet<usize>> = HashMap::new();
    for (observation, seen) in evidence.iter().enumerate() {
        let Some(instance) = seen.emission else {
            continue;
        };
        instance_span.insert(instance, seen.span);
        instance_accumulator.insert(instance, seen.accumulator);
        instance_ancestors.insert(instance, seen.ancestor_spans.clone());
        if let Some(survivor) = survivor_of(observation) {
            claimed.entry(instance).or_default().insert(survivor);
        }
    }
    let mut out = HashSet::new();
    for (&instance, members) in &claimed {
        if members.is_empty() || instance_accumulator.get(&instance) != Some(&true) {
            continue;
        }
        let Some(&span) = instance_span.get(&instance) else {
            continue;
        };
        // A witness is another instance on a span strictly below this one.
        let witnessed = |survivor: &usize| {
            claimed.iter().any(|(&other, others)| {
                other != instance
                    && others.contains(survivor)
                    && instance_ancestors
                        .get(&other)
                        .is_some_and(|ancestors| ancestors.contains(&span))
            })
        };
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
        if !(holds_own_output && holds_an_answer) {
            continue;
        }
        if members.iter().all(witnessed) {
            out.insert(instance);
        }
    }
    out
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
/// What the evidence says must come before what, over *blocks*, as a relation rather than a sequence.
///
/// The resolver's answer is one linearisation of its own graph: a topological order chosen by time and
/// legacy index among the many the constraints permit. That choice is presentation. Anything reasoning
/// about causality has to ask the relation instead - and not the resolver's graph, for two reasons that
/// both come from what that graph is *for*.
///
/// **It is over units, not blocks.** An emission is contracted to one node, so an edge into or out of it
/// speaks for every block inside it: `call_a -> result_a` becomes `emission -> result_a`, which also
/// asserts `call_b -> result_a`. Right for presentation, because an emission is atomic and stays
/// contiguous - and wrong here, because a provider's conversation history is exactly what splits an
/// emission apart, writing each call beside its own result.
///
/// **It includes generation dataflow**, whose input side deliberately keeps replayed history. "The tool
/// result this answer cites came first" is the right reading there and a false statement about global
/// order, as that class documents. Dedup then makes it self-contradictory: a span whose input replays
/// the calls it is about to re-emit has them collapsed onto the same survivors, so the graph acquires
/// `call_b -> barrier -> call_a` for two calls of one emission.
///
/// So this is built from the three classes that describe one emission and one payload, at block
/// granularity: an emission's own sequence, exact call to result, and a carrier's stated order. Indices
/// are into the survivor slice - the same indexing the pre-resolve transcript uses.
#[derive(Debug, Clone, Default)]
pub(super) struct Precedence {
    /// Reverse adjacency over survivor indices.
    predecessors: Vec<Vec<u32>>,
    /// Forward adjacency, for looking one step ahead when choosing between interchangeable candidates.
    successors: Vec<Vec<u32>>,
}

impl Precedence {
    /// A relation stated directly as `(before, after)` pairs over block indices, for tests that need to
    /// exercise the matcher against a shape no captured fixture contains.
    #[cfg(test)]
    pub(super) fn from_edges(blocks: usize, edges: &[(usize, usize)]) -> Self {
        let mut predecessors = vec![Vec::new(); blocks];
        let mut successors = vec![Vec::new(); blocks];
        for &(before, after) in edges {
            if before < blocks && after < blocks {
                predecessors[after].push(before as u32);
                successors[before].push(after as u32);
            }
        }
        Self {
            predecessors,
            successors,
        }
    }

    /// What this block immediately precedes. Used to look one step ahead, not to reason about order.
    pub(super) fn successors_of(&self, block: usize) -> &[u32] {
        self.successors.get(block).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Everything that must precede `b`, accumulated into `seen` and skipping what it already holds.
    ///
    /// Amortised: a matcher checking many candidates against a growing set of matched occurrences pays
    /// for each block and edge once in total, not once per query.
    pub(super) fn collect_ancestors(&self, b: usize, seen: &mut HashSet<u32>) {
        if b >= self.predecessors.len() {
            return;
        }
        let mut stack = vec![b as u32];
        while let Some(node) = stack.pop() {
            for &p in self.predecessors.get(node as usize).into_iter().flatten() {
                if seen.insert(p) {
                    stack.push(p);
                }
            }
        }
    }
}

/// Which ordered thing a sequence edge came from. Both state an order over their own members, and
/// neither says anything about the other's, so they are collected separately and keyed apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum SequenceSource {
    /// One emission instance: the blocks a single response produced, in the order it produced them.
    Emission(usize),
    /// One carrier: the order a payload states between its own observations.
    Carrier(usize),
}

/// Exact call/result pairs recovered from every observation that dedup projected to each survivor.
///
/// A representative may be an id-less snapshot even though another copy carried a correlated id.
/// Reading the evidence makes the edge independent of that quality tie. Distinct ids or callers
/// remain ambiguous and deliberately produce no pair.
fn exact_tool_pairs(
    evidence: &[OrderEvidence],
    survivors: &[BlockEntry],
    lineage: &[Option<usize>],
    repeat_ordinals: &[u32],
) -> HashSet<(usize, usize)> {
    type CallKey = (String, u32);

    let mut calls: HashMap<CallKey, HashSet<usize>> = HashMap::new();
    let mut result_ids: HashMap<usize, HashSet<String>> = HashMap::new();
    for (survivor, block) in survivors.iter().enumerate() {
        let Some(id) = block.tool_use_id.as_ref().filter(|id| !id.is_empty()) else {
            continue;
        };
        let ordinal = repeat_ordinals.get(survivor).copied().unwrap_or(0);
        if block.entry_type == "tool_use" {
            calls
                .entry((id.clone(), ordinal))
                .or_default()
                .insert(survivor);
        } else if block.entry_type == "tool_result" {
            result_ids.entry(survivor).or_default().insert(id.clone());
        }
    }
    for (observation, seen) in evidence.iter().enumerate() {
        let Some(survivor) = lineage.get(observation).copied().flatten() else {
            continue;
        };
        let ordinal = repeat_ordinals.get(survivor).copied().unwrap_or(0);
        match &seen.tool_reference {
            Some(ToolReference::Call(id)) => {
                calls
                    .entry((id.clone(), ordinal))
                    .or_default()
                    .insert(survivor);
            }
            Some(ToolReference::Result(id)) => {
                result_ids.entry(survivor).or_default().insert(id.clone());
            }
            None => {}
        }
    }

    let mut pairs = HashSet::new();
    for (result, ids) in result_ids {
        if survivors
            .get(result)
            .is_none_or(|block| block.entry_type != "tool_result")
            || ids.len() != 1
        {
            continue;
        }
        let ordinal = repeat_ordinals.get(result).copied().unwrap_or(0);
        let Some(id) = ids.into_iter().next() else {
            continue;
        };
        let Some(callers) = calls
            .get(&(id, ordinal))
            .filter(|callers| callers.len() == 1)
        else {
            continue;
        };
        let Some(&call) = callers.iter().next() else {
            continue;
        };
        pairs.insert((call, result));
    }
    pairs
}

/// Build the block-level causal relation over a trace's survivors - see [`Precedence`] for why it is
/// not the resolver's graph.
pub(super) fn causal_precedence(
    evidence: &[OrderEvidence],
    survivors: &[BlockEntry],
    lineage: &[Option<usize>],
    repeat_ordinals: &[u32],
) -> Precedence {
    let n = survivors.len();
    let mut predecessors: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut successors: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut add = |from: usize,
                   to: usize,
                   predecessors: &mut Vec<Vec<u32>>,
                   successors: &mut Vec<Vec<u32>>| {
        if from != to && from < n && to < n && edges.insert((from as u32, to as u32)) {
            predecessors[to].push(from as u32);
            successors[from].push(to as u32);
        }
    };
    let survivor_of = |observation: usize| lineage.get(observation).copied().flatten();

    // An emission's own sequence, and a carrier's stated order: the same shape, so collected together.
    // Consecutive members only - the transitive closure is what the walk computes.
    let mut sequences: HashMap<SequenceSource, Vec<EmissionMember>> = HashMap::new();
    for (observation, seen) in evidence.iter().enumerate() {
        let Some(survivor) = survivor_of(observation) else {
            continue;
        };
        if let Some(instance) = seen.emission {
            sequences
                .entry(SequenceSource::Emission(instance))
                .or_default()
                .push((seen.message_index, seen.entry_index, survivor));
        }
        if seen.carrier_ordered {
            sequences
                .entry(SequenceSource::Carrier(seen.carrier))
                .or_default()
                .push((seen.message_index, seen.entry_index, survivor));
        }
    }
    let mut keys: Vec<SequenceSource> = sequences.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        let mut members = sequences.remove(&key).unwrap_or_default();
        members.sort_unstable();
        let mut sequence: Vec<usize> = Vec::with_capacity(members.len());
        for (_, _, survivor) in members {
            if sequence.last() != Some(&survivor) {
                sequence.push(survivor);
            }
        }
        for pair in sequence.windows(2) {
            add(pair[0], pair[1], &mut predecessors, &mut successors);
        }
    }

    for (call, result) in exact_tool_pairs(evidence, survivors, lineage, repeat_ordinals) {
        add(call, result, &mut predecessors, &mut successors);
    }

    Precedence {
        predecessors,
        successors,
    }
}

/// Which constraints the resolver is allowed to *change the answer* with.
///
/// This is the promotion dial. `NEUTRAL` builds the whole graph and runs the whole resolve while
/// enforcing nothing, so its output is provably the legacy order — the proof that the machinery
/// cannot move anything on its own, kept checkable as classes are promoted.
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
    /// Express generation dataflow as the product of a span's inputs and outputs, rather than through
    /// a barrier node.
    ///
    /// The two must produce the same order - the barrier exists only to make the edge count linear in a
    /// span's messages instead of quadratic - and `a_barrier_orders_exactly_as_pairwise_edges_do`
    /// compares them across the corpus. Production uses the barrier.
    pub pairwise_dataflow_edges: bool,
    /// Enforce that what a generation span *received* precedes what it *produced*.
    ///
    /// The minimal turn structure, and deliberately local dataflow rather than a rule about roles: a
    /// model call's input caused its output, so the system prompt and the tool results a call was given
    /// precede the answer it produced, and transitivity carries that across spans. A global rule like
    /// "the terminal assistant message follows the last user message and every intervening tool" says
    /// something similar and is false for parallel branches, subagents, retries and abandoned calls.
    pub generation_dataflow_edges: bool,
    /// Enforce that a *detached request frame* precedes every other input of the request that carried
    /// it.
    ///
    /// The one confirmed ordering defect: a framework reports the instruction on the span that sent it -
    /// the generation span - while the question arrived on an orchestration span that started earlier,
    /// so ordering by evidence time put the frame after the question. Two scalar repairs failed,
    /// measured (one tripped `assert_carrier_subsequence` on ADK, the other moved 15 feed views and
    /// repaired nothing), because "before" here is a constraint, not a position.
    ///
    /// The request is the generation span that carried the frame, and the edge is drawn to the other
    /// *input* units of that same span, projected through lineage - so the surviving copy of the
    /// question, wherever it sits, inherits the edge. Scoped to one request, deliberately: a trace
    /// legitimately holds several instructions (`adk/reasoning` repeats one at three request
    /// boundaries; `adk/image_gen` changes it mid-trace), and any wider scope is falsified by a
    /// committed fixture.
    pub request_framing_edges: bool,
    /// Complete one unambiguous tool turn whose terminal generation ties its sibling tool spans.
    ///
    /// OpenTelemetry JavaScript records start times at millisecond precision. A fast
    /// `generation -> tool -> generation` turn can therefore place every sibling in one millisecond,
    /// or the first generation in the preceding millisecond and the tool/final siblings together in
    /// the next. The database then falls back to their random span ids. The payloads still state the
    /// causal shape: one generation finished with `tool_use`, one or more tool spans each carry an
    /// exact call/result pair, and one generation finished normally.
    ///
    /// This is deliberately narrower than a global role rule. It applies only when one sibling wave
    /// has exactly two generation outputs (one tool-use preamble and one terminal answer), every
    /// included tool edge has an exact id on one tool span, and the two generation spans differ.
    /// Parallel generations, retries, abandoned calls and ambiguous ids add no edge.
    pub sibling_tool_turn_edges: bool,
}

impl Constraints {
    /// Provably output-neutral: every constraint is built and resolved, none can move a block.
    ///
    /// Kept as its own configuration after promotions begin, because it is what
    /// `the_neutral_resolver_reproduces_the_legacy_order` tests: the proof that the machinery cannot
    /// move anything on its own has to stay checkable, or a promotion could hide a resolver bug.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) const NEUTRAL: Self = Self {
        pairwise_dataflow_edges: false,
        contract_non_contiguous_emissions: false,
        enforce_backward_edges: false,
        time_priority: false,
        source_position_member_order: false,
        carrier_sequence_edges: false,
        generation_dataflow_edges: false,
        request_framing_edges: false,
        sibling_tool_turn_edges: false,
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
    /// - **Generation dataflow**, with direction read from the carrier declaration: what a model call
    ///   received precedes what it produced. This is the class that pins a tool result before the
    ///   answer citing it - no other class states that, because the consuming span is the only place
    ///   the two meet - and it is what cleared all ten `PerCarrier` reorders, so
    ///   `REORDERS_UNDER_PER_CARRIER` is now empty.
    ///
    ///   History is kept on its input side, unlike carrier-sequence edges, because the two ask
    ///   different questions of a re-send: "these precede what this generation produced" is true of a
    ///   replay; "these are in this relative order globally" is not, and that reading dragged ADK's
    ///   second system prompt to the front of a session.
    ///
    /// - **Occurrence anchors** (`time_priority`): a unit sorts at the earliest time its *evidence*
    ///   gives it, and ties break on where it was **first observed** across every copy - not on the
    ///   surviving block's index in the previous sort, which is what survivor choice could move.
    ///
    ///   This needed the emission scope generalised first. Vercel spreads one response across sibling
    ///   attributes (`ai.response.text` beside `ai.response.toolCalls`), which have different position
    ///   roots, so contraction could not hold that response together and promoting time alone displaced
    ///   its intro text behind its own calls. `emission_scope` now uses the attribute *family* for split
    ///   carriers and the payload root for event carriers - `(span, direction)` was the tempting
    ///   generalisation and overreaches, since one span can hold several emissions.
    ///
    ///   Effect, measured: two fixtures, both repairs. An `adk/tool_use` span view showed both tool
    ///   *results before* both calls and now reads `call, result, call, result`; the whole-view
    ///   causality invariant had missed it because span views are exempt (a span usually holds only one
    ///   half of a pair) and this span holds both. `agent-framework/swarm` stops batching all three
    ///   specialists' system prompts ahead of all three answers and interleaves each prompt with its own
    ///   agent's reply.
    ///
    /// - **Quantised sibling tool turns**: one complete, unambiguous sibling turn is ordered by its
    ///   payload causality when the terminal generation ties its tool spans. OpenTelemetry JavaScript
    ///   timestamps fast spans only to the millisecond; the database then falls back to random span
    ///   ids and the same conversation changes order between captures. The promoted rule requires
    ///   exactly one tool-use generation, one terminal generation, and exact call/result pairs carried
    ///   by the same tool spans. It changed only the two JavaScript conformance views, making SDK and
    ///   raw OTel both read `user, preamble, call, result, final`; all 125 pre-existing tracked
    ///   expectations were byte-identical after regeneration.
    ///
    /// Every class is now promoted. What remains is not a dial:
    ///
    /// - **The dataflow class no longer emits a product anywhere.** It used to, where a span's input and
    ///   output sets overlap - a span re-sending a message it also produced - because one barrier cannot
    ///   express `u -> b` for a shared `u` without also asserting `u -> barrier -> u`. That branch was
    ///   documented as unreachable and is not: **six corpus spans** take it, the largest with 6 inputs
    ///   and 2 outputs, so the quadratic growth the barrier exists to remove was live. And for two or
    ///   more shared units the product is *self-contradictory* - it contains `u -> v` and `v -> u` - so
    ///   the resolver broke a cycle the code had manufactured.
    ///
    ///   Two barriers express the consistent part linearly: `(only_in ∪ shared) -> only_out` and
    ///   `only_in -> shared`, which is everything except the shared-to-shared pairs. Omitting those is
    ///   the honest reading rather than a concession - a unit a span both received and produced is a
    ///   replay, and one span's dataflow says nothing about where two such units sit relative to each
    ///   other. On that largest span it is 6 edges against 12.
    ///
    ///   Measured: the corpus equivalence with the product holds
    ///   (`a_barrier_orders_exactly_as_pairwise_edges_do`), and the cycle counts of the two pinned
    ///   contradicting fixtures are unchanged, so nothing on the corpus depended on the manufactured
    ///   cycle. The contradiction itself is proven on a constructed span
    ///   (`an_overlapping_generation_span_is_ordered_without_a_manufactured_cycle`), which is also what
    ///   keeps the overlap branch exercised - no fixture reaches two shared units through the golden
    ///   path.
    ///
    /// Replay matching used to be on this list. It is now injective matching against
    /// [`causal_precedence`], which is a *different* relation from this graph on purpose - see that
    /// type for why a presentation graph cannot answer a causality question.
    pub(super) const PRODUCTION: Self = Self {
        pairwise_dataflow_edges: false,
        contract_non_contiguous_emissions: true,
        enforce_backward_edges: true,
        time_priority: true,
        source_position_member_order: true,
        carrier_sequence_edges: true,
        generation_dataflow_edges: true,
        request_framing_edges: true,
        sibling_tool_turn_edges: true,
    };

    /// Every constraint enforced - the redesign's intended answer.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) const FULL: Self = Self {
        pairwise_dataflow_edges: false,
        contract_non_contiguous_emissions: true,
        enforce_backward_edges: true,
        time_priority: true,
        source_position_member_order: true,
        carrier_sequence_edges: true,
        generation_dataflow_edges: true,
        request_framing_edges: true,
        sibling_tool_turn_edges: true,
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
    edges: &mut std::collections::HashSet<(usize, usize)>,
) {
    if from == to {
        return;
    }
    let backward = unit_min_legacy[&from] > unit_min_legacy[&to];
    if backward && !constraints.enforce_backward_edges {
        return;
    }
    // Set membership, not a linear scan of the successor list: a generation span with many inputs and
    // many outputs produces their product in edges, and checking each against a growing `Vec` made
    // building the graph quadratic in that product.
    if edges.insert((from, to)) {
        successors.get_mut(&from).expect("unit present").push(to);
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
    let inside: HashSet<usize> = members.iter().copied().collect();
    let mut successors: HashMap<usize, Vec<usize>> =
        members.iter().map(|&m| (m, Vec::new())).collect();
    let mut indegree: HashMap<usize, usize> = members.iter().map(|&m| (m, 0)).collect();
    // A set rather than `succ.contains(&to)`, which was a linear scan of the adjacency list per edge and so
    // quadratic in a member's degree. The deduplication itself is load-bearing: a repeated edge would
    // otherwise raise the indegree twice and the target would never be released.
    let mut seen_edges: HashSet<(usize, usize)> = HashSet::new();
    for &(from, to) in intra_edges {
        if !inside.contains(&from) || !inside.contains(&to) {
            continue;
        }
        if !seen_edges.insert((from, to)) {
            continue;
        }
        successors.get_mut(&from).expect("member present").push(to);
        *indegree.get_mut(&to).expect("member present") += 1;
    }

    // Kahn's algorithm with a min-heap, which is the same selection rule as before - "the smallest member
    // still having indegree zero" - without rescanning every remaining member to find it. The previous loop
    // was a filter plus a `min_by_key` plus a `retain` per step, so O(members^2) for a unit that is one long
    // chain.
    let mut ready: BinaryHeap<Reverse<usize>> = members
        .iter()
        .filter(|m| indegree[m] == 0)
        .map(|&m| Reverse(m))
        .collect();
    let mut out: Vec<usize> = Vec::with_capacity(members.len());
    while let Some(Reverse(next)) = ready.pop() {
        out.push(next);
        for &s in &successors[&next] {
            if let Some(d) = indegree.get_mut(&s) {
                *d = d.saturating_sub(1);
                if *d == 0 {
                    ready.push(Reverse(s));
                }
            }
        }
    }

    if out.len() != members.len() {
        // Cycle: the emissions contradict each other about this unit. Source order stands, exactly as it did
        // when the old loop found no zero-indegree member left.
        return members.to_vec();
    }
    out
}

#[cfg(test)]
#[path = "order_graph_cycle_tests.rs"]
mod cycle_tests;

/// The indexed member ordering against the implementation it replaced.
///
/// Step 4 of the platform plan requires its rewrites to be *provably* answer-preserving, not merely to pass
/// the goldens - a golden covers the shapes the corpus happens to hold, and the interesting inputs here are
/// edge sets no captured framework produces. So the retired implementation is kept as an oracle and the two
/// are compared over generated graphs, which is the same discipline this repository applies to its retired
/// SQL.
///
/// Pure `usize` data, so the generator can be exhaustive about the cases that matter: multi-edges, edges
/// pointing outside the member set, self-loops, and cycles - where both are required to fall back to source
/// order rather than to *some* order.
#[cfg(test)]
#[path = "order_graph_order_within_unit_equivalence_tests.rs"]
mod order_within_unit_equivalence;
