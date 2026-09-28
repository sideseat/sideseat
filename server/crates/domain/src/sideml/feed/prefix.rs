use super::*;

/// What earlier traces of this session already showed, as a relation rather than a sequence.
///
/// A framework re-sends the conversation so far as the input of its next turn, and the same turn does
/// not come back in the order it was emitted: a provider serialises a turn's *parallel* tool calls into
/// a linear message list, so `call, call, result, result` is replayed as `call, result, call, result`.
/// Matching that against a stored linearisation with one forward cursor reads as a mismatch at the
/// second call - and since a mismatch ends the prefix, everything after it leaks. `adk/tool_use` is the
/// corpus witness: its second trace re-executes the first turn, and the session view showed that turn
/// twice, the second time in the provider's order.
///
/// So the prefix is stored as each prior trace's *precedence relation* plus the occurrences it holds,
/// and a replay is accepted when it is some linear extension of that relation - which is what a
/// provider's serialisation is. Injectively: each replayed block consumes one distinct prior
/// occurrence, so a turn holding two identical calls is matched by two, never twice by one.
///
/// Across traces the relation is the trace order itself. Traces of a session are successive turns, so a
/// replay listing turn 2's messages before turn 1's is not a linear extension of anything - and using
/// the sequence there costs nothing, because history is replayed in order between turns even when it is
/// reordered within one.
#[derive(Debug, Default)]
pub(super) struct CrossTracePrefixState {
    entries: Vec<PriorOccurrence>,
    /// Where each `(role, content hash)` occurs among the entries, so a candidate lookup does not scan.
    by_identity: HashMap<(crate::sideml::types::ChatRole, String), Vec<usize>>,
    /// One relation per contributing trace, indexed by that trace's transcript position.
    relations: Vec<order_graph::Precedence>,
    /// Each trace's transcript identities, by position - what a relation's nodes *are*, which the relation
    /// itself does not know. Read only to look one step ahead when choosing between candidates that
    /// nothing else distinguishes.
    identities: Vec<Vec<(crate::sideml::types::ChatRole, String)>>,
}

/// One occurrence an earlier trace established, and where to find it in that trace's relation.
#[derive(Debug)]
struct PriorOccurrence {
    trace: usize,
    /// Index into the trace's transcript, which is what its `Precedence` is indexed by.
    position: usize,
}

impl CrossTracePrefixState {
    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Add a trace's transcript and the relation the resolver derived over it.
    ///
    /// System blocks are skipped: a system prompt is per-trace framing that every turn re-sends, so it
    /// is not evidence of history and matching it would consume a prefix entry for nothing.
    pub(super) fn push_trace(
        &mut self,
        transcript: &[BlockEntry],
        relation: order_graph::Precedence,
    ) {
        let trace = self.relations.len();
        self.relations.push(relation);
        self.identities.push(
            transcript
                .iter()
                .map(|block| (block.role, block.content_hash.clone()))
                .collect(),
        );
        for (position, block) in transcript.iter().enumerate() {
            if block.role == crate::sideml::types::ChatRole::System {
                continue;
            }
            let entry = self.entries.len();
            self.entries.push(PriorOccurrence { trace, position });
            self.by_identity
                .entry((block.role, block.content_hash.clone()))
                .or_default()
                .push(entry);
        }
    }

    /// The longest prefix of `replay` that can be matched to distinct prior occurrences, in an order the
    /// evidence permits.
    ///
    /// Returns the entry each matched block claimed. `replay` is one span's strippable blocks in payload
    /// order, as `(role, content hash)`.
    ///
    /// # Why this searches instead of choosing
    ///
    /// Taking the first locally permitted candidate is wrong, and not subtly. Two unordered branches
    /// establish `callA -> resultA` and `callB -> resultB`, and the two results carry the same identity -
    /// two tools that both answered `"ok"`, which is ordinary. Replayed as `callB, resultB, callA,
    /// resultA`, a valid linear extension, the greedy step matches `resultB` against *`resultA`* because
    /// it comes first among equals. That assignment then requires `callA` to have come earlier, `callA` is
    /// refused, and the prefix ends: the rest of the turn is duplicated in the session. Choosing
    /// `resultB` instead matches everything. The identities are interchangeable, so only the order
    /// constraints can tell the two choices apart, and that is a search.
    ///
    /// Bounded, because a matching problem with interchangeable candidates is exponential in the worst
    /// case. `MATCH_BUDGET` caps the assignments tried; exceeding it returns the longest prefix found so
    /// far, which under-strips rather than over-strips - the failure a user sees as duplicates rather
    /// than as missing messages. In practice the first candidate is right and the search is linear: the
    /// budget is never approached by any corpus fixture.
    pub(super) fn longest_matching_prefix(
        &self,
        replay: &[(crate::sideml::types::ChatRole, &str)],
    ) -> (Vec<usize>, bool) {
        /// Assignments tried before the search gives up and reports what it has.
        const MATCH_BUDGET: u32 = 20_000;

        let mut search = PrefixSearch {
            state: self,
            consumed: vec![false; self.entries.len()],
            consumed_bits: vec![0u64; self.entries.len().div_ceil(64)],
            must_precede: HashMap::new(),
            chosen: Vec::with_capacity(replay.len()),
            best: Vec::new(),
            failed: HashSet::new(),
            budget: MATCH_BUDGET,
        };
        search.extend(replay, 0);
        let exhaustive = search.budget > 0;
        if !exhaustive {
            tracing::warn!(
                replay = replay.len(),
                prior = self.entries.len(),
                matched = search.best.len(),
                "Cross-trace replay matching hit its budget; some history may be shown twice"
            );
        }
        (search.best, exhaustive)
    }
}

/// One depth-first search for the longest matchable prefix of a replay.
///
/// State is mutated and undone as the search backtracks, so each block and edge of a trace's relation is
/// walked once per path rather than once per candidate.
struct PrefixSearch<'a> {
    state: &'a CrossTracePrefixState,
    /// Which prior occurrences the current partial assignment has claimed. Injectivity, and what keeps a
    /// genuinely repeated question from collapsing onto the first ask.
    consumed: Vec<bool>,
    /// `consumed` as a bitset, maintained **incrementally** rather than rebuilt.
    ///
    /// The signature is taken once per recursion, and rebuilding it walked all `N` prior occurrences each
    /// time - so a trace replaying `N` messages did `N` scans of `N` entries, and a replaying session (where
    /// every generation span re-sends the whole conversation) trended cubic in turn count. The assignment
    /// budget does not bound this, because it counts *assignments*, not the work spent describing a state.
    /// Flipping two words on claim and release keeps the signature exact - no hashing, so no collision can
    /// make a dead end look already-explored - at 1/64th of the reads.
    consumed_bits: Vec<u64>,
    /// Per trace, everything that must precede something already matched.
    must_precede: HashMap<usize, HashSet<u32>>,
    chosen: Vec<usize>,
    best: Vec<usize>,
    /// States already known not to extend to a full match, as `(replay position, claimed occurrences)`.
    ///
    /// Without this the search re-explores the same dead end once per path that reaches it, which is what
    /// made a budget necessary at all: nine interchangeable calls have `9!` orderings of the same set, and
    /// every one of them fails identically. With it each distinct state is explored once, so the shapes
    /// that used to exhaust the budget finish immediately - and the budget becomes a guard against
    /// pathological input rather than the thing that decides the answer.
    failed: HashSet<(usize, Vec<u64>)>,
    budget: u32,
}

impl PrefixSearch<'_> {
    fn extend(&mut self, replay: &[(crate::sideml::types::ChatRole, &str)], min_trace: usize) {
        if self.chosen.len() > self.best.len() {
            self.best = self.chosen.clone();
        }
        let Some(&(role, content_hash)) = replay.get(self.chosen.len()) else {
            return; // the whole replay matched
        };
        // Seen this exact state fail before? Then it fails again - which set of occurrences has been
        // claimed is all that matters, not the order they were claimed in.
        let signature = (self.chosen.len(), self.consumed_signature());
        if self.failed.contains(&signature) {
            return;
        }
        let Some(candidates) = self
            .state
            .by_identity
            .get(&(role, content_hash.to_string()))
        else {
            return; // nothing prior looks like this block, so the prefix ends here
        };
        // Permitted candidates, most-constrained first.
        //
        // The order matters as much as the search does, because the budget is finite. Ten independent
        // branches whose results are all `"ok"`, replayed in reverse, give every step ten permitted
        // candidates that differ only in which call they answer - and taking them in stored order picks
        // the wrong one nine times out of ten, so the search spends its whole budget backtracking and
        // gives up part way through a replay it should have matched entirely.
        //
        // "Most constrained" is: how many of the candidate's own ancestors are still unmatched. The one
        // whose call the replay has *just* matched has none, and it is exactly the right choice, so the
        // common shapes are matched first-try and the search never branches. This is a heuristic on the
        // order of exploration, not on the answer: what is permitted is unchanged, so a shape the
        // heuristic guesses wrong is still found by backtracking.
        let mut permitted: Vec<(u8, usize, usize)> = Vec::new();
        for &entry in candidates {
            if self.consumed[entry] {
                continue;
            }
            let occurrence = &self.state.entries[entry];
            if occurrence.trace < min_trace {
                continue; // a later turn's message cannot be replayed before an earlier turn's
            }
            // Would taking this candidate claim an order the evidence contradicts? It would exactly when
            // the candidate must precede something the replay has already matched.
            if self
                .must_precede
                .get(&occurrence.trace)
                .is_some_and(|ancestors| ancestors.contains(&(occurrence.position as u32)))
            {
                continue;
            }
            permitted.push((
                // Does taking this candidate line up with what the replay says comes next? Zero first.
                self.lookahead_cost(replay, occurrence.trace, occurrence.position),
                self.unmatched_ancestors(occurrence.trace, occurrence.position),
                entry,
            ));
        }
        permitted.sort_unstable();

        for (_, _, entry) in permitted {
            if self.budget == 0 {
                return;
            }
            let occurrence = &self.state.entries[entry];

            self.budget -= 1;
            self.consumed[entry] = true;
            self.consumed_bits[entry / 64] |= 1u64 << (entry % 64);
            self.chosen.push(entry);
            let added = self.add_ancestors(occurrence.trace, occurrence.position);

            self.extend(replay, occurrence.trace);

            for node in added {
                self.must_precede
                    .get_mut(&occurrence.trace)
                    .expect("the set this step added to")
                    .remove(&node);
            }
            self.chosen.pop();
            self.consumed[entry] = false;
            self.consumed_bits[entry / 64] &= !(1u64 << (entry % 64));

            if self.best.len() == replay.len() {
                return; // nothing beats a full match
            }
        }

        // Every candidate was tried and none led to a full match, so this state never will.
        if self.best.len() < replay.len() {
            self.failed.insert(signature);
        }
    }

    /// The set of claimed occurrences, as a bitset - the part of the state that decides whether a dead
    /// end is the same dead end.
    ///
    /// A copy of the incrementally-maintained `consumed_bits`, not a fresh walk of `consumed`: see that
    /// field for why rebuilding it made the search quadratic in the replay length.
    fn consumed_signature(&self) -> Vec<u64> {
        self.consumed_bits.clone()
    }

    /// Whether this candidate's immediate successors include what the replay asks for next: `0` if they
    /// do, `1` if they do not.
    ///
    /// One step of lookahead, and it separates candidates that nothing else can. A tool call's identity
    /// excludes the provider's call id, so nine calls of the same tool with the same input - a model
    /// retrying - are one identity; and a call has no ancestors, so "fewest unmatched ancestors" ties
    /// across all nine. What distinguishes them is which result follows, and the replay says which result
    /// is next.
    ///
    /// A heuristic on the order of exploration only: what is permitted is unchanged, so a shape it guesses
    /// wrong is still found by backtracking.
    fn lookahead_cost(
        &self,
        replay: &[(crate::sideml::types::ChatRole, &str)],
        trace: usize,
        position: usize,
    ) -> u8 {
        let Some(&(next_role, next_hash)) = replay.get(self.chosen.len() + 1) else {
            return 0; // nothing follows, so nothing to line up with
        };
        let identities = &self.state.identities[trace];
        let lines_up = self.state.relations[trace]
            .successors_of(position)
            .iter()
            .any(|&successor| {
                identities
                    .get(successor as usize)
                    .is_some_and(|(role, hash)| *role == next_role && hash == next_hash)
            });
        u8::from(!lines_up)
    }

    /// How many of this occurrence's ancestors the replay has not matched yet.
    ///
    /// Zero means everything it depends on is already accounted for, which is what makes it the right
    /// candidate among interchangeable ones - see the ordering in `extend`.
    fn unmatched_ancestors(&self, trace: usize, position: usize) -> usize {
        let mut ancestors: HashSet<u32> = HashSet::new();
        self.state.relations[trace].collect_ancestors(position, &mut ancestors);
        ancestors
            .iter()
            .filter(|&&node| {
                // An ancestor counts as unmatched when no chosen entry is that occurrence.
                !self.chosen.iter().any(|&chosen| {
                    let occurrence = &self.state.entries[chosen];
                    occurrence.trace == trace && occurrence.position as u32 == node
                })
            })
            .count()
    }

    /// Record everything that must precede this occurrence, returning what was newly added so the
    /// caller can undo it.
    fn add_ancestors(&mut self, trace: usize, position: usize) -> Vec<u32> {
        let mut reached: HashSet<u32> = HashSet::new();
        self.state.relations[trace].collect_ancestors(position, &mut reached);
        let known = self.must_precede.entry(trace).or_default();
        let mut added = Vec::new();
        for node in reached {
            if known.insert(node) {
                added.push(node);
            }
        }
        added
    }
}
